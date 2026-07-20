# Current-State Coordination

Stateful V2 coordinates the present rather than reconstructing it from memory.
The shipped product center is **presence, freshness, and handoff**. Reservations
and claims communicate intent; broad denial is an enforcement option, not the
default experience. The canonical operational history is the indefinite V2
event journal, while live state is intentionally expiring.

For commands and the complete public event schema, see
[Usage Reference](usage-reference.md). [State Model](state-model.md),
[Implementation Contract](implementation-contract.md), and
[Architecture](architecture.md) give the detailed contracts.

## Shipped V2 Matrix

| Capability | Status | Safe to rely on today |
| --- | --- | --- |
| V2 protocol and routes | Shipped | Clients use `stateful.v2` and `/v2/**`; `/v2/runtime/identity` reports the compatible runtime identity. |
| Default coordination mode | Shipped | `stateful server start` defaults to `awareness`; `--coordination-mode enforcement` is explicit opt-in. |
| Presence and rendered context | Shipped | Sessions register, update presence, and receive versioned context deliveries that are acknowledged through `/v2/context/ack`. |
| Exact-read freshness | Shipped | A previously stable exact-read observation that changed requires an exact reread in either mode. Non-stable or incomplete evidence warns in awareness and denies in enforcement. |
| Reservations and claims | Shipped | They are advisory intent in awareness and can deny overlap in enforcement. A claimable wait requires reread and claim before retry. |
| Human signals and reconciliation | Shipped | `human save-check` warns humans; a high-confidence unreconciled human write stops later agent writes until exact reread and reconciliation. |
| Thin write fences and outcome recovery | Shipped | An active fence and an unknown prior write outcome stop a later write in either mode. |
| Final handoff | Shipped | Finalization records outcome, summary, and blocker/next action where applicable; it is not inferred from cleanup activity. |
| Event history | Shipped | `/v2/events` serializes the fixed event schema into the canonical indefinite journal. `stateful doctor` reports journal growth. |
| Task allocation or merge integration | Not a Stateful function | An orchestrator or human assigns work; Git integrates it. |

## The Coordination Model

Memory is evidence about the past. Current state is a fresh, scoped briefing
about nearby activity, the validity of the reader's evidence, and the last
handoff. It answers questions such as:

```text
Another actor is editing auth validation.
My exact read of src/auth.ts no longer matches current state.
I should reread, choose unrelated work, or ask the orchestrator or human.
```

Rendered context is not a scheduler. It is a write-time or planning briefing;
it must not be used to seize tasks, infer ownership, or replace Git review.

### Presence and Handoff

Presence is published by session registration, prompt/tool lifecycle updates,
and finalization. It conveys current goal, phase, planned resources, and recent
activity to the context renderer. It expires when it is no longer fresh.

A useful handoff is explicit: final status, what changed or was learned, and
what the next actor must do or why work is blocked. Expiration preserves the
journal record but does not make stale presence current authority.

### Freshness Is Read-Time Evidence

Freshness is derived from an exact read observation, not a claim hash or a
write-time guess. A complete, stable observation may support the next write to
that resource. If that stable observation has changed, the safe recovery is an
exact reread in either mode. Non-stable or incomplete evidence is treated as
missing: it warns in awareness and denies in enforcement.

This keeps the decision local to the resource and the mutation boundary rather
than granting a session long-lived possession of a task.

## Awareness and Enforcement

Awareness is the default. It renders reservation, claim, phase, and overlap
signals as advisory context or warnings, and it does not create wait-queue side
effects from those broad warnings. Agents should use that information to avoid
duplicated work and to produce a good handoff.

Enforcement is an explicit operator choice. It can deny broad coordination
conflicts under the configured reservation and claim policy. It is useful when
a workflow requires denial-capable overlap control, but it is not evidence that
claims are the product center.

Both modes retain the same narrow hard stops:

1. invalid write targets;
2. an unknown outcome for a previous write to that target;
3. a previously stable exact-read observation that has changed;
4. an active same-target write fence; and
5. an unreconciled high-confidence human write.

The recovery is specific: reread exactly for stale evidence, complete and
reconcile an unknown write outcome with its denial-provided `intent_id`, wait
for the in-flight fence then reread, or acknowledge the human change after
rereading. A non-owner cannot reconcile an unknown outcome while the owner is
active; after the owner leaves, it needs an active reservation with exact file
scope for every recovered target. Warnings are not permission to replay an
unsafe patch.

## V2 Lifecycle

A typical V2 flow is:

1. Register the session with `POST /v2/session/register`.
2. Publish goal, phase, plan, or resources with `POST /v2/presence/update`.
3. Read exactly through `POST /v2/read/start` and `/v2/read/complete`.
4. Render context with `POST /v2/context/render`; acknowledge a delivered
   version with `/v2/context/ack`.
5. Declare advisory intent when useful. In enforcement, obtain the needed
   reservation and claim authority before a governed write.
6. Authorize at the write boundary through `POST /v2/authorize`, then complete
   it with `/v2/write/complete`; recover an unknown outcome through
   `/v2/write/recover`.
7. Record a meaningful final status with `POST /v2/activity/finalize`.

Notifications and `/v2/resume/next` are recovery aids for explicit enforcement
waits. They are not the normal presence protocol. An acknowledgement identifies
a context delivery and version; it is not a once-per-session suppression flag.

## Human Changes

A human remains in control of their save. `POST /v2/human/save-check` and
`stateful human save-check` provide advisory warning before a save that overlaps
known activity. `POST /v2/human/observe` records an observed human change.

A high-confidence, unreconciled human write is deliberately different from a
presence hint: it stops a later agent write. The agent must reread the affected
resource and record an adopt, reapply, ask-user, or abandon decision through
`/v2/reconcile/ack` or `stateful reconcile ack` before continuing.

## Event Journal

The V2 journal records serialized events indefinitely; active rows may expire,
but expiration is also recorded rather than erased as if it had never happened.
`stateful doctor` exposes journal size, row count, event types, time range, and
growth diagnostics so operators can plan storage deliberately.

The public family/variant list is fixed by the shipped core and is reproduced
exactly in [Usage Reference](usage-reference.md#journal-events), including
`HumanAcknowledgement::Recorded` and `Recovery::Attempted`. Historical V1
materials are historical records only and are not authority for this runtime.

## Boundary Rule

```text
Presence, freshness, and handoff are the product.
Claims and reservations state intent.
Enforcement can deny overlap when explicitly enabled.
Blocking remains a thin safety rail at the mutation boundary.
```
