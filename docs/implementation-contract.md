# Implementation Contract

This document fixes the implementation defaults for v1. Product policy remains
in the architecture and state-model documents; this file defines the concrete
API, storage, hook, runtime, and test contracts that implement that policy.

Implementation details may change without a new product decision only when they
do not weaken enforcement, change user-facing policy, or change committed config
semantics.

## Implementation Stack

V1 is Rust-only.

The state server, pure policy primitives, store-backed authorization service,
SQLite persistence, CLI, hook adapter commands, MCP adapter, sandbox wrapper,
and implemented prompt/context endpoints live in the same Rust codebase.
Watcher-driven human observation and richer store-backed prompt rendering remain
design targets unless a section below says they are implemented.

The prototype supports user-level installation with repo allowlist gating.
`stateful install --yes` installs stateful global files only. `stateful install
--agent codex --yes` configures global Codex hooks and MCP, and writes
`skills/stateful-command-policy/SKILL.md` and
`skills/dispatching-parallel-agents/SKILL.md`. `stateful install
--agent omp --yes` configures the isolated OMP `stateful` profile with stateful
hooks, MCP, `sandbox_bash` for non-external sandbox profiles, `ext_ro_bash`
for read-only `--fs external`, `ext_rw_bash` for scoped-grant external writes,
`lazy_edit_resume` for strict replay of queued line-based OMP edits, and approval
entries that deny raw Bash while setting Python/JavaScript/JS/Ruby/Julia eval tools to false. The OMP installer also
writes `rules/stateful-required.md`,
`skills/stateful-command-policy/SKILL.md`, and
`skills/dispatching-parallel-agents/SKILL.md` under that isolated agent
directory: the always-apply rule owns model-facing activation, the
`stateful-command-policy` manual owns detailed procedure, and hooks remain the
boundary. `stateful enable` opts the
current repo into enforcement.
For OMP, the extension prefers the actual OMP runtime session id from
`event.sessionId` or `ctx.sessionManager.session.id`, stores it in
`process.env.STATEFUL_SESSION_ID`, and `stateful hook omp session-start`
persists current-session files so session-aware MCP tools resolve the same
session.

Hook adapters should invoke the compiled `stateful` binary instead of embedding
policy or adapter logic in separate scripts. Hook configuration may reference
the binary directly by absolute path or by PATH lookup.

This keeps parsing, authorization, rendering, sandboxing, and outbox behavior in
one implementation and avoids policy drift.

## Protocol Version

The `stateful.v1` envelope is the target request shape for side-effecting
protocol calls. The current implementation enforces it for `/v1/authorize` and
the reservation declare/request/claim/cancel endpoints. Session, claim, activity,
conflicts/check, context, reconciliation, notification, resume, outbox, and read
endpoints still use their current flat request bodies.

Envelope-shaped requests include:

```text
protocol_version: stateful.v1
request_id
session
workspace
source
payload
```

`protocol_version` uses `name.major` form. A major mismatch fails closed on
endpoints that currently enforce the envelope. Clients should not assume every
endpoint accepts or requires the envelope until those routes are migrated.

## Common Request Envelope

```text
protocol_version
request_id
observed_at
session:
  session_id
  turn_id
  actor_id
  actor_type: agent | subagent | human | system
  owner_id
  parent_session_id
  parent_actor_id
workspace:
  root
  workspace_id
  repo_id
  worktree_id
  branch
source:
  kind: hook | mcp | cli | watcher | ide | server
  event
  tool_name
  source_ref
payload:
  endpoint-specific object
```

Adapters may omit unknown nullable identity fields, but the server must record
which fields were absent. Missing identity can lower confidence or reduce a
soft-conflict check, but it cannot create a stronger denial than the policy
allows.

## HTTP API

The state server exposes local HTTP under `/v1`.

```text
GET  /health
GET  /v1/current
GET  /v1/events
POST /v1/session/register
POST /v1/session/heartbeat
POST /v1/reservation/declare
POST /v1/reservation/request
POST /v1/reservation/claim
POST /v1/reservation/cancel
POST /v1/claim/acquire
POST /v1/claim/refresh-observation
POST /v1/claim/release
POST /v1/activity/observe
POST /v1/activity/finalize
POST /v1/authorize
POST /v1/conflicts/check
POST /v1/context/render
POST /v1/reconcile/ack
POST /v1/notifications/poll
GET  /v1/notifications/stream
POST /v1/resume/next
POST /v1/outbox/sync
GET  /v1/runtime/identity
```

