# DeNovo OMP CLI Benchmark Design

## Goal

Add OMP as a first-class DeNovoSWE benchmark runner so the existing condition matrix can run with `--agent omp-cli` and model `deepseek-v4-flash`.

## Decisions

### Scope

Add `DeNovoAgentKind::OmpCli` and route it through the existing DeNovo adapter pipeline. The benchmark keeps the current dataset selection, workspace export, prompt generation, patch capture, evaluation, reporting, and condition aggregation.

The implementation must not introduce a generic runtime abstraction or a second full adapter file. The adapter gets one narrow runtime branch: Codex command construction/execution versus OMP command construction/execution.

### Condition axes

OMP supports the same condition matrix as Codex:

```text
stateful:on/off × subagent:on/off
```

For OMP, `stateful:on` and `stateful:off` must both use isolated OMP home/profile state. They must not inherit host Codex config, active Codex session state, Codex rules, or Codex skills.

The only difference between OMP `stateful:on` and `stateful:off` is whether the isolated OMP agent directory contains the stateful OMP install/config:

- `stateful:on`: install/write the stateful OMP extension, MCP config, and approval config into the isolated OMP agent directory.
- `stateful:off`: use the same isolated OMP layout without stateful extension, MCP config, or stateful hook integration.

### OMP command

The OMP runtime branch runs non-interactively:

```text
omp -p --mode json --model deepseek-v4-flash --cwd <workspace> --approval-mode yolo
```

The Rust CLI exposes this through:

```text
stateful-bench denovo run --agent omp-cli --benchmark-model deepseek-v4-flash
```

The OMP binary should be configurable separately from `--codex-bin` with an `--omp-bin` option, defaulting to `omp`.

### Metadata and limits

Codex-specific resume, token accounting, and native Codex subagent-count enforcement stay Codex-only unless OMP exposes equivalent reliable metadata from its own agent state.

OMP result records still include command, return code, duration, patch diff, eval result, and benchmark metadata. If OMP subagent metadata is not reliably available, `subagent:on` remains a declared condition axis but must not reuse Codex-native subagent database checks.

## Tests

Add the smallest regression coverage:

- Rust command-builder test: `--agent omp-cli` uses the existing adapter with `--cli-runtime omp`, `--omp-bin`, and `--benchmark-model deepseek-v4-flash`.
- Python adapter test: OMP command construction emits `omp -p --mode json --model deepseek-v4-flash --cwd <workspace> --approval-mode yolo` and does not include Codex flags or Codex config paths.
- Isolation test: OMP `stateful:on/off` use isolated OMP agent state, with stateful install/config present only for `stateful:on`.

Existing Codex tests should keep passing unchanged.

## Documentation

Update DeNovo benchmark command docs with an OMP example using `deepseek-v4-flash`.

Document that OMP condition isolation is OMP-owned: no host Codex config, active Codex session, Codex rules, or Codex skills may leak into OMP benchmark runs.
