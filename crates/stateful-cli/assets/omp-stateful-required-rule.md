---
alwaysApply: true
---

# Stateful Required

Stateful hooks are the authority for repository writes and command-shaped work. The model-facing rule is deliberately short; read `skill://stateful-command-policy` for the detailed procedure whenever this rule applies.

## Trigger

Before any of these actions, read and follow `skill://stateful-command-policy`:

- running shell, eval, build, test, git, GitHub PR, benchmark, generator, formatter, or repo-external commands;
- creating, editing, deleting, moving, renaming, or overwriting repository files;
- responding to `missing_reservation`, `missing_claim`, `claim_conflict`, same-session claim denial, or any Stateful hook denial;
- authorizing OMP yolo/write approval, external sandbox approval, or subagent write recovery.

## Non-negotiable defaults

- Inspect current state first. When target paths are known, prefer `state_context_render(mode="brief", resource="<target>")`; use broad `state_current_read` only before targets are known or when a denial lacks a path.
- Declare exact intended files with `state_reservation_declare(files_planned=[...])` and acquire matching claims with `state_claim_acquire(paths=[...])` before writes.
- Use active Stateful MCP tool names for coordination; never repair session state through Bash or eval.
- In OMP, use `process_find` for process inspection. Use `sandbox_bash` for non-external sandbox profiles and `ext_ro_bash` for read-only `--fs external` commands with only `purpose` and `command` and no OMP UI confirmation. `ext_rw_bash` prompts for a scoped OMP UI grant unless `stateful.autoApprove: true` or per-call `auto_approve: true` is set; auto-approval skips only the Stateful-owned UI prompt and does not bypass Stateful sandbox scope validation, hooks, reservation/claim checks, or grant limits. Repo-relative external write scopes require Stateful reservation and claims, and raw Bash/eval wrappers are never allowed.
- If a hook denies an action, read the denial and choose the documented Stateful alternative instead of retrying variants.
