use std::{
    collections::VecDeque,
    fs,
    io::{Read, Write},
    net::TcpListener,
    path::Path,
    process::{Command, Stdio},
    sync::mpsc,
    thread,
    time::Duration,
};

use stateful_cli::{
    GlobalPaths, HookOutcome, STATEFUL_CODEX_RUN_ID_ENV, ServerRuntime, enable_repo,
    handle_post_tool_use_in_repo, handle_pre_tool_use, handle_pre_tool_use_in_repo,
    read_current_session_file, read_current_session_file_for_codex_run, write_global_runtime_file,
};

fn assert_raw_bash_denied_with_sandbox_run_guidance(outcome: HookOutcome) {
    let HookOutcome::Deny { reason } = outcome else {
        panic!("raw Bash should be denied");
    };

    assert!(
        reason.contains("stateful sandbox run"),
        "reason `{reason}` should direct callers to stateful sandbox run"
    );
}

fn assert_bash_denial_mentions(outcome: HookOutcome, expected: &str) {
    let HookOutcome::Deny { reason } = outcome else {
        panic!("expected Bash denial");
    };
    assert!(
        reason.contains(expected),
        "reason `{reason}` should contain `{expected}`"
    );
}

fn trusted_stateful_path() -> String {
    std::env::current_exe()
        .expect("test executable path should resolve")
        .to_string_lossy()
        .into_owned()
}

#[test]
fn pre_tool_use_denies_raw_read_only_bash_after_sandbox_runner_migration() {
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

    assert_bash_denial_mentions(outcome, "stateful sandbox run");
}

#[test]
fn pre_tool_use_raw_bash_denial_mentions_command_policy_skill_and_example() {
    let input = r#"{
      "session_id": "s1",
      "cwd": "/repo",
      "hook_event_name": "PreToolUse",
      "tool_name": "Bash",
      "tool_input": {
        "command": "git status"
      }
    }"#;

    let outcome = handle_pre_tool_use(input).expect("hook input should parse");
    let HookOutcome::Deny { reason } = outcome else {
        panic!("raw Bash should be denied");
    };

    assert!(reason.contains("stateful-command-policy"));
    assert!(reason.contains("--fs read-only --network disabled"));
    assert!(reason.contains("--command"));
}

#[test]
fn pre_tool_use_allows_canonical_sandbox_run_read_only() {
    let stateful = trusted_stateful_path();
    let input = serde_json::json!({
        "session_id": "s1",
        "cwd": "/repo",
        "hook_event_name": "PreToolUse",
        "tool_name": "Bash",
        "tool_input": {
            "command": format!("{stateful} sandbox run --fs read-only --network disabled --timeout-seconds 30 --command 'rg auth src'")
        }
    })
    .to_string();

    let outcome = handle_pre_tool_use(&input).expect("hook input should parse");

    assert_eq!(outcome, HookOutcome::Allow);
}

#[test]
fn pre_tool_use_allows_canonical_sandbox_run_write_targets() {
    let stateful = trusted_stateful_path();
    let input = serde_json::json!({
        "session_id": "s1",
        "cwd": "/repo",
        "hook_event_name": "PreToolUse",
        "tool_name": "Bash",
        "tool_input": {
            "command": format!("{stateful} sandbox run --fs write-targets --network enabled --write-target README.md --command 'printf x > README.md'")
        }
    })
    .to_string();

    let outcome = handle_pre_tool_use(&input).expect("hook input should parse");

    assert_eq!(outcome, HookOutcome::Allow);
}

#[test]
fn pre_tool_use_denies_sandbox_run_with_outer_command_separator() {
    let stateful = trusted_stateful_path();
    let input = serde_json::json!({
        "session_id": "s1",
        "cwd": "/repo",
        "hook_event_name": "PreToolUse",
        "tool_name": "Bash",
        "tool_input": {
            "command": format!("{stateful} sandbox run --fs read-only --command 'rg auth src'; rm README.md")
        }
    })
    .to_string();

    let outcome = handle_pre_tool_use(&input).expect("hook input should parse");

    assert_bash_denial_mentions(outcome, "single stateful sandbox run command");
}

