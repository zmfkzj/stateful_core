# Implementation Contract

This document fixes the implementation defaults for v1. Product policy remains
in the architecture and state-model documents; this file defines the concrete
API, storage, hook, runtime, and test contracts that implement that policy.

Implementation details may change without a new product decision only when they
do not weaken enforcement, change user-facing policy, or change committed config
semantics.

## Implementation Stack

V1 is Rust-only.

The state server, policy engine, SQLite persistence, CLI, hook adapter commands,
MCP adapter, watcher, validation runner, and prompt renderer should live in the
same Rust codebase.

The prototype supports user-level installation with repo allowlist gating.
`stateful install --yes` configures global Codex hooks and MCP. `stateful enable`
opts the current repo into enforcement. Repo-local hooks remain available through
`stateful enable --repo-local-codex` as a compatibility fallback.

Codex hooks should invoke the compiled `stateful` binary instead of embedding
policy or adapter logic in separate scripts. Hook configuration may reference
the binary directly by absolute path or by PATH lookup.

This keeps parsing, authorization, rendering, validation, and outbox behavior in
one implementation and avoids policy drift.

## Protocol Version

Every request from hooks, MCP tools, CLI commands, observers, and future IDE
clients must include:

```text
protocol_version: stateful.v1
request_id
session
workspace
source
```

`protocol_version` uses `name.major` form. A major mismatch fails closed for
write authorization, validation, reconciliation, intent declaration, lease
acquisition, and lease refresh. Read-only context requests may return
`error: protocol_mismatch` with no side effects.

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
POST /v1/lease/acquire
POST /v1/lease/release
POST /v1/activity/observe
POST /v1/activity/finalize
POST /v1/authorize
POST /v1/conflicts/check
POST /v1/context/render
POST /v1/reconcile/ack
POST /v1/validation/run
POST /v1/notifications/poll
POST /v1/resume/next
POST /v1/outbox/sync
GET  /v1/runtime/identity
```

`/v1/authorize` is the single policy entry point for supported tool actions.
`/v1/conflicts/check` is a read-only dry-run wrapper around the same policy
engine and must not create leases or write-authorizing state.
`/v1/notifications/poll` returns pending coordination notifications for a
session. `/v1/resume/next` returns the first active reservation that the session
can claim after rereading the target. `/v1/runtime/identity` is an authenticated
server identity endpoint used by `stateful server stop` to verify that the
runtime file and process id describe the same stateful server. Full scheduling
endpoints such as `intent/request`, `intent/claim`, and `intent/cancel` remain
future contract work; the prototype implements `intent/declare` plus
notifications and resume discovery.

MCP tools map directly onto these endpoints. MCP handlers do not implement
policy branches; they validate tool arguments, call the HTTP API, and return the
server result.

## Authorization Input

```text
action:
  read | search | diff
  write_file
  delete_file | rename_file | move_file
  bash
  validation_run
  reconcile_ack
targets:
  - operation: read | write | delete | rename | move
    resource_type: file | directory | test | task | port | migration
    path
    old_path
    new_path
command
validation_profile
override_instruction
```

`state.file.write` maps to `write_file` and supplies the target path through
structured tool arguments before calling `/v1/authorize`. Native Codex edit
tools such as `apply_patch`, `Edit`, and `Write` may expose targets to hooks,
but the `stateful codex` read-only tmp profile does not rely on them as the
normal repo write path. For Bash, the adapter sends the command string and any
extracted paths; ambiguous mutation targets are denied by default.

## Decision Output

All policy decisions use the same shape:

```text
decision: allow | warn | deny | error
reason_code
message
conflicts[]
required_next_action
context_items[]
audit_event
```

`deny` means the adapter must block the action. `warn` means the action may
continue but the warning must be surfaced to the agent or human where the
runtime allows it. `error` is used for state-server, protocol, or validation
execution failures; write, validation, and reconciliation paths treat `error` as
fail-closed.

For scheduling APIs, a hard conflict decision may produce a queued intent
request instead of an immediate write-authorizing grant. V1 queues only hard
conflicts in the same `workspace_id` and normalized `absolute_path`. Soft
repo-relative conflicts remain warning context because they signal future
integration risk rather than immediate physical file overwrite.

Queued requests are promoted FIFO. A request is grantable only when all requested
resources are available. The server must not partially grant a multi-resource
request. Promotion is triggered by explicit lease release, session or activity
finalization, or lease expiry. Promotion creates a short reservation and a
pending notification for the waiting session.

Promotion creates a reservation first. A reservation is not active write
authority. The waiting session must claim the reservation before the server
creates active intent and active leases. The default reservation TTL is 120
seconds; the default lease TTL is 300 seconds and is refreshed by heartbeat.

For multi-resource requests, the request is grantable only when it is the head
entry for every requested resource queue and none of those resources has an
active lease. `request_id` is the idempotency key: repeating a request with the
same id returns the existing state and must not enqueue a duplicate.

Full scheduling APIs should return immediately with request state. Blocking
waits can be implemented as a future client convenience by polling
notifications or resume endpoints. Queued and reserved request cancellation is
future contract work. Session or activity finalization should cancel that
session's queued and reserved requests once those queues are implemented.
Explicit user overrides do not reorder the wait queue or transfer reservations.

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
validations
reconciliations
human_observations
outbox
```

`events` is append-only and is the canonical audit log. Materialized tables are
updated in the same transaction that appends the accepted event. If
materialization fails, the event append must roll back.

Required indexes:

```text
events(workspace_id, created_at)
events(session_id, sequence)
sessions(workspace_id, session_id)
activities(workspace_id, expires_at)
intents(session_id, status, expires_at)
leases(workspace_id, absolute_path, status, expires_at)
leases(repo_id, relative_path, status, expires_at)
wait_queue(workspace_id, relative_path, status)
wait_queue(session_id, status)
notifications(target_session_id, status)
conflicts(session_id, checked_at)
validations(workspace_id, profile_id, status)
reconciliations(session_id, created_at)
outbox(session_id, sequence, sync_status)
```

Expiration may run in a background loop and may also be triggered lazily by
reads. Lazy expiration must be transactionally equivalent to background
expiration.

## Runtime Files

Committed project config lives under:

```text
.stateful/config.yml
.stateful/validation.yml
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
`protocol_version`, and `started_at`. The prototype writes it with normal local
filesystem defaults; user-only file permissions are a future hardening item.

## Local HTTP Trust

The server binds to `127.0.0.1` by default. Requests, except `/health`, must
include the workspace bearer token from runtime discovery.

The token is a local trust guard, not a hard security boundary. It prevents
casual spoofing by unrelated local processes but does not replace OS-level
process isolation or managed hooks.

If the token is missing or invalid, write, validation, reconciliation, intent,
and lease paths fail closed. Read-only paths may return a minimal unauthorized
error.

## Hook Adapter Contract

Hook scripts are thin adapters:

```text
parse hook input
derive session/workspace/source envelope
extract action and targets
call /v1/authorize or the relevant endpoint
translate decision into hook output
append outbox evidence when observation cannot reach the server
```

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
stateful server start [--foreground]
stateful server stop
stateful server status
stateful status
stateful current
stateful events
stateful doctor
stateful validate <profile>
stateful intent declare [--session-id <id>] [--workspace-id <id>] <paths...>
stateful notifications poll [--session-id <id>] [--workspace-id <id>]
stateful resume next [--session-id <id>] [--workspace-id <id>]
stateful mcp call <tool> [arguments-json]
stateful mcp serve
stateful hook <event>
stateful sync-outbox
stateful commit -m <message> -- <paths...>
stateful push [remote branch]
```

`stateful install --yes` configures global Codex hooks and MCP. `stateful enable`
opts a repo into enforcement and can install repo-local Codex hooks with
`--repo-local-codex` as a compatibility fallback. `stateful server start`
without `--foreground` uses the detached lazy lifecycle. Bare legacy
`stateful server` and `stateful server start --foreground` run in the
foreground and write runtime discovery. `stateful doctor` checks hook
installation files, repo config files, global install fields, repo enabled
status, and global path or registry errors. Active server reachability, config
schema validation, SQLite migration inspection, and validation profile
parseability checks are future doctor extensions.
`stateful commit -m <message> -- <paths...>` is the structured commit wrapper.
`stateful push [remote branch]` is the structured push wrapper. It requires a
clean working tree, an attached current branch, either the current branch's
configured upstream or an explicit `<remote> <branch>` pair matching the current
branch, and rejects force-like target values. Raw `git add`, `git commit`, and
`git push` through Bash remain denied.

## Prototype Verification

The prototype alignment pass must run:

```text
cargo test --workspace
./target/debug/stateful doctor
```

The Task 10 verification run passed `cargo test --workspace`. The doctor command
returned JSON containing the global install fields `global_config_yml`,
`global_runtime_server_json`, `global_state_db`, `repo_enabled`,
`global_paths_error`, and `global_registry_error`.

## Config Defaults

`.stateful/config.yml` owns repo-level defaults:

```text
protocol_version: stateful.v1
intent_ttl_seconds: 900
intent_max_seconds: 3600
directory_scope_depth: 2
delete_requires_exact_file_scope: true
rename_requires_exact_file_scope: true
default_write_policy: deny
event_retention_days: 14
```

`.stateful/validation.yml` owns controlled validation profiles. Agents may
select a profile by name, but they may not provide arbitrary commands at
runtime.

Glob semantics should use gitignore-style path matching relative to the
workspace root. Identity and policy checks still use normalized canonical paths
after glob expansion.

## Test Contract

V1 must have tests for:

- policy decisions as table-driven unit tests
- intent scope matching, including depth-2 directory behavior
- exact file scope for delete, rename, and move
- Bash allowlist and mutation-deny classification
- `state_file_write` authorization plus Bash and native Codex edit hook fixtures
- prompt renderer golden output for brief and detailed modes
- SQLite event append plus materialized-view transaction behavior
- validation profile execution in a temporary git worktree
- state-server unavailable behavior
- outbox idempotent sync
- human-write reconciliation blocks and acknowledgements

The policy engine should be testable without starting the HTTP server. HTTP,
MCP, and hook tests should prove adapter correctness, not duplicate policy
coverage.

## Migration Path

The prototype supports user-level installation with repo allowlist gating.
`stateful install --yes` configures global Codex hooks and MCP. `stateful enable`
opts the current repo into enforcement. Repo-local hooks remain available through
`stateful enable --repo-local-codex` as a compatibility fallback. Plugin
packaging and managed hooks must reuse the same hook adapter library and HTTP
protocol.

The migration order is:

```text
user-level install with repo allowlist gating
repo-local hooks compatibility fallback
plugin packaging for team beta
managed hooks for organization enforcement
IDE extension for human save-gate signals
```

No migration step may introduce a separate policy engine. New clients must call
the same state server API.
