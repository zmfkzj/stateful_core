# Final Closure Task 1 Report

## Files
- `crates/stateful-store/src/activity.rs`
- `crates/stateful-store/src/handoff.rs`
- `crates/stateful-store/src/presence.rs`
- `crates/stateful-store/src/reservations.rs`
- `crates/stateful-store/tests/presence_handoff.rs`
- `crates/stateful-store/tests/coordination_aggregates.rs`

## RED
- `cargo test -p stateful-store --test presence_handoff`: failed as expected for missing activity fallback, immutable-registration rejection, actor ownership, and finalization waiter promotion.
- `cargo test -p stateful-store --test coordination_aggregates reservation_expiry_ends_child_claims_before_promoted_successor_acquires`: failed as expected because the expired parent claim stayed active.
- Focused resumed-lifecycle and retained-handoff identity tests failed as expected before their production changes.
- `cargo test -p stateful-store --test coordination_aggregates ttl_batch_finalization_releases_all_blockers_before_promoting_directory_waiter`: failed as expected once reservations were heartbeated past the presence TTL.

## GREEN
- `cargo test -p stateful-store --test presence_handoff`: 28 passed.
- `cargo test -p stateful-store --test coordination_aggregates`: 30 passed.

## Commit and Push
- Implementation commit: `e44e925` (`fix(store): close presence handoff lifecycles`).
- Push: `280ca99..e44e925` to `origin/presence-first-event-journal-v2` succeeded.

## Self-review
- Finalization builds one transaction-scoped event plan: handoff first, presence/coordination cleanup next, then waiter grants after every released blocker is included.
- Fresh presences replace a retained older handoff; duplicate terminal commands continue to replay their frozen receipts.
- Presence and retained-handoff identity comparisons include actor, actor type, owner, and parent lineage before journal mutation.
- Reservation expiry releases child claims before promotion; finalization grants use the existing waiter-promotion helper and preserve the stored `stop`/`ttl` fallback-cause payload.

## Concerns
- None.

## Follow-up P1: Activity Start Identity
- RED: `cargo test -p stateful-store --test presence_handoff activity_start_rejects_changed_identity_for_live_or_retained_presence` failed because `start_activity` accepted a different actor for a live presence.
- GREEN: `cargo test -p stateful-store --test presence_handoff` passed with 29 tests after `start_activity` validated live presence or relevant retained-handoff identity before `register_record`.
- Fix commit: `a68ebfa` (`fix(store): protect activity identity`); push `807bcf1..a68ebfa` to `origin/presence-first-event-journal-v2` succeeded.
