# DeNovoSWE Benchmark Commands

Last updated: 2026-06-30.

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

Do not reuse older historical `target/stateful-bench/denovo/extracts/.../results.jsonl`
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
- If the model provider key is exported from a shell startup file, source that
  file before launching. Docker OMP runs pass allowlisted API-key environment
  variables such as `DEEPSEEK_API_KEY`, but they do not inherit host OMP login
  state or unsourced shell config.
- When launching through a restricted wrapper or external sandbox, do not rely
  on an interactive shell startup file that performs writes before exporting the
  model key. Source it in a normal shell first, or pass `DEEPSEEK_API_KEY`
  explicitly in the launch environment. A failed startup-file side effect such
  as `mkdir ~/.cache/oh-my-zsh: Operation not permitted` can prevent the key
  export and makes OMP exit before model execution.
  When using Colima, prefer the real socket path in `DOCKER_HOST`, for example
  `unix://$HOME/.colima/default/docker.sock`; `/var/run/docker.sock` may be a
  dangling Docker Desktop compatibility symlink.
- The DeNovo CLI patch harvester excludes Codex/stateful runtime dirs,
  `.stateful-tmp/**`, common Python cache/coverage dirs, `target/**`, root
  `clean.sh`, and `upstream/**`. Repo `tmp/**` changes are harvested like other
  source-tree changes.
- For host-side inputs and tools, use absolute paths for `STATEFUL_BENCH_BIN`,
  `--aweagent-root`, `--python`, `--data-file`, `--config`, `STATEFUL_BIN`,
  `OMP_BIN`, `TMUX`, and `TMUX_SOCKET`.
- For Docker-backed OMP agent runs through Docker Desktop or Colima, set
  `DOCKER_HOST` to the Unix socket used by the Docker CLI before launching.
- DeNovo code defaults write run outputs to `.stateful_bench/denovo/runs` and
  patch extracts to `.stateful_bench/denovo/extracts`. The commands below
  override the run root to `$REPO_ROOT/target/stateful_bench_runs/denovo/runs`
  so Docker/Colima can bind a host-mounted scratch path. Avoid historical
  `target/stateful-bench/denovo/...` paths.
- Change run IDs and isolated OMP home directories before reusing commands.
- For Docker-isolated OMP runs, build or tag the agent image from
  `crates/stateful-bench/docker/denovo-omp-agent.Dockerfile`. The image includes
  Bun-installed `omp`, the Linux `stateful` binary, and `bubblewrap` for
  stateful sandbox tool execution inside the agent container.
- Use `--prompt-version v2` for new official-style runs. Clap still defaults
  `denovo run --prompt-version` to `v1` for compatibility, so keep the flag
  explicit. Use `--prompt-version v1` only when continuing or comparing against
  historical v1 runs.
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
explicitly labeled as a debug or behavior test for concurrent-work or subagent
mechanics. For normal score, patch-quality, or stateful/no-state comparison
runs, keep the prompt limited to the benchmark task, official prompt-version
behavior, and declared condition axes; do not add ad hoc orchestration
instructions, role assignments, implementation hints, or strategy nudges. If a
behavior test needs injected instructions, label the run as such, keep the
injection limited to the behavior being measured, and do not treat the result as
a normal scored comparison.

## Command Variables

Set these values before reusing the commands:

```bash
REPO_ROOT=${REPO_ROOT:-$(pwd)}
STATEFUL_BENCH_RUNS=${STATEFUL_BENCH_RUNS:-$REPO_ROOT/target/stateful_bench_runs}
AWEAGENT_ROOT=${AWEAGENT_ROOT:-$REPO_ROOT/tmp/AweAgent}
PYTHON=${PYTHON:-$REPO_ROOT/tmp/aweagent-venv/bin/python}
STATEFUL_BENCH_BIN=${STATEFUL_BENCH_BIN:-$STATEFUL_BENCH_RUNS/cargo-target/debug/stateful-bench}
STATEFUL_BIN=${STATEFUL_BIN:-$STATEFUL_BENCH_RUNS/cargo-target/debug/stateful}
OMP_BIN=${OMP_BIN:-omp}
TMUX=${TMUX:-tmux}
TMUX_SOCKET=${TMUX_SOCKET:?set TMUX_SOCKET to the tmux server socket path}
STATEFUL_HOME=${STATEFUL_HOME:-$HOME/.stateful_core}
STATEFUL_SERVER_URL=$(python3 -c 'import json, os, pathlib; print(json.load(open(pathlib.Path(os.environ["STATEFUL_HOME"]) / "runtime/server.json"))["base_url"])')
STATEFUL_SERVER_TOKEN=$(python3 -c 'import json, os, pathlib; print(json.load(open(pathlib.Path(os.environ["STATEFUL_HOME"]) / "runtime/server.json"))["token"])')
DENOVO_OMP_AGENT_IMAGE=stateful-denovo-omp-agent:local
DOCKER_OMP_BIN=omp
DOCKER_STATEFUL_BIN=/usr/local/bin/stateful
DOCKER_HOST=${DOCKER_HOST:?set DOCKER_HOST to unix://<docker-socket>}
DENOVO_OUTPUT_ROOT=${DENOVO_OUTPUT_ROOT:-$STATEFUL_BENCH_RUNS/denovo/runs}
RUN_SERIES=rNN-denovo
TRIAL=1
RUN_ID=$RUN_SERIES-t$TRIAL
```