#[test]
fn pre_tool_use_denies_sandbox_run_write_targets_without_target() {
    let stateful = trusted_stateful_path();
    let input = serde_json::json!({
        "session_id": "s1",
        "cwd": "/repo",
        "hook_event_name": "PreToolUse",
        "tool_name": "Bash",
        "tool_input": {
            "command": format!("{stateful} sandbox run --fs write-targets --network enabled --command 'printf x > README.md'")
        }
    })
    .to_string();

    let outcome = handle_pre_tool_use(&input).expect("hook input should parse");

    assert_bash_denial_mentions(outcome, "requires at least one write target");
}

#[test]
fn pre_tool_use_denies_sandbox_run_with_shell_escape_quote_mismatch() {
    let stateful = trusted_stateful_path();
    let input = serde_json::json!({
        "session_id": "s1",
        "cwd": "/repo",
        "hook_event_name": "PreToolUse",
        "tool_name": "Bash",
        "tool_input": {
            "command": format!("{stateful} sandbox run --command \\\"rg; rm README.md #\\\"")
        }
    })
    .to_string();

    let outcome = handle_pre_tool_use(&input).expect("hook input should parse");

    assert_bash_denial_mentions(outcome, "shell escapes");
}

#[test]
fn pre_tool_use_denies_invalid_sandbox_run_outer_wrappers() {
    let stateful = trusted_stateful_path();
    let cases = [
        (
            "outer env assignment",
            format!("FOO=bar {stateful} sandbox run --command 'rg auth src'"),
            "environment assignments",
        ),
        (
            "command substitution",
            format!("{stateful} sandbox run --command \"$(pwd)\""),
            "command substitution",
        ),
        (
            "outer redirect",
            format!("{stateful} sandbox run --command 'rg auth src' > /tmp/out"),
            "single stateful sandbox run command",
        ),
        (
            "outer pipeline",
            format!("{stateful} sandbox run --command 'rg auth src' | cat"),
            "single stateful sandbox run command",
        ),
        (
            "untrusted executable",
            "/bin/echo sandbox run --command 'rg auth src'".to_string(),
            "trusted absolute stateful binary",
        ),
        (
            "duplicate command",
            format!("{stateful} sandbox run --command 'rg auth src' --command pwd"),
            "exactly one --command",
        ),
        (
            "invalid fs",
            format!("{stateful} sandbox run --fs read-write --command 'rg auth src'"),
            "supports only read-only and write-targets profiles",
        ),
        (
            "invalid network",
            format!("{stateful} sandbox run --network inherited --command 'rg auth src'"),
            "network must be disabled or enabled",
        ),
        (
            "missing option value",
            format!("{stateful} sandbox run --command 'rg auth src' --write-target"),
            "argument `--write-target` requires a value",
        ),
        (
            "missing timeout value",
            format!("{stateful} sandbox run --command 'rg auth src' --timeout-seconds"),
            "argument `--timeout-seconds` requires a value",
        ),
        (
            "non-integer timeout value",
            format!("{stateful} sandbox run --timeout-seconds nope --command 'rg auth src'"),
            "--timeout-seconds requires an integer value",
        ),
    ];

    for (name, command, expected) in cases {
        let input = serde_json::json!({
            "session_id": "s1",
            "cwd": "/repo",
            "hook_event_name": "PreToolUse",
            "tool_name": "Bash",
            "tool_input": {
                "command": command
            }
        })
        .to_string();

        let outcome = handle_pre_tool_use(&input).expect("hook input should parse");

        let HookOutcome::Deny { reason } = outcome else {
            panic!("{name}: expected Bash denial");
        };
        assert!(
            reason.contains(expected),
            "{name}: reason `{reason}` should contain `{expected}`"
        );
    }
}

