# statefulbench-lite

`statefulbench-lite` compares the efficiency of three execution arms: `sequential`, `parallel-off`, and `parallel-on`. It measures whether parallel execution and Stateful coordination change agent cost or elapsed time; it is not a behavioral-quality benchmark.

Every arm/trial creates a fresh repository checkout shared only by the agents inside that arm. It contains five independent coding tasks (`slug`, `stats`, `rle`, `roman`, and `intervals`). The task agents run in spec order for `sequential`, or concurrently for the two parallel arms. One final integration reviewer then runs the full suite and fixes failures.

## Completion and results

An arm is `cleared` only when every task agent and the final reviewer exit with code 0 without timing out **and** the post-final full suite passes. `post_suite_ok: false` is an arm failure and therefore forces `cleared: false`; it remains a smoke gate rather than a behavioral-quality score.

Each agent record contains:

```json
{
  "agent_id": "task-slug",
  "kind": "task",
  "exit_code": 0,
  "timed_out": false,
  "wall_time_s": 0,
  "total_tokens": 0,
  "tool_calls": 0
}
```

Each arm/trial `results.json` record contains:

```json
{
  "arm": "parallel-on",
  "trial": 1,
  "cleared": true,
  "error": null,
  "arm_wall_time_s": 0,
  "tasks_wall_time_s": 0,
  "final_wall_time_s": 0,
  "total_tokens": 0,
  "total_tool_calls": 0,
  "post_suite_ok": true,
  "agents": []
}
```

`summary.json` contains `model`, `thinking`, `tasks`, `trials`, `generated_at`, and `arms`. The harness also prints one markdown-table row per arm and trial with `cleared`, `arm_wall_time_s`, `total_tokens`, `total_tool_calls`, and `post_suite_ok`. Tool calls come from OMP's explicit usage count when present, otherwise from `tool_execution_start` events. The `parallel-on` arm starts one enforcement server per arm/trial and gives every agent in that arm the same server URL and token.

## Commands

Generate a deterministic seed workspace (five tasks by default):

```sh
python3 crates/stateful-bench/scripts/statefulbench_lite.py generate --dest tmp/lite-seed
```

The command form is:

```sh
python3 crates/stateful-bench/scripts/statefulbench_lite.py generate --dest <dir> [--tasks 5]
```

Run all three arms with the defaults:

```sh
python3 crates/stateful-bench/scripts/statefulbench_lite.py run
```

This default full three-arm run consumes live model credits, so run it only on an explicit request. The run command accepts:

```sh
python3 crates/stateful-bench/scripts/statefulbench_lite.py run [--arms sequential,parallel-off,parallel-on] [--tasks 5] [--trials 1] [--model openai-codex/gpt-5.6-terra] [--thinking high] [--omp-bin omp] [--stateful-binary <shutil.which("stateful")>] [--timeout-s 900] [--out tmp/statefulbench-lite/<UTC yyyymmdd-HHMMSS>]
```

The frozen full task-graph StatefulBench protocol is cancelled. `statefulbench-lite` is the maintained replacement.
