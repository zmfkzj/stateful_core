# Benchmark Overhead Reduction Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Reduce DeNovo/OMP `stateful:on` overhead from verbose denial recovery, unsummarized stateful output, repeated same-path denials, and empty-stop retry churn without weakening authorization.

**Architecture:** Keep policy decisions fail-closed and unchanged. Reduce output at existing boundaries: stale denial next-action strings, brief context rendering, hook-local repeated denial formatting, and benchmark/OMP empty-output handling.

**Tech Stack:** Rust 2024 workspace (`stateful-core`, `stateful-server`, `stateful-cli`, `stateful-bench`), Python 3 benchmark scripts, generated OMP extension JavaScript embedded in Rust string templates.

## Global Constraints

- Keep Stateful authorization fail-closed.
- Do not turn denied writes into allowed writes.
- Do not steal claims or cancel other sessions.
- Preserve existing `reason_code` values where callers may rely on them.
- Keep detailed context rendering available for manual inspection.
- Add tests before implementation for every behavior change.
- Prefer the shortest implementation that works; no new protocol layer unless the existing hook/server seams cannot carry the behavior.
- No new dependency.

---

## File Structure

- Modify: `crates/stateful-server/src/policy_service.rs`
  - Owns stale-target/stale-claim `Decision::deny` next-action text.
- Modify: `crates/stateful-server/tests/routes.rs`
  - Verifies stale denial JSON contract through the server route.
- Modify: `crates/stateful-core/src/context.rs`
  - Owns `ContextPackage` prompt rendering and brief/detailed output.
- Modify: `crates/stateful-core/tests/context.rs`
  - Verifies summary-first brief rendering and detailed behavior preservation.
- Modify: `crates/stateful-cli/src/hook.rs`
  - Owns hook denial formatting and hook-local repeated-denial state.
- Modify: `crates/stateful-cli/tests/hook.rs`
  - Verifies concise/repeated/wait denial hook behavior.
- Modify: `crates/stateful-bench/scripts/codex_pair_agent.py`
  - Owns Codex retry loop and output event parsing.
- Modify: `crates/stateful-bench/scripts/denovo_codex_agent.py`
  - Owns DeNovo finish reason mapping and instance result creation.
- Modify: `crates/stateful-bench/tests/cli.rs`
  - Verifies Python adapter behavior via existing import/subprocess test style.
- Modify: `crates/stateful-cli/src/install.rs`
  - Owns generated OMP JavaScript tool output formatting.

---

### Task 1: Concise Stale Denial Recovery

**Files:**
- Modify: `crates/stateful-server/src/policy_service.rs:998-1011`
- Modify: `crates/stateful-server/tests/routes.rs`
- Modify: `crates/stateful-cli/src/hook.rs:2354-2385`
- Test: `crates/stateful-server/tests/routes.rs`
- Test: `crates/stateful-cli/tests/hook.rs`

**Interfaces:**
- Consumes: existing `Decision::deny(reason_code, message, required_next_action)`.
- Produces: unchanged `reason_code` values with shorter `required_next_action` strings.

- [ ] **Step 1: Add failing server tests for concise stale next actions**

Add two tests in `crates/stateful-server/tests/routes.rs` near existing authorization stale-observation tests. If equivalent route helpers already exist, reuse them instead of copying setup.

