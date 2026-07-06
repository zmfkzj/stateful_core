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
SQLite persistence, CLI, hook adapter commands, native Stateful tool handlers,
sandbox wrapper, human observation/reconciliation endpoints, advisory VS Code
save gate, and store-backed prompt/context endpoints live in the same Rust
codebase. Richer human observer coverage remains future adapter work unless a
section below says it is implemented.

The prototype supports user-level installation with repo allowlist gating.
`stateful install --yes` installs stateful global files only. `stateful install
--agent codex --yes` configures global Codex hooks and writes
`skills/stateful-command-policy/` (`SKILL.md`, `omp-tools.md`,
`sandbox-tools.md`, `denial-recovery.md`, `subagent-write-recovery.md`) and
`skills/dispatching-parallel-agents/SKILL.md`. `stateful install
--agent omp --yes` configures the isolated OMP `stateful` profile, or
`--agent omp --profile <name> --yes` configures `~/.omp/profiles/<name>/agent`, with stateful
hooks, native Stateful tool injection, built-in Bash preflight for strict
trusted `stateful sandbox run ...` and `stateful sandbox process find ...`
commands, and OMP native `edit`/`write` pre-tool handling that predeclares
exact simple repo file scope and acquires same-reservation claims before first
authorization when no explicit reservation id is supplied, can reacquire
same-reservation claims for the same active reservation, keeps auto-claims
across `stale_target_observation` reread/retry blocks, and retries authorization as needed,
`lazy_edit_resume` for strict replay of queued/conflicting line-based OMP edits,
`lazy_write_resume` for queued full OMP writes, both with captured-wait
notification/resume lookup before claiming and re-authorization, stale-target
guards, and `lazy_bash_resume` for queued external Bash commands waiting on
scoped grants, plus approval entries that deny arbitrary raw Bash while setting
Python/JavaScript/JS/Ruby/Julia eval tools to false. External
write/create/write-dir/socket/signal scope and repo-external OMP native
`edit`/`write` file targets auto-approve the scoped Stateful-owned OMP grant
prompt by default through `stateful.autoApprove: true`, while sandbox scope
validation, hooks, reservation/claim checks, and grant limits still apply.
The OMP installer also writes `rules/stateful-required.md` and
`skills/stateful-command-policy/` (`SKILL.md`, `omp-tools.md`,
`sandbox-tools.md`, `denial-recovery.md`, `subagent-write-recovery.md`) under
that isolated agent directory: the always-apply rule owns model-facing
activation, the `stateful-command-policy` manual owns detailed procedure, and
hooks remain the boundary. `stateful enable` opts the current repo into
enforcement.
For OMP, the extension/native tool bridge derives the active Stateful `agent_id`
from `ctx.sessionManager` and fails closed when it cannot provide a valid
session id; `docs/usage-reference.md#hooks-and-identity` is the canonical OMP
identity derivation.
Codex hooks use Codex's hook `session_id` parameter as the Stateful `agent_id`;
this is a hook payload parameter, not an environment-variable fallback.

Hook adapters should invoke the compiled `stateful` binary instead of embedding
policy or adapter logic in separate scripts. Hook configuration may reference
the binary directly by absolute path or by PATH lookup.

This keeps parsing, authorization, rendering, sandboxing, and outbox behavior in
one implementation and avoids policy drift.

## Protocol Version

The `stateful.v1` envelope is the target request shape for side-effecting
protocol calls. The current implementation enforces it for `/v1/authorize`,
`/v1/human/observe`, and the reservation declare/request/claim/cancel
endpoints. Session, claim, activity finalize, context, notification, resume,
outbox, and read endpoints still use their current flat request bodies.

Envelope-shaped requests include:

```text
protocol_version: stateful.v1
request_id
agent
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
agent:
  agent_id
  turn_id
  actor_id
  actor_type: agent | subagent | human | system
  owner_id
  parent_agent_id
  parent_actor_id
workspace:
  root
  workspace_id
  repo_id
  worktree_id
  branch
source:
  kind: hook | native_tool | cli | watcher | ide | server
  event
  tool_name
  source_ref
payload:
  endpoint-specific object
```

