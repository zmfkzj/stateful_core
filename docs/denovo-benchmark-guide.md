# DeNovoSWE Benchmark Guide

Last updated: 2026-06-22.

This guide records the protocol we use when running DeNovoSWE through
`stateful-bench`. It follows the official AweAgent DeNovoSWE recipe/task
documentation and the DeNovoSWE paper, then adds the local rules needed for
stateful/no-state comparisons.

Official sources:

- AweAgent recipe guide:
  https://github.com/AweAI-Team/AweAgent/blob/main/recipes/denovo_swe/README.md
- AweAgent task guide:
  https://github.com/AweAI-Team/AweAgent/blob/main/aweagent/tasks/denovo_swe/README.md
- DeNovoSWE paper:
  https://arxiv.org/html/2606.10728v1

## What DeNovoSWE Measures

DeNovoSWE is a from-scratch repository construction benchmark. The Docker image
starts with the original source present, but the runtime runs `clean.sh` before
the agent can inspect it. The agent receives the package specification through
`README.md` and must rebuild the package implementation from that spec.

Evaluation runs in a fresh container session:

1. Reset to the parent commit and rerun `clean.sh`.
2. Reinject the benchmark `README.md`.
3. Apply the agent patch.
4. Delete agent-written tests and verification scripts.
5. Apply the golden `test_patch` and binary fixtures.
6. Reinstall the generated package with `pip install -e .`.
7. Run pytest per test file.

The primary score is the unit-test pass ratio. A score between `0` and `1` is
normal for this benchmark because whole-repository generation can fail
partially across files, APIs, dependency behavior, and implementation details.

## Official Reliability Rule

Do not treat a single agent rollout as a publishable result.

The DeNovoSWE paper reports evaluation metrics averaged across three
independent execution trials to improve statistical stability and reduce
experimental variance. Our local benchmark summaries should follow the same
rule for any stateful/no-state claim:

- Run at least three independent trials per condition.
- Keep the same instance set, shard boundaries, model, prompt version,
  temperature, context window, max turns, evaluator settings, and runtime
  limits across compared conditions.
- Report mean score and success rate across the three trials.
- Include per-trial values or standard deviation when comparing small samples.
- Label smoke tests, debug runs, interrupted runs, or one-off runs as
  non-comparable.

`--eval-iters N` repeats the evaluator and averages evaluation output. It does
not replace independent agent trials, because it does not rerun the agent
policy from scratch.

## Official-Style Defaults

Use these defaults unless a run is explicitly labeled as a compatibility or
debug run:

- `--mode batch` for scored runs.
- `--prompt-version v2`, the official recipe default with the finish gate.
- `--benchmark-temperature 1`.
- `--benchmark-model-context-window 256000`, matching the paper's 262,144 token
  evaluation context as closely as this adapter supports.
- `--benchmark-max-turns 500` for DeNovoSWE package reconstruction.
- `--eval-iters 1` for agent comparisons, unless specifically testing
  evaluator stability.
- `--max-concurrent 1` when comparing stateful versus no-state behavior unless
  the experiment is explicitly about throughput.
- `--agent omp-cli` and `--benchmark-model deepseek-v4-flash` for OMP-backed
  runs, unless the experiment explicitly compares agent CLIs or models.
- Isolated OMP homes must have the benchmark model's API key seeded, or the
  equivalent provider API key must be present in the launch environment.
- For Docker-isolated OMP agent runs, build or tag the image from
  `crates/stateful-bench/docker/denovo-omp-agent.Dockerfile`; it includes
  Bun-installed `omp` plus the Linux `stateful` binary. Add
  `--agent-docker-image <image>`. `--agent-docker-stateful-binary <path>` only
  needs to be set when the image's `stateful` binary is not at
  `/usr/local/bin/stateful`.

Historical runs may use `--prompt-version v1`; do not mix v1 and v2 results in
the same comparison table.

## Prompt Policy

Do not add ad hoc prompt instructions for normal scored, patch-quality, or
stateful/no-state comparison runs. Keep the agent prompt limited to the
benchmark task, official prompt version behavior, and declared condition axes.
Extra strategy hints, orchestration instructions, lifecycle reminders, role
assignments, or implementation guidance can bias the rollout and make the result
non-comparable.

Stateful lifecycle enforcement belongs in hooks, extensions, MCP/tool policy,
installed skills, and runtime configuration rather than in benchmark task
prompt text.

If a run intentionally tests lifecycle, concurrency, or other agent-behavior
mechanics with injected instructions, label it as a debug or behavior test,
record the exact prompt addition, and do not use the result as a scored
stateful/no-state comparison.

## Local Comparison Design

For stateful/no-state comparisons, prefer a paired matrix:

```text
stateful:off,subagent:off
stateful:on,subagent:off
stateful:off,subagent:on
stateful:on,subagent:on
```

When the experiment is specifically about subagent behavior, a reduced paired
matrix is acceptable:

```text
stateful:off,subagent:on
stateful:on,subagent:on
```

Run both conditions over the same shard and the same instance order. Interpret
differences only after three independent trials. For partial or interrupted
runs, compare only the common completed instance set and clearly mark the result
as exploratory.

OMP retains the `subagent` axis for matrix compatibility, but it does not use
Codex native subagent enforcement or Codex subagent usage counters.

## Docker OMP Stateful Lifecycle

Docker OMP runs execute the agent in a dedicated container instead of using the
host OMP binary. For `stateful:on`, the adapter uses `/home/stateful` as the
container runtime home so the isolated OMP `stateful` profile and extension path
are visible in the container. It rewrites the mounted `$STATEFUL_HOME/config.yml`
repo registry and `repos/*.json` metadata from host workspace paths to the
container workspace path `/workspace`.

Use lifecycle events to distinguish a valid stateful-on Docker run from an OMP
process that merely completed. A successful stateful-on Docker smoke run should
show `SessionRegistered`, repeated `SessionHeartbeat`, and `ActivityFinalized`
events for the nested OMP session. The verified run
`r110-denovo-one-omp-docker-stateful-onoff-subagent-on` completed the
stateful-off/stateful-on subagent-on pair with that event sequence. Treat missing
registration, no heartbeat, or missing finalization as lifecycle evidence
failure, not as a model-quality result.

## Reporting Rules

Use `denovo-report.json` as the primary progress source. During matrix runs,
adapter `results.jsonl` files can be incremental or reset as conditions advance;
they are useful for per-instance inspection but should not be the sole source
for a live progress total.

Recommended reporting order:

1. `denovo-report.json` for condition-level totals.
2. `comparison.json` for completed matrix comparisons.
3. Per-instance `eval-result.json` files for failure analysis.
4. `results.jsonl` only as a fallback or for raw row inspection.

Every report should state:

- run IDs and trial IDs
- dataset or shard files
- condition axes
- prompt version
- model, temperature, context window, max turns
- number of independent trials
- whether the run completed or was interrupted
- whether reported numbers come from `denovo-report.json`,
  `comparison.json`, or raw `results.jsonl`

## Failure Analysis

When one condition underperforms another, inspect per-instance
`eval-result.json` first:

- Compare `score`, `accepted`, `details.pass_rate`, `passed`, `failed`, and
  `errors` for matched instances.
- Group failures by import/setup errors, missing APIs, dependency behavior,
  test collection failures, and partial behavioral mismatches.
- Do not attribute a condition effect to one failed rollout unless the same
  pattern repeats across independent trials.

Runtime failures such as missing Docker images, usage limits, `omp exited`, or
agent CLI transport errors should be reported separately from model quality metrics.