```rust
#[tokio::test]
async fn stale_base_observation_uses_concise_recovery_action() {
    let app = build_router(ServerConfig::new("secret-token"));
    let root = tempfile::tempdir().expect("temp root");
    let file = root.path().join("src").join("pkg.py");
    std::fs::create_dir_all(file.parent().unwrap()).expect("parent dir");
    std::fs::write(&file, "new contents").expect("target file");

    let body = serde_json::json!({
        "session_id": "s1",
        "workspace_id": "w1",
        "repo_id": "repo-1",
        "worktree_id": "wt-1",
        "root": root.path(),
        "action": "write_file",
        "path": "src/pkg.py",
        "base_observations": [{
            "path": "src/pkg.py",
            "exists": false
        }]
    });

    let response = app
        .oneshot(json_request("POST", "/v1/authorize", body, "secret-token"))
        .await
        .expect("authorize response");
    let json = response_json(response, 4096).await;

    assert_eq!(json["decision"], "deny");
    assert_eq!(json["reason_code"], "stale_target_observation");
    assert_eq!(
        json["required_next_action"],
        "Reread target, retry same edit with fresh base observation."
    );
}

#[tokio::test]
async fn stale_claim_observation_uses_concise_recovery_action() {
    let app = build_router(ServerConfig::new("secret-token"));
    let root = tempfile::tempdir().expect("temp root");
    let file = root.path().join("src").join("pkg.py");
    std::fs::create_dir_all(file.parent().unwrap()).expect("parent dir");
    std::fs::write(&file, "old contents").expect("target file");

    let declare = app.clone().oneshot(json_request(
        "POST",
        "/v1/reservation/declare",
        serde_json::json!({
            "session_id": "s1",
            "workspace_id": "w1",
            "repo_id": "repo-1",
            "worktree_id": "wt-1",
            "root": root.path(),
            "purpose": "Fix stale file",
            "files_planned": ["src/pkg.py"]
        }),
        "secret-token",
    )).await.expect("declare response");
    assert!(declare.status().is_success());

    let claim = app.clone().oneshot(json_request(
        "POST",
        "/v1/claim/acquire",
        serde_json::json!({
            "session_id": "s1",
            "workspace_id": "w1",
            "repo_id": "repo-1",
            "worktree_id": "wt-1",
            "root": root.path(),
            "paths": ["src/pkg.py"]
        }),
        "secret-token",
    )).await.expect("claim response");
    assert!(claim.status().is_success());

    std::fs::write(&file, "changed outside claim").expect("mutate target");

    let response = app.oneshot(json_request(
        "POST",
        "/v1/authorize",
        serde_json::json!({
            "session_id": "s1",
            "workspace_id": "w1",
            "repo_id": "repo-1",
            "worktree_id": "wt-1",
            "root": root.path(),
            "action": "write_file",
            "path": "src/pkg.py"
        }),
        "secret-token",
    )).await.expect("authorize response");
    let json = response_json(response, 4096).await;

    assert_eq!(json["decision"], "deny");
    assert_eq!(json["reason_code"], "stale_claim_observation");
    assert_eq!(
        json["required_next_action"],
        "Reread target, reacquire claim, retry same edit."
    );
}
```

- [ ] **Step 2: Run server tests to verify RED**

Run:

```bash
cargo test -p stateful-server stale_base_observation_uses_concise_recovery_action stale_claim_observation_uses_concise_recovery_action
```

Expected: both tests fail because the old longer next-action strings are returned.

- [ ] **Step 3: Implement concise stale next actions**

Change `crates/stateful-server/src/policy_service.rs`:

```rust
fn stale_observation_decision(message: &str) -> Decision {
    Decision::deny(
        "stale_target_observation",
        message,
        "Reread target, retry same edit with fresh base observation.",
    )
}

fn stale_claim_observation_decision(message: &str) -> Decision {
    Decision::deny(
        "stale_claim_observation",
        message,
        "Reread target, reacquire claim, retry same edit.",
    )
}
```

Do not change `reason_code`.

- [ ] **Step 4: Run server tests to verify GREEN**

Run:

```bash
cargo test -p stateful-server stale_base_observation_uses_concise_recovery_action stale_claim_observation_uses_concise_recovery_action
```

Expected: PASS.

- [ ] **Step 5: Add hook-level wait preservation test**

In `crates/stateful-cli/tests/hook.rs`, add this test near the existing apply-patch denial tests. Reuse the existing fake-server helpers in that file.

```rust
#[test]
fn pre_tool_use_apply_patch_wait_denial_keeps_resume_guidance() {
    let temp_root = std::env::temp_dir().join(format!(
        "stateful-hook-wait-denial-test-{}",
        std::process::id()
    ));
    if temp_root.exists() {
        fs::remove_dir_all(&temp_root).expect("old temp root should be removable");
    }
    let paths = GlobalPaths::new(temp_root.join("home"));
    let repo_root = temp_root.join("repo");
    fs::create_dir_all(&repo_root).expect("repo root should be creatable");
    enable_test_repo(&paths, &repo_root);
    let (runtime, _rx) = spawn_fake_stateful_server(r#"{
        "decision":"deny",
        "reason_code":"active_claim_conflict",
        "message":"Write target is covered by another active session claim.",
        "required_next_action":"Wait for the claim to release.",
        "wait":{"wait_id":"wait-1","path":"src/auth.ts","queue_position":1,"blocking_session_id":"s2"}
    }"#);
    write_global_runtime_file(&paths, &runtime).expect("global runtime file should write");

    let input = r#"{
      "session_id": "s1",
      "cwd": "/repo",
      "hook_event_name": "PreToolUse",
      "tool_name": "apply_patch",
      "tool_input": {
        "command": "*** Begin Patch\n*** Update File: src/auth.ts\n*** End Patch\n"
      }
    }"#;

    let output = run_hook_subprocess(&repo_root, &paths, &["hook", "codex", "pre-tool-use"], input);
    assert!(output.status.success(), "stateful hook failed: {}", String::from_utf8_lossy(&output.stderr));
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("wait_id wait-1"), "{stdout}");
    assert!(stdout.contains("state_resume_next"), "{stdout}");

    fs::remove_dir_all(&temp_root).expect("temp root should be removable");
}
```

