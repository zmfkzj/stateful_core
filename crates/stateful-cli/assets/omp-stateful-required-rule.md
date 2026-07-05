---
alwaysApply: true
---

# Stateful Required

Stateful hooks are the authority for repository writes and command-shaped work. The model-facing rule is deliberately short; read `skill://stateful-command-policy` for the detailed procedure whenever this rule applies.

## Trigger

Before any of these actions, read and follow `skill://stateful-command-policy`:

- running shell, eval, build, test, git, GitHub PR, benchmark, generator, formatter, or repo-external commands;
- creating, editing, deleting, moving, renaming, or overwriting repository files;
- responding to `missing_reservation`, `missing_claim`, `claim_conflict`, `missing_base_observation`, `stale_target_observation`, `stale_claim_observation`, `unreconciled_human_write`, same-reservation claim denial, Stateful awareness warnings, or any Stateful hook denial;
- authorizing OMP yolo/write approval, external sandbox approval, or subagent write recovery.

## Non-negotiable defaults

- Use `state_context_render(mode="brief", resource="<target>")` only for planning/manual inspection when active coordination may affect the plan. Do not call it as routine denial recovery; follow the denial's next action instead.
- Server coordination mode is explicit: `stateful server --coordination-mode enforcement|awareness` and `stateful server start --coordination-mode enforcement|awareness`. Awareness mode turns reservation/claim/conflict denials into warnings; still review context, reread targets, and reconcile human writes before overwriting.
- OMP native `edit` and `write` may auto-declare exact tool-visible file scope and acquire same-reservation claims when no explicit `reservation_id` is supplied and the only denial is missing reservation/scope. Use explicit `state_reservation_declare(purpose=<task purpose>, files_planned=[...])` plus `state_claim_acquire(reservation_id=<reservation_id>, paths=[...])` for command-shaped writes, directory writes, multi-resource intent, deletes, moves, renames, or whenever a specific reservation boundary matters only when those tools appear in the active tool list. For queued OMP lazy edit/write recovery, `lazy_edit_resume` and `lazy_write_resume` wait/poll for the saved `wait_id` before claiming; manual queued reservation claim is only for exposed `state_reservation_claim` or explicitly permitted CLI claim. If they are absent, do not invent them; use OMP lazy resume, native edit/write auto-declare, or an already-authorized `reservation_id` write boundary.
- Use active Stateful native tool names for coordination only when they appear in the tool list; never repair session state through Bash or eval. Current allowlisted names include `state_session_register`, `state_session_heartbeat`, `state_reservation_declare`, `state_reservation_request`, `state_reservation_claim`, `state_reservation_cancel`, `state_claim_acquire`, `state_claim_release`, `state_activity_finalize`, `state_current_read`, `state_events_read`, `state_context_render`, `state_reconcile_ack` / `state.reconcile.ack`, `state_notifications_poll`, and `state_resume_next`; runtime-specific aliases may be MCP-prefixed or shown as `state.reconcile_ack` / `stateful_reconcile_ack`.
- In OMP, use built-in Bash only for strict trusted `stateful sandbox run ...` and `stateful sandbox process find ...` commands. Bare `stateful` is trusted only after session-start or per-tool preflight hash-verifies the first PATH `stateful` binary against the installed Stateful binary; commands using the installed absolute binary path remain trusted. External write/create/write-dir/socket/signal scope and repo-external native `edit`/`write` file targets auto-approve the scoped Stateful-owned OMP grant prompt by default through `stateful.autoApprove: true`; auto-approval skips only the Stateful-owned UI prompt and does not bypass Stateful sandbox scope validation, hooks, reservation/claim checks, or grant limits. Set `stateful.autoApprove: false` to require the prompt. Repo-relative external write scopes require Stateful reservation and claims, and raw Bash/eval wrappers are never allowed.
- For `unreconciled_human_write`, reread the file, summarize the human change, then call the active reconcile-ack tool with `reservation_id` or `stateful reconcile ack --reservation-id <reservation_id> --resource <path> --files-reread <path> --summary <text> --decision <adopt|reapply|ask_user|abandon>` before retrying only `adopt` or `reapply`.
- If a hook denies an action, read the denial and choose the documented Stateful alternative instead of retrying variants.