`DENOVO_OUTPUT_ROOT` is a documented scratch override for these examples, not
the CLI default. Without `--output-dir`, DeNovo uses `.stateful_bench/denovo/runs`
for run output; patch extraction defaults to `.stateful_bench/denovo/extracts`.

## OMP CLI Default

Use `--agent omp-cli` for DeNovoSWE benchmark runs. Use `deepseek-v4-flash`
unless deliberately testing another model:

```bash
"$STATEFUL_BENCH_BIN" denovo run \
  --agent omp-cli \
  --aweagent-root "$AWEAGENT_ROOT" \
  --python "$PYTHON" \
  --data-file "$REPO_ROOT/datasets/denovo/shards/denovoswe_public_shard_a.jsonl" \
  --output-dir "$DENOVO_OUTPUT_ROOT" \
  --run-id "$RUN_ID-shard-a-omp" \
  --mode batch \
  --condition stateful:off,subagent:off \
  --condition stateful:on,subagent:off \
  --condition stateful:off,subagent:on \
  --condition stateful:on,subagent:on \
  --omp-bin "$OMP_BIN" \
  --stateful-binary "$STATEFUL_BIN" \
  --benchmark-model deepseek-v4-flash \
  --benchmark-reasoning-effort high \
  --benchmark-model-context-window 256000 \
  --benchmark-temperature 1 \
  --benchmark-max-turns 500 \
  --prompt-version v2 \
  --eval-iters 1
```

For a one-instance stateful smoke run, keep the same defaults and narrow the
matrix:

```bash
"$STATEFUL_BENCH_BIN" denovo run \
  --agent omp-cli \
  --aweagent-root "$AWEAGENT_ROOT" \
  --python "$PYTHON" \
  --data-file "$REPO_ROOT/datasets/denovo/shards/denovoswe_public_shard_b.jsonl" \
  --output-dir "$DENOVO_OUTPUT_ROOT" \
  --run-id "$RUN_ID-one-stateful-on-omp" \
  --config "$AWEAGENT_ROOT/configs/tasks/denovoswe.yaml" \
  --mode batch \
  --condition stateful:on,subagent:on \
  --max-concurrent 1 \
  --instance-id aurzenligl_prophy_pr33 \
  --omp-bin "$OMP_BIN" \
  --stateful-binary "$STATEFUL_BIN" \
  --benchmark-model deepseek-v4-flash \
  --benchmark-reasoning-effort high \
  --benchmark-model-context-window 256000 \
  --benchmark-temperature 1 \
  --benchmark-max-turns 500 \
  --prompt-version v2 \
  --eval-iters 1
```

For the same one-instance smoke shape with OMP running inside the benchmark
Docker agent image, use an image built from
`crates/stateful-bench/docker/denovo-omp-agent.Dockerfile`. Docker already
isolates the agent workspace, so the sample disables the nested OMP sandbox to
avoid bubblewrap namespace failures inside the container. It also passes the
default in-image `stateful` path explicitly; omit
`--agent-docker-stateful-binary` when the image uses the default:

```bash
"$STATEFUL_BENCH_BIN" denovo run \
  --agent omp-cli \
  --aweagent-root "$AWEAGENT_ROOT" \
  --python "$PYTHON" \
  --data-file "$REPO_ROOT/datasets/denovo/shards/denovoswe_public_shard_b.jsonl" \
  --output-dir "$DENOVO_OUTPUT_ROOT" \
  --run-id "$RUN_ID-one-omp-docker" \
  --config "$AWEAGENT_ROOT/configs/tasks/denovoswe.yaml" \
  --mode batch \
  --condition stateful:off,subagent:on \
  --condition stateful:on,subagent:on \
  --max-concurrent 1 \
  --instance-id aurzenligl_prophy_pr33 \
  --omp-bin "$DOCKER_OMP_BIN" \
  --stateful-binary "$STATEFUL_BIN" \
  --agent-docker-image "$DENOVO_OMP_AGENT_IMAGE" \
  --agent-docker-stateful-binary "$DOCKER_STATEFUL_BIN" \
  --agent-docker-sandbox off \
  --benchmark-model deepseek-v4-flash \
  --benchmark-reasoning-effort high \
  --benchmark-model-context-window 256000 \
  --benchmark-temperature 1 \
  --benchmark-max-turns 500 \
  --prompt-version v2 \
  --eval-iters 1
```

## Current 12-Instance Docker OMP Subagent-On Run

This is the relaunch shape used for the 2026-06-24 OMP Docker run over 12
instances with `subagent:on`, `stateful:on/off`, six-way instance concurrency,
and three independent trials. It is a declared subagent/concurrency behavior
test, not the official-style `--max-concurrent 1` default from
`docs/denovo-benchmark-guide.md`.

Report and collection commands in this section summarize that behavior run only.
They do not convert the 12-instance `--max-concurrent 6` reduced matrix into
official-style `--max-concurrent 1` quality evidence. Final comparison tables
must state both `max-concurrent` and matrix type, for example `reduced
stateful:on/off,subagent:on` versus the full four-axis matrix.

Rebuild the Docker agent image before relaunching after local `stateful` or OMP
integration changes. The examples below pass `--agent-docker-sandbox off`
because Docker isolation is the sandbox boundary for the benchmark agent:

```bash
DOCKER_HOST="$DOCKER_HOST" docker build --pull --no-cache \
  -f "$REPO_ROOT/crates/stateful-bench/docker/denovo-omp-agent.Dockerfile" \
  -t "$DENOVO_OMP_AGENT_IMAGE" \
  "$REPO_ROOT"
```

Source the shell file that exports the model API key before launching the
benchmark. The adapter passes allowlisted provider keys into the Docker agent
container:

```bash
source "$HOME/.zshrc"

export PATH="/opt/homebrew/bin:/opt/homebrew/sbin:$HOME/.cargo/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin:${PATH:-}"
export STATEFUL_SERVER_URL
export STATEFUL_SERVER_TOKEN
export DOCKER_HOST

for trial in 1 2 3; do
  "$STATEFUL_BENCH_BIN" denovo run \
    --agent omp-cli \
    --aweagent-root "$AWEAGENT_ROOT" \
    --python "$PYTHON" \
    --data-file "$REPO_ROOT/datasets/denovo/shards/denovoswe_public_shard_b.jsonl" \
    --output-dir "$DENOVO_OUTPUT_ROOT" \
    --run-id "$RUN_SERIES-t${trial}" \
    --config "$AWEAGENT_ROOT/configs/tasks/denovoswe.yaml" \
    --condition stateful:off,subagent:on \
    --condition stateful:on,subagent:on \
    --mode batch \
    --max-concurrent 6 \
    --instance-id tlambert03_mkdocs-api-autonav_pr25 \
    --instance-id russhousley_pyasn1-alt-modules_pr92 \
    --instance-id francof2a_fxpmath_pr63 \
    --instance-id cloudtools_troposphere_pr2343 \
    --instance-id ramonhagenaars_jsons_pr143 \
    --instance-id thebjorn_pydeps_pr233 \
    --instance-id hh-h_aiohttp-swagger3_pr109 \
    --instance-id pusher_pusher-http-python_pr207 \
    --instance-id lepture_flask-oauthlib_pr385 \
    --instance-id aurzenligl_prophy_pr33 \
    --instance-id phfaist_pylatexenc_pr66 \
    --instance-id pahaz_sshtunnel_pr247 \
    --omp-bin "$DOCKER_OMP_BIN" \
    --stateful-binary "$STATEFUL_BIN" \
    --agent-docker-image "$DENOVO_OMP_AGENT_IMAGE" \
    --agent-docker-stateful-binary "$DOCKER_STATEFUL_BIN" \
    --agent-docker-sandbox off \
    --benchmark-model deepseek-v4-flash \
    --benchmark-reasoning-effort high \
    --benchmark-model-context-window 256000 \
    --benchmark-temperature 1 \
    --benchmark-max-turns 500 \
    --subagent-min-count 3 \
    --max-resumes 1 \
    --codex-timeout-seconds 7200 \
    --eval-iters 1 \
    --prompt-version v2 \
    --del-done-images
done
```

