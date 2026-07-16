# Final Closure Task 6 Report

## RED/GREEN
- RED: `cargo test -p stateful-cli --test hook full_successful_read_posts_start_and_complete_with_one_operation_id` failed because a normal read was classified `exact`.
- GREEN: `cargo test -p stateful-cli --test hook normal_read_posts_structural_completion_with_one_operation_id` passed after requiring a raw, full-file selector for `Exact`.
- GREEN: `cargo test -p stateful-cli --lib hook::write_lifecycle::tests` passed (2 tests): exact completion replay and multi-release retention.
- Focused `hook` and `outbox` binaries were launched together but exceeded the 360-second sandbox deadline; no pass result was observed.

## State-machine invariants
- Read start/complete and activity-finalize envelopes are persisted in the trusted outbox before POST, preserving their request UUIDs for replay.
- Write authorization is persisted before POST; all pending writes use synced temporary-file replacement and retain a frozen completion or recovery request until acknowledged.
- Replay reuses exact authorization/completion/release envelopes. A started intent without completion creates and freezes a `/v2/write/recover` envelope from captured targets rather than returning silently.
- A lifecycle record is removed only after all release requests succeed.

## Changes
- `crates/stateful-cli/src/hook.rs`: strict raw/full read evidence, selector target normalization, namespaced PostToolUse handling, mixed action denial, durable lifecycle posts, automatic Stop fallback, heartbeat and typed test-tool transitions.
- `crates/stateful-cli/src/hook/write_lifecycle.rs`: durable authorization state, atomic pending updates, frozen recovery replay, release retention.
- `crates/stateful-cli/src/outbox.rs`: exact-envelope trusted queue primitive.
- `crates/stateful-cli/tests/hook.rs`: normal read regression proof.

## Self-review and concerns
- Confirmed normal reads cannot create an exact after-fingerprint, raw selectors use the underlying file target, and Codex mixed patch targets are rejected rather than authorized using the first action.
- Independent review identified typed Testing transitions; these were added.
- Commit/push: `7645eca` pushed to `origin/presence-first-event-journal-v2`.
- Concern: the full focused hook/outbox command timed out in the sandbox and must be rerun successfully before final integration.
