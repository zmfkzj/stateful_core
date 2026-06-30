# Plan-Only Context Rendering Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make `context_render` a planning/manual-inspection feature and remove automatic denial/pre-tool/sandbox context render calls.

**Architecture:** Keep the server route, MCP tool, MCP resource, and prompt-submit rendering path. Remove speculative `context_render` calls from hook authorization and sandbox denial construction. Update agent guidance so planning may inspect context once, while write and denial recovery follow reservation/claim/next-action primitives.

**Tech Stack:** Rust CLI hooks and tests, Stateful sandbox runner, generated policy assets, Markdown docs.

## Global Constraints

- Keep `/v1/context/render` and the `state_context_render` MCP tool available for explicit planning and manual inspection.
- Stop automatic denial-time calls to `/v1/context/render`.
- Stop automatic `PreToolUse` calls to `/v1/context/render` for Bash, edit, write, and apply_patch authorization.
- Stop sandbox authorization denial bodies from calling `/v1/context/render` to attach `current_state`.
- Keep direct denial payloads actionable without ambient context.
- Keep prompt-time context rendering at most once per session marker where the hook already supports it.
- Update agent-facing guidance so `context_render` is described as planning/manual inspection, not as a required default write-flow step.
- Do not add a new cache in the first pass.
- Do not remove the server endpoint, MCP tool, or MCP resource.

---

## File Structure

- Modify `crates/stateful-cli/tests/hook.rs`: invert tests that currently expect pre-tool or denial live context rendering; retain prompt-submit context rendering coverage.
- Modify `crates/stateful-cli/src/hook.rs`: remove `with_file_tool_live_context` from Bash/edit/write/apply_patch authorization paths; keep user-prompt rendering and manual MCP behavior untouched; update policy reminder strings.
- Modify `crates/stateful-cli/tests/mcp.rs`: invert sandbox write-target denial coverage so no context request and no `current_state` field are expected.
- Modify `crates/stateful-cli/src/sandbox.rs`: remove context rendering from sandbox denial bodies and delete now-unused helper functions.
- Modify `crates/stateful-cli/assets/stateful-command-policy/SKILL.md`: make context inspection optional planning/manual inspection, not default write-flow step 1.
- Modify `crates/stateful-cli/assets/omp-stateful-required-rule.md`: remove blanket “inspect current state first” default and replace it with exact reservation/claim/write guidance plus optional planning inspection.
- Modify `docs/architecture.md`, `docs/current-state-coordination.md`, `docs/state-model.md`, and `docs/usage-reference.md`: describe `context_render` as planning/manual inspection and direct denial recovery as next-action driven.

---

### Task 1: Hook tests define no automatic pre-tool context rendering

**Files:**
- Modify: `crates/stateful-cli/tests/hook.rs:2293-2350`
- Modify: `crates/stateful-cli/tests/hook.rs:3560-3623`
- Modify: `crates/stateful-cli/tests/hook.rs:3843-4106`
- Modify: `crates/stateful-cli/tests/hook.rs:4501-4568`

**Interfaces:**
- Consumes: existing fake server helpers `spawn_fake_stateful_server_sequence`, `spawn_fake_stateful_server`, `run_hook_subprocess`, `request_json_body`.
- Produces: failing tests proving pre-tool and deny paths no longer call `/v1/context/render`.

- [ ] **Step 1: Invert the Bash denial context test**

In `crates/stateful-cli/tests/hook.rs`, rename `pre_tool_use_bash_denial_in_repo_includes_live_context` to:

```rust
fn pre_tool_use_bash_denial_in_repo_does_not_render_live_context()
```

Change its fake server setup from:

```rust
let (runtime, rx) = spawn_fake_stateful_server_sequence(vec![
    r#"{"status":"ok","prompt_text":"Nearby Activity\n- [info] src/auth.ts: Session s2 declared reservation for src/auth.ts."}"#,
]);
```

to:

```rust
let (runtime, rx) = spawn_fake_stateful_server_sequence(vec![]);
```

Replace the context request assertions after the subprocess run with:

```rust
assert!(
    rx.recv_timeout(Duration::from_millis(200)).is_err(),
    "Bash denial should not render live context"
);
let rendered: serde_json::Value =
    serde_json::from_slice(&output.stdout).expect("deny outcome should serialize");
assert_eq!(rendered["hookSpecificOutput"]["permissionDecision"], "deny");
let reason = rendered["hookSpecificOutput"]["permissionDecisionReason"]
    .as_str()
    .expect("deny reason should be text");
assert!(reason.contains("Raw Bash is denied"));
assert!(!reason.contains("Nearby Activity"));
```

- [ ] **Step 2: Invert the Edit allow context test**

Rename `pre_tool_use_edit_posts_authorize_and_renders_live_context_when_server_allows` to:

```rust
fn pre_tool_use_edit_posts_authorize_without_rendering_live_context_when_server_allows()
```

Change the fake server setup to only return the authorization response:

```rust
let (runtime, rx) = spawn_fake_stateful_server_sequence(vec![
    r#"{"decision":"allow","reason_code":"authorized","message":"ok","required_next_action":null}"#,
]);
```

Replace the `context_request` assertions with:

```rust
assert!(
    rx.recv_timeout(Duration::from_millis(200)).is_err(),
    "Edit authorization should not render live context"
);
assert!(
    output.stdout.is_empty(),
    "allowed Edit writes should not inject live context: {}",
    String::from_utf8_lossy(&output.stdout)
);
```

Keep the existing `/v1/authorize`, `write_file`, and `src/auth.ts` assertions.

- [ ] **Step 3: Invert the apply_patch allow context test**

In `pre_tool_use_apply_patch_posts_authorize_and_allows_when_server_allows`, change the fake server setup to only include the authorization response:

```rust
let (runtime, rx) = spawn_fake_stateful_server_sequence(vec![
    r#"{"decision":"allow","reason_code":"authorized","message":"ok","required_next_action":null}"#,
]);
```

Replace the `context_request` block with:

```rust
assert!(
    rx.recv_timeout(Duration::from_millis(200)).is_err(),
    "apply_patch authorization should not render live context"
);
assert!(
    output.stdout.is_empty(),
    "allowed writes should not inject live context: {}",
    String::from_utf8_lossy(&output.stdout)
);
```

Keep the existing request-body assertions for `/v1/authorize`, identity, `queue_on_conflict`, and purpose.

- [ ] **Step 4: Remove warn/block additional-context tests**

Delete these tests entirely because additional context injection is no longer a feature:

```rust
fn pre_tool_use_apply_patch_injects_warn_context_when_server_allows()
fn pre_tool_use_apply_patch_injects_block_context_when_server_allows()
```

Do not replace them with new behavior tests; Step 3 already proves allowed apply_patch writes do not render context or inject `additionalContext`.

- [ ] **Step 5: Invert the apply_patch denial context test**

Rename `pre_tool_use_apply_patch_denial_keeps_info_context` to:

```rust
fn pre_tool_use_apply_patch_denial_does_not_render_live_context()
```

Change the fake server setup to only include the authorization denial:

```rust
let (runtime, rx) = spawn_fake_stateful_server_sequence(vec![
    r#"{"decision":"deny","reason_code":"scope_mismatch","message":"Write target is outside active reservation scope.","required_next_action":"Declare matching reservation."}"#,
]);
```

Replace the context request and reason assertions with:

```rust
assert!(
    rx.recv_timeout(Duration::from_millis(200)).is_err(),
    "apply_patch denial should not render live context"
);
let json: serde_json::Value =
    serde_json::from_slice(&output.stdout).expect("deny outcome should serialize");
assert_eq!(json["hookSpecificOutput"]["permissionDecision"], "deny");
let reason = json["hookSpecificOutput"]["permissionDecisionReason"]
    .as_str()
    .expect("deny reason should be text");
assert!(reason.contains("Write target is outside active reservation scope."));
assert!(reason.contains("Declare matching reservation."));
assert!(!reason.contains("Nearby Activity"));
```

- [ ] **Step 6: Invert the local shadowing denial context test**

In `pre_tool_use_denies_new_dependency_shadowing_python_root_before_authorize`, keep `spawn_fake_stateful_server` only to make a runtime available. Replace the `context_request` assertions with:

```rust
assert!(
    rx.recv_timeout(Duration::from_millis(200)).is_err(),
    "shadowing guard should not post /v1/context/render or /v1/authorize"
);
```

Keep the stdout assertions for `permissionDecision`, `dependency shadowing guard`, `langchain_core`, and `langchain-core`.

