# State Model

This document defines the v1 current-state coordination model.

## State Kinds

The model is intentionally narrow:

- `session`
- `activity`
- `intent`
- `lease`
- `conflict`
- `finalization`
- `override`

These state kinds describe active coordination, not general-purpose memory or
all durable agent facts.

## Goal, Turn, and Intent Boundaries

V1 separates the user's work objective from the authorization unit.

`goal` is the user-visible objective that may span multiple agent turns. It is
used for grouping, handoff, summaries, and context rendering. It does not
authorize writes.

`turn_id` identifies one agent execution slice inside a session: one prompt,
the tool calls that follow, and the final response, pause, or stop. V1 records
turn identity where available, but write authorization is still enforced by
session identity plus active intent.

`intent` is the write-authorization unit. A write-authorizing intent belongs to
one session, expires, and must include matching file or directory scope. Agents
should redeclare intent when the active turn moves to a different file or
directory scope, even if the broader goal is unchanged.

The practical hierarchy is:

```text
session
  goal
    turn
      intent
        write actions
```

For v1, hooks must treat the current Codex hook `session_id` as authoritative.
MCP intent declaration may omit `session_id`; in that case the adapter uses the
current session recorded by lifecycle hooks so that MCP-declared intent and
write authorization evaluate against the same session.

## Activity Record

An activity record summarizes what one actor is doing now.

```text
session_id
actor_id
actor_type: agent | subagent | human | system
owner_id
parent_session_id
parent_actor_id
workspace
workspace_id
repo_id
worktree_id
branch
goal
phase: exploring | editing | testing | blocked | done | failed | idle | expired
files_read
files_editing
files_planned
resource_leases
next_intent
summary
last_result
last_updated_at
expires_at
confidence
source_refs
```

`next_intent` is part of the core model because planned work is often the best
early signal for avoiding conflicts.

## Lease Record

A lease is an advisory claim on a resource.

```text
lease_id
session_id
actor_id
actor_type: agent | subagent | human | system
owner_id
workspace
workspace_id
repo_id
worktree_id
branch
resource_type: file | directory | test | port | task | migration
resource_id
relative_path
absolute_path
mode: read | write | planned
reason
acquired_at
last_heartbeat_at
expires_at
status: active | released | expired
source_ref
```

V1 leases are coordination signals. They are not hard distributed locks and do
not replace source-control review. Only `file` and `directory` leases can
authorize supported write actions in v1.

Lease freshness defaults to `lease_ttl_seconds = 300`. Heartbeats refresh active
lease expiry while the owning session remains active. Missing heartbeats do not
mean success; they eventually make the lease eligible for expiry and FIFO queue
promotion.

## Resource Authorization Classes

V1 separates write authorization from broader coordination.

```text
write-authorizing resources:
  file
  directory

controlled-action resources:
  test

coordination-only resources:
  task
  port
  migration
```

Only `file` and `directory` resources can authorize filesystem writes. Other
resource types can warn, coordinate, or constrain controlled actions, but cannot
authorize filesystem mutation.

`test` resources may inform future test coordination policy, but they do not
authorize filesystem mutation. Test commands write artifacts through explicit
sandbox-run write directories.

`task`, `port`, and `migration` resources are context and warning signals in v1.
They can appear in prompt context and conflict records, but cannot block general
filesystem writes by themselves. Migration file edits are still governed by
file/directory scope. Actual migration execution is outside v1 unless a later
controlled action is added.

## Resource Identity

File and directory resources should carry both physical and repository-relative
identity.

```text
workspace_id
repo_id
worktree_id
branch
relative_path
absolute_path
```

The hard conflict domain is the same `workspace_id` and same normalized
`absolute_path`. This represents two actors touching the same physical file in
the same working tree.

The soft conflict domain is the same `repo_id` and same `relative_path` across
different workspaces, worktrees, or branches. This does not block by default in
v1, but it should produce warning context because later merge or integration
work may conflict.

If repository identity is unknown, only same normalized `absolute_path` should be
treated as a hard conflict. Repo-relative matches with incomplete identity should
be surfaced as unknown-confidence warnings, not denials.

### ID Derivation

V1 derives identity conservatively:

```text
workspace_id = hash(machine_id + canonical realpath of git worktree root)
repo_id = hash(normalized primary git remote URL)
worktree_id = hash(repo_id + canonical realpath of git worktree root)
branch = symbolic branch name, or DETACHED:<short_head>
relative_path = normalized POSIX path relative to the git worktree root
absolute_path = canonical realpath when the target exists; otherwise canonical
                parent realpath + target basename
```

