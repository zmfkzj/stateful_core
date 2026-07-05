# Current-State Coordination

This page is a rationale and index for current-state coordination. The shipped
contract details are authoritative in [README](../README.md),
[Implementation Contract](implementation-contract.md), [State Model](state-model.md),
and [Architecture](architecture.md). When this page says `target`, `future`, or
`should`, treat it as direction rather than shipped behavior unless the canonical
docs above say otherwise.

## Shipped v1 vs Target

Use this table as a quick boundary; the linked canonical docs remain authoritative.

| Capability | Status | Safe to rely on today | Canonical detail |
| --- | --- | --- | --- |
| Exact file/directory scope rules | Shipped v1 | Enforcement mode uses exact file scopes for exact file writes; directory scopes authorize only exact `write_directory` for the exact directory resource and do not authorize child file/delete/rename/move actions. Awareness mode renders the same scope problems as warnings unless a thin safety stop applies. | [State Model](state-model.md), [Implementation Contract](implementation-contract.md), [Usage Reference](usage-reference.md) |
| Coordination modes | Shipped v1 | `stateful server --coordination-mode enforcement|awareness`; enforcement is the default, awareness warns for broad reservation/claim/phase denials and suppresses wait-queue side effects. | [State Model](state-model.md), [Implementation Contract](implementation-contract.md), [Usage Reference](usage-reference.md) |
| FIFO single-path `write_file`/`write_directory` queueing | Shipped v1 | Enforcement mode wait requests store one path and schedule only `write_file` or `write_directory`; awareness mode warning outcomes do not enqueue waiters. | [Implementation Contract](implementation-contract.md), this page's Wait Queue section |
| Lazy resume notifications | Shipped v1 | Poll/SSE/`resume next` expose claimable reservations; OMP lazy edit/write resume can replay captured writes after claiming and rereading. | [Implementation Contract](implementation-contract.md), [Usage Reference](usage-reference.md) |
| Human observation, save-check, and reconciliation | Shipped v1 | `stateful human observe`, `human save-check`, and `reconcile ack` record high-confidence human writes, warn humans before saves that overlap active claims/fences, and deny later agent writes until `adopt` or `reapply` reconciliation succeeds. | [State Model](state-model.md), [Implementation Contract](implementation-contract.md), [Usage Reference](usage-reference.md) |
| Active write fences | Shipped v1 | Supported writes acquire short-lived write fences at the mutation boundary; another agent targeting the same path gets `write_fence_conflict`, denied in both enforcement and awareness. | [State Model](state-model.md), [Implementation Contract](implementation-contract.md) |
| Cross-workspace repo warnings | Target | Local v1 coordinates one workspace boundary; repo-relative warnings across workspaces, worktrees, or branches are future behavior. | [State Model](state-model.md), [Architecture](architecture.md) |
| Phase-aware authorization | Shipped v1 | Enforcement denies blocked/finalized phases; awareness warns for phase denials. Finalization or expiration ends authority. | [State Model](state-model.md), [Implementation Contract](implementation-contract.md) |
| Multi-resource queueing | Target | Shipped scheduling is single-path; atomic all-or-nothing multi-resource queueing is target-model behavior. | [State Model](state-model.md), [Implementation Contract](implementation-contract.md) |
| Rename/move queueing | Target | `rename_file` and `move_file` are immediate authorization actions requiring exact source and destination scopes/claims in enforcement; awareness can warn for broad claim/scope issues, but conflicting rename/move queueing waits for the multi-resource scheduler. | [Implementation Contract](implementation-contract.md), [Usage Reference](usage-reference.md) |
| Presence-first advisory reservations | Shipped as awareness mode | Awareness mode is the shipped presence-first arm: warnings before broad blocking, with hard stops retained for data-loss edges such as stale observations, unreconciled human writes, and active write fences. Enforcement remains available and is still the default. | [ADR 0002](adr/0002-presence-first-not-lock-first.md) |

## Problem

Coding agents are usually bounded by their own session. A session can know its
conversation, tools, and local context, but it cannot reliably see:

- what another session is doing now
- what another agent plans to modify next
- whether a human is already editing the same area
- whether a stale memory reflects current work or old work

