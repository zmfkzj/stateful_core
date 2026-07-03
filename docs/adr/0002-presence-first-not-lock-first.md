# ADR 0002: Presence-first, not lock-first

Status: Proposed

## Context

The founding failure list in [Core Concept](../core-concept.md) is, read
plainly, a list of information and freshness failures:

- two agents edit the same file without knowing it
- one session repeats investigation another session already started
- an agent writes a file that changed after its claim was acquired
- stale memory is mistaken for current activity
- an interrupted session leaves no structured handoff state

Four of these are unknown-neighbor-activity or missing-handoff problems. One is
a freshness problem. None of them asks for an access-control product. V1
nonetheless grew a reservation/claim/queue/blocking surface large enough that
the docs began describing the guardrail as the product center. The means became
the message.

Fable5's review cautions that Cursor's large-N scaling experiment
(https://cursor.com/blog/scaling-agents) is warning evidence, not a universal
proof. Cursor reported that agents coordinating through task-scale claims and
locks collapsed throughput even when locking worked correctly: "Twenty agents
would slow down to the effective throughput of two or three, with most time
spent waiting." Replacing locks with optimistic concurrency control - read
freely, fail the write if state changed since the read - was "simpler and more
robust." What OCC did not fix was task allocation, which a planner/worker
hierarchy fixed.

Do not overgeneralize that large-N convoy result to small-N shared-checkout
tests. Here it is a warning against broad waits and task-scale locks, not proof
that every enforcement claim is bad. The surviving division of labor:

```text
plan-time   task allocation    orchestrator or human, not this system
write-time  execution safety   presence + freshness + thin fences (this system)
merge-time  integration        Git
```

Lock-wait cost scales with every write and every concurrent actor; OCC conflict
cost scales only with actual conflicts. In the small-N shared-checkout regime
this system targets, presence plus freshness checks are the hypothesis to test,
not a result to assume.

Two time scales were previously conflated in one claim concept:

```text
task-scale    minutes; intent, scope, status; useful rendered, dangerous as a lock
write-scale   the mutation boundary; base-hash compare and short fences; data safety
```

## Decision

The product center is one live-checkout presence layer: agents and humans see
nearby work, freshness, and handoff before writing. Blocking is a thin safety
rail, not the product.

Concretely:

- Reservations are advisory intent in the target model: rendered to neighbors,
  not a broad denial source by themselves.
- Claims narrow toward short write fences or active edit leases that protect
  the mutation boundary, not minutes-long possession.
- Freshness is mechanical, not only visible: long-lived base observations
  (path, existence, content hash, observed-at) recorded at read time and
  compared at write time. A stale base is a hard stop with a reread
  instruction, because replaying a stale patch is data loss, not coordination
  friction.
- Queueing, FIFO promotion, and lazy resume are recovery ergonomics, not the
  core collaboration protocol.
- Hard denial is reserved for thin safety edges:
  1. stale base hash on edit or patch replay
  2. simultaneous same-file in-flight write fences
  3. multi-file operation leases, because per-file checks cannot prevent a
     semantically torn multi-file write
  4. git index/stage/commit serialization within one checkout
  5. destructive delete/rename/reset without explicit approval
  6. raw command execution outside sandbox/scope policy
- Rendered coordination context (`context_render`) is a scoped write-time
  briefing - what nearby work is active, what changed since the actor last
  looked, whether to reread, wait, or proceed. It is not a task scheduler; task
  allocation belongs to the orchestrator or human, and integration belongs to
  Git. Cursor's flat self-coordination failure is the recorded warning for that
  misuse, not a reason to turn this system into a planner.
- Human observation sensors are shipped as advisory CLI/HTTP and IDE-facing
  safety input. `human save-check` warns before a human save overlaps active
  claims or write fences, but the human remains in control. High-confidence
  `human observe` writes (`save`, `change`, `delete`) raise
  `HumanWriteObserved`; later agent writes stop until reconciliation records
  `adopt` or `reapply`. Low-confidence presence/dirty signals warn rather than
  deny.

Shipped v1 now has two coordination modes. `enforcement` remains the default and
requires active reservation scope plus same-reservation claim before supported
writes, as the [README](../../README.md) and [Implementation Contract](../implementation-contract.md)
describe. `awareness` is the presence-first comparison arm: broad reservation,
scope, claim, phase, and active-claim conflicts warn instead of blocking, while
thin data-safety edges still deny. Those edges include stale base or claim-time
observations, unreconciled human writes, unsupported actions, and simultaneous
same-file write fences. The machinery question is tested through the three-arm
comparison in the [DeNovoSWE guide](../denovo-benchmark-guide.md): off versus
awareness versus enforcement. If awareness holds enforcement's safety at lower
waiting and complexity cost, default machinery can shrink to thin fences; if
enforcement is meaningfully safer, the broader guardrail stays with evidence.

Current evidence pointer: [2026-07-03 forced-overlap three-arm](../benchmarks/2026-07-03-forced-overlap-three-arm.md)
exercises the no-state/awareness/enforcement plumbing, but produced no
differentiated safety outcome. Status therefore stays Proposed.

## Consequences

Positive:

- The product sentence matches the founding problems and the external evidence
  instead of a lock-manager framing.
- Convoy risk is designed out of the default path; waiting is reserved for
  actual mutation races and explicit recovery.
- The hook surface stays, repurposed: observe, inject briefings, check
  freshness, hold thin fences - deny only at safety edges.
- Human-edit observation gets a principled priority instead of an indefinite
  deferral.

Negative:

- Advisory intent depends on agents reading and honoring rendered context;
  overlap that information does not deter is absorbed by freshness stops,
  human-write reconciliation stops, write fences, and merge-time work.
- Two concepts (intent records, write fences) replace one overloaded claim
  concept, and docs and tests must keep mode-specific behavior explicit.
- Awareness mode, base-observation coverage, write-scale fences, human
  observation, and sensor confidence tiers are shipped surfaces now, but the
  default-mode decision remains evidence-gated by benchmark and product data.

## Boundary Rule

```text
Presence, freshness, and handoff are the product.
Blocking is a safety rail at thin edges.
Do not use coordination context as a task scheduler.
```
