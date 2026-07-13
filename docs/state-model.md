# State Model

This document defines the v1 current-state coordination model, including the default `enforcement` mode and the presence-first `awareness` mode.

## State Kinds

The shipped live/current-state model is intentionally narrow:

- `agent`
- `activity`
- `reservation`
- `claim`
- `write_fence`
- `human_observation`
- `wait_queue`
- `notification`
- `outbox`

Target-only policy artifacts described later include durable `conflict` and
`override` records. Reconciliation is shipped as an acknowledgement API, an
event, and updates to `human_observation` rows rather than as a current-state
table.

## Goal, Turn, Task, and Reservation Boundaries

V1 separates the user's work objective from the authorization resource.

`goal` is the user-visible objective that may span multiple agent turns. It is
used for grouping, handoff, summaries, and context rendering. It does not
authorize writes.

`turn_id` identifies one agent execution slice inside a session: one prompt,
the tool calls that follow, and the final response, pause, or stop. V1 records
turn identity where available. Write behavior is coordination-mode dependent:
`enforcement` is the default and requires active `reservation_id` scope plus an
active same-reservation claim. `awareness` keeps the same reservation, claim,
phase, and overlap signals visible but converts broad coordination denials into
warnings without queue side effects. Unsupported actions, stale base or
claim-time observations, unreconciled human writes, and active write-fence
conflicts remain safety stops when those checks are reached.

`reservation` is the task-level write intent. A reservation belongs to one
session, expires, and groups the known file or directory scopes for one task
purpose under a stable `reservation_id`. Agents should declare the complete known
file set for a task, then add scopes to that same reservation as the target set
grows.

`claim` is the live resource-level ownership signal. In `enforcement`, each
supported write requires a fresh same-reservation claim for the exact file or
directory resource being mutated. In `awareness`, claim overlap is warning and
context input rather than a broad lock; short write fences protect the actual
mutation boundary.

The practical hierarchy is:

```text
agent_id + workspace_id
  goal
    turn
      reservation_id
        file/directory scopes
          resource claims
            write actions
```

For v1, agent-facing coordination identity is the active `agent_id` scoped by
`workspace_id`. Codex hooks and native tools inject that identity into hook and
state operations. OMP derives its Stateful `agent_id` only from
`ctx.sessionManager`: `getSessionId()` supplies the required session UUID and
the generated id is the session-stable `omp-${sessionId}`. Session leaf/branch
ids never enter the identity: the leaf advances on every appended entry, so a
leaf-derived id would churn per tool call and break notification targeting.
If `getSessionId()` is unavailable or invalid, OMP Stateful actions fail closed.
Agents do not repair current-session files, choose session environment
variables, read event/ctx agent or session fields, use process ids, or fall back
to runtime session aliases. Reservation declaration may omit low-level storage
identifiers when the runtime integration provides the active `agent_id` and
`workspace_id`; write authorization uses the `reservation_id` attached to that
identity.

## Activity Record

An activity record summarizes what one actor is doing now.

```text
agent_id
actor_id
actor_type: agent | subagent | human | system
owner_id
parent_agent_id
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
agent_id
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
not replace source-control review. In `enforcement`, only `file` and `directory`
claims can authorize supported write actions. In `awareness`, they drive
warnings and rendered context rather than broad denial.

Claim freshness defaults to `claim_ttl_seconds = 300`. Heartbeats refresh active
claim expiry while the owning agent remains active. Missing heartbeats do not
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

The shipped hard conflict domain is only the same `workspace_id` and same
normalized `relative_path`. In operator terms, two sessions in the same enabled
workspace writing the same normalized path conflict; the same repo-relative path
in another workspace, worktree, or branch does not hard-block in shipped v1.

Cross-workspace/repo identity handling is target warning behavior unless another
shipped contract says otherwise. The target soft conflict domain is the same
`repo_id` and same `relative_path` across different workspaces, worktrees, or
branches; it should warn because later merge or integration work may conflict,
but the shipped authorization path does not yet emit this warn tier.

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

A reservation records the file set an agent plans to touch for a task before it
performs important actions.

```text
reservation_id
agent_id
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

