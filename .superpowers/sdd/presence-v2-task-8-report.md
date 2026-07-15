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

- `cargo test -p stateful-server` — 38 route/protocol tests passed.
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
- Complete: maintenance expires every workspace's stale presence/handoff, reservations, claims, claimable waits, fences, observations, started write intents, human observations, notifications, and context deliveries without no-op receipts; each transition is journaled, replayable, and idempotent.
- Complete: notification delivery callbacks require the target identity and exact live sequence; acknowledged reconnects do not replay.

## Concerns

None.
