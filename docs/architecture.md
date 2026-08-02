# Architecture

Stateful is a local, store-backed coordinator. Runtimes and the CLI are thin
clients: the server owns resource identity, evidence checks, queueing, lease
transitions, and durable audit records.

## Components

```text
OMP extension / CLI / fail-closed Codex adapter
        |
        | stateful.v2, lease-1 envelope
        v
loopback HTTP server
        |
        v
SQLite store (state.db)
  |- task and process projection
  |- resource observations and read evidence
  |- active leases and FIFO lease requests
  |- read/write attempts and command receipts
  `- append-only audit events
```

The CLI discovers the local runtime, supplies task and agent identity, and uses
the same HTTP surface as hooks. The server is the sole policy owner; adapters
must not reimplement conflict rules.

Codex native-tool conformance is intentionally treated as unavailable. Codex
hooks provide the derived task and agent ids and the installed Stateful binary
as context, then allow only an exact installed-binary adapter wrapper. The
wrapper permits read-only, Git, and process sandbox operations plus `status`,
`doctor`, and `repos list`; mutation sandbox operations, structured commits, and
lease release require matching derived task and agent identity. It rejects
outer-shell chaining and expansion. After `UserPromptSubmit`, the hidden Codex
heartbeat helper refreshes the root task at `heartbeat_interval_seconds` only
while its owner record and parent PID/start identity match. This keeps a live
Codex task from expiring solely because prompt work is inactive; a failed
heartbeat is retried at the next interval. `Stop` removes the record; the
helper exits when that record is removed or changes, or when the parent exits
or its start identity no longer matches.

## Runtime boundary

The server binds loopback addresses only. `/health` is the only unauthenticated
endpoint; the bearer token protects all `/v2/**` endpoints. `STATEFUL_HOME`
selects the global runtime home (`$HOME/.stateful_core` when unset), including
`state.db`, runtime metadata, and enabled-repository registry.

This scope is local to one machine and its local database. It does not share
coordination over LAN or other networks, observe arbitrary filesystem writes,
replace Git history and merge, schedule agents, or enforce a security boundary.
Trusted clients must use the protocol for Stateful to coordinate them.

## Request contract

Every command request uses the `stateful.v2` protocol version and `lease-1`
contract revision. The envelope contains a request id, task id, observation
time, agent identity, workspace identity, and source reference; command input
is supplied as `payload`. Request ids make accepted commands replay-safe.

A protocol mismatch is a `400` error with `decision: "error"`,
`reason_code: "protocol_mismatch"`, `protocol_version: "stateful.v2"`, and
`contract_revision: "lease-1"`. Valid command errors use the same protocol and
revision fields in their error response.

## Flow

### Task and reads

A client starts a task with settings, expiry, and next action. Heartbeats extend
that task while it is active. A read starts with the resources observed, then
completes only if it was terminally successful, complete, stable, exact, and
unchanged between start and completion. Such a read creates evidence tied to
resource generations.

### Writes and commits

A write or structured commit submits the exact observed resources and operation
at `/v2/writes/prepare` or `/v2/commits/prepare`. The server checks the supplied
observation against stored resources and valid read evidence.

- **ready** returns an attempt id, permit id, and active lease batch id.
- **queued** returns a batch id for the task's nonterminal request.
- **reread_required** means observations or evidence cannot support the write.
- **denied** reports a reason code.

The holder performs the operation only after `ready`, then reports success,
known failure, or uncertainty through the matching `complete` endpoint. A
structured commit also observes its explicit tracked files and Git metadata
before preparing, so Git changes are within the leased operation.

### Lease queue

An overlapping request is assigned a monotonic queue sequence. On release or
expiry, the earliest eligible request is offered. Its owner rereads, then calls
`/v2/leases/activate` with the offer id and version; activation compares those
values and checks evidence again. An expired, cancelled, or superseded request
cannot become active.

## Invariants

1. **One holder per physical resource.** `lease_resources` has a unique index
   over `(workspace_id, resource_id)`. An active lease is exclusive for each
   resource it covers, including overlap resolved to the same physical resource.
2. **One active batch per agent.** `active_leases` has a unique index over
   `(workspace_id, agent_id)`. Active and draining leases both occupy the slot;
   another task for that agent queues even when its resources are disjoint.
3. **One pending acquisition per task.** A task has at most one queued or
   offered lease request. A later required resource is folded into that request
   before it can acquire.
4. **Waiting writers stay ordered.** Requests receive a global, monotonic queue
   sequence. Active holders and older conflicting requests block younger ones;
   promotion selects eligible queued requests in sequence order.
5. **No I/O under a lease transition.** Preparation records an in-flight attempt
   before the client performs I/O. Release while an attempt is in flight changes
   the lease to draining and defers deletion; completion clears the in-flight
   mark before final release/promotion.
6. **Evidence gates mutation.** A write requires exact, stable read evidence
   for the observations it presents. Changed, incomplete, or stale observations
   cause rereading instead of a permit.
7. **Terminal tasks drain safely.** Finalization or cancellation releases reads
   and pending requests immediately. If a write is still in flight, leases drain
   until that attempt reaches a terminal result.
8. **Accepted commands are durable.** The store records the accepted command,
   response receipt, and audit event in its SQLite transaction. Replaying the
   same request id returns its recorded response rather than repeating effects.

See [state model](state-model.md) for the tables and reconstruction rules, and
[usage reference](usage-reference.md) for the concrete API surface.
