---
alwaysApply: true
---

# Stateful Required

Stateful hooks are the authority for repository writes and command-shaped work. The model-facing rule is deliberately short; read `skill://stateful-command-policy` for the detailed procedure whenever this rule applies.

## Trigger

Before any of these actions, read and follow `skill://stateful-command-policy`:

- running shell, eval, build, test, git, GitHub PR, benchmark, generator, formatter, or repo-external commands;
- creating, editing, deleting, moving, renaming, or overwriting repository files;
- responding to `missing_reservation`, `missing_claim`, `claim_conflict`, same-reservation claim denial, or any Stateful hook denial;
- authorizing OMP yolo/write approval, external sandbox approval, or subagent write recovery.

## Non-negotiable defaults

- Use `state_context_render(mode="brief", resource="<target>")` only for planning/manual inspection when active coordination may affect the plan. Do not call it as routine denial recovery; follow the denial's next action instead.
- OMP native `edit` and `write` may auto-declare exact tool-visible file scope and acquire same-reservation claims when no explicit `reservation_id` is supplied and the only denial is missing reservation/scope. Use explicit `state_reservation_declare(purpose=<task purpose>, files_planned=[...])` plus `state_claim_acquire(reservation_id=<reservation_id>, paths=[...])` for command-shaped writes, directory writes, multi-resource intent, queued conflict recovery, deletes, moves, renames, or whenever a specific reservation boundary matters.
- Use active Stateful native tool names for coordination; never repair session state through Bash or eval.
- In OMP, use built-in Bash only for strict trusted `stateful sandbox run ...` and `stateful sandbox process find ...` commands. Bare `stateful` is trusted only after session-start or per-tool preflight hash-verifies the first PATH `stateful` binary against the installed Stateful binary; commands using the installed absolute binary path remain trusted. External write/create/write-dir/socket/signal scope prompts for a scoped OMP UI grant unless `stateful.autoApprove: true`; auto-approval skips only the Stateful-owned UI prompt and does not bypass Stateful sandbox scope validation, hooks, reservation/claim checks, or grant limits. Repo-relative external write scopes require Stateful reservation and claims, and raw Bash/eval wrappers are never allowed.
- If a hook denies an action, read the denial and choose the documented Stateful alternative instead of retrying variants.