`/v1/authorize` is the single policy entry point for supported tool actions.
`/v1/conflicts/check` is a read-only dry-run wrapper around the same policy
engine and must not create claims or write-authorizing state.
`/v1/notifications/poll` returns pending coordination notifications for a
session and marks returned notifications as delivered so a later poll does not
redeliver the same notification. Reservation notification payloads include the
stored, non-empty reservation purpose. `/v1/resume/next` is the durable recovery
path: it returns the first claimable reservation for the session, including its
stored purpose, after rereading the target, even if the reservation notification
was already delivered or the client missed the poll response. `/v1/reservation/claim`
is the explicit reservation claim path; it takes a `wait_id`, uses the stored
reservation purpose, and creates active reservation scope plus an active
claim for the reservation owner. `/v1/authorize` may lazy-claim a claimable
reservation for hook and sandbox authorization sources after the client rereads
and retries the write boundary; read-only conflict checks must not claim reservations.
`/v1/claim/acquire` records the target existence and content hash when `root` is
supplied; hook-originated native file writes compare that observation before
authorization. `/v1/claim/refresh-observation` refreshes the same-session exact
file claim observation while a claim remains active. Completed native edit and
`write-targets` hook flows release their authorizing claim instead of carrying
it forward, so later writes must reread and acquire a fresh claim or claim a
claimable reservation. `/v1/reservation/request`
creates or returns an idempotent queued or claimable (`reserved`) request by
`request_id`; `/v1/reservation/cancel` cancels queued or claimable (`reserved`)
requests owned by the caller.
`/v1/runtime/identity` is an authenticated server identity endpoint used by
`stateful server stop` to verify that the runtime file and process id describe
the same stateful server.

MCP tools map directly onto these endpoints. MCP handlers do not implement
policy branches; they validate tool arguments, resolve the current session from
explicit arguments, `STATEFUL_SESSION_ID`, or hook-persisted current-session
files as appropriate, call the HTTP API, and return the server result. The OMP
current-session path supports `state_session_register` ->
`state_reservation_declare` -> `state_claim_acquire` without a caller-supplied env
override after `stateful hook omp session-start` has persisted the active
session.

Current envelope enforcement is limited to `/v1/authorize` and
`/v1/reservation/declare`, `/v1/reservation/request`, `/v1/reservation/claim`, and
`/v1/reservation/cancel`. Reservation declare requires non-empty `purpose` and non-empty
`files_planned`; empty arrays and empty or normalized-empty paths fail with
`missing_scope`. `files_planned` is a task-level target set and may contain every
known file or directory the task expects to mutate; clients redeclare when that
set grows. Reservation request requires non-empty `purpose` and a non-empty
`path`; empty or normalized-empty request paths fail with `missing_scope`.
Reservation claim clients provide a `wait_id` only; they must not provide a purpose
because the server uses the stored reservation purpose.

`state_claim_acquire` accepts `paths: string[]` and creates one active exact file
or directory claim per path, after normalizing each entry against active
reservation scope. Legacy server requests with a single `path` remain accepted
for compatibility. `state_claim_release` still accepts `path: string` and
releases one exact resource claim per call.

## Authorization Input

```text
action:
  write_file | write_directory
  delete_file | rename_file | move_file
path
old_path: required for rename_file | move_file
new_path: required for rename_file | move_file
queue_on_conflict: bool
purpose: required when queue_on_conflict is true
base_observations:
  - path
    exists
    content_hash
```

The richer `targets[]` policy model, read/search/diff actions,
command/override-instruction authorization, and reconciliation actions are target
model vocabulary, not accepted `/v1/authorize` payload fields in the shipped v1
server. Unknown or unsupported actions fail closed with `unsupported_action`.
The shipped `/v1/reservation/request` scheduling API accepts only `write_file` and
`write_directory` requests with one `path`. Opportunistic `queue_on_conflict`
from `/v1/authorize` does not queue `rename_file` or `move_file`, because those
actions affect multiple paths and need the target all-or-nothing scheduler.

