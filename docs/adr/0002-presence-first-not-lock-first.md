# ADR 0002: Presence-first, not lock-first

Status: Accepted

## Context

The coordination failures worth solving are information and freshness failures:
nearby work is invisible, investigation is duplicated, a human edit is missed,
or a stale read is replayed. A task-scale lock manager would make those
failures visible only by turning ordinary work into waiting.

The historical V1 design made enforcement the default and gave reservation and
claim machinery too much of the product narrative. That design is historical
only. V2 keeps denial-capable enforcement for workflows that explicitly choose
it, but makes awareness the shipped default.

## Decision

The product center is a live-checkout presence layer:

- show nearby presence, freshness, and handoff in rendered context;
- record reservations and claims as advisory intent in awareness;
- let enforcement, when explicitly selected, deny broad coordination overlap;
- require exact reread after a previously stable exact observation changed;
- preserve only thin hard stops at the actual mutation boundary; and
- leave task allocation to an orchestrator or human and merge integration to
  Git.

`stateful server start` defaults to `--coordination-mode awareness`.
`--coordination-mode enforcement` is opt-in. Awareness warnings do not create
wait-queue side effects. A claimable enforcement wait remains recovery
machinery, not an allocation protocol.

The hard stops in both modes are invalid targets, an unknown prior write
outcome, a changed previously stable exact-read observation, an active
same-target write fence, and an unreconciled high-confidence human write.
Missing, incomplete, or non-stable read evidence warns in awareness and denies
in enforcement.

A human save-check remains advisory to the human. A high-confidence observed
human write has stronger semantics: an agent must reread and record
reconciliation before writing that resource again.

## Concrete Implementation Evidence

The decision is implemented at head by these shipped surfaces:

- `crates/stateful-cli/src/lib.rs` gives both `stateful server` and `stateful
  server start` an `awareness` default and accepts `enforcement` only as the
  explicit alternative.
- `crates/stateful-core/src/freshness.rs` makes invalid targets, unknown write
  outcomes, changed previously stable observations, active fences, and
  unreconciled human writes denial conditions in both modes; the V2
  authorization path maps non-stable or incomplete evidence to missing, which
  warns in awareness and denies in enforcement.
- `crates/stateful-server/src/routes_v2.rs` exposes V2 authorization, exact
  read, human observation/reconciliation, context, notification, and recovery
  routes.
- `crates/stateful-cli/tests/v2_two_session.rs` verifies that awareness
  overlap warnings do not enqueue notifications while write-fence, human-write,
  and stale-observation cases remain deniable.
- `crates/stateful-core/src/journal.rs` serializes the corresponding presence,
  read-observation, write-fence, write-intent, human acknowledgement, context,
  authorization, and recovery evidence.

## Consequences

Positive:

- The default interaction is useful coordination context rather than broad
  locking.
- Freshness and short write fences defend the data-loss edge without claiming
  ownership of an entire task.
- Enforcement remains available when an operator needs denial-capable overlap
  control.

Negative:

- Awareness depends on agents honoring useful warnings and producing real
  handoffs.
- Context does not prove task ownership or replace a coordinator.
- A one-trial or credit-free benchmark run can validate plumbing only; it cannot
  prove causal, statistical, or quality superiority for either mode.

## Boundary Rule

```text
Presence, freshness, and handoff are the product.
Claims communicate intent.
Enforcement is explicit.
Blocking is a thin safety rail at the mutation boundary.
```
