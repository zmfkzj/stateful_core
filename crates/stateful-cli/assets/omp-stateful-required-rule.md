---
alwaysApply: true
---

# Stateful Required

Stateful hooks are the authority for repository writes and command-shaped work. The model-facing rule is deliberately short; read `skill://stateful-command-policy` for the detailed procedure whenever this rule applies.

## Trigger

Before any of these actions, read and follow `skill://stateful-command-policy`:

- running shell, eval, build, test, git, GitHub PR, benchmark, generator, formatter, or repo-external commands;
- creating, editing, deleting, moving, renaming, or overwriting repository files;
- responding to `missing_intent`, `missing_lease`, `lease_conflict`, same-session lease denial, or any Stateful hook denial;
- authorizing OMP yolo/write approval, external sandbox approval, or subagent write recovery.

## Non-negotiable defaults

- Inspect current state first with `state_current_read` or `state_context_render`.
- Declare exact intended files with `state_intent_declare` and acquire matching leases with `state_lease_acquire` before writes.
- Use active Stateful MCP tool names for coordination; never repair session state through Bash or eval.
- In OMP, use `sandbox_bash` for non-external sandbox profiles and `external_bash` for `--fs external`; no-target external read-only commands are allowed through `external_bash`, repo-relative external write scopes require Stateful intent and leases, and raw Bash/eval wrappers are never allowed.
- If a hook denies an action, read the denial and choose the documented Stateful alternative instead of retrying variants.