Memory helps recover prior information. It does not provide a live view of
neighboring activity. The missing capability is observation of the present.

## Thesis

`stateful_core` should model "current state" as a first-class coordination
surface.

Current state is not raw logs and not long-term memory. It is a scoped,
time-bound summary of what an actor is doing now, what it plans to do next,
and which files, tasks, or resources are likely to conflict.

```text
memory = past evidence and recall
current state = active, expiring operational truth
```

The goal is to let an agent reason like this before writing:

```text
Another actor is nearby in auth validation.
My view of auth.ts is stale, and the prior session left a blocked handoff.
I should reread, choose unrelated work, or ask the orchestrator/human.
```

Do not use rendered coordination context (`context_render`) as a task scheduler.
Use it as a scoped write-time briefing: what nearby work is active, what changed
since the actor last looked, and whether to reread, wait, or proceed. Task
allocation belongs to the orchestrator or human; merge-time integration belongs
to Git ([ADR 0002](adr/0002-presence-first-not-lock-first.md)).

## Expected Value

The primary value is safer concurrency.

Current-state coordination can reduce:

- overlapping edits to the same file
- duplicated investigation
- accidental rollback of another actor's changes
- stale assumptions after a session is interrupted
- hidden contention between agents and humans

It also improves handoff quality because a stopped or blocked session leaves a
structured status rather than only an implicit transcript.

## State Shape

An agent activity state should be compact, fresh, and directly useful for
coordination.