Reservation declarations add to the agent's active reservation scope in that workspace. This
lets an agent keep one reservation while adding a newly discovered source
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

`write_directory` is the only action backed by directory reservation and a
matching directory claim. The `sandbox run --fs write-targets --write-dir
<repo-dir>` target is repo-relative and requires that exact directory
reservation/claim coverage. The resulting directory claim fences the entire
subtree because command-shaped execution can write anywhere below that
directory.

Multi-file writes are allowed only when every file target has exact file scope
and every directory target has exact directory scope. Delete and rename/move
operations require exact file scope; directory scope does not authorize file
operations.

## Conflict Record

A conflict record is the target durable shape for a policy decision.

```text
conflict_id
agent_id
target_resource
conflicting_agent_id
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
`AuthorizationDenied` or `AuthorizationWarned` events for policy decisions
instead of writing rows to the `conflicts` table.

Write attempts without active reservation should be audited as missing-reservation denials.
In the shipped server this is an `AuthorizationDenied` event; the target conflict
table representation would use `conflict_type: missing_reservation`.

## Wait Queue Record

A wait queue record captures a hard-conflict reservation request that cannot be
reserved immediately.

```text
wait_id
request_id
agent_id
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
blocking_agent_id
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

Promotion is triggered by explicit claim release, agent or activity
finalization, claim/reservation expiry, or current-state materialization that
finds an already-unblocked queued waiter, but the current row does not persist
the trigger reason. Soft repo-relative conflicts do not create wait queue
records in v1.

Awareness mode does not create wait queue side effects when broad coordination
checks soften from deny to warn. Queueing remains an enforcement-mode recovery
path for hard active-claim conflicts.

Promotion creates claimable reservations, not active write authority. Each
waiting agent must reread the target. Manual native-tool/CLI flows then
explicitly claim the reservation with `state_reservation_claim(reservation_id=<reservation_id>, wait_id=<wait_id>)`
or `stateful reservation claim --reservation-id <reservation_id> --wait-id <wait_id>`;
native edit hooks and sandbox `write-targets` authorization can lazy-claim it at
the retried write boundary. Claiming creates active reservation scope and active
same-reservation claims.
The default claimable reservation TTL is 120 seconds. If a claimable reservation
expires, the server may promote the next eligible FIFO waiter.

Reservation notifications are delivery hints, not the durable reservation
record. Each pending notification has a monotonic `sequence` for its target
agent in that workspace. `stateful notifications poll` /
`state_notifications_poll` returns pending notifications and marks the returned
rows delivered. The SSE stream sends each notification as `id: <sequence>` with
the same `sequence` in JSON data, but stream delivery alone does not mark it
delivered. Reconnecting clients send `Last-Event-ID` / `last-event-id`; the
server marks notifications through that sequence delivered and replays later
pending notifications. If the client misses that response, `stateful resume next`
/ `state_resume_next` can still rediscover the claimable reservation until it is
claimed or expires.

For target multi-resource requests, a request is eligible only when it is at the
head of every resource queue it participates in and every requested resource has
no active claim. A request that is first for one resource but blocked behind
another request on a second resource stays queued.

`request_id` is the idempotency key. Repeating a reservation request with the same
`request_id` must return the existing queue, claimable reservation, claim,
cancellation, or expiry state instead of creating a duplicate queue item.
Repeating an expired request requeues the same waiter in place, preserving its
original FIFO row while requiring a new reservation and claim before writing.

Queued or claimable (`reserved`) requests can be canceled explicitly. Stop or
activity finalization releases active claims, cancels that agent's queued and
claimable (`reserved`) requests, and promotes the next eligible waiter for any
released or canceled resource.

## Write Fence Record