The corresponding authenticated run used:

```bash
REPO_ROOT=${REPO_ROOT:-$(pwd)}
STATEFUL_BENCH_RUNS=${STATEFUL_BENCH_RUNS:-$REPO_ROOT/target/stateful_bench_runs}
RUN_SERIES=r20260624-denovo-12-omp-docker-subagent-on-auth
STATEFUL_BENCH_BIN=${STATEFUL_BENCH_BIN:-$STATEFUL_BENCH_RUNS/cargo-target/debug/stateful-bench}
STATEFUL_BIN=${STATEFUL_BIN:-$STATEFUL_BENCH_RUNS/cargo-target/debug/stateful}
PYTHON=${PYTHON:-$REPO_ROOT/tmp/aweagent-venv/bin/python}
AWEAGENT_ROOT=${AWEAGENT_ROOT:-$REPO_ROOT/tmp/AweAgent}
DENOVO_OUTPUT_ROOT=${DENOVO_OUTPUT_ROOT:-$STATEFUL_BENCH_RUNS/denovo/runs}
DOCKER_HOST=${DOCKER_HOST:-unix://$HOME/.colima/default/docker.sock}
```

Relaunch pitfalls observed while debugging single-instance Docker OMP runs:

- Do not treat the tmux shard launcher as the default path for a one-instance
  Docker rerun. The direct `stateful-bench denovo run ... --agent-docker-image`
  command above is the source of truth; tmux is only a convenience wrapper for
  prebuilt shard scripts.
- In agent harnesses that clean up child process groups when a sandboxed command
  exits, `daemonize.py`/background `&` launches may create a pid file and then
  die before `stateful-bench` creates the run directory. For long-running
  fire-and-forget launches from that environment, use an existing tmux server as
  the process owner and run the same direct `stateful-bench denovo run` command
  inside it, passing `STATEFUL_*`, `DOCKER_HOST`, and provider key variables
  with `tmux new-session -e`.
- When launching from a sandboxed or external-command harness, include the tmux
  server socket path in the approved external socket scope before starting a
  detached run. Use a short, existing-parent socket path because long paths can
  exceed tmux's socket length limit; create the session with that socket and
  keep benchmark stdout/stderr in a launch log under the run output directory.
- If Docker reports `invalid mount config for type "bind": bind source path
  does not exist` for an `omp-homes/<instance>/home` path, first verify whether
  the benchmark output root is under `/private/tmp` or another path not mounted
  into the Docker daemon. The host directory can exist while Colima still cannot
  bind it. Move `DENOVO_OUTPUT_ROOT` to the documented scratch override under
  `$REPO_ROOT/target/stateful_bench_runs` or another `/Users/...` path and
  relaunch with fresh run IDs.
- Rebuild or retag the Docker OMP agent image after changing the Dockerfile. If
  a Docker OMP run still shows `bubblewrap` namespace failures, confirm the
  command includes `--agent-docker-sandbox off`; otherwise nested OMP sandboxing
  is still enabled inside the already-isolated container.
- If OMP exits in about one second with empty `patch.diff`, zero subagent
  spawns, and `omp exited 1`, first check provider auth propagation. In Docker
  OMP mode the isolated home does not inherit host OMP login state, and the
  adapter only forwards allowlisted provider key environment variables that are
  present before `stateful-bench` starts.
- Treat `finish_reason: "benchmark-contamination"` as an invalid rollout, not a
  scored model failure. It means the adapter found an `upstream` checkout in the
  harvested workspace or explicit target upstream access in session artifacts:
  `.read.log` URL headers, `read`/`browser` URL or path tool-call arguments, or
  shell command tool-call arguments containing forbidden target-upstream commands.
  OMP runs now also pre-block configured target-source patterns before tool use,
  and Docker OMP runs deny matching GitHub/raw/patch/API HTTP and `CONNECT`
  traffic through the adapter proxy.
