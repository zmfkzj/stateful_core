# Usage reference

This reference covers the supported `stateful.v2` / `lease-1` surface. The
server is local-only and its protected endpoints require the runtime bearer
token. Use the CLI and installed OMP extension for ordinary operation; direct
HTTP is for adapters that can supply the full protocol envelope and resource
observations.

## Installation and runtime

```bash
stateful install --agent omp --yes
stateful enable
stateful server start
stateful status
```

`install --agent omp --yes` writes the isolated OMP `stateful` profile and its
extension. `enable` adds the current repository to the user-level enabled
registry. `stateful status` reports only live coordination counts. Use
`stateful doctor` for installation and configuration, and `stateful server
status` for server runtime health.

`STATEFUL_HOME` selects the global installation and runtime directory. When
unset it is `$HOME/.stateful_core`. Its database is
`$STATEFUL_HOME/state.db`; configuration, repository registry, and server
runtime files live beside it. Set `STATEFUL_HOME` to a nonempty path to isolate
one local runtime from another.

The server commands are:

```text
stateful server start [--foreground] [--host 127.0.0.1] [--port 43873]
stateful server restart
stateful server stop
stateful server status
```

A server rejects non-loopback listeners. `stateful doctor`, `stateful repos
list`, and `stateful disable [--repo <path>]` are local administration commands.

## Structured commits

Use an active task and its owning agent id. Every commit target must be an
explicit file path; broad pathspecs, directories, absolute paths, and globs are
rejected.

```bash
stateful commit --task-id <id> --agent-id <id> -m "Document leases" README.md docs/architecture.md
```

The command reads the named files and repository metadata, prepares a commit
lease, and, when queued, polls the lease request. On an offer it takes fresh
exact reads, activates the lease only when the result is `active: true`, then
takes fresh exact reads and prepares again to receive a ready attempt and
permit; a superseded request follows its `superseded_by` batch. It reports
the terminal result to `/v2/commits/complete`. Once the Git HEAD compare-and-swap
succeeds, the CLI result remains successful; any later index synchronization,
observation, server completion, or hook-apply problem appears in `warnings`
instead of an error that could invite a duplicate commit. A failed post-CAS
observation or transition check is reported to the server as `uncertain`, so its
lease remains draining for explicit resolution. The command does not
commit files that were not named.

Release an active lease batch explicitly only when its owner is finished:

```bash
stateful lease release <batch-id> --task-id <id> --agent-id <id>
```

A release returns `released` if no operation is in flight, otherwise `deferred`.
A deferred lease drains and is released after its in-flight attempt is completed.

## OMP lifecycle integration

The installed extension registers `agent_start`, `tool_call`, `tool_result`,
`agent_end`, and `session_shutdown` handlers. It obtains root and leaf ownership
from `ctx.sessionManager.getSessionId()` and `ctx.sessionManager.getLeafId()`.
Both values must be valid for ownership; there is no fallback to event fields,
environment variables, a PID, or a session file.

- `agent_start` starts the task and begins its heartbeat.
- `tool_call` performs pre-operation coordination and retains a write attempt
  when the server permits it.
- `tool_result` atomically stores each correlated write terminal payload exactly
  once in the owner-only `$STATEFUL_HOME/omp-terminal-outbox` (or
  `$HOME/.stateful_core/omp-terminal-outbox`) before sending it. Live retries
  and the next extension startup replay that exact payload; only an `allow`
  response deletes its outbox file.
- `agent_end` ends a leaf task when it will not continue.
- `session_shutdown` ends remaining tasks for the session.

The extension is an adapter, not an independent lease manager. It must preserve
the server's task, resource, permit, and terminal-operation contract.

## Codex fallback boundary

Codex native-tool conformance is unverified, so native tool calls fail closed.
`UserPromptSubmit` supplies derived `task_id`, `agent_id`, and the installed
Stateful binary in hook context, then starts the hidden Codex heartbeat helper
for the root task. While its owner record and parent PID/start identity match,
the helper sends `/v2/tasks/heartbeat` every
`heartbeat_interval_seconds`, refreshing expiry so prompt inactivity alone does
not expire a live task. A failed request or server response is retried at the
next interval. `Stop` removes the owner record; the helper exits when that
record is removed or changes, or when the parent exits or its start identity no
longer matches. `PreToolUse`
permits only an exact installed-binary adapter wrapper:
read-only, Git, and process sandbox operations plus `stateful status`,
`stateful doctor`, and `stateful repos list` are allowed. Mutation sandbox
operations, structured commits, and lease release must carry the matching
derived task and agent identity. The wrapper rejects outer-shell chaining and
expansion.

## HTTP API

