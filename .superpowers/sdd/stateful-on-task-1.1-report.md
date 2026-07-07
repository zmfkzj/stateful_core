# Task 1.1 Report

## Summary

Implemented Python DeNovo orchestration trace summary metrics that split value from friction:

- `true_collisions_prevented`
- `self_inflicted_denials`
- `scope_overlap_warnings`

## RED evidence

Command:

```sh
stateful sandbox run --fs build --network enabled --write-dir task11-red-uv2 --command 'env UV_CACHE_DIR=$TMPDIR/uv-cache uv run --with pytest python -m pytest crates/stateful-bench/scripts/tests/test_reports.py::test_summary_splits_friction_from_true_collision -q'
```

Output excerpt:

```text
F                                                                        [100%]
=================================== FAILURES ===================================
_______________ test_summary_splits_friction_from_true_collision _______________

>       assert summary["true_collisions_prevented"] == 1
               ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
E       KeyError: 'true_collisions_prevented'

crates/stateful-bench/scripts/tests/test_reports.py:41: KeyError
=========================== short test summary info ============================
FAILED crates/stateful-bench/scripts/tests/test_reports.py::test_summary_splits_friction_from_true_collision
1 failed, 2 warnings in 0.10s
```

This is the expected RED failure: the new metric key was missing before implementation.

## GREEN evidence

Focused command:

```sh
stateful sandbox run --fs build --network enabled --write-dir task11-green-focused2 --command 'env UV_CACHE_DIR=$TMPDIR/uv-cache uv run --with pytest python -m pytest crates/stateful-bench/scripts/tests/test_reports.py::test_summary_splits_friction_from_true_collision -q -p no:cacheprovider'
```

Output:

```text
.                                                                        [100%]
1 passed in 0.05s
```

Full file command:

```sh
stateful sandbox run --fs build --network enabled --write-dir task11-green-full2 --command 'env UV_CACHE_DIR=$TMPDIR/uv-cache uv run --with pytest python -m pytest crates/stateful-bench/scripts/tests/test_reports.py -q -p no:cacheprovider'
```

Output:

```text
............                                                             [100%]
12 passed in 0.08s
```

## Files changed

Committed files:

- `crates/stateful-bench/scripts/denovo_codex_agent.py`
- `crates/stateful-bench/scripts/tests/test_reports.py`

## Commit

- `5668a2d bench: split self-inflicted friction from true collisions in trace summary`

Commit contents checked with:

```sh
stateful sandbox run --fs git --network disabled --command 'git show --stat --name-only --oneline --no-ext-diff HEAD'
```

Output:

```text
5668a2d bench: split self-inflicted friction from true collisions in trace summary
crates/stateful-bench/scripts/denovo_codex_agent.py
crates/stateful-bench/scripts/tests/test_reports.py
```

## Self-review

- The test exercises exactly the requested three representative events in workspace `w1`.
- The implementation reuses existing `matching_orchestration_events` and `event_field` helpers.
- True collisions require `active_claim_conflict` plus a blocker from `wait.blocking_agent_id` or `blocking_agent_id`.
- Self-inflicted denials count only requested denial reason codes without a blocker.
- Scope overlap warnings count `kind == "scope_overlap"`.

## Concerns

- System `python` is not available in this sandbox and system `python3` does not have pytest installed, so verification used `uv run --with pytest python -m pytest ...` inside the required Stateful sandbox. The inner test invocation is still `python -m pytest`.

## Review fix: payload-nested wait

Fixed reviewer finding that `true_collisions_prevented` stayed at 0 for production-shaped `AuthorizationDenied` events where `reason_code` and `wait.blocking_agent_id` are under `payload`.

### RED evidence

Command:

```sh
stateful sandbox run --fs build --network enabled --write-dir task11-fix-red --command 'env UV_CACHE_DIR=$TMPDIR/uv-cache uv run --with pytest python -m pytest crates/stateful-bench/scripts/tests/test_reports.py::test_summary_splits_friction_from_true_collision -q -p no:cacheprovider'
```

Output excerpt:

```text
F                                                                        [100%]
>       assert summary["true_collisions_prevented"] == 1
E       assert 0 == 1

crates/stateful-bench/scripts/tests/test_reports.py:44: AssertionError
FAILED crates/stateful-bench/scripts/tests/test_reports.py::test_summary_splits_friction_from_true_collision
1 failed in 0.06s
```

### GREEN evidence

Focused command:

```sh
stateful sandbox run --fs build --network enabled --write-dir task11-fix-green-focused --command 'env UV_CACHE_DIR=$TMPDIR/uv-cache uv run --with pytest python -m pytest crates/stateful-bench/scripts/tests/test_reports.py::test_summary_splits_friction_from_true_collision -q -p no:cacheprovider'
```

Output:

```text
.                                                                        [100%]
1 passed in 0.05s
```

Full file command:

```sh
stateful sandbox run --fs build --network enabled --write-dir task11-fix-green-full --command 'env UV_CACHE_DIR=$TMPDIR/uv-cache uv run --with pytest python -m pytest crates/stateful-bench/scripts/tests/test_reports.py -q -p no:cacheprovider'
```

Output:

```text
............                                                             [100%]
12 passed in 0.08s
```

### Files changed

- `crates/stateful-bench/scripts/denovo_codex_agent.py`
- `crates/stateful-bench/scripts/tests/test_reports.py`
- `.superpowers/sdd/stateful-on-task-1.1-report.md`

### Commit

- `74bc756 bench: read payload-nested wait for collision metric`
