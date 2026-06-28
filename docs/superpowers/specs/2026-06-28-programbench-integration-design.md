# ProgramBench Stateful Integration Design

## Goal

Add ProgramBench support to `stateful-bench` as a real stateful/no-state benchmark, not an evaluation-only wrapper. The integration must run Codex or OMP agents on ProgramBench instances, produce ProgramBench-compatible submissions, evaluate them with official ProgramBench tooling, and compare quality plus efficiency across stateful and no-state conditions.

## Context

`stateful-bench` already supports SWE-bench paired-agent runs, synthetic coordination runs, and DeNovoSWE adapters. DeNovoSWE is the closest existing pattern: it runs benchmark-specific agents, records condition metadata, stores official evaluation results, and reports stateful/subagent axes with timing and token efficiency metrics.

ProgramBench is a reverse-engineering benchmark. An agent receives a cleanroom container with a compiled `./executable` and bundled documentation, then must write an original source tree and `compile.sh` that rebuilds an equivalent executable. ProgramBench evaluation expects this layout:

```text
<run-dir>/
  <instance_id>/
    submission.tar.gz
```

Official evaluation is done by `programbench eval <run-dir>`, and official summaries come from `programbench info` and submission packaging artifacts such as `_stats/score.json`. The ProgramBench Docker images are Linux `amd64`; local macOS use should generate commands and reports, but real scored runs require a compatible Docker host.

## Requirements

1. Run ProgramBench agent rollouts through `stateful-bench`, not only evaluate pre-existing submissions.
2. Support both Codex CLI and OMP CLI agent adapters.
3. Compare stateful and no-state conditions on the same instance set and runtime settings.
4. Include subagent-on/off as a first-class axis by default.
5. Preserve ProgramBench anti-cheat constraints: no internet/source lookup, no wrapping the provided binary, no decompilation, no `strace`/`ltrace` on the provided executable.
6. Use official ProgramBench evaluation/scoring artifacts. Do not fork or reinterpret official scoring.
7. Report efficiency alongside quality: wall time, token counts, uncached token counts, subagent usage, and quality-per-cost ratios.
8. Store enough metadata to reproduce a condition run: command lines, image tag, agent kind, model, timeout, filter/slice, and stateful integration mode.

## Non-Goals

- Reimplement ProgramBench official scoring.
- Claim benchmark results without official ProgramBench eval artifacts.
- Enable internet access during inference by default.
- Add leaderboard publishing automation in the first implementation.
- Treat an eval-only wrapper as sufficient ProgramBench support.

## CLI Design

Add a top-level `programbench` subcommand under `stateful-bench`:

```text
stateful-bench programbench run
stateful-bench programbench eval
stateful-bench programbench report
stateful-bench programbench compare
```

### `programbench run`

Runs agent inference for one or more conditions and writes ProgramBench-compatible submissions.

Important options:

- `--output-dir .stateful_bench/programbench/runs`
- `--run-id <id>`
- `--agent codex-cli|omp-cli`
- `--condition stateful:on,subagent:off` repeated; defaults to the full four-axis matrix
- `--model <model>`
- `--benchmark-max-turns <n>`
- `--timeout-seconds <n>`
- `--filter <regex>`
- `--slice <slice>`
- `--max-instances <n>`
- `--programbench-bin programbench`
- `--docker-bin docker`
- `--image-tag task_cleanroom_v6`
- `--stateful-binary <path>` for stateful-on nested integration

Output layout:

```text
<output-dir>/<run-id>/
  run.json
  conditions/
    stateful-off_subagent-off/
      condition.json
      <instance_id>/
        submission.tar.gz
        agent.stdout.log
        agent.stderr.log
        instance.json
    stateful-on_subagent-off/
      condition.json
      <instance_id>/
        submission.tar.gz
        agent.stdout.log
        agent.stderr.log
        instance.json
```

Each instance record stores status, timing, token usage, subagent usage, generated submission path, and error classification.

### `programbench eval`

