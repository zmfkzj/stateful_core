# OMP Generated Bash Tool Removal Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Remove Stateful-generated OMP Bash/process custom tools and rely on OMP built-in Bash passthrough for trusted Stateful sandbox/process commands.

**Architecture:** The OMP extension keeps one built-in Bash preflight hook plus lazy edit/write resume tools. Trusted command execution flows through OMP's native Bash runtime; Stateful only validates the command shape, handles external grant prompts, and runs normal hook authorization. Removed generated tools no longer register, no longer appear in installed docs, and no longer produce sandbox job IDs.

**Tech Stack:** Rust CLI generating a JavaScript OMP extension string, Rust integration tests, Markdown docs/assets.

## Global Constraints

- Use TDD: write failing tests and observe failure before production-code changes.
- Do not remove `lazy_edit_resume` or `lazy_write_resume`.
- Do not add per-call `auto_approve` syntax to built-in Bash commands.
- Do not change Codex command policy.
- Do not add dependencies.
- Built-in Bash must remain enabled with `bash.enabled: true`.
- OMP built-in Bash may allow only strict trusted `stateful sandbox run ...` and `stateful sandbox process find ...` commands.
- External write/create/write-dir/socket/signal scope must still prompt unless `stateful.autoApprove: true`.

---

### Task 1: Red tests for removed generated OMP tools

**Files:**
- Modify: `crates/stateful-cli/tests/install_global.rs`
- Modify: `crates/stateful-cli/tests/hook.rs`

**Interfaces:**
- Consumes: existing `install_omp_yes_creates_extension_and_mcp_config`, `handle_omp_pre_tool_use_with_runtime`, and `OmpHookOutcome` test helpers.
- Produces: failing tests that define the removal contract before implementation.

- [ ] **Step 1: Update install-generation assertions to require removed tools absent**

In `crates/stateful-cli/tests/install_global.rs`, inside `install_omp_yes_creates_extension_and_mcp_config`, replace assertions that expect removed tool registrations with absence assertions:

```rust
assert!(!extension.contains("name: \"sandbox_bash\""));
assert!(!extension.contains("name: \"ext_ro_bash\""));
assert!(!extension.contains("name: \"ext_rw_bash\""));
assert!(!extension.contains("name: \"process_find\""));
assert!(!extension.contains("name: \"sandbox_job_poll\""));
assert!(extension.contains("name: \"lazy_edit_resume\""));
assert!(extension.contains("name: \"lazy_write_resume\""));
```

Delete assertion blocks that require generated sandbox helper internals, including these string checks:

```rust
assert!(extension.contains("SANDBOX_BASH_FS_PROFILES"));
assert!(extension.contains("sandbox_bash does not support --fs external; use ext_ro_bash"));
assert!(extension.contains("function processFindArgs(params)"));
assert!(extension.contains("const args = [\"sandbox\", \"process\", \"find\"];"));
assert!(extension.contains("stateful_sandbox_bash_output"));
assert!(extension.contains("stateful_sandbox_bash_result"));
assert!(extension.contains("ext_ro_bash does not accept write, socket, or signal scope"));
assert!(extension.contains("ext_rw_bash requires at least one write_targets"));
```

Keep assertions for built-in Bash passthrough and external grant helpers:

```rust
assert!(extension.contains("pi.on(\"tool_call\""));
assert!(extension.contains("function statefulBashPassthroughDecision"));
assert!(extension.contains("function externalGrantDescriptor(params)"));
assert!(extension.contains("async function ensureExternalBashGrant(ctx, params, signal)"));
assert!(extension.contains("Built-in Bash external sandbox command requires OMP UI confirmation"));
```

- [ ] **Step 2: Update hook tests for removed generated tools**

In `crates/stateful-cli/tests/hook.rs`, remove tests that assert generated tools are allowed:

```rust
fn omp_sandbox_bash_tool_is_allowed_for_internal_sandbox_runner()
fn omp_process_find_tool_is_allowed_for_internal_runner()
fn omp_sandbox_job_poll_tool_is_allowed_for_internal_runner()
fn omp_external_tool_wrappers_are_allowed_for_internal_runners()
```

Add one replacement test near the OMP tool-action tests:

```rust
#[test]
fn omp_removed_generated_command_tools_are_not_allowlisted() {
    for tool_name in [
        "sandbox_bash",
        "ext_ro_bash",
        "ext_rw_bash",
        "process_find",
        "sandbox_job_poll",
    ] {
        let input = serde_json::json!({
            "session_id": "omp-parent",
            "cwd": "/repo",
            "yolo": false,
            "tool_name": tool_name,
            "tool_input": {
                "command": "pwd",
                "purpose": "test removed generated tool",
                "fs": "read-only"
            }
        })
        .to_string();

        let OmpHookOutcome::Block { reason } = handle_omp_pre_tool_use_with_runtime(
            &input,
            None,
            Some(Path::new("/repo")),
            Some(Path::new("/repo")),
        )
        .unwrap() else {
            panic!("{tool_name} should no longer be allowlisted");
        };
        assert!(reason.contains("not classified") || reason.contains("unclassified"));
    }
}
```

- [ ] **Step 3: Verify RED**

Run:

```bash
CARGO_HOME=$TMPDIR/cargo cargo test -p stateful-cli --test install_global install_omp_yes_creates_extension_and_mcp_config -- --nocapture
```

Expected: FAIL because the extension still registers removed generated tools.

Run:

```bash
CARGO_HOME=$TMPDIR/cargo cargo test -p stateful-cli --test hook omp_removed_generated_command_tools_are_not_allowlisted -- --nocapture
```

Expected: FAIL because the hook still allowlists at least some removed generated tools.

- [ ] **Step 4: Commit red tests**

```bash
git add crates/stateful-cli/tests/install_global.rs crates/stateful-cli/tests/hook.rs
git commit -m "test: define OMP generated tool removal"
```

---

### Task 2: Remove generated tool registration and tool-only helpers

**Files:**
- Modify: `crates/stateful-cli/src/install.rs`
- Modify: `crates/stateful-cli/tests/install_global.rs`

**Interfaces:**
- Consumes: tests from Task 1.
- Produces: generated OMP extension with only built-in Bash gate, lazy resume tools, and Stateful event hooks.

- [ ] **Step 1: Delete generated tool registrations**

In `crates/stateful-cli/src/install.rs`, inside `statefulOmpExtension(pi)`, delete the complete `pi.registerTool` blocks for:

```js
name: "process_find"
name: "sandbox_job_poll"
name: "sandbox_bash"
name: "ext_ro_bash"
name: "ext_rw_bash"
```

Do not delete the `pi.registerTool` blocks for:

```js
name: "lazy_edit_resume"
name: "lazy_write_resume"
```

- [ ] **Step 2: Delete generated sandbox job state and polling helpers**

Delete constants/functions only used by removed tools:

```js
const MAX_SANDBOX_TOOL_OUTPUT_BYTES = 50 * 1024;
const SANDBOX_BASH_FS_PROFILES = new Set(["read-only", "write-targets", "build", "git", "github-pr"]);
const activeSandboxJobs = new Map();
function sandboxJobSnapshot(job) { ... }
function pollSandboxJob(params) { ... }
function createSandboxStdoutStreamer(onUpdate, runId, label) { ... }
function startSandboxBackgroundTool(pi, toolCallId, params, args, ctx, label, signal, onUpdate) { ... }
function runSandboxAwaitedTool(params, args, ctx, label, signal, onUpdate) { ... }
function spawnSandboxToolProcess(params, ctx, label, signal, onStdout, onStderr) { ... }
function runSandboxDisabledToolProcess(params, ctx, label, signal, onStdout, onStderr) { ... }
function finishSandboxToolProcess(params, ctx, label, signal, onStdout, onStderr) { ... }
```

If a listed function name differs slightly in the current file, delete the exact helper only when no remaining lazy/Bash passthrough code references it.

- [ ] **Step 3: Delete generated tool argument builders**

Delete helpers only used by removed generated tools:

```js
function addCommonSandboxArgs(args, params, toolName) { ... }
function sandboxBashArgs(params) { ... }
function processFindArgs(params) { ... }
function validateExternalPurposeAndCommand(params, toolName) { ... }
function externalReadOnlyBashArgs(params) { ... }
function externalReadWriteBashArgs(params) { ... }
function sandboxToolError(error) { ... }
function sandboxToolResultText(exitCode, stdout, stderr, error) { ... }
function truncateSandboxToolText(text, label) { ... }
```

