# State Model

This is the shipped V2 model. The append-only event journal is canonical; the
current tables are projections for live coordination. There is no alternate
active model.

## Live Coordination State

A workspace has an ordered journal sequence and projected records for:

- **presence** — actor identity, phase, goal excerpt, resources, recent tool
  activity, timestamps, and expiry;
- **reservation and claim** — declared intent and currently active resource
  claims, including waits that can become claimable;
- **read observation and resource version** — a complete exact read, its stable
  before/after evidence, freshness, and the target's current version;
- **write intent and write fence** — the in-flight write boundary and whether
  its result is committed, failed, unknown, or reconciled;
- **human observation and acknowledgement** — observed high-confidence human
  changes and the reconciliation acknowledgement that follows an exact reread;
- **handoff** — explicit final status/summary, changed files, tests, remaining
  work, and fallback status when a session stops without an explicit handoff;
- **context delivery and notification** — per-agent cursor, delivery id,
  sequence, acknowledgement, and replayable notification state; and
- **command receipt** — idempotent request outcome.

Presence is current only while it is fresh. Handoffs are retained as projected
context for their relevance window (explicit handoffs longer than fallbacks),
but their journal events are retained indefinitely. A fallback is intentionally
marked as fallback; cleanup activity does not qualify as a handoff.

## Exact Read, Write, and Reconciliation

A `ReadObservation` starts and completes for one exact target. It is usable
only when complete, stable, fresh, and matched to the supplied write target's
`before` observation and current resource version. A claim's legacy hash is not
read evidence.

A write authorization evaluates these facts before broad coordination mode:

```text
invalid target                    -> deny
another unknown write outcome     -> deny
changed exact read              -> deny
active same-target write fence  -> deny
unreconciled human write        -> deny
non-stable/missing/expired read -> warn in awareness; deny in enforcement
```

Reservation and claim failures are advisory warnings in awareness and denials
in enforcement. In either mode, a client completes the write with
`/v2/write/complete` or uses `/v2/write/recover` to reconcile an interrupted
outcome. A high-confidence human observation must be reread exactly and
acknowledged with `/v2/reconcile/ack` before a later write may proceed.

## Context and Handoff State

Context is a versioned workspace view, not a once-per-session message. Rendering
from an agent cursor returns unchanged context or creates a `Context` delivery
with `delivery_id`, `sequence`, and `workspace_version`. The recipient sends all
three to `/v2/context/ack`; only that acknowledgement advances the delivery
cursor. If it is not acknowledged, the context can be delivered again.

An explicit handoff records `done`, `failed`, `blocked`, or `unknown` status
with a required summary and optional files, tests, remaining work, resources,
goal, and next plan. Finalization without that input creates a fallback handoff
that says what is known without fabricating results.

## Serialized Event Schema

`EventPayload` is serialized with `family` and `event` tags in snake case. The
following Rust family and variant names are the complete public serialized V2
event schema, cross-checked against `crates/stateful-core/src/journal.rs`:

```text
Migration:
  Started, LegacyAuditImported, PresenceSnapshotSeeded,
  ReservationSnapshotSeeded, ClaimSnapshotSeeded, WaitSnapshotSeeded,
  WriteFenceSnapshotSeeded, HumanObservationSnapshotSeeded,
  LegacyHandoffSnapshotSeeded, DeliverySnapshotSeeded, Validated, Completed
Presence:
  Registered, Heartbeat, GoalUpdated, PhaseUpdated, PlanUpdated,
  ResourcesUpdated, ToolStarted, ToolCompleted, Finalized, Expired
Reservation: Declared, Refreshed, Released, Expired
Claim: Acquired, ObservationRefreshed, Released, Expired
Wait: Requested, BecameClaimable, Claimed, Cancelled, Expired
WriteFence: Acquired, ConflictObserved, Released, Expired
ReadObservation: Started, Stabilized, Unstable, Aborted, Invalidated, Expired
WriteIntent: Started, Committed, Failed, OutcomeUnknown, Reconciled
HumanObservation: Observed, Reconciled, Expired
HumanAcknowledgement: Recorded
Handoff: Finalized, Expired
Authorization: Allowed, Warned, Denied, OverrideGranted
Context: Rendered, DeliveryCreated, DeliveryAcknowledged, DeliverySuperseded
Notification: Created, Delivered, Expired, Coalesced
Recovery: Queued, Attempted, Delivered, Failed
```

In wire form these use snake case, for example `human_acknowledgement` /
`recorded` and `recovery` / `attempted`. `HumanAcknowledgement::Recorded` and
`Recovery::Attempted` are part of the live schema and must remain in clients,
allowlists, and documentation.

## Migration Projection Rule

V2 migration appends `Migration` lifecycle and seed events rather than treating
legacy tables as a second canonical model. The projector applies every accepted
event into current tables. Migration validates replay against shadow projections
before cutover, so a projection can be rebuilt from the canonical journal.
