# Usage Reference

This is the operator reference for the shipped V2 runtime. It describes current
behavior only; [State Model](state-model.md) and
[Implementation Contract](implementation-contract.md) provide the detailed
model and integration contracts.

Stateful coordinates the present: nearby presence, exact-read freshness, and a
final handoff. Its canonical history is the indefinite V2 event journal; use
`stateful doctor` to monitor its growth rather than treating age-based event
pruning as a coordination mechanism.

## Install and Enable

```bash
stateful install --yes
stateful install --agent codex --yes
stateful install --agent omp --yes

stateful enable [--repo <path>]
stateful disable [--repo <path>]
stateful repos list
```

Installation writes global files under `$STATEFUL_HOME`, or
`$HOME/.stateful_core` when it is unset. Without `--yes`, `install` prints its
plan. Hooks are active only for an enabled repository. Use `stateful doctor` to
inspect installation, runtime, protocol, and journal diagnostics.

## Server Lifecycle

```bash
stateful server start [--coordination-mode awareness|enforcement]
stateful server status
stateful server restart
stateful server stop
stateful status
```

`stateful server start` detaches by default; `--foreground` keeps it attached.
A bare `stateful server` is the foreground form. **Awareness is the default.**
Choose `--coordination-mode enforcement` only when the stricter, denial-capable
coordination policy is intended.

To use a remote runtime, keep the bearer token on a loopback connection, for
example through an SSH tunnel:

```bash
# host
stateful server start --host 0.0.0.0 --workspace-id shared

# remote client
ssh -L 43873:127.0.0.1:43873 <host>
stateful server join http://127.0.0.1:43873 --token <token> --workspace-id shared
```

`server join` accepts `--allow-plain-http` only as an explicit exception. It
can also enable the current repository with `--enable-repo`.

## Operator Workflow

Lifecycle hooks register the active session, publish presence, record exact
reads, render and acknowledge context deliveries, and finalize activity. They
supply the active identity; do not manufacture session identity from environment
variables or local session files.

Before a write, use the delivered context and reread an affected resource when
its evidence is stale. A final status is a handoff only when it includes the
result and next action or blocker; cleanup counts alone are not a handoff.

Manual reservation and recovery commands are for flows outside the active native
integration:

```bash
reservation_id=$(stateful reservation declare \
  --agent-id demo-agent \
  --workspace-id <workspace> \
  --purpose "Update README content requested by the user." \
  README.md | jq -r '.reservation_id')

stateful reservation request \
  --agent-id demo-agent \
  --workspace-id <workspace> \
  --request-id <id> \
  --reservation-id "$reservation_id" \
  --action write_file \
  --path README.md \
  --purpose "Update README content requested by the user."
stateful reservation claim \
  --agent-id demo-agent \
  --workspace-id <workspace> \
  --reservation-id "$reservation_id" \
  --wait-id <wait_id> \
  --path README.md
stateful reservation cancel \
  --agent-id demo-agent \
  --workspace-id <workspace> \
  --wait-id <wait_id>

stateful notifications poll --agent-id demo-agent --workspace-id <workspace>
stateful resume next --agent-id demo-agent --workspace-id <workspace>
```

Reservations and claims express intent. In awareness they are advisory context
and overlap is warned, not broadly denied. In enforcement they can deny an
overlap according to the configured policy. A claimable wait is not write
authority: reread the exact target, then claim it before retrying.

Human signals and reconciliation use the V2 runtime:

```bash
stateful human observe <path> \
  [--kind save] [--confidence high] [--source watcher:save] [--summary <text>]
stateful human save-check <paths...>
stateful reconcile ack \
  --resource <path> \
  --files-reread <path> \
  --summary <text> \
  --decision adopt|reapply|ask_user|abandon
```

`human save-check` warns a human before an overlapping save; it does not take
control from the human. A high-confidence unreconciled human write stops a later
agent write until the agent rereads and records reconciliation.

`unknown_write_outcome` is reconciled against the denied write intent: first
complete an exact reread of every intent target, then pass the denial's
`intent_id` to `state_reconcile_ack` (or `--intent-id` to the CLI). The native
tool accepts optional `read_tokens` as a path-to-token map; each path must also
appear in `files_reread`. For an `adopt` or `reapply` unknown-outcome
recovery, the OMP extension declares the exact-file reservation when one is
omitted and forwards its `reservation_id`; a direct CLI successor must declare
and pass that reservation itself.
The owner may reconcile its own intent. Another agent is rejected while the
owner is active; once that presence ends, it may reconcile only with an active
reservation whose file scopes exactly cover every intent target.
A `continuation_token` or `reconciliation_token` is bound to its issuing OMP session/workspace and cannot be passed to another agent or session.

## Awareness, Enforcement, and Hard Stops

Awareness is the product default: reservations, claims, phases, and overlap
signals produce presence/freshness/handoff context. Enforcement is opt-in and
can deny the same broad coordination conflicts. Neither mode turns Stateful
into a task scheduler; allocation belongs to an orchestrator or human and
integration belongs to Git.

Both modes retain thin, shipped stops at the mutation boundary:

- an invalid target;
- an unknown prior write outcome for that target;
- a previously stable exact-read observation that has changed;
- an active write fence; and
- an unreconciled high-confidence human write.

