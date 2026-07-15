# Task 8 V2 Server Cutover Report

## Status

Complete. The server exposes only the locked `stateful.v2` surface: 21 POST routes and 4 flattened-identity GET routes. The earlier 18 POST / 7 GET shorthand was corrected to the Task 8 brief's authoritative 21 / 4 contract.

## Implementation

- Deleted the V1 router, legacy protocol envelope, legacy policy adapter, and V1 route tests.
- Added V2 router, generic POST `RequestEnvelope<T>` parsing, flattened GET `QueryEnvelope<Q>` validation, structured V2 errors, and bearer protection.
- Routed commands through Task 4–7 Store APIs, preserving Store command-receipt replay for identical mutation responses and rejecting UUID misuse.
- Added shared core reservation and thin-safety policy evaluation; awareness is the default and enforcement is explicit.
- Added runtime identity capabilities, readiness gated by schema/replay checks, lifecycle maintenance, presence/handoff queries, context delivery ACK, notifications, and outbox handlers.

## Verification

- `cargo test -p stateful-server` — 46 route/protocol tests passed.
- `cargo test -p stateful-core` — 46 tests passed.
- `cargo test -p stateful-store` — 107 tests passed.
- Active server source has no V1 or legacy symbols; route wiring contains exactly 21 POST and 4 GET V2 registrations; server source contains no direct projection writes.

## Contract Hardening RED/GREEN

- RED: `malformed_post_and_invalid_flattened_query_return_v2_errors` failed because Axum returned a non-V2 body for malformed JSON.
- GREEN: the same test passes after replacing framework JSON/query extractors with structured V2 extraction and validation.
- RED: `request_id_reused_across_mutation_routes_is_rejected` returned `400` instead of `409`; `sse_reconnect_acknowledges_live_sequence_without_replay` replayed an acknowledged live notification; and `every_v2_route_executes_a_real_store_flow` stopped on an incomplete flow.
- GREEN: all three pass after full-envelope receipt identity, target/sequence-bound notification delivery acknowledgement, and complete real-store route coverage.

## Hardening Status

- Complete: structured POST/GET V2 extraction; phase/action/claim/per-target/fingerprint/Store-clock authorization checks; workspace-scoped event SQL; structured caller 4xx and sanitized internal 5xx mapping; live SSE polling with `Last-Event-ID`; atomic receipted notification polling; authorization receipts bound to the full original authorization envelope.
- Complete: maintenance expires stale presence/handoff, reservations, claims, claimable waits, fences, observations, human observations, notifications, and context deliveries without no-op receipts; `Started` write intents remain unclassified until explicit fingerprint recovery.
- Complete: notification delivery callbacks require the target identity and exact live sequence; acknowledged reconnects do not replay.
- Complete: polling returns queued notifications without terminalizing delivery; only an explicit delivery callback or sequence acknowledgement prevents later poll/SSE replay.
- Complete: expired context acknowledgements cannot advance a cursor; resume runs maintenance; `Last-Event-ID` must belong to the target workspace; bearer failures use the V2 error envelope; corrupt journal metadata makes readiness return `503`.

## Delivery and Recovery Lifecycle Correction

### RED

- `lost_poll_response_redelivers_until_sequence_acknowledgement` failed: the second poll returned `[]` after the first poll journaled a terminal delivery.
- `unchanged_unknown_reconciliation_releases_fences_without_versions_or_peer_invalidations` failed: reconciliation created a resource version despite exact rereads matching every original `before`.
- `stale_heartbeat_finalizes_before_returning_missing_presence_without_reusing_receipts` and `stale_heartbeat_cannot_revive_presence_when_a_live_handoff_already_exists` failed: heartbeat refreshed a live record after TTL, including when a prior fallback handoff was still relevant.
- `successful_heartbeat_receipt_does_not_mask_later_presence_expiry` failed: retrying a previously successful heartbeat UUID after TTL replayed the frozen success receipt even though maintenance had finalized the presence.
- `maintenance_leaves_started_write_intents_unclassified_without_post_fingerprints` was already green: `started_at` is persisted as a time tuple while the maintenance predicate only reads RFC 3339 strings, leaving the unsafe classifier unreachable. The dead classifier and its maintenance call were removed rather than making that path reachable.

### GREEN

- Polling records no terminal delivery event; a new poll and SSE replay pending work until the target sends the sequence acknowledgement.
- No-change `OutcomeUnknown` reconciliation emits `Reconciled`, releases fences, and leaves resource versions and peer observations untouched; changed reconciliation remains versioning/invalidation-capable.
- Heartbeat first executes stale presence expiry through a fresh system-maintenance request and verifies that presence still exists before consulting the caller receipt, then returns the inner V2 `presence_not_found` error. Retries of both previously failed and previously successful caller UUIDs, plus a stale presence beside an existing live handoff, create no revival or false success.
- Generic maintenance no longer guesses a `Started` write outcome; explicit `recover_write_intent` remains the post-fingerprint authority.

### Additional Verification

- `StoreError::code()` forwards nested V2 codes, so the lifecycle regression and server protocol mapping both report `presence_not_found`.
- Focused verification passes: 20 presence/handoff tests, 17 freshness tests, 5 core freshness-policy tests, and 46 V2 server protocol tests.
