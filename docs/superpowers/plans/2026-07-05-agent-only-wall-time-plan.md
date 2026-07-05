# Agent-only wall time implementation plan

## Summary

Implement the approved spec `docs/superpowers/specs/2026-07-05-agent-only-wall-time-design.md`: keep existing elapsed `running_time_ms` fields, add agent-only timing fields, make agent-only efficiency the primary reported efficiency axis, and leave historical artifacts backward-compatible.

## Constraints

- Do not remove, rename, or change the meaning of `running_time_ms`.
- Do not change benchmark prompts, scoring, evaluator behavior, smoke compile behavior, worker budgets, or condition axes.
- Do not infer agent-only timing from historical elapsed wall time.
- Do not add dependencies or persistent services.
- Use TDD for behavior changes: add failing focused tests first, then implementation.
- Use the smallest cutover: one new timing metric path per benchmark, no compatibility shims beyond serde defaults.

## Files to touch

- `crates/stateful-bench/src/denovo.rs`
- `crates/stateful-bench/src/programbench.rs`
- `crates/stateful-bench/scripts/denovo_codex_agent.py`
- `crates/stateful-bench/scripts/denovo_progress_report.py`
- `crates/stateful-bench/scripts/programbench_codex_agent.py`
- `crates/stateful-bench/scripts/programbench_omp_agent.py`
- `crates/stateful-bench/tests/denovo.rs`
- `crates/stateful-bench/tests/programbench.rs`
- `crates/stateful-bench/scripts/tests/test_denovo_codex_agent.py`
- `crates/stateful-bench/scripts/tests/test_reports.py`
- `crates/stateful-bench/scripts/tests/test_programbench_agents.py` (new, only if no focused existing ProgramBench adapter test can host the checks)
- Existing docs that already describe benchmark timing/report fields:
  - `docs/denovo-benchmark-guide.md`
  - `docs/denovo-benchmark-commands.md`
  - `docs/programbench-benchmark-guide.md`
  - `docs/usage-reference.md`

## Data contract

### DeNovo result rows

Add a top-level per-instance field emitted by `denovo_codex_agent.py`:

```json
"agent_running_time_ms": 123456
```

Source: the adapter's existing `codex_command["duration"]` seconds value, converted to integer milliseconds. This measures only the OMP/Codex subprocess around `run_omp_with_timeout(...)` / `run_codex_with_timeout(...)`, not evaluation.

Rust struct change in `DeNovoOfficialResult`:

```rust
#[serde(default, skip_serializing_if = "Option::is_none")]
pub agent_running_time_ms: Option<u64>,
```

Do not read `codex_command.duration` as a report fallback; old rows without the new field produce `None` agent-time metrics.

### DeNovo condition reports

Add optional aggregate fields to `DeNovoConditionReport`:

```rust
#[serde(default, skip_serializing_if = "Option::is_none")]
pub agent_running_time_ms: Option<u64>,
#[serde(default, skip_serializing_if = "Option::is_none")]
pub average_agent_running_time_ms: Option<f64>,
#[serde(default, skip_serializing_if = "Option::is_none")]
pub score_per_agent_hour: Option<f64>,
```

Rules in `build_denovo_condition_report(...)`:

- `agent_running_time_ms = Some(sum)` only when at least one result row has `agent_running_time_ms`.
- `average_agent_running_time_ms = Some(sum / observed_count)` over rows with observed agent time only.
- `score_per_agent_hour = score_per_hour(average_score, sum)` when `sum > 0`.
- Missing field on all rows leaves all agent-time metrics `None`.
- Existing `score_per_hour` continues using elapsed `running_time_ms`.

### DeNovo comparison reports

Add comparison-level fields:

```rust
#[serde(default, skip_serializing_if = "Option::is_none")]
pub total_agent_running_time_ms: Option<u64>,
#[serde(default, skip_serializing_if = "Option::is_none")]
pub stateful_agent_running_time_ms_delta_without_subagent: Option<i64>,
#[serde(default, skip_serializing_if = "Option::is_none")]
pub subagent_agent_running_time_ms_delta_without_stateful: Option<i64>,
#[serde(default, skip_serializing_if = "Option::is_none")]
pub combined_interaction_agent_running_time_ms_delta: Option<i64>,
```