If no git worktree is present, `workspace_id` uses the configured workspace root
realpath, `repo_id` is `unknown`, and `worktree_id` equals `workspace_id`.

Remote URL normalization removes credentials, lowercases the host, strips a
trailing `.git`, and normalizes common SSH/HTTPS forms to the same host/path.
If no remote exists, `repo_id` is `unknown` rather than guessing from local
paths. Unknown repository identity can warn but cannot create repo-relative
denials.

Symlinks are resolved for identity. Display paths may preserve the user's
original spelling, but conflict checks use canonical paths.

## Intent Record

An intent records what a session plans to do before it performs important
actions.

```text
intent_id
session_id
purpose
goal
phase
files_planned
resources_planned
next_intent
declared_at
updated_at
expires_at
max_expires_at
status: active | superseded | finalized | expired
```

Hooks must require an active intent before allowing supported write tool paths.
For v1, a write-authorizing intent must include a non-empty `purpose` plus at
least one file or directory scope in `files_planned`. Callers must infer purpose
from the user or agent instruction when it is not explicit; the server does not
generate a fallback purpose. Abstract resources in `resources_planned`, such as
`task`, `test`, `port`, or `migration`, can provide context but cannot authorize
writes.

Intent declarations replace the session's active scope in that workspace; they
do not append. When a caller adds or changes targets, it must redeclare the
complete intended file set for that session and workspace.

## Intent Scope Matching

All paths are normalized relative to the workspace root before matching.

File scopes authorize writes only to the exact same file:

```text
scope: src/auth.ts
target: src/auth.ts -> allow
target: src/session.ts -> deny
```

Directory scopes authorize writes only up to two path segments below the scoped
directory:

```text
scope: src/
target: src/auth/auth.ts -> allow
target: src/auth/codex/auth.ts -> deny
```

Multi-file writes are allowed only when every target matches at least one file
or directory scope. Delete and rename/move operations require exact file scope;
directory scope does not authorize deletes, renames, or moves.

## Conflict Record

A conflict record captures a policy decision.

```text
conflict_id
session_id
target_resource
conflicting_session_id
conflict_type: missing_intent | active_lease | planned_edit | stale_state | human_change | unreconciled_human_write | soft_repo_conflict | unknown_repo_identity | coordination_resource_overlap
severity: warn | block
decision: allow | warn | deny
reason
collision_domain: hard_workspace_path | soft_repo_relative_path | unknown
checked_at
source_refs
```

Conflict records are audit artifacts. They also help render useful context into
later prompts.

Write attempts without active intent should be recorded as denied conflict
checks with `conflict_type: missing_intent`.

## Wait Queue Record

A wait queue record captures a hard-conflict intent request that cannot be
granted immediately.

```text
queue_id
request_id
session_id
workspace_id
purpose
resources
queue_sequence
queued_at
status: queued | reserved | granted | canceled | expired
blocked_by_session_id
blocked_by_lease_id
reservation_expires_at
grant_trigger: explicit_release | session_finalization | lease_expiry
```

The queue is FIFO by `queue_sequence`. A queued request can be promoted only
when every requested resource is available. `purpose` is required and remains the
caller-supplied purpose from the original request. V1 uses atomic all-or-nothing
grant: multi-resource requests are never partially granted.

The current implementation handles one requested path per wait request.
Multi-resource all-or-nothing scheduling is the target model for future
hardening.

Grant triggers are limited to explicit lease release, session or activity
finalization, or lease expiry. Soft repo-relative conflicts do not create wait
queue records in v1.

Promotion creates a reservation, not active write authority. The waiting session
must reread the target, then explicitly claim the reservation with
`state.intent.claim` or `stateful intent claim --wait-id <id>`. Only that claim
creates write-authorizing intent and active same-session leases. The default
reservation TTL is 120 seconds. If the reservation is not claimed before
`reservation_expires_at`, the reservation expires and the server may promote the
next eligible FIFO waiter.

For multi-resource requests, a request is eligible only when it is at the head of
every resource queue it participates in and every requested resource has no
active lease. A request that is first for one resource but blocked behind another
request on a second resource stays queued.

`request_id` is the idempotency key. Repeating an intent request with the same
`request_id` must return the existing queue, reservation, grant, cancellation, or
expiry state instead of creating a duplicate queue item.

