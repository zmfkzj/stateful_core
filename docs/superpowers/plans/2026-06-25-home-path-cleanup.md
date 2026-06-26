# HOME-like Path Cleanup Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Remove hardcoded user HOME paths from tracked repo content, replacing them with repo-relative paths, env-var examples, or neutral fixtures where possible.

**Architecture:** Keep real container contract paths where they are part of Docker behavior (`/home/stateful`, `/workspace`, `/repo/...`). Replace host-specific macOS home paths and non-contract HOME-like examples with env vars or relative fixture paths. Do not add new abstractions.

**Tech Stack:** Rust tests, Python adapter tests embedded in Rust strings, Markdown docs.

## Global Constraints

- Replace personal host paths everywhere tracked.
- Prefer repo-relative paths or env vars for docs and examples.
- Keep Docker/container contract paths only when tests or adapter behavior depend on them.
- Use existing tests; add only tiny assertions if a replacement needs regression coverage.
- No new dependency.

---

### Task 1: Documentation path cleanup

**Files:**
- Modify: `docs/denovo-benchmark-commands.md`

**Interfaces:**
- Consumes: existing benchmark command examples.
- Produces: docs with `$REPO_ROOT`, `$STATEFUL_BENCH_RUNS`, `$AWEAGENT_ROOT`, `$PYTHON`, `$DOCKER_HOST` instead of personal absolute paths.

- [ ] **Step 1: Replace local run snippet paths**

Use env-var based paths:

```bash
REPO_ROOT=${REPO_ROOT:-$(pwd)}
STATEFUL_BENCH_RUNS=${STATEFUL_BENCH_RUNS:-$REPO_ROOT/target/stateful_bench_runs}
RUN_SERIES=r20260624-denovo-12-omp-docker-subagent-on-auth
STATEFUL_BENCH_BIN=${STATEFUL_BENCH_BIN:-$STATEFUL_BENCH_RUNS/cargo-target/debug/stateful-bench}
STATEFUL_BIN=${STATEFUL_BIN:-$STATEFUL_BENCH_RUNS/cargo-target/debug/stateful}
PYTHON=${PYTHON:-$REPO_ROOT/tmp/aweagent-venv/bin/python}
AWEAGENT_ROOT=${AWEAGENT_ROOT:-$REPO_ROOT/tmp/AweAgent}
DENOVO_OUTPUT_ROOT=${DENOVO_OUTPUT_ROOT:-$STATEFUL_BENCH_RUNS/denovo/runs}
DOCKER_HOST=${DOCKER_HOST:-unix://$HOME/.colima/default/docker.sock}
```

- [ ] **Step 2: Verify docs no longer contain a personal macOS home path**

Run: `search \"macOS home prefix\" docs/denovo-benchmark-commands.md`
Expected: no matches.

### Task 2: stateful-bench test fixture cleanup

**Files:**
- Modify: `crates/stateful-bench/tests/cli.rs`
- Modify: `crates/stateful-bench/tests/denovo.rs`

**Interfaces:**
- Consumes: existing test expectations for command construction and prompt redaction.
- Produces: neutral fixture paths such as `/opt/stateful/bin/stateful`, `/workspace/.stateful-home`, or repo-relative `target/...` where accepted.

- [ ] **Step 1: Replace personal stateful binary fixtures**

Use `/opt/stateful/bin/stateful` or existing container fake paths instead of a personal cargo `stateful` binary path.

- [ ] **Step 2: Replace non-contract HOME fixtures where possible**

Use `target/...` for nested home roots in pure command-generation tests. Keep `/home/stateful` only for Docker agent contract assertions.

- [ ] **Step 3: Run focused tests**

Run: `CARGO_HOME="$TMPDIR/cargo" cargo test -p stateful-bench --test cli denovo_run_command_parses_omp_cli_agent_options`
Expected: PASS.

Run: `CARGO_HOME="$TMPDIR/cargo" cargo test -p stateful-bench --test denovo`
Expected: PASS.

### Task 3: stateful-cli and server fixture cleanup

**Files:**
- Modify: `crates/stateful-cli/src/codex_benchmark.rs`
- Modify: `crates/stateful-cli/tests/cli.rs`
- Modify: `crates/stateful-server/tests/routes.rs`

**Interfaces:**
- Consumes: existing CLI parser and sandbox profile tests.
- Produces: neutral fixture socket/root paths without personal host paths.

- [ ] **Step 1: Replace personal Docker socket fixtures**

Use `/var/run/docker.sock` or `/tmp/colima/docker.sock` consistently in tests.

- [ ] **Step 2: Replace personal workspace fixture**

Use `/workspace/edge/core` for route test fixture root.

- [ ] **Step 3: Run focused tests**

Run: `CARGO_HOME="$TMPDIR/cargo" cargo test -p stateful-cli --test cli`
Expected: PASS.

Run: `CARGO_HOME="$TMPDIR/cargo" cargo test -p stateful-server --test routes`
Expected: PASS.

### Task 4: Final grep and verification

**Files:**
- Search only unless earlier tasks reveal another tracked personal path.

**Interfaces:**
- Consumes: changes from Tasks 1-3.
- Produces: no tracked personal macOS home path matches except historical scratch artifacts if intentionally ignored by git.

- [ ] **Step 1: Search tracked relevant files**

Run: `search \"macOS home prefix\" README.md docs crates .github`
Expected: no matches in active source/docs/tests.

- [ ] **Step 2: Run focused suites touched by changes**

Run the three commands from Tasks 2-3 that cover changed Rust tests.
Expected: PASS.

- [ ] **Step 3: Commit**

```bash
git add docs/denovo-benchmark-commands.md crates/stateful-bench/tests/cli.rs crates/stateful-bench/tests/denovo.rs crates/stateful-cli/src/codex_benchmark.rs crates/stateful-cli/tests/cli.rs crates/stateful-server/tests/routes.rs docs/superpowers/plans/2026-06-25-home-path-cleanup.md
git commit -m "fix: replace hardcoded home paths"
```