Use the same axis pairing logic as score deltas. If either paired condition lacks `agent_running_time_ms`, the related delta is `None`. Keep `total_running_time_ms` unchanged.

### ProgramBench instance metadata

Add optional per-instance agent-only timing to `ProgramBenchInstanceMetadata`:

```rust
#[serde(default, skip_serializing_if = "Option::is_none")]
pub agent_running_time_ms: Option<u64>,
```

Python adapters populate it differently:

- `programbench_omp_agent.py`: measure only `subprocess.run(docker exec ... omp ...)` in `run_omp_command(...)` or immediately around its call. Exclude container copy, smoke compile, archive, cleanup, and teardown.
- `programbench_codex_agent.py`: measure only `run_agent_func(...)` inside `run_main(...)`. Exclude temporary airlock setup, workspace copy, smoke compile, archive, and cleanup.

### ProgramBench condition and comparison reports

Add the same aggregate metric shape as DeNovo to `ProgramBenchConditionReport`:

```rust
#[serde(default, skip_serializing_if = "Option::is_none")]
pub agent_running_time_ms: Option<u64>,
#[serde(default, skip_serializing_if = "Option::is_none")]
pub average_agent_running_time_ms: Option<f64>,
#[serde(default, skip_serializing_if = "Option::is_none")]
pub score_per_agent_hour: Option<f64>,
```

Add comparison totals/deltas to `ProgramBenchComparisonReport` using existing common-axis comparison logic.

## Report rendering

### Markdown tables

Make agent-time efficiency primary by placing agent-time columns before elapsed wall-time columns:

DeNovo condition table:

```text
| Condition | Stateful | Subagent | Instances | Success rate | Average score | Agent running time ms | Score per agent hour | Running time ms | Score per hour | ... |
```

ProgramBench condition table:

```text
| Condition | Stateful | Subagent | Instances | Partial score | Resolved | Agent running time ms | Score per agent hour | Running time ms | Score per hour | ... |
```

Comparison delta sections should label elapsed fields explicitly as elapsed wall time and agent fields as agent time. JSON keeps both.

### Progress script

Update `denovo_progress_report.py` to carry these optional report fields through JSON/markdown summaries:

- `agent_running_time_ms`
- `average_agent_running_time_ms`
- `score_per_agent_hour`

For live `results.jsonl` summaries, aggregate only explicit row-level `agent_running_time_ms`. If absent, leave derived fields absent/null; do not infer from `codex_command.duration`.

## Test plan

### 1. DeNovo Rust tests first

In `crates/stateful-bench/tests/denovo.rs`:

- Add a test where rows have `agent_running_time_ms: Some(...)` and assert:
  - report sum
  - report average over observed agent-time rows
  - `score_per_agent_hour` uses agent sum
  - existing `score_per_hour` still uses elapsed `running_time_ms`
- Add a missing-field test and assert agent-time fields are `None` while elapsed fields still serialize.
- Add comparison test for agent-time deltas on paired axes.
- Add markdown test that agent-time columns appear before elapsed `Running time ms`.

Expected initial state: tests fail because fields and renderer columns do not exist.

### 2. DeNovo implementation

- Add serde fields to `DeNovoOfficialResult`, `DeNovoConditionReport`, and `DeNovoComparisonReport`.
- Add one small helper if needed:

```rust
fn sum_observed_agent_running_time_ms(results: &[DeNovoOfficialResult]) -> Option<(u64, usize)> {
    let mut sum = 0_u64;
    let mut count = 0_usize;
    for value in results.iter().filter_map(|result| result.agent_running_time_ms) {
        sum = sum.saturating_add(value);
        count += 1;
    }
    (count > 0).then_some((sum, count))
}
```

Use existing `score_per_hour(...)` and existing average helpers where possible.

### 3. DeNovo Python tests and adapter

In `crates/stateful-bench/scripts/tests/test_denovo_codex_agent.py`:

- Extend the existing fake command/eval path test so each row contains top-level `agent_running_time_ms`.
- Assert the value is integer milliseconds derived from the command duration, not evaluator duration.