```text
agent_id
workspace_id
actor_id
actor_type: agent | subagent | human | system
owner_id
parent_agent_id
parent_actor_id
workspace
branch
purpose
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

The shipped phase vocabulary matches the policy enum. `idle` and `expired` are
context or status labels, not activity phases.

The important fields are not only current edits. `next_plan` is often more
valuable than `files_editing`, because it allows other actors to avoid future
conflicts before they happen.

## Freshness Model

Current state decays quickly. Every active state must therefore include:

- `last_updated_at`
- `expires_at`
- `phase`
- `confidence`
- a source or trigger reference

When `expires_at` passes, the state is no longer treated as authoritative
current truth. It may still be retained as historical evidence, but conflict
checks should label it as stale or expired.

Heartbeat updates keep active work fresh. Missing heartbeats should not be
treated as success. They should move the state toward `unknown`, `idle`, or
`expired` depending on policy.

Reservation, claim, claimable-reservation, heartbeat, and retention defaults are
canonicalized in [Canonical Freshness Defaults](#canonical-freshness-defaults).

## Protocol

The coordination protocol should be explicit:

1. register session
2. choose server `coordination-mode` (`enforcement` default, `awareness` warn-first)
3. declare reservation
4. acquire advisory claim when the mode or workflow requires it
5. send heartbeat while active
6. update phase as work changes
7. observe tool effects and acquire/release write fences around writes
8. observe human saves or dirty/presence signals
9. reconcile high-confidence human writes before resuming affected agent edits
10. finalize as done, failed, or blocked
11. release or expire claim
12. create claimable reservations for released resources with queued waiters
13. notify sessions with claimable reservations so they can resume

Both start and end matter. Start-only reporting creates stale blocking state. End-only
reporting fails to prevent conflicts while work is happening.

## Wait Queue and Resume

Conflict handling is resource-scoped. If one session has an active claim for a
file, other sessions are not globally blocked. They can continue reading,
searching, validating, or editing unrelated files. In enforcement mode, only
writes that target a resource with an active claim are denied; in awareness mode,
the same overlap is warning context unless a thin safety stop also applies.

When an enforcement-mode denied write asks to queue on conflict, the state server
records a waiter for the normalized workspace resource:

```text
workspace_id + relative_path + action
```

Awareness-mode broad reservation/claim denials become warnings and do not create
waiters. The wait queue is FIFO. A queued hard-conflict request is promoted only
when the requested resource is available. The shipped queue stores one requested
path per wait request, and `/v1/reservation/request` accepts only `write_file`
and `write_directory` scheduling requests.

The target multi-resource reservation model is atomic all-or-nothing: if one
file or directory in a multi-resource request is still blocked, the whole
request stays queued. For a multi-resource
request, "available" means the request is at the head of every resource queue it
participates in and every requested resource has no active claim.

`rename_file` and `move_file` remain immediate authorization actions only in the
shipped v1 policy. They require exact source and destination file reservation and
claims, but conflicting rename/move attempts do not enqueue wait records until
the multi-resource scheduler is implemented.

The server promotes every eligible queued waiter when one of these grant
triggers makes the requested resource available:

- explicit claim release
- session or activity finalization
- claim expiry

Each promoted waiter receives a short claimable reservation (the shipped API
state is `reserved`). Claimable reservations prevent a later session from taking
the same resource ahead of an earlier conflicting waiter, but they are not active
write authority. The promotion records a pending notification payload containing
the wait queue row's stored, non-empty purpose. The agent with the claimable
reservation must reread the target. Manual native-tool or CLI flows then call
`state_reservation_claim(reservation_id=<reservation_id>, wait_id=<wait_id>)` or
`stateful reservation claim --reservation-id <reservation_id> --wait-id <wait_id>` before writing;
the claim uses the stored reservation purpose, and clients do not provide a new
claim purpose. Unclaimable claim responses use `reason_code`
`reservation_queued`, `reservation_expired`, `reservation_owner_mismatch`, or
`reservation_not_found`; existing waits also include `reservation_status`.
Native edit hooks and sandbox `write-targets` authorization can lazy-claim the
claimable reservation when the write is retried. Claiming creates active
reservation scope and the active same-reservation claim. Claimable reservation
expiry uses [Canonical Freshness Defaults](#canonical-freshness-defaults); after
expiry, the server may promote the next eligible FIFO waiter.

Resume is notification-driven rather than process-driven. The state server
records a pending notification with the stored purpose and a monotonic
per-target-agent/workspace `sequence` when it promotes a claimable reservation.
Agents and orchestrators discover that signal by polling notifications,
subscribing to the SSE stream, asking for the next claimable reservation, or
receiving that context from a lifecycle hook. Polling returns each pending
notification once and marks returned notifications delivered. The SSE stream
sends each notification as `id: <sequence>` with the same `sequence` in the JSON
event data, and stream delivery alone does not mark the notification delivered.
On reconnect, `Last-Event-ID` / `last-event-id` acknowledges all notifications
through that sequence, then the server replays later pending notifications.
Callers that miss or discard a poll response or SSE event should use
`stateful resume next` / `state_resume_next` to rediscover any still-active
claimable reservation and its stored purpose. OMP lazy edit/write resume checks
notifications/resume state for the saved `wait_id` before claiming. The state
server does not wake a
sleeping Codex process by itself; external orchestration can build on the
notification and resume APIs.

Full scheduling works through immediate request/response plus replayable
notification delivery. Reservation request APIs return `queued`, `reserved`
(claimable reservation), `claimed`, `canceled`, or `expired` state without
blocking indefinitely. Immediate availability creates a claimable reservation
with API state `reserved`; the agent must still reread the target. Manual
native-tool or CLI flows claim with
`state_reservation_claim(reservation_id=<reservation_id>, wait_id=<wait_id>)` or
`stateful reservation claim --reservation-id <reservation_id> --wait-id <wait_id>` before retrying;
the server uses the stored reservation purpose. Native edit hooks and sandbox
`write-targets` authorization can lazy-claim at the retry write boundary.
Waiting is handled by polling `stateful notifications poll`, subscribing to the
SSE stream, or using `stateful resume next`; all reservation surfaces expose the
stored purpose.
`stateful reservation wait --timeout` is not part of the v1 hardening implementation.

`request_id` and non-empty `purpose` are required for idempotency and live state
explanation. Reservation declaration also requires non-empty `files_planned`; empty
arrays and empty or normalized-empty paths are rejected with `missing_scope`.
Reservation request also requires a non-empty `path`; empty or normalized-empty
request paths are rejected with `missing_scope`. Repeating the same request id
returns the existing request state and must not create duplicate queue entries or
replace the original purpose. Repeating an expired request requeues the same
waiter in place, preserving its original FIFO row while requiring a new
reservation and claim before writing. A queued or claimable (`reserved`) request can be
canceled explicitly with `state_reservation_cancel` /
`stateful reservation cancel --request-id <id>`. Stop or activity finalization
releases that agent's active claims, cancels that agent's queued and
claimable (`reserved`) requests, and promotes the next eligible waiter for any
released or canceled resource.

## Coordination Modes

The system should not rely only on prompt instructions such as "remember to
update state." That is too weak. Sessions can be interrupted, tools can fail,
context can be compacted, and models can forget.

Instead, important actions should pass through a state protocol:

```text
before important action -> authorize against current state
after important action  -> observe and update current state
before active agent stops    -> require final status
```

`enforcement` is the default shipped mode: supported writes are blocked unless
the active agent has active reservation scope and, where required, an active
same-reservation claim. `awareness` is the shipped presence-first mode: broad
reservation, claim, scope, phase, and active-claim conflicts warn instead of
blocking and do not create wait-queue side effects. Thin safety stops still deny:
unsupported actions, stale base or claim-time observations, unreconciled human
writes, and active write-fence conflicts.

## Agent Runtime MVP

The chosen MVP direction is:

```text
Codex lifecycle hooks and OMP extension hooks
+ native Stateful coordination tools
+ state server
```

This gives useful control without forking Codex or changing the default OMP
profile.

The runtime details are intentionally not repeated here. Use this page for the
current-state rationale and queue/resume shape; use the canonical docs for
shipped hook, tool, sandbox, API, and storage behavior:

- [README](../README.md) summarizes shipped status and operator-facing use.
- [Implementation Contract](implementation-contract.md) is the concrete v1
  source for installation, hooks, native Stateful tools, lazy resume,
  `write-targets`, OMP scoped UI grant behavior, API, storage, and tests.
- [State Model](state-model.md) defines coordination modes, active reservation,
  same-reservation claim, claimable reservation, `wait_id`, `reservation_id`,
  write fences, human observations, reconciliation, freshness, queues, and
  current-state views.
- [Architecture](architecture.md) defines the hook/native-tool/server split:
  adapters stay thin, the local HTTP state server owns policy, and native tools
  are API adapters rather than independent policy engines.

The important design point for this rationale: supported writes pass through an
authorization boundary before mutation, effects are observed afterward, and
runtime adapters should classify tools only enough to call the state server.
Lazy resume belongs at the retry boundary: a queued `wait_id` may become a
claimable reservation, but OMP resume first checks notification/resume state for
that saved id. Enforcement-mode write authority exists only after the target is
reread and an active same-reservation claim is created. Awareness mode
can warn instead of blocking at that broad coordination layer, while thin safety
stops still apply.

## Conflict Policy

Initial policy should prefer warnings before blocking, while preserving hard
stops at data-loss edges:

- `stateful server --coordination-mode enforcement`: default mode; broad
  reservation, scope, phase, missing-claim, reservation-conflict, and
  active-claim-conflict policy failures deny.
- `stateful server --coordination-mode awareness`: the same broad coordination
  failures warn and instruct the agent to review rendered context and reread
  before writing; these warning outcomes do not enqueue waiters.
- unsupported write action: deny in both modes
- stale base observation or changed claim-time file observation: deny in both
  modes because replaying a stale write risks data loss
- unreconciled high-confidence human write on the target: deny later agent writes
  until `state.reconcile.ack` / `state_reconcile_ack` records `adopt` or
  `reapply`
- simultaneous same-path active write fence by another agent:
  `write_fence_conflict`, deny in both modes
- supported write with no active reservation: deny in enforcement, warn in
  awareness
- supported write with only abstract task/test/port/migration reservation: deny
  in enforcement, warn in awareness
- supported write with expired reservation: deny in enforcement, warn in
  awareness
- supported write while phase is `blocked`: deny in enforcement, warn in
  awareness
- supported write after session finalization: deny in enforcement, warn in
  awareness
- supported write outside matching exact file scope or exact directory
  `write_directory` scope: deny in enforcement, warn in awareness
- Codex raw Bash or non-wrapper Bash: deny, including repo-external
  command-shaped work
- OMP built-in Bash may run only strict trusted `stateful sandbox run ...` and
  `stateful sandbox process find ...` commands after Stateful preflight;
  arbitrary raw Bash and Python/JavaScript/JS/Ruby/Julia eval-tool execution is
  denied at host approval and hook levels. Scoped external writes and
  repo-external native `edit`/`write` file targets auto-approve the Stateful OMP
  UI grant by default and ask only when `stateful.autoApprove: false` is
  configured.
- directory reservation and directory claim authorize only `write_directory` for
  the exact directory resource; they do not authorize `write_file`, delete,
  rename, or move actions on child paths
- `write_directory` requires exact directory scope, and the matching directory
  claim blocks the whole subtree in enforcement because command-shaped
  `--write-dir` execution receives writable access to that subtree
- delete without exact file scope: deny in enforcement, warn in awareness unless
  another safety stop applies
- rename or move without exact file scope for both source and destination: deny
  in enforcement, warn in awareness unless another safety stop applies
- active claim in the hard conflict domain by another actor: deny in enforcement
  unless the current agent has an explicit user override for that resource; warn
  in awareness
- same `workspace_id` and same normalized `relative_path`: shipped hard conflict
  domain for active claim, write-fence, human-write, and wait-queue checks
- same normalized `absolute_path`: target physical-file hard conflict domain
- same area planned by another actor: warning context
- same `repo_id` and same `relative_path` across different workspace, worktree,
  or branch: target soft repo-relative warning
- unknown repository identity: target behavior is to hard-block only on same
  normalized absolute path and render weaker matches as unknown-confidence
  context
- expired claim: allow but surface stale-state context
- test execution: allow only through trusted sandbox-run wrappers with
  authorized targets
- task, port, or migration resource conflict: warn or info only in v1
- `stateful human save-check`: warn a human before saving over active agent
  claims or write fences, but do not block the human save
- `stateful human observe --kind save|change|delete --confidence high`: record a
  human write; later agent writes to that file deny until reconciliation
- reads, searches, diffs, and sandboxed tests after human writes: allow


This avoids making the system too rigid while still preventing the highest-risk
collisions.

## Collision Domains

V1 should distinguish shipped physical write collisions from target
repository-relative coordination risks.

Shipped hard conflict domain:

```text
same workspace_id + same normalized relative_path
```

This represents the same workspace-relative file in the same enabled workspace.
Supported writes in this domain are denied when another actor has an active
write claim or reservation. `absolute_path` is part of the target physical-file
domain but is not populated for shipped claim conflict checks.

Target soft conflict domain:

```text
same repo_id + same relative_path + different workspace/worktree/branch
```

This represents likely future integration risk, not immediate file overwrite
risk. It should warn and add context, but should not deny by default. The shipped
authorization path does not yet emit warn-tier decisions for these matches.

Branch-aware warning:

```text
same repo_id + same relative_path + different branch
```

This should be rendered prominently because merge conflicts are likely, but it
should remain a warning unless a later policy explicitly promotes it.

If `repo_id` cannot be determined, the state server should only hard-block on
same normalized `absolute_path` once that target domain is populated. Any weaker
match should be rendered as unknown-confidence context.

## Human Activity

Human activity cannot be captured only through agent runtime hooks. The system
should also support external observation:

- git working tree diff
- filesystem watcher
- IDE integration
- pre-commit or pre-push hooks
- explicit human status updates

Human-derived state should carry lower confidence when inferred indirectly. For
example, a file watcher can say a file changed, but it cannot always know the
human's goal.

The current implementation ships explicit human observation and advisory
save-check surfaces, not a mandatory human save lock. `stateful human observe`
records watcher/IDE signals with kind `save`, `change`, `delete`, `presence`, or
`dirty` and confidence `high` or `low`. High-confidence save/change/delete
observations emit `HumanWriteObserved` unless attributed to an active agent write
fence; those observations block later agent writes to the file until
reconciliation. Low-confidence presence/dirty signals are coordination context.

### IDE Advisory Save Check

The chosen IDE direction is a soft save check. The VS Code integration can check
the state server before a save when the editor exposes the needed pre-save hook,
then warn without taking ownership of the human's decision.

The shipped VS Code extension:

- posts low-confidence open/presence and dirty observations
- calls `stateful human save-check` for save attempts
- warns the user when a save conflicts with an active agent claim or write fence
- allows the user to continue the save explicitly
- posts high-confidence save observations after completed saves
- fails open with a warning if the state server cannot be reached

Selected-file telemetry and broader arbitrary filesystem/editor watching remain
future work.

This is not a hard lock. Autosave, external processes, other editors, git
operations, and editor-specific save paths can bypass the extension. The state
server therefore treats IDE save-check data as high-quality coordination signal,
not as a security boundary.

### Agent Reconciliation After Human Writes

The current implementation emits `HumanWriteObserved` for high-confidence human
writes and denies later affected agent writes as `unreconciled_human_write`.
The block is not meant to stop investigation; the agent may still read the file,
search, inspect diffs, and run sandboxed tests.

To resume writing, the agent must acknowledge reconciliation:

```text
state.reconcile.ack / state_reconcile_ack
stateful reconcile ack --reservation-id <reservation_id> \
  --files-reread <path> --summary <text> \
  --decision adopt|reapply|ask_user|abandon