Native edit tools with hook-visible targets, such as Codex `apply_patch`,
`Edit`, and `Write` or OMP `edit` and `write`, expose targets to hooks. After a
task-level reservation covers the target and a successful same-session file
claim is active, hooks call `/v1/authorize` with the operation-specific action
before allowing the edit,
including `write_file`, `delete_file`, and `move_file` with source `path` /
`old_path` and destination `new_path`. PreToolUse authorization sends current
`base_observations` for each affected target when the hook can read the
workspace file state. PostToolUse observes completed native edits and sandbox
`write-targets` transactions, records the result, and releases the same-session
claims that authorized the completed write boundary. Released claims leave the
live context render and do not authorize a later write; the session must reread
and reacquire a claim, or lazy-claim a claimable reservation, before retrying.
OMP `edit` denials are captured by the generated extension as live-session lazy
edit operations when the patch has safe repo-relative line targets. Denials with
a wait id reuse that id; missing reservation or missing claim denials receive a
generated live-session operation id. `lazy_edit_resume` re-authorizes the original
edit after the agent fixes the missing scope or receives a claimable reservation,
verifies the file content still matches the queued base text, and applies only
line-based edit patch operations; block operations or changed files require
regenerating the patch.
For Bash, command text alone never authorizes tool use.
`/v1/authorize` accepts optional `base_observations` for OCC-style freshness
checks. When supplied, each observation is compared against the current
workspace file state under `workspace.root`; existence or `content_hash` changes
for an affected target return `deny` with `reason_code:
stale_target_observation` and require the caller to reread before retrying.
Hook adapters normalize namespaced runtime tool names to their leaf before
policy classification: `functions.bash` follows Bash rules,
`functions.python` / `functions.javascript` / `functions.js` /
`functions.ruby` / `functions.julia` follow eval-tool rules, and
`functions.read` / `functions.search` follow native read/search rules.
Codex raw Bash is denied by stateful hooks with sandbox guidance. Bash hook
calls for repo-internal shell work are allowed only when the outer command is a
single strict invocation of the trusted absolute `stateful` binary running
`<absolute-stateful-binary> sandbox run ... --command <cmd>`. Ordinary read work
should use agent-native read/search/diff tools when available. Read-only
command-shaped inspection uses `<absolute-stateful-binary> sandbox run --fs
read-only --network disabled --command <cmd>`; the read-only profile rejects
`--network enabled`. Command-shaped repo writes use `--fs write-targets` with
explicit `--write-target <file>` / `--create-target <file>` values and target authorization.
OMP raw Bash and Python/JavaScript/JS/Ruby/Julia eval-tool execution are denied
at host approval and hook levels, even when the raw command itself invokes
`stateful sandbox run`. OMP
sandbox command execution uses generated custom tools: `sandbox_bash` invokes
the trusted stateful binary for read-only, write-targets, build, git, and
github-pr profiles, including common sandbox flags, and rejects `--fs external`
with guidance to use `ext_ro_bash` or `ext_rw_bash`; `ext_ro_bash` starts
purpose-and-command-only external reads without OMP UI confirmation; `ext_rw_bash`
asks OMP UI confirmation for a scoped purpose grant before starting the trusted
stateful binary with `sandbox run --fs external --purpose ...` for external
writes that declare at least one write target, create target, or write dir. The
grant prompt shows purpose, declared scope, examples, max uses, and expiry rather
than raw command text, and matching calls reuse the grant until it expires or
reaches its use limit. All generated `*_bash` tools run
sandbox commands in the background by default. With `async` omitted or `true`,
they return a job id immediately, stream stdout/output via OMP messages using
`pi.sendMessage`, and send final stdout/stderr/exit status as a follow-up
message. Set `async: false` to keep the old awaited foreground behavior with
final stdout/stderr/status in returned tool details.
The external sandbox profile requires
purpose and command; read-only/no-declared-scope operations may
omit targets, while supplied external write scopes are validated as absolute
paths outside the repo. On macOS, external profile runs also allow
trust/identity Mach lookups for `trustd` and DirectoryService so Go TLS clients
such as `gh` can verify certificates. It starts through the sandbox after Codex
approval, `ext_ro_bash` invocation, or `ext_rw_bash` scoped-grant approval and does not
require repo reservation or claim unless repo-relative write scope is supplied.
Local git operations use `<absolute-stateful-binary> sandbox run --fs git
--network disabled --command 'git <args>'`; use `--network enabled` only for
remote git operations. GitHub pull request list/view/status/create commands use
`<absolute-stateful-binary> sandbox run --fs github-pr --network enabled --command
'gh pr <list|view|status|create> ...'`; in OMP, call `sandbox_bash`. Use the
GitHub connector instead when that connector is explicitly allowlisted for the repo.

