---
alwaysApply: true
---

# Stateful Required

Stateful hooks are the authority for repository writes and command-shaped work. This rule is deliberately short: before it applies, read `skill://stateful-command-policy` for the V2 procedure and exact active tool names.

## Trigger

Before any of these actions, read and follow `skill://stateful-command-policy`:

- running shell, eval, build, test, git, GitHub PR, benchmark, generator, formatter, or repo-external commands;
- creating, editing, deleting, moving, renaming, or overwriting repository files;
- responding to a Stateful warning or denial, including `missing_reservation`, `missing_claim`, `coordination_conflict`, `missing_read_provenance`, `stale_observation`, `write_fence_conflict`, `unknown_write_outcome`, or `unreconciled_human_write`;
- authorizing OMP yolo/write approval, external sandbox approval, or subagent write recovery.

## V2 Defaults And Boundaries

- Awareness is the default coordination mode. Start normally with `stateful server` or `stateful server start`; use `--coordination-mode enforcement` only as an explicit opt-in.
- Presence, fresh complete exact reads, rendered delivery/ACK context, and useful handoffs are the coordination center. The canonical record is the indefinite `stateful.v2` journal; native tools and hooks use `/v2/**`, not direct model-authored HTTP requests.
- Warnings in awareness still require judgment: inspect relevant context, reread before writing, and narrow overlap. Explicit enforcement can deny missing reservations, claims, and fresh reads.
- In both modes, stop for invalid targets, unknown write outcomes, stale exact evidence, active write fences, and unreconciled high-confidence human writes. Follow the response's next action; do not retry variants.
- Use only native Stateful tools shown in the active tool list. If a needed tool is absent, use the skill's OMP native edit/write or lazy-resume path; never repair Stateful session state through Bash/eval.
- OMP built-in Bash is permitted only for strict trusted `stateful sandbox run ...` and `stateful sandbox process find ...` commands. Do not wrap them in arbitrary Bash/eval.