```

The acknowledgement must include:

- the active reservation id
- the affected file read after the human write
- a summary of the human change
- the next direction: `adopt`, `reapply`, `ask_user`, or `abandon`

`adopt` means the agent accepts the human change and continues from it.
`reapply` means the agent will reapply its planned change on top of the human
change. `ask_user` keeps writes blocked until the user gives direction.
`abandon` records that the agent will not resume the affected work and keeps
writes blocked for that file.

Even after reconciliation, writes still require active, unexpired reservation
with matching file or directory scope and, in enforcement mode, the required
same-reservation claim.

If the state server is unavailable, reconciliation cannot be acknowledged. The
agent may continue read/search/diff investigation, but writes stay blocked until
the server is reachable and `state.reconcile.ack` / `state_reconcile_ack`
succeeds.

## State Server Availability

V1 uses a split failure policy:

```text
agent write path: fail closed
sandbox write-target path: fail closed
reconciliation path: fail closed
human save path: fail open with warning
non-Bash read/search/diff path: allow
```

At the OMP adapter boundary, a stateful hook deny or unavailable result is
returned as block, not warning, regardless of OMP yolo metadata. After OMP
runtime identity and discovery succeed, authorization failures can surface as
structured block JSON. If runtime identity/discovery subprocesses fail first, the
OMP extension maps the non-zero failure to a hard block. Built-in Bash passthrough
is limited to strict trusted Stateful sandbox/process commands, and repo-external
command-shaped work must pass Stateful external grant checks. Arbitrary raw OMP
Bash plus Python/JavaScript/JS/Ruby/Julia eval-tool sandbox invocations are
denied.

When the state server is unavailable:

- supported writes are denied because active reservation, claim conflict, and
  reconciliation state cannot be proven
- Codex raw Bash and repo-internal Bash calls remain denied unless they use a
  strict trusted `<absolute-stateful-binary> sandbox run ... --command <cmd>`
  wrapper
- OMP built-in Bash passthrough remains limited to strict trusted Stateful
  sandbox/process commands; arbitrary raw Bash and Python/JavaScript/JS/Ruby/Julia
  eval-tool execution remains denied at host approval and hook levels
- sandbox-run wrappers that need authorization fail closed and do not run the
  command
- `state.reconcile.ack` fails and cannot clear an unreconciled-human-write block
- reservation declaration, claim acquisition, and claim refresh fail
- non-Bash read, search, and diff actions are allowed
- the IDE save gate warns the user but allows the human save to proceed
- hook and observer events that cannot be sent should be appended to a local
  outbox for later sync

The local outbox is append-only recovery evidence. It may hold heartbeat,
finalization, human activity, and observer events until the server becomes
reachable. It cannot authorize writes, clear reconciliation blocks, or extend
claims while the server is unavailable.

Outbox sync uses `outbox_id` as an idempotency key. Replaying the same outbox
entry must not create duplicate state events. Pending events should be synced in
local creation order per agent; failed entries stay in the outbox with failure
metadata for later retry or inspection.

V1 has no cached write grace period. A recent successful claim or reservation check is
not enough to continue writing while the state server is down.

## Known Limits

Agent runtime hooks are a practical guardrail, not a complete enforcement
boundary. They can intercept important supported paths such as shell commands,
`apply_patch`, and native Stateful tool calls, but they do not make the state
protocol a full security boundary. Post-action hooks also cannot undo side
effects that already happened.

For OMP, yolo metadata does not downgrade a stateful denial to a warning; deny
and unavailable-state decisions remain hard blocks.

IDE save interception has the same character. It can warn before many human
saves, but it cannot guarantee exclusive file ownership.

This is acceptable for the MVP because the goal is coordination, not sandbox
security.

## Fork Option

Forking Codex CLI remains a possible later step.

A fork would be justified if the MVP proves that state coordination is valuable
but hooks cannot enforce the needed coverage. In that case, the fork should add
thin runtime primitives rather than product policy:

```text
ToolGate.before(tool_call) -> state-server authorize
ToolGate.after(tool_result) -> state-server observe
TurnLifecycle.before_stop -> state-server finalize check
HeartbeatLoop -> state-server heartbeat
```

Target shared policy primitives should still live in `stateful_core` so the fork
stays small and easier to rebase. The shipped prototype keeps store-backed
authorization orchestration in `stateful-server::policy_service` and uses
`stateful_core` for protocol types and pure scope-policy primitives.

## Initial Decision

Build the first version with Codex lifecycle hooks, OMP extension hooks, native
Stateful coordination tools, and a `stateful_core` state server.

The v1 MVP includes Codex and OMP hooks, native Stateful tools, the state server,
sandboxed test execution, explicit reconciliation acknowledgements,
human-observation/save-check flows, awareness mode, and active write fences.

V1 is local-only. It coordinates one machine/workspace boundary. Team-shared,
cross-machine, or hosted state sync is deferred to v1.5/v2.

The project should keep the policy model portable across agent runtimes. The
shipped implementation remains Codex-first, with OMP extension support for the
runtime lifecycle events OMP exposes. Runtime-specific behavior belongs in
adapters, and broader non-Codex runtime support remains future work.

V1 defaults to strict enforcement. Supported writes and reconciliation fail
closed when state cannot be trusted. Usability comes from
clear denial messages and diagnostics, not from silent grace periods.

The shipped retention policy follows the canonical freshness/defaults below.
Expired state may be shown as handoff evidence until it leaves the retention
window, but retention does not extend live write authority.

The v1 hard block policy is:

```text
supported write action + no active reservation -> deny
supported write action + expired reservation -> deny as missing active reservation
supported write action + reservation without file/directory scope -> deny
supported write action + target outside reservation scope -> deny
Codex raw Bash -> deny; OMP built-in Bash -> allow only strict trusted
  `stateful sandbox run ...` and `stateful sandbox process find ...` commands
  after Stateful preflight; arbitrary raw OMP Bash and Python/JavaScript/JS/Ruby/Julia
  eval tools -> deny. Scoped external writes and repo-external native
  `edit`/`write` file targets auto-approve the Stateful OMP UI grant by default
  and ask only when `stateful.autoApprove: false` is configured.
