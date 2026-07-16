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
