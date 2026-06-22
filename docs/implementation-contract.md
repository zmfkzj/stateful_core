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
--agent codex --yes` configures global Codex hooks and MCP. `stateful install
--agent omp --yes` configures the isolated OMP `stateful` profile with stateful
hooks and MCP. `stateful enable` opts the current repo into enforcement.
Repo-local hooks remain available through `stateful enable --repo-local-codex`
as a compatibility fallback.

Hook adapters should invoke the compiled `stateful` binary instead of embedding
policy or adapter logic in separate scripts. Hook configuration may reference
the binary directly by absolute path or by PATH lookup.

This keeps parsing, authorization, rendering, sandboxing, and outbox behavior in
one implementation and avoids policy drift.

## Protocol Version

The `stateful.v1` envelope is the target request shape for side-effecting
protocol calls. The current implementation enforces it for `/v1/authorize` and
the intent declare/request/claim/cancel endpoints. Session, lease, activity,
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
POST /v1/intent/declare
POST /v1/intent/request
POST /v1/intent/claim
POST /v1/intent/cancel
POST /v1/lease/acquire
POST /v1/lease/refresh-observation
POST /v1/lease/release
POST /v1/activity/observe
POST /v1/activity/finalize
POST /v1/authorize
POST /v1/conflicts/check
POST /v1/context/render
POST /v1/reconcile/ack
POST /v1/notifications/poll
POST /v1/resume/next
POST /v1/outbox/sync
GET  /v1/runtime/identity
```

`/v1/authorize` is the single policy entry point for supported tool actions.
`/v1/conflicts/check` is a read-only dry-run wrapper around the same policy
engine and must not create leases or write-authorizing state.
`/v1/notifications/poll` returns pending coordination notifications for a
session and marks returned notifications as delivered so a later poll does not
redeliver the same notification. `/v1/resume/next` is the durable recovery path:
it returns the first active reservation that the session can claim after
rereading the target, even if the reservation notification was already delivered
or the client missed the poll response. `/v1/intent/claim` is the explicit
reservation claim path; it creates write-authorizing intent and an active lease
for the reservation owner. `/v1/authorize` may lazy-claim an active reservation
for hook and sandbox authorization sources after the client rereads and retries
the write boundary; read-only conflict checks must not claim reservations.
`/v1/lease/acquire` records the target existence and content hash when `root` is
supplied; hook-originated native file writes compare that observation before
authorization. `/v1/lease/refresh-observation` refreshes the same-session exact
file lease observation while a lease remains active. Completed native edit and
`write-targets` hook flows release their authorizing lease instead of carrying
it forward, so later writes must reread and acquire a fresh lease or claim an
eligible reservation. `/v1/intent/request`
creates or returns an idempotent queued or reserved request by `request_id`;
`/v1/intent/cancel` cancels queued or reserved requests owned by the caller.
`/v1/runtime/identity` is an authenticated server identity endpoint used by
`stateful server stop` to verify that the runtime file and process id describe
the same stateful server.

MCP tools map directly onto these endpoints. MCP handlers do not implement
policy branches; they validate tool arguments, call the HTTP API, and return the
server result.

Current envelope enforcement is limited to `/v1/authorize` and
`/v1/intent/declare`, `/v1/intent/request`, `/v1/intent/claim`, and
`/v1/intent/cancel`. Intent declare requires non-empty `purpose` and non-empty
`files_planned`; empty arrays and empty or normalized-empty paths fail with
`missing_scope`. Intent request requires non-empty `purpose` and a non-empty
`path`; empty or normalized-empty request paths fail with `missing_scope`.

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
The shipped `/v1/intent/request` scheduling API accepts only `write_file` and
`write_directory` requests with one `path`. Opportunistic `queue_on_conflict`
from `/v1/authorize` does not queue `rename_file` or `move_file`, because those
actions affect multiple paths and need the target all-or-nothing scheduler.

Native edit tools with hook-visible targets, such as Codex `apply_patch`,
`Edit`, and `Write` or OMP `edit` and `write`, expose targets to hooks. After
exact intent and a successful same-session file lease, hooks call
`/v1/authorize` with the operation-specific action before allowing the edit,
including `write_file`, `delete_file`, and `move_file` with source `path` /
`old_path` and destination `new_path`. PreToolUse authorization sends current
`base_observations` for each affected target when the hook can read the
workspace file state. PostToolUse observes completed native edits and sandbox
`write-targets` transactions, records the result, and releases the same-session
leases that authorized the completed write boundary. Released leases leave the
live context render and do not authorize a later write; the session must reread
and reacquire a lease, or lazy-claim an eligible reservation, before retrying.
For Bash, command text alone never authorizes tool use.
`/v1/authorize` accepts optional `base_observations` for OCC-style freshness
checks. When supplied, each observation is compared against the current
workspace file state under `workspace.root`; existence or `content_hash` changes
for an affected target return `deny` with `reason_code:
stale_target_observation` and require the caller to reread before retrying.
Codex raw Bash is denied by stateful hooks with sandbox guidance. Bash hook
calls for repo-internal shell work are allowed only when the outer command is a
single strict invocation of the trusted absolute `stateful` binary running
`<absolute-stateful-binary> sandbox run ... --command <cmd>`. Ordinary read work
should use agent-native read/search/diff tools when available. Read-only
command-shaped inspection uses `<absolute-stateful-binary> sandbox run --fs
read-only --network disabled --command <cmd>`; the read-only profile rejects
`--network enabled`. Command-shaped repo writes use `--fs write-targets` with
explicit `--write-target` / `--create-target` values and target authorization.
OMP raw Bash follows the same block-unless-wrapper rule. Repo-external OMP Bash
is also blocked unless it uses the trusted wrapper:
`<absolute-stateful-binary> sandbox run --fs external --purpose ...`; the
external sandbox profile validates absolute external write scopes, rejects
repo-internal targets, runs through the sandbox, and does not require repo intent
or lease. Git operations use `--fs git` for one `git ...`
command. GitHub pull request list/view/status/create commands use
`<absolute-stateful-binary> sandbox run --fs github-pr --network enabled
--command 'gh pr <list|view|status|create> ...'`; use the GitHub connector
instead when that connector is explicitly allowlisted for the repo.

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