- [ ] **Step 6: Run hook test**

Run:

```bash
cargo test -p stateful-cli pre_tool_use_apply_patch_wait_denial_keeps_resume_guidance
```

Expected: PASS before and after the stale text change; wait guidance remains in `authorization_denial_reason`.

- [ ] **Step 7: Commit Task 1**

```bash
git add crates/stateful-server/src/policy_service.rs crates/stateful-server/tests/routes.rs crates/stateful-cli/tests/hook.rs
git commit -m "Reduce stale denial recovery text"
```

---

### Task 2: Summary-First Brief Context Rendering

**Files:**
- Modify: `crates/stateful-core/src/context.rs:278-470`
- Modify: `crates/stateful-core/tests/context.rs`
- Test: `crates/stateful-core/tests/context.rs`

**Interfaces:**
- Consumes: `ContextPackage`, `CurrentItem`, `RenderMode`.
- Produces: `render_prompt_text(package, RenderMode::Brief)` beginning with `Stateful summary:` when package has items.

- [ ] **Step 1: Add failing summary-first tests**

Append tests to `crates/stateful-core/tests/context.rs`:

```rust
#[test]
fn brief_context_starts_with_stateful_summary() {
    let package = ContextPackage::from_items(vec![
        CurrentItem::new(
            CurrentItemKind::Claim,
            CurrentSeverity::Block,
            CurrentFreshness::Live,
            "src/auth.ts",
            "Fix auth validation behavior.",
            "Session s2 has an active write claim.",
        )
        .with_next_action("Wait for s2 to release the claim."),
        CurrentItem::new(
            CurrentItemKind::Reservation,
            CurrentSeverity::Info,
            CurrentFreshness::Live,
            "src/cache.ts",
            "Warm cache cleanup.",
            "Session s1 declared reservation for src/cache.ts.",
        ),
    ]);

    let text = render_prompt_text(&package, RenderMode::Brief);

    assert!(text.starts_with("Stateful summary:"), "{text}");
    assert!(text.contains("1 blocking"), "{text}");
    assert!(text.contains("1 info"), "{text}");
    assert!(text.contains("Blocking"), "{text}");
    assert!(text.contains("Required Next Action"), "{text}");
}

#[test]
fn brief_context_omits_info_purpose_lines_after_summary() {
    let package = ContextPackage::from_items(vec![CurrentItem::new(
        CurrentItemKind::Reservation,
        CurrentSeverity::Info,
        CurrentFreshness::Live,
        "src/cache.ts",
        "Warm cache cleanup.",
        "Session s1 declared reservation for src/cache.ts.",
    )]);

    let text = render_prompt_text(&package, RenderMode::Brief);

    assert!(text.starts_with("Stateful summary:"), "{text}");
    assert!(text.contains("Nearby Activity"), "{text}");
    assert!(!text.contains("purpose: Warm cache cleanup"), "{text}");
}

#[test]
fn detailed_context_keeps_info_purpose_lines() {
    let package = ContextPackage::from_items(vec![CurrentItem::new(
        CurrentItemKind::Reservation,
        CurrentSeverity::Info,
        CurrentFreshness::Live,
        "src/cache.ts",
        "Warm cache cleanup.",
        "Session s1 declared reservation for src/cache.ts.",
    )]);

    let text = render_prompt_text(&package, RenderMode::Detailed);

    assert!(text.contains("purpose: Warm cache cleanup"), "{text}");
}
```

- [ ] **Step 2: Run context tests to verify RED**

Run:

```bash
cargo test -p stateful-core brief_context_starts_with_stateful_summary brief_context_omits_info_purpose_lines_after_summary detailed_context_keeps_info_purpose_lines
```

Expected: first two fail because no summary exists and brief mode still renders info purposes.

- [ ] **Step 3: Implement summary helper**

Add near `render_prompt_text` in `crates/stateful-core/src/context.rs`:

```rust
fn render_summary(output: &mut String, package: &ContextPackage) {
    if package.items.is_empty() {
        return;
    }
    let live = package
        .items
        .iter()
        .filter(|item| item.freshness == CurrentFreshness::Live)
        .count();
    let blocking = package
        .items
        .iter()
        .filter(|item| {
            item.freshness == CurrentFreshness::Live && item.severity == CurrentSeverity::Block
        })
        .count();
    let warnings = package
        .items
        .iter()
        .filter(|item| {
            item.freshness == CurrentFreshness::Live && item.severity == CurrentSeverity::Warn
        })
        .count();
    let info = package
        .items
        .iter()
        .filter(|item| {
            item.freshness == CurrentFreshness::Live && item.severity == CurrentSeverity::Info
        })
        .count();
    let stale = package
        .items
        .iter()
        .filter(|item| item.freshness != CurrentFreshness::Live)
        .count();

    output.push_str(&format!(
        "Stateful summary: {live} live, {blocking} blocking, {warnings} warning, {info} info, {stale} stale.\n"
    ));
}
```

At the start of `render_prompt_text`, after `rendered` initialization, call it for brief mode:

```rust
if matches!(mode, RenderMode::Brief) {
    render_summary(&mut output, package);
}
```

- [ ] **Step 4: Make brief info sections omit purpose lines**

Change `render_section` signature to include a boolean:

```rust
fn render_section(
    output: &mut String,
    title: &str,
    items: &[&CurrentItem],
    mode: RenderMode,
    max_total: usize,
    rendered: &mut usize,
    show_info_purpose: bool,
) {
```

Update every existing `render_section` call to pass the new `show_info_purpose` boolean:

- `Your Active Scope`, `Blocking`, and `Warnings`: pass `true`.
- `Nearby Activity`: pass `!matches!(mode, RenderMode::Brief)`.
- `Stale/Expired`: pass `!matches!(mode, RenderMode::Brief)`.

Inside `render_section`, replace the unconditional purpose write with:

```rust
if item.severity != CurrentSeverity::Info || show_info_purpose {
    output.push_str(&format!(
        "  purpose: {}\n",
        trim_trailing_period(&item.purpose)
    ));
}
```

- [ ] **Step 5: Run context tests to verify GREEN**

Run:

```bash
cargo test -p stateful-core context
```

Expected: PASS.

- [ ] **Step 6: Commit Task 2**

```bash
git add crates/stateful-core/src/context.rs crates/stateful-core/tests/context.rs
git commit -m "Render brief stateful context summary first"
```

---

### Task 3: Repeated Same-Path Denial Single-Writer Guidance

**Files:**
- Modify: `crates/stateful-cli/src/hook.rs`
- Modify: `crates/stateful-cli/tests/hook.rs`
- Test: `crates/stateful-cli/tests/hook.rs`

**Interfaces:**
- Consumes: `AuthorizeDecision` and hook input values already available during pre-tool authorization.
- Produces: second same denial emits single-writer guidance while still blocking the write.

- [ ] **Step 1: Add failing hook test for repeated denial**

Add to `crates/stateful-cli/tests/hook.rs` near apply-patch denial tests:

```rust
#[test]
fn pre_tool_use_apply_patch_repeated_same_path_denial_suggests_single_writer() {
    let temp_root = std::env::temp_dir().join(format!(
        "stateful-hook-repeat-denial-test-{}",
        std::process::id()
    ));
    if temp_root.exists() {
        fs::remove_dir_all(&temp_root).expect("old temp root should be removable");
    }
    let paths = GlobalPaths::new(temp_root.join("home"));
    let repo_root = temp_root.join("repo");
    fs::create_dir_all(&repo_root).expect("repo root should be creatable");
    enable_test_repo(&paths, &repo_root);
    let (runtime, _rx) = spawn_fake_stateful_server_sequence(vec![
        r#"{"decision":"deny","reason_code":"stale_target_observation","message":"Target content changed since the supplied base observation.","required_next_action":"Reread target, retry same edit with fresh base observation."}"#,
        r#"{"decision":"deny","reason_code":"stale_target_observation","message":"Target content changed since the supplied base observation.","required_next_action":"Reread target, retry same edit with fresh base observation."}"#,
    ]);
    write_global_runtime_file(&paths, &runtime).expect("global runtime file should write");

    let input = r#"{
      "session_id": "s1",
      "cwd": "/repo",
      "hook_event_name": "PreToolUse",
      "tool_name": "apply_patch",
      "tool_input": {
        "command": "*** Begin Patch\n*** Update File: src/auth.ts\n*** End Patch\n"
      }
    }"#;

    let first = run_hook_subprocess(&repo_root, &paths, &["hook", "codex", "pre-tool-use"], input);
    assert!(first.status.success());
    let first_stdout = String::from_utf8_lossy(&first.stdout);
    assert!(first_stdout.contains("Reread target"), "{first_stdout}");
    assert!(!first_stdout.contains("Use one writer"), "{first_stdout}");

    let second = run_hook_subprocess(&repo_root, &paths, &["hook", "codex", "pre-tool-use"], input);
    assert!(second.status.success());
    let second_stdout = String::from_utf8_lossy(&second.stdout);
    assert!(second_stdout.contains("Repeated denial for src/auth.ts"), "{second_stdout}");
    assert!(second_stdout.contains("Use one writer"), "{second_stdout}");

    fs::remove_dir_all(&temp_root).expect("temp root should be removable");
}
```

