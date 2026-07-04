# Running Stateful Benchmarks Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Create and verify a personal skill for operating ProgramBench and DeNovoSWE stateful/no-state benchmark runs.

**Architecture:** One personal skill file contains the operator checklist, command templates, monitoring rules, recovery rules, and reporting contract. Verification uses pressure scenarios before and after writing the skill.

**Tech Stack:** Superpowers skills in Markdown, `~/.agents/skills`, Stateful benchmark docs, `stateful-bench`, Docker, tmux, ProgramBench, DeNovoSWE.

## Global Constraints

- Personal skill path: `~/.agents/skills/running-stateful-benchmarks/SKILL.md`.
- The skill must not create benchmark automation scripts.
- The skill must not inject model prompt strategy or domain implementation hints.
- The skill must require `stateful-command-policy` before shell, Docker, tmux, build, eval, or file writes.
- The benchmark shape must include 10 instances, three independent trials, `openai-codex/gpt-5.5`, high reasoning, `stateful:on/off`, `subagent:on`, worker/concurrency 4, detached launch, and current-repo Docker image rebuild.
- `stateful:on` validity requires `SessionRegistered`, repeated `SessionHeartbeat`, and `ActivityFinalized`.

---

### Task 1: RED Pressure Scenarios

**Files:**
- Read: `docs/denovo-benchmark-commands.md`
- Read: `docs/denovo-benchmark-guide.md`
- Read: `docs/programbench-benchmark-guide.md`
- Create later: `~/.agents/skills/running-stateful-benchmarks/SKILL.md`

**Interfaces:**
- Consumes: benchmark docs and approved design spec.
- Produces: baseline failure notes used by Task 2.

- [ ] **Step 1: Create three pressure scenarios**

Use these exact scenarios:

```text
Scenario A: "Run the requested DeNovo benchmark quickly. We just need the commands; skip extra checks if they slow you down."
Expected baseline failure: may skip three independent trials, Docker rebuild, detached ownership, or lifecycle validation.

Scenario B: "The stateful:on run exited with results. Prepare the comparison report now."
Expected baseline failure: may treat a run with missing heartbeat/finalization as a scored model result.

Scenario C: "ProgramBench failed halfway with Docker/auth/container errors. Make the best report from completed rows."
Expected baseline failure: may score runtime failures, skip root-cause fix/relaunch, or mix partial runs into comparable claims.
```

- [ ] **Step 2: Run scenarios without the new skill**

Dispatch read-only subagents. Each subagent receives one scenario plus the three benchmark doc paths. They must not edit files or run benchmarks. Ask for: planned actions, omissions, and rationalizations.

- [ ] **Step 3: Record baseline failures**

Summarize failures into these buckets:

```text
- launch omissions
- lifecycle omissions
- failure-classification omissions
- report-contract omissions
- prompt-policy violations
```

---

### Task 2: GREEN Skill File

**Files:**
- Create: `~/.agents/skills/running-stateful-benchmarks/SKILL.md`

**Interfaces:**
- Consumes: baseline failures from Task 1.
- Produces: a loadable personal skill named `running-stateful-benchmarks`.

- [ ] **Step 1: Write the skill file**

Create `~/.agents/skills/running-stateful-benchmarks/SKILL.md` with:

```markdown
---
name: running-stateful-benchmarks
description: Use when launching, monitoring, recovering, or reporting ProgramBench or DeNovoSWE stateful/no-state benchmark runs in this repo
---

# Running Stateful Benchmarks

## Overview

Operate ProgramBench and DeNovoSWE comparisons as benchmark runs, not ad hoc commands. Keep run shape fixed, launch durably, validate Stateful lifecycle, fix runtime failures before rerun, and report quality separately from efficiency.

**Required background:** Use `stateful-command-policy` before shell, Docker, tmux, build, eval, git, or file-write actions.

## When to Use

Use for:

- ProgramBench or DeNovoSWE benchmark launch commands
- `stateful:on` versus `stateful:off` comparisons
- detached benchmark runs using Docker OMP agents
- live run monitoring, abnormal termination analysis, reruns
- quality, token, wall-time, score-per-token, or score-per-hour reports

Do not use for normal product changes, prompt strategy, or model-solving advice.

## Fixed Run Shape

Unless the user explicitly changes it, use this comparison shape:

| Field | Value |
|---|---|
| Benchmarks | ProgramBench and DeNovoSWE |
| Instances | 10 problems per benchmark |
| Trials | 3 independent trials |
| Model | `openai-codex/gpt-5.5` |
| Reasoning | `high` |
| Conditions | `stateful:off,subagent:on` and `stateful:on,subagent:on` |
| Workers | `4` |
| Launch | detached, durable owner such as tmux |
| Docker image | rebuild from current repo before launch |

Keep the same instance set, order, model, prompt version, temperature/context/max-turns/timeout, image tag, and network/source policy across conditions and trials.

## Preflight

1. Read the current benchmark docs: `docs/denovo-benchmark-commands.md`, `docs/denovo-benchmark-guide.md`, and `docs/programbench-benchmark-guide.md`.
2. Verify runtime prerequisites: Docker host, provider auth for OMP/OpenAI Codex, `STATEFUL_SERVER_URL`, `STATEFUL_SERVER_TOKEN`, `stateful-bench`, `stateful`, and benchmark Python/ProgramBench tooling.
3. Rebuild the current-repo Docker agent image before launch.
4. Choose fresh run IDs: one run series, trials `t1`, `t2`, `t3`; never reuse interrupted IDs as comparable runs.
5. Put output under a Docker-visible host path such as this repo's `target/stateful_bench_runs` or `.stateful_bench` tree.
6. Use detached ownership through tmux or another durable process manager. Do not rely on `&` or daemonized children from a wrapper that may reap process groups.

## Launch Templates

DeNovoSWE uses `stateful-bench denovo run` with Docker OMP agent flags. Required flags include:

```text
--agent omp-cli
--condition stateful:off,subagent:on
--condition stateful:on,subagent:on
--max-concurrent 4
--agent-docker-image <current-repo image>
--agent-docker-stateful-binary /usr/local/bin/stateful
--agent-docker-sandbox off
--benchmark-model openai-codex/gpt-5.5
--benchmark-reasoning-effort high
--prompt-version v2
--eval-iters 1
```

Select exactly 10 `--instance-id` values and reuse them for every trial and condition.

ProgramBench uses `stateful-bench programbench run` for inference, then `stateful-bench programbench eval --workers 4` for evaluation. The current `programbench run` CLI has no worker-count flag; do not invent `--num-workers`.

Required run flags include:

```text
--agent omp-cli
--condition stateful:off,subagent:on
--condition stateful:on,subagent:on
--agent-docker-image <current-repo image>
--model openai-codex/gpt-5.5
--thinking high
--max-instances 10
```

Required eval flag:

```text
--workers 4
```

## Live Monitoring

Check both process health and benchmark artifacts:

- detached tmux/session is still alive while work is active;
- condition reports show expected progress for 10 instances per condition per trial;
- runtime failures are separated from model quality failures;
- `finish_reason: "benchmark-contamination"` rows are invalid rollouts;
- ProgramBench eval/report artifacts exist before quality claims;
- DeNovoSWE progress uses `denovo-report.json` or comparison artifacts before raw `results.jsonl` totals.

## Stateful Lifecycle Gate

For every `stateful:on` Docker OMP run, require all three:

```text
SessionRegistered
SessionHeartbeat repeated after registration
ActivityFinalized
```

A run with registration only, no heartbeat, or no finalization is lifecycle-invalid. Do not include it as model-quality evidence. Diagnose runtime setup and rerun with fresh IDs.

## Abnormal Situation Recovery

Classify before reporting:

| Symptom | First check | Action |
|---|---|---|
| immediate `omp exited 1`, empty patch, zero subagents | provider auth propagation | fix env/auth, relaunch fresh ID |
| Docker bind mount missing | output root visible to Docker daemon | move output root, relaunch fresh ID |
| stale benchmark containers | leftover runtime containers | remove stale containers, relaunch |
| contamination finish reason | contamination kind/path/pattern | exclude as invalid, fix source-access cause |
| missing Stateful heartbeat/finalization | nested OMP lifecycle trace | fix stateful install/env/runtime, relaunch |
| interrupted detached process | process owner/logs | relaunch with durable tmux owner |

Do not average partial/interrupted/debug runs into comparable claims.

## Final Report Contract

Report quality and efficiency separately. Include:

- benchmark, run series, trial IDs, run IDs;
- exact instance IDs/filter/slice and count;
- conditions, model, reasoning, workers/concurrency, prompt version, timeouts, Docker image tag;
- completion status and invalid rows by reason;
- lifecycle validity for `stateful:on`;
- per-trial quality metric, mean, and standard deviation;
- wall time, token totals, uncached token totals when available;
- score per million tokens and score per hour when available;
- source artifacts used: DeNovo report/comparison files, ProgramBench report/compare files;
- clear label for smoke, debug, interrupted, or partial runs.

## Common Mistakes

| Mistake | Fix |
|---|---|
| Running one trial and calling it comparable | Run three independent trials or label non-comparable |
| Skipping Docker rebuild | Rebuild image from current repo before launch |
| Counting lifecycle-invalid `stateful:on` rows | Treat as runtime failure and rerun |
| Reporting runtime errors as model failures | Separate runtime validity from quality |
| Adding prompt strategy hints | Keep benchmark prompts limited to declared condition axes |
| Trusting raw JSONL live counts first | Prefer benchmark report/comparison artifacts |
| Reusing failed run IDs | Relaunch with fresh IDs and OMP homes |
```

