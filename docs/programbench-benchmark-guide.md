# ProgramBench Benchmark Guide

ProgramBench is a reverse-engineering benchmark: an agent receives a cleanroom
container with a compiled `./executable` and bundled documentation, then writes
an original source tree and `compile.sh` that rebuilds an equivalent executable.

## Runtime Requirements

ProgramBench Docker images target Linux `amd64`. macOS developers can inspect
commands and reports, but scored inference/evaluation needs a compatible Docker
host.

`stateful-bench` keeps each target ProgramBench Docker container alive until the
run finishes, then removes it explicitly. For host Codex/OMP runs, the adapters
copy the container's `/workspace` into an empty temporary host airlock, run the
host CLI there, and sync that airlock back into the live target container only
for the smoke `./compile.sh`. The host airlock is then archived/reported as the
ProgramBench submission. Adapter runtime state uses `STATEFUL_HOME` under the
airlock's `.stateful` directory.

With `programbench run --agent omp-cli --agent-docker-image <image>`, OMP runs
inside a separate agent container instead of on the host. The image must include
`omp` and `stateful`; defaults are `--agent-docker-omp-bin omp`,
`--agent-docker-stateful-binary /usr/local/bin/stateful`,
`--agent-docker-home /home/stateful`, and `--agent-docker-sandbox off`.
The Docker container is the sandbox boundary, so the adapter sets
`STATEFUL_OMP_SANDBOX=off` for the in-container `docker exec` by default. When
Stateful is enabled, the agent container inherits the parent Stateful runtime
environment so it uses the same server/token. The adapter copies the target
workspace into `/workspace`, runs Stateful install/enable and OMP, copies
`/workspace` back to the host airlock, then uses the existing target container
smoke compile and archive flow. Copy-back failures are reported as
`workspace_copy_error`; `smoke_compile_error` is reserved for target-container
compile failures, and primary OMP exit errors stay primary.

For OMP runs, the adapter mirrors DeNovoSWE auth seeding: it copies only the
`openai-codex` OAuth provider credential from `OMP_AUTH_SOURCE_AGENT_DIR`,
`~/.omp/profiles/stateful/agent`, or `~/.omp/agent` into the isolated OMP
profile/home. Submission archives exclude agent/runtime/cache directories and
files such as the provided top-level `executable`, `.omp`, `.codex`,
`.stateful*`, `.config`, `.cache`, `.git`, `Library/Caches`, `__pycache__`,
`.pytest_cache`, and Python bytecode files.

Install the official `programbench` CLI with one of:

```bash
pip install programbench
uv pip install programbench
uvx programbench
```

## Stateful Comparison Matrix

By default, `stateful-bench programbench run` compares Stateful off versus on
with subagents enabled:

```text
stateful:off,subagent:on
stateful:on,subagent:on
```

Use the same instance set, model, image tag, max turns, timeout, and network
policy across compared conditions. `--timeout-seconds` is passed to the Python
adapter, and the Rust wrapper enforces that timeout plus a fixed 30-minute
adapter lifecycle grace.
Explicit `subagent:off`
conditions still parse for diagnostic or backwards-compatible runs.

For `stateful:on`, the adapter installs and enables Stateful where the agent
runs: the host airlock for host CLI mode, or the separate OMP agent container
when `--agent-docker-image` is set. The target ProgramBench container stays
available for bundled `./executable` behavior checks and smoke compile.


## Reliability and Reporting

Use at least three independent trials per condition for stateful/control claims.
Keep the same instance set or filter, model, Docker image, timeout, max turns,
and network policy across compared conditions.

Reports should include each trial, mean, and standard deviation for the primary
quality metric. Label smoke, debug, interrupted, or partial runs as
non-comparable. Separate quality claims from efficiency claims: score/resolved
count establish quality, while wall time, tokens, and score-per-cost metrics
describe efficiency.

## Inference Rules

ProgramBench inference is offline by default. Agents must not search the
internet, clone repositories, fetch target source from package registries, wrap
the provided binary, decompile it, or run `strace`/`ltrace` on it.

Agents may run `./executable` normally and read bundled documentation from the
target container's `/workspace`. They should not use it for internet,
package-manager, source-control, or unrelated host filesystem work.

## Commands

Run a small Codex matrix over matching instances:

```bash
stateful-bench programbench run \
  --run-id pb-dev \
  --agent codex-cli \
  --model gpt-5.4-mini \
  --filter 'ripgrep.*' \
  --max-instances 2
```

Run OMP with a modern Codex model in a separate Docker agent image while keeping
ProgramBench's target container as the compile/smoke-test boundary:

```bash
stateful-bench programbench run \
  --run-id pb-omp-docker \
  --agent omp-cli \
  --model gpt-5.4-mini \
  --thinking high \
  --filter 'ripgrep.*' \
  --max-instances 2 \
  --agent-docker-image stateful-programbench-omp-agent:local
```

For OMP ProgramBench runs, `--thinking <value>` is passed through to the OMP
agent; omit it to use the model/runtime default.

Pass `--agent-docker-omp-bin`, `--agent-docker-stateful-binary`, or
`--agent-docker-home` only when the image does not use the defaults listed in
Runtime Requirements.

Evaluate the run with official ProgramBench tooling:

```bash
stateful-bench programbench eval \
  --run-dir .stateful_bench/programbench/runs/pb-dev \
  --workers 4 \
  --branch-workers 2 \
  --docker-cpus 8
```

Write JSON reports for each compared condition:

```bash
stateful-bench programbench report \
  --condition-dir .stateful_bench/programbench/runs/pb-dev/conditions/stateful-off_subagent-on \
  --format json \
  --output .stateful_bench/programbench/runs/pb-dev/reports/stateful-off_subagent-on.json

stateful-bench programbench report \
  --condition-dir .stateful_bench/programbench/runs/pb-dev/conditions/stateful-on_subagent-on \
  --format json \
  --output .stateful_bench/programbench/runs/pb-dev/reports/stateful-on_subagent-on.json
```

Compare two saved reports:

```bash
stateful-bench programbench compare \
  --report .stateful_bench/programbench/runs/pb-dev/reports/stateful-off_subagent-on.json \
  --report .stateful_bench/programbench/runs/pb-dev/reports/stateful-on_subagent-on.json \
  --format markdown
```

## Scoring

Official ProgramBench artifacts are the source of truth. `stateful-bench
programbench eval` runs `programbench eval`, `programbench info`, and
`programbench submit package` by default.

Reports read `_stats/score.json` and label the score source.

Report tables call this value `Partial score` because it is not the same as
solving the instance. Use `Resolved`/`resolved_count` for solved-instance
comparisons, especially when running a single diagnostic instance.

## Efficiency Metrics

Reports include wall time, token totals, uncached token totals, subagent usage,
score per million tokens, and score per hour. Subagent usage is observed from
adapter metadata and native `task` tool-call JSON events when available. Treat
quality and efficiency separately.
