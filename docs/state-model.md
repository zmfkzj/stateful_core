# State model

Stateful persists one local coordination database at
`$STATEFUL_HOME/state.db` (`$HOME/.stateful_core/state.db` when
`STATEFUL_HOME` is unset). SQLite is the source of truth for a local runtime.
The schema marker is `stateful.v2.lease1`.

The database separates immutable records from the materialized projection used
for admission checks. It is not a long-term activity archive: maintenance keeps
the audit journal for 14 days and expires stale tasks, offers, and leases.

## Resource identity and observations

A resource is identified within a workspace by `resource_id`; it records a kind
(`object`, `entry`, or `directory_tree`), canonical path, state JSON, generation,
and update time. `resource_aliases` associates equivalent canonical paths with
that physical resource.

A `ResourceObservation` is a client assertion about a resource state and
generation. The store canonicalizes it and records it in `resources`. A
successful exact read produces `resource_evidence` tied to a task, observation,
source, and generation. Evidence is valid only when the read completed
terminally, completely, stably, and exactly, and it remains compatible with the
resource state.

`read_attempts` records start/completion state per invocation. `read_intents`
tracks read ownership and is released when a task ends or expires.

## Task projection

`tasks` has one row per `(workspace_id, task_id)` with owner, next action,
settings, heartbeat, expiry, handoff, and status:

```text
active -> draining -> completed | failed | cancelled
```

A terminal request moves the task to `draining` first. If no write is in flight,
its leases are released and it reaches the requested terminal status in the
same transition. Otherwise its leases stay draining until completion makes a
safe release possible.

`runtime_processes` optionally associates an agent with a process identity and
its heartbeat. It is operational liveness data, not an authentication mechanism.

For a Codex root task, a local owner record identifies the originating parent
PID and start identity. Its hidden Codex heartbeat helper refreshes the task
only while that record and identity still match. `Stop` removes the record; the
helper exits when that record is removed or changes, or when the parent exits
or its start identity no longer matches. Failed heartbeat requests are retried
at the next configured interval.

## Lease projection

An active lease is a batch in `active_leases`, owned by one task and agent.
Its unique `(workspace_id, agent_id)` index allows that agent only one active or
draining batch in a workspace. `lease_resources` maps each batch to its physical
resources and remembers the generation at acquisition. Its unique
`(workspace_id, resource_id)` index is the exclusive-holder constraint: two
batches cannot hold an overlapping physical resource at once.

Lease requests live in `lease_requests` and their resource sets in
`lease_request_resources`. Request states are:

```text
queued -> offered -> activated
queued/offered -> cancelled | expired | superseded
```

`queue_sequence` is a global monotonic sequence. The partial unique index on
`(workspace_id, task_id)` for queued/offered rows permits only one nonterminal
acquisition per task. A task expands that request's resource set rather than
issuing another competing request.

When an active batch releases or expires, eligible queued requests are considered
in sequence order. The store offers the earliest request whose resources can be
held without jumping an older conflicting request. An offer includes an id,
version, and expiry. Activation is a compare-and-swap on the offered id and
version and requires post-offer fresh exact evidence; the client then takes fresh
evidence again and prepares again before write I/O.

`in_flight_attempt_id` on each lease resource marks I/O that is executing under
the batch. A release during I/O sets `release_pending` and `draining`; it does
not delete the active lease. Completion clears the in-flight marker, records the
outcome, and only then can release promote a waiter.

## Attempts and permits

`write_attempts` records a prepared write or structured commit: attempt id,
unique permit id, task, invocation, lease batch, operation, starting
observations, deadline, and terminal result.

```text
executing -> completed | failed | uncertain
```

A permit authorizes exactly the prepared operation. `uncertain` is explicit:
the client could not establish a known success or failure, so the record remains
visible in `/v2/status` rather than being treated as success.

## Journal, receipts, and projection replay

Each accepted command transaction inserts a `command_events` row containing the
full command kind, payload, response, request identity, contract revision, and
recording time. `command_receipts` indexes the request identity and response for
idempotent replay. Neither is updated to change a previously accepted command.

`audit_events` is the append-only audit journal. It records event id, workspace,
optional task, agent, event type, JSON payload, and creation time. `/v2/audit`
returns the newest retained entries; rows older than 14 days are pruned by the
maintenance loop.

The operational projection (`tasks`, resources, evidence, leases, requests, and
attempts) is a stored acceleration of accepted commands, not the authoritative
history. While command journal records are retained, it is replayable by applying
the accepted command stream in order; receipts ensure a repeated request does
not apply twice. There is no public endpoint that rebuilds a database in place.

`coordination_sequences` persists the lease queue counter, so queue ordering is
not reset by process restart.

## Schema inventory

| Area | Tables |
| --- | --- |
| Schema | `schema_migrations`, `coordination_sequences` |
| Tasks | `tasks`, `runtime_processes` |
| Resources and read evidence | `resources`, `resource_aliases`, `resource_evidence`, `read_intents`, `read_attempts` |
| Exclusivity and queue | `active_leases`, `lease_resources`, `lease_requests`, `lease_request_resources` |
| Mutation | `write_attempts` |
| Durable record | `command_events`, `command_receipts`, `audit_events` |

## Migration at database open

Opening `state.db` initializes an empty database with the schema marker when no
database exists. An exact current V2 schema opens normally. One narrow in-place
V2 upgrade is accepted: the marker and every schema object must match except for
the missing `idx_unique_active_lease_agent` index, and the existing rows must
already contain at most one active or draining lease per workspace and agent.
The opener creates that index. Duplicate slots and every other current-marker
partial or mixed layout fail closed without modifying the database.

The recognized legacy schema is the final V1 set: `schema_migrations`,
`events`, `agents`, `activities`, `reservations`, `claims`, `wait_queue`,
`notifications`, and `outbox`, with their final required column shapes and named
indexes. It may also contain either or both exact obsolete current-state groups:
`human_observations` with `idx_human_obs_unreconciled`, and `write_fences` with
its two named indexes. Partial optional groups, mixed layouts, and unknown
layouts fail closed. SQLite internal objects are ignored; every other unexpected
schema object rejects migration.

For a known legacy layout, the opener uses an exclusive migration lock and
writes a small manifest beside the database. It takes a SQLite backup from one
locked snapshot, checkpoints and quarantines the old database set, creates and
validates a private candidate database with the new schema, then atomically
promotes it. The candidate is a clean schema: prior coordination rows are not
translated into leases or tasks.

If migration stops after a backup is created, the next open reads the manifest
and restores the backup set before retrying. Database, manifest, lock, and
migration artifacts are created with owner-restricted permissions on supported
Unix platforms.