A write fence is the shipped short-lived mutation-boundary guardrail. It is not
a task-scale reservation and not a long-lived claim replacement.

```text
fence_id
agent_id
workspace_id
relative_path
action
acquired_at
expires_at
released_at
```

The server acquires write fences after reservation/claim policy, human-write
reconciliation checks, and base-observation checks allow a supported write.
Fences are acquired all-or-nothing across the normalized affected paths. The
same agent may refresh its own active fence; another agent targeting the same
path receives `write_fence_conflict`, which denies in both `enforcement` and
`awareness`.

Write-fence freshness defaults to 300 seconds. Native edit hooks and
`sandbox run --fs write-targets` release the fence when the write transaction
completes; session finalization also releases that session's active fences.
Expiry is a fallback for interrupted writers, not the normal completion path.

## Override Record

An override records an explicit user instruction to permit a blocked resource for
the current agent. Overrides are never inferred or granted automatically.
Override records are target model only; the current server does not expose an
override authorization path.

```text
override_id
agent_id
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
be scoped to a specific resource, current agent, and current turn. Overrides
apply only to active claim conflicts. They cannot bypass missing reservation,
expired reservation, target blocked/finalized state, file or directory scope
matching, delete exact-scope rules, or rename/move exact-scope rules.
Overrides do not reorder the wait queue, grant queue priority, or transfer a
reservation from one agent to another.

## Identity Rules

V1 identifies agent-facing work with explicit agent and actor fields:

```text
agent_id
turn_id
actor_id
actor_type: agent | subagent | human | system
owner_id
parent_agent_id
parent_actor_id
```

Subagent-specific `actor_type`, `parent_agent_id`, and `parent_actor_id` fields
are protocol vocabulary for native subagent-aware adapters. For OMP, each
session (including each in-process subagent session) uses the Stateful
`agent_id` derived from its own `ctx.sessionManager.getSessionId()`.
The Codex integration records each native subagent under its own
effective agent identity when Codex exposes one. Parent and child agents coordinate
through the same workspace state, but a child does not inherit the parent's
same-reservation claim authority. Same-owner agents do not receive automatic
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
as Codex `apply_patch` or Edit. OMP native `edit`/`write` predeclare/claim the
exact tool-visible file scope before first authorization for the default
simple-write path when no explicit reservation id is supplied; other native edit
paths require task-level reservation and an
active same-reservation file claim. Native edit hooks and `sandbox run --fs
write-targets` release their authorized same-reservation claims after the write
transaction completes; subsequent writes must reread and reacquire a claim or
claim a claimable reservation. Command-shaped source writes must use exact
`--write-target <file>`, `--create-target <file>`, or repo-relative
`--write-dir <repo-dir>` entries, not the `tmp/` artifact directory scope.
`--write-dir` requires matching `write_directory` reservation and
same-reservation claim coverage.

## Finalization Record

A finalization record closes out active work for a turn or session.

```text
agent_id
status: done | failed | blocked
summary
files_changed
tests_run
remaining_work
released_claims
finalized_at
source_ref
```

The shipped `Stop` hook posts finalization for the agent, which closes active
work and releases the agent's claims.

## Reconciliation Record

A reconciliation record acknowledges that an agent has reread and accounted for a
human write before resuming work on the affected file.

The shipped `/v1/reconcile/ack` API and `stateful reconcile ack` command record:

```text
agent_id
workspace_id
reservation_id
files_reread
human_change_summary
decision: adopt | reapply | ask_user | abandon
```

The native command-policy tool names are `state.reconcile.ack` and
`state_reconcile_ack`; use the exact name exposed by the active runtime. The CLI
shape is:

```text
stateful reconcile ack --files-reread <path> \
  --summary <text> --decision adopt|reapply|ask_user|abandon \
  --reservation-id <reservation_id>