#[test]
fn pre_tool_use_denies_raw_read_only_bash_with_sandbox_run_guidance() {
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

    assert_raw_bash_denied_with_sandbox_run_guidance(outcome);
}

#[test]
fn pre_tool_use_denies_raw_bash_even_when_legacy_sandbox_metadata_exists() {
    let input = r#"{
      "session_id": "s1",
      "cwd": "/repo",
      "hook_event_name": "PreToolUse",
      "tool_name": "Bash",
      "sandbox": {
        "mode": "read-only",
        "writable_roots": ["/tmp"],
        "network_access": false
      },
      "tool_input": {
        "command": "rg auth src"
      }
    }"#;

    let outcome = handle_pre_tool_use(input).expect("hook input should parse");

    assert_raw_bash_denied_with_sandbox_run_guidance(outcome);
}

#[test]
fn pre_tool_use_denies_raw_quoted_rg_regex_alternation() {
    let input = r#"{
      "session_id": "s1",
      "cwd": "/repo",
      "hook_event_name": "PreToolUse",
      "tool_name": "Bash",
      "tool_input": {
        "command": "rg -n \"future work|Future work\" docs crates README.md .stateful"
      }
    }"#;

    let outcome = handle_pre_tool_use(input).expect("hook input should parse");

    assert_raw_bash_denied_with_sandbox_run_guidance(outcome);
}

#[test]
fn pre_tool_use_denies_raw_read_only_bash_dev_null_redirection() {
    let input = r#"{
      "session_id": "s1",
      "cwd": "/repo",
      "hook_event_name": "PreToolUse",
      "tool_name": "Bash",
      "tool_input": {
        "command": "rg -n \"future work\" docs 2>/dev/null"
      }
    }"#;

    let outcome = handle_pre_tool_use(input).expect("hook input should parse");

    assert_raw_bash_denied_with_sandbox_run_guidance(outcome);
}

#[test]
fn pre_tool_use_denies_raw_test_bash_with_sandbox_run_guidance() {
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

    assert_raw_bash_denied_with_sandbox_run_guidance(outcome);
}

#[test]
fn pre_tool_use_denies_raw_stateful_diagnostic_bash() {
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

    assert_raw_bash_denied_with_sandbox_run_guidance(outcome);
}

#[test]
fn pre_tool_use_denies_raw_stateful_controlled_validation_bash() {
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

    assert_raw_bash_denied_with_sandbox_run_guidance(outcome);
}