- [ ] **Step 2: Add non-repeat control test**

```rust
#[test]
fn pre_tool_use_apply_patch_different_path_denial_does_not_trigger_single_writer() {
    let temp_root = std::env::temp_dir().join(format!(
        "stateful-hook-repeat-control-test-{}",
        std::process::id()
    ));
    if temp_root.exists() {
        fs::remove_dir_all(&temp_root).expect("old temp root should be removable");
    }
    let paths = GlobalPaths::new(temp_root.join("home"));
    let repo_root = temp_root.join("repo");
    fs::create_dir_all(&repo_root).expect("repo root should be creatable");
    enable_test_repo(&paths, &repo_root);
    let (runtime, _rx) = spawn_fake_stateful_server_sequence(vec![
        r#"{"decision":"deny","reason_code":"stale_target_observation","message":"Target changed.","required_next_action":"Reread target, retry same edit with fresh base observation."}"#,
        r#"{"decision":"deny","reason_code":"stale_target_observation","message":"Target changed.","required_next_action":"Reread target, retry same edit with fresh base observation."}"#,
    ]);
    write_global_runtime_file(&paths, &runtime).expect("global runtime file should write");

    let first_input = r#"{"session_id":"s1","cwd":"/repo","hook_event_name":"PreToolUse","tool_name":"apply_patch","tool_input":{"command":"*** Begin Patch\n*** Update File: src/auth.ts\n*** End Patch\n"}}"#;
    let second_input = r#"{"session_id":"s1","cwd":"/repo","hook_event_name":"PreToolUse","tool_name":"apply_patch","tool_input":{"command":"*** Begin Patch\n*** Update File: src/session.ts\n*** End Patch\n"}}"#;

    let first = run_hook_subprocess(&repo_root, &paths, &["hook", "codex", "pre-tool-use"], first_input);
    assert!(first.status.success());
    let second = run_hook_subprocess(&repo_root, &paths, &["hook", "codex", "pre-tool-use"], second_input);
    assert!(second.status.success());
    let second_stdout = String::from_utf8_lossy(&second.stdout);
    assert!(!second_stdout.contains("Use one writer"), "{second_stdout}");

    fs::remove_dir_all(&temp_root).expect("temp root should be removable");
}
```

- [ ] **Step 3: Run hook tests to verify RED**

Run:

```bash
cargo test -p stateful-cli repeated_same_path_denial different_path_denial
```

Expected: repeated-denial test fails because no dedupe state exists.

- [ ] **Step 4: Implement hook-local denial counter**

In `crates/stateful-cli/src/hook.rs`, add helpers near prompt context marker helpers:

```rust
fn repeated_denial_marker_path(
    repo_root: &Path,
    session_id: &str,
    path: &str,
    reason_code: &str,
) -> PathBuf {
    let safe_path = path.replace(['/', '\\', ':'], "_");
    let safe_reason = reason_code.replace(['/', '\\', ':'], "_");
    repo_root
        .join(".stateful_core")
        .join("runtime")
        .join("denials")
        .join(session_id)
        .join(format!("{safe_reason}-{safe_path}.seen"))
}

fn repeated_denial_seen(repo_root: &Path, session_id: &str, path: &str, reason_code: &str) -> bool {
    let marker = repeated_denial_marker_path(repo_root, session_id, path, reason_code);
    let seen = marker.exists();
    if !seen {
        if let Some(parent) = marker.parent() {
            let _ = fs::create_dir_all(parent);
        }
        let _ = fs::write(marker, b"seen");
    }
    seen
}

fn repeated_denial_reason(path: &str) -> String {
    format!(
        "Repeated denial for {path}. Stop retrying this path. Use one writer: parent/main agent owns the edit; subagents report findings only."
    )
}
```

Thread `repo_root`, denied path, and reason code into the authorize denial formatting path. The minimal implementation can wrap after receiving a deny decision for each target:

```rust
if repeated_denial_seen(repo_root, session_id, &target.path, &decision.reason_code) {
    return Ok(HookOutcome::Deny {
        reason: repeated_denial_reason(&target.path),
    });
}
```

Pass `repo_root` from the existing pre-tool authorization caller into the denial formatting path; do not add global state.

- [ ] **Step 5: Run hook tests to verify GREEN**

Run:

```bash
cargo test -p stateful-cli pre_tool_use_apply_patch_repeated_same_path_denial_suggests_single_writer pre_tool_use_apply_patch_different_path_denial_does_not_trigger_single_writer
```

Expected: PASS.

- [ ] **Step 6: Run focused hook regression tests**

Run:

```bash
cargo test -p stateful-cli pre_tool_use_apply_patch_denial_does_not_render_live_context pre_tool_use_apply_patch_posts_authorize_and_allows_when_server_allows
```

Expected: PASS.

- [ ] **Step 7: Commit Task 3**

```bash
git add crates/stateful-cli/src/hook.rs crates/stateful-cli/tests/hook.rs
git commit -m "Guide repeated denials to single writer"
```

---

### Task 4: Empty-Stop Detection and Compact Empty Output

**Files:**
- Modify: `crates/stateful-bench/scripts/codex_pair_agent.py`
- Modify: `crates/stateful-bench/scripts/denovo_codex_agent.py`
- Modify: `crates/stateful-bench/tests/cli.rs`
- Modify: `crates/stateful-cli/src/install.rs`
- Modify: `crates/stateful-cli/tests/hook.rs`

**Interfaces:**
- Produces in Python: `codex_output_is_empty_stop(stdout: str, stderr: str) -> bool`.
- Produces in Python: retry event reason `empty_stop`.
- Produces in DeNovo: finish reason `codex-empty-stop` or `omp-empty-stop` for capped empty stop.
- Produces in OMP JS: empty tool output text `No output.`.

- [ ] **Step 1: Add failing Python unit through Rust test**

In `crates/stateful-bench/tests/cli.rs`, add a test that imports `codex_pair_agent.py` and calls the new detector. Follow existing Python import tests in the same file.

```rust
#[test]
fn codex_pair_agent_detects_empty_successful_stop() {
    let script = format!(
        r#"
import importlib.util, json, sys
from pathlib import Path
spec = importlib.util.spec_from_file_location("codex_pair_agent_empty_stop_test", {agent_path})
module = importlib.util.module_from_spec(spec)
sys.modules[spec.name] = module
spec.loader.exec_module(module)
empty = '{{"type":"message","role":"assistant","content":[]}}\n{{"type":"turn.completed","usage":{{}}}}\n'
non_empty = '{{"type":"message","role":"assistant","content":[{{"type":"text","text":"done"}}]}}\n'
print(json.dumps({{
    "empty": module.codex_output_is_empty_stop(empty, ""),
    "non_empty": module.codex_output_is_empty_stop(non_empty, ""),
}}, sort_keys=True))
"#,
        agent_path = serde_json::to_string(&codex_pair_agent_path()).unwrap(),
    );

    let output = run_python_snippet(&script);
    let value: serde_json::Value = serde_json::from_str(output.trim()).expect("json output");
    assert_eq!(value["empty"], true);
    assert_eq!(value["non_empty"], false);
}
```

Use the existing Python helper names already present in `cli.rs` for script paths and snippet execution; keep the assertions identical.

- [ ] **Step 2: Add failing retry-loop test**

```rust
#[test]
fn codex_pair_agent_retries_empty_stop_once() {
    let script = format!(
        r#"
import importlib.util, json, subprocess, sys
from pathlib import Path
spec = importlib.util.spec_from_file_location("codex_pair_agent_retry_empty_stop_test", {agent_path})
module = importlib.util.module_from_spec(spec)
sys.modules[spec.name] = module
spec.loader.exec_module(module)
responses = [
    subprocess.CompletedProcess(["codex"], 0, '{{"type":"message","role":"assistant","content":[]}}\n{{"type":"session","id":"s1"}}\n', ''),
    subprocess.CompletedProcess(["codex"], 0, '{{"type":"message","role":"assistant","content":[{{"type":"text","text":"done"}}]}}\n', ''),
]
prompts = []
def runner(command, input, text, cwd, check, env, stdout, stderr):
    prompts.append(input)
    return responses.pop(0)
code = module.run_codex_with_resume(["codex"], "original", Path("."), {{}}, 1, runner=runner)
print(json.dumps({{"code": code, "attempts": len(prompts), "retry_prompt": prompts[1] if len(prompts) > 1 else ""}}, sort_keys=True))
"#,
        agent_path = serde_json::to_string(&codex_pair_agent_path()).unwrap(),
    );

    let output = run_python_snippet(&script);
    let value: serde_json::Value = serde_json::from_str(output.trim()).expect("json output");
    assert_eq!(value["code"], 0);
    assert_eq!(value["attempts"], 2);
    assert!(value["retry_prompt"].as_str().unwrap().contains("Previous response was empty"));
}
```