delete action + non-exact file scope -> deny
rename/move action + non-exact source or destination scope -> deny
active write claim in hard conflict domain -> deny
```

For wait queue scheduling, the same hard conflict becomes a queued request when
the caller asks to queue on conflict. Queue promotion happens only after
explicit release, session or activity finalization, or claim expiry. Soft
repo-relative conflicts remain warning context in v1.

Explicit overrides are target policy and are not implemented in the current
server. When implemented, an explicit override can allow a direct write
authorization exception for the current agent and resource, but it must not
reorder existing waiters, steal a claimable reservation, or move the overriding
agent to the head of a queue.

Phase-aware authorization is shipped: enforcement denies blocked or finalized
phases, and awareness warns for those phase denials.

A v1 task reservation must be active, unexpired, belong to the same agent, and
include matching exact file or directory scope. Abstract task, test, port, or
migration reservation can be stored as context but does not permit writes.
Directory scope authorizes `write_directory` for the exact directory
resource; file writes, deletes, renames, and moves require exact file scope.

Resource authorization classes:

```text
file/directory -> write-authorizing
test -> sandboxed test coordination only
task/port/migration -> prompt context and warning only
```
Delete operations require exact file scope. Rename and move operations require
exact file scope for both source and destination.

### Canonical Freshness Defaults

Canonical freshness/defaults:

```text
default reservation TTL: 15 minutes
default claim TTL: 5 minutes
default claimable reservation TTL: 120 seconds
retention pruning window: 14 days for old events and expired/delivered notifications
heartbeat extension: shipped for explicit heartbeats and implicit authorize-time
heartbeat events on active unexpired reservation rows
maximum rolling reservation lifetime: 60 minutes from declared_at
activity phases: exploring | editing | testing | blocked | done | failed
context/status labels, not phases: idle | expired
phase-aware authorization: blocked/finalized phases deny in enforcement and warn
in awareness; expiration/finalization ends write authority
```

The shipped hook path records target existence and content hash when an exact
file claim is acquired with `root`, denies hook-originated native file writes
when that file changes before authorization, and releases the same-reservation claim
after a completed native edit or `write-targets` transaction. This is a
per-claim freshness check, not a filesystem watcher or IDE human-save observer.

Override policy:

```text
automatic override: never allowed
override authority: explicit user instruction only
scope: current agent + current turn + specific resource
applies to: active claim conflict only
does not bypass: missing reservation, expired reservation, target blocked/finalized state
does not bypass: file/directory scope matching
does not bypass: delete/rename/move exact-scope rules
example: "Allow override for src/auth.ts."
responsibility: user owns the judgment and risk
```

This override policy is future work until the server has an explicit override
record and authorization path.

Prompt context rendering:

```text
state_context_render(workspace, agent_id, resource?, mode)
mode: brief | detailed
sections: Blocking, Required Next Action, Warnings, Nearby Activity, Stale/Expired
```

`brief` is used for session start, user prompt context, and planning-time
known-target resource checks. `detailed` is used for manual deep inspection when
brief planning context lacks enough evidence. Rendering should be actionable,
not a raw event dump, and denial recovery should follow direct next-action
payloads instead of automatically rendering context.

The current server route renders store-backed live context from active reservations,
active claims, and queued or claimable (`reserved`) wait records. The response includes
summary counts, structured `items`, and prompt-ready `prompt_text`.

The shipped route returns structured data plus prompt-ready markdown:

```text
status: ok | error
mode: brief | detailed
current
items[]
prompt_text
```

A separate context-level `clear | warn | blocked` status is target model
vocabulary. The current route reports request success or failure in `status` and
leaves block/warn/info semantics on individual items.

Each rendered item uses:

```text
- [severity] resource: summary.
  next: concrete action.
  evidence kind: coordination signal classification.
  evidence: detailed mode only.