Keep these helpers because built-in Bash passthrough still uses them:

```js
function splitStatefulCommandWords(command) { ... }
function parseStatefulSandboxRunWords(words) { ... }
function parseStatefulProcessFindWords(words) { ... }
function statefulBashPassthroughDecision(command) { ... }
function externalGrantDescriptor(params) { ... }
function ensureExternalBashGrant(ctx, params, signal) { ... }
function shouldAutoApproveStatefulPrompt(ctx, params) { ... }
function approveExternalBashGrantWithoutPrompt(params) { ... }
function hasExternalWriteScope(params) { ... }
function stringList(value) { ... }
function boolValue(value) { ... }
function numericOption(value, fallback) { ... }
```

- [ ] **Step 4: Run install test to verify green for extension shape**

Run:

```bash
CARGO_HOME=$TMPDIR/cargo cargo test -p stateful-cli --test install_global install_omp_yes_creates_extension_and_mcp_config -- --nocapture
```

Expected: PASS.

- [ ] **Step 5: Commit implementation**

```bash
git add crates/stateful-cli/src/install.rs crates/stateful-cli/tests/install_global.rs
git commit -m "refactor: remove generated OMP command tools"
```

---

### Task 3: Remove hook allowlist entries and update denials

**Files:**
- Modify: `crates/stateful-cli/src/hook.rs`
- Modify: `crates/stateful-cli/tests/hook.rs`

**Interfaces:**
- Consumes: removed generated tool test from Task 1.
- Produces: hook policy where only built-in Bash trusted Stateful commands get command-execution allowance in OMP.

- [ ] **Step 1: Remove OMP allowlist branches for removed generated tools**

In `omp_pre_tool_action`, remove direct allow/target branches for these tool names:

```rust
"sandbox_bash"
"ext_ro_bash"
"ext_rw_bash"
"process_find"
"sandbox_job_poll"
```

Keep the built-in Bash branch:

```rust
tool_name if tool_name.eq_ignore_ascii_case("bash") => {
    if let Some(action) = input.command().and_then(omp_sandbox_run_action) {
        return Ok(action);
    }
    if let Some(action) = input.command().and_then(omp_process_find_action) {
        return Ok(action);
    }
    Ok(OmpPreToolAction::Block {
        reason: format!(
            "OMP raw {} is denied; only trusted stateful sandbox run or stateful sandbox process find commands are allowed through built-in Bash",
            input.tool_name
        ),
    })
}
```

Keep namespaced Bash support if present for `functions.bash`, because OMP may report built-in Bash under that tool name.

- [ ] **Step 2: Update eval denial text**

Replace denial guidance that says to use removed tools with built-in Bash guidance:

```rust
reason: format!(
    "OMP eval tool {} is denied; use built-in Bash with trusted stateful sandbox run or stateful sandbox process find commands",
    input.tool_name
),
```

- [ ] **Step 3: Run hook red/green tests**

Run:

```bash
CARGO_HOME=$TMPDIR/cargo cargo test -p stateful-cli --test hook omp_removed_generated_command_tools_are_not_allowlisted -- --nocapture
```

Expected: PASS.

Run:

```bash
CARGO_HOME=$TMPDIR/cargo cargo test -p stateful-cli --test hook bash_allows -- --nocapture
```

Expected: PASS; trusted built-in Bash sandbox/process paths still work.

- [ ] **Step 4: Commit hook policy**

```bash
git add crates/stateful-cli/src/hook.rs crates/stateful-cli/tests/hook.rs
git commit -m "refactor: route OMP command execution through built-in Bash"
```

---

### Task 4: Update documentation and installed policy assets

**Files:**
- Modify: `README.md`
- Modify: `docs/usage-reference.md`
- Modify: `docs/architecture.md`
- Modify: `docs/current-state-coordination.md`
- Modify: `docs/implementation-contract.md`
- Modify: `docs/core-concept.md`
- Modify: `crates/stateful-cli/assets/stateful-command-policy/omp-tools.md`
- Modify: `crates/stateful-cli/assets/stateful-command-policy/sandbox-tools.md` if it references removed OMP generated tools
- Modify: `crates/stateful-cli/tests/install_global.rs` if installed asset assertions require removed tool names