#[test]
fn pre_tool_use_denies_raw_stateful_bench_operational_bash() {
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

    assert_raw_bash_denied_with_sandbox_run_guidance(outcome);
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
fn pre_tool_use_records_current_session_for_codex_run() {
    let temp_root = std::env::temp_dir().join(format!(
        "stateful-hook-codex-run-session-test-{}",
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

    let output = run_hook_subprocess_with_extra_env(
        &repo_root,
        &paths,
        &["hook", "pre-tool-use"],
        input,
        &[(STATEFUL_CODEX_RUN_ID_ENV, "run-a")],
    );

    assert!(
        output.status.success(),
        "stateful hook failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let session = read_current_session_file_for_codex_run(&repo_root, "run-a")
        .expect("run-bound current session should read");
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
fn pre_tool_use_denies_raw_stateful_intent_declare() {
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

    assert_raw_bash_denied_with_sandbox_run_guidance(outcome);
}

#[test]
fn pre_tool_use_denies_raw_other_stateful_control_commands() {
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

    assert_raw_bash_denied_with_sandbox_run_guidance(outcome);
}

#[test]
fn pre_tool_use_denies_raw_bash_control_syntax_even_with_legacy_read_only_tmp_sandbox() {
    let input = r#"{
      "session_id": "s1",
      "cwd": "/repo",
      "hook_event_name": "PreToolUse",
      "tool_name": "Bash",
      "sandbox": {
        "mode": "read-only",
        "writable_roots": ["/tmp"],
        "network_access": false
      },
      "tool_input": {
        "command": "rg auth src | head > /tmp/stateful-rg.out"
      }
    }"#;

    let outcome = handle_pre_tool_use(input).expect("hook input should parse");

    assert_raw_bash_denied_with_sandbox_run_guidance(outcome);
}

#[test]
fn pre_tool_use_denies_raw_pipeline_even_with_legacy_read_only_tmp_sandbox() {
    let input = r#"{
      "session_id": "s1",
      "cwd": "/repo",
      "hook_event_name": "PreToolUse",
      "tool_name": "Bash",
      "sandbox": {
        "mode": "read-only",
        "writable_roots": ["/tmp"],
        "network_access": false
      },
      "tool_input": {
        "command": "rg -n \"future work|Future work\" docs | head > /tmp/stateful-rg.out"
      }
    }"#;

    let outcome = handle_pre_tool_use(input).expect("hook input should parse");

    assert_raw_bash_denied_with_sandbox_run_guidance(outcome);
}

#[test]
fn pre_tool_use_denies_bash_control_syntax_without_sandbox_run_wrapper() {
    let input = r#"{
      "session_id": "s1",
      "cwd": "/repo",
      "hook_event_name": "PreToolUse",
      "tool_name": "Bash",
      "tool_input": {
        "command": "rg auth src | head > /tmp/stateful-rg.out"
      }
    }"#;

    let outcome = handle_pre_tool_use(input).expect("hook input should parse");

    assert_raw_bash_denied_with_sandbox_run_guidance(outcome);
}

#[test]
fn run_hook_pre_tool_use_denies_raw_bash_with_legacy_trusted_sandbox_env() {
    let temp_root = std::env::temp_dir().join(format!(
        "stateful-hook-sandbox-env-test-{}",
        std::process::id()
    ));
    if temp_root.exists() {
        fs::remove_dir_all(&temp_root).expect("old temp root should be removable");
    }
    let paths = GlobalPaths::new(temp_root.join("home"));
    let repo_root = temp_root.join("repo");
    fs::create_dir_all(&repo_root).expect("repo root should be creatable");
    enable_test_repo(&paths, &repo_root);
    let (runtime, _rx) = spawn_fake_stateful_server(r#"{"status":"ok"}"#);
    write_global_runtime_file(&paths, &runtime).expect("global runtime file should write");
    let input = r#"{
      "session_id": "s1",
      "cwd": "/repo",
      "hook_event_name": "PreToolUse",
      "tool_name": "Bash",
      "tool_input": {
        "command": "rg auth src | head > /tmp/stateful-rg.out"
      }
    }"#;
    let trusted_sandbox = serde_json::json!({
        "mode": "read-only",
        "writable_roots": ["/tmp"],
        "network_access": false,
        "source": "stateful-codex-wrapper"
    })
    .to_string();

    let output = run_hook_subprocess_with_extra_env(
        &repo_root,
        &paths,
        &["hook", "pre-tool-use"],
        input,
        &[("STATEFUL_HOOK_TRUSTED_SANDBOX", trusted_sandbox.as_str())],
    );

    assert!(
        output.status.success(),
        "stateful hook failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let json: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("deny outcome should serialize");
    assert_eq!(json["hookSpecificOutput"]["permissionDecision"], "deny");
    assert!(
        json["hookSpecificOutput"]["permissionDecisionReason"]
            .as_str()
            .expect("reason should be string")
            .contains("stateful sandbox run")
    );

    fs::remove_dir_all(&temp_root).expect("temp root should be removable");
}

#[test]
fn pre_tool_use_denies_tool_input_spoofed_read_only_bash_sandbox() {
    let input = r#"{
      "session_id": "s1",
      "cwd": "/repo",
      "hook_event_name": "PreToolUse",
      "tool_name": "Bash",
      "tool_input": {
        "command": "rg auth src | head",
        "sandbox": {
          "mode": "read-only",
          "writable_roots": ["/tmp"],
          "network_access": false
        }
      }
    }"#;

    let outcome = handle_pre_tool_use(input).expect("hook input should parse");

    assert_raw_bash_denied_with_sandbox_run_guidance(outcome);
}

#[test]
fn pre_tool_use_denies_raw_bash_even_with_legacy_repo_writable_root_metadata() {
    let input = r#"{
      "session_id": "s1",
      "cwd": "/repo",
      "hook_event_name": "PreToolUse",
      "tool_name": "Bash",
      "sandbox": {
        "mode": "read-only",
        "writable_roots": ["/repo"],
        "network_access": false
      },
      "tool_input": {
        "command": "rg auth src | head"
      }
    }"#;

    let outcome = handle_pre_tool_use(input).expect("hook input should parse");

    assert_raw_bash_denied_with_sandbox_run_guidance(outcome);
}

#[test]
fn pre_tool_use_denies_raw_bash_even_with_legacy_network_access_metadata() {
    let input = r#"{
      "session_id": "s1",
      "cwd": "/repo",
      "hook_event_name": "PreToolUse",
      "tool_name": "Bash",
      "sandbox": {
        "mode": "read-only",
        "writable_roots": ["/tmp"],
        "network_access": true
      },
      "tool_input": {
        "command": "rg auth src | head"
      }
    }"#;

    let outcome = handle_pre_tool_use(input).expect("hook input should parse");

    assert_raw_bash_denied_with_sandbox_run_guidance(outcome);
}

