# Architecture

`stateful_core` is a local coordination service for a shared checkout. Runtime
adapters, the CLI, and human observers use one V2 HTTP API; policy and durable
state live in the server rather than being duplicated in hooks.

```text
hooks / CLI / watcher / OMP extension
               |
          /v2 HTTP API
               |
  append event + apply projectors + receipt
               |
SQLite journal_events + current projections
               |
context rendering, presence, handoff, freshness, delivery
```

## Runtime Modes and Boundaries

`stateful server start` defaults to `--coordination-mode awareness`.
Awareness shows presence, reservations, claims, waits, handoffs, and freshness
in rendered context. It converts reservation/scope/claim denials to warnings.
`--coordination-mode enforcement` is the explicit opt-in that denies those
coordination failures.

Thin write-boundary safety is evaluated before mode-specific broad
coordination policy. Invalid targets, unknown previous write outcomes, changed
exact-read observations, active write fences, and unreconciled high-confidence
human writes deny in both modes. A non-stable, missing, or expired exact-read
observation warns in awareness and denies in enforcement. This is deliberately
not a claim/lock-first architecture: claims are advisory intent in awareness, and a
write fence protects only an in-flight mutation.

## Durable Command Flow

A V2 request carries `stateful.v2`, request id, observation time, agent,
workspace, source, and endpoint payload. Query envelopes carry the same
identity fields in the query. The server executes each accepted command as:

1. validate the envelope and command;
2. append one or more typed journal events;
3. apply the projectors to current-state tables;
4. persist an idempotent command receipt; and
5. return the projected result.

The event journal is canonical and append-only. Projections are replaceable
current-state indexes, not a second source of truth. They support live
presence, reservations, claims, waits, fences, read observations, human
acknowledgements, handoffs, notifications, context delivery, and resource
versions. `/v2/events` reads the journal; `/v2/current` reads the projection.

There is no age-based canonical-event pruning. `stateful doctor` reports the
journal footprint, row count, event types, range, growth, and a warning when
size reaches `STATEFUL_DOCTOR_JOURNAL_THRESHOLD_BYTES` (512 MiB by default).
The warning is capacity guidance, not automatic deletion.

## Migration and Replay

Opening a persistent legacy database first validates its legacy shape and takes
a permission-preserving SQLite backup beside it (`*.v1.backup.sqlite`, adding a
numeric suffix when needed). The migration then takes an exclusive transaction,
appends migration/audit events and typed snapshot seeds, projects them into
shadow tables, replays the journal and compares the replayed projections,
replaces live projections atomically, writes the `stateful.v2.event-journal`
checkpoint, and drops legacy tables only after the backup is confirmed. A
changed source database version aborts that attempt, removes its backup, and
retries. Failed migration transactions roll back without cutover.

Snapshot seeds preserve legacy facts with their limits: legacy claim hashes are
legacy base observations, never exact-read provenance; unavailable attribution
is `unknown`; and a legacy finalization without an explicit handoff becomes a
fallback handoff rather than invented completion detail.

## Presence, Context, and Delivery

`/v2/session/register` establishes presence and `/v2/presence/update` updates
goal, phase, resources, or activity. Expiration removes stale state from live
coordination while retaining its events. `/v2/activity/finalize` creates an
explicit handoff when provided, otherwise a marked fallback. Context rendering
is versioned: `/v2/context/render` creates a delivery for changed context, and
`/v2/context/ack` advances the recipient's delivery cursor only for the exact
delivery id, sequence, and workspace version. A missed acknowledgement is
eligible for redelivery; it is not suppressed by a session-local marker.

Notifications are independently resumable through `/v2/notifications/poll`
and `/v2/notifications/stream`. `/v2/resume/next` recovers a claimable wait
reservation even if notification delivery was missed.

## Read and Write Lifecycle

Before a file write, a client records a complete exact read with
`/v2/read/start` and `/v2/read/complete`. `/v2/authorize` evaluates the
resulting observation against the current resource version and, on an accepted
write boundary, records a write intent/fence. `/v2/write/complete` records the
outcome; `/v2/write/recover` reconciles an interrupted or unknown outcome.
This prevents write-time provenance from being inferred from a prior claim.

Human observers use `/v2/human/observe` and `/v2/human/save-check`.
`/v2/reconcile/ack` records the agent's exact reread and reconciliation of a
high-confidence human write before it writes again.

## V2 HTTP Surface

All public server routes are V2:

```text
POST /v2/session/register          POST /v2/presence/update
POST /v2/read/start                POST /v2/read/complete
POST /v2/write/complete            POST /v2/write/recover
POST /v2/activity/finalize         POST /v2/reservation/declare
POST /v2/reservation/request       POST /v2/reservation/claim
POST /v2/reservation/cancel        POST /v2/claim/acquire
POST /v2/claim/release             POST /v2/authorize
POST /v2/human/observe             POST /v2/human/save-check
POST /v2/reconcile/ack             POST /v2/context/render
POST /v2/context/ack               POST /v2/notifications/poll
POST /v2/resume/next               POST /v2/outbox/sync
GET  /v2/current                   GET  /v2/events
GET  /v2/notifications/stream      GET  /v2/runtime/identity
```

Adapters may add runtime-specific target extraction and lifecycle observation,
but they must not own store-backed policy. The API, journal, projections, and
receipt path are the integration contract.