Adapters may omit unknown nullable agent fields, but the server must record
which fields were absent. Missing agent identity can lower confidence or reduce
a soft-conflict check, but it cannot create a stronger denial than the policy
allows.

## HTTP API

The state server exposes local HTTP under `/v1`.

```text
GET  /health
GET  /v1/current
GET  /v1/events?workspace_id=&since=&limit=
POST /v1/session/register
POST /v1/session/heartbeat
POST /v1/reservation/declare
POST /v1/reservation/request
POST /v1/reservation/claim
POST /v1/reservation/cancel
POST /v1/claim/acquire
POST /v1/claim/refresh-observation
POST /v1/claim/release
POST /v1/activity/finalize
POST /v1/authorize
POST /v1/human/observe
POST /v1/human/save-check
POST /v1/reconcile/ack
POST /v1/context/render
POST /v1/notifications/poll
GET  /v1/notifications/stream
POST /v1/resume/next
POST /v1/outbox/sync
GET  /v1/runtime/identity
```

`/v1/events` accepts optional `workspace_id`, `since`, and `limit` query
parameters. `limit` defaults to 100 and is clamped to 1..100; successful
responses return `{ "status": "ok", "events": [...] }`.

`/v1/authorize` is the single policy entry point for supported tool actions.
`/v1/human/observe` records human presence/save/change/delete observations from
watcher or CLI sources. High-confidence `save`, `change`, and `delete`
observations become unreconciled human writes unless they occur while an active
same-file write fence attributes the save to an agent write. `/v1/human/save-check`
is advisory for human tools: it returns `decision: "clear"` or `decision: "warn"`
with claim/write-fence conflicts, and clients fail open for human saves when the
server is unavailable. `/v1/reconcile/ack` accepts `agent_id`, `workspace_id`,
required `reservation_id`, `decision`, non-empty `files_reread`, and
`human_change_summary`; it clears unreconciled human-write blocks only for
`adopt` or `reapply`, only after reread files are covered by the caller's active
exact file reservation.
`/v1/notifications/poll` returns pending coordination notifications for a
target agent and marks returned notifications as delivered so a later poll does
not redeliver the same notification. `/v1/notifications/stream` returns an SSE
stream for a target agent in a workspace: each event uses `id: <sequence>` from
that target agent/workspace's monotonic notification sequence, and the JSON data
includes the same `sequence`. Stream delivery does not mark notifications
delivered immediately. On reconnect, `Last-Event-ID` / `last-event-id`
acknowledges notifications through that sequence as delivered, and the stream
then replays later pending notifications. Reservation notification payloads
include the stored, non-empty reservation purpose. `/v1/resume/next` is the
durable recovery path: it returns the first claimable reservation for the
session, including its stored purpose, after rereading the target, even if the
reservation notification was already delivered or the client missed the poll or
SSE response. `/v1/reservation/claim` is the explicit reservation claim path; it
takes a `wait_id`, uses the stored reservation purpose, and creates active
reservation scope plus an active claim for the reservation owner. Unclaimable
claim responses use `reason_code` `reservation_queued`,
`reservation_expired`, `reservation_owner_mismatch`, or
`reservation_not_found`; existing waits also include `reservation_status`.
`/v1/authorize` may lazy-claim a claimable reservation for hook and sandbox
authorization sources after the client rereads and retries the write boundary;
read-only conflict checks must not claim reservations. `/v1/claim/acquire`
records the target existence and content hash when `root` is supplied;
hook-originated native file writes compare that observation before authorization.

