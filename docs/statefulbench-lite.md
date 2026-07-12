# StatefulBench benchmarks

StatefulBench has two separate shared-checkout efficiency workloads:

- **Synthetic smoke (`statefulbench_lite.py`)** is the inexpensive five-task
  `taskset` workload. Use it to prove that OMP, credentials, a Stateful binary,
  and the three-arm launcher work together before spending credits on the
  corpus.
- **Real-world (`statefulbench_realworld.py`)** runs 100 issue-derived coding
  tasks: ten tasks in each of ten pinned Python repositories. It is the
  benchmark for reporting.

Both compare the same arms:

- `sequential` runs task agents one at a time in specification order;
- `parallel-off` runs task agents concurrently without Stateful coordination;
- `parallel-on` runs task agents concurrently with an arm-local Stateful
  enforcement server.

Each arm, repository, and trial receives a fresh checkout. The final reviewer
starts only after every task agent in its arm has been reaped. Do not compare
arms that use different models, thinking settings, task selections, trial
counts, or corpus revisions.

## Synthetic smoke

The generated `taskset` repository has two to five deliberately incomplete
tasks (`slug`, `stats`, `rle`, `roman`, and `intervals`) plus one final
integration reviewer. Generation itself consumes no model credits:

```sh
python3 crates/stateful-bench/scripts/statefulbench_lite.py generate \
  --dest tmp/lite-seed --tasks 5
```

The generated test suite is intentionally RED until agents implement the
tasks. A cheap live launcher smoke uses one non-Stateful arm, two task agents,
and one final agent:

```sh
python3 crates/stateful-bench/scripts/statefulbench_lite.py run \
  --arms parallel-off --tasks 2 --timeout-s 600 \
  --out tmp/statefulbench-lite/smoke
```

Run the synthetic three-arm comparison only when intended; the default command
selects all three arms, five tasks, and one trial:

```sh
python3 crates/stateful-bench/scripts/statefulbench_lite.py run
```

The supported synthetic command shapes are:

```text
statefulbench_lite.py generate --dest DEST [--tasks TASKS]
statefulbench_lite.py run [--arms ARMS] [--tasks TASKS] [--trials TRIALS]
  [--model MODEL] [--thinking THINKING] [--omp-bin OMP_BIN]
  [--stateful-binary STATEFUL_BINARY] [--timeout-s TIMEOUT_S] [--out OUT]
```

`--tasks` must be from 2 through 5. `parallel-on` requires a resolvable
Stateful binary. Synthetic output is rooted at the requested `--out` path:

```text
<out>/
  summary.json
  <arm>/trial-<n>/
    results.json
    workspace/
    prompts/
    logs/<agent>.stdout.log
    logs/<agent>.stderr.log
```

`parallel-on` also writes its arm-local server logs under
`logs/stateful-server.{stdout,stderr}.log`.

### Synthetic clearance

The implemented synthetic `cleared` predicate requires all selected task
agents and the final agent to exit with code 0 without timing out, no harness
error, **and** `post_suite_ok: true`. The latter is the generated `unittest`
suite rerun by the harness after the final agent exits. It is a workload smoke,
not a behavioral-quality score.

Each synthetic `results.json` includes the arm/trial identity, clearance and
error state, arm/task/final wall times, aggregate tokens and tool calls,
`post_suite_ok`, and per-agent exit, timeout, wall-time, token, and tool-call
records. `<out>/summary.json` records model, thinking, task count, trial
count, generation time, and all arm records. The command prints one table row
per arm/trial.

## Real-world corpus: freeze, qualify, then run

The checked-in corpus is an issue-derived, pinned dataset at
`datasets/statefulbench-realworld/manifest.json`. Its ten repositories each
have five bug tasks and five feature tasks: 100 coding tasks total. The
manifest records requested and canonical URLs, commit and archive hashes,
Python version, setup and upstream-suite commands, and the per-repository
corpus path. `metadata.exclusions`, when present, maps an exact suite
`--deselect` or `--ignore` argument to its audited reason; validation rejects
notes that do not match the suite. Task records retain their issue-source URLs
and frozen-source hashes.

For django-storages, four exact `tests/test_s3.py::S3StorageTests` nodes are
deselected because their upstream expectations are superseded by the corpus
contracts for legacy credential alias removal, the cache/pickle model, token
alias removal, and unsigned URL endpoint behavior. This is an auditable
node-level list, not a broad test filter.

### Freeze

