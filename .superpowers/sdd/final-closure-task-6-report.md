# Final Closure Task 6 Report

## Status
- Implementation commit: `ecc6734` (`Fix durable CLI hook lifecycles`).
- Push: implementation and this report commit pushed to `origin/presence-first-event-journal-v2`.

## RED/GREEN
- RED: `cargo test -p stateful-cli testing_command_accepts_only_executed_supported_test_grammar` proved `python -m pytest` was incorrectly reported instead of canonical `pytest`.
- RED: `cargo test -p stateful-cli --test hook omp_namespaced_nested_raw_read_selectors_use_file_target_and_are_partial` exposed nested-selector target parsing and the missing OMP non-exact `tool_result`.
- RED: `cargo test -p stateful-cli --test hook denied_recognized_test_commands_do_not_emit_testing_starts` showed denied raw Codex commands entered Testing.
- RED: `cargo test -p stateful-cli --test hook lost_read_start_response_queues_failed_completion_and_replays_frozen_envelopes` showed an ambiguous read-start left no terminal completion.
- RED: `cargo test -p stateful-cli --test hook lost_read_complete_response_replays_the_frozen_completion_after_restart` showed first delivery used bytes different from replay.
- RED: `cargo test -p stateful-cli --test outbox sync_outbox_discards_deterministic_client_rejections` showed deterministic HTTP 400 records replayed forever.
- GREEN: `cargo test -p stateful-cli --test hook` — 148 passed.
- GREEN: `cargo test -p stateful-cli --test outbox` — 17 passed.
- GREEN: `cargo test -p stateful-cli hook::write_lifecycle::tests` — 5 passed.

## State-machine invariants
- Durable read start, read complete, and activity finalize serialize once, queue before first send, and use those same bytes and request UUIDs for first delivery and replay.
- An ambiguous read-start queues a failed completion immediately behind its start with the same operation ID, preventing a replay orphan.
- Lifecycle replay preserves per-agent sequence order; deterministic 4xx responses are discarded while 408 and 429 remain retryable.
- Exact reads require successful, complete, untruncated raw full-file evidence. Nested line selectors are stripped from the lifecycle target and remain Partial.
- Testing presence starts only after a recognized Bash command is permitted; Codex and OMP emit typed start/result envelopes and refresh heartbeat after their successful post-tool paths.
- Pending write state is fsynced to a temporary file and atomically renamed; authorization, completion/recovery, and every frozen release remain stable until acknowledged.

## Files
- `crates/stateful-cli/src/hook.rs`
- `crates/stateful-cli/src/hook/write_lifecycle.rs`
- `crates/stateful-cli/src/outbox.rs`
- `crates/stateful-cli/tests/hook.rs`
- `crates/stateful-cli/tests/outbox.rs`
- `.superpowers/sdd/final-closure-task-6-report.md`

## Self-review and concerns
- Verified raw full reads fingerprint the underlying path; namespaced Codex and OMP reads complete their lifecycle against that target.
- Verified unknown Stop sends null for automatic fallback and explicit handoff objects are retained unchanged.
- Concern: terminal 4xx retention intentionally excludes only 408/429; any server-specific retryable 4xx must use one of those status codes.
