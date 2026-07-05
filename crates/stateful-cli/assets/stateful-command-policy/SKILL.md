---
name: stateful-command-policy
description: Detailed procedure for using Stateful native coordination tools, claims, sandbox profiles, and hook-denial recovery after a Stateful rule or denial says this policy applies
---

# Stateful Command Policy

This is the procedural manual for Stateful hooks. Rules decide when the skill applies; this skill tells agents which native Stateful tool, sandbox profile, or recovery path to use without probing through raw Bash/eval or widening write scope.

## Role Split With Rules

- Rules own activation: short always-apply or TTSR guidance tells agents when to consult this skill.
- This skill owns procedure: exact native-tool flow, sandbox profile selection, denial recovery, and edge cases.
- Hooks own enforcement: hook allow means proceed; hook deny or unavailable means stop and choose the documented alternative.

## Coordination Modes

- Run or start the server in enforcement mode by default: `stateful server --coordination-mode enforcement` or `stateful server start --coordination-mode enforcement`.
- `--coordination-mode awareness` changes reservation/claim/conflict denials into warnings for coordination practice, not permission to ignore coordination. Review rendered context and reread the target before writing.
- `unreconciled_human_write` still requires human-change reconciliation before overwrite in either mode.

## Default Write Flow

1. For planning, inspect context once only when active coordination may affect the plan. When target paths are known, `state_context_render(mode="brief", resource="<target>")` is optional planning/manual inspection; use broad `state_current_read` only before targets are known or when assigning parallel work.
2. For simple repo-internal OMP native `edit` and `write`, rely on the write boundary: if no explicit `reservation_id` is supplied and the only denial is missing reservation/scope, the OMP extension declares the exact tool-visible file scope, acquires same-reservation claims, and retries authorization. Repo-external native OMP `edit`/`write` file targets use the scoped external grant path; it auto-approves by default through `stateful.autoApprove: true` and prompts only when `stateful.autoApprove: false` is configured.
3. When the active tool list exposes `state_reservation_declare` and `state_claim_acquire`, use them for command-shaped writes, directory writes, multi-resource intent, deletes/moves/renames, or whenever a specific reservation boundary matters. For queued OMP lazy edit/write recovery, use `lazy_edit_resume` or `lazy_write_resume`; helpers wait/poll for the saved `wait_id` before claiming. Manual queued reservation claim requires exposed `state_reservation_claim` or an explicitly permitted CLI claim. If those tools are absent, do not invent them; use OMP native edit/write auto-declare, lazy resume, or an already-authorized `reservation_id` write boundary.
4. Keep paths narrow. Directory claims authorize only `write_directory`; exact file writes still need exact file reservation scope and a same-reservation claim.
5. Re-read files immediately before native edits so hooks can send fresh `base_observations`. Native edits and write-target sandbox writes release authorized claims after the transaction; reacquire before another explicit write under the active reservation. If authorization returns `missing_base_observation`, `stale_target_observation`, or `stale_claim_observation`, reread/reconcile and retry with a fresh write.
6. For hook denials, follow the denial's next action or `denial-recovery.md`; do not call `state_context_render` unless you need to revise the plan.
7. Use active Stateful native tool names only when they appear in the tool list. Canonical allowlisted names include `state_session_register`, `state_session_heartbeat`, `state_reservation_declare`, `state_reservation_request`, `state_reservation_claim`, `state_reservation_cancel`, `state_claim_acquire`, `state_claim_release`, `state_activity_finalize`, `state_current_read`, `state_events_read`, `state_context_render`, `state_reconcile_ack` / `state.reconcile.ack`, `state_notifications_poll`, and `state_resume_next`. Runtime-specific names may be MCP-prefixed or shown as `state.reconcile_ack` / `stateful_reconcile_ack`; copy the exact active name. If a tool is absent, choose the documented OMP lazy-resume/write-boundary path instead.

## Support Files

Read only the focused support file needed for the current denial or command shape:

- `omp-tools.md`: OMP tool mapping, built-in Bash sandbox/process guidance, skill URI expansion, SSE/lazy edit and write resume behavior, and DeNovo OMP notes.
- `sandbox-tools.md`: sandbox profile selection, command examples, git/GitHub/build/external profiles, tmp retention, and raw Bash/eval avoidance.
- `denial-recovery.md`: missing reservation, missing claim, freshness denials, claim conflict, wait queue, hook denial, raw Bash/eval block, and no-supported-path recovery.
- `subagent-write-recovery.md`: how native Codex/OMP subagents recover write authorization without shell session repair.

## Skill Authoring Boundary

This skill may teach agents how to use `stateful` correctly. Do not add benchmark-specific success strategies, direct orchestration plans, role assignments, or mandates for domain task strategy. Work-allocation guidance belongs here only when necessary to avoid Stateful coordination errors such as overlapping claims, shared scratch paths, stale reservations, or same-reservation claim denials.

Benchmark prompts must not be injected through this skill except for instructions explicitly part of testing concurrent-work behavior. The skill must not try to improve benchmark patch quality by telling agents which domain roles to spawn, files to assign, or non-stateful task strategy to follow.
