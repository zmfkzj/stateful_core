### Task 9: CLI V2 request identity repair

**Status:** Complete for the CLI/runtime/lifecycle scope. Server-side PID proof remains separately owned.

#### RED evidence

- `stateful sandbox run --fs build --network enabled --write-dir task9-cli-red-runtime --command 'cargo test -p stateful-cli --test runtime'` exited 101: **20 passed, 5 failed**. The failures proved regenerated request IDs for request/cancel, a per-file declaration returning the last reservation identity, missing workspace status fields, and `current` using unknown repo identity.
- `stateful sandbox run --fs build --network enabled --write-dir task9-cli-red-lifecycle --command 'cargo test -p stateful-cli --test server_lifecycle'` exited 101: **20 passed, 2 failed**. The failures proved restart discarded `enforcement` mode and evaluated a non-HTTP runtime as remote before refusing its scheme.
- `stateful sandbox run --fs build --network enabled --write-dir task9-cli-red-directory --command 'cargo test -p stateful-cli --test runtime declare_reservation_via_http_posts_one_task_envelope_and_returns_its_identity'` exited 101: **0 passed, 1 failed**. The declaration incorrectly collapsed a planned `src/generated/` directory scope into a file scope.

#### Implementation

- Reservation declare now sends one V2 envelope with all normalized file and directory scopes, preserves its caller UUID, and returns that single server response.
- Reservation claim/request/cancel and their protocol-body helpers retain supplied caller UUIDs; cancellation carries its server wait ID separately from its envelope UUID.
- `current` and `events` reuse enabled-repository discovery when available, producing complete repo/worktree/root/branch query identity.
- Runtime status includes server workspace ID and workspace version while preserving the V2 schema/mode/capability handshake.
- Runtime files retain coordination mode; restart derives host, port, token, workspace, and mode from that runtime, and rejects unsupported schemes before PID handling.

#### GREEN evidence

- `stateful sandbox run --fs build --network enabled --write-dir task9-cli-final --command 'cargo test -p stateful-cli --test runtime --test cli --test server_lifecycle'`: **runtime 25 passed; cli 56 passed; server_lifecycle 22 passed**.

#### Concerns

- No endpoint-to-PID proof was added; server ownership covers that separately.
- Hooks, install/OMP, watcher/outbox, documentation, benchmarks, and server integration tests were intentionally untouched.

### Task 9: Journal diagnostic footprint and growth

**Files:** `crates/stateful-cli/src/lib.rs`, `crates/stateful-cli/tests/cli.rs`.

#### RED evidence

- `stateful sandbox run --fs build --network disabled --write-dir task9-journal-red-rerun --command 'cargo test -p stateful-cli --test cli doctor_'` exited 101: **2 passed, 4 failed** (53 filtered), proving main-only footprint and permanent `baseline_unavailable`.
- `stateful sandbox run --fs build --network disabled --write-dir task9-baseline-write-red --command 'cargo test -p stateful-cli --test cli doctor_does_not_claim_a_baseline_when_atomic_replacement_fails'` exited 101: **0 passed, 1 failed** (59 filtered), proving a failed atomic replacement was reported as captured.

#### GREEN evidence

- `stateful sandbox run --fs build --network disabled --write-dir task9-journal-green-final --command 'cargo test -p stateful-cli --test cli doctor_'`: **7 passed, 0 failed** (53 filtered). It covers exact main+WAL footprint, bounded physical delta after a real persistent-store mutation, first/corrupt/future/expired recapture, atomic-replace failure truthfulness, byte-preserving doctor reads, sanitized output, and inclusive 512 MiB warning.

#### Concerns

- Existing Task 9 regressions were attempted: CLI passed **60/60**; runtime had **24 passed, 2 failed** in PID-identity assertions; standalone lifecycle had **22 passed, 1 failed** in mismatched-identity signaling. [INFERENCE] These failures are outside the changed files and journal diagnostic path; the disabled-network attempt also blocked all listener-binding runtime tests.

# Task 9 V2 CLI Cutover Report

## Status

Complete pending independent review. CLI runtime traffic, lifecycle hooks, outbox replay, watcher reconciliation, server discovery, and installed OMP integration use the locked `stateful.v2` protocol only.

## Implementation

- Added typed V2 POST/GET helpers with complete agent/workspace/source identity, request UUID validation, runtime identity/capability handshakes, and structured V2 error decoding.
- Migrated CLI command paths and server lifecycle discovery to `/v2/*`; unsupported runtimes fail before mutation.
- Split hook support into input, observation, delivery, and write-lifecycle modules; successful writes now persist start/completion identity and post-write fingerprints, while unresolved writes remain recoverable.
- Migrated OMP/Codex lifecycle events, context rendering/acknowledgement, reservation/claim flows, outbox replay, watcher human-write observation/reconciliation, sandbox authorization, commit authorization, and LAN join checks.
- Defaulted server/install operation to awareness mode while preserving explicit enforcement mode.
- Updated the installed OMP extension and its install assertions for V2 notification streaming, reservation delivery, request identity, and context acknowledgement.
- Removed every active `/v1/*` and `stateful.v1` marker from `crates/stateful-cli/src`.