```

The server requires a reservation id, non-empty `files_reread`, and active exact
file intent covering every reread file under that reservation. Only `adopt` and
`reapply` clear unreconciled-human-write blocks. `ask_user` records that the
agent needs user direction and keeps writes blocked. `abandon` records that the
agent will not resume the affected work and also leaves the block uncleared.
Clearing the block does not authorize writes by itself; active, unexpired
matching reservation and, in enforcement mode, same-reservation claim authority
are still required.

## Events

The shipped event log is append-only audit evidence for coordination decisions
and lifecycle mutations. Current-state tables remain the active coordination
source for conflict checks. Event-backed materialization is used for accepted
agent registration and reservation declaration events; other shipped lifecycle APIs update
materialized tables directly and append audit events in the same transaction.

The shipped store event log emits:

- `AgentRegistered`
- `AgentHeartbeat`
- `ReservationDeclared`
- `ClaimAcquired`
- `ClaimReleased`
- `ReservationRequested`
- `ReservationClaimed`
- `ReservationCanceled`
- `ActivityFinalized`
- `AuthorizationDenied`
- `AuthorizationWarned`
- `HumanWriteObserved`
- `ReconciliationAcknowledged`

The target model also includes these explicit coordination events:

- `HeartbeatObserved`
- `ToolUseObserved`
- `ConflictChecked`
- `OverrideGranted`
- `HumanActivityObserved`
- `HumanSaveGateShown`
- `HumanSaveContinued`
- `ActivityUpdated`
- `OutboxEventQueued`
- `OutboxEventSynced`
- `StateExpired`

Expiration may be driven by background TTL processing or by reads that discover
stale state. Target events above are not yet emitted by the current
implementation, including override and human save-gate events. `stateful human
observe` records human observations; a high-confidence `save`, `change`, or
`delete` not attributed to an active agent write fence emits `HumanWriteObserved`.
`stateful human save-check` returns `clear` or `warn` and does not emit
`HumanSaveGateShown` or `HumanSaveContinued`.

## Freshness Rules

Freshness is required for all active coordination records.

- Active activity and claims must have `expires_at`.
- Supported write actions require active, unexpired reservation/claim authority
  plus fresh base observations when the adapter can identify and read the target.
- Default reservation TTL is 15 minutes.
- Heartbeats may extend active reservation TTL, but never beyond 60 minutes from
  `declared_at`.
- Operator math: a long-running build/test can keep a reservation alive with
  heartbeats, but any write after the 60-minute cap needs a fresh reservation and
  same-reservation claim.
- Active claims expire after 300 seconds without heartbeat; if a waiter is
  queued, expiry can promote it.
- A promoted claimable reservation (`reserved`) expires after 120 seconds, so
  resume or lazy resume promptly or expect the next eligible FIFO waiter to move.
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
  implementation also refreshes active reservation expiry during `AgentHeartbeat`
  materialization, capped at 60 minutes from `declared_at`.
- Missing heartbeats do not imply success.
- Shipped finalization completes active reservations and appends a terminal activity
  phase, defaulting to `done` unless the request supplies another phase.
- Target override behavior expires unused overrides at turn end.
- Expired records remain historical evidence but stop blocking new work.
- Reads should distinguish fresh, stale, and expired state.

## Retention Rules

The shipped V1 retention policy prunes event and audit history older than 14
days. Retention affects historical evidence only; it does not extend live TTLs,
claims, reservations, or write authorization.

The current implementation expires live coordination rows through the state
server maintenance loop and lazily when policy reads detect stale state. The
maintenance loop also prunes old `events` rows and `notifications` rows whose
status is `expired` or `delivered`. It preserves active current-state rows,
pending notifications, and outbox sync evidence.

Projects may configure a longer retention window once runtime config loading for
retention ships. Shorter retention should be allowed only when the system can
still preserve required audit evidence for active conflicts, unreconciled human
writes, and pending outbox sync.

## Availability Rules

When the state server is unavailable, coordination must fail closed for agent
write authorization and fail open for human saves.

- Supported writes are denied because neither `enforcement` nor `awareness` can
  prove reservation state, freshness, human reconciliation, or write-fence safety.
- Codex raw Bash and Bash calls that are not a strict
  `<absolute-stateful-binary> sandbox run ... --command <cmd>` wrapper are
  denied, including repo-external shell work. Repo-external command-shaped writes
  must use `sandbox run --fs external --purpose ...`. Command-shaped repo
  writes through `--fs write-targets` fail closed when target authorization
  cannot be proven.
- write-target sandbox authorization fails closed and does not execute the
  command.
- `state.reconcile.ack` / `state_reconcile_ack` fails and cannot clear an
  unreconciled-human-write block.
- Reservation declaration, claim acquisition, and claim refresh fail.
- Read, search, and diff actions are allowed.
- Human save checks should warn the user and allow the save rather than block
  human work on server availability.
- Heartbeat, finalization, and observer events may be queued in a local outbox.
- Local outbox events cannot authorize writes, clear reconciliation blocks, or
  extend claims until synced through the state server.
- V1 does not allow cached write grace periods.

Local outbox entries should carry enough metadata to sync later:

```text
outbox_id
event_type
agent_id
actor_id
workspace_id
sequence
created_at
payload
sync_status: pending | synced | failed
last_sync_attempt_at
sync_error
```

`sync_status` defaults to `pending`. Legacy local outbox records without a
recorded `sync_status` are treated as pending so they remain eligible for
ordered sync rather than being skipped.

`outbox_id` is the idempotency key for sync. The state server must treat repeated
sync attempts for the same `outbox_id` as the same event. Pending entries should
sync in `sequence` order per agent. Failed entries remain available for retry
and inspection.

## Human Observation and Save-Check Rules

Human observation is shipped as advisory input plus a hard safety stop for later
agent writes that would overwrite unreconciled human changes. The CLI/HTTP
surfaces are:

```text
stateful human observe <path> --kind save|change|delete|presence|dirty \
  --confidence high|low --source <source> --summary <text>
