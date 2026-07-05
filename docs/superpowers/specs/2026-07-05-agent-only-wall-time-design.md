# Agent-only wall time evaluation axis

## Goal

Promote agent-only wall time to the primary efficiency axis across DeNovoSWE and ProgramBench reports while preserving existing elapsed wall-time fields for compatibility and auditability.

## Current behavior

DeNovo and ProgramBench currently expose `running_time_ms` and `score_per_hour` as the main time-efficiency fields, but those fields do not consistently mean agent-only work.

DeNovo seams:

- `crates/stateful-bench/scripts/denovo_codex_agent.py` already measures the agent subprocess only in `codex-command.json` as `duration` seconds. The timer starts immediately before `run_omp_with_timeout` or `run_codex_with_timeout` and stops when that subprocess returns.
- The same adapter runs evaluation after the agent subprocess returns and writes `eval-result.json` with a separate evaluator `duration`.
- `crates/stateful-bench/src/denovo.rs` measures condition `running_time_ms` around the whole adapter command, so DeNovo report `running_time_ms` includes agent execution, evaluator execution, setup, trace capture, patch harvesting, and report overhead.
- `DeNovoConditionReport`, `DeNovoComparisonReport`, markdown rendering, and `crates/stateful-bench/scripts/denovo_progress_report.py` currently have no first-class agent-only timing fields.

ProgramBench seams:

- `crates/stateful-bench/src/programbench.rs` reports `ProgramBenchInstanceMetadata.running_time_ms` and condition `running_time_ms` from the Rust runner/wrapper elapsed time.
- `crates/stateful-bench/scripts/programbench_codex_agent.py::run_main` starts its timer before airlock setup and stops after agent run, smoke compile, archive, and error normalization.
- `crates/stateful-bench/scripts/programbench_codex_agent.py::run_agent` runs the Codex subprocess and then smoke-compiles in a `finally` block.
- `crates/stateful-bench/scripts/programbench_omp_agent.py` has the same shape for host OMP. Docker OMP additionally copies the workspace into/out of the agent container and smoke-compiles in `finally` paths around the subprocess.
- ProgramBench reports and comparisons currently use only elapsed `running_time_ms` for time deltas and `score_per_hour`.

## Requirements

- Add agent-only wall-time fields without changing the meaning of existing `running_time_ms`.
- Treat agent-only wall time as the primary efficiency axis in JSON reports, comparison JSON, markdown tables, progress output, docs, and final benchmark reporting guidance.
- Keep existing `score_per_hour` and elapsed `running_time_ms` as secondary compatibility fields.
- Add `score_per_agent_hour` beside `score_per_hour`.
- For historical artifacts that lack agent-only fields, deserialize safely and report agent-time-derived metrics as zero or `null`; do not silently reinterpret elapsed wall time as agent-only time.
- Keep quality metrics separate from efficiency metrics.
- Do not alter model prompts, benchmark task instructions, scoring, eval logic, worker budget, or condition axes.
- Do not add new dependencies.
- Tests must be written first and must cover DeNovo, ProgramBench, markdown/progress output, and historical-artifact compatibility.

## Design

Use additive schema fields. This avoids breaking historical artifacts and external scripts that already consume `running_time_ms`, while making agent-only wall time explicit and prominent.

### Field semantics

Use these definitions everywhere:

```text
running_time_ms = elapsed benchmark wrapper/condition time, including harness overhead and any eval/smoke/archive/copy steps already included today.
agent_running_time_ms = only the benchmark agent subprocess wall time, from process start to process exit.
average_running_time_ms = running_time_ms / attempted or total instances.
average_agent_running_time_ms = agent_running_time_ms / attempted or total instances.
score_per_hour = score over elapsed running_time_ms, retained as compatibility/secondary efficiency.
score_per_agent_hour = score over agent_running_time_ms, primary efficiency metric.
```

For condition-level reports, `agent_running_time_ms` is the sum of per-instance agent subprocess durations. This is work-time, not condition makespan under concurrency. Existing `running_time_ms` remains the elapsed condition wall clock/makespan.

### DeNovo data model