- [ ] **Step 3: Run tests to verify RED**

Run:

```bash
cargo test -p stateful-bench codex_pair_agent_detects_empty_successful_stop codex_pair_agent_retries_empty_stop_once
```

Expected: fail because detector and empty retry behavior do not exist.

- [ ] **Step 4: Implement empty-stop detector and prompt**

In `crates/stateful-bench/scripts/codex_pair_agent.py`, add near `RESUME_PROMPT`:

```python
EMPTY_STOP_PROMPT = "Previous response was empty. Continue with the requested code change. Do not summarize."
```

Extend `CodexRunResult`:

```python
class CodexRunResult:
    def __init__(
        self,
        returncode: int,
        stdout: str,
        stderr: str,
        session_id: str | None,
        resumeable_token_failure: bool,
        empty_stop: bool,
    ) -> None:
        self.returncode = returncode
        self.stdout = stdout
        self.stderr = stderr
        self.session_id = session_id
        self.resumeable_token_failure = resumeable_token_failure
        self.empty_stop = empty_stop
```

Update `run_codex_once`:

```python
return CodexRunResult(
    returncode=completed.returncode,
    stdout=stdout,
    stderr=stderr,
    session_id=codex_session_id_from_output(stdout),
    resumeable_token_failure=codex_output_has_resumeable_token_failure(stdout, stderr),
    empty_stop=completed.returncode == 0 and codex_output_is_empty_stop(stdout, stderr),
)
```

Add detector helpers near token failure helpers:

```python
def codex_output_is_empty_stop(stdout: str, stderr: str) -> bool:
    if stderr.strip():
        return False
    saw_terminal = False
    saw_assistant = False
    for event in iter_json_events(stdout):
        if codex_event_has_meaningful_assistant_content(event):
            return False
        event_type = str(event.get("type", "")).lower() if isinstance(event, dict) else ""
        if event_type in {"turn.completed", "turn.done", "response.completed"} or event_type.endswith(".completed"):
            saw_terminal = True
        if isinstance(event, dict) and str(event.get("role", "")).lower() == "assistant":
            saw_assistant = True
    return saw_terminal and saw_assistant


def codex_event_has_meaningful_assistant_content(event: object) -> bool:
    if not isinstance(event, dict):
        return False
    candidates = []
    if str(event.get("role", "")).lower() == "assistant":
        candidates.append(event.get("content"))
    payload = event.get("payload")
    if isinstance(payload, dict) and str(payload.get("role", "")).lower() == "assistant":
        candidates.append(payload.get("content"))
    for content in candidates:
        if isinstance(content, str) and content.strip():
            return True
        if isinstance(content, list):
            for item in content:
                if isinstance(item, str) and item.strip():
                    return True
                if isinstance(item, dict) and str(item.get("text", "")).strip():
                    return True
    return False
```

Update `run_codex_with_resume` before the `returncode == 0` success branch:

```python
        if result.empty_stop:
            if session_id and attempts < max_resumes:
                attempts += 1
                pending_resume_failures.append(result)
                current_command = codex_resume_command(command, session_id)
                current_prompt = EMPTY_STOP_PROMPT
                continue
            emit_codex_results(pending_resume_failures, suppress_resumeable_failures=True)
            emit_codex_result(result, suppress_resumeable_failures=False)
            return 2
```

- [ ] **Step 5: Map empty stop return code in DeNovo**

In `crates/stateful-bench/scripts/denovo_codex_agent.py`, define a constant near other finish reason helpers:

```python
CODEX_EMPTY_STOP_EXIT_CODE = 2
```

In the `returncode != 0` block around line 2639, special-case it:

```python
        if returncode != 0:
            patch_path.write_text("", encoding="utf-8")
            orchestration_trace = capture_trace()
            finish_command_record(orchestration_trace)
            cleanup_stateful_repo_enable(workspace, stateful_repo_cleanup)
            stateful_repo_cleanup = None
            finish_reason = (
                f"{args.cli_runtime}-empty-stop"
                if returncode == CODEX_EMPTY_STOP_EXIT_CODE
                else f"{args.cli_runtime}-error"
            )
            error = (
                f"{args.cli_runtime} returned an empty stop after retry cap"
                if returncode == CODEX_EMPTY_STOP_EXIT_CODE
                else f"{args.cli_runtime} exited {returncode}"
            )
            return InstanceResult(
                inst.id,
                False,
                None,
                finish_reason,
                error,
                None,
                subagent_used=subagent_usage["subagent_used"],
                subagent_usage=subagent_usage,
                token_usage=token_usage,
                orchestration_trace=orchestration_trace,
            )
```

