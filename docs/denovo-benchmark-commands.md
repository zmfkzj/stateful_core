# DeNovoSWE Benchmark Commands

Last updated: 2026-06-20.

Use this file to relaunch the 3-way sharded Codex DeNovoSWE benchmark without
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
- `<absolute-stateful-binary>` must include the `codex-benchmark`
  feature.
- Use the official AweAgent DeNovoSWE recipe/task workflow: extract patches
  once, run the agent in batch mode, evaluate in a fresh container, and score
  by unit-test pass ratio.
- The no-state nested Codex config must not inherit host `mcp_servers.stateful`
  or hooks. This is covered by
  `codex_pair_agent_seeds_and_cleans_nested_auth`.
- The DeNovo Codex patch harvester excludes Codex/stateful runtime dirs,
  `.stateful-tmp/**`, common Python cache/coverage dirs, `target/**`, and root
  `clean.sh`. Repo `tmp/**` changes are harvested like other source-tree
  changes. This is covered by
  `denovo_codex_agent_git_diff_excludes_stateful_runtime_artifacts` and
  `denovo_codex_agent_harvests_repo_tmp_changes`.
- Use absolute paths for `--aweagent-root`, `--python`, `--data-file`,
  `--config`, `REPO_ROOT`, `STATEFUL_BIN`, `TMUX`, and `TMUX_SOCKET`.
- Change run IDs and `--codex-home-root` directories before reusing commands.
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
`RUN_ID` and fresh `--codex-home-root` directories. Aggregate only after all
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
STATEFUL_BIN=/absolute/path/to/stateful
TMUX=/absolute/path/to/tmux
TMUX_SOCKET=/absolute/path/to/tmux/socket
RUN_SERIES=rNN-denovo
TRIAL=1
RUN_ID=$RUN_SERIES-t$TRIAL
```

Shard launch scripts are expected at:

```text
$REPO_ROOT/.stateful_bench/denovo/run-control/run-$RUN_ID-shard-a.sh
$REPO_ROOT/.stateful_bench/denovo/run-control/run-$RUN_ID-shard-b.sh
$REPO_ROOT/.stateful_bench/denovo/run-control/run-$RUN_ID-shard-c.sh
```

## Start Shards In Existing tmux

These commands do not call `stateful external-run`. They use an existing tmux
server socket by exposing that Unix socket to the `run-nested-codex-benchmark`
wrapper. Pass the `STATEFUL_*` runtime variables into the new tmux session with
`tmux new-session -e`; otherwise the stateful condition will fail its runtime
preflight instead of silently producing empty patches.

```bash
"$STATEFUL_BIN" sandbox run-nested-codex-benchmark --purpose "start $RUN_ID shard a in existing tmux without external-run" --write-dir target --codex-home-root "target/nested-codex-homes/$RUN_ID-tmux-a" --docker-socket "$TMUX_SOCKET" --timeout-seconds 20 --command "$TMUX -S $TMUX_SOCKET new-session -d -s $RUN_ID-shard-a -e STATEFUL_SERVER_URL=\$STATEFUL_SERVER_URL -e STATEFUL_SERVER_TOKEN=\$STATEFUL_SERVER_TOKEN -e STATEFUL_NESTED_CODEX_HOME_ROOT=\$STATEFUL_NESTED_CODEX_HOME_ROOT /bin/zsh $REPO_ROOT/.stateful_bench/denovo/run-control/run-$RUN_ID-shard-a.sh"

"$STATEFUL_BIN" sandbox run-nested-codex-benchmark --purpose "start $RUN_ID shard b in existing tmux without external-run" --write-dir target --codex-home-root "target/nested-codex-homes/$RUN_ID-tmux-b" --docker-socket "$TMUX_SOCKET" --timeout-seconds 20 --command "$TMUX -S $TMUX_SOCKET new-session -d -s $RUN_ID-shard-b -e STATEFUL_SERVER_URL=\$STATEFUL_SERVER_URL -e STATEFUL_SERVER_TOKEN=\$STATEFUL_SERVER_TOKEN -e STATEFUL_NESTED_CODEX_HOME_ROOT=\$STATEFUL_NESTED_CODEX_HOME_ROOT /bin/zsh $REPO_ROOT/.stateful_bench/denovo/run-control/run-$RUN_ID-shard-b.sh"

"$STATEFUL_BIN" sandbox run-nested-codex-benchmark --purpose "start $RUN_ID shard c in existing tmux without external-run" --write-dir target --codex-home-root "target/nested-codex-homes/$RUN_ID-tmux-c" --docker-socket "$TMUX_SOCKET" --timeout-seconds 20 --command "$TMUX -S $TMUX_SOCKET new-session -d -s $RUN_ID-shard-c -e STATEFUL_SERVER_URL=\$STATEFUL_SERVER_URL -e STATEFUL_SERVER_TOKEN=\$STATEFUL_SERVER_TOKEN -e STATEFUL_NESTED_CODEX_HOME_ROOT=\$STATEFUL_NESTED_CODEX_HOME_ROOT /bin/zsh $REPO_ROOT/.stateful_bench/denovo/run-control/run-$RUN_ID-shard-c.sh"
```

List sessions:

```bash
"$STATEFUL_BIN" sandbox run-nested-codex-benchmark --purpose "list $RUN_ID tmux sessions without external-run" --write-dir target --codex-home-root "target/nested-codex-homes/$RUN_ID-tmux-list" --docker-socket "$TMUX_SOCKET" --timeout-seconds 10 --command "$TMUX -S $TMUX_SOCKET ls"
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
target/stateful-bench/denovo/runs/<run-id>/conditions/<condition>/codex-cli/instances/<instance-id>/eval-result.json
```

Stop sessions:

```bash
"$STATEFUL_BIN" sandbox run-nested-codex-benchmark --purpose "stop $RUN_ID shard a tmux session" --write-dir target --codex-home-root "target/nested-codex-homes/$RUN_ID-tmux-stop-a" --docker-socket "$TMUX_SOCKET" --timeout-seconds 20 --command "$TMUX -S $TMUX_SOCKET kill-session -t $RUN_ID-shard-a"

"$STATEFUL_BIN" sandbox run-nested-codex-benchmark --purpose "stop $RUN_ID shard b tmux session" --write-dir target --codex-home-root "target/nested-codex-homes/$RUN_ID-tmux-stop-b" --docker-socket "$TMUX_SOCKET" --timeout-seconds 20 --command "$TMUX -S $TMUX_SOCKET kill-session -t $RUN_ID-shard-b"

"$STATEFUL_BIN" sandbox run-nested-codex-benchmark --purpose "stop $RUN_ID shard c tmux session" --write-dir target --codex-home-root "target/nested-codex-homes/$RUN_ID-tmux-stop-c" --docker-socket "$TMUX_SOCKET" --timeout-seconds 20 --command "$TMUX -S $TMUX_SOCKET kill-session -t $RUN_ID-shard-c"
```
