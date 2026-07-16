# Task 14 — StatefulBench V2 metrics

## Result

Implementation and the local credit-free test surface are complete. Docker qualification and the Docker E2E gate are blocked because no Docker daemon/socket is available on this host; no image identity or qualification receipt was fabricated.

## TDD evidence

### RED

- `stateful sandbox run --fs build --network disabled --write-dir realworld-v2-red --command 'python3 -m unittest discover -s crates/stateful-bench/scripts/tests -t crates/stateful-bench/scripts -p "test_statefulbench_realworld.py" -v'`
  - 130 tests; 5 failures and 1 error before the V2 runner fixtures and V2 contract were aligned.
- `stateful sandbox run --fs build --network disabled --write-dir realworld-v2-docker-red --command 'python3 -m unittest discover -s crates/stateful-bench/scripts/tests -t crates/stateful-bench/scripts -p "test_statefulbench_docker.py" -v'`
  - 50 tests; 5 errors and 1 skip before legacy V1 diagnostics coverage was replaced.

### GREEN

- `stateful sandbox run --fs build --network disabled --write-dir realworld-v2-script-green --command 'python3 -m unittest discover -s crates/stateful-bench/scripts/tests -t crates/stateful-bench/scripts -p "test_statefulbench*.py" -v'`
  - 190 passed, 1 skipped (the opt-in Docker E2E, pending the image).
- `stateful sandbox run --fs build --network enabled --write-dir stateful-bench-v2-cargo-green-network --command 'cargo test -p stateful-bench'`
  - 140 passed.

The first Rust invocation with `--network disabled` had one unrelated sandbox-only failure: the DeNovo proxy test could not bind a local TCP socket. The rerun above allowed sockets and passed.

An unrestricted `unittest discover` invocation found five unrelated modules requiring `pytest` and top-level `conftest`; the supported StatefulBench pattern above is green.

## V2 metric and sanitization evidence

- `V2DiagnosticContractTests.test_v2_snapshot_emits_only_locked_value_free_metrics` constructs a V2 journal/projection SQLite snapshot, asserts every locked field, sorted categories, lifecycle and wait counters, and checks that private agent IDs, paths, payload text, timestamps, messages, and operation IDs are absent from the complete serialized snapshot.
- `V2RunnerMetricContractTests.test_parallel_on_rows_require_complete_v2_phase_metrics` requires the exact locked object for before-tasks, after-tasks, and after-final; missing a field raises and makes the on-arm row uncleared.
- `V2RunnerMetricContractTests.test_aggregate_nulls_uncleared_rows_and_weights_wait_means` proves totals/count-based wait means and null aggregate metrics for an uncleared scheduled trial.
- Docker runtime preparation removes `$STATEFUL_HOME` for every arm and starts parallel-on with `stateful server start --coordination-mode awareness`; the associated test asserts both calls.
- Sequential and parallel-off rows retain `coordination_metrics: null`.

## Changed files

- `crates/stateful-bench/scripts/statefulbench_container_diagnostics.py`
- `crates/stateful-bench/scripts/statefulbench_docker.py`
- `crates/stateful-bench/scripts/statefulbench_realworld.py`
- `crates/stateful-bench/scripts/tests/test_statefulbench_docker.py`
- `crates/stateful-bench/scripts/tests/test_statefulbench_realworld.py`
- `.superpowers/sdd/presence-v2-task-14-report.md`

## Docker gate status

The prescribed build command could not start: `/Users/arthur/.colima/default/docker.sock` does not exist, and `stateful sandbox process find --name docker` returned `[]`. Therefore:

- image ID/platform/repository digests: unavailable;
- `requests` qualification receipt: not run;
- credit-free Docker E2E with `STATEFULBENCH_DOCKER_TEST_IMAGE=statefulbench-realworld:presence-v2`: not run.

No model-backed benchmark was launched. `git status --short` showed no changes under `datasets/statefulbench-realworld/`.

## Checklist review

Steps 1–4 are evidenced above. Steps 5–7 require a running Docker daemon and must be rerun in sequence with one inspected `linux/arm64` image before this task can be declared fully qualified. Step 8 remains pending that gate.

## Independent review finding

- **Allowlist every published metric category (P1).** `_safe_category`
  currently accepts any regex-shaped journal event, handoff status,
  authorization reason, or wait status, and the runner normalization accepts
  the same arbitrary keys. A value such as `customer_secret` can therefore
  escape through `journal.by_event_type` into published output. Extraction
  must emit only contract-enumerated categories, normalization must reject
  unknown keys, and privacy tests must cover unknown non-notification
  categories.

### P1 resolution

- **Root cause:** extraction and normalization treated a regex match as category authorization. The closed V2 journal/handoff enums and the metric-specific notification, authorization, and wait-status contracts were not enforced at either boundary.
- **Fix:** both scripts now use immutable per-map V2 allowlists. Extraction drops unknown journal event types, handoff statuses, authorization reasons, notification kinds, and wait statuses; normalization rejects unknown keys for each corresponding map before publishing or aggregation. The locked object shape, sorted maps, integer checks, weighted wait means, off-arm nulls, and incomplete-row clearing are unchanged.
- **RED:** `stateful sandbox run --fs build --network enabled --write-dir task14-red --command 'python3 -m unittest crates.stateful-bench.scripts.tests.test_statefulbench_realworld.RealWorldRunnerTests.test_coordination_metrics_reject_unknown_category_keys crates.stateful-bench.scripts.tests.test_statefulbench_docker.V2DiagnosticContractTests.test_v2_snapshot_emits_only_locked_value_free_metrics'` ran 2 tests and failed 7 assertions: all six normalization maps accepted `customer_secret`, and extraction emitted unknown authorization categories.
- **GREEN:** `stateful sandbox run --fs build --network enabled --write-dir task14-green-rerun --command 'python3 -m unittest crates.stateful-bench.scripts.tests.test_statefulbench_realworld.RealWorldRunnerTests crates.stateful-bench.scripts.tests.test_statefulbench_docker.V2DiagnosticContractTests'` ran 27 tests in 0.731s: `OK`.
- **Residual blocker:** Docker qualification remains blocked by the missing `/Users/arthur/.colima/default/docker.sock`; no Docker image identity, qualification receipt, or E2E result was fabricated.