stateful human save-check <path>...
stateful reconcile ack --files-reread <path> \
  --summary <text> --decision adopt|reapply|ask_user|abandon \
  --reservation-id <reservation_id>
```

- IDE open, selection, dirty-buffer, and save-completion sensors may update
  human `activity` or `human_observation` rows; low-confidence presence signals
  warn rather than deny.
- `human save-check` compares requested save paths with active agent claims and
  active write fences. It returns `decision: clear` or `decision: warn` with
  `conflict_kind: claim | write_fence`; it is advisory UX and should not prevent
  the human save.
- A high-confidence `human observe` with kind `save`, `change`, or `delete`
  records an unreconciled human write unless the write is attributed to an active
  agent write fence.
- After `HumanWriteObserved` on a file with active agent work, later agent writes
  are denied until the agent rereads and reconciles the file.
- Read, search, diff, and sandboxed test actions remain allowed while a human
  write is unreconciled.
- `state.reconcile.ack` / `state_reconcile_ack` records reconciliation after the
  agent rereads the file and chooses `adopt`, `reapply`, `ask_user`, or
  `abandon`.

## Views

Target materialized views:

- active agents by workspace
- active claims by resource
- planned edits by workspace
- conflict summaries by agent
- finalization summaries by agent
- prompt context package for Codex hooks and native Stateful tools

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

Active write-fence warning items must set a `next_action` that names both the
fenced path and the fence-owning agent.

The renderer can place supplied expired and finalized records only under
`Stale/Expired`, but the shipped store-backed route currently emits live
reservations, claims, and wait records only. Historical stale/finalized context
windows are target model behavior: brief mode includes at most 3
resource-relevant stale/expired items; detailed mode includes at most 10,
resource-relevant first; expired active records without finalization are shown
for 24 hours as `final status unknown`; finalized `done` records are shown only
when resource-filtered or directly relevant; finalized `failed` and `blocked`
records are shown for 7 days when resource-relevant.
