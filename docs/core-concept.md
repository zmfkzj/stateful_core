# Core Concept

`stateful_core` coordinates people and agents that must work in one live
checkout. It makes nearby work visible, keeps that view fresh, and preserves a
useful handoff when a session stops. It is not a task scheduler, a security
sandbox, or a replacement for Git isolation and review.

## Product Center

The useful questions are:

```text
Who is present, what are they doing, is that information fresh,
what did the last actor hand off, and what context should I see now?
```

Presence, freshness, handoff, and rendered context are the product. Claims and
reservations express intent and help actors coordinate; they are not a general
locking system. The default server mode is `awareness`: it renders conflicts and
turns reservation/scope/claim failures into warnings. `enforcement` is an
explicit opt-in for installations that want those coordination failures denied.

## Freshness Is Read-Time Evidence

A write is not justified by a claim-time hash or a remembered read. A client
uses `/v2/read/start` and `/v2/read/complete` to record a complete, stable,
exact read of the target. Authorization compares that observation with the
current resource version. A changed exact observation is denied in both modes.
A non-stable, missing, or expired observation warns in awareness and denies in
enforcement. The required recovery is an exact reread, not a claim refresh.

The thin safety stops are intentionally narrow and shipped in both modes:

- invalid workspace-relative targets;
- a prior unknown write outcome for the target;
- changed exact-read evidence;
- an active same-target write fence; and
- an unreconciled high-confidence human write.

These stops prevent a known unsafe write boundary. They do not turn ordinary
presence into access control.

## Presence and Handoff

Sessions register and update presence through `/v2/session/register` and
`/v2/presence/update`; tool lifecycle and finalization update the same shared
view. Presence expires, so old activity is context rather than proof of active
work. On completion, `/v2/activity/finalize` records an explicit handoff when
one is supplied. If none is supplied, the runtime creates a clearly fallback
handoff; cleanup counts are not handoff content. Explicit handoffs remain
context-relevant longer than fallback handoffs, but neither changes the
canonical journal's retention.

`/v2/context/render` produces a versioned, prompt-ready view. A changed render
has a delivery id and sequence; the client acknowledges that delivery with
`/v2/context/ack`. An acknowledgement is a delivery cursor, not a
once-per-session prompt marker. Unacknowledged context can be rendered and
redelivered, while `/v2/notifications/poll` and `/v2/notifications/stream`
carry resumable coordination notifications.

## Durable Model

Every accepted command appends typed journal events, projects current state,
and records a command receipt in one durable transaction. The projections make
current reads, conflict checks, context rendering, and delivery cursors fast;
the journal is canonical and is retained indefinitely. `stateful doctor` warns
when its journal footprint crosses its configured threshold so operators can
plan storage, rather than silently pruning coordination history.

A legacy database migrates by making a SQLite backup, appending audit and typed
snapshot-seed events, replaying into shadow projections, comparing replay with
the projected state, then atomically cutting over and checkpointing the
migration. Legacy claim hashes remain legacy observations; they are never
promoted to exact-read proof.

## Scope

Use separate worktrees, branches, containers, or an orchestrator when they
provide sufficient isolation. Use `stateful_core` when actors truly share a
checkout and need live coordination. The system improves visibility and makes
specific unsafe writes stop; it does not prove that a task was allocated
correctly or that an unobserved external actor cannot change files.
