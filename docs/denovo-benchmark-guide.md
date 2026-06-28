# DeNovoSWE Benchmark Guide

Last updated: 2026-06-24.

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
The agent may use ordinary internet access for non-target third-party dependency
research when the configured scaffold exposes search, but must not inspect or
recover the target package's upstream repository, pull request, issue, patch,
commit, raw source, package-manager artifact, wheel, sdist, or source cache while
solving the instance. The adapter invalidates runs when the final workspace
contains `upstream/` or session artifacts show explicit target upstream access:
`.read.log` URL headers, `read`/`browser` URL or path tool-call arguments, or
shell command tool-call arguments containing forbidden target-upstream commands.

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
  For Docker OMP runs, provider API keys must be present in the launch
  environment before `stateful-bench` starts; sourcing a shell startup file such
  as `~/.zshrc` is sufficient when that file exports the key.
  When a wrapper or sandbox blocks shell startup side effects, export the key
  explicitly instead of relying on that startup file. Missing provider auth often
  appears as an immediate `omp exited 1`, empty patch, and zero subagent spawns.
- For Docker-isolated OMP agent runs, build or tag the image from
  `crates/stateful-bench/docker/denovo-omp-agent.Dockerfile`; it includes
  Bun-installed `omp`, the Linux `stateful` binary, and `bubblewrap`/`bwrap` for
  the default `--agent-docker-sandbox on` path. Add `--agent-docker-image
  <image>`. Pass `--agent-docker-sandbox off` when the Docker container itself
  should be the sandbox boundary; `--agent-docker-stateful-binary <path>` only
  needs to be set when the image's `stateful` binary is not at
  `/usr/local/bin/stateful`.
- A reduced `stateful:off/on,subagent:on` matrix with `--max-concurrent 6` is a
  subagent/concurrency behavior test. Keep the same 12-instance list, shard,
  model, prompt version, temperature, context window, max turns, evaluator
  settings, and Docker image across all three trials, and do not mix it with
  official-style `--max-concurrent 1` comparisons.

Historical runs may use `--prompt-version v1`; do not mix v1 and v2 results in
the same comparison table.

## Prompt Policy

Do not add ad hoc prompt instructions for normal scored, patch-quality, or
stateful/no-state comparison runs. Keep the agent prompt limited to the
benchmark task, benchmark isolation/source-control restrictions, official prompt
version behavior, and declared condition axes.
Extra strategy hints, orchestration instructions, lifecycle reminders, role
assignments, or implementation guidance can bias the rollout and make the result
non-comparable.

The adapter's built-in benchmark isolation prompt is an integrity guardrail, not
a strategy hint. Keep it enabled for normal scored runs so agents are evaluated
only on the provided workspace and package specification.

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

The 12-instance Docker OMP command in
`docs/denovo-benchmark-commands.md` uses this reduced subagent matrix and
`--max-concurrent 6`. Treat it as a declared behavior/concurrency run; report it
separately from full four-axis or official-style stateful/no-state comparisons.

For `subagent:on`, the generated DeNovo prompt explicitly requires native
Codex/OMP subagents before implementation or broad repository exploration,
while allowing narrow preflight to read the prompt, inspect tool availability,
or initialize stateful coordination. It tells OMP to use the current `task` tool
or older multi-agent tools such as `multi_agent_v1spawn_agent`, requires every
counted subagent to inspect, edit, and verify a distinct implementation slice,
and requires explicit blocker reporting if the runtime does not expose subagent
tools. OMP runs also unpack bundled task agents into the isolated runtime home,
append the requirement to the system prompt, and enable `features.multi_agent=true`.
The adapter enforces the minimum native subagent spawn count for both Codex and
OMP `subagent:on` runs. Treat that injected instruction as a declared
behavior-test condition axis; do not reuse it as normal scored comparison policy
or as general patch-quality guidance.

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

Captured orchestration traces now keep the raw `orchestration-trace.json` for
audit while condition and progress reports carry compact summary fields:
`orchestration_event_types`, `orchestration_heartbeat_events`,
`orchestration_heartbeat_windows`, `orchestration_heartbeat_max_gap_ms`,
`orchestration_denial_events`, `orchestration_denial_paths`, and
`orchestration_denial_messages`. Use those summary fields for paired run
analysis; open the raw trace only when the summary points at a suspicious event
type, path, or heartbeat gap.


Lifecycle troubleshooting checklist:

- `SessionRegistered` alone is insufficient. Require subsequent nested
  `SessionHeartbeat` events and `ActivityFinalized` before treating the
  stateful-on condition as a valid rollout.
- If `codex-command.json` or `results.jsonl` shows `omp exited 1` with runtime
  under a few seconds, empty `patch.diff`, and zero subagent spawns, inspect
  provider API-key propagation before analyzing model behavior.
- If Docker still shows a runtime container for the instance after the run has
  finished or failed, remove that container before rerunning the same instance.
  Leftover `sleep infinity` runtime containers indicate cleanup did not
  complete cleanly.
- If the benchmark launcher must return before the run finishes, make sure the
  long-running `stateful-bench` process is owned by a durable process manager.
  Some restricted command wrappers reap daemonized/background children on exit;
  the symptom is a pid file or empty launch log with no durable run directory.

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
5. For contaminated instances, inspect
   `codex-command.json.benchmark_contamination`.

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
- whether any rows have `finish_reason: "benchmark-contamination"`; these rows
  are invalid/non-scored rollouts, not model-quality failures. Record the
  contamination `kind` (`upstream-worktree` or `upstream-source-access`) and the
  recorded `path`, `line`, or `pattern` when present.

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