Native Stateful tool integrations that expose the full state surface map
directly onto these endpoints. Native tool handlers do not implement policy
branches; they validate tool arguments, receive the active `agent_id` and
`workspace_id` from the runtime integration, call the HTTP API, and return the
server result. For OMP, the bridge uses that `ctx.sessionManager`-derived
`agent_id` and fails closed when it is unavailable. The shipped OMP extension
registers lazy resume
helpers, not the full `state_*` tool surface, so OMP agents must use the active
tool list: use `state_*` tools only when exposed; otherwise rely on native
`edit`/`write` predeclare/claim, lazy resume, or a write boundary with an existing
`reservation_id`. The single adapter-only exception is duplicate cleanup:
`state_claim_release` maps a server `404 claim_not_found` into a successful
no-op result when the same-agent claim is already gone, while the direct HTTP
route still returns `404`.

Current envelope enforcement covers `/v1/authorize`, `/v1/human/observe`, and
`/v1/reservation/declare`, `/v1/reservation/request`, `/v1/reservation/claim`,
and `/v1/reservation/cancel`. Reservation declare requires non-empty `purpose` and non-empty
`files_planned`; empty arrays and empty or normalized-empty paths fail with
`missing_scope`. `files_planned` is a task-level target set and may contain every
known file or directory the task expects to mutate; clients redeclare when that
set grows. Reservation request requires non-empty `purpose` and a non-empty
`path`; empty or normalized-empty request paths fail with `missing_scope`.
Reservation claim clients provide a `wait_id` only; they must not provide a
purpose because the server uses the stored reservation purpose. Unclaimable
claim responses expose `reason_code` and, for existing waits,
`reservation_status`.

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

Native edit tools such as Codex `apply_patch`, `Edit`, and `Write` or OMP
`edit` and `write` expose targets to hooks. OMP `edit` and `write` use
predeclare/claim as the default simple-write path: if the tool call has no
explicit reservation id and exactly one simple tool-visible repo file target, the
extension declares the exact file scope and acquires same-reservation claims
before the first `/v1/authorize` call. Other native edit flows
require task-level reservation covering the target and an active
same-reservation file claim before hooks call `/v1/authorize` with the
operation-specific action, including `write_file`, `delete_file`, and
`move_file` with source `path` / `old_path` and destination `new_path`.
PreToolUse authorization sends current `base_observations` for each affected
file target when the hook can identify the target. File-changing native, hook,
and sandbox writes fail closed with `reason_code: missing_base_observation`
whenever a freshness-scoped action omits an observation for an affected file,
even if `workspace.root` is absent or not readable by the server. If an
observation is supplied and the server can read the workspace under
`workspace.root`, `/v1/authorize` compares it against current file state;
existence or `content_hash` changes for an affected target return `deny` with
`reason_code: stale_target_observation` and require the caller to reread before
retrying.

This base-observation requirement is one of the presence-first safety rails that
let broad reservation blocking be measured separately from freshness. Shipped v1
also acquires short same-file write fences at authorized write boundaries.
Enforcement mode still requires active reservation scope plus same-reservation
claims for supported writes; awareness mode softens only reservation/scope/claim
coordination denials to warnings. `write_directory` and adapter paths where exact
file target extraction or readable file state is unsupported keep the shipped
reservation/claim and sandbox-scope guardrails until per-file observation or
equivalent independent fence coverage is implemented for that surface.