### Authentication and envelope

`GET /health` returns `200 OK` and is not authenticated. All other routes use
`Authorization: Bearer <runtime-token>`.

Every command `POST` body is an envelope with a nested `payload`:

```json
{
  "protocol_version": "stateful.v2",
  "contract_revision": "lease-1",
  "request_id": "unique-command-id",
  "task_id": "task-id",
  "observed_at": "2026-08-02T00:00:00Z",
  "agent": {
    "agent_id": "agent-id",
    "actor_id": "actor-id",
    "actor_type": "agent"
  },
  "workspace": {
    "root": "/absolute/repository/root",
    "workspace_id": "workspace-id",
    "repo_id": "repo-id",
    "worktree_id": "worktree-id",
    "branch": "branch-name"
  },
  "source": {
    "kind": "cli",
    "event": "command-name",
    "source_ref": "caller-reference"
  },
  "payload": {}
}
```

`agent` may additionally carry turn, ownership, and parent identity fields.
`source.kind` is `cli` or `hook`; `source.tool_name` is optional. Timestamps are
RFC 3339 strings. Reusing a request id for the same command replays its receipt;
do not reuse it for a different command.

### Routes

| Route | Method | Purpose |
| --- | --- | --- |
| `/health` | `GET` | Unauthenticated liveness check |
| `/v2/tasks/start` | `POST` | Start a task with settings, expiry, and next action |
| `/v2/tasks/heartbeat` | `POST` | Refresh an active task's next action and expiry |
| `/v2/tasks/finalize` | `POST` | Finish a task, optionally with handoff text |
| `/v2/tasks/cancel` | `POST` | Cancel a task, optionally with handoff text |
| `/v2/reads/start` | `POST` | Start a resource read |
| `/v2/reads/complete` | `POST` | Complete a read and establish valid evidence when exact and stable |
| `/v2/writes/prepare` | `POST` | Prepare a write and obtain `ready`, `queued`, `reread_required`, or `denied` |
| `/v2/writes/complete` | `POST` | Record write success, known failure, or uncertainty |
| `/v2/commits/prepare` | `POST` | Prepare a structured commit; same request shape as write preparation |
| `/v2/commits/complete` | `POST` | Record structured-commit completion; same shape as write completion |
| `/v2/lease-requests/{batch_id}` | `GET` | Read the caller task's queue/offer state |
| `/v2/leases/activate` | `POST` | Activate a matching offered batch only with post-offer exact reads; `active: true` requires fresh reads and a new prepare |
| `/v2/leases/release` | `POST` | Release or defer release of an active batch |
| `/v2/status` | `GET` | Counts for tasks, leases, queue, attempts, and uncertain writes |
| `/v2/audit` | `GET` | Recent audit records; accepts optional `limit` |

`GET /v2/lease-requests/{batch_id}` requires `workspace_id`, `task_id`, and
`now` query parameters. `GET /v2/audit` defaults to 100 rows when `limit` is
absent.

### Command payloads

Task start payloads provide `next_action`, `settings`, `expires_at`, and an
optional runtime process (`pid`, `process_start_identity`). Task heartbeat
provides `next_action` and `expires_at`; task finalize and cancel accept optional
`handoff` text.

Read start provides `read_id`, `invocation_id`, and resource observations. Read
complete repeats those identifiers and observations and supplies
`terminal_success`, `complete`, `stable`, and `exact`. Only all-true completion
creates evidence usable by mutation.

Write and commit preparation provide:

```text
invocation_id
operation
current: [resource observations]
request_expires_at
lease_expires_at
attempt_deadline
```

Completion provides `attempt_id`, `permit_id`, `invocation_id`, `terminal`
(`success`, `failed_known`, or `uncertain`), `post_resources`,
`expected_post_resources`, and optional `error`. Lease activation provides
`batch_id`, `offer_id`, `version`, and `lease_expires_at`; lease release provides
`batch_id`.

A client must not perform write I/O before a `ready` result. For `queued`, poll
the lease request and follow `superseded_by` when the request is superseded. On
an offer, take fresh exact reads and activate it. An `active: false` result is
not activation: block for a fresh exact read, then prepare again and retry
activation when offered. Only after `active: true`, take fresh exact reads and
prepare again to receive the ready attempt and permit. For `reread_required`,
collect fresh exact evidence and prepare again. Treat `denied`, expired, and
cancelled requests as terminal.

## Boundaries

Stateful does not provide remote collaboration, file-system interception,
identity authentication beyond its local runtime token, Git conflict resolution,
work scheduling, or a general-purpose sandbox. Keep normal Git review and
branch practices; use the platform and agent-runtime sandbox controls for
untrusted code.