#[test]
fn pre_tool_use_denies_raw_bash_even_with_incomplete_legacy_network_metadata() {
    let input = r#"{
      "session_id": "s1",
      "cwd": "/repo",
      "hook_event_name": "PreToolUse",
      "tool_name": "Bash",
      "sandbox": {
        "mode": "read-only",
        "writable_roots": ["/tmp"]
      },
      "tool_input": {
        "command": "rg auth src | head"
      }
    }"#;

    let outcome = handle_pre_tool_use(input).expect("hook input should parse");

    assert_raw_bash_denied_with_sandbox_run_guidance(outcome);
}

#[test]
fn pre_tool_use_denies_raw_mutating_command_even_with_legacy_read_only_sandbox() {
    let input = r#"{
      "session_id": "s1",
      "cwd": "/repo",
      "hook_event_name": "PreToolUse",
      "tool_name": "Bash",
      "sandbox": {
        "mode": "read-only",
        "writable_roots": ["/tmp"],
        "network_access": false
      },
      "tool_input": {
        "command": "rm /tmp/stateful-rg.out"
      }
    }"#;

    let outcome = handle_pre_tool_use(input).expect("hook input should parse");

    assert_raw_bash_denied_with_sandbox_run_guidance(outcome);
}

#[test]
fn pre_tool_use_denies_raw_stateful_control_command_even_with_legacy_read_only_sandbox() {
    let input = r#"{
      "session_id": "s1",
      "cwd": "/repo",
      "hook_event_name": "PreToolUse",
      "tool_name": "Bash",
      "sandbox": {
        "mode": "read-only",
        "writable_roots": ["/tmp"],
        "network_access": false
      },
      "tool_input": {
        "command": "stateful sync-outbox"
      }
    }"#;

    let outcome = handle_pre_tool_use(input).expect("hook input should parse");

    assert_raw_bash_denied_with_sandbox_run_guidance(outcome);
}