PostToolUse observes completed native edits and sandbox `write-targets`
transactions, records the result, and releases the same-reservation claims and
write fences that authorized the completed write boundary. Blocking pre-tool
denials release auto-claims except `stale_target_observation`, which keeps them
so the same agent can reread and retry. Released claims and fences leave the live
context render and do not authorize a later write; the agent must reread and
reacquire a claim, or lazy-claim a claimable reservation, before retrying.
Remaining OMP `edit` denials are captured as live-agent lazy edit
operations when the patch has safe repo-relative line targets. Remaining OMP
`write` denials are captured as live-agent lazy write operations with the
original full write content. Denials with a wait id reuse that id; other
retryable lazy operations receive a generated live-agent operation id. For stored
wait ids, `lazy_edit_resume` and `lazy_write_resume` first check
notifications/resume state for the saved wait id, then claim the queued
reservation and re-authorize the original edit or write. Generated no-wait
operation ids still require the agent to fix scope, receive and claim a
claimable reservation, or recover from another fallback denial before resume.
`lazy_edit_resume` verifies the file content still matches the queued base text,
then applies only line-based edit patch operations; block operations or changed
files require regenerating the patch. `lazy_write_resume` verifies the target
still matches the queued state, then writes the captured full content; changed
targets require retrying the write. `lazy_bash_resume` stores a trusted external
`stateful sandbox run ...` command when the original OMP Bash call cannot display
the scoped grant prompt, asks for the same grant during resume, re-authorizes the
original Bash tool call, and reruns the stored command. For Bash, command text
alone never authorizes tool use.
Hook adapters normalize namespaced runtime tool names to their leaf before
policy classification: `functions.bash` follows Bash rules,
`functions.python` / `functions.javascript` / `functions.js` /
`functions.ruby` / `functions.julia` follow eval-tool rules, and
`functions.read` / `functions.search` / `functions.goal` follow native non-writing rules.
Codex raw Bash is denied by stateful hooks with sandbox guidance. Bash hook
calls for repo-internal shell work use a single strict invocation of the trusted `stateful` binary running `stateful sandbox run ...` with one or more `--command <cmd>` flags. Repeated `--command` values are compiled into one sandbox-internal script; outer Bash wrappers with multiple commands, redirects, pipelines, substitutions, or environment assignments remain denied.
Ordinary read work should use agent-native read/search/diff tools when available. Read-only
command-shaped inspection uses `<absolute-stateful-binary> sandbox run --fs
read-only --network disabled --command <cmd>`; the read-only profile rejects
`--network enabled`. Command-shaped repo writes use `--fs write-targets` with
explicit `--write-target <file>`, `--create-target <file>`, or
`--write-dir <repo-dir>` values. External-profile exact repo file targets
auto-declare/claim after missing-reservation/scope denials when no explicit
reservation is supplied. `--write-dir` is a repo-relative `write_directory`
target and requires matching directory reservation and same-reservation claim
coverage.
OMP built-in Bash may run only strict trusted `stateful sandbox run ...` and
`stateful sandbox process find ...` commands after Stateful preflight.
Arbitrary raw Bash and Python/JavaScript/JS/Ruby/Julia eval-tool execution is
denied at host approval and hook levels. Command execution and process
inspection are not generated tool calls. External write/create/write-dir,
socket, or signal scope and repo-external OMP native `edit`/`write` file targets
auto-approve the scoped Stateful-owned OMP grant prompt by default through
`stateful.autoApprove: true`, while sandbox scope validation, hooks,
reservation/claim checks, and grant limits still apply. Set
`stateful.autoApprove: false` to require the prompt. The grant prompt shows
purpose, declared scope, examples, max uses, and expiry rather than raw command
text, and matching calls reuse the grant until it expires or reaches its use
limit. When auto-approval is enabled, no prompt is shown.
The external sandbox profile requires
purpose and command; read-only/no-declared-scope operations may
omit targets. Repo-relative exact file scopes can auto-declare/claim after
missing-reservation/scope denials when no explicit reservation is supplied;
repo-relative `--write-dir` requires matching directory reservation and
same-reservation claim coverage; supplied absolute external write scopes are
validated as paths outside the repo. On macOS, external profile runs also allow
trust/identity Mach lookups for `trustd` and DirectoryService so Go TLS clients
such as `gh` can verify certificates. In OMP, it starts through built-in Bash
after Stateful preflight; repo-external OMP native `edit`/`write` file targets
use the same scoped grant. `stateful.autoApprove: true` auto-approves only that
Stateful-owned prompt and does not bypass reservation or claim requirements when
repo-relative write scope is supplied.
Local git operations use `<absolute-stateful-binary> sandbox run --fs git
--network disabled --command 'git <args>'`; use `--network enabled` only for
remote git operations. GitHub pull request list/view/status/create commands use
`<absolute-stateful-binary> sandbox run --fs github-pr --network enabled --command
'gh pr <list|view|status|create> ...'`; in OMP, use built-in Bash with the same trusted command. Both profiles reject repeated `--command` because they intentionally validate one direct `git` or `gh pr` command. Use the
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

