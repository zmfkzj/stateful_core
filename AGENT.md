# Agent Workflow

After completing an implementation or behavior change, run follow-up work through subagents before final handoff.

## Post-Change Subagents

Use subagents for these independent checks and updates:

- Update `SKILL.md` files so agent-facing guidance matches the implementation.
- Update `README.md` so user-facing setup and behavior descriptions match the implementation.
- Update other relevant documentation, including docs, plans, specs, and embedded templates.
- Check for implementation-documentation conflicts across code, tests, skills, README content, and generated install-time docs.

## Completion Flow

After all subagent work is complete:

- Review and integrate each subagent result.
- Re-run targeted verification for the changed behavior when the environment allows it.
- Commit the finished changes.
- Push the commit to the active branch.