## Decision Output

Policy decisions use a common core shape. Endpoints may add scheduling fields
such as `wait`, `reservation`, `current`, `items`, or `prompt_text`, and shipped
responses may omit arrays that do not apply instead of returning empty arrays:

```text
decision: allow | deny | error
reason_code
message
conflicts[]
required_next_action
context_items[]
audit_event
```

`deny` means the adapter must block the action. `error` is used for
state-server, protocol, or sandbox execution failures; write and reconciliation
paths treat `error` as fail-closed. A `warn` decision and response-level
`conflicts[]`, `context_items[]`, or `audit_event` fields are target response
vocabulary. The shipped `/v1/authorize` response returns allow/deny/error plus
`wait` or `reservation` details when applicable, and it appends
`AuthorizationDenied` events for deny decisions.

For scheduling APIs, a hard conflict decision may produce a queued reservation
request instead of an immediate reservation. V1 queues only hard
conflicts in the same `workspace_id` and normalized `relative_path`. Soft
repo-relative conflicts remain warning context because they signal future
integration risk rather than immediate physical file overwrite.

Queued requests are promoted FIFO. The shipped queue stores one requested path
per wait request, and a request is reservable only when that requested resource
is available. Promotion is triggered by explicit claim release, session or
activity finalization, claim/reservation expiry, or current-state materialization
that finds an already-unblocked queued waiter. Promotion creates short claimable
reservations and pending notifications whose payloads carry the waiting row's
stored, non-empty purpose.

Promotion creates claimable reservations first. A claimable reservation is not
active write authority. Each waiting session must reread the target. Manual
MCP/CLI flows then explicitly claim with `state_reservation_claim` or
`stateful reservation claim --wait-id <id>`; claim uses the stored reservation
purpose and clients do not provide a claim purpose. Hook and sandbox
authorization sources may lazy-claim the claimable reservation at the retried
write boundary. Claiming creates active reservation scope and active
same-session claims. The default claimable reservation TTL is 120 seconds; the
default claim TTL is 300 seconds and is refreshed by heartbeat.

The target multi-resource model is atomic all-or-nothing: a multi-resource
request is reservable only when it is the head entry for every requested resource
queue and none of those resources has an active claim, and the server must not
partially reserve it. `request_id` is the idempotency key for shipped single-path
requests: repeating a request with the same id returns the existing state and
must not enqueue a duplicate. Repeating an expired request requeues the same
waiter in place, preserving its original FIFO row while requiring a new
reservation and claim before writing.

Full scheduling APIs return immediately with request state. Blocking waits can
be implemented as a future client convenience by polling notifications or resume
endpoints. Queued and claimable (`reserved`) request cancellation is explicit
through `reservation/cancel`; shipped session or activity finalization also
cancels that session's queued and claimable (`reserved`) requests. Explicit user
overrides do not reorder the wait queue or transfer reservations.

## SQLite Storage

V1 uses one SQLite database. In global server mode, the runtime database lives
under the user-level `GlobalPaths` home:

```text
$STATEFUL_HOME/state.db
$HOME/.stateful_core/state.db
```

Repo-local runtime state remains available as a compatibility fallback:

```text
.stateful_core/state.db
```

Minimum tables:

```text
schema_migrations
events
sessions
activities
reservations
claims
wait_queue
notifications
conflicts
overrides
reconciliations
human_observations
outbox
```

`events` is append-only audit history for shipped coordination events. Accepted
session and reservation declaration events materialize current-state rows in the same
transaction as the event append. Lifecycle mutation APIs such as claim release,
reservation claim, reservation cancel, and activity finalize update materialized tables
directly and append their audit events in the same transaction. If the audit
append or event-backed materialization fails, the surrounding mutation rolls
back.

Required indexes:

```text
events(workspace_id, created_at)
events(session_id, created_at)
sessions(workspace_id, session_id)
activities(workspace_id, expires_at)
reservations(session_id, status, expires_at)
claims(workspace_id, relative_path, status)
claims(repo_id, relative_path, status, expires_at)
wait_queue(workspace_id, relative_path, status)
wait_queue(session_id, status)
notifications(target_session_id, status)
conflicts(session_id, checked_at)
reconciliations(session_id, created_at)
outbox(session_id, sequence, sync_status)
```