Freeze is a reviewed dataset-authoring boundary, not a runner subcommand:
commit the manifest, issue snapshots, task prompts, evaluators, and reference
patches together before qualification or inference.
`statefulbench_realworld.py --help` exposes only `qualify` and `run`; do not
invent a `freeze` invocation or mutate a manifest after it has been qualified.
The runner downloads source archives by pinned HTTPS URL and accepts only
bytes matching the manifest's SHA-256.

### Qualify all ten pinned repositories

Qualification has no OMP agents. It downloads missing archives into the cache,
creates fresh workspaces, and verifies each base, reference task patch,
integrated reference patch, overlap graph, evaluator, and upstream suite:

```sh
python3 crates/stateful-bench/scripts/statefulbench_realworld.py qualify \
  --manifest datasets/statefulbench-realworld/manifest.json \
  --cache tmp/statefulbench-realworld/cache
```

To qualify one named repository while preparing or repairing the corpus, add
`--repo <repository-key>`; the option may be repeated. The CLI does not
automatically gate `run` on qualification, so complete qualification
successfully before a live run. Qualification artifacts live at:

```text
<cache>/
  <archive-sha256>.tar.gz
  qualification/<repository>/artifacts/
    <number>.stdout.log
    <number>.stderr.log
```

The content-addressed archive cache is reusable across qualification and runs.
Delete it only to deliberately redownload and re-verify archives.

### Run the full benchmark

Run every repository and every arm by omitting `--repos` and `--arms`:

```sh
python3 crates/stateful-bench/scripts/statefulbench_realworld.py run \
  --manifest datasets/statefulbench-realworld/manifest.json \
  --cache tmp/statefulbench-realworld/cache \
  --out tmp/statefulbench-realworld/run-$(date -u +%Y%m%d-%H%M%S)
```

The real-world `run` command requires `--manifest`, `--cache`, and `--out`.
Its other supported options are:

```text
[--repos REPOS] [--arms ARMS] [--trials TRIALS] [--model MODEL]
[--thinking THINKING] [--omp-bin OMP_BIN]
[--stateful-binary STATEFUL_BINARY] [--timeout-s TIMEOUT_S]
```

`--repos` is a comma-separated set of manifest repository keys. The default
arms are `sequential,parallel-off,parallel-on`; `--trials` defaults to 1.
`parallel-on` requires a resolvable Stateful binary.

> **Cost warning:** one complete real-world trial launches
> $10 \times 3 \times (10 + 1) = 330$ OMP agent processes: ten repositories,
> three arms, ten task agents, and one final agent per arm. It consumes
> substantial model credits and time. Run the full command only on an explicit
> request after the synthetic smoke and full qualification succeed.

Real-world run output is:

```text
<out>/
  summary.json
  <repository>/<arm>/trial-<n>/
    results.json
    prompts/
    logs/<agent>.stdout.log
    logs/<agent>.stderr.log
    logs/stateful-server.{stdout,stderr}.log   # parallel-on only
    artifacts/<number>.stdout.log
    artifacts/<number>.stderr.log
```

The `artifacts/` logs cover setup, evaluator, and upstream-suite commands.
Agent and server logs are separate under `logs/`. The run workspace is a
temporary directory and is removed after the arm; use these retained logs and
the result record for diagnosis. `<out>/summary.json` preserves the model,
thinking setting, selected pinned repositories, trial count, generation time,
per-arm rows, and per-repository/arm aggregates. The command also prints a
row for every repository, arm, and trial.

### Evaluator isolation and real-world clearance

Task agents receive only their own prompt and the pinned shared checkout.
Reference patches and evaluators are absent during task execution. After all
ten task agents finish, the harness copies the canonical evaluator files into
`.statefulbench-evaluators`, makes the copies read-only, gives the final agent
all task specifications, and then independently runs every canonical
evaluator and the pinned upstream suite. It verifies the canonical evaluator
source hashes before and after final-agent execution; a changed source is a
harness error.

Real-world `cleared` is stricter than the synthetic predicate: no arm-level
error; all ten task agents and the final agent exit 0 without timing out; all
canonical evaluators pass; and the pinned upstream suite passes. The result
records `evaluators_ok`, `upstream_suite_ok`, and each evaluator result in
addition to the shared timing, token, tool-call, and agent fields.

## Interpretation boundary

`cleared`, evaluator results, upstream-suite results, tokens, tool calls, and
wall time are run records and efficiency measurements. Report arm/repository
rows and aggregates descriptively. A smoke, a single trial, or an aggregate
must not be presented as evidence of behavioral quality, causal effect,
safety, or statistical superiority. The frozen full task-graph StatefulBench
protocol is cancelled; these synthetic and real-world workflows are the
maintained benchmarks.