Queued or reserved requests can be canceled explicitly. Session or activity
finalization cleanup for queued and reserved requests is target behavior; the
current implementation releases active leases on finalization.

## Override Record

An override records an explicit user instruction to permit a blocked resource in
the current session. Overrides are never inferred or granted automatically.
Override records are target model only; the current server does not expose an
override authorization path.

```text
override_id
session_id
turn_id
actor_id
resource_type: file | directory
resource_id
reason
user_instruction
created_at
expires_at
status: active | used | expired
source_ref
```

The user owns the judgment and responsibility for an override. An override must
be scoped to a specific resource, current session, and current turn. Overrides
apply only to active lease conflicts. They cannot bypass missing intent, expired
intent, blocked/finalized state, file or directory scope matching, delete
exact-scope rules, or rename/move exact-scope rules.
Overrides do not reorder the wait queue, grant queue priority, or transfer a
reservation from one session to another.

## Identity Rules

V1 identifies work with both session and actor fields:

```text
session_id
turn_id
actor_id
actor_type: agent | subagent | human | system
owner_id
parent_session_id
parent_actor_id
```

Subagents may write only inside the parent session's active valid intent scope.
Their activity and leases are still recorded under the subagent `actor_id`.
Same-owner sessions do not receive automatic override authority.

## Sandboxed Test Execution

Raw Bash test commands are denied by hooks. Agents run tests through the trusted
wrapper after exact `target/` directory intent and a successful same-session
directory lease, for example:

```text
stateful intent declare --session-id <session> --workspace-id <workspace> --purpose "Run the requested tests." target/
stateful mcp call state_lease_acquire '{"session_id":"<session>","workspace_id":"<workspace>","path":"target/"}'
stateful sandbox run --fs write-targets --network enabled --write-dir target --command 'cargo test --workspace'
```

Source-tree edits should use native Codex edit tools such as `apply_patch` or
Edit after exact intent declaration and a successful same-session file lease.
Command-shaped source writes must use exact `--write-target` or `--create-target`
entries, not the `target/` artifact directory scope.

## Finalization Record

A finalization record closes out active work for a turn or session.

```text
session_id
status: done | failed | blocked
summary
files_changed
tests_run
remaining_work
released_leases
finalized_at
source_ref
```

The `Stop` hook should require finalization when a session has active work that
has not been closed.

## Reconciliation Record

A reconciliation record acknowledges that an agent has reread and accounted for a
human write before resuming work on the affected file.

```text
reconciliation_id
session_id
turn_id
actor_id
target_resources
trigger_event_id
files_reread
human_change_summary
conflict_with_plan: yes | no | unknown
decision: adopt | reapply | ask_user | abandon
next_action
created_at
source_ref
```

Only `adopt` and `reapply` can clear an unreconciled-human-write block. `ask_user`
keeps writes blocked until the user provides direction. `abandon` keeps writes
blocked for the affected file and should release or shorten the affected lease.
Clearing the block does not authorize writes by itself; active, unexpired,
matching intent is still required.

The current implementation records reconciliation acknowledgements but does not
yet emit `HumanWriteObserved` events or enforce unreconciled-human-write blocks.

## Events

The canonical event log should use explicit coordination events:

- `SessionRegistered`
- `IntentDeclared`
- `LeaseAcquired`
- `LeaseReleased`
- `HeartbeatObserved`
- `ToolUseObserved`
- `ConflictChecked`
- `OverrideGranted`
- `HumanActivityObserved`
- `HumanSaveGateShown`
- `HumanSaveContinued`
- `HumanWriteObserved`
- `ReconciliationAcknowledged`
- `ActivityUpdated`
- `ActivityFinalized`
- `OutboxEventQueued`
- `OutboxEventSynced`
- `StateExpired`

Accepted events update the materialized current-state view. Expiration may be
driven by background TTL processing or by reads that discover stale state.
Some event kinds above are target model entries and are not emitted by the
current implementation, including override and human save-gate events.

## Freshness Rules

Freshness is required for all active coordination records.

- Active activity and leases must have `expires_at`.
- Supported write actions require an active, unexpired intent with matching
  file or directory scope.
- Default intent TTL is 15 minutes.
- Heartbeats may extend active intent TTL, but never beyond 60 minutes from
  `declared_at`.
- Intent is write-authorizing only when `status = active`, `expires_at > now`,
  the session is not finalized, and `phase` is `exploring`, `editing`, or
  `testing`.