The shipped schema may retain legacy or target-model columns and indexes such
as `events.sequence` and `claims.absolute_path`; current v1 authorization and
event queries must not rely on them until those columns are populated.

`SessionHeartbeat` materialization refreshes the session timestamp, active claim
expiry, active activity expiry, and active reservation expiry. Reservation refresh is
capped at 60 minutes from `declared_at`.

The shipped state server runs a background expiration loop while serving and
also triggers expiration lazily from read/write paths. Expiration covers stale
claims, stale reservations, and stale reservation state, promotes eligible FIFO
waiters, and must be transactionally equivalent in both paths.

The same maintenance loop prunes old historical evidence after the built-in
14-day retention window. Pruning deletes old events, reconciliations, conflicts,
human observations, and expired notifications. It must not delete active
current-state rows, pending notifications, or outbox sync evidence.

## Runtime Files

Committed project config lives under:

```text
.stateful/config.yml
```

User-level runtime state for the global server lives under `GlobalPaths`:

```text
$STATEFUL_HOME/
$STATEFUL_HOME/runtime/server.json
$STATEFUL_HOME/outbox/
$STATEFUL_HOME/state.db
```

When `STATEFUL_HOME` is not set, the default global home is
`$HOME/.stateful_core/`. Repo-local compatibility runtime state keeps only
runtime discovery and legacy database artifacts:

```text
.stateful_core/
.stateful_core/runtime/server.json
.stateful_core/state.db
```

`.stateful_core/` is ignored by git. Hook adapters discover the server in this
order:

```text
STATEFUL_SERVER_URL and STATEFUL_SERVER_TOKEN environment variables
global runtime server.json under GlobalPaths
repo-local .stateful_core/runtime/server.json compatibility fallback
```

`server.json` contains `base_url`, `token`, `pid`, `workspace_id`,
`protocol_version`, and `started_at`. Runtime discovery files are written with
user read/write permissions (`0600`) on Unix platforms where file modes are
available.
When the runtime workspace id is a default placeholder (`local`, `shared`, or
`unknown`), enabled-repo hooks, MCP calls, and CLI fallbacks derive the effective
request workspace id from the enabled repo's canonical git root
(`workspace-...`) so two different enabled repos do not share one conflict
domain. Explicit non-default runtime or command `--workspace-id` values are
preserved.

`stateful server start` starts the HTTP runtime and prints
`stateful server join ...` commands. Binding to `0.0.0.0` makes the runtime
LAN-reachable, but printed join commands target loopback so remote machines use
an SSH tunnel before joining. `stateful server join` rejects non-loopback plain
`http://` base URLs before runtime validation or config writes, validates the
host runtime, installs global stateful/Codex MCP configuration, writes global
runtime discovery for the host server, and only enables the current repo when
`--enable-repo` is supplied.

## Local HTTP Trust

The server binds to `127.0.0.1` by default. Requests, except `/health`, must
include the workspace bearer token from runtime discovery.

The token is a local trust guard, not a hard security boundary. It prevents
casual spoofing by unrelated local processes but does not replace OS-level
process isolation or managed hooks.

CLI join never sends the bearer token to a non-loopback plain `http://` base
URL. Use an SSH tunnel and join the loopback endpoint for remote runtimes.

If the token is missing or invalid, write, reconciliation, reservation, and claim
paths fail closed. Read-only paths may return a minimal unauthorized error.

## Hook Adapter Contract

Hook scripts are thin integration adapters:

```text
parse hook input
derive session/workspace/source envelope
classify runtime-specific tool calls
extract action and targets when supported
call /v1/authorize or the relevant endpoint for store-backed policy
deny unclassified write/execute-capable tools in enabled repos
translate decision into hook output
append outbox evidence when observation cannot reach the server
```

Adapter-local policy is limited to fail-closed tool classification and trusted
wrapper validation for command-shaped execution. Conflict, claim, reservation,
freshness, queue, and reconciliation decisions belong to the state server.

Suggested hook timeouts:

```text
authorization: 750 ms
context render: 1500 ms
observation: 500 ms
outbox append: local filesystem only
```

Timeouts follow the availability policy. A timed-out write authorization is
denied. A timed-out observation is queued to the local outbox when possible. A
timed-out context render should show a concise state-unavailable warning rather
than raw error output.

## Global Outbox