Missing or expired read provenance warns in awareness and denies in enforcement.
A denial is a recovery instruction, not permission to replay stale work: reread
exactly, reconcile when required, then retry.

## Native and Sandbox Tools

Use a Stateful native tool only when it is present in the active tool list. The
supported names are:

- `state_session_register` / `state.session.register`
- `state_session_heartbeat` / `state.session.heartbeat`
- `state_reservation_declare`, `state_reservation_request`,
  `state_reservation_claim`, and `state_reservation_cancel` (and their dotted
  forms)
- `state_claim_acquire` and `state_claim_release` (and their dotted forms)
- `state_activity_finalize`, `state_current_read`, `state_events_read`,
  `state_context_render`, `state_reconcile_ack`, `state_notifications_poll`,
  and `state_resume_next` (and their dotted forms)

OMP exposes its active tool list rather than the full `state_*` surface. When a
native tool is absent, use its native `edit`/`write` path or the available lazy
resume helper; do not invent a tool name.

For shell work, use the narrowest trusted wrapper:

```bash
stateful sandbox run --fs read-only --network disabled --command '<read command>'
stateful sandbox run --fs build --network enabled --write-dir <scratch-purpose> --command '<build command>'
stateful sandbox run --fs write-targets --reservation-id "$reservation_id" \
  --write-target README.md --command '<write command>'
stateful sandbox run --fs git --network disabled --command 'git status --short'
stateful sandbox process find --name stateful-bench
```

Raw Bash is not a substitute for a hook-visible write boundary.

## V2 HTTP Surface

V2 clients use the `stateful.v2` envelope. The runtime exposes these routes;
there is no active V1 authority:

- `POST /v2/session/register`, `/v2/presence/update`, `/v2/read/start`,
  `/v2/read/complete`, `/v2/write/complete`, `/v2/write/recover`, and
  `/v2/activity/finalize`
- `POST /v2/reservation/declare`, `/v2/reservation/request`,
  `/v2/reservation/claim`, `/v2/reservation/cancel`, `/v2/claim/acquire`,
  `/v2/claim/release`, and `/v2/authorize`
- `POST /v2/human/observe`, `/v2/human/save-check`, `/v2/reconcile/ack`,
  `/v2/context/render`, `/v2/context/ack`, `/v2/notifications/poll`,
  `/v2/resume/next`, and `/v2/outbox/sync`
- `GET /v2/current`, `/v2/events`, `/v2/notifications/stream`, and
  `/v2/runtime/identity`

`/v2/context/render` returns a delivery when the context version changed; the
recipient acknowledges it through `/v2/context/ack`. A delivery acknowledgement
is distinct from a once-per-session prompt marker: a new or unacknowledged
context version is still deliverable.

## Journal Events

`GET /v2/events` serializes the fixed V2 event schema. The family and variant
set is:

- `Migration`: `Started`, `LegacyAuditImported`, `PresenceSnapshotSeeded`,
  `ReservationSnapshotSeeded`, `ClaimSnapshotSeeded`, `WaitSnapshotSeeded`,
  `WriteFenceSnapshotSeeded`, `HumanObservationSnapshotSeeded`,
  `LegacyHandoffSnapshotSeeded`, `DeliverySnapshotSeeded`, `Validated`,
  `Completed`
- `Presence`: `Registered`, `Heartbeat`, `GoalUpdated`, `PhaseUpdated`,
  `PlanUpdated`, `ResourcesUpdated`, `ToolStarted`, `ToolCompleted`,
  `Finalized`, `Expired`
- `Reservation`: `Declared`, `Refreshed`, `Released`, `Expired`
- `Claim`: `Acquired`, `ObservationRefreshed`, `Released`, `Expired`
- `Wait`: `Requested`, `BecameClaimable`, `Claimed`, `Cancelled`, `Expired`
- `WriteFence`: `Acquired`, `ConflictObserved`, `Released`, `Expired`
- `ReadObservation`: `Started`, `Stabilized`, `Unstable`, `Aborted`,
  `Invalidated`, `Expired`
- `WriteIntent`: `Started`, `Committed`, `Failed`, `OutcomeUnknown`,
  `Reconciled`
- `HumanObservation`: `Observed`, `Reconciled`, `Expired`
- `HumanAcknowledgement`: `Recorded`
- `Handoff`: `Finalized`, `Expired`
- `Authorization`: `Allowed`, `Warned`, `Denied`, `OverrideGranted`
- `Context`: `Rendered`, `DeliveryCreated`, `DeliveryAcknowledged`,
  `DeliverySuperseded`
- `Notification`: `Created`, `Delivered`, `Expired`, `Coalesced`
- `Recovery`: `Queued`, `Attempted`, `Delivered`, `Failed`

## Benchmark Gate

`parallel-on` is never implicit. A required three-arm run names all arms
explicitly:

```bash
--arms sequential,parallel-off,parallel-on
```

`parallel-on` starts Stateful in awareness mode and requires a resolvable
Stateful binary. Use fresh row databases and output directories. A credit-free
smoke or one trial validates plumbing only; it does not establish causal,
statistical, or quality superiority.