For scheduling APIs, a hard conflict decision may produce a queued intent
request instead of an immediate reservation. V1 queues only hard
conflicts in the same `workspace_id` and normalized `relative_path`. Soft
repo-relative conflicts remain warning context because they signal future
integration risk rather than immediate physical file overwrite.

Queued requests are promoted FIFO. The shipped queue stores one requested path
per wait request, and a request is reservable only when that requested resource
is available. Promotion is triggered by explicit lease release, session or
activity finalization, lease/reservation expiry, or current-state materialization
that finds an already-unblocked queued waiter. Promotion creates short
reservations and pending notifications for the waiting sessions.

Promotion creates reservations first. A reservation is not active write
authority. Each waiting session must reread the target. Manual MCP/CLI flows
then explicitly claim with `state_intent_claim` or
`stateful intent claim --wait-id <id>`. Hook and sandbox authorization sources
may lazy-claim the reservation at the retried write boundary. Claiming creates
write-authorizing intent and active same-session leases. The default reservation
TTL is 120 seconds; the default lease TTL is 300 seconds and is refreshed by
heartbeat.

The target multi-resource model is atomic all-or-nothing: a multi-resource
request is reservable only when it is the head entry for every requested resource
queue and none of those resources has an active lease, and the server must not
partially reserve it. `request_id` is the idempotency key for shipped single-path
requests: repeating a request with the same id returns the existing state and
must not enqueue a duplicate. Repeating an expired request requeues the same
waiter in place, preserving its original FIFO row while requiring a new
reservation and claim before writing.

Full scheduling APIs return immediately with request state. Blocking waits can
be implemented as a future client convenience by polling notifications or resume
endpoints. Queued and reserved request cancellation is explicit through
`intent/cancel`; shipped session or activity finalization also cancels that
session's queued and reserved requests. Explicit user overrides do not reorder
the wait queue or transfer reservations.

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
intents
leases
wait_queue
notifications
conflicts
overrides
reconciliations
human_observations
outbox
```

`events` is append-only audit history for shipped coordination events. Accepted
session and intent declaration events materialize current-state rows in the same
transaction as the event append. Lifecycle mutation APIs such as lease release,
intent claim, intent cancel, and activity finalize update materialized tables
directly and append their audit events in the same transaction. If the audit
append or event-backed materialization fails, the surrounding mutation rolls
back.

Required indexes:

```text
events(workspace_id, created_at)
events(session_id, created_at)
sessions(workspace_id, session_id)
activities(workspace_id, expires_at)
intents(session_id, status, expires_at)
leases(workspace_id, relative_path, status)
leases(repo_id, relative_path, status, expires_at)
wait_queue(workspace_id, relative_path, status)
wait_queue(session_id, status)
notifications(target_session_id, status)
conflicts(session_id, checked_at)
reconciliations(session_id, created_at)
outbox(session_id, sequence, sync_status)
```

The shipped schema may retain legacy or target-model columns and indexes such
as `events.sequence` and `leases.absolute_path`; current v1 authorization and
event queries must not rely on them until those columns are populated.

`SessionHeartbeat` materialization refreshes the session timestamp, active lease
expiry, active activity expiry, and active intent expiry. Intent refresh is
capped at 60 minutes from `declared_at`.

The shipped state server runs a background expiration loop while serving and
also triggers expiration lazily from read/write paths. Expiration covers stale
leases, stale reservations, and stale intent state, promotes eligible FIFO
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
`$HOME/.stateful_core/`. Repo-local compatibility runtime state lives under:

```text
.stateful_core/
.stateful_core/runtime/server.json
.stateful_core/outbox/
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

If the token is missing or invalid, write, reconciliation, intent, and lease
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
wrapper validation for command-shaped execution. Conflict, lease, intent,
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

## Local Outbox

When the state server is unavailable, hook and observer events that are allowed
to be queued are appended under `.stateful_core/outbox/`.

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
leases while the state server is unavailable.

## CLI Surface

