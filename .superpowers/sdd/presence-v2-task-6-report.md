# Task 6 report — exact-read freshness and thin safety

## RED

- `cargo test -p stateful-store --test freshness` initially failed because the read-observation, write-intent, resource-version, and lifecycle APIs did not exist.
- `cargo test -p stateful-core --test freshness_policy` failed after the evaluator was reduced to an unconditional allow: changed/missing observations and every thin hard stop incorrectly allowed.
- The exact-read 60-minute expiry assertion failed against the prior 30-minute TTL, and a structural summary carrying `symbols:fn main` incorrectly stabilized.
- The finalization test failed while finalized presence did not remove its stable read observations.

## GREEN

- `stateful sandbox run --fs build --network disabled --write-dir v2-freshness-core-final --command 'cargo test -p stateful-core'` — pass.
- `stateful sandbox run --fs build --network disabled --write-dir v2-freshness-store-suite-final --command 'cargo test -p stateful-store'` — pass.
- Focused store freshness suite: 17 passing tests covering complete exact reads, incomplete classifications, concurrent operation IDs, the 60-minute expiry boundary, write intent/fence atomicity, committed version/invalidation/replay, failed release, unknown-outcome recovery/reconciliation, session finalization, and structural-summary regressions.
- Current focused verification: `cargo test -p stateful-store --test freshness` — 17 passed; `cargo test -p stateful-core --test freshness_policy` — 5 passed. A persisted pre-fix structural observation whose stored status says `stabilized` is not stable or fresh authority; the server classifies it as unstable so awareness mode retains the hard stop, while maintenance still expires the stale row at its stored deadline.

## Self-review

- Every observation, intent, fence, version, invalidation, and release transition is journaled through `Store::execute_command`; projection SQL is confined to `Projector`.
- Read completion pairs only the same workspace/actor/path/operation ID from `read_operation_current`; it never uses a latest-path heuristic.
- Exact stability requires equal complete fingerprints from a full exact read. Semantic markers remain recorded but never affect stabilization; structural summaries, partial, truncated, failed, and ambiguous outcomes remain unstable.
- Committed writes increment resource versions, invalidate other stable observations, create the writer’s stable observation, and release their fences in the same transaction. Replay is covered.
- `evaluate_thin_safety` is the authoritative pure evaluator. It preserves invalid-target, unknown-outcome, stale-observation, active-fence, and human-change stops as denials in awareness mode; only missing/expired provenance becomes a warning.
- Existing exact-file/delete/directory/nested scope coverage remains in the core policy suites. The server integration lives in `crates/stateful-server/src/commands.rs`, where `authorize_request` builds `ThinSafetyState` through `thin_safety_state` and delegates evaluation to `evaluate_thin_safety` rather than duplicating policy.

## Integration status

Task 6 intentionally left the HTTP/runtime cutover to Task 8. The current V2 server command path now consumes the core evaluator from `crates/stateful-server/src/commands.rs`; there is no separate `policy_service.rs` adapter.

## Review fixes — unknown write reconciliation

- RED: new focused regressions failed for stale pre-unknown reads, recovery version/invalidation projection, incomplete present fingerprints, and the expiry boundary.
- GREEN: a changed unknown outcome now requires an exact stable reread whose `origin_event_seq` follows the `OutcomeUnknown` event; its reconciliation derives resource versions from those rereads, invalidates peer observations, updates the reread’s resource version, and releases intent/fences in the same journal transaction.
- Complete present fingerprints require a 64-character hexadecimal SHA-256; canonical missing fingerprints remain allowed where a missing target is valid.
- `cargo test -p stateful-core` and `cargo test -p stateful-store` pass.

## Review concerns

- None.