## Verification

- `cargo test -p stateful-cli` — all CLI unit, integration, and doc tests passed: 121 library tests plus 60 CLI, 136 hook, 29 install, 13 LAN, 16 outbox, 27 runtime, 23 server-lifecycle, one real two-session TCP/replay test, and the remaining suites.
- `cargo test -p stateful-server` — all 47 V2 server integration tests passed.
- `cargo clippy -p stateful-cli --all-targets --no-deps -- -D warnings` — passed.
- Scoped `rustfmt --check` over every Task 9 modified Rust file — passed.
- `grep` over `crates/stateful-cli/src` for `/v1/` and `stateful.v1` — no matches.

## Repository-Wide Gate Note

`cargo fmt --check` and dependency-inclusive CLI clippy still expose pre-existing Task 1–8 cleanup outside the Task 9 diff: unformatted committed files across core/store/server/unchanged CLI tests, plus two core clippy findings. These belong to the plan's Task 12 full-workspace gate and do not affect the verified Task 9 behavior.

## Independent review findings to fix

All eight findings are blocking for Task 9:

1. **Replay failed write completions instead of heartbeats (P1).** If
   `/v2/write/complete` fails after the worktree changed, persist and replay the
   original completion/recovery request with its request and operation IDs.
   Queuing only `/v2/outbox/sync` leaves the write fence unresolved.
2. **Complete non-exact read operations (P2).** Every `/v2/read/start` must get a
   matching `/v2/read/complete`, including partial, truncated, and failed reads,
   so `read_operation_current` is cleared without creating a stable baseline.
3. **Authorize heterogeneous patch operations separately (P2).** Mixed
   write/delete/move patch targets must not inherit the first target's action.
   Split them into valid authorization/write-intent lifecycles or reject them
   before execution with explicit split guidance.
4. **Finalize already authorized sandbox intents on setup errors (P1).** Once a
   multi-target sandbox authorization returns an operation ID, every later
   authorization or writable-path setup failure must complete all accumulated
   intents as failed in both internal and external target paths.
5. **Finalize denied or unlaunchable benchmark intents (P1).** A benchmark
   denial with an intent ID, writable-path preparation failure, or cwd
   resolution failure must complete the captured intent as failed.
6. **Complete commit intents after the commit runs (P1).** Preserve lifecycle
   contexts through temporary-index setup and Git execution. Complete as
   committed only after success, and as failed on every error path.
7. **Retain claim-release recovery state until every claim is released (P1).**
   Persist a completed-but-releases-pending lifecycle state and replay
   unfinished claim releases idempotently; do not delete the only recovery
   record before releases succeed.
8. **Strip the OMP `:raw` selector before recording reads (P1).** Exact OMP reads
   must fingerprint and post lifecycle events for the underlying file
   (`src/lib.rs`), never the synthetic selector (`src/lib.rs:raw`).

## Review-finding closure

1. Fixed: failed write completions persist and replay the original V2 request and operation IDs.
2. Fixed: partial/truncated reads complete without establishing a stable baseline.
3. Fixed: OMP rejects heterogeneous patch actions before authorization with split guidance.
4. Fixed: internal and external sandbox setup failures complete accumulated intents as failed.
5. Fixed: denied benchmark intents and failures before launch complete as failed.
6. Fixed: commit write lifecycles are retained through temporary-index and Git execution, then complete according to the Git result.
7. Fixed: completed intents retain claim-release recovery requests until every release replays.
8. Fixed: OMP removes `:raw` before lifecycle fingerprinting and read events.

### Focused RED/GREEN evidence

- RED `cargo test -p stateful-cli --lib write_lifecycle::tests::retries_failed_write_completion_with_the_original_request_and_operation_ids`: 0 passed, 1 failed; GREEN `stateful sandbox run --fs build --network enabled --write-dir task9-review-green-lifecycle-recovery --command 'cargo test -p stateful-cli --lib write_lifecycle::tests::'`: 2 passed.
- RED `cargo test -p stateful-cli --test hook partial_or_truncated_read_completes_without_baseline`: 0 passed, 1 failed; GREEN `stateful sandbox run --fs build --network enabled --write-dir task9-review-green-read-complete-final --command 'cargo test -p stateful-cli --test hook partial_or_truncated_read_completes_without_baseline'`: 1 passed.
- RED mixed OMP patch authorization: 0 passed, 1 failed; GREEN `stateful sandbox run --fs build --network enabled --write-dir task9-review-green-mixed-action --command 'cargo test -p stateful-cli --test hook omp_edit_rejects_mixed_patch_actions_before_authorization'`: 1 passed.
- RED sandbox setup cleanup: 0 passed, 2 failed; GREEN `stateful sandbox run --fs build --network enabled --write-dir task9-review-green-sandbox-cleanup --command 'cargo test -p stateful-cli --lib setup_failure_completes_authorized_intents'`: 2 passed.
- RED benchmark denied intent: 0 passed, 1 failed; GREEN `stateful sandbox run --fs build --network enabled --write-dir task9-review-green-benchmark-denial-final2 --command 'cargo test -p stateful-cli --lib denied_benchmark_intent_completes_as_failed'`: 1 passed.
- GREEN commit lifecycle build/execution check: `stateful sandbox run --fs build --network enabled --write-dir task9-review-green-commit-lifecycle-build --command 'cargo test -p stateful-cli --test commit structured_commit_authorizes_deleted_files_as_delete_file'`: 1 passed.
- RED OMP `:raw`: 0 passed, 1 failed; GREEN `stateful sandbox run --fs build --network enabled --write-dir task9-review-green-raw-selector-final --command 'cargo test -p stateful-cli --test hook omp_raw_reads_use_the_underlying_file_for_lifecycle_fingerprints'`: 1 passed.