`deny` means the adapter must block the action. `warn` is shipped for awareness
mode and means the adapter should surface coordination context but may proceed
unless another hard safety denial applies. `error` is used for state-server,
protocol, or sandbox execution failures; write and reconciliation paths treat
`error` as fail-closed. The shipped `/v1/authorize` response returns
allow/warn/deny/error plus `wait` or `reservation` details when applicable, and
it appends `AuthorizationDenied` events for deny decisions and warning audit
events for awareness-mode warnings.

For scheduling APIs, a hard conflict decision may produce a queued reservation
request instead of an immediate reservation. V1 queues only hard
conflicts in the same `workspace_id` and normalized `relative_path`. Soft
repo-relative conflicts remain warning context because they signal future
integration risk rather than immediate physical file overwrite.

Queued requests are promoted FIFO. The shipped queue stores one requested path
per wait request, and a request is reservable only when that requested resource
is available. Promotion is triggered by explicit claim release, agent or
activity finalization, claim/reservation expiry, or current-state materialization
that finds an already-unblocked queued waiter. Promotion creates short claimable
reservations and pending notifications whose payloads carry the waiting row's
stored, non-empty purpose.

Promotion creates claimable reservations first. A claimable reservation is not
active write authority. Each waiting agent must reread the target. Manual
native-tool or CLI flows then explicitly claim with `state_reservation_claim` or
`stateful reservation claim --wait-id <id>`; claim uses the stored reservation
purpose and clients do not provide a claim purpose. Hook and sandbox
authorization sources may lazy-claim the claimable reservation at the retried
write boundary. Claiming creates active reservation scope and active
same-reservation claims. The default claimable reservation TTL is 120 seconds; the
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
through `reservation/cancel`; shipped stop or activity finalization also
cancels that agent's queued and claimable (`reserved`) requests. Explicit user
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

Shipped tables from `Store::migrate`:

```text
schema_migrations
events
agents
activities
reservations
claims
write_fences
human_observations
wait_queue
notifications
outbox
```

`events` is append-only audit history for shipped coordination events. Accepted
agent registration and reservation declaration events materialize current-state rows in the same
transaction as the event append. Lifecycle mutation APIs such as claim release,
reservation claim, reservation cancel, and activity finalize update materialized tables
directly and append their audit events in the same transaction. If the audit
append or event-backed materialization fails, the surrounding mutation rolls
back.

Required indexes from `Store::migrate`:

```text
events(workspace_id, created_at)
events(agent_id, created_at)
events(agent_id, sequence)
agents(workspace_id, agent_id)
activities(workspace_id, expires_at)
activities(agent_id, workspace_id, expires_at)
reservations(agent_id, status, expires_at)
reservations(status, expires_at)
claims(workspace_id, relative_path, status)
claims(workspace_id, absolute_path, status, expires_at)
claims(repo_id, relative_path, status, expires_at)
claims(reservation_id, workspace_id, relative_path, status)
claims(status, expires_at)
write_fences(workspace_id, relative_path, released_at, expires_at)
write_fences(agent_id, workspace_id, released_at, expires_at)
human_observations(workspace_id, relative_path, kind, confidence, reconciled_at)
wait_queue(workspace_id, relative_path, status)
wait_queue(agent_id, status)
wait_queue(status, reservation_expires_at)
UNIQUE wait_queue(request_id) WHERE request_id IS NOT NULL
notifications(target_agent_id, status)
notifications(target_agent_id, workspace_id, status, sequence)
notifications(status, expires_at)
outbox(agent_id, sequence, sync_status)
```

