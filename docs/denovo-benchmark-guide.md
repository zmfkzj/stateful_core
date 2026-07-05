# DeNovoSWE Benchmark Guide

Last updated: 2026-07-03.

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
OMP-backed runs also install a pre-tool source-pattern guard for both
`stateful:on` and `stateful:off`; Docker OMP runs add an HTTP proxy that denies
target GitHub/raw/patch/API hosts, including `CONNECT`, before the command reaches
the model runtime.

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

Use these official-style settings unless a run is explicitly labeled as a
compatibility or debug run. Pass the prompt version explicitly in command lines;
the CLI default remains `v1` for compatibility.

- `--mode batch` for scored runs.
- `--prompt-version v2`, the official recipe prompt with the finish gate.
- `--benchmark-temperature 1`.
- `--benchmark-model-context-window 256000`, matching the paper's 262,144 token
  evaluation context as closely as this adapter supports.
- `--benchmark-max-turns 500` for DeNovoSWE package reconstruction.
- `--eval-iters 1` for agent comparisons, unless specifically testing
  evaluator stability.
- `--max-concurrent 1` for official-style patch-quality comparisons. Coordination
  mode claims need an explicitly labeled forced-overlap behavior test with
  `--max-concurrent >= 2`; do not rely on incidental DeNovo same-file collisions.
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
  Bun-installed `omp`, the Linux `stateful` binary, and `bubblewrap`/`bwrap`.
  The Docker command builder grants `--cap-add SYS_ADMIN --security-opt
  seccomp=unconfined --security-opt apparmor=unconfined --security-opt
  systempaths=unconfined` so bwrap can run inside the agent container. Add
  `--agent-docker-image <image>`. `--agent-docker-sandbox off` only controls
  `STATEFUL_OMP_SANDBOX` for OMP's own nested sandbox; it is not the bwrap
  namespace fix. `--agent-docker-stateful-binary <path>` only needs to be set
  when the image's `stateful` binary is not at `/usr/local/bin/stateful`.
- A reduced `stateful:off/on,subagent:on` matrix with `--max-concurrent 6` is a
  subagent/concurrency behavior test. Keep the same 12-instance list, shard,
  model, prompt version, temperature, context window, max turns, evaluator
  settings, and Docker image across all three trials, and do not mix it with
  official-style `--max-concurrent 1` comparisons.

Historical runs may use `--prompt-version v1`; do not mix v1 and v2 results in
the same comparison table.

DeNovo code defaults write run outputs under `.stateful_bench/denovo/runs` and
patch extracts under `.stateful_bench/denovo/extracts`. Docker or host-mounted
scratch runs may override the run root to
`$REPO_ROOT/target/stateful_bench_runs/...`; older
`target/stateful-bench/denovo/...` paths are historical and should not be reused.

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

Stateful lifecycle enforcement belongs in hooks, extensions, native-tool policy,
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

For `subagent:on`, the generated DeNovo prompt appends only `orchestrate`.
`subagent:off` does not add a custom subagent prompt. OMP runs also unpack
bundled task agents into the isolated runtime home and enable
`features.multi_agent=true`. The adapter still enforces the minimum native
subagent spawn count for both Codex and OMP `subagent:on` runs. Treat that
prompt addition as a declared behavior-test condition axis; do not reuse it as
normal scored comparison policy or general patch-quality guidance.

### Coordination-Mode Arms

[ADR 0002](adr/0002-presence-first-not-lock-first.md) records the
presence-first product direction. Coordination-focused comparisons now have three
implemented arms:

```text
A: no-state     no Stateful server/extension rails
B: awareness    presence/context warnings, hard safety denials when checked, no queue/claim blocking
C: stateful     default enforcement with reservation/claim/queue blocking
```

Use the checked-in forced-overlap harness for coordination-mode behavior, not
incidental DeNovo same-file collisions:

```text
crates/stateful-bench/scripts/overlap_manifest_generator.py
crates/stateful-bench/scripts/overlap_omp_agent.py
crates/stateful-bench/scripts/overlap_harness.py
stateful-bench run --mode no-state|awareness|stateful
stateful-bench compare --awareness-run-dir <dir>
```