**Interfaces:**
- Consumes: final behavior from Tasks 2 and 3.
- Produces: user/agent docs that describe built-in Bash passthrough as the OMP command path.

- [ ] **Step 1: Replace generated tool guidance**

Replace OMP guidance like:

```text
use `sandbox_bash` for non-external sandbox profiles
use `process_find` for process inspection
use `ext_ro_bash` / `ext_rw_bash` for external operations
use `sandbox_job_poll` for background jobs
```

with:

```text
In OMP, use built-in Bash for strict trusted Stateful commands:
`stateful sandbox run ...` for sandboxed command execution and
`stateful sandbox process find ...` for process inspection. Stateful preflight
rejects arbitrary Bash before execution. External write/create/write-dir/socket/signal
scope prompts for a Stateful OMP UI grant unless `stateful.autoApprove: true` is set.
```

- [ ] **Step 2: Update installed OMP tools list**

In `crates/stateful-cli/assets/stateful-command-policy/omp-tools.md`, make the installed tool list contain only:

```text
- Stateful extension
- `rules/stateful-required.md`
- `skills/stateful-command-policy/SKILL.md` plus support files
- `lazy_edit_resume` and `lazy_write_resume`
```

- [ ] **Step 3: Remove `sandbox_job_poll` docs**

Delete instructions that tell agents to poll generated sandbox jobs. Built-in Bash background/PTY/job behavior belongs to OMP's native Bash runtime, not Stateful custom tools.

- [ ] **Step 4: Check for stale references**

Run:

```bash
grep -R "sandbox_bash\|ext_ro_bash\|ext_rw_bash\|process_find\|sandbox_job_poll" README.md docs crates/stateful-cli/assets/stateful-command-policy crates/stateful-cli/tests crates/stateful-cli/src/install.rs
```

Expected: remaining matches are either deleted before commit or intentionally limited to old test names/comments that still describe built-in Bash process-find parsing. Prefer zero references to removed generated tool names in docs/assets/install tests.

Use the repository `grep` tool in OMP sessions instead of shell `grep`; the command above is the conceptual check.

- [ ] **Step 5: Commit docs**

```bash
git add README.md docs/usage-reference.md docs/architecture.md docs/current-state-coordination.md docs/implementation-contract.md docs/core-concept.md crates/stateful-cli/assets/stateful-command-policy/omp-tools.md crates/stateful-cli/assets/stateful-command-policy/sandbox-tools.md crates/stateful-cli/tests/install_global.rs
git commit -m "docs: document OMP built-in Bash command path"
```

---

### Task 5: Final verification

**Files:**
- Modify only if verification exposes a defect in files already touched by Tasks 1-4.

**Interfaces:**
- Consumes: all previous tasks.
- Produces: verified branch ready to push.

- [ ] **Step 1: Format**

Run:

```bash
cargo fmt --all
```

Then:

```bash
CARGO_HOME=$TMPDIR/cargo cargo fmt --all --check
```

Expected: exit code 0.

- [ ] **Step 2: Run full hook test**

Run:

```bash
CARGO_HOME=$TMPDIR/cargo cargo test -p stateful-cli --test hook -- --nocapture
```

Expected: all hook tests pass.

- [ ] **Step 3: Run install-global test**

Run:

```bash
CARGO_HOME=$TMPDIR/cargo cargo test -p stateful-cli --test install_global -- --nocapture
```

Expected: all install-global tests pass.

- [ ] **Step 4: Check whitespace**

Run:

```bash
git diff --check
```

Expected: exit code 0.

- [ ] **Step 5: Commit any verification fixes**

If formatting or fixes changed files, stage only the files reported by `git status --short` from this removal task. For the expected task scope, the command is:

```bash
git add README.md docs/usage-reference.md docs/architecture.md docs/current-state-coordination.md docs/implementation-contract.md docs/core-concept.md crates/stateful-cli/assets/stateful-command-policy/omp-tools.md crates/stateful-cli/assets/stateful-command-policy/sandbox-tools.md crates/stateful-cli/src/install.rs crates/stateful-cli/src/hook.rs crates/stateful-cli/tests/install_global.rs crates/stateful-cli/tests/hook.rs
git commit -m "chore: finalize OMP command tool removal"
```

If no files changed, skip this step.

- [ ] **Step 6: Push**

```bash
git push
```

Expected: branch pushes successfully.
