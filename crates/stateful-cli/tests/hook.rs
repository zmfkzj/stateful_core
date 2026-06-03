use std::{
    fs,
    io::{Read, Write},
    net::TcpListener,
    path::Path,
    process::{Command, Stdio},
    sync::mpsc,
    thread,
};

use stateful_cli::{
    GlobalPaths, HookOutcome, ServerRuntime, enable_repo, handle_post_tool_use_in_repo,
    handle_pre_tool_use, handle_pre_tool_use_in_repo, read_current_session_file,
    write_global_runtime_file,
};

#[test]
fn pre_tool_use_allows_read_only_bash() {
    let input = r#"{
      "session_id": "s1",
      "cwd": "/repo",
      "hook_event_name": "PreToolUse",
      "tool_name": "Bash",
      "tool_input": {
        "command": "rg auth src"
      }
    }"#;

    let outcome = handle_pre_tool_use(input).expect("hook input should parse");

    assert_eq!(outcome, HookOutcome::Allow);
}

#[test]
fn pre_tool_use_allows_raw_test_bash() {
    let input = r#"{
      "session_id": "s1",
      "cwd": "/repo",
      "hook_event_name": "PreToolUse",
      "tool_name": "Bash",
      "tool_input": {
        "command": "cargo test"
      }
    }"#;

    let outcome = handle_pre_tool_use(input).expect("hook input should parse");

    assert_eq!(outcome, HookOutcome::Allow);
}

#[test]
fn pre_tool_use_allows_stateful_diagnostic_bash() {
    let input = r#"{
      "session_id": "s1",
      "cwd": "/repo",
      "hook_event_name": "PreToolUse",
      "tool_name": "Bash",
      "tool_input": {
        "command": "./target/debug/stateful doctor"
      }
    }"#;

    let outcome = handle_pre_tool_use(input).expect("hook input should parse");

    assert_eq!(outcome, HookOutcome::Allow);
}

#[test]
fn pre_tool_use_allows_stateful_controlled_validation_bash() {
    let input = r#"{
      "session_id": "s1",
      "cwd": "/repo",
      "hook_event_name": "PreToolUse",
      "tool_name": "Bash",
      "tool_input": {
        "command": "./target/debug/stateful validate cargo-test"
      }
    }"#;

    let outcome = handle_pre_tool_use(input).expect("hook input should parse");

    assert_eq!(outcome, HookOutcome::Allow);
}

#[test]
fn pre_tool_use_allows_stateful_bench_operational_bash() {
    let input = r#"{
      "session_id": "s1",
      "cwd": "/repo",
      "hook_event_name": "PreToolUse",
      "tool_name": "Bash",
      "tool_input": {
        "command": "target/debug/stateful-bench run --pairs .stateful_bench/pairs/all.jsonl --mode no-state --agent-cmd-template codex"
      }
    }"#;

    let outcome = handle_pre_tool_use(input).expect("hook input should parse");

    assert_eq!(outcome, HookOutcome::Allow);
}

#[test]
fn pre_tool_use_in_repo_records_current_session_for_mcp() {
    let temp_root = std::env::temp_dir().join(format!(
        "stateful-hook-current-session-test-{}",
        std::process::id()
    ));
    if temp_root.exists() {
        fs::remove_dir_all(&temp_root).expect("old temp root should be removable");
    }
    let paths = GlobalPaths::new(temp_root.join("home"));
    let repo_root = temp_root.join("repo");
    fs::create_dir_all(&repo_root).expect("repo root should be creatable");
    enable_test_repo(&paths, &repo_root);
    let (runtime, _rx) = spawn_fake_stateful_server(
        r#"{"decision":"allow","reason_code":"authorized","message":"ok","required_next_action":null}"#,
    );
    write_global_runtime_file(&paths, &runtime).expect("global runtime file should write");

    let input = r#"{
      "session_id": "s-current",
      "cwd": "/repo",
      "hook_event_name": "PreToolUse",
      "tool_name": "Bash",
      "tool_input": {
        "command": "rg auth src"
      }
    }"#;

    let output = run_hook_subprocess(&repo_root, &paths, &["hook", "pre-tool-use"], input);

    assert!(
        output.status.success(),
        "stateful hook failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let session = read_current_session_file(&repo_root).expect("current session should read");
    assert_eq!(session.session_id, "s-current");
    assert_eq!(session.workspace_id, "w1");

    fs::remove_dir_all(&temp_root).expect("temp root should be removable");
}

