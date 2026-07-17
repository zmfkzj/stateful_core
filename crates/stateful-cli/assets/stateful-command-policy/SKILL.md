---
name: stateful-command-policy
description: Detailed V2 procedure for Stateful native coordination tools, sandbox profiles, and recovery after a Stateful rule, warning, or denial says this policy applies
---

# Stateful Command Policy

Use this policy for Stateful V2 coordination. It keeps agents aware of active work, fresh evidence, and usable handoffs; it does not replace the hooks that guard writes and command-shaped work.

## Role Split With Rules

- Rules own activation: short always-apply guidance tells agents when to consult this skill.
- This skill owns procedure: native tools, sandbox profile selection, context, and recovery.
- Hooks own the boundary: follow their warning or denial and its next action; do not probe raw Bash/eval or widen write scope.

## V2 Coordination Model

- Awareness is the default coordination mode. Start normally with `stateful server` or `stateful server start`; both default to awareness. It reports reservation, claim, and missing-freshness coordination issues as warnings, not permission to ignore them.
- Enforcement is opt-in only: start with `stateful server --coordination-mode enforcement` or `stateful server start --coordination-mode enforcement` when overlaps and missing fresh reads must deny.
- The product center is active presence, complete exact-read freshness, and handoff context. Use rendered context and its delivery/ACK state to understand active work; explicit handoffs are preferred, with the rendered fallback preserving unfinished-work context.
- The canonical record is the indefinite V2 event journal. Its `stateful.v2` envelope and `/v2/**` routes are runtime protocol, not a model-facing HTTP API: use the active native tools rather than constructing requests.
- Hard stops remain thin in both modes: invalid targets, unknown prior write outcomes, stale exact evidence, active write fences, and unreconciled high-confidence human writes. Reread/reconcile or wait as the denial requires.

## Default Flow

1. Inspect `state_context_render(mode="brief", resource="<target>")` only when active coordination affects planning or manual inspection. Reuse a result already inspected for the same resource; use `state_current_read` only before targets are known or when assigning parallel work.
2. Keep presence current when the active tool list exposes `state_session_register` or `state_session_heartbeat`. Before a write, reread the exact target completely so the hook can supply fresh base observations.
3. For simple repo-internal OMP native `edit` and `write`, rely on the write boundary: when no explicit `reservation_id` is supplied and the only issue is missing reservation/scope, the extension can declare the exact tool-visible file scope, acquire same-reservation claims, and retry authorization.
4. For command-shaped writes, directory writes, multi-resource intent, queued conflict recovery, deletes, moves, renames, or a specific boundary, declare and claim narrow scopes with active native tools. In awareness these are coordination signals; in explicit enforcement they can deny overlap. Directory claims authorize only `write_directory`; exact file actions need exact file scope and same-reservation claims.
5. Read the warning or denial before retrying. Freshness failures require rereading and reconciling the current contents. Native edit/write and write-target sandbox transactions release authorized claims; reacquire before another explicit write.
6. Finalize activity and preserve a useful handoff. Version, delivery, and ACK context is carried by rendered V2 context; cleanup counts alone are not a handoff.

## Active Native Tool Names

Call only names shown in the active tool list. The shipped canonical names are `state_session_register`, `state_session_heartbeat`, `state_reservation_declare`, `state_reservation_request`, `state_reservation_claim`, `state_reservation_cancel`, `state_claim_acquire`, `state_claim_release`, `state_activity_finalize`, `state_current_read`, `state_events_read`, `state_context_render`, `state_reconcile_ack`, `state_notifications_poll`, and `state_resume_next`. Their exact dotted equivalents are `state.session.register`, `state.session.heartbeat`, `state.reservation.declare`, `state.reservation.request`, `state.reservation.claim`, `state.reservation.cancel`, `state.claim.acquire`, `state.claim.release`, `state.activity.finalize`, `state.current.read`, `state.events.read`, `state.context.render`, `state.reconcile.ack`, `state.notifications.poll`, and `state.resume.next`; an MCP-prefixed active name is also valid. If no applicable native tool is exposed, use the documented OMP native edit/write or lazy-resume path, not invented tools or manual session repair.

## Support Files

Read only the focused support file needed for the current denial or command shape:

- `omp-tools.md`: OMP mapping, built-in Bash sandbox/process guidance, skill URI expansion, SSE/lazy edit and write resume behavior, and DeNovo OMP notes.
- `sandbox-tools.md`: sandbox profile selection, command examples, git/GitHub/build/external profiles, tmp retention, and raw Bash/eval avoidance.
- `denial-recovery.md`: warnings, hard stops, freshness, conflicts, wait queues, and no-supported-path recovery.
- `subagent-write-recovery.md`: native Codex/OMP subagent recovery without shell session repair.

## Skill Authoring Boundary

This skill may teach agents how to use `stateful` correctly. Do not add benchmark-specific success strategies, direct orchestration plans, role assignments, or mandates for domain task strategy. Work-allocation guidance belongs here only when necessary to avoid Stateful coordination errors such as overlapping claims, shared scratch paths, stale reservations, or same-reservation claim denials.

Benchmark prompts must not be injected through this skill except for instructions explicitly part of testing concurrent-work behavior. The skill must not try to improve benchmark patch quality by telling agents which domain roles to spawn, files to assign, or non-stateful task strategy to follow.
