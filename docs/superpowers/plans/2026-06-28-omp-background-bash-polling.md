# OMP Background Bash Polling Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add pollable live-session jobs for OMP background sandbox commands so agents can monitor progress and collect final results before handoff.

**Architecture:** Keep the feature inside the generated OMP extension emitted by `crates/stateful-cli/src/install.rs`. Background sandbox tools still return a `runId` immediately, but they also store live job state in an in-memory `Map`; a new `sandbox_job_poll` tool returns output deltas and final status by `runId`. No daemon, persistence layer, or server API is added.

**Tech Stack:** Rust tests and installer string generation, generated OMP JavaScript extension, existing Stateful sandbox command runner.

## Global Constraints

- Keep `async: false` foreground behavior unchanged.
- Keep sandbox profiles, external grant approval, timeout handling, and cancellation behavior unchanged.
- `async: true` background runs return `runId` immediately and store job state in the live OMP extension session.
- Add a polling tool that fetches status and output deltas by `runId`.
- Expose `running`, `done`, `failed`, and `not_found` states.
- Include final exit code, stdout/stderr data, error text, command label, start time, and finish time when available.
- Update model-facing guidance so background sandbox jobs are polled to completion before final handoff.
- Do not add persistent storage, a daemon, a server-side job API, or a new dependency.

---

## File Structure

- Modify `crates/stateful-cli/tests/install_global.rs:225-333`: extend the generated OMP extension assertions for `sandbox_job_poll`, the live job registry, job status strings, and poll offsets.
- Modify `crates/stateful-cli/src/install.rs:2309-2687`: add the in-memory job registry helpers and update `startSandboxBackgroundTool` to store stdout/stderr/final state.
- Modify `crates/stateful-cli/src/install.rs:2690-2882`: register the new `sandbox_job_poll` OMP tool without changing existing tool schemas except descriptions.
- Modify `crates/stateful-cli/assets/stateful-command-policy/omp-tools.md`: tell agents to poll background jobs before final response.
- Modify `docs/architecture.md`, `docs/core-concept.md`, `docs/current-state-coordination.md`, and `docs/implementation-contract.md`: replace next-turn-primary background result wording with pollable-job wording.

---

### Task 1: Generated extension contract tests

**Files:**
- Modify: `crates/stateful-cli/tests/install_global.rs:225-333`

**Interfaces:**
- Consumes: existing `install_omp_yes_creates_extension_and_mcp_config` test and generated extension string checks.
- Produces: failing assertions that define the new generated extension contract:
  - `sandbox_job_poll` tool exists.
  - `activeSandboxJobs` registry exists.
  - jobs track `stdoutPollOffset`, `stderrPollOffset`, `status`, `startedAt`, and `finishedAt`.
  - background start stores jobs before the runner resolves.
  - polling supports `running`, `done`, `failed`, and `not_found`.

- [ ] **Step 1: Add failing generated-extension assertions**

In `crates/stateful-cli/tests/install_global.rs`, inside `install_omp_yes_creates_extension_and_mcp_config`, add these assertions after the existing tool-name assertions around lines 231-238:

```rust
assert!(extension.contains("name: \"sandbox_job_poll\""));
assert!(extension.contains("Poll a background sandbox job by runId"));
```

Add these assertions after the existing background-tool assertions around lines 324-332:

```rust
assert!(extension.contains("const activeSandboxJobs = new Map();"));
assert!(extension.contains("function sandboxJobSnapshot(job)"));
assert!(extension.contains("function pollSandboxJob(params)"));
assert!(extension.contains("stdoutPollOffset"));
assert!(extension.contains("stderrPollOffset"));
assert!(extension.contains("status: \"running\""));
assert!(extension.contains("status: \"done\""));
assert!(extension.contains("status: \"failed\""));
assert!(extension.contains("status: \"not_found\""));
assert!(extension.contains("activeSandboxJobs.set(runId, job)"));
assert!(extension.contains("job.stdout += chunk"));
assert!(extension.contains("job.stderr = details.stderr || \"\""));
assert!(extension.contains("job.finishedAt = new Date().toISOString()"));
```