Migration keeps older databases usable by renaming legacy `sessions` identity
columns to `agent_id`, copying old `sessions` rows into `agents`, dropping the
dead `sessions`, `conflicts`, `overrides`, and `reconciliations` tables, and
backfilling required legacy columns before indexed reads rely on them.

SQLite outbox setup and migration must add or backfill required legacy columns
before creating the canonical `outbox(agent_id, sequence, sync_status)` index.
Legacy outbox tables missing `workspace_id`, `event_type`, `payload_json`, or
`sync_status` must be migrated before startup relies on indexed outbox reads.
Missing `sync_status` values are backfilled as `TEXT NOT NULL DEFAULT 'pending'`
so pre-migration rows remain eligible for sync.
SQLite notification setup and migration must add and backfill the `sequence`
column before creating the canonical
`notifications(target_agent_id, workspace_id, status, sequence)` index. Legacy
notification rows missing `sequence` are backfilled with their SQLite `rowid`
as the legacy insertion-order sequence so detached server startup and health
checks can open older databases before indexed notification reads run.

`AgentHeartbeat` materialization refreshes the agent timestamp, active claim
expiry, active activity expiry, and active reservation expiry. Reservation refresh is
capped at 60 minutes from `declared_at`.

The shipped state server runs a background expiration loop while serving and
also triggers expiration lazily from read/write paths. Expiration covers stale
claims, stale reservations, stale reservation state, stale write fences,
presence/dirty human observations, and eligible FIFO waiter promotion. The
background and lazy paths must be transactionally equivalent.

The same maintenance loop prunes old historical evidence after the built-in
14-day retention window. Pruning deletes old `events` rows and `notifications`
rows whose status is `expired` or `delivered`. It must not delete active
current-state rows, pending notifications, active write fences, or outbox sync
evidence.

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
`unknown`), enabled-repo hooks, native tool calls, and CLI fallbacks derive the
effective request workspace id from the enabled repo's canonical git root
(`workspace-...`) so two different enabled repos do not share one conflict
domain. Explicit non-default runtime or command `--workspace-id` values are
preserved.

`stateful server start` starts the HTTP runtime and prints
`stateful server join ...` commands. Binding to `0.0.0.0` makes the runtime
LAN-reachable, but printed join commands target loopback so remote machines use
an SSH tunnel before joining. `stateful server join` rejects non-loopback plain
`http://` base URLs before runtime validation or config writes unless
`--allow-plain-http` is explicitly passed; that opt-in sends the bearer token in
cleartext. Join validates the host runtime, writes global runtime discovery for
the host server, and only enables the current repo when `--enable-repo` is
supplied.

HTTP runtime discovery and identity checks use a bounded connect attempt.
Endpoints loaded from stale `server.json` files or `STATEFUL_SERVER_URL`
therefore fail promptly under the availability rules instead of stalling hook or
native tool startup. Operators correct stale LAN discovery by restoring the
tunnel and rerunning `stateful server join ...`, or by replacing the explicit
override when `STATEFUL_SERVER_URL` and `STATEFUL_SERVER_TOKEN` are set.

## Local HTTP Trust

The server binds to `127.0.0.1` by default. Requests, except `/health`, must
include the workspace bearer token from runtime discovery.

The token is a local trust guard, not a hard security boundary. It prevents
casual spoofing by unrelated local processes but does not replace OS-level
process isolation or managed hooks.

