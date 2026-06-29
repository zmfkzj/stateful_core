---
name: stateful-command-policy
description: Detailed procedure for using Stateful MCP coordination, claims, sandbox profiles, and hook-denial recovery after a Stateful rule or denial says this policy applies
---

# Stateful Command Policy

This is the procedural manual for Stateful hooks. Rules decide when the skill applies; this skill tells agents which MCP tool, sandbox profile, or recovery path to use without probing through raw Bash/eval or widening write scope.

## Role Split With Rules

- Rules own activation: short always-apply or TTSR guidance tells agents when to consult this skill.
- This skill owns procedure: exact MCP flow, sandbox profile selection, denial recovery, and edge cases.
- Hooks own enforcement: hook allow means proceed; hook deny or unavailable means stop and choose the documented alternative.

## Default Write Flow

1. For planning, inspect context once only when active coordination may affect the plan. When target paths are known, `state_context_render(mode="brief", resource="<target>")` is optional planning/manual inspection; use broad `state_current_read` only before targets are known or when assigning parallel work.
2. Declare the task file set with `state_reservation_declare(purpose=<task purpose>, files_planned=[...])`; the active `reservation_id` is the write authorization batch boundary.
3. Acquire exact same-reservation file claims with `state_claim_acquire(reservation_id=<reservation_id>, paths=[...])` before native edits, deletes, moves, renames, or repo-relative command writes.
4. Keep paths narrow. Directory claims authorize only `write_directory`; exact file writes still need exact file reservation scope and a same-reservation claim.
5. Re-read files immediately before native edits. Native edits and write-target sandbox writes release authorized claims after the transaction; reacquire before another write under the active reservation.
6. For hook denials, follow the denial's next action or `denial-recovery.md`; do not call `state_context_render` unless you need to revise the plan.
7. Use canonical Stateful MCP tool names in guidance: `state_context_render`, `state_current_read`, `state_session_register`, `state_reservation_declare`, `state_claim_acquire`, `state_reservation_request`, `state_notifications_poll`, `state_resume_next`, and `state_reservation_claim`. If the active tool list exposes only runtime-specific tool names, call the exact shown equivalent. Runtime-specific wrappers are aliases, not the API.

## Support Files

Read only the focused support file needed for the current denial or command shape:

- `omp-tools.md`: OMP tool mapping, built-in Bash sandbox/process guidance, skill URI expansion, SSE/lazy edit and write resume behavior, and DeNovo OMP notes.
- `sandbox-tools.md`: sandbox profile selection, command examples, git/GitHub/build/external profiles, tmp retention, and raw Bash/eval avoidance.
- `denial-recovery.md`: missing reservation, missing claim, claim conflict, wait queue, hook denial, raw Bash/eval block, and no-supported-path recovery.
- `subagent-write-recovery.md`: how native Codex/OMP subagents recover write authorization without shell session repair.

## Skill Authoring Boundary

This skill may teach agents how to use `stateful` correctly. Do not add benchmark-specific success strategies, direct orchestration plans, role assignments, or mandates for domain task strategy. Work-allocation guidance belongs here only when necessary to avoid Stateful coordination errors such as overlapping claims, shared scratch paths, stale reservations, or same-reservation claim denials.

Benchmark prompts must not be injected through this skill except for instructions explicitly part of testing concurrent-work behavior. The skill must not try to improve benchmark patch quality by telling agents which domain roles to spawn, files to assign, or non-stateful task strategy to follow.