- [ ] **Step 2: Run the focused test and verify it fails**

Run:

```bash
cargo test -p stateful-cli --test install_global install_omp_yes_creates_extension_and_mcp_config -- --nocapture
```

Expected: FAIL because `sandbox_job_poll`, `activeSandboxJobs`, and the poll helpers do not exist yet.

- [ ] **Step 3: Commit the failing test**

Run:

```bash
git add crates/stateful-cli/tests/install_global.rs
git commit -m "test: define OMP sandbox job polling contract"
```

---

### Task 2: Pollable background job implementation

**Files:**
- Modify: `crates/stateful-cli/src/install.rs:2309-2687`
- Modify: `crates/stateful-cli/src/install.rs:2690-2882`

**Interfaces:**
- Consumes from Task 1: the string-level generated-extension contract.
- Produces:
  - generated JS global `activeSandboxJobs: Map<string, object>`.
  - generated JS function `sandboxJobSnapshot(job)`.
  - generated JS async function `pollSandboxJob(params)`.
  - generated OMP tool `sandbox_job_poll` with `run_id` and optional `wait_ms` parameters.
  - updated `startSandboxBackgroundTool` that stores and finalizes jobs.

- [ ] **Step 1: Add the live job registry and snapshot helpers**

In `crates/stateful-cli/src/install.rs`, find the generated JS globals near `backgroundSandboxToolCallIds`. Add this generated JavaScript. Because this code is inside a Rust `format!` string, double every JavaScript `{` and `}` as shown:

```js
const activeSandboxJobs = new Map();

function sandboxJobSnapshot(job) {{
  if (!job) {{
    return {{
      isError: false,
      content: [{{ type: "text", text: "Sandbox job not found." }}],
      details: {{ status: "not_found" }},
    }};
  }}
  const stdoutDelta = job.stdout.slice(job.stdoutPollOffset);
  const stderrDelta = job.stderr.slice(job.stderrPollOffset);
  job.stdoutPollOffset = job.stdout.length;
  job.stderrPollOffset = job.stderr.length;
  const text = [
    "Sandbox job " + job.runId + " " + job.status + ".",
    stdoutDelta ? "\nstdout:\n" + stdoutDelta : "",
    stderrDelta ? "\nstderr:\n" + stderrDelta : "",
  ].join("");
  return {{
    isError: false,
    content: [{{ type: "text", text }}],
    details: {{
      status: job.status,
      runId: job.runId,
      label: job.label,
      command: job.command,
      commandLabel: job.commandLabel,
      startedAt: job.startedAt,
      finishedAt: job.finishedAt,
      stdoutDelta,
      stderrDelta,
      stdout: job.stdout,
      stderr: job.stderr,
      exitCode: job.exitCode,
      error: job.error,
      result: job.result,
    }},
  }};
}}

function sleep(ms) {{
  return new Promise((resolve) => setTimeout(resolve, ms));
}}

async function pollSandboxJob(params) {{
  const runId = String(params?.run_id || params?.runId || "").trim();
  if (!runId) {{
    return sandboxToolError("sandbox_job_poll requires run_id");
  }}
  const waitMs = Number.isInteger(params?.wait_ms) && params.wait_ms > 0
    ? Math.min(params.wait_ms, 5000)
    : 0;
  const job = activeSandboxJobs.get(runId);
  if (!job) return sandboxJobSnapshot(undefined);
  if (job.status === "running" && waitMs > 0) {{
    const start = Date.now();
    const stdoutLength = job.stdout.length;
    const stderrLength = job.stderr.length;
    while (job.status === "running" && Date.now() - start < waitMs) {{
      if (job.stdout.length !== stdoutLength || job.stderr.length !== stderrLength) break;
      await sleep(100);
    }}
  }}
  return sandboxJobSnapshot(job);
}}
```

- [ ] **Step 2: Store jobs when background commands start**

In `startSandboxBackgroundTool`, immediately after `const commandLabel = ...`, add:

```js
const job = {{
  runId,
  label,
  command: params.command || commandText,
  commandLabel,
  status: "running",
  startedAt: new Date().toISOString(),
  finishedAt: undefined,
  stdout: "",
  stderr: "",
  stdoutPollOffset: 0,
  stderrPollOffset: 0,
  exitCode: undefined,
  error: undefined,
  result: undefined,
}};
activeSandboxJobs.set(runId, job);
```

- [ ] **Step 3: Append stdout chunks into the job buffer**

In the `createSandboxStdoutStreamer` callback inside `startSandboxBackgroundTool`, add the buffer append before `deliverSandboxBackgroundMessage`:

```js
job.stdout = truncateSandboxToolText(job.stdout + chunk, label);
```

Keep the existing `deliverSandboxBackgroundMessage` call for now. This preserves current UI streaming while the polling path becomes available.

- [ ] **Step 4: Finalize successful runner results in the registry**

In `runner.then(async (result) => { ... })`, after `const details = result?.details || {};`, add:

```js
job.status = details.error ? "failed" : "done";
job.finishedAt = new Date().toISOString();
job.stderr = details.stderr || "";
job.exitCode = details.exitCode;
job.error = details.error;
job.result = result;
```

Keep `postBackgroundSandboxToolUse(ctx, label, params)` unchanged.

- [ ] **Step 5: Finalize rejected runner results in the registry**

In `runner.then(...).catch((error) => { ... })`, before `deliverSandboxBackgroundMessage`, add:

```js
job.status = "failed";
job.finishedAt = new Date().toISOString();
job.error = error instanceof Error ? error.message : String(error);
```

- [ ] **Step 6: Register the polling tool**

In `statefulOmpExtension(pi)`, register `sandbox_job_poll` after `process_find` and before `lazy_edit_resume`:

```js
pi.registerTool({{
  name: "sandbox_job_poll",
  label: "Sandbox Job Poll",
  description: "Poll a background sandbox job by runId and return status plus stdout/stderr deltas.",
  parameters: {{
    type: "object",
    properties: {{
      run_id: {{ type: "string", description: "Background sandbox runId returned by sandbox_bash, ext_ro_bash, ext_rw_bash, or process_find." }},
      wait_ms: {{ type: "number", description: "Optional milliseconds to wait for new output or completion, capped at 5000." }},
    }},
    required: ["run_id"],
  }},
  async execute(_toolCallId, params) {{
    return await pollSandboxJob(params || {{}});
  }},
}});
```

- [ ] **Step 7: Run the focused generated-extension test**

Run:

```bash
cargo test -p stateful-cli --test install_global install_omp_yes_creates_extension_and_mcp_config -- --nocapture
```

Expected: PASS.

- [ ] **Step 8: Commit the implementation**

Run:

```bash
git add crates/stateful-cli/src/install.rs
git commit -m "feat: add OMP sandbox job polling"
```

---

### Task 3: Documentation and agent guidance

**Files:**
- Modify: `crates/stateful-cli/assets/stateful-command-policy/omp-tools.md`
- Modify: `docs/architecture.md`
- Modify: `docs/core-concept.md`
- Modify: `docs/current-state-coordination.md`
- Modify: `docs/implementation-contract.md`

**Interfaces:**
- Consumes from Task 2: `sandbox_job_poll(run_id, wait_ms?)` and live-session job semantics.
- Produces: docs that describe background jobs as pollable and tell agents to poll them to completion before final handoff.

- [ ] **Step 1: Update skill guidance**

In `crates/stateful-cli/assets/stateful-command-policy/omp-tools.md`, replace the generated tool behavior bullet that currently says background tools later post stdout/stderr/exit status with this text:

```markdown
- `sandbox_bash`, `ext_ro_bash`, `ext_rw_bash`, and `process_find` run in the background by default. With `async` omitted or `true`, they return a `runId` immediately and store live job state in the OMP extension. Use `sandbox_job_poll` with that `runId` to monitor stdout/stderr deltas and collect final exit status before ending the turn. Set `async: false` for awaited foreground behavior.
```

- [ ] **Step 2: Update architecture docs**

In each of these files, replace the existing paragraph that says background tools stream output/final status through next-turn `pi.sendMessage` as the primary behavior:

```text
docs/architecture.md
docs/core-concept.md
docs/current-state-coordination.md
docs/implementation-contract.md
```

Use this replacement wording, adapted only for surrounding grammar:

```markdown
All generated OMP sandbox command tools run in the background by default. With `async` omitted or `true`, they return a `runId` immediately and store live stdout/stderr/status in the OMP extension. Agents use `sandbox_job_poll` to monitor output deltas and collect final exit status before final handoff. Set `async: false` for awaited foreground behavior with final stdout/stderr/status in the tool result.
```

- [ ] **Step 3: Run doc conflict checks with repository search**

Run these searches with the repository search tool or equivalent approved search path:

```bash
# Use the repo search tool, not raw grep.
Search for: "deliverAs: \"nextTurn\""
Search for: "nextTurn"
Search for: "later post stdout"
Search for: "send final stdout"
Search for: "sandbox_job_poll"
```

Expected:

- `deliverAs: "nextTurn"` may remain in implementation if compatibility streaming stays.
- User-facing docs should no longer describe next-turn delivery as the primary collection path.
- `sandbox_job_poll` appears in the generated extension tests, implementation, docs, and skill guidance.

- [ ] **Step 4: Commit documentation**

Run:

```bash
git add crates/stateful-cli/assets/stateful-command-policy/omp-tools.md docs/architecture.md docs/core-concept.md docs/current-state-coordination.md docs/implementation-contract.md
git commit -m "docs: document OMP sandbox job polling"
```

---

### Task 4: Final verification

**Files:**
- Verify: `crates/stateful-cli/src/install.rs`
- Verify: `crates/stateful-cli/tests/install_global.rs`
- Verify: `crates/stateful-cli/assets/stateful-command-policy/omp-tools.md`
- Verify: `docs/architecture.md`
- Verify: `docs/core-concept.md`
- Verify: `docs/current-state-coordination.md`
- Verify: `docs/implementation-contract.md`

**Interfaces:**
- Consumes from Tasks 1-3: implementation, tests, and docs.
- Produces: verified branch ready for maintainer handoff.

- [ ] **Step 1: Run targeted generated-extension test**

Run:

```bash
cargo test -p stateful-cli --test install_global install_omp_yes_creates_extension_and_mcp_config -- --nocapture
```

Expected: PASS.

- [ ] **Step 2: Run adjacent OMP install tests**

Run:

```bash
cargo test -p stateful-cli --test install_global omp -- --nocapture
```

Expected: PASS for OMP install-related tests.

- [ ] **Step 3: Inspect focused diff**

Run:

```bash
git diff -- crates/stateful-cli/src/install.rs crates/stateful-cli/tests/install_global.rs crates/stateful-cli/assets/stateful-command-policy/omp-tools.md docs/architecture.md docs/core-concept.md docs/current-state-coordination.md docs/implementation-contract.md
```

Expected:

- No unrelated file changes.
- No new dependencies.
- `async:false` branches remain present.
- External approval branches remain present.
- `sandbox_job_poll` is documented and registered.

- [ ] **Step 4: Commit any final fixups**

If Step 3 shows required fixups, stage only the changed files from this plan and commit them:

```bash
git add crates/stateful-cli/src/install.rs crates/stateful-cli/tests/install_global.rs crates/stateful-cli/assets/stateful-command-policy/omp-tools.md docs/architecture.md docs/core-concept.md docs/current-state-coordination.md docs/implementation-contract.md
git commit -m "fix: align OMP sandbox job polling"
```

If Step 3 shows no fixups, do not create an empty commit.

---

## Self-Review

- Spec coverage: Tasks 1-2 cover live-session registry, `runId`, polling, states, deltas, final status, and unchanged foreground behavior. Task 3 covers model-facing guidance and docs. Task 4 covers verification.
- Red-flag scan: the plan contains concrete file paths, tool names, code snippets, commands, and expected results.
- Type consistency: the plan uses `sandbox_job_poll`, `run_id`, `wait_ms`, `runId`, `stdoutPollOffset`, and `stderrPollOffset` consistently across tests, implementation, and docs.