When the state server is unavailable, hook and observer events that are allowed
to be queued are appended under `$STATEFUL_HOME/outbox/`.

The file format is newline-delimited JSON. Each line carries:

```text
outbox_id
event_type
session_id
actor_id
workspace_id
sequence
created_at
payload
sync_status: pending
```

`outbox_id` is the idempotency key. The server must treat repeated sync attempts
for the same `outbox_id` as the same event. Sync preserves sequence order per
session.

The outbox cannot authorize writes, clear reconciliation blocks, or extend
claims while the state server is unavailable.

## CLI Surface

The implementation should include a small CLI for setup and debugging:

```text
stateful install [--yes]
stateful enable [--repo <path>]
stateful disable [--repo <path>]
stateful repos list
stateful server
stateful server start [--foreground] [--host <host>] [--port <port>] [--token <token>] [--workspace-id <id>]
stateful server restart
stateful server stop
stateful server status
stateful server join <base-url> --token <token> [--workspace-id <id>] [--enable-repo]
stateful status
stateful current
stateful events
stateful doctor
stateful sandbox run --fs read-only|write-targets|build|git|github-pr ...
stateful sandbox process find <selector>
stateful reservation declare [--session-id <id>] [--workspace-id <id>] --purpose <purpose> <paths...>
stateful notifications poll [--session-id <id>] [--workspace-id <id>]
stateful resume next [--session-id <id>] [--workspace-id <id>]
stateful mcp call <tool> [arguments-json]
stateful mcp serve
stateful hook <codex|omp> <event>
stateful sync-outbox
```

`stateful install --agent codex --yes` configures global Codex hooks, MCP, MCP
tool approval policy, and external sandbox approval rules. Codex installs wire
`SessionStart`, `UserPromptSubmit`, `PreToolUse`, `PostToolUse`, and `Stop`.
Stateful MCP tools default to automatic approval; `stateful sandbox run --fs
external --purpose ... --command ...` is gated by a Codex execpolicy prompt
before it runs the external sandbox command. Purpose-and-command-only operations
are allowed for read-only/no-declared-scope use; supplied external write scopes
are validated before the sandbox starts. On macOS, that external profile also
permits `trustd` and DirectoryService Mach lookups needed by Go TLS certificate
verification.
`stateful
install --agent omp --yes` registers `sandbox_bash` for read-only,
write-targets, build, git, and github-pr sandbox profiles, `ext_ro_bash` for
read-only external commands, and `ext_rw_bash` for external writes with scoped
purpose grants. All three generated `*_bash` tools run sandbox commands in the
background by default: when `async` is omitted or `true`, the call returns a job
id immediately, streams stdout/output via OMP messages using `pi.sendMessage`,
and sends final stdout/stderr/exit status as a follow-up message. `async: false`
waits for completion and returns final stdout/stderr/exit status in tool
details.
`ext_ro_bash` does not ask OMP UI confirmation; `ext_rw_bash` asks for the grant
before starting the trusted stateful binary with the external profile. Raw OMP Bash and
Python/JavaScript/JS/Ruby/Julia eval-tool sandbox invocations are denied.
OMP `session_start`, `tool_call`, `tool_result`, and `session_shutdown`
extension events to `stateful hook omp session-start`, `pre-tool-use`,
`post-tool-use`, and `stop`; OMP does not expose a stateful
`user-prompt-submit` hook. `stateful enable` opts a repo into enforcement.
`stateful server start`
without `--foreground` uses the detached lazy lifecycle. Bare legacy
`stateful server` and `stateful server start --foreground` run in the
foreground and write runtime discovery. `stateful doctor` checks current Codex
config, repo config files, global install fields, repo enabled status, and
global path or registry errors. Legacy `.codex/hooks.json` and repo-local
`.stateful_core/state.db` artifacts are reported as legacy artifacts, not as
installed-state evidence. Active server reachability, config schema validation
and SQLite migration inspection are future doctor extensions.

## Verification

Before publishing or releasing a build, run:

```text
cargo fmt --all --check
env -u STATEFUL_CODEX_RUN_ID -u CODEX_THREAD_ID cargo test --workspace
```

Unset `STATEFUL_CODEX_RUN_ID` and `CODEX_THREAD_ID` when running workspace tests
from an active Codex session so tests do not inherit a run-bound session file
from the caller.
`stateful doctor` remains the local installation health check after installing
or enabling a repository.

