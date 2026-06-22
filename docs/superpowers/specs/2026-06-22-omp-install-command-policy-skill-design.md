# OMP Install Command-Policy Skill Design

## Goal

`stateful install --agent omp --yes` should install the same `stateful-command-policy` skill that Codex install already installs, so OMP-launched agents receive the repo's stateful write and sandbox guidance.

## Current Behavior

- `stateful install --agent codex` plans and writes `skills/stateful-command-policy/SKILL.md` next to the Codex config path.
- `stateful install --agent omp` plans and writes the OMP config, extension, and MCP config under the OMP agent directory, but does not install a skill file.
- The canonical skill source already lives at `crates/stateful-cli/assets/stateful-command-policy/SKILL.md`.

## Chosen Approach

Reuse the existing command-policy skill asset for OMP install.

Implementation shape:

- Add an OMP skill path under the existing OMP agent directory: `default_omp_agent_dir(paths)/skills/stateful-command-policy/SKILL.md`.
- Include that path in `plan_omp_install` dry-run output.
- During `apply_omp_install`, create the parent directory and write the existing `STATEFUL_COMMAND_POLICY_SKILL` content.
- Keep Codex install behavior unchanged.

## Rejected Alternatives

- Separate OMP-specific skill: unnecessary duplication and likely drift.
- Install all skills during plain `stateful install --yes`: broader side effect; agent-specific files should remain tied to agent install flows.

## Tests

Add or extend `crates/stateful-cli/tests/install_global.rs` coverage:

- OMP dry-run plan includes `skills/stateful-command-policy/SKILL.md` under the OMP agent directory and does not write files.
- OMP apply writes that skill file with the same content as `crates/stateful-cli/assets/stateful-command-policy/SKILL.md`.
- Existing Codex install tests should continue to pass unchanged.

## Risks

The OMP runtime must discover skills from the selected path. This design follows the existing OMP agent home layout used by config, extension, and MCP files; if OMP requires a different skill path, update only the path helper while keeping the same asset reuse.
