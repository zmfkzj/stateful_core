# Task 6 report — exact-read freshness and thin safety

## RED

- `cargo test -p stateful-store --test freshness` initially failed because the read-observation, write-intent, resource-version, and lifecycle APIs did not exist.
- `cargo test -p stateful-core --test freshness_policy` failed after the evaluator was reduced to an unconditional allow: changed/missing observations and every thin hard stop incorrectly allowed.
- The exact-read 60-minute expiry assertion failed against the prior 30-minute TTL, and a structural summary carrying `symbols:fn main` incorrectly stabilized.
- The finalization test failed while finalized presence did not remove its stable read observations.

## GREEN

- `stateful sandbox run --fs build --network disabled --write-dir v2-freshness-core-final --command 'cargo test -p stateful-core'` — pass.
- `stateful sandbox run --fs build --network disabled --write-dir v2-freshness-store-suite-final --command 'cargo test -p stateful-store'` — pass.
- Focused store freshness suite: 16 passing tests covering complete exact reads, incomplete classifications, concurrent operation IDs, the 60-minute expiry boundary, write intent/fence atomicity, committed version/invalidation/replay, failed release, unknown-outcome recovery/reconciliation, session finalization, and the semantic-marker structural-summary regression.
- Current focused verification: `cargo test -p stateful-store --test freshness` — 16 passed; `cargo test -p stateful-core --test freshness_policy` — 4 passed.

## Self-review

- Every observation, intent, fence, version, invalidation, and release transition is journaled through `Store::execute_command`; projection SQL is confined to `Projector`.
- Read completion pairs only the same workspace/actor/path/operation ID from `read_operation_current`; it never uses a latest-path heuristic.
- Exact stability requires equal complete fingerprints from a full exact read. Semantic markers remain recorded but never affect stabilization; structural summaries, partial, truncated, failed, and ambiguous outcomes remain unstable.
- Committed writes increment resource versions, invalidate other stable observations, create the writer’s stable observation, and release their fences in the same transaction. Replay is covered.
- `evaluate_thin_safety` is the authoritative pure evaluator. It preserves invalid-target, unknown-outcome, stale-observation, active-fence, and human-change stops as denials in awareness mode; only missing/expired provenance becomes a warning.
- Existing exact-file/delete/directory/nested scope coverage remains in the core policy suites. `server/policy_service.rs` adds only a thin delegating adapter to the core evaluator, avoiding a Task 8 route/runtime cutover.

## Deferred concern

`cargo test -p stateful-server --test routes --no-run` remains blocked by the intentional Task 5→Task 8 V1/V2 server cutover gap: 157 pre-existing compile errors, including unresolved old store imports and removed V1 Store methods in `server/lib.rs` and `policy_service.rs`. No server routes or runtime were changed for Task 6.

## Review fixes — unknown write reconciliation

- RED: new focused regressions failed for stale pre-unknown reads, recovery version/invalidation projection, incomplete present fingerprints, and the expiry boundary.
- GREEN: a changed unknown outcome now requires an exact stable reread whose `origin_event_seq` follows the `OutcomeUnknown` event; its reconciliation derives resource versions from those rereads, invalidates peer observations, updates the reread’s resource version, and releases intent/fences in the same journal transaction.
- Complete present fingerprints require a 64-character hexadecimal SHA-256; canonical missing fingerprints remain allowed where a missing target is valid.
- `cargo test -p stateful-core` and `cargo test -p stateful-store` pass.

## Review concerns

- None.
