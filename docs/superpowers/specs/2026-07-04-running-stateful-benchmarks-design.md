# Running Stateful Benchmarks Skill Design

## Goal

Create a personal skill that lets an agent operate the repo's ProgramBench and DeNovoSWE benchmark flow without re-deriving commands from the benchmark docs.

## Scope

The skill lives at `~/.agents/skills/running-stateful-benchmarks/SKILL.md`.

It covers:

- rebuilding the current-repo Docker agent image before runs;
- launching ProgramBench and DeNovoSWE with 10 instances, three independent trials, `openai-codex/gpt-5.5`, high reasoning, `stateful:on/off`, `subagent:on`, `num_workers`/concurrency 4, and detached ownership;
- checking live runs for normal completion;
- checking `stateful:on` lifecycle evidence: `SessionRegistered`, repeated `SessionHeartbeat`, and `ActivityFinalized`;
- treating runtime failures, contamination, missing auth, Docker mount failures, stale containers, and missing lifecycle as invalid run failures, not model-quality scores;
- analyzing and fixing abnormal run causes, then relaunching with fresh run IDs;
- producing a final quality and efficiency comparison report for stateful on versus off.

## Non-Goals

- Do not add benchmark strategy hints to model prompts.
- Do not create new benchmark automation scripts.
- Do not change repository code or benchmark adapters.
- Do not make unsupported official-quality claims from partial, interrupted, or one-off runs.

## Approach

Use one compact operator skill, `running-stateful-benchmarks`, with inline command templates and checklists. No support files are needed.

The skill should be procedure-shaped, not narrative-shaped:

1. Preflight: load benchmark docs, verify repo root, build image, set runtime variables, ensure provider auth and Stateful server env are present.
2. Launch: use durable detached process ownership, fresh run series/trial IDs, current Docker image, same instance set and settings across both conditions.
3. Monitor: prefer benchmark reports over raw JSONL row counts; confirm completion and lifecycle traces.
4. Recover: classify failures, fix root cause, clean stale containers, and relaunch with fresh IDs.
5. Report: compare quality separately from efficiency; include trial values, means, standard deviations when small, lifecycle validity, contamination status, model/settings, and source artifacts.

## Command Policy

The skill must tell agents to obey `stateful-command-policy` before running shell, Docker, tmux, build, eval, or file writes. Long detached runs should be owned by tmux or another durable process manager, not a fragile background child process.

## Validation

Skill validation will use pressure scenarios:

- Baseline agent asked to run the benchmark should reveal likely omissions such as skipping the Docker rebuild, using one trial, failing to check Stateful lifecycle, or treating runtime failures as scores.
- With the skill loaded, the agent should produce the required preflight, launch, monitor, failure-recovery, and report plan without injecting prompt strategy or omitting lifecycle validation.

## Success Criteria

- The personal skill exists with valid YAML frontmatter and a discovery description beginning with `Use when`.
- The skill includes the exact requested run shape and the differences between ProgramBench and DeNovoSWE command surfaces.
- The skill keeps benchmark prompt policy clean: no domain implementation hints or role assignments.
- The skill names the minimum lifecycle evidence for `stateful:on`.
- The skill names report fields needed for quality and efficiency comparison.
- The skill is small enough to scan and has no support files unless a later test proves a gap.
