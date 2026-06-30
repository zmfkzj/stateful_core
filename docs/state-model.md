# State Model

This document defines the v1 current-state coordination model.

## State Kinds

The model is intentionally narrow:

- `session`
- `activity`
- `reservation`
- `claim`
- `conflict`
- `finalization`
- `override`

These state kinds describe active coordination, not general-purpose memory or
all durable agent facts.

## Goal, Turn, Task, and Reservation Boundaries

V1 separates the user's work objective from the authorization resource.

`goal` is the user-visible objective that may span multiple agent turns. It is
used for grouping, handoff, summaries, and context rendering. It does not
authorize writes.

`turn_id` identifies one agent execution slice inside a session: one prompt,
the tool calls that follow, and the final response, pause, or stop. V1 records
turn identity where available, but write authorization is reservation-scoped:
active `reservation_id` scope plus an active same-reservation claim.

`reservation` is the task-level write scope. A reservation belongs to one
session, expires, and groups the known file or directory scopes for one task
purpose under a stable `reservation_id`. Agents should declare the complete known
file set for a task, then add scopes to that same reservation as the target set
grows.

`claim` is the live resource-level ownership signal. Each supported write still
requires a fresh same-reservation claim for the exact file or directory resource
being mutated.

The practical hierarchy is:

```text
session
  goal
    turn
      reservation_id
        file/directory scopes
          resource claims
            write actions
```

For v1, Codex hooks must treat the current Codex hook `thread_id` as
authoritative when present, falling back to `session_id` for older payloads. The
OMP extension must store `process.env.STATEFUL_SESSION_ID` from explicit OMP
event/ctx ids (`event.sessionId`, `event.session_id`, session fields), falling
back to the `ctx.sessionManager.getSessionFile()` header/stem and then
`getLeafId()` during `stateful hook omp session-start`. MCP
reservation declaration may omit `session_id`; in that case the adapter uses the
current session recorded by lifecycle hooks so MCP declarations keep the current
session as reservation owner while write authorization uses `reservation_id`.

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
phase: exploring | editing | testing | blocked | done | failed
files_read
files_editing
files_planned
resource_claims
next_plan
summary
last_result
last_updated_at
expires_at
confidence
source_refs
```

The shipped phase vocabulary matches the policy enum above. Expiration is
represented by `expires_at`, record freshness, and status fields rather than a
separate `phase = expired`; `idle` is target vocabulary for future activity
summaries.

`next_plan` is part of the core model because planned work is often the best
early signal for avoiding conflicts.

## Claim Record

A claim is an advisory claim on a resource.

```text
claim_id
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

V1 claims are coordination signals. They are not hard distributed locks and do
not replace source-control review. Only `file` and `directory` claims can
authorize supported write actions in v1.

Claim freshness defaults to `claim_ttl_seconds = 300`. Heartbeats refresh active
claim expiry while the owning session remains active. Missing heartbeats do not
mean success; they eventually make the claim eligible for expiry and FIFO queue
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

The shipped hard conflict domain is the same `workspace_id` and same normalized
`relative_path`. This approximates two actors touching the same file inside the
same enabled workspace. The target physical-file domain also records and compares
same normalized `absolute_path`.

The target soft conflict domain is the same `repo_id` and same `relative_path`
across different workspaces, worktrees, or branches. This should not block by
default, but should produce warning context because later merge or integration
work may conflict. The shipped authorization path does not yet emit this warn
tier.

If repository identity is unknown, the target model should only hard-block on
same normalized `absolute_path`; repo-relative matches with incomplete identity
should be surfaced as unknown-confidence warnings, not denials.

### ID Derivation

The shipped prototype derives enabled-repo identity conservatively from local
repo metadata:

```text
workspace_id = explicit non-default runtime workspace id when configured,
               otherwise "workspace-" + hash(canonical realpath of git
               worktree root). Default runtime ids `local`, `shared`, and
               `unknown` are treated as placeholders when repo identity is
               available.
repo_id = "repo-" + hash(canonical realpath of git worktree root)
worktree_id = repo_id
branch = symbolic branch name, or "unknown"
relative_path = normalized POSIX path relative to the git worktree root
absolute_path = canonical realpath when the target exists; otherwise canonical
                parent realpath + target basename
```

The target model should derive identity from machine/worktree and repository
identity rather than local path alone:

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

## Reservation Record

A reservation records the file set a session plans to touch for a task before it
performs important actions.

```text
reservation_id
session_id
purpose
goal
phase
files_planned
resources_planned
next_plan
declared_at
updated_at
expires_at
max_expires_at
status: active | superseded | completed | expired
```