- [ ] **Step 7: Run the focused hook tests and verify they fail**

Run:

```bash
cargo test -p stateful-cli --test hook pre_tool_use_bash_denial_in_repo_does_not_render_live_context -- --nocapture
cargo test -p stateful-cli --test hook pre_tool_use_edit_posts_authorize_without_rendering_live_context_when_server_allows -- --nocapture
cargo test -p stateful-cli --test hook pre_tool_use_apply_patch_posts_authorize_and_allows_when_server_allows -- --nocapture
cargo test -p stateful-cli --test hook pre_tool_use_apply_patch_denial_does_not_render_live_context -- --nocapture
cargo test -p stateful-cli --test hook pre_tool_use_denies_new_dependency_shadowing_python_root_before_authorize -- --nocapture
```

Expected: FAIL before implementation because `with_file_tool_live_context` still calls `/v1/context/render`.

- [ ] **Step 8: Commit the failing hook contract tests**

Run:

```bash
git add crates/stateful-cli/tests/hook.rs
git commit -m "test: stop automatic hook context rendering"
```

---

### Task 2: Remove automatic hook live context rendering

**Files:**
- Modify: `crates/stateful-cli/src/hook.rs:1284-1318`
- Modify: `crates/stateful-cli/src/hook.rs:2053-2129`
- Modify: `crates/stateful-cli/src/hook.rs:2155-2198`

**Interfaces:**
- Consumes from Task 1: tests expecting no pre-tool context render calls.
- Produces: hook authorization paths that return direct allow/deny results without calling `/v1/context/render`.

- [ ] **Step 1: Stop wrapping Bash authorization with live context**

In `handle_pre_tool_use_with_runtime`, change the Bash arm from:

```rust
tool_name if tool_name.eq_ignore_ascii_case("bash") => {
    let outcome = authorize_bash(&input)?;
    Ok(with_file_tool_live_context(
        outcome,
        &input,
        runtime,
        identity.as_ref(),
        None,
    ))
}
```

to:

```rust
tool_name if tool_name.eq_ignore_ascii_case("bash") => authorize_bash(&input),
```

- [ ] **Step 2: Stop wrapping apply_patch authorization with live context**

In `authorize_apply_patch`, replace:

```rust
let context_resource = context_resource_for_targets(&targets).map(str::to_owned);
let outcome = authorize_targets(input, runtime, repo_root, targets, identity)?;
Ok(with_file_tool_live_context(
    outcome,
    input,
    runtime,
    identity,
    context_resource.as_deref(),
))
```

with:

```rust
authorize_targets(input, runtime, repo_root, targets, identity)
```

- [ ] **Step 3: Stop wrapping file write authorization with live context**

In `authorize_file_write_tool`, replace:

```rust
let outcome = authorize_targets(
    input,
    runtime,
    repo_root,
    vec![PatchTarget::write(&target)],
    identity,
)?;
Ok(with_file_tool_live_context(
    outcome,
    input,
    runtime,
    identity,
    Some(target.as_str()),
))
```

with:

```rust
authorize_targets(
    input,
    runtime,
    repo_root,
    vec![PatchTarget::write(&target)],
    identity,
)
```

- [ ] **Step 4: Delete unused live context helpers**

After Steps 1-3, remove these functions if the compiler reports they are unused:

```rust
fn with_file_tool_live_context(...)
fn context_response_has_actionable_items(...)
fn context_resource_for_targets(...)
```

Keep these functions because user prompt and manual flows still need them:

```rust
fn render_context_prompt_text(...)
fn render_context_response(...)
fn context_render_request_body(...)
```

- [ ] **Step 5: Run the focused hook tests and verify they pass**

Run:

```bash
cargo test -p stateful-cli --test hook pre_tool_use_bash_denial_in_repo_does_not_render_live_context -- --nocapture
cargo test -p stateful-cli --test hook pre_tool_use_edit_posts_authorize_without_rendering_live_context_when_server_allows -- --nocapture
cargo test -p stateful-cli --test hook pre_tool_use_apply_patch_posts_authorize_and_allows_when_server_allows -- --nocapture
cargo test -p stateful-cli --test hook pre_tool_use_apply_patch_denial_does_not_render_live_context -- --nocapture
cargo test -p stateful-cli --test hook pre_tool_use_denies_new_dependency_shadowing_python_root_before_authorize -- --nocapture
```