#[test]
fn pre_tool_use_from_enabled_subdir_records_session_at_repo_root() {
    let temp_root = std::env::temp_dir().join(format!(
        "stateful-hook-subdir-session-test-{}",
        std::process::id()
    ));
    if temp_root.exists() {
        fs::remove_dir_all(&temp_root).expect("old temp root should be removable");
    }
    let paths = GlobalPaths::new(temp_root.join("home"));
    let repo_root = temp_root.join("repo");
    let subdir = repo_root.join("nested/worktree");
    fs::create_dir_all(&subdir).expect("subdir should be creatable");
    enable_test_repo(&paths, &repo_root);
    let (runtime, rx) = spawn_fake_stateful_server(
        r#"{"decision":"allow","reason_code":"authorized","message":"ok","required_next_action":null}"#,
    );
    write_global_runtime_file(&paths, &runtime).expect("global runtime file should write");

    let input = r#"{
      "session_id": "s-subdir",
      "cwd": "/repo/nested/worktree",
      "hook_event_name": "PreToolUse",
      "tool_name": "apply_patch",
      "tool_input": {
        "command": "*** Begin Patch\n*** Update File: src/auth.ts\n*** End Patch\n"
      }
    }"#;

    let output = run_hook_subprocess(&subdir, &paths, &["hook", "pre-tool-use"], input);

    assert!(
        output.status.success(),
        "stateful hook failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let _request = rx.recv().expect("captured request should arrive");
    let session = read_current_session_file(&repo_root).expect("root current session should read");
    assert_eq!(session.session_id, "s-subdir");
    assert!(!subdir.join(".stateful_core/runtime/session.json").exists());

    fs::remove_dir_all(&temp_root).expect("temp root should be removable");
}

#[test]
fn pre_tool_use_in_disabled_repo_noops_without_runtime() {
    let temp_root = std::env::temp_dir().join(format!(
        "stateful-hook-disabled-test-{}",
        std::process::id()
    ));
    if temp_root.exists() {
        fs::remove_dir_all(&temp_root).expect("old temp root should be removable");
    }
    fs::create_dir_all(temp_root.join(".git")).expect("git marker should write");

    let input = r#"{
      "session_id": "s-disabled",
      "cwd": "/repo",
      "hook_event_name": "PreToolUse",
      "tool_name": "apply_patch",
      "tool_input": {
        "command": "*** Begin Patch\n*** Update File: src/auth.ts\n*** End Patch\n"
      }
    }"#;

    let outcome =
        handle_pre_tool_use_in_repo(input, &temp_root).expect("disabled repo should no-op");

    assert_eq!(outcome, HookOutcome::Allow);
    assert!(
        !temp_root
            .join(".stateful_core/runtime/session.json")
            .exists()
    );
    fs::remove_dir_all(&temp_root).expect("temp root should be removable");
}

#[test]
fn pre_tool_use_allows_stateful_intent_declare_in_bash() {
    let input = r#"{
      "session_id": "s1",
      "cwd": "/repo",
      "hook_event_name": "PreToolUse",
      "tool_name": "Bash",
      "tool_input": {
        "command": "stateful intent declare --session-id s1 --workspace-id w1 src/auth.ts"
      }
    }"#;

    let outcome = handle_pre_tool_use(input).expect("hook input should parse");

    assert_eq!(outcome, HookOutcome::Allow);
}

#[test]
fn pre_tool_use_denies_other_stateful_control_commands_in_bash() {
    let input = r#"{
      "session_id": "s1",
      "cwd": "/repo",
      "hook_event_name": "PreToolUse",
      "tool_name": "Bash",
      "tool_input": {
        "command": "stateful sync-outbox"
      }
    }"#;

    let outcome = handle_pre_tool_use(input).expect("hook input should parse");

    assert!(matches!(outcome, HookOutcome::Deny { .. }));
    let json = outcome
        .to_stdout_json()
        .expect("deny outcome should serialize");
    assert_eq!(json["hookSpecificOutput"]["permissionDecision"], "deny");
    assert!(
        json["hookSpecificOutput"]["permissionDecisionReason"]
            .as_str()
            .expect("reason should be string")
            .contains("MCP")
    );
}