- A `stateful:on` Docker run that emits `SessionRegistered` but no nested
  `SessionHeartbeat` or `ActivityFinalized` is not lifecycle-valid. Report it as
  a runtime/lifecycle failure, not as a model-quality score.
- Do not keep abandoned runtime containers around after a failed launch. Remove
  containers named for the instance, such as
  `awe-agent-denovoswe-<instance-id>-<suffix>`, before rerunning the same
  instance so the next run starts from a clean runtime container set.

OMP `stateful:on`/`off` both use isolated OMP home/profile state. Neither
inherits host Codex config, session, rules, or skills. `stateful:on` receives the
stateful OMP install/config; `stateful:off` receives only the benchmark source
guard extension so source-control blocks remain active without Stateful hooks.

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

For `subagent:on`, the generated DeNovo prompt appends only `orchestrate`.
`subagent:off` does not add a custom subagent prompt. OMP runs also unpack
bundled task agents into the isolated runtime home and enable
`features.multi_agent=true`. The adapter still enforces the minimum native
subagent spawn count for both Codex and OMP `subagent:on` runs. This prompt
addition is a declared behavior-test condition axis, not a general prompt policy
for normal scored comparisons.

Shard launch scripts are expected at:

```text
$REPO_ROOT/.stateful_bench/denovo/run-control/run-$RUN_ID-shard-a.sh
$REPO_ROOT/.stateful_bench/denovo/run-control/run-$RUN_ID-shard-b.sh
$REPO_ROOT/.stateful_bench/denovo/run-control/run-$RUN_ID-shard-c.sh
```

## Start Shards In Existing tmux

These commands use an existing tmux server socket. Pass
`STATEFUL_SERVER_URL` and `STATEFUL_SERVER_TOKEN` explicitly on the
`tmux new-session` command with `-e`; exporting or sourcing them inside the
launcher script is not enough when the existing tmux server owns the child
environment. If either variable is missing, the `stateful:on` condition fails
its runtime preflight before the agent starts.

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

Progress output includes compact orchestration trace summaries when stateful
trace capture is available. Prefer the summary counters for first-pass analysis:
`events`, `heartbeat`, and `denial` in the rendered table, plus JSON fields such
as `orchestration_event_types`, `orchestration_denial_paths`, and
`orchestration_heartbeat_max_gap_ms` with `--format json`. The raw
`orchestration-trace.json` remains the audit artifact for detailed event order.

If `results.jsonl` shows `finish_reason: "benchmark-contamination"`, inspect
`codex-command.json.benchmark_contamination`; `kind: "upstream-worktree"` means
an `upstream/` checkout remained in the final workspace, and
`kind: "upstream-source-access"` means session artifacts showed explicit target
upstream access through `.read.log` URL headers, `read`/`browser` URL or path
tool-call arguments, forbidden target-upstream commands in shell command tool
calls, or traffic denied by the Docker OMP source-access proxy.

After all three behavior-test trials complete, collect each trial separately and
report the mean:

These collection commands are not an official-style quality comparison unless
the underlying runs used the official-style `--max-concurrent 1` design. Keep
`max-concurrent` and matrix type in the final table alongside run IDs and trial
IDs.

```bash
python3 crates/stateful-bench/scripts/denovo_progress_report.py --run-prefix "$RUN_SERIES-t1-shard-" --expected-instances-per-condition 3675
python3 crates/stateful-bench/scripts/denovo_progress_report.py --run-prefix "$RUN_SERIES-t2-shard-" --expected-instances-per-condition 3675
python3 crates/stateful-bench/scripts/denovo_progress_report.py --run-prefix "$RUN_SERIES-t3-shard-" --expected-instances-per-condition 3675
```

For per-instance failure analysis, inspect the configured run output root, for
example:

```text
$DENOVO_OUTPUT_ROOT/<run-id>/conditions/<condition>/omp-cli/instances/<instance-id>/eval-result.json
```

Stop sessions:

```bash
"$TMUX" -S "$TMUX_SOCKET" kill-session -t "$RUN_ID-shard-a-omp"

"$TMUX" -S "$TMUX_SOCKET" kill-session -t "$RUN_ID-shard-b-omp"

"$TMUX" -S "$TMUX_SOCKET" kill-session -t "$RUN_ID-shard-c-omp"
```