Expected: PASS.

- [ ] **Step 6: Run the whole hook test target**

Run:

```bash
cargo test -p stateful-cli --test hook -- --nocapture
```

Expected: PASS. If failures are only old string assertions about context-first guidance, leave those for Task 5.

- [ ] **Step 7: Commit hook implementation**

Run:

```bash
git add crates/stateful-cli/src/hook.rs crates/stateful-cli/tests/hook.rs
git commit -m "fix: remove automatic hook context rendering"
```

---

### Task 3: Remove sandbox denial context rendering

**Files:**
- Modify: `crates/stateful-cli/tests/mcp.rs:358-433`
- Modify: `crates/stateful-cli/src/sandbox.rs:407-413`
- Modify: `crates/stateful-cli/src/sandbox.rs:497-502`
- Modify: `crates/stateful-cli/src/sandbox.rs:3420-3492`

**Interfaces:**
- Consumes: existing sandbox authorization denial body structure.
- Produces: sandbox denial body without `current_state` and without `/v1/context/render` calls.

- [ ] **Step 1: Invert the sandbox denial test**

In `sandbox_run_write_targets_reports_allowed_and_denied_without_running_command`, change the fake server sequence from three responses to two responses:

```rust
let (runtime, rx) = spawn_fake_stateful_server_sequence(vec![
    r#"{"decision":"allow","reason_code":"authorized","message":"ok","required_next_action":null}"#,
    r#"{"decision":"deny","reason_code":"scope_mismatch","message":"Target is outside active reservation scope.","required_next_action":"Declare matching reservation."}"#,
]);
```

Remove the `context` receive block:

```rust
let context = rx
    .recv_timeout(Duration::from_secs(1))
    .expect("denial should render current state context");
...
assert!(context.contains("POST /v1/context/render HTTP/1.1"));
assert!(context.contains("\"session_id\":\"s-current\""));
```

After the second request assertions, add:

```rust
assert!(
    rx.recv_timeout(Duration::from_millis(200)).is_err(),
    "sandbox denial should not render current state context"
);
```

Replace stdout assertions:

```rust
assert!(stdout.contains("\"allowed_write_targets\":[\"src/allowed.ts\"]"));
assert!(stdout.contains("\"path\":\"src/denied.ts\""));
assert!(stdout.contains("\"decision\":\"deny\""));
assert!(!stdout.contains("\"current_state\""));
assert!(!stdout.contains("Your Active Scope"));
```

- [ ] **Step 2: Run the focused sandbox test and verify it fails**

Run:

```bash
cargo test -p stateful-cli --test mcp sandbox_run_write_targets_reports_allowed_and_denied_without_running_command -- --nocapture
```

Expected: FAIL before implementation because sandbox denial still calls `/v1/context/render` and includes `current_state`.

- [ ] **Step 3: Simplify sandbox denial body**

In `crates/stateful-cli/src/sandbox.rs`, replace `sandbox_authorization_denied_body` with:

```rust
fn sandbox_authorization_denied_body(
    allowed_write_targets: Vec<String>,
    denied_write_targets: Vec<serde_json::Value>,
) -> serde_json::Value {
    serde_json::json!({
        "status": "error",
        "message": "stateful sandbox run target authorization denied",
        "allowed_write_targets": allowed_write_targets,
        "denied_write_targets": denied_write_targets,
    })
}
```

Update both call sites from:

```rust
let body = sandbox_authorization_denied_body(
    &authorize_context,
    allowed_write_targets,
    denied_write_targets,
)
.to_string();
```

to:

```rust
let body = sandbox_authorization_denied_body(allowed_write_targets, denied_write_targets).to_string();
```

- [ ] **Step 4: Delete unused sandbox context helpers**

Remove these functions if they are no longer referenced:

```rust
fn denied_write_targets_context_resource(...)
fn sandbox_current_state_request_body(...)
fn sandbox_current_state_context(...)
```

Also remove now-unused imports from `sandbox.rs`, including `repo_identity_for_enabled_repo` only if the compiler confirms no remaining references in the file.

- [ ] **Step 5: Run focused sandbox test and verify it passes**

Run:

```bash
cargo test -p stateful-cli --test mcp sandbox_run_write_targets_reports_allowed_and_denied_without_running_command -- --nocapture
```

Expected: PASS.

