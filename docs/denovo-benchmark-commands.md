# DeNovoSWE Benchmark Commands

Last updated: 2026-06-22.

Use this file to relaunch the OMP-backed DeNovoSWE benchmark without
reconstructing the command line.

For benchmark interpretation rules, reliability requirements, and failure
analysis guidance, read `docs/denovo-benchmark-guide.md` first. In particular,
publishable DeNovoSWE comparisons should follow the official paper's practice
of averaging metrics across three independent execution trials.

## Dataset

Full public dataset:

```text
<repo-root>/datasets/denovo/denovoswe_public.jsonl
```

Verification:

```text
rows: 3675
bytes: 4568928853
sha256: 4ffc25f17f71988228b856af8c78307c6f528e1ad3fe198086ff6a98ff82e31a
source file: AweAI-Team/DeNovoSWE denovoswe_public.jsonl
```

Current shard files:

```text
datasets/denovo/shards/denovoswe_public_shard_a.jsonl  rows 1-1225
datasets/denovo/shards/denovoswe_public_shard_b.jsonl  rows 1226-2450
datasets/denovo/shards/denovoswe_public_shard_c.jsonl  rows 2451-3675
```

Do not reuse older `target/stateful-bench/denovo/extracts/.../results.jsonl`
paths as "full" dataset inputs. The r33-r35 attempts used a 6-row extracted
file, not the public full JSONL. The aborted r36 attempt duplicated the full
3675-row input three times instead of sharding it.

## Preconditions

- `target/debug/stateful-bench` must exist.
- When not using Docker, `omp` must be installed on the host. Docker OMP runs
  use the OMP executable inside the agent image. `<absolute-stateful-binary>` and
  the local stateful server must be installed and reachable.
- For `stateful:on`, export `STATEFUL_SERVER_URL` and `STATEFUL_SERVER_TOKEN`
  from the local runtime metadata before launching nested benchmark agents.
- Use the official AweAgent DeNovoSWE recipe/task workflow: extract patches
  once, run the agent in batch mode, evaluate in a fresh container, and score
  by unit-test pass ratio.
- OMP runs use isolated OMP home/profile state. Seed the required OMP API key
  into that isolated profile, or provide it through an environment variable;
  host OMP profile auth is not inherited automatically.
- The DeNovo CLI patch harvester excludes Codex/stateful runtime dirs,
  `.stateful-tmp/**`, common Python cache/coverage dirs, `target/**`, and root
  `clean.sh`. Repo `tmp/**` changes are harvested like other source-tree
  changes.
- For host-side inputs and tools, use absolute paths for `--aweagent-root`,
  `--python`, `--data-file`, `--config`, `STATEFUL_BIN`, `OMP_BIN`, `TMUX`, and
  `TMUX_SOCKET`.
- Change run IDs and isolated OMP home directories before reusing commands.
- For Docker-isolated OMP runs, build or tag the agent image from
  `crates/stateful-bench/docker/denovo-omp-agent.Dockerfile`. The image includes
  Bun-installed `omp` plus the Linux `stateful` binary.
- Use `--prompt-version v2` for new official-style runs. Keep
  `--prompt-version v1` only when continuing or comparing against historical
  v1 runs.
- Treat `--eval-iters` as evaluator repetition only. It does not replace the
  three independent agent trials needed for stable benchmark comparisons.

## Reliability Policy

Official DeNovoSWE results are averaged across three independent execution
trials to reduce experimental variance. Follow the same policy locally:

```text
RUN_SERIES=rNN-denovo
TRIALS=1,2,3
RUN_ID=$RUN_SERIES-t$TRIAL
```

For each trial, launch the same shard files and condition matrix with a fresh
`RUN_ID` and fresh isolated OMP home directories. Aggregate only after all
three trials finish. One-off, interrupted, or partial runs are useful for
debugging and failure analysis, but should be labeled non-comparable.

## Prompt Policy

Benchmark runs must not inject extra prompt instructions except when the run is
explicitly testing concurrent-work behavior. For normal score or patch-quality
runs, keep the prompt limited to the benchmark task and declared condition axes;
do not add ad hoc orchestration instructions, role assignments, implementation
hints, or strategy nudges. If a concurrency-behavior test needs injected
instructions, label the run as such and keep the injection limited to the
concurrent-work behavior being measured.

## Command Variables

Set these values before reusing the commands:

```bash
REPO_ROOT=/absolute/path/to/stateful_core
AWEAGENT_ROOT=/absolute/path/to/AweAgent
PYTHON=/absolute/path/to/python3
STATEFUL_BIN=/absolute/path/to/stateful
OMP_BIN=/absolute/path/to/omp
TMUX=/absolute/path/to/tmux
TMUX_SOCKET=/absolute/path/to/tmux/socket
STATEFUL_HOME=${STATEFUL_HOME:-$HOME/.stateful_core}
STATEFUL_SERVER_URL=$(python3 -c 'import json, os, pathlib; print(json.load(open(pathlib.Path(os.environ["STATEFUL_HOME"]) / "runtime/server.json"))["base_url"])')
STATEFUL_SERVER_TOKEN=$(python3 -c 'import json, os, pathlib; print(json.load(open(pathlib.Path(os.environ["STATEFUL_HOME"]) / "runtime/server.json"))["token"])')
DENOVO_OMP_AGENT_IMAGE=stateful-denovo-omp-agent:local
DOCKER_OMP_BIN=omp
DOCKER_STATEFUL_BIN=/usr/local/bin/stateful
RUN_SERIES=rNN-denovo
TRIAL=1
RUN_ID=$RUN_SERIES-t$TRIAL
```

## OMP CLI Default

Use `--agent omp-cli` for DeNovoSWE benchmark runs. Use `deepseek-v4-flash`
unless deliberately testing another model:

```bash
stateful-bench denovo run \
  --agent omp-cli \
  --aweagent-root "$AWEAGENT_ROOT" \
  --python "$PYTHON" \
  --data-file "$REPO_ROOT/datasets/denovo/shards/denovoswe_public_shard_a.jsonl" \
  --output-dir "$REPO_ROOT/.stateful_bench/denovo/runs" \
  --run-id "$RUN_ID-shard-a-omp" \
  --mode batch \
  --condition stateful:off,subagent:off \
  --condition stateful:on,subagent:off \
  --condition stateful:off,subagent:on \
  --condition stateful:on,subagent:on \
  --omp-bin "$OMP_BIN" \
  --stateful-binary "$STATEFUL_BIN" \
  --benchmark-model deepseek-v4-flash \
  --benchmark-reasoning-effort low \
  --benchmark-model-context-window 256000 \
  --benchmark-temperature 1 \
  --benchmark-max-turns 500 \
  --prompt-version v2 \
  --eval-iters 1
```

For a one-instance stateful smoke run, keep the same defaults and narrow the
matrix:

```bash
stateful-bench denovo run \
  --agent omp-cli \
  --aweagent-root "$AWEAGENT_ROOT" \
  --python "$PYTHON" \
  --data-file "$REPO_ROOT/datasets/denovo/shards/denovoswe_public_shard_b.jsonl" \
  --output-dir "$REPO_ROOT/target/stateful-bench/denovo/runs" \
  --run-id "$RUN_ID-one-stateful-on-omp" \
  --config "$REPO_ROOT/target/stateful-bench/denovo/configs/denovoswe-cpu2.yaml" \
  --mode batch \
  --condition stateful:on,subagent:on \
  --max-concurrent 1 \
  --instance-id aurzenligl_prophy_pr33 \
  --omp-bin "$OMP_BIN" \
  --stateful-binary "$STATEFUL_BIN" \
  --benchmark-model deepseek-v4-flash \
  --benchmark-reasoning-effort low \
  --benchmark-model-context-window 256000 \
  --benchmark-temperature 1 \
  --benchmark-max-turns 500 \
  --prompt-version v2 \
  --eval-iters 1
```

For the same one-instance smoke shape with OMP running inside the benchmark
Docker agent image, use an image built from
`crates/stateful-bench/docker/denovo-omp-agent.Dockerfile`. The sample passes
the default in-image `stateful` path explicitly; omit
`--agent-docker-stateful-binary` when the image uses the default:

```bash
stateful-bench denovo run \
  --agent omp-cli \
  --aweagent-root "$AWEAGENT_ROOT" \
  --python "$PYTHON" \
  --data-file "$REPO_ROOT/datasets/denovo/shards/denovoswe_public_shard_b.jsonl" \
  --output-dir "$REPO_ROOT/target/stateful-bench/denovo/runs" \
  --run-id "$RUN_ID-one-omp-docker" \
  --config "$REPO_ROOT/target/stateful-bench/denovo/configs/denovoswe-cpu2.yaml" \
  --mode batch \
  --condition stateful:off,subagent:on \
  --condition stateful:on,subagent:on \
  --max-concurrent 1 \
  --instance-id aurzenligl_prophy_pr33 \
  --omp-bin "$DOCKER_OMP_BIN" \
  --stateful-binary "$STATEFUL_BIN" \
  --agent-docker-image "$DENOVO_OMP_AGENT_IMAGE" \
  --agent-docker-stateful-binary "$DOCKER_STATEFUL_BIN" \
  --benchmark-model deepseek-v4-flash \
  --benchmark-reasoning-effort low \
  --benchmark-model-context-window 256000 \
  --benchmark-temperature 1 \
  --benchmark-max-turns 500 \
  --prompt-version v2 \
  --eval-iters 1
```

OMP `stateful:on`/`off` both use isolated OMP home/profile state. Neither
inherits host Codex config, session, rules, or skills. Only `stateful:on`
receives stateful OMP install/config.