Hooks must require an active reservation before allowing supported write tool paths.
For v1, a task reservation that authorizes writes must include a non-empty `purpose` plus at
least one file or directory scope in `files_planned`. `files_planned` is the
task's current known file set, not a single-file-only primitive; callers should
send all known task targets together and redeclare when the task expands. The
server rejects an empty `files_planned` array or empty or normalized-empty path
with `missing_scope`. Callers must infer purpose from the user or agent
instruction when it is not explicit; the server does not generate a fallback
purpose. Abstract resources in `resources_planned`, such as `task`, `test`,
`port`, or `migration`, can provide context but cannot authorize writes.

`phase` is shipped for activity records and write authorization. `goal`,
`resources_planned`, `next_plan`, and `max_expires_at` remain target model
fields. Shipped authorization is based on active, unexpired reservation scope
rows, active same-reservation claims, and active phases.

Reservation declarations add to the session's active reservation scope in that workspace. This
lets a session keep one reservation while adding a newly discovered source
file or `tmp/` build/test scope without invalidating existing claimed paths. If
the same path is declared again, the latest matching active declaration supplies
the purpose used for future claim acquisition.

## Reservation Scope Matching

All paths are normalized relative to the workspace root before matching.

File scopes authorize writes only to the exact same file:

```text
scope: src/auth.ts
target: src/auth.ts -> allow
target: src/session.ts -> deny
```

Directory scopes authorize `write_directory` actions only for the exact scoped
directory. They do not authorize `write_file`, delete, rename, or move actions
for child paths:

```text
scope: src/
action: write_directory target: src/ -> allow
action: write_directory target: src/auth/ -> deny
action: write_file target: src/auth.ts -> deny
```

`write_directory` is the only action backed by directory reservation and a matching
directory claim. The resulting directory claim fences the entire subtree because
command-shaped `--write-dir` execution can write anywhere below that directory.

Multi-file writes are allowed only when every file target has exact file scope
and every directory target has exact directory scope. Delete and rename/move
operations require exact file scope; directory scope does not authorize file
operations.

## Conflict Record

A conflict record is the target durable shape for a policy decision.

```text
conflict_id
session_id
target_resource
conflicting_session_id
conflict_type: missing_reservation | active_claim | planned_edit | stale_state | human_change | unreconciled_human_write | soft_repo_conflict | unknown_repo_identity | coordination_resource_overlap
severity: warn | block
decision: allow | warn | deny
reason
collision_domain: hard_workspace_path | soft_repo_relative_path | unknown
checked_at
source_refs
```

Conflict records are target audit artifacts. They also help render useful
context into later prompts. The shipped authorization path currently appends
`AuthorizationDenied` events for deny decisions instead of writing rows to the
`conflicts` table, and it does not emit warn-tier conflict records.

Write attempts without active reservation should be audited as missing-reservation denials.
In the shipped server this is an `AuthorizationDenied` event; the target conflict
table representation would use `conflict_type: missing_reservation`.

## Wait Queue Record

A wait queue record captures a hard-conflict reservation request that cannot be
reserved immediately.

```text
wait_id
request_id
session_id
workspace_id
repo_id
worktree_id
root
branch
purpose
relative_path
action: write_file | write_directory
requested_at
status: queued | reserved | claimed | canceled | expired
reservation_expires_at
blocking_session_id
```

The shipped queue is FIFO by insertion order (`rowid`). A queued request can be
promoted only when its requested resource is available. `purpose` is required
and remains the caller-supplied purpose from the original request.

The current implementation handles one requested path per wait request, and
explicit reservation requests accept only `write_file` and `write_directory`.
`rename_file` and `move_file` conflicts do not enqueue wait records until the
multi-resource scheduler is implemented.
Multi-resource `resources[]`, explicit `queue_sequence`, `blocked_by_claim_id`,
and recorded `grant_trigger` fields are target model for future hardening.

Promotion is triggered by explicit claim release, session or activity
finalization, claim/reservation expiry, or current-state materialization that
finds an already-unblocked queued waiter, but the current row does not persist
the trigger reason. Soft repo-relative conflicts do not create wait queue
records in v1.

Promotion creates claimable reservations, not active write authority. Each
waiting session must reread the target. Manual MCP/CLI flows then explicitly
claim the reservation with `state_reservation_claim(reservation_id=<reservation_id>, wait_id=<wait_id>)`
or `stateful reservation claim --reservation-id <reservation_id> --wait-id <wait_id>`;
native edit hooks and sandbox `write-targets` authorization can lazy-claim it at
the retried write boundary. Claiming creates active reservation scope and active
same-reservation claims.
The default claimable reservation TTL is 120 seconds. If a claimable reservation
expires, the server may promote the next eligible FIFO waiter.