- [ ] **Step 6: Run Python behavior tests to verify GREEN**

Run:

```bash
cargo test -p stateful-bench codex_pair_agent_detects_empty_successful_stop codex_pair_agent_retries_empty_stop_once
```

Expected: PASS.

- [ ] **Step 7: Add compact OMP empty tool output test**

In `crates/stateful-cli/src/install.rs`, first make the extension text testable:

1. Create `fn omp_extension_contents(binary_path: &str) -> anyhow::Result<String>` immediately above `write_omp_extension`.
2. Move the current `binary_json` assignment and generated JavaScript `format!` expression from `write_omp_extension` into that helper.
3. End the helper with `Ok(contents)`.
4. Replace the body of `write_omp_extension` with:

```rust
let contents = omp_extension_contents(binary_path)?;
write_or_create_text_file(extension_path, &contents)
```

Then add this failing test inside the existing `#[cfg(test)] mod tests` in `install.rs`:

```rust
#[test]
fn omp_extension_compacts_empty_tool_output() {
    let extension = omp_extension_contents("/usr/local/bin/stateful")
        .expect("extension contents should render");
    assert!(extension.contains("function emptyToolOutputText"), "{extension}");
    assert!(extension.contains("return \"No output.\""), "{extension}");
}
```

- [ ] **Step 8: Implement compact empty output helper in generated JS**

In the generated JavaScript string in `crates/stateful-cli/src/install.rs`, add near `truncateSandboxToolText`:

```javascript
function emptyToolOutputText(text) {
  return text && text.trim() ? text : "No output.";
}
```

Change `sandboxToolResultText` so its final return path wraps the joined parts:

```javascript
return emptyToolOutputText(parts.join("\n"));
```

Do not alter non-empty stdout/stderr formatting.

- [ ] **Step 9: Run OMP output tests**

Run:

```bash
cargo test -p stateful-cli generated_omp_extension_compacts_empty_tool_output
```

Expected: PASS.

- [ ] **Step 10: Commit Task 4**

```bash
git add crates/stateful-bench/scripts/codex_pair_agent.py crates/stateful-bench/scripts/denovo_codex_agent.py crates/stateful-bench/tests/cli.rs crates/stateful-cli/src/install.rs crates/stateful-cli/tests/hook.rs
git commit -m "Detect empty Codex stops efficiently"
```

---

## Final Verification

- [ ] Run union of touched tests:

```bash
cargo test -p stateful-server stale_base_observation_uses_concise_recovery_action stale_claim_observation_uses_concise_recovery_action
cargo test -p stateful-core context
cargo test -p stateful-cli pre_tool_use_apply_patch_repeated_same_path_denial_suggests_single_writer pre_tool_use_apply_patch_different_path_denial_does_not_trigger_single_writer pre_tool_use_apply_patch_wait_denial_keeps_resume_guidance generated_omp_extension_compacts_empty_tool_output
cargo test -p stateful-bench codex_pair_agent_detects_empty_successful_stop codex_pair_agent_retries_empty_stop_once
```

Expected: all PASS.

- [ ] Commit final implementation changes only when previous task commits were intentionally deferred. Stage the exact implementation files:

```bash
git status --short
git add crates/stateful-server/src/policy_service.rs crates/stateful-server/tests/routes.rs crates/stateful-core/src/context.rs crates/stateful-core/tests/context.rs crates/stateful-cli/src/hook.rs crates/stateful-cli/tests/hook.rs crates/stateful-cli/src/install.rs crates/stateful-bench/scripts/codex_pair_agent.py crates/stateful-bench/scripts/denovo_codex_agent.py crates/stateful-bench/tests/cli.rs
git commit -m "Reduce Stateful benchmark overhead"
```

## Self-Review Notes

- Spec coverage: all four requested changes map to Tasks 1-4.
- Placeholder scan: no `TBD`, `TODO`, or open-ended implementation steps.
- Type consistency: new Python helper names are `codex_output_is_empty_stop` and `codex_event_has_meaningful_assistant_content`; repeated denial helper names use `repeated_denial_*`; context summary helper is `render_summary`.