By default, CLI join never sends the bearer token to a non-loopback plain
`http://` base URL. Use an SSH tunnel and join the loopback endpoint for remote
runtimes, or pass `--allow-plain-http` only when cleartext token exposure is
acceptable.

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
agent_id
workspace_id
sequence
created_at
payload
sync_status: pending
```

`outbox_id` is the idempotency key. The server must treat repeated sync attempts
for the same `outbox_id` as the same event. Sync preserves sequence order per
agent.

The outbox cannot authorize writes, clear reconciliation blocks, or extend
claims while the state server is unavailable.

## CLI Surface

The implementation should include a small CLI for setup and debugging:

```text
stateful install [--yes]
stateful install --agent omp [--profile <name>] [--yes]
stateful enable [--repo <path>]
stateful disable [--repo <path>]
stateful repos list
stateful server
stateful server start [--foreground] [--host <host>] [--port <port>] [--token <token>] [--workspace-id <id>] [--coordination-mode enforcement|awareness]
stateful server restart
stateful server stop
stateful server status
stateful server join <base-url> --token <token> [--workspace-id <id>] [--enable-repo] [--allow-plain-http]
stateful status
stateful current
stateful events
stateful doctor
stateful human observe <path> [--kind save|change|delete|presence|dirty] [--confidence high|low] [--summary <text>]
stateful human save-check <paths...>
stateful reconcile ack --files-reread <path> --summary <text> --decision adopt|reapply|ask_user|abandon --reservation-id <id>
stateful sandbox run --fs write-targets [--write-target <file>|--create-target <file>|--write-dir <repo-dir>] ...
stateful sandbox run --fs read-only|build|git|github-pr ...
stateful sandbox process find <selector>
stateful reservation declare [--agent-id <id>] [--workspace-id <id>] --purpose <purpose> <paths...>
stateful notifications poll [--agent-id <id>] [--workspace-id <id>]
stateful resume next [--agent-id <id>] [--workspace-id <id>]
stateful hook <codex|omp> <event>
stateful sync-outbox
```

`stateful install --agent codex --yes` configures global Codex hooks, generated
skills, and external sandbox approval rules. Codex installs wire `SessionStart`,
`UserPromptSubmit`, `PreToolUse`, `PostToolUse`, and `Stop`. `stateful sandbox
run --fs external --purpose ... --command ...` is gated by a Codex execpolicy
prompt
before it runs the external sandbox command. Purpose-and-command-only operations
are allowed for read-only/no-declared-scope use; supplied external write scopes
are validated before the sandbox starts. On macOS, that external profile also
permits `trustd` and DirectoryService Mach lookups needed by Go TLS certificate
verification.
`stateful install --agent omp --yes` or
`stateful install --agent omp --profile <name> --yes` enables built-in Bash only for strict trusted
`stateful sandbox run ...` and `stateful sandbox process find ...` commands
after Stateful preflight. Command execution and process inspection are not
generated tool calls. External write/create/write-dir/socket/signal scope and
repo-external OMP native `edit`/`write` file targets auto-approve the scoped
Stateful-owned OMP UI grant by default through `stateful.autoApprove: true`,
while sandbox scope validation, hooks, reservation/claim checks, and grant
limits still apply. Set `stateful.autoApprove: false` to require the prompt.
The grant prompt shows purpose, declared scope, examples, max uses, and expiry
rather than raw command text, and matching calls reuse the grant until it
expires or reaches its use limit. When auto-approval is enabled, no prompt is shown.
Raw arbitrary OMP Bash and Python/JavaScript/JS/Ruby/Julia eval-tool sandbox invocations are denied.
OMP `session_start`, `tool_call`, `tool_result`, and `session_shutdown`
extension events to `stateful hook omp session-start`, `pre-tool-use`,
`post-tool-use`, and `stop`; OMP does not expose a stateful
`user-prompt-submit` hook. `stateful enable` opts a repo into enforcement.
`stateful server start` defaults to `--coordination-mode enforcement`. Passing
`--coordination-mode awareness` returns warnings for reservation/scope/claim
coordination denials while keeping hard denials for stale observations,
unreconciled human writes, same-file write fences, and unsafe commands when
those checks apply. Detached `stateful server restart` reuses the coordination
mode recorded in the runtime file. `stateful human observe` posts an enveloped
watcher-style observation to `/v1/human/observe`; the shown `--kind` and
`--confidence` values are server/API-validated values, while Clap accepts
strings. `stateful human save-check` calls the advisory `/v1/human/save-check`;
`stateful reconcile ack` calls `/v1/reconcile/ack` and requires the reservation
id that covers the reread files.

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

Before publishing or releasing a build, run the same verification gates as
`.github/workflows/rust.yml`:

```text
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
env -u STATEFUL_CODEX_RUN_ID -u CODEX_THREAD_ID cargo test --workspace
cargo test -p stateful-cli --features codex-benchmark --test hook
python3 -m venv .venv && . .venv/bin/activate && python -m pip install pytest && python -m pytest crates/stateful-bench/scripts/tests
```

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
does not load them at runtime. The built-in Rust windows are active
reservation/activity refresh at 900 seconds, active reservation maximum at 3600
seconds, active claim at 300 seconds, claimable reservation/notification at 120
seconds, write fence at 300 seconds plus a 5-second release-observation grace,
and historical event plus eligible notification retention at 14 days.
Configurable runtime loading is future hardening work.

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
- coordination-mode parsing and awareness-mode warning behavior, including hard
  denials that must not soften
- short write-fence acquisition, conflict, release, expiration, and context render
- nested Codex benchmark sandbox hook authorization only when the
  `codex-benchmark` cargo feature is enabled
- native edit hook authorization plus Bash sandbox fixtures
- prompt renderer assertion and output coverage for shipped store-backed rendering
- heartbeat refresh of activities, capped active reservation TTL, and active claims
  still covered by active reservation
- background and lazy expiration of stale claims, active reservations, claimable
  reservations, write fences, and human observations
- missing purpose and empty `files_planned` rejection
- SQLite event append plus materialized-view transaction behavior
- sandbox-run execution with artifact writes limited to an authorized directory
- state-server unavailable behavior
- outbox idempotent sync
- human observation, advisory save-check conflicts, unreconciled-human-write
  denial, and reconciliation acknowledgement clearing rules

Pure scope-policy primitives should be testable without starting the HTTP
server. The shipped store-backed authorization service is currently tested
through server-level tests; future extractions should move reusable policy
pieces into `stateful-core` without duplicating product policy in adapters.

## Migration Path

The prototype supports user-level installation with repo allowlist gating.
`stateful install --yes` installs stateful global files only. `stateful install
--agent codex --yes` configures global Codex hooks and writes
`skills/stateful-command-policy/` (`SKILL.md`, `omp-tools.md`,
`sandbox-tools.md`, `denial-recovery.md`, `subagent-write-recovery.md`) and
`skills/dispatching-parallel-agents/SKILL.md`. `stateful install
--agent omp --yes` installs the OMP extension entry point, always-apply
`rules/stateful-required.md` rule, `skills/stateful-command-policy/`
(`SKILL.md`, `omp-tools.md`, `sandbox-tools.md`, `denial-recovery.md`,
`subagent-write-recovery.md`) manual files, and OMP config under the
`stateful` profile agent directory (`~/.omp/profiles/stateful/agent`) with
`tools.approvalMode: yolo`, `stateful.autoApprove: true`,
`bash.enabled: true`, `eval.py: false`, `eval.js: false`, `eval.rb: false`,
and `eval.jl: false`. The installer removes `tools.approval` from the stateful
profile because yolo mode delegates safety to
Stateful hooks. OMP built-in Bash may run only strict trusted
`stateful sandbox run ...` and `stateful sandbox process find ...` commands
after Stateful preflight; arbitrary raw Bash and Python/JavaScript/JS/Ruby/Julia
eval-tool execution is still denied at host approval and hook levels. Command
execution and process inspection are not generated tool calls; `stateful.autoApprove: true`
skips only the Stateful-owned grant prompt for external sandbox scope and
repo-external native file targets while sandbox scope validation, hooks,
reservation/claim checks, and grant limits still apply. `stateful enable`
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
