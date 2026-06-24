# Reservation-Scoped Concurrency Control

This document defines the coordination model behind `stateful_core`.

The short version:

```text
reservation graph
+ scoped claim
+ OCC-style revision/hash check
+ effect log
+ validation hooks
+ explicit override/audit path
```

This is a current-state concurrency coordination model for coding agents,
sessions, subagents, and humans working in the same repository. It is not a
collaborative text-editing engine.

## Problem

Multiple actors can work in the same workspace at the same time:

- a human editing a file in an editor
- a root agent changing implementation code
- a subagent checking tests or docs
- another session preparing a related edit
- a system runner producing artifacts

Git handles committed history and final merges. It does not prevent avoidable
collisions before a write happens. `stateful_core` fills that gap by answering:

```text
May this actor perform this action on this resource right now?
```

The decision uses current, expiring coordination state rather than long-term
memory.

## Non-Goals

`stateful_core` should not implement Operational Transformation or CRDTs for
source files.

OT and CRDTs solve a different problem: allowing independent replicas of the
same data structure to converge after concurrent edits. That is appropriate for
real-time document editors and local-first collaborative data structures. It is
not the right core primitive for coordinating coding agents in a repository.

The system also does not replace:

- Git branches, worktrees, review, or merge conflict handling
- containers, sandboxed clones, or orchestrators that can partition work cleanly
- editor ownership of human buffers
- security sandboxes or access-control systems
- OS advisory locks such as `flock` when the only problem is process-level
  mutual exclusion on one machine
- long-term memory or knowledge retrieval

The product boundary is narrower: prevent or surface risky current-state
collisions before supported actions mutate the workspace. Prefer the
alternatives above when actors can work independently and reconcile later.
Use reservation-scoped concurrency control when the actors must share one live
checkout because the environment is too expensive to duplicate, tasks need each
other's uncommitted edits immediately, or a human is editing in the canonical
workspace alongside agents.

## Model Name

The preferred name is:

```text
reservation-scoped concurrency control
```

It can also be described as:

```text
claim-backed optimistic coordination
```

The model combines pessimistic coordination at the narrow write boundary with
optimistic work outside that boundary. Actors can read, inspect, plan, and work
on unrelated files concurrently. Writes require active reservation, matching scope,
and a fresh claim or explicit policy path.

## Core Flow

The normal lifecycle is:

```text
Reservation Declaration
-> Claim Acquisition
-> Conflict Detection
-> Write Authorization
-> Effect Recording
-> Validation
-> Claim Release / Expiration
```

Reservation is the authorization scope. Claim is the live claim. The effect log is
the audit trail and target replay source; shipped v1 materializes selected
session and reservation events and appends audit events for lifecycle mutations.
Validation is post-effect evidence, not a substitute for authorization.

## Implementation Status

Existing v1 docs and code already cover the core reservation, claim, event log,
current-state view, wait queue, sandboxed write boundary, and audited policy
decision shape.

This spec also names target hardening work that can be added incrementally:

- base revision or hash observations for OCC-style freshness checks
- explicit claim fencing semantics where the active `claim_id` is treated as the
  current write token
- first-class validation records tied to reservations and resources
- explicit override records when the server exposes an override authorization
  path

Those target pieces should preserve the same model boundary: they strengthen
pre-write coordination, but they do not turn source files into CRDT or OT
documents.

## Reservation Graph

A reservation declares what a session plans to do before it performs important
actions. The graph connects user goals, sessions, turns, actors, and resources:

```text
goal
  session
    turn
      actor
        reservation
          resource scopes
```

Reservation answers:

```text
What may this session work on?
```

The graph matters because a single user goal can span multiple turns and a
session can spawn subagents. Subagents may contribute work, but they should not
gain broader write authority than the parent session's active reservation scope.

Reservation must be:

- explicit
- scoped to files or directories for write authorization
- attached to a purpose
- time-bound
- extensible when the active target set grows

Reservation declarations add to the active file scope for the session in the
workspace. If a session expands from `src/auth.ts` to `src/auth.ts` plus
`src/session.ts`, it may declare the new target without invalidating the
existing one. If the same path is declared again, the latest matching active
declaration supplies the purpose used for future claim acquisition.

## Scoped Claim

A claim declares that an actor currently holds a live claim on a resource.

Claim answers:

```text
Who is actively holding this now?
```

For v1, only file and directory claims authorize supported filesystem writes.
Other resources, such as tasks, tests, ports, and migrations, can provide
coordination context but do not authorize source-tree mutation by themselves.

Claims are advisory as product semantics, but checked where the runtime has a
reliable hook or sandbox boundary. This split is intentional:

```text
planning and context: advisory
write boundary: pre-tool authorization with observation checks
```

A write authorization validates the current same-session claim and rejects stale
claim or base-file observations when the hook can supply them. This catches the
common lost-update case where the file changed between claim acquisition,
reread, and write authorization.

Continuous post-authorization fencing is target behavior. The shipped v1 hook
does not yet pass a claim id through the native write and confirm after
`PostToolUse` that the same claim was continuously held until the write
completed. Long sandboxed commands are authorized before execution and are not
currently bounded by remaining claim TTL.

At authorization time, the policy engine checks:

- the target has active matching reservation
- the claim is active and unexpired
- the claim belongs to the current session or authorized actor path
- the target is inside the declared file or directory scope
- no stronger conflict has appeared since the claim was acquired

## OCC-Style Revision and Hash Check

Optimistic concurrency control belongs at the workspace boundary, not inside a
collaborative text data structure.

When a session reads, declares reservation, acquires a claim, or attempts a write, the
system can record a compact base observation for the target:

```text
repo head
index or worktree generation when available
target file content hash
target file existence
observed_at
observed_event_sequence
```

Before a write, the policy engine compares the current observation with the
actor's base observation and the effect log.

Expected same-session effects are allowed when they are recorded under the
active reservation and current claim. Unexpected effects produce a warning or denial:

- another actor wrote the target
- a human write was observed and not reconciled
- the file hash changed since the actor last read it
- the claim generation changed
- the session's reservation expired or was superseded

This is OCC-style because actors can work optimistically while reading,
planning, and editing in memory, but must prove that the target is still the
same coordination state before mutation.

## Effect Log

The append-only effect log is the canonical evidence stream for coordination.
It records facts such as:

- session registered
- reservation declared
- claim acquired or released
- conflict checked
- write attempted
- tool effect observed
- human write observed
- reconciliation acknowledged
- validation run completed
- override granted
- activity finalized

The materialized current-state view is derived from this event stream. Policy
checks should read the materialized view for speed, but audit, replay,
debugging, and recovery depend on the event log.

Effects are evidence, not authority. A previous successful write does not allow a
future write unless the session still has active reservation, matching scope, and a
fresh claim.

## Validation Hooks

Validation hooks run after effects or before high-risk lifecycle transitions.
They provide confidence that a change is acceptable, but they do not bypass
reservation or claim requirements.

Examples:

- run targeted tests after source edits
- run formatting or static checks through authorized sandbox targets
- require reconciliation after a human write
- require finalization before a session stops with active work
- block commit or push wrappers when required validation is missing

Validation records should include:

```text
validation_id
session_id
reservation_id
resources_checked
command_or_tool
started_at
completed_at
status: passed | failed | skipped | blocked
artifact_refs
summary
```

Validation failure usually blocks finalization, commit, push, or handoff. It
should not retroactively erase the effect log. Failed validation is still useful
coordination evidence.

## Explicit Override and Audit Path

Overrides must be explicit user decisions, scoped to a resource, and audited.
They are not inferred from ownership, confidence, urgency, or same-user
sessions.

An override can apply only to the conflict class it names. In v1, that means an
active claim conflict on a specific file or directory. It cannot bypass:

- missing reservation
- expired reservation
- finalized or blocked session state
- file or directory scope mismatch
- exact-scope requirements for delete, rename, or move
- unreconciled human writes
- wait queue ordering

Override records should include the user's instruction, resource, reason, turn,
expiration, and whether the override was used. The audit trail should make it
clear that the user accepted responsibility for that exception.

## Authorization Decision

The policy engine owns one decision shape:

```text
authorize_action(input) -> decision
```

The output should include:

```text
decision: allow | warn | deny | error
reason
conflicts[]
required_next_action
context_items[]
audit_event
```

The decision order is:

```text
1. Validate protocol and identity.
2. Classify action and write targets.
3. Require active, unexpired reservation for supported writes.
4. Match targets against file or directory scope.
5. Require active same-session claim for write-authorizing resources.
6. Check hard workspace conflicts.
7. Check OCC-style target freshness.
8. Check human-write reconciliation state.
9. Apply explicit override only if the conflict class permits it.
10. Record the conflict check and return the decision.
```

Reads, searches, and diffs can proceed without write reservation. Command-shaped
writes, test artifacts, delete, rename, move, commit, and push paths must use
the appropriate controlled action surface.

## Consistency Model

The consistency target is practical coordination, not replica convergence.

`stateful_core` aims for:

- fresh current-state visibility for active actors
- fail-closed write authorization when state cannot be trusted
- fail-open human saves with visible warnings
- auditable conflict decisions
- clear next actions for denied writes
- eventual expiry of stale claims

It does not guarantee that no external process can mutate the repository. When
external mutation is observed, the system records it, marks affected state as
stale or conflicted, and requires reread or reconciliation before further agent
writes.

## Wait Queue and Reservation

When a write conflicts with an active claim, the actor can request the resource
instead of spinning or overriding.

The scheduling flow is:

```text
blocked -> queued -> claimable_reservation -> claimed -> active
```

Queued requests are FIFO per resource. A claimable reservation (the current API
state is `reserved`) is not write authority. The owning session must reread the
target, claim the reservation, and then retry the write. The claim creates a
fresh write-authorizing reservation and claim for that resource.

This preserves ordering without letting a sleeping or stale session mutate the
workspace later on old state.

## Relationship to Existing Docs

This document defines the concurrency model at a product and policy level.
Detailed implementation rules live in the existing docs:

- [Core Concept](core-concept.md) defines current state and product boundary.
- [State Model](state-model.md) defines records, freshness, overrides, queues,
  and views.
- [Architecture](architecture.md) defines hooks, server, MCP, policy engine, and
  failure modes.
- [Implementation Contract](implementation-contract.md) defines concrete v1 API,
  CLI, storage, and test expectations.

When this document and a lower-level implementation document differ, treat this
document as the model reservation and the implementation contract as the current
shipping behavior. Changes that alter the model should update both layers.

## Invariants

- Reservation authorizes scope; claim represents active possession.
- A claim without matching reservation does not authorize a write.
- Reservation without a current claim does not authorize a write to a
  claim-required write-authorizing resource.
- Expired state is historical evidence, not live blocking authority.
- Same physical workspace path is the hard conflict domain.
- Same repo-relative path across worktrees or branches is warning context unless
  policy explicitly promotes it.
- Subagents inherit only the parent session's active valid reservation scope.
- Human writes are never discarded or blocked silently.
- Overrides are specific, temporary, audited, and user-owned.
- Validation evidence improves confidence but never replaces authorization.
