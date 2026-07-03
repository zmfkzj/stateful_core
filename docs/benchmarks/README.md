# Benchmark Evidence

Only checked reports and compare outputs belong here. Do not commit raw logs, runtime homes, session dumps, or per-agent transcripts.

Before adding a result:

- scrub absolute local paths and tokens from every checked artifact;
- reject any artifact containing authorization-token markers or the local home path;
- label evidence kind, trial count, instance set, model, prompt/runtime limits, and evaluator version;
- state non-comparable caveats and omitted metrics explicitly;
- keep quality metrics separate from efficiency metrics.

## Results

- `2026-07-03-forced-overlap-three-arm.md` — three OMP forced-overlap trials across no-state, awareness, and stateful/enforcement arms; compare artifacts are checked in as `compare-t*.md/json`.
- `2026-07-03-programbench-stateful-pair.md` — three ProgramBench inference trials plus the recorded macOS arm64 official-eval blocker; no quality comparison claim.
