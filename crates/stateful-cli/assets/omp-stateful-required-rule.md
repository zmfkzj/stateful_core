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

- Inspect current state first with `state_current_read` or `state_context_render`.
- Declare exact intended files with `state_reservation_declare` and acquire matching claims with `state_claim_acquire` before writes.
- Use active Stateful MCP tool names for coordination; never repair session state through Bash or eval.
- In OMP, use `sandbox_bash` for non-external sandbox profiles, `ext_ro_bash` for read-only `--fs external` commands with only `purpose` and `command` and no OMP UI confirmation, and `ext_rw_bash` for `--fs external` commands with at least one write target, create target, or write directory scope and an OMP UI confirmation prompt; repo-relative external write scopes require Stateful reservation and claims, and raw Bash/eval wrappers are never allowed.
- If a hook denies an action, read the denial and choose the documented Stateful alternative instead of retrying variants.
