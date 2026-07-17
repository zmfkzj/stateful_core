# Final Closure Task 6 Report

## Status
- Implementation commit: `9449940` (`Guard direct write lifecycle origin`).
- Push: implementation and this report commit pushed to `origin/presence-first-event-journal-v2`.

## RED/GREEN
- RED: `cargo test -p stateful-cli testing_command_accepts_only_executed_supported_test_grammar` proved `python -m pytest` was incorrectly reported instead of canonical `pytest`.
- RED: `cargo test -p stateful-cli --test hook omp_namespaced_nested_raw_read_selectors_use_file_target_and_are_partial` exposed nested-selector target parsing and the missing OMP non-exact `tool_result`.
- RED: `cargo test -p stateful-cli --test hook denied_recognized_test_commands_do_not_emit_testing_starts` showed denied raw Codex commands entered Testing.
- RED: `cargo test -p stateful-cli --test hook lost_read_start_response_queues_failed_completion_and_replays_frozen_envelopes` showed an ambiguous read-start left no terminal completion.
- RED: `cargo test -p stateful-cli --test hook lost_read_complete_response_replays_the_frozen_completion_after_restart` showed first delivery used bytes different from replay.
- RED: `cargo test -p stateful-cli --test outbox sync_outbox_discards_deterministic_client_rejections` showed deterministic HTTP 400 records replayed forever.
- GREEN: `cargo test -p stateful-cli --test hook` — 148 passed.
- GREEN: `cargo test -p stateful-cli --test outbox` — 18 passed.
- GREEN: `cargo test -p stateful-cli hook::write_lifecycle::tests` — 9 passed.
- Review RED: a start and fallback completion were appended independently, allowing a crash-visible start-only outbox; generic heartbeat replay lost structured 404 status; recovery fell back to a triggering repository when no captured root existed.
- Review GREEN: `cargo test -p stateful-cli durable_read_start_pair_persists_start_and_failed_completion_before_send` — the pair is atomically persisted before a dropped first response.
- Review GREEN: `cargo test -p stateful-cli --test outbox sync_outbox_discards_a_structured_404_heartbeat_and_sends_the_next_record` — raw 404 is discarded and the following heartbeat record is sent.
- Review GREEN: `cargo test -p stateful-cli recovery_` — 4 passed, including no fallback from an unknown captured root.
- Review RED: new writes derived recovery roots from envelope identity and successful pair acknowledgement treated empty-file cleanup as durable failure.
- Review GREEN: `cargo test -p stateful-cli identityless_authorization_captures_the_actual_root_for_recovery` — new writes capture the actual canonical root; legacy pending records without an origin remain preserved rather than guessed.
- Review GREEN: `cargo test -p stateful-cli durable_read_start_pair_persists_start_and_failed_completion_before_send` — a dropped start response leaves the pair durable; empty-file unlink is nonfatal after the atomic acknowledgement rewrite.
- Review RED: global pending write replay used the trigger repository's runtime for every record.
- Review GREEN: `cargo test -p stateful-cli pending_intent_bound_to_another_origin_is_not_replayed` — records persist their EnvOverride/Global/RepoLocal origin and a mismatch is retained without network delivery.
- Fixture RED: strict runtime reuse correctly rejected test-only fake global runtimes with PID `42`, while isolated hook subprocesses had cleared environment and could not prove ownership.
- Fixture GREEN: focused subprocess lifecycle regressions use the fixture runtime as an explicit environment override, preserving the intended fake-server contract under strict reuse.
- Review RED: direct authorization retry, completion, and frozen release paths could reload a pending origin-A envelope and post it to current origin B.
- Review GREEN: `cargo test -p stateful-cli direct_lifecycle_posts_reject_a_changed_runtime_origin_and_allow_same_origin_restart` — changed-origin retries preserve pending state with zero B requests; a restarted server in the same Global origin accepts the original frozen authorization.

## State-machine invariants
- Read start and its failed fallback completion are serialized and atomically persisted as one pair under the global outbox lock before start dispatch; success removes both in one locked rewrite, while error leaves both for ordered replay.
- Lifecycle replay preserves per-agent sequence order; generic `/v2/outbox/sync` uses raw transport so structured 4xx status is retained and discarded, while exact lifecycle records retain canonical frozen serialization. 408 and 429 remain retryable.
- Write recovery uses only an authorization-captured absolute root. Missing, relative, or `unknown` roots fail safely while preserving the pending intent and fences.
- New write authorizations capture their actual canonical repository root before posting, independent of envelope identity; a bound legacy record may backfill a known absolute frozen authorization root, while an unbound record is never guessed.
- Pending writes capture the selected runtime origin, not a process endpoint. Replay identifies the current trigger origin and sends only exact-origin records, preserving legacy unbound or mismatched records; matching Global and RepoLocal origins use current runtime discovery after restart.
- Every direct authorization, completion, denial-release, and claim-release path checks the current runtime origin against the persisted origin before its first lifecycle POST; mismatch errors retain the pending record.
- Hook subprocess fixtures expose their explicitly configured fake runtime as an environment override; this is test-only alignment with the production no-fallback runtime ownership policy.
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
