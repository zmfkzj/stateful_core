# Task 14 — StatefulBench V2 metrics

## Result

Implementation, the local credit-free test surface, immutable `linux/arm64` image qualification, and the credit-free Docker E2E gate are complete.

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
- `STATEFULBENCH_DOCKER_TEST_IMAGE=statefulbench-realworld:presence-v2 python3 -m unittest discover -s crates/stateful-bench/scripts/tests -t crates/stateful-bench/scripts -p "test_statefulbench_docker.py" -v`
  - 47 tests passed, including the opt-in Docker E2E, against the qualified image.

The first Rust invocation with `--network disabled` had one unrelated sandbox-only failure: the DeNovo proxy test could not bind a local TCP socket. The rerun above allowed sockets and passed.

An unrestricted `unittest discover` invocation found five unrelated modules requiring `pytest` and top-level `conftest`; the supported StatefulBench pattern above is green.

## V2 metric and sanitization evidence

- `V2DiagnosticContractTests.test_v2_snapshot_emits_only_locked_value_free_metrics` constructs a V2 journal/projection SQLite snapshot, asserts every locked field, sorted categories, lifecycle and wait counters, and checks that private agent IDs, paths, payload text, timestamps, messages, and operation IDs are absent from the complete serialized snapshot.
- `V2RunnerMetricContractTests.test_parallel_on_rows_require_complete_v2_phase_metrics` requires the exact locked object for every diagnostic phase from `initialized` through `before-remove`; missing or non-monotonic evidence makes the on-arm row uncleared.
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

The current checkout was built once as `statefulbench-realworld:presence-v2`. Because the external sandbox correctly denied Docker Buildx writes under `$HOME/.docker`, the successful build set `BUILDX_CONFIG` inside the declared benchmark write directory. Inspection returned:

- image ID: `sha256:14512ad1c659d399d2b6fd190634be1c563889fef9499c7527714796b3a8c170`;
- platform: `linux/arm64`;
- repository digest: `statefulbench-realworld@sha256:14512ad1c659d399d2b6fd190634be1c563889fef9499c7527714796b3a8c170`.

`requests` qualification exited 0. The receipt at `$HOME/.cache/statefulbench-realworld-presence-v2/cache/qualification/receipts/requests.json` records `qualified: true`, the same image identity/platform/digest, all graded-input hashes, archive and corpus hashes, and the six-tool map.

The first credit-free E2E invocation exposed a missing sandbox temporary-directory prerequisite and failed before test setup. After creating the declared write directory's `.stateful-tmp`, the fresh invocation exited 0 with all 47 tests passing, including `DockerEndToEndTests.test_all_arms_share_home_grade_and_cleanup` and the V2 diagnostic contract tests.

No model-backed benchmark was launched. The frozen dataset was not edited.

## Checklist review

Steps 1–7 are now evidenced. Step 8 is this report-only closure commit and push; no implementation or frozen-corpus file changed during the Docker gate.

## P1 admission finding closure

1. **Late failures leaked on-arm metrics.** Metrics were assembled before post-suite, evaluator, diagnostic, and container-removal admission. The final admission decision now precedes metric serialization; every uncleared row receives `coordination_metrics: null`.
2. **Daemon architecture admitted amd64 images.** `inspect_runtime` and inner qualification both require the inspected image itself to be exactly `linux/arm64`; a `linux/amd64` daemon is retained only as private provenance and is not an admission substitute.
3. **Row reruns reused evidence.** A run-level preflight rejects every scheduled existing `repo/arm/trial` directory before Docker inspection, corpus/receipt loading, prompts, artifacts, or runtime execution. The runner creates new rows atomically with `exist_ok=False`, and writers only update rows they own.
4. **Late diagnostic snapshots were ignored.** All six required snapshots are normalized and checked monotonically, including journal bytes; `before-remove` supplies the published final metrics.
5. **Published summaries carried raw private results.** `summary.json` now contains only sanitized report rows and locked aggregate metrics. Raw agents, artifacts, container/runtime evidence, qualification identity, diagnostics, and messages remain only in per-row `results.json`.

### RED

- `stateful sandbox run --fs build --network disabled --write-dir benchmark-admission-red-realworld --command 'python3 -m unittest discover -s crates/stateful-bench/scripts/tests -t crates/stateful-bench/scripts -p test_statefulbench_realworld.py -v'`
  - 133 tests; 6 expected regression failures.
- `stateful sandbox run --fs build --network disabled --write-dir benchmark-admission-red-docker --command 'python3 -m unittest discover -s crates/stateful-bench/scripts/tests -t crates/stateful-bench/scripts -p test_statefulbench_docker.py -v'`
  - 47 tests; 1 expected regression failure and 1 opt-in Docker E2E skip.
- `stateful sandbox run --fs build --network disabled --write-dir benchmark-admission-red-existing-row-main --command 'python3 -m unittest discover -s crates/stateful-bench/scripts/tests -t crates/stateful-bench/scripts -p test_statefulbench_realworld.py -v'`
  - 134 tests; the new main-path collision regression failed as expected while the intermediate explicit-row-ownership refactor also exposed 7 writer-contract errors.
- `stateful sandbox run --fs build --network disabled --write-dir benchmark-admission-red-review-gaps --command 'python3 crates/stateful-bench/scripts/tests/test_statefulbench_realworld.py QualificationTests.test_inner_qualification_accepts_arm64_image_on_non_arm_daemon RealWorldRunnerTests.test_late_phase_journal_bytes_must_not_decrease'`
  - 2 focused tests; 1 expected error and 1 expected failure.
- `stateful sandbox run --fs build --network disabled --write-dir benchmark-admission-red-journal-regression --command 'python3 crates/stateful-bench/scripts/tests/test_statefulbench_realworld.py RealWorldRunnerTests.test_late_phase_journal_bytes_must_not_decrease'`
  - 1 focused test; 1 expected failure after preserving a well-formed decreasing journal snapshot.

### GREEN

- `stateful sandbox run --fs build --network disabled --write-dir benchmark-admission-green-realworld-final2 --command 'python3 -m unittest discover -s crates/stateful-bench/scripts/tests -t crates/stateful-bench/scripts -p test_statefulbench_realworld.py -v'`
  - 136 tests passed.
- `stateful sandbox run --fs build --network disabled --write-dir benchmark-admission-green-docker-final2 --command 'python3 -m unittest discover -s crates/stateful-bench/scripts/tests -t crates/stateful-bench/scripts -p test_statefulbench_docker.py -v'`
  - 47 tests passed; 1 opt-in Docker E2E skipped.

Docker qualification and the immutable-image credit-free E2E are complete against the identity recorded above. No receipt, image identity, or result was inferred or fabricated.