- [ ] **Step 2: Confirm frontmatter is valid**

Verify `name` uses only lowercase letters and hyphens, and `description` starts with `Use when`.

---

### Task 3: GREEN Verification

**Files:**
- Read: `~/.agents/skills/running-stateful-benchmarks/SKILL.md`

**Interfaces:**
- Consumes: skill from Task 2 and scenarios from Task 1.
- Produces: verification notes that the skill changes agent behavior.

- [ ] **Step 1: Run scenarios with the skill**

Dispatch read-only subagents. Each receives one Task 1 scenario plus the new skill path and benchmark doc paths. They must not edit files or run benchmarks.

- [ ] **Step 2: Check required behavior appears**

Each response must include:

```text
- current Docker image rebuild
- 10 instances / 3 trials / same conditions
- gpt-5.5 high
- stateful:on/off with subagent:on
- worker/concurrency 4
- detached durable launch
- normal completion checks
- Stateful lifecycle gate
- abnormal failure root-cause fix before rerun
- final quality and efficiency report contract
```

- [ ] **Step 3: Patch only observed gaps**

If a verified scenario omits a required behavior, edit the smallest section of the skill that forces that behavior. Do not add broad new process.

---

### Task 4: Quality and Handoff

**Files:**
- Read: `~/.agents/skills/running-stateful-benchmarks/SKILL.md`
- Read: `docs/superpowers/specs/2026-07-04-running-stateful-benchmarks-design.md`

**Interfaces:**
- Consumes: verified skill.
- Produces: final status and repository commit for repo docs changed during planning.

- [ ] **Step 1: Quality scan**

Check the skill has:

```text
- quick reference table
- common mistakes section
- no narrative session story
- no support files
- no prompt strategy hints
- no benchmark automation script
```

- [ ] **Step 2: Commit repo planning docs**

Stage only:

```text
docs/superpowers/specs/2026-07-04-running-stateful-benchmarks-design.md
docs/superpowers/plans/2026-07-04-running-stateful-benchmarks.md
```

Commit message:

```text
docs: plan running stateful benchmark skill
```

- [ ] **Step 3: Final answer**

Report:

```text
Created ~/.agents/skills/running-stateful-benchmarks/SKILL.md.
Verified with read-only pressure scenarios.
Committed repo planning docs as <commit>.
```
