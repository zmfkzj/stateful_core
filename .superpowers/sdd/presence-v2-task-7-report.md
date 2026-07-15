# Presence V2 Task 7 Report

## Status

Implemented core and store versioned coordination-context delivery on canonical V2 journal/projection state, with follow-up hardening for delivery sequence identity, notification lifecycle context effects, and claimable waits.

## Implementation commits

`8657b1f feat: deliver versioned coordination context`

`8a0b81e fix: harden versioned context delivery`

## Delivered behavior

- `ContextDelta` carries version range, delivery ID/sequence, structured current-state items, and immutable prompt text.
- Store rendering reads the ACK cursor, selects context-affecting aggregate identities after it, and renders final projection state only. Render audit events do not advance the workspace version or mark a delivery delivered.
- Delivery records are scoped by workspace and target agent. Re-render returns the same frozen delivery until a matching ACK.
- ACK validates delivery ID, sequence, version, agent, and workspace; it advances the cursor monotonically and cumulatively. Older ACKs leave newer deliveries pending; duplicate ACKs are inert.
- New versions create a new delivery, preserve prior snapshot content, supersede old unread records, and coalesce only queued `context_invalidated` notifications for the same target and kind.
- Empty final-state deltas still create an ACK-required delivery. Presence, handoff, reservation, claim, wait, fence, human-write, and unknown-write final state are eligible delta context.
- Unacknowledged deliveries expire after 24 hours through journaled recovery events into replayable `dead_letter` state. At 21 unread deliveries, the stored prompt becomes an exact queue summary; the 65th is dead-lettered.
- Added `context_delivery_current` projection and index. Projectors alone write it; commands only append canonical events through `execute_command`.

### Follow-up hardening

- Notification coalescing now advances its target-agent sequence. Delivery callbacks carry that sequence and stale callbacks are accepted inertly rather than marking a newer coalesced payload delivered.
- Context-invalidated notification and recovery transport lifecycle events do not advance the canonical workspace context version.
- Claimable waits render as a concrete actionable item. The recipient's own coordination records use the active-scope source reference and informational severity, instead of implying another-agent conflict.

## Verification

`stateful sandbox run --fs build --network disabled --write-dir task7-core-store-final3 --command 'cargo test -p stateful-core -p stateful-store'`

Passed: all Stateful Core and Stateful Store unit, integration, and doc-test suites, including 11 focused context-delivery tests.

`stateful sandbox run --fs build --network disabled --write-dir task7-server-check --command 'cargo check -p stateful-server'`

Failed before Task 7 integration: the current server contains 157 V1-store API mismatch errors across `lib.rs` and `policy_service.rs`; no server files were changed for this task.

## Concerns

Task 8 server routes and all HTTP/runtime/CLI/hooks/OMP/VSCode work were intentionally untouched. The Store APIs are ready for server wiring. The report path is gitignored, so it is force-added below as requested.
