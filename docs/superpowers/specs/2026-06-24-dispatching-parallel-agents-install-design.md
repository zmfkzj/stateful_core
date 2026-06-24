# Dispatching Parallel Agents Install Design

## Goal

Install the `dispatching-parallel-agents` skill with every Stateful Codex and OMP install so DeNovo OMP benchmark runs receive the same skill through the normal OMP install path.

## Scope

- Add `skills/dispatching-parallel-agents/SKILL.md` to Codex installs.
- Add `skills/dispatching-parallel-agents/SKILL.md` to OMP installs.
- Keep DeNovo benchmark adapter behavior unchanged; `stateful:on` OMP runs already call `stateful install --agent omp --yes` in the isolated OMP home.
- Do not add benchmark-specific duplicate copying or prompt text.

## Design

Reuse the existing installer asset-copy pattern used for `stateful-command-policy`:

1. Embed the dispatching skill text as a CLI asset.
2. Write it under each target agent directory during install:
   - Codex: sibling of `skills/stateful-command-policy/SKILL.md`.
   - OMP: sibling of `skills/stateful-command-policy/SKILL.md`.
3. Keep `stateful-command-policy` unchanged; this feature only adds an additional skill file.

This is the smallest path because all DeNovo OMP `stateful:on` setup already flows through OMP install. Installing it globally also satisfies normal Codex and OMP users without adding benchmark-only code.

## Data Flow

```text
stateful install --agent codex
  -> write Codex config/hooks/MCP
  -> write stateful-command-policy skill
  -> write dispatching-parallel-agents skill

stateful install --agent omp
  -> write OMP config/extension/MCP/rule/tools
  -> write stateful-command-policy skill
  -> write dispatching-parallel-agents skill

stateful-bench DeNovo OMP stateful:on
  -> prepare isolated OMP home
  -> stateful install --agent omp --yes
  -> isolated OMP home contains dispatching-parallel-agents skill
```

## Errors

Install should fail with context if the skill directory cannot be created or the skill file cannot be written, matching the existing command-policy skill behavior.

## Tests

Update installer tests to assert:

- Codex install creates `skills/dispatching-parallel-agents/SKILL.md`.
- OMP install creates `skills/dispatching-parallel-agents/SKILL.md`.
- The installed content matches the source asset.

A DeNovo-specific test is unnecessary because DeNovo OMP uses the same OMP install function; the installer tests cover the shared path.
