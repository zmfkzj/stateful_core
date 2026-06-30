# OMP background bash polling

## Goal

Make OMP background sandbox commands observable and collectable by the agent before it ends the turn. A background `sandbox_bash`, `ext_ro_bash`, `ext_rw_bash`, or `process_find` call should start quickly, then expose a pollable job result so the agent can monitor progress, detect failures, and report the final exit status without relying on next-turn message delivery.

## Current behavior

The generated OMP extension starts background sandbox commands in `startSandboxBackgroundTool`. With `async` omitted or `true`, the tool returns immediately with a `runId`. Stdout chunks and final stdout/stderr/exit status are sent with `pi.sendMessage(..., { triggerTurn: true, deliverAs: "nextTurn" })`.

That keeps the UI responsive, but it lets the assistant finish a turn while a sandbox command is still running. When the next user turn starts, buffered output from the previous turn can arrive all at once.

`async: false` remains useful for foreground commands, but it does not cover the intended workflow: start long-running work in the background, inspect progress while it runs, and collect the final result before handoff.

## Requirements

- Keep `async: false` foreground behavior unchanged.
- Keep existing sandbox profiles, external grant approval, timeout handling, and cancellation behavior unchanged.
- For `async: true` background runs, return a `runId` immediately and store job state in the live OMP extension session.
- Add a polling tool that lets the agent fetch status and output deltas by `runId`.
- Let the agent distinguish `running`, `done`, `failed`, and `not_found` states.
- Include final `exit_code`, stdout/stderr totals or deltas, error text, command label, start time, and finish time when available.
- Make the model-facing guidance say that background sandbox jobs must be polled to completion before final handoff.
- Avoid persistent storage, a daemon, or a server-side job API. The current background job lifetime is already live-session scoped.

## Design

Add an in-memory registry in the generated OMP extension:

```text
activeSandboxJobs: Map<runId, SandboxJob>
```

Each job records:

```text
runId
label              // sandbox_bash, ext_ro_bash, ext_rw_bash, process_find
command
commandLabel
status             // running, done, failed
startedAt
finishedAt
stdout
stderr
stdoutPollOffset
stderrPollOffset
exitCode
error
result
```

`startSandboxBackgroundTool` should create the job before spawning the process. Stdout chunks append to the job buffer as they arrive. When the process resolves, the final result updates the job with `done` or `failed`, `exitCode`, `stderr`, `error`, and `finishedAt`.

Register a new OMP tool:

```text
sandbox_job_poll
```

Parameters:

```text
run_id: string
wait_ms?: number
```

`wait_ms` defaults to a small value such as `0` or `250`. If the job is still running and no new output is available, the tool may wait up to that duration before returning. This is enough for monitoring loops without creating another long-running command path.

Return shape:

```text
status: running | done | failed | not_found
runId
label
command
startedAt
finishedAt
stdoutDelta
stderrDelta
stdout
stderr
exitCode
error
```

Deltas are cursor-based per job inside the extension. The first poll returns all accumulated output. Later polls return only output since the previous poll. The full stdout/stderr fields can be truncated with the existing sandbox output truncation helper so the final poll remains useful without unbounded memory growth.

Keep next-turn `pi.sendMessage` delivery only as a compatibility fallback if needed, but it should no longer be the primary result path for background sandbox commands. The primary path is explicit polling by `runId`.

## Agent workflow

Expected agent behavior:

```text
1. Call sandbox_bash/ext_ro_bash/ext_rw_bash/process_find with async:true.
2. Store the returned runId.
3. Call sandbox_job_poll(runId) while doing other work or when blocked on the command.
4. Brief progress from stdout/stderr deltas when useful.
5. Before final response, poll every started background job until it is done or failed.
6. Report the final exit code and relevant stdout/stderr.
```

If the agent wants a simple command with no progress monitoring, it should continue to use `async:false`.

## Error handling

- Unknown `run_id` returns `status: "not_found"` with `isError: true` only if that is consistent with existing tool conventions; otherwise return a normal result with a clear status.
- A sandbox process timeout is represented as `failed` with the existing timeout error text.
- A non-zero exit code is `done` with `exitCode != 0` unless the existing result builder marks it as an error. The agent can decide whether the command failure blocks the task.
- If the OMP extension process restarts, old run IDs are lost and polling returns `not_found`. This matches the current live-session nature of background messages.

## Testing

Add or update generated-extension tests to assert that the installed extension includes:

- `sandbox_job_poll` tool registration.
- An active sandbox job registry.
- Background tools storing jobs by `runId`.
- Poll responses containing `running`, `done`, and `not_found` paths.
- Cursor/delta handling for stdout and stderr.
- Existing `async:false` awaited behavior remains present.
- Existing external approval and timeout code paths remain present.

Prefer focused string/integration checks in `crates/stateful-cli/tests/install_global.rs`, matching the current test style for generated OMP extension contents.

## Documentation

Update OMP-facing documentation and skill guidance:

- `docs/architecture.md`
- `docs/core-concept.md`
- `docs/current-state-coordination.md`
- `docs/implementation-contract.md`
- `crates/stateful-cli/assets/stateful-command-policy/omp-tools.md`

The docs should say that background sandbox tools return a `runId`, and agents should use `sandbox_job_poll` to monitor and collect final results before ending the turn. `async:false` remains the direct foreground option.