#[test]
fn pre_tool_use_denies_arbitrary_raw_bash_even_with_legacy_read_only_sandbox() {
    let input = r#"{
      "session_id": "s1",
      "cwd": "/repo",
      "hook_event_name": "PreToolUse",
      "tool_name": "Bash",
      "sandbox": {
        "mode": "read-only",
        "writable_roots": ["/tmp"],
        "network_access": false
      },
      "tool_input": {
        "command": "echo hi > src/auth.ts; stateful sync-outbox || true"
      }
    }"#;

    let outcome = handle_pre_tool_use(input).expect("hook input should parse");

    assert_raw_bash_denied_with_sandbox_run_guidance(outcome);
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
    let body = request_json_body(&request);
    assert_eq!(body["protocol_version"], "stateful.v1");
    assert_eq!(body["session"]["session_id"], "s1");
    assert_eq!(body["workspace"]["workspace_id"], "w1");
    assert_eq!(body["source"]["kind"], "hook");
    assert_eq!(body["source"]["event"], "pre_tool_use");
    assert_eq!(body["payload"]["action"], "write_file");
    assert_eq!(body["payload"]["path"], "src/auth.ts");
    assert_eq!(body["payload"]["queue_on_conflict"], true);
    assert!(body.get("action").is_none());

    fs::remove_dir_all(&temp_root).expect("temp root should be removable");
}

