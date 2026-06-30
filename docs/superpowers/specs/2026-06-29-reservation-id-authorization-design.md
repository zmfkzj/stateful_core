# Reservation Id Authorization Design

## Goal

Make the reservation id the write-authorization batch id for reservation, claim, and edit/write operations. Keep `session_id` for lifecycle, ownership, notification, and audit context instead of requiring it as the core write-authority key.

## Current Behavior

The v1 state model describes write authorization as session identity plus active reservation and claim:

```text
session
  goal
    turn
      task reservation
        file/directory scopes
          resource claims
            write actions
```

A reservation belongs to a session, and each supported write requires a same-session claim for the exact resource. MCP and CLI request bodies commonly carry `session_id` and `workspace_id` so declarations, claims, requests, and writes evaluate against the same session.

This makes write authorization depend on a long-lived lifecycle identity even though the write path only needs to prove that one edit/write transaction is covered by the reservation and claims acquired for that transaction.

## Chosen Approach

Promote the existing reservation id to the batch id.

Do not add a separate `batch_id` concept. A reservation is already the batch-level object that groups purpose, planned paths, queued/reserved state, and claimable write scope. Claims and edit/write authorization should point at that reservation id.

```text
session_id
  └─ reservation_id   // batch id
       ├─ planned paths
       ├─ claims
       └─ edit/write authorization
```

`session_id` remains on the reservation as owner/audit context, but write authorization keys off `reservation_id`.

## Core Semantics

`reservation_id` is the authority root for a write batch.

```text
reservation declare/request
  -> creates or returns reservation_id

claim acquire/claim queued reservation
  -> requires reservation_id
  -> creates claims under that reservation_id

edit/write
  -> requires reservation_id
  -> allowed only when every target path has an active claim under that reservation_id
```

Authorization rule:

```text
write allowed iff
  reservation_id is active
  AND target path is in the reservation's planned scope
  AND target path has an active claim under the same reservation_id
```

The essential equality check becomes:

```text
claim.reservation_id == write.reservation_id
```

## Session Semantics

`session_id` is no longer the primary write-authority key. It still records who owns and receives lifecycle events for the reservation.

Keep `session_id` for:

- reservation owner identity
- notification and resume target
- heartbeat and finalization cleanup
- audit/event grouping
- context rendering
- parent/subagent attribution where available

Do not require agents to keep passing `session_id` through every reservation/claim/edit call when the adapter already knows the current session from lifecycle hooks.

## API Shape

Agent-facing MCP should prefer reservation-id flow:

```text
state_reservation_declare(files_planned, purpose) -> reservation_id
state_reservation_request(request_id, action, path, purpose) -> reservation_id plus queued/reserved state
state_claim_acquire(reservation_id, paths) -> claims
edit/write(reservation_id, target)
```

Manual CLI can continue accepting explicit `--session-id` and `--workspace-id` for use outside an active hook-bound session, but active Codex/OMP/MCP flows should resolve current session internally and return the reservation id that future batch operations reuse.

## Queue And Resume

Queued requests should store and promote the same reservation id.

```text
request conflict
  -> queued reservation_id

resource released
  -> queued reservation_id becomes reserved/claimable

claim wait_id or reservation_id
  -> claims resources under that reservation_id

lazy_edit_resume/lazy_write_resume
  -> replays stored operation with reservation_id
```

Notification still needs `session_id` because the server must know who to wake. The write authority after wakeup is the promoted reservation id, not same-session matching at the write boundary.

## Migration Plan

1. Ensure every reservation declaration/request has a stable `reservation_id` returned to callers.
2. Add `reservation_id` to claim records and claim acquisition responses.
3. Change write authorization to verify reservation scope and claims by `reservation_id`.
4. Keep `session_id` on reservations and events for owner/audit/notification.
5. Update MCP/CLI active-session helpers so agent-facing calls can omit `session_id` while still recording the current session as reservation owner.
6. Update lazy edit/write resume payloads to store and replay `reservation_id`.
7. Replace documentation language such as “same-session claim” with “same-reservation claim” where it describes write authorization.

## Compatibility

This is an internal authority-model cutover with a cleaner public workflow.

Compatibility rules:

- Existing explicit `session_id` arguments may remain accepted for manual CLI and protocol compatibility.
- New active-session docs and examples should not require agents to pass `session_id` for reservation/claim/edit flow.
- Event payloads may keep `session_id`; their meaning becomes owner/audit context, not write-authority proof.
- Do not introduce a second `batch_id` field or compatibility alias.

## Files Expected To Change

- `crates/stateful-core` and/or store models
  - Persist `reservation_id` on claim records and expose it in current-state data where needed.
- `crates/stateful-server`
  - Return stable reservation ids from declare/request paths and authorize writes by reservation id.
- `crates/stateful-cli`
  - Update MCP/CLI adapters and hook authorization code to thread reservation ids through claim/edit/write flows.
- `docs/state-model.md`, `docs/current-state-coordination.md`, `docs/architecture.md`, `docs/usage-reference.md`, and `README.md`
  - Reword same-session authorization semantics to same-reservation authorization.
- Targeted tests under affected crates
  - Cover same-reservation success, different-reservation denial, and active-session calls that omit explicit session id.

## Testing

Use TDD for implementation:

1. Add a failing authorization test where a claim from reservation A does not authorize a write using reservation B, even under the same session.
2. Add a passing authorization test where reservation A, claim A, and write A share `reservation_id`.
3. Add an MCP/CLI active-session test showing reservation/claim calls can omit explicit `session_id` and still record the current session as owner.
4. Add a resume/lazy replay test showing the stored operation resumes with the promoted reservation id.
5. Run only the targeted crate tests touched by the implementation.

## Non-Goals

- Do not add a new `batch_id` concept.
- Do not remove `session_id` from audit, notification, resume, or lifecycle records.
- Do not change workspace identity semantics.
- Do not implement multi-resource atomic queueing beyond the existing planned model.
- Do not add compatibility aliases for `batch_id`.
