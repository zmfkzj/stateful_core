# DeNovoSWE Official Integration Design

## Context

`stateful-bench` currently supports SWE-bench Verified pair workflows:
fetching rows, preparing pair manifests, running paired agents, reporting, and
comparing stateful versus no-state runs. DeNovoSWE is a different benchmark
shape: each instance asks an agent to reconstruct a package from a natural
language specification after the original source has been removed from a Docker
workspace.

The integration should follow the official DeNovoSWE manual as closely as
possible. The official execution surface lives in AweAgent:

- Dataset: https://huggingface.co/datasets/AweAI-Team/DeNovoSWE
- Recipe guide: https://github.com/AweAI-Team/AweAgent/blob/main/recipes/denovo_swe/README.md
- Task guide: https://github.com/AweAI-Team/AweAgent/blob/main/aweagent/tasks/denovo_swe/README.md

The benchmark axes requested for local comparisons are:

- `stateful`: `on` or `off`
- `subagent`: `on` or `off`
- `running_time_ms`: measured wall time for each official recipe invocation

## Goals

Add DeNovoSWE support without reimplementing the official evaluator. The new
workflow should let users run the official AweAgent DeNovoSWE preprocessing,
agent execution, evaluation, and analysis from `stateful-bench`, while producing
stateful-bench-native artifacts that can compare the four requested conditions.

The four canonical conditions are:

1. `stateful=off`, `subagent=off`
2. `stateful=on`, `subagent=off`
3. `stateful=off`, `subagent=on`
4. `stateful=on`, `subagent=on`

Each condition must use the same prepared DeNovoSWE data file, instance
selection, model, prompt version, evaluation iterations, and concurrency unless
the user explicitly overrides them.

## Non-Goals

This integration will not port the DeNovoSWE evaluator into Rust, rewrite the
AweAgent task, or change the official clean/eval workflow. It also will not
invent a subagent implementation inside AweAgent. Instead, the subagent axis is
expressed by selecting an official-compatible AweAgent config or environment
that the user provides for the subagent condition.

## Recommended Architecture

Add a DeNovoSWE namespace under `stateful-bench`:

```text
stateful-bench denovo extract
stateful-bench denovo run
stateful-bench denovo report
stateful-bench denovo compare
```

`stateful-bench` remains the orchestrator and artifact normalizer. AweAgent
remains the benchmark implementation. The wrapper invokes these official files
from a user-supplied AweAgent checkout:

```text
<aweagent-root>/recipes/denovo_swe/extract_patch.py
<aweagent-root>/recipes/denovo_swe/run.py
<aweagent-root>/recipes/denovo_swe/analyze_results.py
```

The wrapper requires `--aweagent-root` or `AWEAGENT_ROOT`. It records the
AweAgent git commit when the path is a git checkout, the exact command line,
environment overrides, start/end timestamps, and elapsed wall time.

## Command Design

### `denovo extract`

Runs the official preprocessing step:

```text
python recipes/denovo_swe/extract_patch.py
```

Inputs:

- `--input`: raw DeNovoSWE JSONL
- `--output`: official extraction output directory
- `--config`: AweAgent task config, defaulting to
  `configs/tasks/denovoswe.yaml`
- official passthrough flags: `--max-concurrent`, `--dry-run`,
  `--instance-ids`, `--del-done-images`, `--no-extract-package-info`

Output:

- official `extract_patch_*/results.jsonl`
- stateful-bench `denovo-extract.json` metadata beside the official output

### `denovo run`

Runs the official agent plus evaluation recipe:

```text
python recipes/denovo_swe/run.py
```

Inputs:

- `--data-file`: prepared JSONL from `denovo extract`
- `--output-dir`: stateful-bench run root
- `--run-id`: stable run id
- `--mode`: `batch`, `debug`, `prompt`, or `dry-run`
- official passthrough flags: `--instance-id`, `--instance-ids`,
  `--llm-config`, `--model`, `--max-steps`, `--max-concurrent`,
  `--enable-search`, `--no-search`, `--skip-eval`, `--validate-run`,
  `--eval-iters`, `--del-done-images`, `--dump-clean-snapshot`,
  `--prompt-version`, `--verbose`

Condition inputs:

- `--condition`: may be repeated as `stateful:on,subagent:off,config:path`
- `--stateful-off-config`
- `--stateful-on-config`
- `--subagent-off-config`
- `--subagent-on-config`
- `--condition-env KEY=VALUE`: may be repeated and scoped per condition in a
  later implementation task