Runs official ProgramBench evaluation per condition.

Behavior:

1. For each condition directory, call `programbench eval <condition-dir>` or an equivalent `uv run programbench eval <condition-dir>` when configured.
2. Save stdout/stderr logs.
3. Run `programbench info <condition-dir>` after eval and save its output.
4. Run `programbench submit package <condition-dir>` by default after successful eval to create `_stats/score.json`; allow `--no-package` for debug runs.

The command should fail clearly when Docker, ProgramBench, or Linux `amd64` prerequisites are missing.

### `programbench report`

Summarizes one condition from official eval artifacts and agent metadata.

Outputs JSON by default and Markdown when requested. Report fields:

- `condition_id`
- `instances`
- `attempted_instances`
- `evaluated_instances`
- `average_score`
- `resolved_count`
- `resolved_rate`
- `eval_error_count`
- `agent_error_count`
- `timeout_count`
- `running_time_ms`
- `average_running_time_ms`
- `token_observed_instances`
- `token_usage_turns`
- `token_input_tokens`
- `token_cached_input_tokens`
- `token_output_tokens`
- `token_reasoning_output_tokens`
- `token_input_plus_output_tokens`
- `token_uncached_input_tokens`
- `token_uncached_input_plus_output_tokens`
- `average_input_plus_output_tokens`
- `average_uncached_input_plus_output_tokens`
- `subagent_observed_instances`
- `subagent_used_count`
- `subagent_used_rate`
- `score_per_million_input_plus_output_tokens`
- `score_per_million_uncached_input_plus_output_tokens`
- `score_per_hour`

### `programbench compare`

Compares condition reports from the same run.

Comparison fields:

- stateful-on versus stateful-off score delta
- stateful-on versus stateful-off running-time delta
- stateful-on versus stateful-off token delta
- subagent-on versus subagent-off score delta
- subagent-on versus subagent-off efficiency delta
- quality-per-cost ratios per condition
- common-instance count used for comparison
- exclusions and reasons

## Condition Matrix

Default matrix:

```text
stateful:off,subagent:off
stateful:on,subagent:off
stateful:off,subagent:on
stateful:on,subagent:on
```

A reduced `stateful:off/on,subagent:on` matrix is allowed for targeted behavior runs, but reports must label it as reduced and must not present it as a full four-axis comparison.

All conditions in a comparison must use the same:

- instance set
- instance order
- ProgramBench image tag
- agent kind
- model
- temperature or equivalent decoding settings
- max turns
- timeout
- network policy
- evaluation command and ProgramBench version

## Agent Adapters

### Codex CLI

The Codex adapter follows existing `codex_pair_agent.py` and DeNovo Codex patterns:

- launch Codex with JSON event output when available;
- parse token usage from known event shapes;
- support context/token-limit resume only when safe;
- isolate Codex home/config per instance;
- inject stateful hooks/MCP/skills only for stateful-on conditions.

### OMP CLI

The OMP adapter follows the DeNovo OMP CLI pattern:

- use isolated OMP home/profile per instance;
- use the OMP `stateful` profile for stateful-on conditions;
- preserve ProgramBench offline inference constraints;
- parse OMP token usage from stdout when available;
- record lifecycle evidence for stateful-on runs.

### Prompt

ProgramBench prompts must be benchmark prompts, not stateful strategy hints. They include:

- reverse-engineering task statement;
- allowed behavior: run the executable normally and read bundled docs;
- forbidden behavior: internet/source lookup, package-registry source recovery, wrapping/reusing the binary, decompiling, `strace`/`ltrace` on the provided executable;
- required final artifact: executable `compile.sh` producing `./executable`;
- finish signal or adapter-controlled completion marker.

For subagent-on conditions, only the declared condition axis may require native subagent use. That injected requirement must be reported as part of the condition and not reused as generic benchmark advice.

## Stateful Application

For `stateful:on`:

1. Start or join a nested stateful runtime reachable by the agent process.
2. Enable the ProgramBench workspace inside the container or mounted workspace.
3. Install or expose agent-specific stateful integration:
   - Codex hooks and MCP config for Codex;
   - OMP stateful profile, generated tools, rules, and skills for OMP.
4. Record lifecycle evidence: session registration, heartbeat, activity finalization, and relevant claim/reservation events.

For `stateful:off`:

- Use the same task prompt, image, model, and runtime limits.
- Do not install hooks, MCP config, or stateful skills into the nested agent home.

## Evaluation and Scoring

Official ProgramBench tooling remains the source of truth.

Primary flow:

1. `programbench eval <condition-dir>` writes `<instance_id>/<instance_id>.eval.json`.
2. `programbench info <condition-dir>` prints official summary using active branches and ignored tests.
3. `programbench submit package <condition-dir>` writes `_stats/score.json`, which stores per-test pass/fail maps derived from official eval output.
4. `stateful-bench programbench report` reads official artifacts plus agent metadata to produce condition summaries.

If `_stats/score.json` is absent, the report command may read eval JSONs for a best-effort summary, but it must label the score source as `eval-json` and recommend running the package step for official score-derived machine-readable data.

## Efficiency Metrics

Efficiency is first-class in ProgramBench reports because stateful coordination can improve or harm cost even when final score is unchanged.

Per instance:

- wall-clock running time;
- timeout/error classification;
- token usage turns;
- input, cached input, output, reasoning output tokens;
- input+output tokens;
- uncached input tokens;
- uncached input+output tokens;
- subagent observed/used status.

Per condition:

- totals and averages for time and tokens;
- observed-instance counts for metrics that may be unavailable;
- score-per-token and score-per-hour ratios.

Comparison reports must show quality and efficiency deltas separately. No single composite score should hide a quality regression behind lower cost.

## Testing Plan

Use TDD for implementation.

Initial tests:

1. CLI parser accepts `programbench run/eval/report/compare` and default matrix.
2. Condition parser accepts `stateful:on,subagent:off` and rejects unknown keys.
3. Command builder creates expected Codex/OMP adapter commands without running Docker.
4. Report builder aggregates fixture ProgramBench eval artifacts and `_stats/score.json`.
5. Efficiency aggregation matches DeNovo-style token/time calculations.
6. Compare report computes score, time, and token deltas over common instances.
7. Reduced matrix is labeled as reduced in report metadata.

No Docker-dependent test is required for the first unit-test layer. Docker smoke tests can be documented as manual or ignored integration checks until CI has a compatible Linux `amd64` runner.

## Documentation Updates

Update:

- `README.md` benchmark tooling summary;
- `docs/usage-reference.md` benchmark command list;
- new `docs/programbench-benchmark-guide.md` with setup, constraints, default matrix, scoring, and interpretation rules;
- no separate command sheet in the first implementation; add one later only if reusable launch commands become too long for the guide.

## Risks

- ProgramBench containers are Linux `amd64`; macOS developers may only be able to build/report, not run scored evaluation locally.
- OMP/Codex token usage formats can change; parsers must treat missing token data as absent, not zero.
- Prompt additions can bias results. Keep strategy out of prompts except declared condition axes and benchmark integrity constraints.
- Official ProgramBench scoring may evolve. Reports should record ProgramBench version and prefer official artifacts.

## Implementation Decisions

- `programbench eval` runs `programbench submit package` by default after successful eval so `_stats/score.json` exists for machine-readable reports. A `--no-package` flag may skip it for debug runs.
- Use separate Codex and OMP adapter scripts for the first implementation. This matches DeNovo, keeps CLI-specific setup isolated, and avoids a shared abstraction before the duplicated shape is proven.
- Use host-driven Docker orchestration for the first implementation: `stateful-bench` starts the ProgramBench container, injects agent config, executes the selected agent inside it, and archives `/workspace` to `submission.tar.gz`. A prepared all-in-one image can be added later if host-driven setup becomes the bottleneck.
