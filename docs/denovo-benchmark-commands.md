# DeNovoSWE Benchmark Commands

Last updated: 2026-06-19.

Use this file to relaunch the 3-way sharded Codex DeNovoSWE benchmark without
reconstructing the command line.

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
- The no-state nested Codex config must not inherit host `mcp_servers.stateful`
  or hooks. This is covered by
  `codex_pair_agent_seeds_and_cleans_nested_auth`.
- The DeNovo Codex patch harvester excludes Codex/stateful runtime dirs,
  sandbox scratch dirs such as `tmp/**` and `.stateful-tmp/**`, common Python
  cache/coverage dirs, `target/**`, and root `clean.sh`. This is covered by
  `denovo_codex_agent_git_diff_excludes_stateful_runtime_artifacts`.
- Use absolute paths for `--aweagent-root`, `--python`, `--data-file`,
  `--config`, `REPO_ROOT`, `STATEFUL_BIN`, `TMUX`, and `TMUX_SOCKET`.
- Change run IDs and `--codex-home-root` directories before reusing commands.

## Command Variables

Set these values before reusing the commands:

```bash
REPO_ROOT=/absolute/path/to/stateful_core
STATEFUL_BIN=/absolute/path/to/stateful
TMUX=/absolute/path/to/tmux
TMUX_SOCKET=/absolute/path/to/tmux/socket
RUN_ID=rNN-denovo
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

Stop sessions:

```bash
"$STATEFUL_BIN" sandbox run-nested-codex-benchmark --purpose "stop $RUN_ID shard a tmux session" --write-dir target --codex-home-root "target/nested-codex-homes/$RUN_ID-tmux-stop-a" --docker-socket "$TMUX_SOCKET" --timeout-seconds 20 --command "$TMUX -S $TMUX_SOCKET kill-session -t $RUN_ID-shard-a"

"$STATEFUL_BIN" sandbox run-nested-codex-benchmark --purpose "stop $RUN_ID shard b tmux session" --write-dir target --codex-home-root "target/nested-codex-homes/$RUN_ID-tmux-stop-b" --docker-socket "$TMUX_SOCKET" --timeout-seconds 20 --command "$TMUX -S $TMUX_SOCKET kill-session -t $RUN_ID-shard-b"

"$STATEFUL_BIN" sandbox run-nested-codex-benchmark --purpose "stop $RUN_ID shard c tmux session" --write-dir target --codex-home-root "target/nested-codex-homes/$RUN_ID-tmux-stop-c" --docker-socket "$TMUX_SOCKET" --timeout-seconds 20 --command "$TMUX -S $TMUX_SOCKET kill-session -t $RUN_ID-shard-c"
```