#[test]
fn pre_tool_use_denies_apply_patch_until_intent_protocol_exists() {
    let input = r#"{
      "session_id": "s1",
      "cwd": "/repo",
      "hook_event_name": "PreToolUse",
      "tool_name": "apply_patch",
      "tool_input": {
        "command": "*** Begin Patch\n*** Update File: src/auth.ts\n*** End Patch\n"
      }
    }"#;

    let outcome = handle_pre_tool_use(input).expect("hook input should parse");

    assert!(matches!(outcome, HookOutcome::Deny { .. }));
}

#[test]
fn pre_tool_use_denies_edit_and_write_without_runtime() {
    for (tool_name, tool_input) in [
        (
            "Edit",
            serde_json::json!({
                "file_path": "src/auth.ts",
                "old_string": "old",
                "new_string": "new"
            }),
        ),
        (
            "Write",
            serde_json::json!({
                "file_path": "src/auth.ts",
                "content": "new"
            }),
        ),
    ] {
        let input = serde_json::json!({
            "session_id": "s1",
            "cwd": "/repo",
            "hook_event_name": "PreToolUse",
            "tool_name": tool_name,
            "tool_input": tool_input
        })
        .to_string();

        let outcome = handle_pre_tool_use(&input).expect("hook input should parse");

        assert!(
            matches!(outcome, HookOutcome::Deny { .. }),
            "{tool_name} should require stateful authorization"
        );
    }
}