Generate one manifest with a fixed seed, then run all arms with the same
manifest, model, image/runtime, `--jobs`, timeout, and instance order. The
checked-in manifest generator creates `exact_file_overlap` pairs over `doc.txt`
so every arm sees the same forced same-file pressure. Do not wait for DeNovoSWE
to produce emergent same-file collisions; ordinary small-N DeNovo runs may have
too few overlaps to measure coordination behavior, and null results there can be
underpowered rather than dispositive.

The checked-in harness and comparison report directly measure only artifacts
they can observe: uncoordinated same-file collisions, lost edit events, denied
writes, coordinated blocks, authorization warnings, warned writes that were
applied, wait events, preserved/missing expected edits, false blocks, missed
conflicts, wall time, and token/tool overhead when the agent logs expose usage.
`manual_intervention_count` is read only if a harness result supplies it; the
checked-in overlap harness writes `0` and does not observe real human/manual
interventions. Duplicated-investigation time is not observable in this harness;
mark it omitted/N.A. rather than inventing a proxy.

Comparison reports label evidence kind. `synthetic_fixture` validates report
plumbing only. Product efficacy claims require `paired_agent_run` evidence from
the forced-overlap harness, enough paired-valid samples, and overhead reporting.
The checked-in [2026-07-03 forced-overlap result](benchmarks/2026-07-03-forced-overlap-three-arm.md)
is a three-arm plumbing/smoke result with no differentiated safety outcome, not
final efficacy evidence.

Fable5's review cautions against overgeneralizing Cursor's large-N convoy
evidence to small-N shared-checkout tests. Use Cursor as a warning against broad
waits and task-scale locks, not as proof that every enforcement claim is bad or
that every small-N null result validates the pivot.

The registered hypothesis is that arm B keeps arm C's safety metrics while
spending less waiting and complexity. If that holds, claim machinery shrinks to
thin safety fences; if arm C is meaningfully safer, the fences stay and the
result documents why.

## Success Criteria

Define the claim before reading the results:

- Improvement claim: the stateful condition must improve the primary
  score/success metric under fixed conditions.
- Non-inferiority or efficiency claim: the stateful condition must keep the
  primary quality metric comparable while improving an efficiency metric; report
  quality separately from efficiency.

Minimum evidence for either claim is three independent trials per condition over
the same instance set, model, prompt version, temperature, context window, max
turns, evaluator settings, runtime limits, and network/source policy. For small
samples, report every trial plus mean and variance or standard deviation.

## Efficiency Metrics

DeNovoSWE reports include agent subprocess time (`agent_running_time_ms`),
elapsed harness/adapter wall time (`running_time_ms`), token totals, uncached
token totals, subagent usage, and score-per-cost metrics. Treat
`score_per_agent_hour` as the primary time-efficiency metric when present.
Elapsed `score_per_hour` remains a compatibility/audit metric and uses
`running_time_ms`, which can include setup, evaluation, trace, patch harvesting,
and report overhead. Historical artifacts may omit agent-time fields; do not
convert elapsed wall time into agent-only time. `average_score` establishes
quality; timing, tokens, and score-per-cost metrics describe efficiency.

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
audit while condition and progress reports carry compact summary fields. Trace
capture requests `/v1/events?workspace_id=<id>&limit=100` when a workspace id is
known, writes workspace-local events, and records per-instance
`events_window_saturated` when the 100-event window is full. Report summary
fields include `orchestration_event_types`,
`orchestration_lifecycle_event_types`, `orchestration_heartbeat_events`,
`orchestration_heartbeat_windows`, `orchestration_heartbeat_max_gap_ms`,
`orchestration_denial_events`, `orchestration_denial_paths`, and
`orchestration_denial_messages`. Trace/result rows keep
`lifecycle_event_types` separate from the latest-window `event_types`; use the
separate lifecycle fields when checking run validity. Use those report summary
fields for paired run analysis; open the raw trace only when the summary points
at a suspicious event type, path, heartbeat gap, or saturated event window.


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
- If Docker reports `invalid mount config for type "bind": bind source path
  does not exist` for an `omp-homes/<instance>/home` path that exists on the
  host, the Docker daemon cannot see that host path. This is common with Colima
  when the output root is under `/private/tmp` or another unmounted host tree.
  Relaunch with `DENOVO_OUTPUT_ROOT` overriding the code default to a
  Docker-mounted scratch path such as
  `$REPO_ROOT/target/stateful_bench_runs/...` or another `/Users/...` path, then
  use fresh run IDs and OMP homes.

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