Reservation notifications are delivery hints, not the durable reservation
record. `stateful notifications poll` / `state_notifications_poll` returns each
pending notification once and marks it delivered. If the client misses that
response, `stateful resume next` / `state_resume_next` can still rediscover the
claimable reservation until it is claimed or expires.

For target multi-resource requests, a request is eligible only when it is at the
head of every resource queue it participates in and every requested resource has
no active claim. A request that is first for one resource but blocked behind
another request on a second resource stays queued.

`request_id` is the idempotency key. Repeating a reservation request with the same
`request_id` must return the existing queue, claimable reservation, claim,
cancellation, or expiry state instead of creating a duplicate queue item.
Repeating an expired request requeues the same waiter in place, preserving its
original FIFO row while requiring a new reservation and claim before writing.

Queued or claimable (`reserved`) requests can be canceled explicitly. Session or
activity finalization releases active claims, cancels that session's queued and
claimable (`reserved`) requests, and promotes the next eligible waiter for any
released or canceled resource.

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
apply only to active claim conflicts. They cannot bypass missing reservation, expired
reservation, target blocked/finalized state, file or directory scope matching, delete
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

Subagent-specific `actor_type`, `parent_session_id`, and `parent_actor_id`
fields are protocol vocabulary for native subagent-aware adapters. The Codex
integration records each native subagent under its own effective session
identity when Codex exposes a thread id. Parent and child sessions coordinate
through the same workspace state, but a child does not inherit the parent's
same-reservation claim authority. Same-owner sessions do not receive automatic
override authority.

## Sandboxed Test Execution

Raw Bash test commands are denied by hooks. Agents run tests through the trusted
build wrapper with a scratch purpose, for example:

```text
stateful sandbox run --fs build --network enabled --write-dir test-run --command 'cargo test --workspace'
```

The build profile sets standard temp variables under
`/tmp/stateful/<session>/<scratch-purpose>/.stateful-tmp` and sets `CARGO_TARGET_DIR` to
the scratch `target` child. Other tool-specific build directories should be
configured under the same external scratch root.

Source-tree edits should use native edit tools with hook-visible targets, such
as Codex `apply_patch` or Edit, after task-level reservation covers the target
and a successful same-reservation file claim is active. Native edit hooks and
`sandbox run --fs write-targets` release their authorized same-reservation claims
after the write transaction completes; subsequent writes must reread and
reacquire a claim or claim a
claimable reservation. Command-shaped source writes must use exact
`--write-target` or `--create-target` entries, not the `tmp/` artifact directory
scope.

## Finalization Record

A finalization record closes out active work for a turn or session.

```text
session_id
status: done | failed | blocked
summary
files_changed
tests_run
remaining_work
released_claims
finalized_at
source_ref
```

The shipped `Stop` hook posts finalization for the session, which closes active
work and releases the session's claims.

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
blocked for the affected file and should release or shorten the affected claim.
Clearing the block does not authorize writes by itself; active, unexpired,
matching reservation is still required.

The current implementation records reconciliation acknowledgements but does not
yet emit `HumanWriteObserved` events or enforce unreconciled-human-write blocks.

## Events

The shipped event log is append-only audit evidence for coordination decisions
and lifecycle mutations. Current-state tables remain the active coordination
source for conflict checks. Event-backed materialization is used for accepted
session and reservation declaration events; other shipped lifecycle APIs update
materialized tables directly and append audit events in the same transaction.

The shipped server emits:

- `SessionRegistered`
- `SessionHeartbeat`
- `ReservationDeclared`
- `ClaimAcquired`
- `ClaimReleased`
- `ReservationRequested`
- `ReservationClaimed`
- `ReservationCanceled`
- `ActivityFinalized`
- `AuthorizationDenied`

The target model also includes these explicit coordination events:

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

Expiration may be driven by background TTL processing or by reads that discover
stale state. Target events above are not yet emitted by the current
implementation, including override and human save-gate events.

## Freshness Rules

Freshness is required for all active coordination records.

- Active activity and claims must have `expires_at`.
- Supported write actions require an active, unexpired reservation with matching
  file or directory scope.
- Default reservation TTL is 15 minutes.
- Heartbeats may extend active reservation TTL, but never beyond 60 minutes from
  `declared_at`.
- Shipped reservation authorization is based on active, unexpired scope rows. Expired
  rows are removed from the active policy state and deny as `missing_reservation`.
