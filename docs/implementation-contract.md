# Implementation Contract

This document describes the shipped V2 contract. `stateful.v2` is required for
all active client requests.

## Envelope and Identity

Mutating routes accept `RequestEnvelope<T>`:

```text
protocol_version: stateful.v2
request_id: non-nil UUID
observed_at: RFC 3339 timestamp
agent: AgentIdentity
workspace: WorkspaceIdentity
source: SourceRef
payload: endpoint-specific object
```

`QueryEnvelope<Q>` carries the same `protocol_version`, `request_id`, and
`observed_at`, then flattens `AgentIdentity`, `WorkspaceIdentity`, `SourceRef`,
and query fields. Invalid, missing, or other protocol versions are rejected.
Errors use `V2ErrorEnvelope` with `protocol_version`, `request_id`, and an
error containing `code`, `message`, and optional `required_next_action`.

The server owns authorization and persistent state. Hooks, CLI commands,
watchers, and the OMP extension may derive runtime identity and extract targets,
but they send the same V2 envelope and do not reimplement store-backed policy.

## Public Routes

`crates/stateful-server/src/routes_v2.rs` is the route source of truth:

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

`/v2/runtime/identity` reports `protocol_version: stateful.v2`, journal schema
version, coordination mode, process/workspace identity, and capabilities.
`/v2/current` and `/v2/events` are query-envelope reads. `/v2/outbox/sync`
replays durable local outbox records idempotently; an outbox is recovery/audit
evidence and cannot authorize a write while the server is unreachable.

## Write Contract

A write authorization has an operation id, a supported action, one or more
normalized workspace-relative targets, and optional reservation id. Supported
actions are `write_file`, `write_directory`, `delete_file`, `rename_file`, and
`move_file`; rename and move require exactly two targets.

The exact-read sequence is mandatory evidence, not a convention:

1. `POST /v2/read/start` records the exact target read.
2. `POST /v2/read/complete` records stable completion evidence.
3. `POST /v2/authorize` supplies each target's complete `before` observation.
4. `POST /v2/write/complete` records success or failure after the mutation.
5. `POST /v2/write/recover` reconciles an interrupted or unknown result.

The server denies invalid targets, another unknown outcome, changed exact
observations, active write fences, and unreconciled high-confidence human
writes in both modes. No stable current read observation warns in awareness and
denies in enforcement. In awareness, missing or inactive reservation, or a
missing claim, is a warning; enforcement denies it. No client may substitute
a claim observation for the exact-read sequence.

`/v2/human/observe` records human activity; `/v2/human/save-check` is advisory
for the human caller. `/v2/reconcile/ack` records the exact reread and decision
needed to clear a high-confidence human-write block. `/v2/reservation/*` and
`/v2/claim/*` record intent and active coordination; `/v2/resume/next` recovers
a claimable queued reservation.

## Presence, Context, and Redelivery

`/v2/session/register`, `/v2/presence/update`, and
`/v2/activity/finalize` maintain current presence and explicit/fallback
handoffs. Context is rendered by `/v2/context/render` as a `ContextDelta`.
When changed, it has `delivery_id`, `sequence`, `workspace_version`, and
prompt text. `/v2/context/ack` accepts `ContextAcknowledgement` with those
three fields and returns `ContextAcknowledgementResult` with
`acknowledged_version` and `cursor`.

A render is not considered consumed merely because a session started. Missing
acknowledgement leaves it eligible for redelivery. Notification poll and SSE
stream delivery are separately resumable; SSE uses the notification sequence as
its event id and accepts the last delivered id on reconnect.

## Journal, Projection, Receipt, and Retention

For each accepted command the store appends typed `EventPayload` values to
`journal_events`, projects them to current tables, and persists an idempotent
command receipt. The journal has ordered `event_seq`, stable event/request ids,
identity metadata, event type, timestamp, context-affects flag, and payload.
`/v2/events` exposes that canonical event stream; current tables exist only to
serve state reads and policy efficiently.

Canonical events are retained indefinitely. No maintenance loop prunes them by
age. `stateful doctor` measures the journal and sets its warning flag when the
footprint meets the configurable
`STATEFUL_DOCTOR_JOURNAL_THRESHOLD_BYTES` threshold (512 MiB by default). The
operator decides storage management; the runtime does not silently discard the
journal.

## Persistent Migration

On opening a legacy persistent database, the store validates it and creates a
permission-preserving `*.v1.backup.sqlite` SQLite backup before exclusive
cutover. It emits `Migration` audit and snapshot-seed events, projects them to
shadow tables, replays and compares the full journal, atomically swaps the
projections, records the `stateful.v2.event-journal` checkpoint, and only then
drops legacy tables. A source-version race retries; a failed transaction rolls
back with no V2 cutover.

Legacy hash fields are recorded as legacy base observations, never exact-read
provenance. Missing historical actor/handoff facts remain `unknown` or empty;
cleanup metadata is not converted into a claimed handoff.

## Public Event Type Contract

The complete `EventPayload` family/variant list is maintained in
[State Model](state-model.md#serialized-event-schema). It is serialized by
`crates/stateful-core/src/journal.rs` and is the only public event schema for
`/v2/events`. In particular, clients must recognize
`HumanAcknowledgement::Recorded` and `Recovery::Attempted`.
