# Final Closure Task 9 Report

## Status

DONE

## Scope

- `crates/stateful-bench/scripts/statefulbench_realworld.py`
- `crates/stateful-bench/scripts/statefulbench_lite.py`
- `crates/stateful-bench/scripts/tests/test_statefulbench_realworld.py`
- `crates/stateful-bench/scripts/tests/test_statefulbench_lite.py`

Both runners now default to `sequential,parallel-off`. `parallel-on` remains available only through an explicit `--arms` list. The explicit three-arm list is parsed and consumed in caller order; qualification/admission commands were unchanged.

## RED evidence

Command:

```sh
python3 -m unittest tests.test_statefulbench_lite.StatefulBenchLiteTests.test_run_arms_require_explicit_parallel_on tests.test_statefulbench_realworld.RealWorldRunnerTests.test_run_arms_require_explicit_parallel_on
```

Working directory: `crates/stateful-bench/scripts`

Result: failed 2 tests. Each runner parsed the previous default `['sequential', 'parallel-off', 'parallel-on']` where the test required `['sequential', 'parallel-off']`.

## GREEN evidence

Command:

```sh
python3 -m unittest tests.test_statefulbench_lite.StatefulBenchLiteTests.test_run_arms_require_explicit_parallel_on tests.test_statefulbench_realworld.RealWorldRunnerTests.test_run_arms_require_explicit_parallel_on
```

Working directory: `crates/stateful-bench/scripts`

Result: `Ran 2 tests in 0.030s` / `OK`.

The focused parser tests prove both runners agree on the two-arm default, retain explicit `--arms parallel-on`, and retain explicit `--arms sequential,parallel-off,parallel-on` in that exact order.

## Review

- `--arms` help and stateful-binary errors identify `parallel-on` as opt-in.
- The real-world runner still iterates `arguments.arms` in supplied order.
- No qualification/admission command, metrics, Docker logic, corpus data, or Rust code changed.

## Commit and push

Implementation commit: `fb2fb3f` (`Make parallel-on benchmark arm opt-in`)

Push: `presence-first-event-journal-v2` pushed successfully to `origin` (`91de7e8..fb2fb3f`).

## Concerns

None.
