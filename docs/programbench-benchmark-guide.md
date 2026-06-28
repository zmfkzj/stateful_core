# ProgramBench Benchmark Guide

ProgramBench is a reverse-engineering benchmark: an agent receives a cleanroom
container with a compiled `./executable` and bundled documentation, then writes
an original source tree and `compile.sh` that rebuilds an equivalent executable.

## Runtime Requirements

ProgramBench Docker images target Linux `amd64`. macOS developers can inspect
commands and reports, but scored inference/evaluation needs a compatible Docker
host.

`stateful-bench` keeps each ProgramBench Docker container alive until the run
finishes, then removes it explicitly. The Codex and OMP adapters copy the
container's `/workspace` into an empty temporary host airlock, run the host CLI
there, and archive that airlock as the ProgramBench submission.

Install the official `programbench` CLI with one of:

```bash
pip install programbench
uv pip install programbench
uvx programbench
```

## Stateful Comparison Matrix

By default, `stateful-bench programbench run` plans the four condition matrix:

```text
stateful:off,subagent:off
stateful:on,subagent:off
stateful:off,subagent:on
stateful:on,subagent:on
```

Use the same instance set, model, image tag, max turns, timeout, and network
policy across compared conditions.

For `stateful:on`, the adapter installs and enables Stateful in the host
airlock used by that agent run. The ProgramBench container seeds the airlock and
stays available for bundled `./executable` behavior checks.

## Inference Rules

ProgramBench inference is offline by default. Agents must not search the
internet, clone repositories, fetch target source from package registries, wrap
the provided binary, decompile it, or run `strace`/`ltrace` on it.

Agents may run `./executable` normally and read bundled documentation from the
airlock seeded from the target container. The airlock should not be used for
internet, package-manager, source-control, or unrelated host filesystem work.

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
  --condition-dir .stateful_bench/programbench/runs/pb-dev/conditions/stateful-off_subagent-off \
  --format json \
  --output .stateful_bench/programbench/runs/pb-dev/reports/stateful-off_subagent-off.json

stateful-bench programbench report \
  --condition-dir .stateful_bench/programbench/runs/pb-dev/conditions/stateful-on_subagent-off \
  --format json \
  --output .stateful_bench/programbench/runs/pb-dev/reports/stateful-on_subagent-off.json
```

Compare two saved reports:

```bash
stateful-bench programbench compare \
  --report .stateful_bench/programbench/runs/pb-dev/reports/stateful-off_subagent-off.json \
  --report .stateful_bench/programbench/runs/pb-dev/reports/stateful-on_subagent-off.json \
  --format markdown
```

## Scoring

Official ProgramBench artifacts are the source of truth. `stateful-bench
programbench eval` runs `programbench eval`, `programbench info`, and
`programbench submit package` by default.

Reports read `_stats/score.json` and label the score source.

## Efficiency Metrics

Reports include wall time, token totals, uncached token totals, subagent usage,
score per million tokens, and score per hour. Subagent usage is observed from
adapter metadata and native `task` tool-call JSON events when available. Treat
quality and efficiency separately.