Extend `DeNovoOfficialResult` with:

```rust
#[serde(default, skip_serializing_if = "Option::is_none")]
pub agent_running_time_ms: Option<u64>,
```

The DeNovo adapter already has the exact source: `codex-command.json.duration` seconds. Convert with millisecond rounding that never underflows:

```python
agent_running_time_ms = max(0, round(duration * 1000))
```

Every `InstanceResult` returned after agent execution should carry this value, including `stop`, `benchmark-contamination`, `codex-error`/`omp-error`, and timeout/error rows when a duration exists. Pure setup errors before the agent subprocess starts leave it absent.

Extend `DeNovoConditionReport` with:

```rust
#[serde(default)]
pub agent_running_time_ms: u64,
#[serde(default, skip_serializing_if = "Option::is_none")]
pub average_agent_running_time_ms: Option<f64>,
#[serde(default, skip_serializing_if = "Option::is_none")]
pub score_per_agent_hour: Option<f64>,
```

`build_denovo_condition_report` sums only observed `agent_running_time_ms`. If the sum is zero, `score_per_agent_hour` is `None`.

Extend `DeNovoComparisonReport` with agent-time totals and deltas mirroring elapsed-time naming:

```rust
pub stateful_agent_running_time_ms_delta_without_subagent: Option<i64>,
pub subagent_agent_running_time_ms_delta_without_stateful: Option<i64>,
pub total_agent_running_time_ms: u64,
```

The reduced two-condition `subagent:on` matrix may not have all four axes; missing deltas stay `None` like existing score deltas.

### ProgramBench data model

Extend `ProgramBenchInstanceMetadata` and `ProgramBenchInstanceReport` with:

```rust
#[serde(default, skip_serializing_if = "is_zero")]
pub agent_running_time_ms: u64,
```

Extend `ProgramBenchConditionReport` with:

```rust
#[serde(default)]
pub agent_running_time_ms: u64,
#[serde(default, skip_serializing_if = "Option::is_none")]
pub average_agent_running_time_ms: Option<f64>,
#[serde(default, skip_serializing_if = "Option::is_none")]
pub score_per_agent_hour: Option<f64>,
```

Extend `ProgramBenchComparisonReport` with:

```rust
pub stateful_agent_running_time_ms_delta_without_subagent: Option<i64>,
pub subagent_agent_running_time_ms_delta_without_stateful: Option<i64>,
pub total_agent_running_time_ms: u64,
```

ProgramBench adapter metadata should write `agent_running_time_ms` from subprocess-only measurement:

- Codex host: wrap only `subprocess.run(command, ...)` in `programbench_codex_agent.py::run_agent`.
- OMP host: wrap only `run_omp_command(...)` in `programbench_omp_agent.py::run_agent`.
- OMP Docker: wrap only `subprocess.run(docker exec ... omp ..., ...)` in `programbench_omp_agent.py::run_agent_in_docker`.

The measurement excludes airlock copy, stateful installation, workspace copy-back, smoke compile, archiving, target container startup/removal, and report writing.

### Rendering and progress

Markdown table ordering should make agent-only efficiency primary:

1. quality metric (`Average score`, `Resolved`, or success rate);
2. `Agent running time ms`;
3. `Score per agent hour`;
4. elapsed `Running time ms`;
5. elapsed `Score per hour`;
6. token metrics.

`denovo_progress_report.py` should include agent-time fields in both JSON and markdown summaries. It should prefer `denovo-report.json` agent fields when present and fall back to row-level `agent_running_time_ms` when only incremental `results.jsonl` is available.

ProgramBench markdown rendering should apply the same ordering. ProgramBench does not need a new standalone progress script unless an existing ProgramBench live-report path already renders time fields.

### Documentation

Update existing documentation only:

- `docs/denovo-benchmark-guide.md`: define `agent_running_time_ms` as primary efficiency; label `running_time_ms` elapsed/eval-inclusive.
- `docs/denovo-benchmark-commands.md`: update reporting guidance and live-progress interpretation.
- `docs/programbench-benchmark-guide.md`: define ProgramBench agent-only timing and distinguish smoke/eval/archive overhead.
- `docs/usage-reference.md`: update benchmark command descriptions from generic `running_time_ms` axis to agent-time plus elapsed-time axes.