- [ ] **Step 6: Run the whole mcp test target**

Run:

```bash
cargo test -p stateful-cli --test mcp -- --nocapture
```

Expected: PASS. If failures are old context-render string expectations, update them to the new plan-only behavior in this task before committing.

- [ ] **Step 7: Commit sandbox changes**

Run:

```bash
git add crates/stateful-cli/src/sandbox.rs crates/stateful-cli/tests/mcp.rs
git commit -m "fix: remove sandbox denial context rendering"
```

---

### Task 4: Keep prompt and manual context rendering covered

**Files:**
- Modify: `crates/stateful-cli/tests/hook.rs:5319-5398`
- Modify: `crates/stateful-cli/tests/mcp.rs:1763-1800`
- Modify if needed: `crates/stateful-cli/src/hook.rs:929-1108`
- Modify if needed: `crates/stateful-cli/src/mcp.rs:569-664`

**Interfaces:**
- Consumes: Task 2 and Task 3 removal of denial/pre-tool/sandbox auto renders.
- Produces: retained coverage that prompt-submit and manual MCP context rendering still work.

- [ ] **Step 1: Preserve prompt-submit once-per-session coverage**

In `user_prompt_submit_posts_context_render`, keep assertions that first prompt submit posts `/v1/context/render`:

```rust
assert!(request.contains("POST /v1/context/render HTTP/1.1"));
assert!(request.contains("\"mode\":\"brief\""));
assert!(request.contains("\"workspace_id\":\"w1\""));
assert!(request.contains("\"repo_id\""));
assert!(request.contains("\"worktree_id\""));
assert!(request.contains("\"root\""));
```

Keep the second-submit assertion:

```rust
assert!(
    rx.recv_timeout(Duration::from_millis(200)).is_err(),
    "second UserPromptSubmit should not call /v1/context/render"
);
```

Only update reminder string assertions that Task 5 changes; do not weaken the context render request assertions.

- [ ] **Step 2: Preserve manual MCP context render coverage**

In `crates/stateful-cli/tests/mcp.rs`, keep `mcp_context_render_defaults_to_current_session_workspace` asserting:

```rust
assert!(request.contains("POST /v1/context/render HTTP/1.1"));
assert!(request.contains("Authorization: Bearer secret-token"));
let body = request_json_body(&request);
assert_eq!(body["session_id"], "s1");
assert_eq!(body["workspace_id"], "w1");
assert_eq!(body["mode"], "brief");
assert_eq!(body["resource"], "src/auth.ts");
```

If this test fails after earlier tasks, fix the implementation rather than deleting the test.

- [ ] **Step 3: Run retained context tests**

Run:

```bash
cargo test -p stateful-cli --test hook user_prompt_submit_posts_context_render -- --nocapture
cargo test -p stateful-cli --test mcp mcp_context_render_defaults_to_current_session_workspace -- --nocapture
```

Expected: PASS.

- [ ] **Step 4: Commit retained coverage adjustments if any**

If no files changed in this task, skip the commit. If reminder string assertions changed here, run:

```bash
git add crates/stateful-cli/tests/hook.rs crates/stateful-cli/tests/mcp.rs crates/stateful-cli/src/hook.rs crates/stateful-cli/src/mcp.rs
git commit -m "test: retain planning context render coverage"
```

---

### Task 5: Update agent guidance and docs

**Files:**
- Modify: `crates/stateful-cli/src/hook.rs:1110-1123`
- Modify: `crates/stateful-cli/src/hook.rs:1738-1742`
- Modify: `crates/stateful-cli/assets/stateful-command-policy/SKILL.md:16-23`
- Modify: `crates/stateful-cli/assets/omp-stateful-required-rule.md:18-24`
- Modify: `crates/stateful-cli/tests/hook.rs:340-350`
- Modify: `crates/stateful-cli/tests/hook.rs:1845-1853`
- Modify: `crates/stateful-cli/tests/hook.rs:5355-5368`
- Modify: `docs/architecture.md:632-636`
- Modify: `docs/current-state-coordination.md:936-945`
- Modify: `docs/state-model.md:707-711`
- Modify: `docs/usage-reference.md:348-375`

**Interfaces:**
- Consumes: behavior from Tasks 2-4.
- Produces: guidance that makes context rendering optional planning/manual inspection and keeps write/denial recovery focused on reservation, claim, resume, and denial next actions.