The implementation should include a small CLI for setup and debugging:

```text
stateful init
stateful install [--yes]
stateful enable [--repo <path>] [--repo-local-codex]
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
stateful intent declare [--session-id <id>] [--workspace-id <id>] --purpose <purpose> <paths...>
stateful notifications poll [--session-id <id>] [--workspace-id <id>]
stateful resume next [--session-id <id>] [--workspace-id <id>]
stateful mcp call <tool> [arguments-json]
stateful mcp serve
stateful hook <codex|omp> <event>
stateful sync-outbox
stateful commit -m <message> -- <paths...>
stateful push [remote branch]
```

`stateful install --agent codex --yes` configures global Codex hooks, MCP, MCP
tool approval policy, and external sandbox approval rules. Codex installs wire
`SessionStart`, `UserPromptSubmit`, `PreToolUse`, `PostToolUse`, and `Stop`.
Stateful MCP tools default to automatic approval; `stateful sandbox run --fs
external --purpose ...` is gated by a Codex execpolicy prompt before it validates
the external write scope and runs the command through the sandbox. `stateful
install --agent omp --yes` wires the
OMP `session_start`, `tool_call`, `tool_result`, and `session_shutdown`
extension events to `stateful hook omp session-start`, `pre-tool-use`,
`post-tool-use`, and `stop`; OMP does not expose a stateful
`user-prompt-submit` hook. `stateful enable` opts a repo into enforcement and
can install repo-local Codex hooks with `--repo-local-codex` as a compatibility
fallback. `stateful server start`
without `--foreground` uses the detached lazy lifecycle. Bare legacy
`stateful server` and `stateful server start --foreground` run in the
foreground and write runtime discovery. `stateful doctor` checks current Codex
config, repo config files, global install fields, repo enabled status, and
global path or registry errors. Legacy `.codex/hooks.json` and repo-local
`.stateful_core/state.db` artifacts are reported as legacy artifacts, not as
installed-state evidence. Active server reachability, config schema validation
and SQLite migration inspection are future doctor extensions.
`stateful commit -m <message> -- <paths...>` is the structured commit wrapper.
`stateful push [remote branch]` is the structured push wrapper. It requires a
clean working tree, an attached current branch, either the current branch's
configured upstream or an explicit `<remote> <branch>` pair matching the current
branch, and rejects force-like target values. Raw `git add`, `git commit`, and
`git push` through Bash remain denied.

## Verification

Before publishing or releasing a build, run:

```text
cargo fmt --all --check
env -u STATEFUL_CODEX_RUN_ID -u CODEX_THREAD_ID cargo test --workspace
env -u STATEFUL_CODEX_RUN_ID -u CODEX_THREAD_ID cargo clippy --workspace --all-targets -- -D warnings
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
lease_ttl_seconds: 300
reservation_ttl_seconds: 120
directory_scope_depth: 2
delete_requires_exact_file_scope: true
rename_requires_exact_file_scope: true
default_write_policy: deny
event_retention_days: 14
```

The current implementation writes these keys as target repo-level defaults but
does not load them at runtime. Intent, lease, reservation, directory-scope, and
retention windows are built-in Rust constants today. The shipped server uses
the built-in 14-day retention window for historical pruning; configurable
runtime loading is future hardening work.

Command-shaped tests and checks run through the trusted `stateful sandbox run`
wrapper. Build and test artifact writes are scoped with `--fs build` after
`--write-dir <scratch-purpose>`. The profile writes disposable artifacts under
`/tmp/stateful/<session>/<scratch-purpose>/`, sets standard temp variables under
that scratch root, and sets `CARGO_TARGET_DIR` to its `target` child.

Glob semantics should use gitignore-style path matching relative to the
workspace root. Identity and policy checks still use normalized canonical paths
after glob expansion.

## Test Contract

Implemented v1 behavior must have tests for:

- policy decisions as table-driven unit tests
- intent scope matching, including depth-2 directory behavior
- directory lease conflict behavior, including whole-subtree fencing for
  `write_directory` command-shaped writes
- exact file scope for delete, rename, and move
- Bash full-deny classification and sandbox-gated hook authorization
- nested Codex benchmark sandbox hook authorization only when the
  `codex-benchmark` cargo feature is enabled
- native edit hook authorization plus Bash sandbox fixtures
- prompt renderer golden output for shipped store-backed rendering
- heartbeat refresh of activities, capped active intent TTL, and active leases
  still covered by active intent
- background and lazy expiration of stale leases, reservations, and intents,
  including FIFO waiter promotion after stale reservation expiry
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
--agent codex --yes` configures global Codex hooks and MCP. `stateful install
--agent omp --yes` installs the OMP extension entry point, MCP config, and
`tools.approvalMode: write` under the isolated OMP `stateful` profile agent
directory (`$STATEFUL_HOME/.omp/profiles/stateful/agent`, default
`~/.stateful_core/.omp/profiles/stateful/agent`) so that profile carries the
stateful approval context. `stateful enable` opts the current repo into
enforcement. Repo-local packaging and managed hooks must reuse the same hook
adapter library and HTTP protocol.

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