## Data flow

```text
DeNovo agent subprocess
  -> duration seconds in codex-command.json
  -> agent_running_time_ms in results.jsonl row
  -> sum in denovo-report.json
  -> comparison totals/deltas
  -> markdown/progress primary efficiency columns

ProgramBench agent subprocess
  -> agent_running_time_ms in instance.json
  -> sum in condition report
  -> comparison totals/deltas
  -> markdown primary efficiency columns

Existing elapsed timers
  -> running_time_ms remains unchanged
  -> score_per_hour remains compatibility/secondary metric
```

## Error handling and compatibility

- Missing `agent_running_time_ms` in historical DeNovo rows or ProgramBench metadata deserializes to `None`/`0`.
- `score_per_agent_hour` is `None` when agent time is absent or zero.
- Runtime/setup failures before an agent subprocess starts do not fake a duration.
- Runtime failures after the subprocess starts keep the measured agent duration, because failed agent attempts still consumed agent wall time.
- Timeout rows keep measured duration when the adapter can record it; otherwise they leave agent time absent.
- Existing consumers that only read `running_time_ms` keep working.

## Testing plan

Write failing tests first.

Rust tests:

- `crates/stateful-bench/tests/denovo.rs`: `build_denovo_condition_report` sums row `agent_running_time_ms`, computes `average_agent_running_time_ms`, and computes `score_per_agent_hour` while preserving existing elapsed `score_per_hour`.
- `crates/stateful-bench/tests/denovo.rs`: `compare_denovo_reports` emits `total_agent_running_time_ms` and agent-time deltas.
- `crates/stateful-bench/tests/denovo.rs`: historical result JSON without `agent_running_time_ms` still deserializes and leaves agent-time metrics absent/zero.
- `crates/stateful-bench/tests/programbench.rs`: `build_programbench_condition_report` sums instance `agent_running_time_ms`, computes `average_agent_running_time_ms`, and computes `score_per_agent_hour` while preserving elapsed fields.
- `crates/stateful-bench/tests/programbench.rs`: `compare_programbench_reports` emits agent-time totals and deltas.
- Markdown render tests or existing render assertions should verify agent-time columns precede elapsed-time columns.

Python tests:

- `crates/stateful-bench/scripts/tests/test_denovo_codex_agent.py`: completed DeNovo agent result rows include `agent_running_time_ms` derived from command duration; setup errors before agent execution omit it.
- `crates/stateful-bench/scripts/tests/test_reports.py`: progress summaries aggregate agent time from report fields and from raw incremental results rows.
- `crates/stateful-bench/scripts/tests/test_reports.py`: markdown progress output shows agent time before elapsed time where both exist.
- ProgramBench adapter tests should verify Codex and OMP wrappers record subprocess-only `agent_running_time_ms` and do not include smoke compile/archive/copy-back time.

Targeted commands:

```text
cargo test -p stateful-bench --test denovo
cargo test -p stateful-bench --test programbench
python3 -m pytest crates/stateful-bench/scripts/tests/test_denovo_codex_agent.py crates/stateful-bench/scripts/tests/test_reports.py
```

Add narrower test selectors during TDD red/green cycles before running these grouped commands.

## Non-goals

- Do not remove or rename `running_time_ms`.
- Do not change benchmark prompts.
- Do not change scoring, evaluator behavior, smoke compile behavior, worker budgets, or condition axes.
- Do not infer agent-only time from elapsed wall time for historical artifacts.
- Do not add a new database, cache, dependency, or post-processing service.

## Rollout

1. Add schema fields and report math for DeNovo.
2. Add DeNovo adapter row emission.
3. Add DeNovo progress and markdown rendering updates.
4. Add ProgramBench schema fields and report math.
5. Add ProgramBench adapter subprocess-only timing.
6. Update ProgramBench markdown rendering.
7. Update docs.
8. Run targeted Rust and Python verification.