- [ ] **Step 1: Update `stateful-command-policy` default write flow**

In `crates/stateful-cli/assets/stateful-command-policy/SKILL.md`, replace the `## Default Write Flow` numbered list with:

```markdown
## Default Write Flow

1. For planning, inspect context once only when active coordination may affect the plan. When target paths are known, `state_context_render(mode="brief", resource="<target>")` is optional planning/manual inspection; use broad `state_current_read` only before targets are known or when assigning parallel work.
2. Declare the task file set with `state_reservation_declare(purpose=<task purpose>, files_planned=[...])`.
3. Acquire exact same-session file claims with `state_claim_acquire(paths=[...])` before native edits, deletes, moves, renames, or repo-relative command writes.
4. Keep paths narrow. Directory claims authorize only `write_directory`; exact file writes still need exact file reservation and claim.
5. Re-read files immediately before native edits. Native edits and write-target sandbox writes release authorized claims after the transaction; reacquire before another write.
6. For hook denials, follow the denial's next action or `denial-recovery.md`; do not call `state_context_render` unless you need to revise the plan.
7. Use canonical Stateful MCP tool names in guidance: `state_context_render`, `state_current_read`, `state_session_register`, `state_reservation_declare`, `state_claim_acquire`, `state_reservation_request`, `state_notifications_poll`, `state_resume_next`, and `state_reservation_claim`. If the active tool list exposes only runtime-specific tool names, call the exact shown equivalent. Runtime-specific wrappers are aliases, not the API.
```

- [ ] **Step 2: Update OMP required rule defaults**

In `crates/stateful-cli/assets/omp-stateful-required-rule.md`, replace the non-negotiable default at line 20 with:

```markdown
- Use `state_context_render(mode="brief", resource="<target>")` only for planning/manual inspection when active coordination may affect the plan. Do not call it as routine denial recovery; follow the denial's next action instead.
```

Keep the reservation/claim, tool mapping, and denial-action bullets.

- [ ] **Step 3: Update prompt reminder string**

In `stateful_command_policy_reminder()` in `crates/stateful-cli/src/hook.rs`, replace the first bullet with:

```text
- Use `state_context_render` only for planning/manual inspection when active coordination may affect the plan; if you already inspected this turn for the same resource, reuse that result.
```

Ensure the reminder still includes:

```text
stateful-command-policy
state_reservation_declare
state_claim_acquire
Do not run `stateful reservation declare`
--fs read-only --network disabled
--fs build --network enabled
--fs git --network disabled
--fs github-pr --network enabled
--fs write-targets --write-target <file>
```

- [ ] **Step 4: Update Bash denial guidance string**

In `bash_policy_guidance()` in `crates/stateful-cli/src/hook.rs`, replace the opening sentence:

```text
Inspect current state first with `state_current_read` or `state_context_render`, then use the `stateful-command-policy` skill before Bash or eval tools.
```

with:

```text
Use the `stateful-command-policy` skill before Bash or eval tools; use `state_context_render` only for planning/manual inspection when active coordination may affect the plan.
```

Keep the raw Bash denial alternatives and sandbox command examples unchanged.

- [ ] **Step 5: Update hook reminder tests**

Update assertions in `crates/stateful-cli/tests/hook.rs` so they no longer require context-first wording. Keep assertions for policy, reservation, claim, and sandbox alternatives.

For `user_prompt_submit_posts_context_render`, replace:

```rust
assert!(rendered.contains("Before using Bash"));
```

with:

```rust
assert!(rendered.contains("planning/manual inspection"));
```

If any tests assert `state_current_read` solely because the old first bullet mentioned it, replace that assertion with:

```rust
assert!(reason.contains("state_context_render"));
assert!(reason.contains("planning/manual inspection"));
```

- [ ] **Step 6: Update architecture docs**

In `docs/architecture.md`, replace lines 632-636 with:

```markdown
`state_context_render` supports `brief` and `detailed` modes plus an optional
singular `resource` filter. `brief` is for session start, prompt submit context,
and planning-time known-target resource checks. `detailed` is for manual deep
inspection when planning context lacks enough evidence. Denial recovery should
follow the denial's direct next action rather than rendering ambient context.
```

- [ ] **Step 7: Update current-state coordination docs**

In `docs/current-state-coordination.md`, replace the paragraph around lines 942-945 with:

```markdown
`brief` is used for session start, user prompt context, and planning-time
known-target resource checks. `detailed` is used for manual deep inspection when
brief planning context lacks enough evidence. Rendering should be actionable,
not a raw event dump, and denial recovery should follow direct next-action
payloads instead of automatically rendering context.
```

- [ ] **Step 8: Update state model docs**

In `docs/state-model.md`, replace the context render paragraph around lines 707-711 with:

```markdown
The current server exposes `/v1/context/render` and `state_context_render` as a
store-backed planning/manual inspection view over active reservations, active
claims, and queued or claimable (`reserved`) wait records. Responses include
current summary counts, structured `items`, and prompt-ready `prompt_text`; an
empty unfiltered live render means no planning context needs to be shown.
```

- [ ] **Step 9: Update usage reference surface description**

In `docs/usage-reference.md`, after the MCP tool list entry for `state_context_render`, add:

```markdown
`state_context_render` is for planning/manual inspection. Routine write and
denial recovery should use reservation, claim, resume, and the denial's direct
next action instead of rendering ambient context.
```

- [ ] **Step 10: Run guidance and docs tests**

Run:

```bash
cargo test -p stateful-cli --test hook user_prompt_submit_posts_context_render -- --nocapture
cargo test -p stateful-cli --test hook pre_tool_use_bash_denial_in_repo_does_not_render_live_context -- --nocapture
cargo test -p stateful-cli --test install_global -- --nocapture
```

Expected: PASS.

- [ ] **Step 11: Commit guidance/docs changes**

Run:

```bash
git add crates/stateful-cli/src/hook.rs crates/stateful-cli/assets/stateful-command-policy/SKILL.md crates/stateful-cli/assets/omp-stateful-required-rule.md crates/stateful-cli/tests/hook.rs docs/architecture.md docs/current-state-coordination.md docs/state-model.md docs/usage-reference.md
git commit -m "docs: make context rendering plan-only"
```

---

### Task 6: Final targeted verification

**Files:**
- No source edits expected unless a verification failure exposes a missed context-render expectation.

**Interfaces:**
- Consumes: all prior tasks.
- Produces: final evidence that automatic context rendering was removed while prompt/manual rendering remains.

- [ ] **Step 1: Run hook tests**

Run:

```bash
cargo test -p stateful-cli --test hook -- --nocapture
```

Expected: PASS.

- [ ] **Step 2: Run MCP/sandbox tests**

Run:

```bash
cargo test -p stateful-cli --test mcp -- --nocapture
```

Expected: PASS.

- [ ] **Step 3: Run server route tests for context render**

Run:

```bash
cargo test -p stateful-server --test routes context_render -- --nocapture
```

Expected: PASS. This confirms the endpoint remains available.

- [ ] **Step 4: Run MCP crate tests for context tool mapping**

Run:

```bash
cargo test -p stateful-mcp --test tools context_render_tool_maps_to_http_endpoint -- --nocapture
```

Expected: PASS. This confirms `state_context_render` still maps to `POST /v1/context/render`.

- [ ] **Step 5: Run formatter**

Run:

```bash
cargo fmt
```

Expected: no output or formatting updates only in touched Rust files.

- [ ] **Step 6: Run focused clippy if available in the repo workflow**

Run:

```bash
cargo clippy -p stateful-cli -p stateful-server -p stateful-mcp --tests -- -D warnings
```

Expected: PASS. If clippy is too broad for the active environment, run the four test commands above plus `cargo test -p stateful-cli --test install_global -- --nocapture` and report clippy as not run with the exact blocker.

- [ ] **Step 7: Commit verification fixes if any**

If verification required source or test fixes, run:

```bash
git add <exact changed files>
git commit -m "fix: align context rendering verification"
```

If no files changed, skip this commit.

---

## Self-Review

- Spec coverage: Tasks 1-3 remove automatic denial/pre-tool/sandbox calls; Task 4 preserves prompt/manual rendering; Task 5 updates agent guidance/docs; Task 6 verifies endpoint/tool availability.
- Placeholder scan: no `TBD`, `TODO`, or unspecified test commands remain.
- Type/name consistency: `state_context_render`, `/v1/context/render`, `with_file_tool_live_context`, `sandbox_authorization_denied_body`, and test names are used consistently across tasks.
