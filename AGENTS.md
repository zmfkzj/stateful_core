# Maintainer Agent Workflow

This file describes the repository maintainer workflow used for local release
work. External contributors do not need to follow it, and it is not a general
agent policy for downstream users.

## User Decisions

When a workflow, skill, or "superpower" offers choices, do not pick a default,
recommendation, approval, or next step on the user's behalf. Ask one question at
a time and wait for the user's explicit choice before proceeding.

After completing an implementation or behavior change, maintainers should run
follow-up checks before final handoff.

## Post-Change Subagents

When available, use subagents for these independent checks and updates:

- Update `SKILL.md` files so agent-facing guidance matches the implementation.
- Update `README.md` so user-facing setup and behavior descriptions match the implementation.
- Update other relevant documentation, including docs, plans, specs, and embedded templates.
- Check for implementation-documentation conflicts across code, tests, skills, README content, and generated install-time docs.

## Completion Flow

After follow-up work is complete:

- Review and integrate each subagent result.
- Re-run targeted verification for the changed behavior when the environment allows it.
- When a turn ends with file modifications, commit the finished changes from that turn.
- Push that commit after it is created.
- Stage only the files changed for the current turn; do not include unrelated dirty work.