The first implementation should support the four canonical conditions by
building the effective config path as follows:

- If `--condition` entries are supplied, use them exactly.
- Otherwise use the four canonical conditions.
- For each condition, prefer a fully specified condition config.
- If no condition config is supplied, use the base `--config` and record the
  axis value as metadata without claiming the underlying agent behavior changed.

This makes the official recipe the source of execution truth while still
allowing users to provide stateful-enabled or subagent-enabled AweAgent configs.

### `denovo report`

Reads one `denovo run` output directory and normalizes official result rows.

The normalized report includes:

- run id
- AweAgent root and commit
- official config path
- condition axis values
- total instances
- completed instances
- success count and success rate
- average score
- average pass rate from `eval_result.details.pass_rate`
- correct rate where `score == 1.0`
- almost-correct rate where `score >= 0.8`
- error count
- total and average `running_time_ms`

### `denovo compare`

Reads a set of condition reports and emits a comparison matrix keyed by
`stateful` and `subagent`.

The comparison emphasizes:

- score and success deltas for `stateful=on` versus `stateful=off`
- score and success deltas for `subagent=on` versus `subagent=off`
- interaction effect when both are enabled
- total and average running time per condition

## Data Model

Add focused DeNovoSWE types alongside the existing SWE-bench pair types:

- `DeNovoCondition`: `stateful`, `subagent`, `config_path`, env overrides
- `DeNovoRunMetadata`: run id, condition, official command, AweAgent root,
  AweAgent commit, timestamps, `running_time_ms`
- `DeNovoOfficialResult`: permissive deserializer for official `results.jsonl`
- `DeNovoConditionReport`: normalized aggregate for one condition
- `DeNovoComparisonReport`: four-condition comparison matrix

The official result deserializer should be tolerant of extra fields so that
AweAgent can evolve without breaking old stateful-bench reports.

## Artifact Layout

Use a layout that preserves official output without moving or rewriting it:

```text
.stateful_bench/denovo/
  extracts/
    <extract-id>/
      official/
        extract_patch_.../
          results.jsonl
          status.jsonl
          run_config.json
          extract.log
      denovo-extract.json
  runs/
    <run-id>/
      run.json
      conditions/
        stateful-off_subagent-off/
          condition.json
          official/
            results.jsonl
            trajectories.jsonl
            run_config.json
          denovo-report.json
        stateful-on_subagent-off/
          ...
        stateful-off_subagent-on/
          ...
        stateful-on_subagent-on/
          ...
      comparison.json
```

When official output is timestamped by AweAgent, `stateful-bench` records the
resolved official directory in `condition.json` instead of renaming it.

## Error Handling

Preflight failures should stop before running expensive jobs:

- missing `AWEAGENT_ROOT` or invalid `--aweagent-root`
- missing official recipe files
- missing Python executable
- missing input JSONL
- invalid condition definitions

Per-condition execution failures should be recorded as condition errors and
should not erase official artifacts. `denovo compare` should include failed
conditions with `error_count` and no score, so partial benchmark runs remain
inspectable.

## Testing Strategy

Use TDD for implementation.

Unit tests:

- parse DeNovoSWE rows with required official fields
- parse official `results.jsonl` rows with extra fields
- expand default four-condition matrix
- reject malformed condition definitions
- compute condition report aggregates
- compute comparison deltas including running time

CLI tests:

- parse `denovo extract`, `denovo run`, `denovo report`, and `denovo compare`
- verify official command construction without launching Docker

Fixture-based integration tests:

- create fake AweAgent recipe scripts under a temp directory
- emit official-shaped `results.jsonl`
- verify stateful-bench artifact layout, metadata, and reports

Real Docker/AweAgent runs:

- remain opt-in because they require Docker, an LLM backend, API credentials,
  and DeNovoSWE images
- should be documented as manual validation commands, not required for default
  workspace tests

## Documentation Updates

Update `README.md` after implementation to describe:

- required AweAgent checkout and install
- DeNovoSWE extract/run/report/compare commands
- how to provide official-compatible configs for `stateful` and `subagent`
  conditions
- how `running_time_ms` is measured
- why official evaluator semantics are delegated to AweAgent

## Open Decision Resolved

The integration will follow the official manual by invoking AweAgent recipes
instead of reimplementing DeNovoSWE. Stateful and subagent axes are represented
as reproducible stateful-bench condition metadata plus user-provided
official-compatible AweAgent configs or environment overrides.