- `phase = blocked` keeps the activity visible but stops write authorization.
- Directory intent scope permits writes only up to two path segments below that
  directory.
- Delete operations require exact file scope.
- Rename and move operations require exact file scope for both source and
  destination.
- Heartbeats extend active leases and activity records.
- Missing heartbeats do not imply success.
- Finalization as `done`, `failed`, or `blocked` finalizes active intents.
- Turn end expires unused overrides.
- Expired records remain historical evidence but stop blocking new work.
- Reads should distinguish fresh, stale, and expired state.

## Retention Rules

V1 retains event and audit history for 14 days by default. Retention affects
historical evidence only; it does not extend live TTLs, leases, intents, or
write authorization.

The current implementation expires live coordination rows lazily when policy
reads detect stale state. Configurable event-retention pruning is future work.

Projects may configure a longer retention window. Shorter retention should be
allowed only when the system can still preserve required audit evidence for
active conflicts, unreconciled human writes, and pending outbox sync.

## Availability Rules

When the state server is unavailable, coordination must fail closed for agent
write authorization and fail open for human saves.

- Supported writes are denied.
- Raw Bash and Bash calls that are not a strict
  `<absolute-stateful-binary> sandbox run ... --command <cmd>` wrapper are
  denied. Command-shaped writes through `--fs write-targets` fail closed when
  target authorization cannot be proven.
- write-target sandbox authorization fails closed and does not execute the
  command.
- `state.reconcile.ack` fails and cannot clear an unreconciled-human-write block.
- Intent declaration, lease acquisition, and lease refresh fail.
- Read, search, and diff actions are allowed.
- Future IDE human save gates warn the user and allow the save.
- Heartbeat, finalization, and observer events may be queued in a local outbox.
- Local outbox events cannot authorize writes, clear reconciliation blocks, or
  extend leases until synced through the state server.
- V1 does not allow cached write grace periods.

Local outbox entries should carry enough metadata to sync later:

```text
outbox_id
event_type
session_id
actor_id
workspace_id
sequence
created_at
payload
sync_status: pending | synced | failed
last_sync_attempt_at
sync_error
```

`outbox_id` is the idempotency key for sync. The state server must treat repeated
sync attempts for the same `outbox_id` as the same event. Pending entries should
sync in `sequence` order per session. Failed entries remain available for retry
and inspection.

## Human Save Gate Rules

Human save-gate events are advisory coordination evidence for future IDE
integration. They are not emitted by the current CLI, MCP, or HTTP
implementation.

- IDE open, selection, dirty-buffer, and save-completion events may update
  human `activity` records.
- A save attempt that conflicts with an active agent lease should produce
  `HumanSaveGateShown`.
- If the user explicitly continues the save, record `HumanSaveContinued`.
- A completed save should produce `HumanWriteObserved`.
- Human save decisions do not grant agent override authority.
- After `HumanWriteObserved` on a file with active agent work, later agent writes
  should be denied or warned until the agent refreshes state and reconciles the
  file.
- Read, search, diff, and sandboxed test actions remain allowed while a
  human write is unreconciled.
- `state.reconcile.ack` records reconciliation after the agent rereads the file
  and chooses `adopt`, `reapply`, `ask_user`, or `abandon`.

## Views

Expected materialized views:

- active sessions by workspace
- active leases by resource
- planned edits by workspace
- conflicts by session
- finalization summaries by session
- prompt context package for Codex hooks and MCP tools

The current server exposes context rendering but returns an empty package.
Store-backed prompt context packages are future hardening work.

Prompt context packages should support:

```text
mode: brief | detailed
resources: optional file or directory filter
status: clear | warn | blocked
prompt_text
sections:
  Blocking
  Required Next Action
  Warnings
  Nearby Activity
  Stale/Expired
```

Each item has structured fields:

```text
severity: block | warn | info
resource
summary
next_action
evidence
source_refs
```

`brief` mode is capped at 8 total bullets. `detailed` mode is capped at 20 total
bullets. `next_action` is required for `block` and `warn`.

Expired and finalized records may appear only under `Stale/Expired`. They are
handoff evidence, not live blocking state. Brief mode includes at most 3
resource-relevant stale/expired items. Detailed mode includes at most 10,
resource-relevant first. Expired active records without finalization are shown
for 24 hours as `final status unknown`. Finalized `done` records are shown only
when resource-filtered or directly relevant. Finalized `failed` and `blocked`
records are shown for 7 days when resource-relevant.