Implementation in `denovo_codex_agent.py` near row construction:

```python
agent_running_time_ms = int(round(command_record.get("duration", 0.0) * 1000))
...
"agent_running_time_ms": agent_running_time_ms,
```

No new helper unless the test needs the conversion in more than one place.

### 4. DeNovo progress report tests and implementation

In `crates/stateful-bench/scripts/tests/test_reports.py`:

- Add report JSON fixture with agent-time fields and assert JSON summary carries them.
- Add live `results.jsonl` fixture with row-level `agent_running_time_ms` and assert summed/averaged/score-per-agent-hour output.
- Add fixture with no row-level agent timing and assert fields are absent/null.
- Add markdown assertion for `agent_running_time_ms` / `score_per_agent_hour` columns.

Implementation in `denovo_progress_report.py`:

- Extend `empty_stats()` with agent-time accumulators.
- Extend `add_row()` to sum explicit row-level `agent_running_time_ms`.
- Extend `build_summary()` / `summarize_report()` with report-level optional values.
- Extend `render_markdown()` with agent-time columns.

### 5. ProgramBench Rust tests first

In `crates/stateful-bench/tests/programbench.rs`:

- Add instance metadata rows with `agent_running_time_ms: Some(...)`.
- Assert condition report sum/average/score-per-agent-hour.
- Assert elapsed score-per-hour remains based on `running_time_ms`.
- Assert comparison total and axis deltas use agent-time fields when both sides are present and `None` when missing.
- Assert markdown places agent-time columns before elapsed wall-time columns.

### 6. ProgramBench Rust implementation

- Add serde fields to `ProgramBenchInstanceMetadata`, `ProgramBenchConditionReport`, and `ProgramBenchComparisonReport`.
- Reuse the same small optional-sum pattern locally; avoid a shared abstraction unless Rust duplication becomes materially larger than two short helpers.
- Update `build_programbench_condition_report(...)`, `build_programbench_comparison_report(...)`, and `render_programbench_comparison_markdown(...)`.

### 7. ProgramBench Python tests and adapters

Create `crates/stateful-bench/scripts/tests/test_programbench_agents.py` if no focused existing adapter test can host these checks. Keep it small: unit-test the metadata-writing path with fake subprocess/runner functions, not Docker.

Implementation details:

- `programbench_codex_agent.py`: start `agent_started = now_ms()` immediately before `run_agent_func(...)`; set `agent_finished = now_ms()` immediately after it returns/raises if metadata is still written; write `agent_running_time_ms = max(agent_finished - agent_started, 0)`.
- `programbench_omp_agent.py`: do the same around the OMP subprocess only.
- Preserve existing `started_at_ms`, `finished_at_ms`, and `running_time_ms` semantics.

If an adapter error occurs before the agent subprocess starts, omit `agent_running_time_ms` instead of writing `0`.

### 8. Documentation

Update existing docs only:

- Explain `running_time_ms` = elapsed harness/adapter wall time.
- Explain `agent_running_time_ms` = measured agent subprocess time.
- Explain `score_per_agent_hour` is the primary time-efficiency metric when available.
- State old artifacts may omit agent-time fields.

No new docs beyond this plan/spec.

## Verification commands

Run after the related implementation steps, not before fields exist:

```bash
cargo test -p stateful-bench --test denovo
cargo test -p stateful-bench --test programbench
python3 -m pytest crates/stateful-bench/scripts/tests/test_denovo_codex_agent.py crates/stateful-bench/scripts/tests/test_reports.py crates/stateful-bench/scripts/tests/test_programbench_agents.py
```

If ProgramBench adapter Python tests are added, include their exact test file in the final pytest command.

## Execution options after plan approval

1. Subagent-driven implementation: split DeNovo Rust, DeNovo Python/progress, ProgramBench Rust, ProgramBench Python, docs into independent workers; main session integrates and runs final targeted verification.
2. Inline implementation: execute the same tasks in this session with TDD checkpoints.

Recommended: subagent-driven implementation because the DeNovo and ProgramBench paths are independent after this contract.