```

Severity values are `block`, `warn`, and `info`. `brief` mode has at most 8
bullets. `detailed` mode has at most 20 bullets. Empty sections are omitted
except `Blocking: None` when a resource filter is present. Raw event dumps are
forbidden.

Evidence kind distinguishes declared reservation, claim-only blockers, queue or
reservation state, observed writes, and verified diffs; `evidence` remains
optional supporting detail.

When the block is an unreconciled human write, `Required Next Action` should tell
the agent to reread the file, summarize the human change, decide whether to
adopt, reapply, ask the user, or abandon, and call `state.reconcile.ack`.

The renderer can show supplied expired and finalized state only in
`Stale/Expired`; the shipped store-backed route currently emits live reservations,
claims, and wait records only. Stale or finalized evidence never blocks a live
action by itself.

Target summary windows for future historical context:

```text
brief: max 3 stale/expired items, resource-relevant only
detailed: max 10 stale/expired items, resource-relevant first
expired active state without finalization: show for 24h
finalized done: show only when resource-filtered or directly relevant
finalized failed/blocked: show for 7d when resource-relevant
```

For expired active state without finalization, the renderer should say `final
status unknown`. For finalized work, it should include status, changed files,
remaining work, and age when available.

Do not fork Codex at the start. Use hooks to validate the protocol and identify
the exact enforcement gaps. Reconsider a fork only after those gaps are proven
by usage.

## References

- Codex Hooks: https://developers.openai.com/codex/hooks
- Codex Advanced Configuration, Hooks:
  https://developers.openai.com/codex/config-advanced#hooks
- Codex Configuration Reference:
  https://developers.openai.com/codex/config-reference#configtoml