- Phase-aware authorization requires the latest activity phase to be
  `exploring`, `editing`, or `testing` when a phase is present.
- `phase = blocked`, `done`, or `failed` keeps activity visible but stops write
  authorization.
- Directory reservation scope authorizes `write_directory` only for the exact
  directory resource.
- Delete operations require exact file scope.
- Rename and move operations require exact file scope for both source and
  destination.
- Heartbeats extend active claims and activity records. The current
  implementation also refreshes active reservation expiry during `SessionHeartbeat`
  materialization, capped at 60 minutes from `declared_at`.
- Missing heartbeats do not imply success.
- Shipped finalization completes active reservations and appends a terminal activity
  phase, defaulting to `done` unless the request supplies another phase.
- Turn end expires unused overrides.
- Expired records remain historical evidence but stop blocking new work.
- Reads should distinguish fresh, stale, and expired state.

## Retention Rules

The shipped V1 retention policy prunes event and audit history older than 14
days. Retention affects historical evidence only; it does not extend live TTLs,
claims, reservations, or write authorization.

The current implementation expires live coordination rows through the state
server maintenance loop and lazily when policy reads detect stale state. The
maintenance loop also prunes old events, reconciliations, conflicts, human
observations, and expired notifications. It preserves active current-state rows,
pending notifications, and outbox sync evidence.

Projects may configure a longer retention window once runtime config loading for
retention ships. Shorter retention should be allowed only when the system can
still preserve required audit evidence for active conflicts, unreconciled human
writes, and pending outbox sync.

## Availability Rules

When the state server is unavailable, coordination must fail closed for agent
write authorization and fail open for human saves.

- Supported writes are denied.
- Codex raw Bash and Bash calls that are not a strict
  `<absolute-stateful-binary> sandbox run ... --command <cmd>` wrapper are
  denied, including repo-external shell work. Repo-external command-shaped writes
  must use `sandbox run --fs external --purpose ...`. Command-shaped repo
  writes through `--fs write-targets` fail closed when target authorization
  cannot be proven.
- write-target sandbox authorization fails closed and does not execute the
  command.
- `state.reconcile.ack` fails and cannot clear an unreconciled-human-write block.
- Reservation declaration, claim acquisition, and claim refresh fail.
- Read, search, and diff actions are allowed.
- Future IDE human save gates warn the user and allow the save.
- Heartbeat, finalization, and observer events may be queued in a local outbox.
- Local outbox events cannot authorize writes, clear reconciliation blocks, or
  extend claims until synced through the state server.
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
- A save attempt that conflicts with an active agent claim should produce
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
- active claims by resource
- planned edits by workspace
- conflicts by session
- finalization summaries by session
- prompt context package for Codex hooks and MCP tools

The current server exposes `/v1/context/render` and `state_context_render` as a
store-backed planning/manual inspection view over active reservations, active
claims, and queued or claimable (`reserved`) wait records. Responses include
current summary counts, structured `items`, and prompt-ready `prompt_text`; an
empty unfiltered live render means no planning context needs to be shown.

Prompt context packages should support:

```text
mode: brief | detailed
resource: optional file or directory filter
status: ok | error
prompt_text
sections:
  Blocking
  Required Next Action
  Warnings
  Nearby Activity
  Stale/Expired
```

The `status` field above is the shipped route response status. A separate
context-level `clear | warn | blocked` status is target model vocabulary; current
block/warn/info semantics live on individual items.

Each item has structured fields:

```text
severity: block | warn | info
resource
summary
next_action
evidence_kind: declared_reservation | claim_only | wait_queue | reservation | observed_write | verified_diff
evidence
source_refs
```

`evidence_kind` classifies the coordination signal behind the item, while
`evidence` is optional supporting detail. Prompt text includes `evidence_kind`
in both modes and includes supporting `evidence` text only in `detailed` mode.
`brief` mode is capped at 8 total bullets. `detailed` mode is capped at 20 total
bullets. `next_action` is required for `block` and `warn`.

The renderer can place supplied expired and finalized records only under
`Stale/Expired`, but the shipped store-backed route currently emits live
reservations, claims, and wait records only. Historical stale/finalized context
windows are target model behavior: brief mode includes at most 3
resource-relevant stale/expired items; detailed mode includes at most 10,
resource-relevant first; expired active records without finalization are shown
for 24 hours as `final status unknown`; finalized `done` records are shown only
when resource-filtered or directly relevant; finalized `failed` and `blocked`
records are shown for 7 days when resource-relevant.