Add `--agent-docker-image <image>` to run the OMP CLI inside a dedicated
container instead of the host OMP binary. In this mode, `--omp-bin` names the
OMP executable inside the image. The adapter mounts only the instance
workspace, prompt file, and isolated OMP home into that container. For
`stateful:on`, it uses `/home/stateful` as the runtime home so the isolated OMP
`stateful` profile is container-visible, and rewrites both the mounted
`$STATEFUL_HOME/config.yml` repo registry and `repos/*.json` metadata from host
workspace paths to `/workspace`.

The Docker agent image at
`crates/stateful-bench/docker/denovo-omp-agent.Dockerfile` includes Bun-installed
`omp` plus the Linux `stateful` binary. The in-container `stateful` binary path
defaults to `/usr/local/bin/stateful`; override it with
`--agent-docker-stateful-binary <path>` when the image uses another path.

A lifecycle-valid Docker `stateful:on` run should emit `SessionRegistered`,
repeated `SessionHeartbeat`, and `ActivityFinalized` for the nested OMP session.
The verified smoke run
`r110-denovo-one-omp-docker-stateful-onoff-subagent-on` completed
stateful-off/stateful-on with `subagent:on` and emitted that sequence. Treat a
missing registration, absent heartbeat, or missing finalization as a lifecycle
failure rather than a model-quality result.

The `subagent` axis is retained for matrix shape, but OMP does not use Codex
native subagent enforcement or Codex subagent usage counters.

Shard launch scripts are expected at:

```text
$REPO_ROOT/.stateful_bench/denovo/run-control/run-$RUN_ID-shard-a.sh
$REPO_ROOT/.stateful_bench/denovo/run-control/run-$RUN_ID-shard-b.sh
$REPO_ROOT/.stateful_bench/denovo/run-control/run-$RUN_ID-shard-c.sh
```

## Start Shards In Existing tmux

These commands use an existing tmux server socket. Pass the `STATEFUL_*`
runtime variables into each tmux session with `tmux new-session -e`; otherwise
the stateful condition will fail its runtime preflight.

```bash
"$TMUX" -S "$TMUX_SOCKET" new-session -d -s "$RUN_ID-shard-a-omp" -e STATEFUL_SERVER_URL="$STATEFUL_SERVER_URL" -e STATEFUL_SERVER_TOKEN="$STATEFUL_SERVER_TOKEN" /bin/zsh "$REPO_ROOT/.stateful_bench/denovo/run-control/run-$RUN_ID-shard-a.sh"

"$TMUX" -S "$TMUX_SOCKET" new-session -d -s "$RUN_ID-shard-b-omp" -e STATEFUL_SERVER_URL="$STATEFUL_SERVER_URL" -e STATEFUL_SERVER_TOKEN="$STATEFUL_SERVER_TOKEN" /bin/zsh "$REPO_ROOT/.stateful_bench/denovo/run-control/run-$RUN_ID-shard-b.sh"

"$TMUX" -S "$TMUX_SOCKET" new-session -d -s "$RUN_ID-shard-c-omp" -e STATEFUL_SERVER_URL="$STATEFUL_SERVER_URL" -e STATEFUL_SERVER_TOKEN="$STATEFUL_SERVER_TOKEN" /bin/zsh "$REPO_ROOT/.stateful_bench/denovo/run-control/run-$RUN_ID-shard-c.sh"
```

List sessions:

```bash
"$TMUX" -S "$TMUX_SOCKET" ls
```

Progress report:

```bash
python3 crates/stateful-bench/scripts/denovo_progress_report.py --run-prefix "$RUN_ID-shard-" --expected-instances-per-condition 3675
```

During live runs, prefer cumulative `denovo-report.json` values over raw
adapter `results.jsonl` files. Matrix runs may rewrite or reset raw
`results.jsonl` files while conditions advance, so raw row counts can be
misleading until the run has settled.

After all three trials complete, collect each trial separately and report the
mean:

```bash
python3 crates/stateful-bench/scripts/denovo_progress_report.py --run-prefix "$RUN_SERIES-t1-shard-" --expected-instances-per-condition 3675
python3 crates/stateful-bench/scripts/denovo_progress_report.py --run-prefix "$RUN_SERIES-t2-shard-" --expected-instances-per-condition 3675
python3 crates/stateful-bench/scripts/denovo_progress_report.py --run-prefix "$RUN_SERIES-t3-shard-" --expected-instances-per-condition 3675
```

For per-instance failure analysis, inspect:

```text
target/stateful-bench/denovo/runs/<run-id>/conditions/<condition>/omp-cli/instances/<instance-id>/eval-result.json
```

Stop sessions:

```bash
"$TMUX" -S "$TMUX_SOCKET" kill-session -t "$RUN_ID-shard-a-omp"

"$TMUX" -S "$TMUX_SOCKET" kill-session -t "$RUN_ID-shard-b-omp"

"$TMUX" -S "$TMUX_SOCKET" kill-session -t "$RUN_ID-shard-c-omp"
```
