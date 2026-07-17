# ADR 0001: Current-state first, not memory-first

Status: Accepted

## Context

Memory can recall prior work, but it cannot reliably establish what another
agent or human is doing in the shared checkout now. Live coordination needs
fresh presence, read-time evidence, and a final handoff rather than a
best-effort reconstruction from transcripts or model memory.

The historical V1 proposal framed this need around long-lived reservations and
broad write authorization. That proposal is historical only. The accepted V2
runtime treats current state as a live, expiring operational view and retains a
canonical indefinite event journal for history.

## Decision

`stateful_core` is current-state first:

- **Presence** communicates active goal, phase, plan, resources, and nearby
  activity.
- **Freshness** is based on a complete exact read observation and is checked at
  the mutation boundary; it is not a claim hash or write-time provenance.
- **Handoff** is an explicit finalized outcome with useful result, next action,
  or blocker; cleanup activity is not a handoff.
- **Journal history** is serialized V2 evidence, including expiration and
  recovery, rather than an age-pruned substitute for live state.
- **Memory** may inform investigation but never authorizes a current write or
  makes stale activity fresh.

The runtime accepts the `stateful.v2` envelope on `/v2/**`. Context is rendered
as a versioned delivery and acknowledged explicitly, so delivery reflects
versioned state rather than a once-per-session prompt marker. Current state is
a briefing, not a task scheduler: an orchestrator or human assigns work and Git
integrates it.

## Concrete Implementation Evidence

The decision is implemented at head by these shipped surfaces:

- `crates/stateful-server/src/routes_v2.rs` routes session registration,
  presence updates, exact-read start/completion, write completion/recovery,
  finalization, context rendering/acknowledgement, `/v2/current`, and
  `/v2/events`.
- `crates/stateful-cli/src/hook.rs` posts V2 session, presence, exact-read, and
  finalization lifecycle records and renders then acknowledges context.
- `crates/stateful-core/src/freshness.rs` models stable, missing, expired,
  changed, and unstable read observations. The V2 authorization path treats
  non-stable or incomplete evidence as missing, while a changed previously
  stable exact observation requires reread in either mode.
- `crates/stateful-core/src/journal.rs` defines the serialized V2 event
  families, including `Handoff`, `Context`, `HumanAcknowledgement`, and
  `Recovery`.
- `crates/stateful-cli/tests/v2_two_session.rs` exercises V2 presence,
  exact-read, context, human-change, and finalization behavior between two
  sessions.

## Consequences

Positive:

- Operators can see nearby work and handoffs without mistaking old memory for
  live authority.
- Exact rereads repair stale evidence at the resource boundary.
- The event journal supplies durable audit and recovery evidence without making
  expired presence block new work.

Negative:

- Presence is only useful when integrations publish and agents use it.
- Current state does not allocate tasks, merge branches, or replace source
  control review.
- A context briefing can be stale by the next write, so the write boundary still
  checks freshness and the narrow safety conditions.

## Boundary Rule

```text
Memory recalls the past.
Current state coordinates the present.
Only fresh exact evidence may support a live write decision.
```