## Config Defaults

`.stateful/config.yml` documents target repo-level defaults:

```text
# stateful-core repo policy config
# These are informational target defaults for this repository.
# Runtime loading of these keys is not yet shipped.
protocol_version: stateful.v1
intent_ttl_seconds: 900
intent_max_seconds: 3600
claim_ttl_seconds: 300
reservation_ttl_seconds: 120
directory_scope_depth: 2
delete_requires_exact_file_scope: true
rename_requires_exact_file_scope: true
default_write_policy: deny
event_retention_days: 14
```

The current implementation writes these keys as target repo-level defaults but
does not load them at runtime. Reservation, claim, reservation, directory-scope, and
retention windows are built-in Rust constants today. The shipped server uses
the built-in 14-day retention window for historical pruning; configurable
runtime loading is future hardening work.

Command-shaped tests and checks run through the trusted `stateful sandbox run`
wrapper. Build and test artifact writes are scoped with
`--fs build --network enabled --write-dir <scratch-purpose>`. The profile writes
disposable artifacts under `/tmp/stateful/<session>/<scratch-purpose>/`, sets
standard temp variables under that scratch root, and sets `CARGO_TARGET_DIR` to its `target` child.

Glob semantics should use gitignore-style path matching relative to the
workspace root. Identity and policy checks still use normalized canonical paths
after glob expansion.

## Test Contract

Implemented v1 behavior must have tests for:

- policy decisions as table-driven unit tests
- reservation scope matching, including depth-2 directory behavior
- directory claim conflict behavior, including whole-subtree fencing for
  `write_directory` command-shaped writes
- exact file scope for delete, rename, and move
- Bash full-deny classification and sandbox-gated hook authorization
- nested Codex benchmark sandbox hook authorization only when the
  `codex-benchmark` cargo feature is enabled
- native edit hook authorization plus Bash sandbox fixtures
- prompt renderer golden output for shipped store-backed rendering
- heartbeat refresh of activities, capped active reservation TTL, and active claims
  still covered by active reservation
- background and lazy expiration of stale claims, active reservations,
  and claimable reservations, including FIFO waiter promotion after stale reservation expiry
- missing purpose and empty `files_planned` rejection
- SQLite event append plus materialized-view transaction behavior
- sandbox-run execution with artifact writes limited to an authorized directory
- state-server unavailable behavior
- outbox idempotent sync
- human-write reconciliation blocks and acknowledgements when human-write
  observation is shipped

Pure scope-policy primitives should be testable without starting the HTTP
server. The shipped store-backed authorization service is currently tested
through server-level tests; future extractions should move reusable policy
pieces into `stateful-core` without duplicating product policy in adapters.

## Migration Path

The prototype supports user-level installation with repo allowlist gating.
`stateful install --yes` installs stateful global files only. `stateful install
--agent codex --yes` configures global Codex hooks and MCP, and writes
`skills/stateful-command-policy/SKILL.md` and
`skills/dispatching-parallel-agents/SKILL.md`. `stateful install
--agent omp --yes` installs the OMP extension entry point, MCP config,
always-apply `rules/stateful-required.md` rule,
`skills/stateful-command-policy/SKILL.md` manual,
`skills/dispatching-parallel-agents/SKILL.md` skill, and OMP config under the
`stateful` profile agent directory (`~/.omp/profiles/stateful/agent`) with
`tools.approvalMode: yolo`, `bash.enabled: false`, `eval.py: false`,
`eval.js: false`, `eval.rb: false`, and `eval.jl: false`; it removes
`tools.approval` from the stateful profile because yolo mode delegates safety to
Stateful hooks. Raw Bash and Python/JavaScript/JS/Ruby/Julia eval-tool execution
is still denied at host approval and hook levels, sandbox runs still go through
`sandbox_bash`, `ext_ro_bash`, or `ext_rw_bash`, and the trusted external write
grant prompt stays inside `ext_rw_bash`. `stateful enable`
opts the current repo into enforcement. Repo-local packaging and managed hooks
must reuse the same hook adapter library and HTTP protocol.

The migration order is:

```text
user-level install with repo allowlist gating
repo-local hooks compatibility fallback
plugin packaging for team beta
managed hooks for organization enforcement
IDE extension for human save-gate signals
```

No migration step may introduce a separate adapter-local policy engine. New
clients must call the same state server API.