#[test]
fn pre_tool_use_apply_patch_patch_field_authorizes_every_file_target() {
    let temp_root = std::env::temp_dir().join(format!(
        "stateful-hook-patch-field-test-{}",
        std::process::id()
    ));
    if temp_root.exists() {
        fs::remove_dir_all(&temp_root).expect("old temp root should be removable");
    }
    let paths = GlobalPaths::new(temp_root.join("home"));
    let repo_root = temp_root.join("repo");
    fs::create_dir_all(&repo_root).expect("repo root should be creatable");
    enable_test_repo(&paths, &repo_root);
    let (runtime, rx) = spawn_fake_stateful_server_sequence(vec![
        r#"{"decision":"allow","reason_code":"authorized","message":"ok","required_next_action":null}"#,
        r#"{"decision":"deny","reason_code":"scope_mismatch","message":"Write target is outside active intent scope.","required_next_action":"Declare matching intent."}"#,
    ]);
    write_global_runtime_file(&paths, &runtime).expect("global runtime file should write");

    let input = serde_json::json!({
        "session_id": "s1",
        "cwd": repo_root,
        "hook_event_name": "PreToolUse",
        "tool_name": "apply_patch",
        "tool_input": {
            "patch": "*** Begin Patch\n*** Update File: doc.txt\n@@\n base\n*** Update File: persisted_doc.txt\n@@\n base\n*** End Patch\n"
        }
    })
    .to_string();

    let output = run_hook_subprocess(&repo_root, &paths, &["hook", "pre-tool-use"], &input);

    assert!(
        output.status.success(),
        "stateful hook failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let first = rx
        .recv_timeout(Duration::from_secs(2))
        .expect("first authorize request should arrive");
    let second = rx
        .recv_timeout(Duration::from_secs(2))
        .expect("second authorize request should arrive");
    assert!(first.contains("\"path\":\"doc.txt\""));
    assert!(second.contains("\"path\":\"persisted_doc.txt\""));
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
fn pre_tool_use_apply_patch_raw_string_payload_posts_authorize() {
    let temp_root = std::env::temp_dir().join(format!(
        "stateful-hook-raw-patch-test-{}",
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
        r#"{"decision":"allow","reason_code":"authorized","message":"ok","required_next_action":null}"#,
    );
    write_global_runtime_file(&paths, &runtime).expect("global runtime file should write");

    let input = serde_json::json!({
        "session_id": "s1",
        "cwd": repo_root,
        "hook_event_name": "PreToolUse",
        "tool_name": "apply_patch",
        "tool_input": "*** Begin Patch\n*** Update File: doc.txt\n@@\n base\n*** End Patch\n"
    })
    .to_string();

    let output = run_hook_subprocess(&repo_root, &paths, &["hook", "pre-tool-use"], &input);

    assert!(
        output.status.success(),
        "stateful hook failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let request = rx
        .recv_timeout(Duration::from_secs(2))
        .expect("authorize request should arrive");
    assert!(request.contains("\"action\":\"write_file\""));
    assert!(request.contains("\"path\":\"doc.txt\""));

    fs::remove_dir_all(&temp_root).expect("temp root should be removable");
}

#[test]
fn pre_tool_use_file_change_posts_authorize_for_changed_paths() {
    let temp_root = std::env::temp_dir().join(format!(
        "stateful-hook-file-change-test-{}",
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
        r#"{"decision":"allow","reason_code":"authorized","message":"ok","required_next_action":null}"#,
    );
    write_global_runtime_file(&paths, &runtime).expect("global runtime file should write");

    let input = serde_json::json!({
        "session_id": "s1",
        "cwd": repo_root,
        "hook_event_name": "PreToolUse",
        "tool_name": "file_change",
        "tool_input": {
            "changes": [
                {"path": "doc.txt", "kind": "update"}
            ]
        }
    })
    .to_string();

    let output = run_hook_subprocess(&repo_root, &paths, &["hook", "pre-tool-use"], &input);

    assert!(
        output.status.success(),
        "stateful hook failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let request = rx
        .recv_timeout(Duration::from_secs(2))
        .expect("authorize request should arrive");
    assert!(request.contains("\"action\":\"write_file\""));
    assert!(request.contains("\"path\":\"doc.txt\""));

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
    assert!(rendered.contains("Before using Bash"));
    assert!(rendered.contains("stateful-command-policy"));
    assert!(rendered.contains("--fs read-only --network disabled"));
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

fn request_json_body(request: &str) -> serde_json::Value {
    let (_, body) = request
        .split_once("\r\n\r\n")
        .expect("request should contain a body separator");
    serde_json::from_str(body).expect("request body should be json")
}

fn enable_test_repo(paths: &GlobalPaths, repo_root: &std::path::Path) {
    fs::create_dir_all(repo_root.join(".git")).expect("git marker should write");
    enable_repo(paths, repo_root, false).expect("repo should enable");
}

fn spawn_fake_stateful_server(
    actual_response: &'static str,
) -> (ServerRuntime, mpsc::Receiver<String>) {
    spawn_fake_stateful_server_sequence(vec![actual_response])
}

fn spawn_fake_stateful_server_sequence(
    actual_responses: Vec<&'static str>,
) -> (ServerRuntime, mpsc::Receiver<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("listener should bind");
    let addr = listener.local_addr().expect("listener addr should load");
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let mut actual_responses = VecDeque::from(actual_responses);
        while !actual_responses.is_empty() {
            let (mut stream, _) = listener.accept().expect("connection should arrive");
            let request = read_http_request_maybe_body(&mut stream);
            if request.contains("GET /health HTTP/1.1") {
                write_json_response(&mut stream, r#"{"status":"ok"}"#);
            } else if request.contains("GET /v1/current HTTP/1.1") {
                write_json_response(&mut stream, r#"{"status":"ok","current":{}}"#);
            } else {
                tx.send(request).expect("request should send to test");
                let response = actual_responses
                    .pop_front()
                    .expect("response should exist while loop is active");
                write_json_response(&mut stream, response);
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

fn run_hook_subprocess_with_extra_env(
    repo_root: &std::path::Path,
    paths: &GlobalPaths,
    args: &[&str],
    input: &str,
    extra_env: &[(&str, &str)],
) -> std::process::Output {
    run_hook_subprocess_from_with_extra_env(repo_root, paths, args, input, extra_env)
}

fn run_hook_subprocess_from(
    cwd: &Path,
    paths: &GlobalPaths,
    args: &[&str],
    input: &str,
) -> std::process::Output {
    run_hook_subprocess_from_with_extra_env(cwd, paths, args, input, &[])
}

fn run_hook_subprocess_from_with_extra_env(
    cwd: &Path,
    paths: &GlobalPaths,
    args: &[&str],
    input: &str,
    extra_env: &[(&str, &str)],
) -> std::process::Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_stateful"));
    command
        .args(args)
        .current_dir(cwd)
        .env_clear()
        .env("STATEFUL_HOME", &paths.home);
    for (key, value) in extra_env {
        command.env(key, value);
    }
    let mut child = command
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