#[test]
fn pre_tool_use_edit_posts_authorize_and_denies_when_server_denies() {
    let temp_root = std::env::temp_dir().join(format!(
        "stateful-hook-edit-deny-test-{}",
        std::process::id()
    ));
    if temp_root.exists() {
        fs::remove_dir_all(&temp_root).expect("old temp root should be removable");
    }
    let paths = GlobalPaths::new(temp_root.join("home"));
    let repo_root = temp_root.join("repo");
    fs::create_dir_all(&repo_root).expect("repo root should be creatable");
    enable_test_repo(&paths, &repo_root);
    let (runtime, rx) = spawn_fake_stateful_server(
        r#"{"decision":"deny","reason_code":"scope_mismatch","message":"Write target is outside active intent scope.","required_next_action":"Declare matching intent."}"#,
    );
    write_global_runtime_file(&paths, &runtime).expect("global runtime file should write");

    let input = serde_json::json!({
        "session_id": "s1",
        "cwd": repo_root,
        "hook_event_name": "PreToolUse",
        "tool_name": "Edit",
        "tool_input": {
            "file_path": "src/auth.ts",
            "old_string": "old",
            "new_string": "new"
        }
    })
    .to_string();

    let output = run_hook_subprocess(&repo_root, &paths, &["hook", "pre-tool-use"], &input);

    assert!(
        output.status.success(),
        "stateful hook failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let request = rx.recv().expect("captured request should arrive");
    assert!(request.contains("POST /v1/authorize HTTP/1.1"));
    assert!(request.contains("\"action\":\"write_file\""));
    assert!(request.contains("\"path\":\"src/auth.ts\""));
    let json: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("deny outcome should serialize");
    assert_eq!(json["hookSpecificOutput"]["permissionDecision"], "deny");

    fs::remove_dir_all(&temp_root).expect("temp root should be removable");
}

#[test]
fn pre_tool_use_edit_relative_path_is_resolved_from_payload_cwd() {
    let temp_root = std::env::temp_dir().join(format!(
        "stateful-hook-edit-cwd-test-{}",
        std::process::id()
    ));
    if temp_root.exists() {
        fs::remove_dir_all(&temp_root).expect("old temp root should be removable");
    }
    let paths = GlobalPaths::new(temp_root.join("home"));
    let repo_root = temp_root.join("repo");
    let docs_dir = repo_root.join("docs");
    fs::create_dir_all(&docs_dir).expect("docs dir should be creatable");
    enable_test_repo(&paths, &repo_root);
    let (runtime, rx) = spawn_fake_stateful_server(
        r#"{"decision":"allow","reason_code":"authorized","message":"ok","required_next_action":null}"#,
    );
    write_global_runtime_file(&paths, &runtime).expect("global runtime file should write");

    let input = serde_json::json!({
        "session_id": "s1",
        "cwd": docs_dir,
        "hook_event_name": "PreToolUse",
        "tool_name": "Edit",
        "tool_input": {
            "file_path": "plan.md",
            "old_string": "old",
            "new_string": "new"
        }
    })
    .to_string();

    let output = run_hook_subprocess_from(&temp_root, &paths, &["hook", "pre-tool-use"], &input);

    assert!(
        output.status.success(),
        "stateful hook failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let request = rx.recv().expect("captured request should arrive");
    assert!(request.contains("\"action\":\"write_file\""));
    assert!(request.contains("\"path\":\"docs/plan.md\""));

    fs::remove_dir_all(&temp_root).expect("temp root should be removable");
}

#[test]
fn run_hook_uses_payload_cwd_for_repo_gate() {
    let temp_root = std::env::temp_dir().join(format!(
        "stateful-hook-payload-cwd-test-{}",
        std::process::id()
    ));
    if temp_root.exists() {
        fs::remove_dir_all(&temp_root).expect("old temp root should be removable");
    }
    let paths = GlobalPaths::new(temp_root.join("home"));
    let repo_root = temp_root.join("repo");
    let outside = temp_root.join("outside");
    fs::create_dir_all(&repo_root).expect("repo root should be creatable");
    fs::create_dir_all(&outside).expect("outside dir should be creatable");
    enable_test_repo(&paths, &repo_root);
    let (runtime, rx) = spawn_fake_stateful_server(
        r#"{"decision":"allow","reason_code":"authorized","message":"ok","required_next_action":null}"#,
    );
    write_global_runtime_file(&paths, &runtime).expect("global runtime file should write");

    let input = serde_json::json!({
        "session_id": "s-cwd",
        "cwd": repo_root,
        "hook_event_name": "PreToolUse",
        "tool_name": "apply_patch",
        "tool_input": {
            "command": "*** Begin Patch\n*** Update File: src/auth.ts\n*** End Patch\n"
        }
    })
    .to_string();

    let output = run_hook_subprocess_from(&outside, &paths, &["hook", "pre-tool-use"], &input);

    assert!(
        output.status.success(),
        "stateful hook failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let request = rx.recv().expect("captured request should arrive");
    assert!(request.contains("POST /v1/authorize HTTP/1.1"));
    let session = read_current_session_file(&repo_root).expect("root current session should read");
    assert_eq!(session.session_id, "s-cwd");
    assert!(!outside.join(".stateful_core/runtime/session.json").exists());

    fs::remove_dir_all(&temp_root).expect("temp root should be removable");
}

#[test]
fn pre_tool_use_apply_patch_posts_authorize_and_allows_when_server_allows() {
    let temp_root = std::env::temp_dir().join(format!("stateful-hook-test-{}", std::process::id()));
    if temp_root.exists() {
        fs::remove_dir_all(&temp_root).expect("old temp root should be removable");
    }
    let paths = GlobalPaths::new(temp_root.join("home"));
    let repo_root = temp_root.join("repo");
    fs::create_dir_all(&repo_root).expect("repo root should be creatable");
    enable_test_repo(&paths, &repo_root);
    let (runtime, rx) = spawn_fake_stateful_server(
        r#"{"decision":"allow","reason_code":"authorized","message":"ok","required_next_action":null}"#,
    );
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

    let output = run_hook_subprocess(&repo_root, &paths, &["hook", "pre-tool-use"], input);

    assert!(
        output.status.success(),
        "stateful hook failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let request = rx.recv().expect("captured request should arrive");
    assert!(request.contains("POST /v1/authorize HTTP/1.1"));
    assert!(request.contains("Authorization: Bearer secret-token"));
    assert!(request.contains("\"protocol_version\":\"stateful.v1\""));
    assert!(request.contains("\"request_id\":\""));
    assert!(request.contains("\"observed_at\":\"2026-05-31T00:00:00Z\""));
    assert!(request.contains("\"session_id\":\"s1\""));
    assert!(request.contains("\"workspace_id\":\"w1\""));
    assert!(request.contains("\"action\":\"write_file\""));
    assert!(request.contains("\"path\":\"src/auth.ts\""));
    assert!(request.contains("\"queue_on_conflict\":true"));

    fs::remove_dir_all(&temp_root).expect("temp root should be removable");
}

#[test]
fn hook_pre_tool_use_discovers_global_runtime_file() {
    let temp_root = std::env::temp_dir().join(format!(
        "stateful-hook-global-runtime-test-{}",
        std::process::id()
    ));
    if temp_root.exists() {
        fs::remove_dir_all(&temp_root).expect("old temp root should be removable");
    }
    fs::create_dir_all(&temp_root).expect("temp root should be creatable");
    let repo_root = temp_root.join("repo");
    fs::create_dir_all(&repo_root).expect("repo root should be creatable");
    let paths = GlobalPaths::new(temp_root.join("home"));
    enable_test_repo(&paths, &repo_root);
    let (runtime, rx) = spawn_fake_stateful_server(
        r#"{"decision":"allow","reason_code":"authorized","message":"ok","required_next_action":null}"#,
    );
    write_global_runtime_file(&paths, &runtime).expect("global runtime file should write");

    let input = r#"{
      "session_id": "s-global",
      "cwd": "/repo",
      "hook_event_name": "PreToolUse",
      "tool_name": "apply_patch",
      "tool_input": {
        "command": "*** Begin Patch\n*** Update File: src/auth.ts\n*** End Patch\n"
      }
    }"#;

    let mut child = Command::new(env!("CARGO_BIN_EXE_stateful"))
        .args(["hook", "pre-tool-use"])
        .current_dir(&repo_root)
        .env_clear()
        .env("STATEFUL_HOME", &paths.home)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("stateful hook should spawn");
    let mut stdin = child.stdin.take().expect("stdin should be piped");
    stdin
        .write_all(input.as_bytes())
        .expect("hook input should write");
    drop(stdin);
    let output = child
        .wait_with_output()
        .expect("stateful hook should complete");

    assert!(
        output.status.success(),
        "stateful hook failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let request = rx.recv().expect("captured request should arrive");
    assert!(request.contains("POST /v1/authorize HTTP/1.1"));
    assert!(request.contains("\"workspace_id\":\"w1\""));

    fs::remove_dir_all(&temp_root).expect("temp root should be removable");
}

#[test]
fn pre_tool_use_apply_patch_denies_when_server_denies() {
    let temp_root =
        std::env::temp_dir().join(format!("stateful-hook-deny-test-{}", std::process::id()));
    if temp_root.exists() {
        fs::remove_dir_all(&temp_root).expect("old temp root should be removable");
    }
    let paths = GlobalPaths::new(temp_root.join("home"));
    let repo_root = temp_root.join("repo");
    fs::create_dir_all(&repo_root).expect("repo root should be creatable");
    enable_test_repo(&paths, &repo_root);
    let (runtime, rx) = spawn_fake_stateful_server(
        r#"{"decision":"deny","reason_code":"scope_mismatch","message":"Write target is outside active intent scope.","required_next_action":"Declare matching intent."}"#,
    );
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

    let output = run_hook_subprocess(&repo_root, &paths, &["hook", "pre-tool-use"], input);

    assert!(
        output.status.success(),
        "stateful hook failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let _request = rx.recv().expect("captured request should arrive");
    let json: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("deny outcome should serialize");
    assert_eq!(json["hookSpecificOutput"]["permissionDecision"], "deny");
    assert_eq!(
        json["hookSpecificOutput"]["permissionDecisionReason"],
        "Declare matching intent."
    );

    fs::remove_dir_all(&temp_root).expect("temp root should be removable");
}

#[test]
fn pre_tool_use_apply_patch_delete_posts_delete_file_action() {
    let temp_root =
        std::env::temp_dir().join(format!("stateful-hook-delete-test-{}", std::process::id()));
    if temp_root.exists() {
        fs::remove_dir_all(&temp_root).expect("old temp root should be removable");
    }
    let paths = GlobalPaths::new(temp_root.join("home"));
    let repo_root = temp_root.join("repo");
    fs::create_dir_all(&repo_root).expect("repo root should be creatable");
    enable_test_repo(&paths, &repo_root);
    let (runtime, rx) = spawn_fake_stateful_server(
        r#"{"decision":"allow","reason_code":"authorized","message":"ok","required_next_action":null}"#,
    );
    write_global_runtime_file(&paths, &runtime).expect("global runtime file should write");

    let input = r#"{
      "session_id": "s1",
      "cwd": "/repo",
      "hook_event_name": "PreToolUse",
      "tool_name": "apply_patch",
      "tool_input": {
        "command": "*** Begin Patch\n*** Delete File: src/auth.ts\n*** End Patch\n"
      }
    }"#;

    let output = run_hook_subprocess(&repo_root, &paths, &["hook", "pre-tool-use"], input);

    assert!(
        output.status.success(),
        "stateful hook failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let request = rx.recv().expect("captured request should arrive");
    assert!(request.contains("\"action\":\"delete_file\""));
    assert!(request.contains("\"path\":\"src/auth.ts\""));

    fs::remove_dir_all(&temp_root).expect("temp root should be removable");
}

#[test]
fn normalized_pre_tool_use_uses_same_authorization_path_as_codex_input() {
    let temp_root = std::env::temp_dir().join(format!(
        "stateful-normalized-hook-test-{}",
        std::process::id()
    ));
    if temp_root.exists() {
        fs::remove_dir_all(&temp_root).expect("old temp root should be removable");
    }
    let paths = GlobalPaths::new(temp_root.join("home"));
    let repo_root = temp_root.join("repo");
    fs::create_dir_all(repo_root.join("src")).expect("repo should be creatable");
    enable_test_repo(&paths, &repo_root);
    let (runtime, rx) = spawn_fake_stateful_server(
        r#"{"decision":"allow","reason_code":"authorized","message":"ok","required_next_action":null}"#,
    );
    write_global_runtime_file(&paths, &runtime).expect("runtime should write");

    let input = serde_json::json!({
        "event": "pre_tool_use",
        "session_id": "s-normalized",
        "cwd": repo_root,
        "tool_name": "Write",
        "tool_input": {
            "file_path": "src/auth.ts"
        },
        "source": {
            "kind": "hook",
            "agent": "generic"
        }
    })
    .to_string();

    let output = run_hook_subprocess(&repo_root, &paths, &["hook", "run", "pre-tool-use"], &input);
    assert!(
        output.status.success(),
        "stateful hook failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let request = rx.recv().expect("fake server should receive request");
    assert!(request.contains("POST /v1/authorize HTTP/1.1"));
    assert!(request.contains("src/auth.ts"));

    fs::remove_dir_all(&temp_root).expect("temp root should be removable");
}

#[test]
fn codex_hook_subcommand_remains_compatible() {
    let input = r#"{
      "session_id": "s1",
      "cwd": "/repo",
      "hook_event_name": "PreToolUse",
      "tool_name": "Bash",
      "tool_input": {
        "command": "rg auth src"
      }
    }"#;

    let outcome =
        stateful_cli::handle_codex_pre_tool_use(input).expect("codex hook input should parse");

    assert_eq!(outcome, HookOutcome::Allow);
}

#[test]
fn session_start_posts_session_register() {
    let temp_root =
        std::env::temp_dir().join(format!("stateful-hook-session-test-{}", std::process::id()));
    if temp_root.exists() {
        fs::remove_dir_all(&temp_root).expect("old temp root should be removable");
    }
    let paths = GlobalPaths::new(temp_root.join("home"));
    let repo_root = temp_root.join("repo");
    fs::create_dir_all(&repo_root).expect("repo root should be creatable");
    enable_test_repo(&paths, &repo_root);
    let (runtime, rx) = spawn_fake_stateful_server(r#"{"status":"ok"}"#);
    write_global_runtime_file(&paths, &runtime).expect("global runtime file should write");

    let input = r#"{
      "session_id": "s1",
      "hook_event_name": "SessionStart"
    }"#;

    let output = run_hook_subprocess(&repo_root, &paths, &["hook", "session-start"], input);
    assert!(
        output.status.success(),
        "stateful hook failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let request = rx.recv().expect("captured request should arrive");
    assert!(request.contains("POST /v1/session/register HTTP/1.1"));
    assert!(request.contains("\"protocol_version\":\"stateful.v1\""));
    assert!(request.contains("\"request_id\":\""));
    assert!(request.contains("\"session_id\":\"s1\""));
    assert!(request.contains("\"workspace_id\":\"w1\""));

    fs::remove_dir_all(&temp_root).expect("temp root should be removable");
}

#[test]
fn post_tool_use_posts_session_heartbeat() {
    let temp_root =
        std::env::temp_dir().join(format!("stateful-hook-post-test-{}", std::process::id()));
    if temp_root.exists() {
        fs::remove_dir_all(&temp_root).expect("old temp root should be removable");
    }
    let paths = GlobalPaths::new(temp_root.join("home"));
    let repo_root = temp_root.join("repo");
    fs::create_dir_all(&repo_root).expect("repo root should be creatable");
    enable_test_repo(&paths, &repo_root);
    let (runtime, rx) = spawn_fake_stateful_server(r#"{"status":"ok"}"#);
    write_global_runtime_file(&paths, &runtime).expect("global runtime file should write");

    let input = r#"{
      "session_id": "s1",
      "hook_event_name": "PostToolUse",
      "tool_name": "Bash",
      "tool_input": {"command": "rg auth src"}
    }"#;

    let output = run_hook_subprocess(&repo_root, &paths, &["hook", "post-tool-use"], input);
    assert!(
        output.status.success(),
        "stateful hook failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let request = rx.recv().expect("captured request should arrive");
    assert!(request.contains("POST /v1/session/heartbeat HTTP/1.1"));
    assert!(request.contains("\"protocol_version\":\"stateful.v1\""));
    assert!(request.contains("\"request_id\":\""));
    assert!(request.contains("\"session_id\":\"s1\""));
    assert!(request.contains("\"workspace_id\":\"w1\""));

    fs::remove_dir_all(&temp_root).expect("temp root should be removable");
}

#[test]
fn post_tool_use_in_disabled_repo_noops_without_outbox() {
    let temp_root =
        std::env::temp_dir().join(format!("stateful-hook-outbox-test-{}", std::process::id()));
    if temp_root.exists() {
        fs::remove_dir_all(&temp_root).expect("old temp root should be removable");
    }
    fs::create_dir_all(temp_root.join(".git")).expect("git marker should write");

    let input = r#"{
      "session_id": "s1",
      "hook_event_name": "PostToolUse",
      "tool_name": "Bash",
      "tool_input": {"command": "rg auth src"}
    }"#;

    handle_post_tool_use_in_repo(input, &temp_root).expect("disabled repo should no-op");

    assert!(!temp_root.join(".stateful_core/outbox/s1.jsonl").exists());

    fs::remove_dir_all(&temp_root).expect("temp root should be removable");
}

#[test]
fn user_prompt_submit_posts_context_render() {
    let temp_root =
        std::env::temp_dir().join(format!("stateful-hook-prompt-test-{}", std::process::id()));
    if temp_root.exists() {
        fs::remove_dir_all(&temp_root).expect("old temp root should be removable");
    }
    let paths = GlobalPaths::new(temp_root.join("home"));
    let repo_root = temp_root.join("repo");
    fs::create_dir_all(&repo_root).expect("repo root should be creatable");
    enable_test_repo(&paths, &repo_root);
    let (runtime, rx) =
        spawn_fake_stateful_server(r#"{"status":"ok","prompt_text":"Nearby Activity\n- none"}"#);
    write_global_runtime_file(&paths, &runtime).expect("global runtime file should write");

    let input = r#"{
      "session_id": "s1",
      "hook_event_name": "UserPromptSubmit",
      "prompt": "work on auth"
    }"#;

    let output = run_hook_subprocess(&repo_root, &paths, &["hook", "user-prompt-submit"], input);

    assert!(
        output.status.success(),
        "stateful hook failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let rendered = String::from_utf8(output.stdout).expect("prompt output should be utf8");
    assert!(rendered.contains("Nearby Activity"));
    let request = rx.recv().expect("captured request should arrive");
    assert!(request.contains("POST /v1/context/render HTTP/1.1"));
    assert!(request.contains("\"mode\":\"brief\""));

    fs::remove_dir_all(&temp_root).expect("temp root should be removable");
}

#[test]
fn stop_posts_activity_finalize() {
    let temp_root =
        std::env::temp_dir().join(format!("stateful-hook-stop-test-{}", std::process::id()));
    if temp_root.exists() {
        fs::remove_dir_all(&temp_root).expect("old temp root should be removable");
    }
    let paths = GlobalPaths::new(temp_root.join("home"));
    let repo_root = temp_root.join("repo");
    fs::create_dir_all(&repo_root).expect("repo root should be creatable");
    enable_test_repo(&paths, &repo_root);
    let (runtime, rx) = spawn_fake_stateful_server(r#"{"status":"ok"}"#);
    write_global_runtime_file(&paths, &runtime).expect("global runtime file should write");

    let input = r#"{
      "session_id": "s1",
      "hook_event_name": "Stop"
    }"#;

    let output = run_hook_subprocess(&repo_root, &paths, &["hook", "stop"], input);
    assert!(
        output.status.success(),
        "stateful hook failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let request = rx.recv().expect("captured request should arrive");
    assert!(request.contains("POST /v1/activity/finalize HTTP/1.1"));
    assert!(request.contains("\"protocol_version\":\"stateful.v1\""));
    assert!(request.contains("\"request_id\":\""));
    assert!(request.contains("\"session_id\":\"s1\""));
    assert!(request.contains("\"workspace_id\":\"w1\""));

    fs::remove_dir_all(&temp_root).expect("temp root should be removable");
}

fn read_http_request_maybe_body(stream: &mut std::net::TcpStream) -> String {
    let mut buffer = Vec::new();
    let mut byte = [0_u8; 1];
    while !buffer.ends_with(b"\r\n\r\n") {
        stream
            .read_exact(&mut byte)
            .expect("request header byte should read");
        buffer.push(byte[0]);
    }

    let headers = String::from_utf8(buffer.clone()).expect("headers should be utf8");
    let content_length = headers
        .lines()
        .find_map(|line| line.strip_prefix("Content-Length: "))
        .map(|value| value.parse::<usize>().expect("content length should parse"))
        .unwrap_or(0);

    let mut body = vec![0_u8; content_length];
    if content_length > 0 {
        stream
            .read_exact(&mut body)
            .expect("request body should read");
        buffer.extend_from_slice(&body);
    }

    String::from_utf8(buffer).expect("request should be utf8")
}

fn enable_test_repo(paths: &GlobalPaths, repo_root: &std::path::Path) {
    fs::create_dir_all(repo_root.join(".git")).expect("git marker should write");
    enable_repo(paths, repo_root, false).expect("repo should enable");
}

fn spawn_fake_stateful_server(
    actual_response: &'static str,
) -> (ServerRuntime, mpsc::Receiver<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("listener should bind");
    let addr = listener.local_addr().expect("listener addr should load");
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        for _ in 0..3 {
            let (mut stream, _) = listener.accept().expect("connection should arrive");
            let request = read_http_request_maybe_body(&mut stream);
            if request.contains("GET /health HTTP/1.1") {
                write_json_response(&mut stream, r#"{"status":"ok"}"#);
            } else if request.contains("GET /v1/current HTTP/1.1") {
                write_json_response(&mut stream, r#"{"status":"ok","current":{}}"#);
            } else {
                tx.send(request).expect("request should send to test");
                write_json_response(&mut stream, actual_response);
                break;
            }
        }
    });

    (
        ServerRuntime::new(format!("http://{addr}"), "secret-token", "w1", 42),
        rx,
    )
}

fn run_hook_subprocess(
    repo_root: &std::path::Path,
    paths: &GlobalPaths,
    args: &[&str],
    input: &str,
) -> std::process::Output {
    run_hook_subprocess_from(repo_root, paths, args, input)
}

fn run_hook_subprocess_from(
    cwd: &Path,
    paths: &GlobalPaths,
    args: &[&str],
    input: &str,
) -> std::process::Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_stateful"))
        .args(args)
        .current_dir(cwd)
        .env_clear()
        .env("STATEFUL_HOME", &paths.home)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("stateful hook should spawn");
    let mut stdin = child.stdin.take().expect("stdin should be piped");
    stdin
        .write_all(input.as_bytes())
        .expect("hook input should write");
    drop(stdin);
    child
        .wait_with_output()
        .expect("stateful hook should complete")
}

fn write_json_response(stream: &mut std::net::TcpStream, body: &str) {
    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n{}",
        body.len(),
        body
    );
    stream
        .write_all(response.as_bytes())
        .expect("response should write");
}