Changed files: `crates/stateful-cli/src/hook.rs`, `src/hook/write_lifecycle.rs`, `src/sandbox.rs`, `src/codex_benchmark.rs`, `src/commit.rs`, `tests/hook.rs`, and this report.

Residual concern: commit lifecycle ordering is covered by the focused existing commit execution test; no live V2 transport fixture was added for it.
- Final affected binaries: `stateful sandbox run --fs build --network enabled --write-dir task9-review-final-lib --command 'cargo test -p stateful-cli --lib'`: 126 passed; `stateful sandbox run --fs build --network enabled --write-dir task9-review-final-hook --command 'cargo test -p stateful-cli --test hook'`: 138 passed.

### Second independent review closure

#### RED evidence

- `stateful sandbox run --fs build --network enabled --write-dir task9-red-write-recovery --command 'cargo test -p stateful-cli --lib write_lifecycle::tests::retries_failed_write_completion_with_the_original_request_and_operation_ids'` exited 101 because the production `replay_pending` scanner did not exist.
- `stateful sandbox run --fs build --network enabled --write-dir task9-red-read-order --command 'cargo test -p stateful-cli --test hook partial_or_truncated_read_completes_without_baseline'` exited 101: **0 passed, 1 failed**; it observed `/v2/presence/update` before `/v2/read/complete`.
- `stateful sandbox run --fs build --network enabled --write-dir task9-red-denied-intent --command 'cargo test -p stateful-cli --test hook denied_v2_write_intent_completes_the_exact_started_intent_as_failed'` exited 101: **0 passed, 1 failed**; no failed completion arrived after a 200 V2 authorization response containing both an intent ID and `decision: deny`.
- `stateful sandbox run --fs build --network enabled --write-dir task9-red-sandbox-transport --command 'cargo test -p stateful-cli --lib second_authorization_transport_failure_completes_the_first_started_intent'` initially exited 101 before the CLI test ran because concurrent core work temporarily omitted `actor_id` from `ReadObservationRecord` literals. The focused test was written before the cleanup implementation; its GREEN run below exercised the intended second-authorization transport failure.

#### GREEN evidence

- `stateful sandbox run --fs build --network enabled --write-dir task9-final-write-recovery-current --command 'cargo test -p stateful-cli --lib write_lifecycle::tests::'`: **2 passed**. It scans persisted records, replays the frozen completion request unchanged, retains two releases independently after the second fails, and deletes only after the replay succeeds.
- `stateful sandbox run --fs build --network enabled --write-dir task9-final-sandbox-recovery --command 'cargo test -p stateful-cli --lib second_authorization_transport_failure_completes_the_first_started_intent'`: **1 passed**. It observes the first exact intent completed failed after the second authorization transport failure.
- `stateful sandbox run --fs build --network enabled --write-dir task9-final-benchmark-denial --command 'cargo test -p stateful-cli --lib denied_benchmark_intent_completes_as_failed'`: **1 passed**. It parses the completion request body and verifies `intent_id: intent-1` with `outcome: failed`.
- `stateful sandbox run --fs build --network enabled --write-dir task9-final-hook --command 'cargo test -p stateful-cli --test hook'`: **140 passed**. It covers required completion before optional presence/heartbeat, presence-drop ordering, and exact failed-intent bodies.
- `stateful sandbox run --fs build --network enabled --write-dir task9-final-commit --command 'cargo test -p stateful-cli --test commit structured_commit_v2_completion_follows_the_git_outcome'`: **1 passed**. A real V2 fake transport observed no completion until Git had succeeded, then `committed`; a pre-commit setup failure left Git at its original commit count and completed `failed`.
- `stateful sandbox run --fs build --network enabled --write-dir task9-final-cli --command 'cargo test -p stateful-cli --lib --test hook --test commit'`: **127 library, 140 hook, and 29 commit tests passed**.

#### Residual risks

- A record that contains only a Started intent and no frozen completion request is retained, not guessed at or deleted. The later scan replays only frozen requests so it cannot generate a new request or operation ID.
- A corrupt record remains on disk and causes a replay warning; valid sibling records are still scanned. Repairing irrecoverably corrupt JSON remains an operator action.
- The earlier claim that commit ordering had no live V2 fixture is superseded by `structured_commit_v2_completion_follows_the_git_outcome`.
