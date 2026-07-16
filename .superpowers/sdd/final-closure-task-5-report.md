# Final Closure Task 5 Report

## Status
DONE

## Implementation
- SHA: `d8d9ede` (`fix: freeze server authorization denials`)
- `/v2/authorize` now records denied decisions as canonical `authorization.denied` events and frozen `server.authorize` receipts. The replay response remains the original bare `Decision` JSON and status; allow/warn replies retain the existing `{intent_id, fence_ids, decision}` shape.
- Denial audit is the sole event in a denial transaction; warning audit precedes intent, planned-resource, and fence events. Projector failure rolls back the audit and receipt together.
- Action validation precedes freshness/authority policy. Expired, invalidated, partial, and structural evidence is missing evidence; only a current exact baseline that changed remains a hard stale denial.
- Awareness persists overlapping reservation/claim state with a `coordination_conflict` warning and canonical audit. Enforcement retains the denial. Raw V1 query envelopes return `unsupported_protocol` on every V2 GET route.

## RED
- `cargo test -p stateful-server --test v2_coordination` failed before implementation for frozen denied replay, awareness overlap, and incomplete evidence.
- `cargo test -p stateful-server --test v2_protocol v1_query_envelopes_are_unsupported_on_every_v2_get_route` failed with `invalid_query_envelope` before raw V1 handling.
- `cargo test -p stateful-store --test freshness projector_failure_rolls_back_denied_authorization_audit_and_receipt` failed to compile before the receipt-backed authorization API existed.
- Focused action-order and invalidated-evidence regressions were observed failing before their minimal policy changes.

## GREEN
- `cargo test -p stateful-server --test v2_coordination` — 9 passed.
- `cargo test -p stateful-server --test v2_protocol` — 37 passed.
- `cargo test -p stateful-store --test freshness` — 30 passed.
- `cargo test -p stateful-store --test coordination_aggregates` — 30 passed.
- `cargo test -p stateful-store --test task9_authority` — 9 passed.

## Review and Push
- Independent Task 5 review: no findings.
- Implementation commit pushed to `origin/presence-first-event-journal-v2`.

## Concerns
- None.
