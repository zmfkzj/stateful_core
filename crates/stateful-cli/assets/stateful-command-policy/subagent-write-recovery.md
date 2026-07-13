# Subagent Write Recovery

Native Codex or OMP subagents must recover write authorization in their own active native tool context. They must not repair identity through shell commands, environment overrides, or hook-state archaeology.

## Recovery Flow

When a subagent hits `apply_patch writes require ... same-reservation file claim`, `Target is outside active reservation scope`, `missing_reservation`, `missing_claim`, `claim_conflict`, or `state_reservation_declare cannot resolve active agent identity`:

1. Stop retrying command variants; denials are the API.
2. Re-read the exact target if it exists.
3. Use active Stateful native tools in the same subagent tool context. Codex
   contexts may call `state_session_register` when the denial asks for session
   registration. OMP contexts do not supply or repair identity: OMP derives the
   active Stateful `agent_id` from `ctx.sessionManager.getSessionId()`,
   producing the session-stable `omp-${sessionId}`. If `getSessionId()` is
   unavailable or invalid, OMP Stateful actions fail closed and the subagent
   should report the denial.
4. For simple OMP native `edit`/`write` with no explicit `reservation_id`, let the extension predeclare and claim the exact tool-visible file scope before first authorization.
5. For new files outside that simple path, reserve and claim every exact new file path, not only the parent directory.
6. Edit with native tools such as `apply_patch`/`edit`, or use `sandbox run --fs write-targets` with matching targets for command-shaped writes.
7. If another agent owns the claim, do not retry or steal it. Follow the wait queue when available; otherwise report the path, `blocking_agent_id`, and wait/reservation id to the parent.

If the active tool list exposes runtime-specific native Stateful names, use the exact shown equivalent. If no native Stateful coordination tool is visible, report the missing tool and denial. If the first recovery call returns `unsupported call`, stop and report the exact call name, active tool list if visible, and denial.

## Denial Report Template

When recovery is blocked, report:
- Missing tool or unsupported call, if any:
- Active tool list, if visible:
- Exact denial:
- Path:
- `blocking_agent_id`:
- `wait_id` or `reservation_id`:
- Target reread? yes/no:
- Parent next action:

## Parent-Agent Rules For Large Reconstruction

Before dispatching implementation subagents for large source-tree reconstruction, the parent must prove the write path:

1. Build a concrete file manifest from the failing task and similar successful artifacts.
2. Declare a reservation and acquire a claim on the first exact file.
3. Land a tiny native edit or command-shaped write as a smoke test.
4. Dispatch subagents only to disjoint source files and disjoint scratch directories.
5. Require each implementation subagent to prove its first exact-file native-tool write before receiving many files, or give narrow read-only research tasks instead.

The parent should not claim a broad source-tree directory or shared `tmp/upstream` directory and then ask subagents to write under it. Do not send subagents CLI `reservation declare/request/claim` recipes to repair their session. Avoid reconstructing many files through large heredocs, `tee`, interactive stdin, or command-length experiments.
