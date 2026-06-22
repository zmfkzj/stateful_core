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
    CurrentSession, GlobalPaths, HookOutcome, OmpHookOutcome, STATEFUL_SESSION_ID_ENV,
    ServerRuntime, allow_tool_for_repo, enable_repo, handle_omp_post_tool_use_with_runtime,
    handle_omp_pre_tool_use_with_runtime, handle_omp_session_start_with_runtime,
    handle_post_tool_use_in_repo, handle_pre_tool_use, handle_pre_tool_use_in_repo,
    read_current_session_file_for_session, tool_list_for_repo, write_global_runtime_file,
};
use time::{OffsetDateTime, format_description::well_known::Rfc3339};

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

fn assert_bash_denial_mentions_all(outcome: HookOutcome, expected: &[&str]) {
    let HookOutcome::Deny { reason } = outcome else {
        panic!("expected Bash denial");
    };
    for expected in expected {
        assert!(
            reason.contains(expected),
            "reason `{reason}` should contain `{expected}`"
        );
    }
}

fn trusted_stateful_path() -> String {
    std::env::current_exe()
        .expect("test executable path should resolve")
        .to_string_lossy()
        .into_owned()
}

#[cfg(feature = "codex-benchmark")]
fn trusted_tmux_path() -> &'static str {
    "/opt/homebrew/bin/tmux"
}

fn read_legacy_current_session_file(repo_root: &Path) -> CurrentSession {
    let path = repo_root.join(".stateful_core/runtime/session.json");
    let contents = fs::read_to_string(path).expect("legacy current session should read");
    serde_json::from_str(&contents).expect("legacy current session should decode")
}

#[test]
fn session_start_records_current_session_under_thread_id_when_present() {
    let temp_root = std::env::temp_dir().join(format!(
        "stateful-hook-session-start-session-id-test-{}",
        std::process::id()
    ));
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
      "session_id": "parent-session-1",
      "thread_id": "codex-thread-1",
      "transcript_path": "/tmp/transcript.jsonl",
      "cwd": "/repo",
      "hook_event_name": "SessionStart"
    }"#;

    let output = run_hook_subprocess(
        &repo_root,
        &paths,
        &["hook", "codex", "session-start"],
        input,
    );

    assert!(
        output.status.success(),
        "stateful hook failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let request = rx.recv().expect("session register request should arrive");
    assert!(request.contains("POST /v1/session/register HTTP/1.1"));
    assert!(request.contains("\"session_id\":\"codex-thread-1\""));
    let session = read_current_session_file_for_session(&repo_root, "codex-thread-1")
        .expect("current session should be keyed by thread id");
    assert_eq!(session.session_id, "codex-thread-1");
    assert_eq!(session.workspace_id, "w1");

    fs::remove_dir_all(&temp_root).expect("temp root should be removable");
}

#[test]
fn session_start_derives_workspace_id_for_default_local_runtime() {
    let temp_root = std::env::temp_dir().join(format!(
        "stateful-hook-derived-workspace-test-{}",
        std::process::id()
    ));
    if temp_root.exists() {
        fs::remove_dir_all(&temp_root).expect("old temp root should be removable");
    }
    let paths = GlobalPaths::new(temp_root.join("home"));
    let repo_root = temp_root.join("repo");
    fs::create_dir_all(&repo_root).expect("repo root should be creatable");
    enable_test_repo(&paths, &repo_root);
    let (mut runtime, rx) = spawn_fake_stateful_server(r#"{"status":"ok"}"#);
    runtime.workspace_id = "local".to_string();
    write_global_runtime_file(&paths, &runtime).expect("global runtime file should write");

    let input = r#"{
      "session_id": "derived-session",
      "thread_id": "derived-thread-1",
      "transcript_path": "/tmp/transcript.jsonl",
      "cwd": "/repo",
      "hook_event_name": "SessionStart"
    }"#;

    let output = run_hook_subprocess(
        &repo_root,
        &paths,
        &["hook", "codex", "session-start"],
        input,
    );

    assert!(
        output.status.success(),
        "stateful hook failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let request = rx.recv().expect("session register request should arrive");
    let body = request_json_body(&request);
    let workspace_id = body["workspace_id"]
        .as_str()
        .expect("workspace_id should be a string");
    assert!(workspace_id.starts_with("workspace-"));
    assert_ne!(workspace_id, "local");

    assert_eq!(body["session_id"], "derived-thread-1");

    let session = read_current_session_file_for_session(&repo_root, "derived-thread-1")
        .expect("current session should be keyed by thread id");
    assert_eq!(session.session_id, "derived-thread-1");
    assert_eq!(session.workspace_id, workspace_id);

    fs::remove_dir_all(&temp_root).expect("temp root should be removable");
}

#[test]
fn session_start_derives_workspace_id_for_default_shared_runtime() {
    let temp_root = std::env::temp_dir().join(format!(
        "stateful-hook-derived-shared-workspace-test-{}",
        std::process::id()
    ));
    if temp_root.exists() {
        fs::remove_dir_all(&temp_root).expect("old temp root should be removable");
    }
    let paths = GlobalPaths::new(temp_root.join("home"));
    let repo_root = temp_root.join("repo");
    fs::create_dir_all(&repo_root).expect("repo root should be creatable");
    enable_test_repo(&paths, &repo_root);
    let (mut runtime, rx) = spawn_fake_stateful_server(r#"{"status":"ok"}"#);
    runtime.workspace_id = "shared".to_string();
    write_global_runtime_file(&paths, &runtime).expect("global runtime file should write");

    let input = r#"{
      "session_id": "derived-shared-session",
      "thread_id": "derived-shared-thread-1",
      "transcript_path": "/tmp/transcript.jsonl",
      "cwd": "/repo",
      "hook_event_name": "SessionStart"
    }"#;

    let output = run_hook_subprocess(
        &repo_root,
        &paths,
        &["hook", "codex", "session-start"],
        input,
    );

    assert!(
        output.status.success(),
        "stateful hook failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let request = rx.recv().expect("session register request should arrive");
    let body = request_json_body(&request);
    let workspace_id = body["workspace_id"]
        .as_str()
        .expect("workspace_id should be a string");
    assert!(workspace_id.starts_with("workspace-"));
    assert_ne!(workspace_id, "shared");

    assert_eq!(body["session_id"], "derived-shared-thread-1");

    let session = read_current_session_file_for_session(&repo_root, "derived-shared-thread-1")
        .expect("current session should be keyed by thread id");
    assert_eq!(session.session_id, "derived-shared-thread-1");
    assert_eq!(session.workspace_id, workspace_id);

    fs::remove_dir_all(&temp_root).expect("temp root should be removable");
}

#[test]
fn pre_tool_use_authorization_uses_thread_id_when_present() {
    let temp_root = std::env::temp_dir().join(format!(
        "stateful-hook-pre-tool-session-id-test-{}",
        std::process::id()
    ));
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
      "session_id": "parent-session-1",
      "thread_id": "codex-thread-1",
      "transcript_path": "/tmp/transcript.jsonl",
      "cwd": "/repo",
      "hook_event_name": "PreToolUse",
      "tool_name": "apply_patch",
      "tool_input": {
        "command": "*** Begin Patch\n*** Update File: src/auth.ts\n*** End Patch\n"
      }
    }"#;

    let output = run_hook_subprocess(
        &repo_root,
        &paths,
        &["hook", "codex", "pre-tool-use"],
        input,
    );

    assert!(
        output.status.success(),
        "stateful hook failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let request = rx.recv().expect("captured request should arrive");
    let body = request_json_body(&request);
    assert_eq!(body["session"]["session_id"], "codex-thread-1");

    fs::remove_dir_all(&temp_root).expect("temp root should be removable");
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
    assert!(reason.contains("state_current_read"));
    assert!(reason.contains("state_intent_declare"));
    assert!(reason.contains("state_lease_acquire"));
    assert!(reason.contains("--fs read-only --network disabled"));
    assert!(reason.contains("--fs build --network enabled"));
    assert!(reason.contains("--fs write-targets --write-target <file>"));
    assert!(reason.contains("--command"));
}

#[test]
fn pre_tool_use_requires_read_only_sandbox_for_shell_read_fallback() {
    let input = r#"{
      "session_id": "s1",
      "cwd": "/repo",
      "hook_event_name": "PreToolUse",
      "tool_name": "Bash",
      "tool_input": {
        "command": "cat README.md"
      }
    }"#;

    let outcome = handle_pre_tool_use(input).expect("hook input should parse");

    assert_bash_denial_mentions_all(
        outcome,
        &[
            "Raw Bash is denied",
            "stateful-command-policy",
            "--fs read-only --network disabled",
        ],
    );
}

#[test]
fn pre_tool_use_allows_sandbox_external_for_repo_external_write_approval_path() {
    let stateful = trusted_stateful_path();
    let input = serde_json::json!({
        "session_id": "s1",
        "cwd": "/repo",
        "hook_event_name": "PreToolUse",
        "tool_name": "Bash",
        "tool_input": {
            "command": format!("{stateful} sandbox run --fs external --purpose 'write external artifact' --write-target /tmp/stateful-outside.txt --command 'printf ok > /tmp/stateful-outside.txt'")
        }
    })
    .to_string();

    let outcome = handle_pre_tool_use(&input).expect("hook input should parse");

    assert_eq!(outcome, HookOutcome::Allow);
}

#[test]
fn pre_tool_use_denies_external_run_request_for_repo_external_write() {
    let stateful = trusted_stateful_path();
    let input = serde_json::json!({
        "session_id": "s1",
        "cwd": "/repo",
        "hook_event_name": "PreToolUse",
        "tool_name": "Bash",
        "tool_input": {
            "command": format!("{stateful} external-run request --purpose 'write external artifact' --write-target /tmp/stateful-outside.txt --command 'printf ok > /tmp/stateful-outside.txt'")
        }
    })
    .to_string();

    let outcome = handle_pre_tool_use(&input).expect("hook input should parse");

    assert_bash_denial_mentions(outcome, "stateful sandbox run");
}

#[test]
fn pre_tool_use_denies_raw_repo_external_write_without_approval_wrapper() {
    let input = r#"{
      "session_id": "s1",
      "cwd": "/repo",
      "hook_event_name": "PreToolUse",
      "tool_name": "Bash",
      "tool_input": {
        "command": "printf ok > /tmp/stateful-outside.txt"
      }
    }"#;

    let outcome = handle_pre_tool_use(input).expect("hook input should parse");

    assert_bash_denial_mentions_all(
        outcome,
        &[
            "Raw Bash is denied",
            "stateful sandbox run",
            "stateful-command-policy",
        ],
    );
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
fn pre_tool_use_allows_structured_process_find() {
    let stateful = trusted_stateful_path();
    let input = serde_json::json!({
        "session_id": "s1",
        "cwd": "/repo",
        "hook_event_name": "PreToolUse",
        "tool_name": "Bash",
        "tool_input": {
            "command": format!("{stateful} sandbox process find --contains denovo_codex_agent")
        }
    })
    .to_string();

    let outcome = handle_pre_tool_use(&input).expect("hook input should parse");

    assert_eq!(outcome, HookOutcome::Allow);
}

#[test]
fn pre_tool_use_denies_process_find_without_selector() {
    let stateful = trusted_stateful_path();
    let input = serde_json::json!({
        "session_id": "s1",
        "cwd": "/repo",
        "hook_event_name": "PreToolUse",
        "tool_name": "Bash",
        "tool_input": {
            "command": format!("{stateful} sandbox process find")
        }
    })
    .to_string();

    let outcome = handle_pre_tool_use(&input).expect("hook input should parse");

    assert_bash_denial_mentions(
        outcome,
        "stateful sandbox process find requires at least one selector",
    );
}

#[test]
fn pre_tool_use_denies_sandbox_run_with_raw_process_inspection() {
    let stateful = trusted_stateful_path();
    let input = serde_json::json!({
        "session_id": "s1",
        "cwd": "/repo",
        "hook_event_name": "PreToolUse",
        "tool_name": "Bash",
        "tool_input": {
            "command": format!("{stateful} sandbox run --fs read-only --network disabled --command 'pgrep -f denovo_codex_agent'")
        }
    })
    .to_string();

    let outcome = handle_pre_tool_use(&input).expect("hook input should parse");

    assert_bash_denial_mentions(
        outcome,
        "process inspection must use stateful sandbox process find",
    );
}

#[test]
fn pre_tool_use_denies_sandbox_run_with_wrapped_raw_process_inspection() {
    let stateful = trusted_stateful_path();
    let input = serde_json::json!({
        "session_id": "s1",
        "cwd": "/repo",
        "hook_event_name": "PreToolUse",
        "tool_name": "Bash",
        "tool_input": {
            "command": format!("{stateful} sandbox run --fs read-only --network disabled --command 'env -i pgrep -f denovo_codex_agent'")
        }
    })
    .to_string();

    let outcome = handle_pre_tool_use(&input).expect("hook input should parse");

    assert_bash_denial_mentions(
        outcome,
        "process inspection must use stateful sandbox process find",
    );
}

#[test]
fn pre_tool_use_denies_untrusted_process_find() {
    let input = serde_json::json!({
        "session_id": "s1",
        "cwd": "/repo",
        "hook_event_name": "PreToolUse",
        "tool_name": "Bash",
        "tool_input": {
            "command": "/bin/echo sandbox process find --contains denovo_codex_agent"
        }
    })
    .to_string();

    let outcome = handle_pre_tool_use(&input).expect("hook input should parse");

    assert_bash_denial_mentions(
        outcome,
        "stateful sandbox process find requires the trusted absolute stateful binary",
    );
}

#[test]
fn pre_tool_use_denies_read_only_sandbox_run_with_network_enabled() {
    let stateful = trusted_stateful_path();
    let input = serde_json::json!({
        "session_id": "s1",
        "cwd": "/repo",
        "hook_event_name": "PreToolUse",
        "tool_name": "Bash",
        "tool_input": {
            "command": format!("{stateful} sandbox run --fs read-only --network enabled --command 'curl https://example.test'")
        }
    })
    .to_string();

    let outcome = handle_pre_tool_use(&input).expect("hook input should parse");

    assert_bash_denial_mentions(outcome, "read-only sandbox run requires --network disabled");
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
fn pre_tool_use_allows_sandbox_run_git_profile_for_git_commands() {
    let stateful = trusted_stateful_path();
    let input = serde_json::json!({
        "session_id": "s1",
        "cwd": "/repo",
        "hook_event_name": "PreToolUse",
        "tool_name": "Bash",
        "tool_input": {
            "command": format!("{stateful} sandbox run --fs git --network enabled --timeout-seconds 30 --command 'git fetch --all'")
        }
    })
    .to_string();

    let outcome = handle_pre_tool_use(&input).expect("hook input should parse");

    assert_eq!(outcome, HookOutcome::Allow);
}

#[test]
fn pre_tool_use_allows_sandbox_run_github_pr_profile_for_pr_commands() {
    let stateful = trusted_stateful_path();
    for command in [
        "gh pr list",
        "gh pr view 123 --json title,url",
        "gh pr status",
        "gh pr create --title 'Update policy' --body 'Adds github-pr profile' --base dev --head feature --draft",
    ] {
        let input = serde_json::json!({
            "session_id": "s1",
            "cwd": "/repo",
            "hook_event_name": "PreToolUse",
            "tool_name": "Bash",
            "tool_input": {
                "command": format!("{stateful} sandbox run --fs github-pr --network enabled --timeout-seconds 30 --command {command:?}")
            }
        })
        .to_string();

        let outcome = handle_pre_tool_use(&input).expect("hook input should parse");
        assert_eq!(
            outcome,
            HookOutcome::Allow,
            "expected `{command}` to be allowed"
        );
    }
}

#[test]
fn pre_tool_use_denies_sandbox_run_github_pr_profile_unsafe_requests() {
    let stateful = trusted_stateful_path();
    let cases = [
        (
            "network disabled",
            format!(
                "{stateful} sandbox run --fs github-pr --network disabled --command 'gh pr status'"
            ),
            "github-pr sandbox run requires --network enabled",
        ),
        (
            "explicit write target",
            format!(
                "{stateful} sandbox run --fs github-pr --network enabled --write-target README.md --command 'gh pr status'"
            ),
            "github-pr profile manages transient PR state automatically",
        ),
        (
            "non pr gh command",
            format!(
                "{stateful} sandbox run --fs github-pr --network enabled --command 'gh auth status'"
            ),
            "github-pr profile requires a single gh pr command",
        ),
        (
            "merge",
            format!(
                "{stateful} sandbox run --fs github-pr --network enabled --command 'gh pr merge 123'"
            ),
            "github-pr profile does not allow gh pr subcommand `merge`",
        ),
        (
            "web",
            format!(
                "{stateful} sandbox run --fs github-pr --network enabled --command 'gh pr create --web'"
            ),
            "github-pr profile does not allow interactive/browser flag `--web`",
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
fn pre_tool_use_denies_sandbox_run_git_profile_dispatch_surfaces() {
    let stateful = trusted_stateful_path();
    let cases = [
        (
            "alias config",
            format!(
                "{stateful} sandbox run --fs git --network enabled --command \"git -c alias.x='!curl https://example.test' x\""
            ),
        ),
        (
            "config env",
            format!(
                "{stateful} sandbox run --fs git --network enabled --command 'git --config-env=alias.x=STATEFUL_ALIAS x'"
            ),
        ),
        (
            "submodule foreach",
            format!(
                "{stateful} sandbox run --fs git --network enabled --command \"git submodule foreach 'printf nope'\""
            ),
        ),
        (
            "rebase exec",
            format!(
                "{stateful} sandbox run --fs git --network enabled --command \"git rebase --exec 'printf nope'\""
            ),
        ),
        (
            "grep pager",
            format!(
                "{stateful} sandbox run --fs git --network enabled --command \"git grep --open-files-in-pager='sh -c id' TODO\""
            ),
        ),
        (
            "remote mutation",
            format!(
                "{stateful} sandbox run --fs git --network enabled --command 'git remote add origin https://example.test/repo.git'"
            ),
        ),
    ];

    for (name, command) in cases {
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
            reason.contains("git profile"),
            "{name}: reason `{reason}` should mention git profile"
        );
    }
}

#[test]
fn pre_tool_use_allows_sandbox_run_write_dir() {
    let stateful = trusted_stateful_path();
    let input = serde_json::json!({
        "session_id": "s1",
        "cwd": "/repo",
        "hook_event_name": "PreToolUse",
        "tool_name": "Bash",
        "tool_input": {
            "command": format!("{stateful} sandbox run --fs write-targets --network enabled --write-dir tmp/run-1 --command 'cargo test'")
        }
    })
    .to_string();

    let outcome = handle_pre_tool_use(&input).expect("hook input should parse");

    assert_eq!(outcome, HookOutcome::Allow);
}

#[test]
fn pre_tool_use_allows_sandbox_run_direct_tmp_write_dir() {
    let stateful = trusted_stateful_path();
    let input = serde_json::json!({
        "session_id": "s1",
        "cwd": "/repo",
        "hook_event_name": "PreToolUse",
        "tool_name": "Bash",
        "tool_input": {
            "command": format!("{stateful} sandbox run --fs write-targets --network enabled --write-dir tmp --command 'cargo test'")
        }
    })
    .to_string();

    let outcome = handle_pre_tool_use(&input).expect("hook input should parse");

    assert_eq!(outcome, HookOutcome::Allow);
}

#[test]
fn pre_tool_use_allows_sandbox_run_write_dir_outside_artifact_tree() {
    let stateful = trusted_stateful_path();
    let input = serde_json::json!({
        "session_id": "s1",
        "cwd": "/repo",
        "hook_event_name": "PreToolUse",
        "tool_name": "Bash",
        "tool_input": {
            "command": format!("{stateful} sandbox run --fs write-targets --network enabled --write-dir target --command 'cargo test'")
        }
    })
    .to_string();

    let outcome = handle_pre_tool_use(&input).expect("hook input should parse");

    assert_eq!(outcome, HookOutcome::Allow);
}

#[test]
fn pre_tool_use_allows_sandbox_run_build_profile_with_scoped_tmp_write_dir() {
    let stateful = trusted_stateful_path();
    let input = serde_json::json!({
        "session_id": "s1",
        "cwd": "/repo",
        "hook_event_name": "PreToolUse",
        "tool_name": "Bash",
        "tool_input": {
            "command": format!("{stateful} sandbox run --fs build --network enabled --write-dir build-tests --command 'npm test'")
        }
    })
    .to_string();

    let outcome = handle_pre_tool_use(&input).expect("hook input should parse");

    assert_eq!(outcome, HookOutcome::Allow);
}

#[test]
fn pre_tool_use_denies_sandbox_run_build_profile_without_scoped_tmp_write_dir() {
    let stateful = trusted_stateful_path();
    let input = serde_json::json!({
        "session_id": "s1",
        "cwd": "/repo",
        "hook_event_name": "PreToolUse",
        "tool_name": "Bash",
        "tool_input": {
            "command": format!("{stateful} sandbox run --fs build --network enabled --command 'npm test'")
        }
    })
    .to_string();

    let outcome = handle_pre_tool_use(&input).expect("hook input should parse");

    assert_bash_denial_mentions(outcome, "--write-dir <scratch-purpose>");
}

#[test]
#[cfg(not(feature = "codex-benchmark"))]
fn pre_tool_use_denies_nested_codex_benchmark_sandbox_without_feature() {
    let stateful = trusted_stateful_path();
    let input = serde_json::json!({
        "session_id": "s1",
        "cwd": "/repo",
        "hook_event_name": "PreToolUse",
        "tool_name": "Bash",
        "tool_input": {
            "command": format!(
                "{stateful} sandbox run-nested-codex-benchmark --purpose 'run nested Codex chaos benchmark' --write-dir target --codex-home-root target/nested-codex-homes/run-1 --timeout-seconds 120 --command 'cargo run -p stateful-bench -- run'"
            )
        }
    })
    .to_string();

    let outcome = handle_pre_tool_use(&input).expect("hook input should parse");

    assert_bash_denial_mentions(
        outcome,
        "run-nested-codex-benchmark hook authorization requires the codex-benchmark feature",
    );
}

#[test]
#[cfg(feature = "codex-benchmark")]
fn pre_tool_use_allows_nested_codex_benchmark_sandbox_with_feature() {
    let stateful = trusted_stateful_path();
    let input = serde_json::json!({
        "session_id": "s1",
        "cwd": "/repo",
        "hook_event_name": "PreToolUse",
        "tool_name": "Bash",
        "tool_input": {
            "command": format!(
                "{stateful} sandbox run-nested-codex-benchmark --purpose 'run nested Codex chaos benchmark' --write-dir target --codex-home-root target/nested-codex-homes/run-1 --timeout-seconds 120 --command 'cargo run -p stateful-bench -- run'"
            )
        }
    })
    .to_string();

    let outcome = handle_pre_tool_use(&input).expect("hook input should parse");

    assert_eq!(outcome, HookOutcome::Allow);
}

#[test]
#[cfg(feature = "codex-benchmark")]
fn pre_tool_use_allows_nested_codex_benchmark_sandbox_with_docker_socket() {
    let stateful = trusted_stateful_path();
    let input = serde_json::json!({
        "session_id": "s1",
        "cwd": "/repo",
        "hook_event_name": "PreToolUse",
        "tool_name": "Bash",
        "tool_input": {
            "command": format!(
                "{stateful} sandbox run-nested-codex-benchmark --purpose bench --write-dir target --codex-home-root target/nested-codex-homes/run-1 --docker-socket /private/tmp/docker.sock --command true"
            )
        }
    })
    .to_string();

    let outcome = handle_pre_tool_use(&input).expect("hook input should parse");

    assert_eq!(outcome, HookOutcome::Allow);
}

#[test]
#[cfg(feature = "codex-benchmark")]
fn pre_tool_use_allows_tmux_new_session_for_nested_codex_benchmark() {
    let stateful = trusted_stateful_path();
    let tmux = trusted_tmux_path();
    let nested = format!(
        "{stateful} sandbox run-nested-codex-benchmark --purpose 'run DeNovoSWE full dataset Codex benchmark fixture a' --write-dir target --codex-home-root target/nested-codex-homes/fixture-denovo-full-3-codex-a --docker-socket /private/tmp/docker.sock --timeout-seconds 43200 --command 'target/debug/stateful-bench denovo run --run-id fixture-denovo-full-3-codex-a'"
    );
    let input = serde_json::json!({
        "session_id": "s1",
        "cwd": "/repo",
        "hook_event_name": "PreToolUse",
        "tool_name": "Bash",
        "tool_input": {
            "command": format!(
                "{tmux} new-session -d -s fixture-denovo-full-3-codex-a -c /repo \"{nested}\""
            )
        }
    })
    .to_string();

    let outcome = handle_pre_tool_use(&input).expect("hook input should parse");

    assert_eq!(outcome, HookOutcome::Allow);
}

#[test]
#[cfg(feature = "codex-benchmark")]
fn pre_tool_use_denies_tmux_send_keys_even_for_benchmark_sessions() {
    let tmux = trusted_tmux_path();
    let input = serde_json::json!({
        "session_id": "s1",
        "cwd": "/repo",
        "hook_event_name": "PreToolUse",
        "tool_name": "Bash",
        "tool_input": {
            "command": format!(
                "{tmux} send-keys -t fixture-denovo-full-3-codex-a 'rm -rf target' C-m"
            )
        }
    })
    .to_string();

    let outcome = handle_pre_tool_use(&input).expect("hook input should parse");

    assert_bash_denial_mentions(outcome, "tmux benchmark launcher supports only new-session");
}

#[test]
fn pre_tool_use_denies_external_run_request() {
    let stateful = trusted_stateful_path();
    let input = serde_json::json!({
        "session_id": "s1",
        "cwd": "/repo",
        "hook_event_name": "PreToolUse",
        "tool_name": "Bash",
        "tool_input": {
            "command": format!("{stateful} external-run request --purpose 'install rebuilt binaries' --write-dir /Users/me/.cargo/bin --command 'install -m 755 target/release/stateful /Users/me/.cargo/bin/stateful'")
        }
    })
    .to_string();

    let outcome = handle_pre_tool_use(&input).expect("hook input should parse");

    assert_bash_denial_mentions(outcome, "stateful sandbox run");
}

#[test]
fn pre_tool_use_denies_external_run_approve_and_run() {
    let stateful = trusted_stateful_path();
    let input = serde_json::json!({
        "session_id": "s1",
        "cwd": "/repo",
        "hook_event_name": "PreToolUse",
        "tool_name": "Bash",
        "tool_input": {
            "command": format!("{stateful} external-run approve request-123 --run")
        }
    })
    .to_string();

    let outcome = handle_pre_tool_use(&input).expect("hook input should parse");

    assert_bash_denial_mentions(outcome, "stateful sandbox run");
}

#[test]
fn pre_tool_use_denies_external_run_run() {
    let stateful = trusted_stateful_path();
    let input = serde_json::json!({
        "session_id": "s1",
        "cwd": "/repo",
        "hook_event_name": "PreToolUse",
        "tool_name": "Bash",
        "tool_input": {
            "command": format!("{stateful} external-run run request-123")
        }
    })
    .to_string();

    let outcome = handle_pre_tool_use(&input).expect("hook input should parse");

    assert_bash_denial_mentions(outcome, "stateful sandbox run");
}

#[test]
fn pre_tool_use_denies_trusted_stateful_git_wrappers() {
    let stateful = trusted_stateful_path();
    let cases = [
        format!("{stateful} commit -m docs-update -- README.md"),
        format!("{stateful} pull"),
        format!("{stateful} push origin dev"),
    ];

    for command in cases {
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

        assert_bash_denial_mentions(outcome, "stateful sandbox run");
    }
}

#[test]
fn pre_tool_use_allows_trusted_stateful_server_control() {
    let stateful = trusted_stateful_path();
    let cases = [
        format!("{stateful} server stop"),
        format!("{stateful} server start"),
    ];

    for command in cases {
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

        assert_eq!(outcome, HookOutcome::Allow);
    }
}

#[test]
fn pre_tool_use_denies_external_run_with_outer_command_separator() {
    let stateful = trusted_stateful_path();
    let input = serde_json::json!({
        "session_id": "s1",
        "cwd": "/repo",
        "hook_event_name": "PreToolUse",
        "tool_name": "Bash",
        "tool_input": {
            "command": format!("{stateful} external-run approve request-123 --run; rm README.md")
        }
    })
    .to_string();

    let outcome = handle_pre_tool_use(&input).expect("hook input should parse");

    assert_bash_denial_mentions(outcome, "single stateful sandbox run command");
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

    assert_bash_denial_mentions(
        outcome,
        "requires at least one write target, create target, or write dir",
    );
}

#[test]
#[cfg(feature = "codex-benchmark")]
fn pre_tool_use_denies_invalid_nested_codex_benchmark_sandbox_wrappers() {
    let stateful = trusted_stateful_path();
    let cases = [
        (
            "missing purpose",
            format!(
                "{stateful} sandbox run-nested-codex-benchmark --write-dir target --codex-home-root target/nested-codex-homes/run-1 --command 'cargo test'"
            ),
            "requires --purpose",
        ),
        (
            "missing home root",
            format!(
                "{stateful} sandbox run-nested-codex-benchmark --purpose 'run nested Codex chaos benchmark' --write-dir target --command 'cargo test'"
            ),
            "requires --codex-home-root",
        ),
        (
            "source write dir",
            format!(
                "{stateful} sandbox run-nested-codex-benchmark --purpose 'run nested Codex chaos benchmark' --write-dir crates --codex-home-root target/nested-codex-homes/run-1 --command 'cargo test'"
            ),
            "requires --write-dir target",
        ),
        (
            "home outside target",
            format!(
                "{stateful} sandbox run-nested-codex-benchmark --purpose 'run nested Codex chaos benchmark' --write-dir target --codex-home-root /Users/me/.codex --command 'cargo test'"
            ),
            "requires --codex-home-root under target",
        ),
        (
            "unsupported write target",
            format!(
                "{stateful} sandbox run-nested-codex-benchmark --purpose 'run nested Codex chaos benchmark' --write-dir target --codex-home-root target/nested-codex-homes/run-1 --write-target README.md --command 'cargo test'"
            ),
            "unsupported stateful sandbox run-nested-codex-benchmark argument",
        ),
        (
            "generic relaxed profile",
            format!(
                "{stateful} sandbox run --fs relaxed --network enabled --write-dir target --command 'cargo test'"
            ),
            "supports only read-only, write-targets, external, build, git, and github-pr profiles",
        ),
        (
            "build profile with explicit write target",
            format!(
                "{stateful} sandbox run --fs build --network enabled --write-target README.md --command 'npm test'"
            ),
            "build profile rejects explicit write targets and create targets",
        ),
        (
            "git profile with non-git command",
            format!("{stateful} sandbox run --fs git --network enabled --command 'rm README.md'"),
            "git profile requires a single git command",
        ),
        (
            "git profile with shell control in inner command",
            format!(
                "{stateful} sandbox run --fs git --network enabled --command 'git status; rm README.md'"
            ),
            "git profile requires a single git command",
        ),
        (
            "git profile with explicit write target",
            format!(
                "{stateful} sandbox run --fs git --network enabled --write-target README.md --command 'git status'"
            ),
            "git profile manages repo writes automatically",
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
        assert_bash_denial_mentions(outcome, expected);
        let _ = name;
    }
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
            "supports only read-only, write-targets, external, build, git, and github-pr profiles",
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

    let output = run_hook_subprocess(
        &repo_root,
        &paths,
        &["hook", "codex", "pre-tool-use"],
        input,
    );

    assert!(
        output.status.success(),
        "stateful hook failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let session = read_legacy_current_session_file(&repo_root);
    assert_eq!(session.session_id, "s-current");
    assert_eq!(session.workspace_id, "w1");

    fs::remove_dir_all(&temp_root).expect("temp root should be removable");
}

#[test]
fn pre_tool_use_records_current_session_for_stateful_session_id() {
    let temp_root = std::env::temp_dir().join(format!(
        "stateful-hook-stateful-session-test-{}",
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
        &["hook", "codex", "pre-tool-use"],
        input,
        &[(STATEFUL_SESSION_ID_ENV, "s-current")],
    );

    assert!(
        output.status.success(),
        "stateful hook failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let session = read_current_session_file_for_session(&repo_root, "s-current")
        .expect("session-bound current session should read");
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

    let output = run_hook_subprocess(&subdir, &paths, &["hook", "codex", "pre-tool-use"], input);

    assert!(
        output.status.success(),
        "stateful hook failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let _request = rx.recv().expect("captured request should arrive");
    let session = read_legacy_current_session_file(&repo_root);
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
fn pre_tool_use_allows_read_only_sandbox_when_runtime_unreachable() {
    let temp_root = std::env::temp_dir().join(format!(
        "stateful-hook-readonly-unreachable-runtime-test-{}",
        std::process::id()
    ));
    if temp_root.exists() {
        fs::remove_dir_all(&temp_root).expect("old temp root should be removable");
    }
    let paths = GlobalPaths::new(temp_root.join("home"));
    let repo_root = temp_root.join("repo");
    fs::create_dir_all(&repo_root).expect("repo root should be creatable");
    enable_test_repo(&paths, &repo_root);
    let stateful = env!("CARGO_BIN_EXE_stateful");
    let input = serde_json::json!({
        "session_id": "s1",
        "cwd": "/repo",
        "hook_event_name": "PreToolUse",
        "tool_name": "Bash",
        "tool_input": {
            "command": format!("{stateful} sandbox run --fs read-only --network disabled --command 'rg auth src'")
        }
    })
    .to_string();

    let output = run_hook_subprocess_with_extra_env(
        &repo_root,
        &paths,
        &["hook", "codex", "pre-tool-use"],
        &input,
        &[
            ("STATEFUL_SERVER_URL", "http://127.0.0.1:9"),
            ("STATEFUL_SERVER_TOKEN", "unreachable-token"),
        ],
    );

    assert!(
        output.status.success(),
        "stateful hook failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        output.stdout.is_empty(),
        "allowed hook should not print a denial: {}",
        String::from_utf8_lossy(&output.stdout)
    );

    fs::remove_dir_all(&temp_root).expect("temp root should be removable");
}

#[test]
fn pre_tool_use_denies_native_write_when_runtime_unreachable() {
    let temp_root = std::env::temp_dir().join(format!(
        "stateful-hook-write-unreachable-runtime-test-{}",
        std::process::id()
    ));
    if temp_root.exists() {
        fs::remove_dir_all(&temp_root).expect("old temp root should be removable");
    }
    let paths = GlobalPaths::new(temp_root.join("home"));
    let repo_root = temp_root.join("repo");
    fs::create_dir_all(&repo_root).expect("repo root should be creatable");
    enable_test_repo(&paths, &repo_root);
    let input = r#"{
      "session_id": "s1",
      "cwd": "/repo",
      "hook_event_name": "PreToolUse",
      "tool_name": "apply_patch",
      "tool_input": {
        "command": "*** Begin Patch\n*** Update File: src/auth.ts\n*** End Patch\n"
      }
    }"#;

    let output = run_hook_subprocess_with_extra_env(
        &repo_root,
        &paths,
        &["hook", "codex", "pre-tool-use"],
        input,
        &[
            ("STATEFUL_SERVER_URL", "http://127.0.0.1:9"),
            ("STATEFUL_SERVER_TOKEN", "unreachable-token"),
        ],
    );

    assert!(
        output.status.success(),
        "stateful hook should return a structured denial, not crash: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("\"permissionDecision\":\"deny\""));
    assert!(stdout.contains("reachable stateful server"));

    fs::remove_dir_all(&temp_root).expect("temp root should be removable");
}

#[test]
fn pre_tool_use_denies_raw_stateful_intent_declare_with_mcp_guidance() {
    let input = r#"{
      "session_id": "s1",
      "cwd": "/repo",
      "hook_event_name": "PreToolUse",
      "tool_name": "Bash",
      "tool_input": {
      "command": "stateful intent declare --session-id s1 --workspace-id w1 --purpose 'Fix auth validation behavior.' src/auth.ts"
      }
    }"#;

    let outcome = handle_pre_tool_use(input).expect("hook input should parse");

    assert_bash_denial_mentions_all(
        outcome,
        &[
            "Use canonical Stateful MCP tool names",
            "state_intent_declare",
            "state_lease_acquire",
            "Do not run `stateful intent declare`",
        ],
    );
}

#[test]
fn pre_tool_use_denies_stateful_mcp_call_intent_declare_with_mcp_guidance() {
    let input = r#"{
      "session_id": "s1",
      "cwd": "/repo",
      "hook_event_name": "PreToolUse",
      "tool_name": "Bash",
      "tool_input": {
        "command": "stateful mcp call state_intent_declare"
      }
    }"#;

    let outcome = handle_pre_tool_use(input).expect("hook input should parse");

    assert_bash_denial_mentions_all(
        outcome,
        &[
            "Use canonical Stateful MCP tool names",
            "state_intent_declare",
            "state_lease_acquire",
            "`stateful mcp call` through Bash",
        ],
    );
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
        &["hook", "codex", "pre-tool-use"],
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
fn pre_tool_use_allows_known_non_repo_write_tools_without_runtime() {
    for tool_name in [
        "Read",
        "Grep",
        "Glob",
        "LS",
        "NotebookRead",
        "WebFetch",
        "WebSearch",
        "TodoWrite",
        "update_plan",
        "tool_search",
        "tool_search_tool",
        "get_goal",
        "search_tool_bm25",
        "create_goal",
        "update_goal",
        "request_user_input",
        "view_image",
        "spawn_agent",
        "multi_agent_v1spawn_agent",
        "wait_agent",
        "multi_agent_v1wait_agent",
        "send_input",
        "multi_agent_v1send_input",
        "close_agent",
        "multi_agent_v1close_agent",
        "resume_agent",
        "multi_agent_v1resume_agent",
        "state_intent_declare",
        "state_lease_acquire",
        "state_current_read",
        "state_context_render",
        "mcp__stateful__state_intent_declare",
        "mcp__stateful__state_lease_acquire",
        "mcp__stateful_state_intent_declare",
        "mcp__stateful_state_lease_acquire",
        "mcp__stateful_state_current_read",
        "mcp__stateful_state_context_render",
        "mcp__stateful__state_current_read",
        "mcp__stateful__state_context_render",
        "mcp__codex_apps__github__update_pull_request",
        "mcp__codex_apps__github__create_pull_request",
        "mcp__codex_apps__github__add_comment_to_issue",
        "mcp__codex_apps__github__fetch_file",
        "mcp__codex_apps__github__search_branches",
        "mcp__codex_apps__github__search_repositories",
        "mcp__codex_apps__microsoft_teams__resolve_channel",
        "mcp__codex_apps__microsoft_teams__send_message",
    ] {
        let input = serde_json::json!({
            "session_id": "s1",
            "cwd": "/repo",
            "hook_event_name": "PreToolUse",
            "tool_name": tool_name,
            "tool_input": {}
        })
        .to_string();

        let outcome = handle_pre_tool_use(&input).expect("hook input should parse");

        assert_eq!(
            outcome,
            HookOutcome::Allow,
            "{tool_name} should be classified as safe without repo write authorization"
        );
    }
}

#[test]
fn pre_tool_use_denies_unclassified_tool_names() {
    for tool_name in [
        "FutureWriteTool",
        "mcp__codex_apps__github__merge_pull_request",
        "mcp__codex_apps__microsoft_teams__delete_message",
    ] {
        let input = serde_json::json!({
          "session_id": "s1",
          "cwd": "/repo",
          "hook_event_name": "PreToolUse",
          "tool_name": tool_name,
          "tool_input": {
            "file_path": "src/auth.ts"
          }
        })
        .to_string();

        let outcome = handle_pre_tool_use(&input).expect("hook input should parse");

        let HookOutcome::Deny { reason } = outcome else {
            panic!("{tool_name} should be denied");
        };
        assert!(
            reason.contains(&format!("unclassified tool {tool_name}")),
            "reason `{reason}` should name the unclassified tool"
        );
        assert!(
            reason.contains("write or execute"),
            "reason `{reason}` should explain why classification is required"
        );
    }
}

#[test]
fn pre_tool_use_bash_denial_in_repo_includes_live_context() {
    let temp_root = std::env::temp_dir().join(format!(
        "stateful-hook-bash-denial-context-test-{}",
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
        r#"{"status":"ok","prompt_text":"Nearby Activity\n- [info] src/auth.ts: Session s2 declared intent for src/auth.ts."}"#,
    ]);
    write_global_runtime_file(&paths, &runtime).expect("global runtime file should write");

    let input = serde_json::json!({
        "session_id": "s1",
        "cwd": repo_root,
        "hook_event_name": "PreToolUse",
        "tool_name": "Bash",
        "tool_input": {
            "command": "rg auth src"
        }
    })
    .to_string();

    let output = run_hook_subprocess(
        &repo_root,
        &paths,
        &["hook", "codex", "pre-tool-use"],
        &input,
    );

    assert!(
        output.status.success(),
        "stateful hook failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let context_request = rx
        .recv_timeout(Duration::from_secs(2))
        .expect("Bash denial should render live context");
    assert!(context_request.contains("POST /v1/context/render HTTP/1.1"));
    assert!(context_request.contains("\"session_id\":\"s1\""));
    let rendered: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("deny outcome should serialize");
    assert_eq!(rendered["hookSpecificOutput"]["permissionDecision"], "deny");
    assert!(
        rendered["hookSpecificOutput"]["permissionDecisionReason"]
            .as_str()
            .expect("deny reason should contain rendered context")
            .contains("Nearby Activity")
    );

    fs::remove_dir_all(&temp_root).expect("temp root should be removable");
}

#[test]
fn pre_tool_use_denies_github_remote_repository_mutation_tools() {
    for tool_name in [
        "mcp__codex_apps__github__create_file",
        "mcp__codex_apps__github__update_file",
        "mcp__codex_apps__github__create_branch",
        "mcp__codex_apps__github__update_ref",
    ] {
        let input = serde_json::json!({
            "session_id": "s1",
            "cwd": "/repo",
            "hook_event_name": "PreToolUse",
            "tool_name": tool_name,
            "tool_input": {}
        })
        .to_string();

        let outcome = handle_pre_tool_use(&input).expect("hook input should parse");

        let HookOutcome::Deny { reason } = outcome else {
            panic!("{tool_name} should be denied");
        };
        assert!(
            reason.contains("remote repository mutation"),
            "reason `{reason}` should classify {tool_name} as remote repository mutation"
        );
    }
}

#[test]
fn pre_tool_use_records_unclassified_tools_for_tools_list() {
    let temp_root = std::env::temp_dir().join(format!(
        "stateful-hook-unclassified-tools-list-test-{}",
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

    let input = serde_json::json!({
        "session_id": "s1",
        "cwd": repo_root,
        "hook_event_name": "PreToolUse",
        "tool_name": "FutureWriteTool",
        "tool_input": {}
    })
    .to_string();
    let output = run_hook_subprocess(
        &repo_root,
        &paths,
        &["hook", "codex", "pre-tool-use"],
        &input,
    );

    assert!(
        output.status.success(),
        "stateful hook failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let json: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("deny outcome should serialize");
    assert_eq!(json["hookSpecificOutput"]["permissionDecision"], "deny");

    let list = tool_list_for_repo(&paths, &repo_root).expect("tool list should load");
    assert_eq!(list.unclassified_tools, vec!["FutureWriteTool"]);

    fs::remove_dir_all(&temp_root).expect("temp root should be removable");
}

#[test]
fn pre_tool_use_allows_repo_tool_allowlist_but_preserves_hard_denies() {
    let temp_root = std::env::temp_dir().join(format!(
        "stateful-hook-tool-allowlist-test-{}",
        std::process::id()
    ));
    if temp_root.exists() {
        fs::remove_dir_all(&temp_root).expect("old temp root should be removable");
    }
    let paths = GlobalPaths::new(temp_root.join("home"));
    let repo_root = temp_root.join("repo");
    fs::create_dir_all(&repo_root).expect("repo root should be creatable");
    enable_test_repo(&paths, &repo_root);
    allow_tool_for_repo(
        &paths,
        &repo_root,
        "mcp__codex_apps__github__merge_pull_request",
    )
    .expect("unclassified tool should be user-allowed");
    allow_tool_for_repo(&paths, &repo_root, "mcp__filesystem__write_file")
        .expect("hard-denied tool name can be stored but must not override hard deny");
    let (runtime, _rx) = spawn_fake_stateful_server(r#"{"status":"ok"}"#);
    write_global_runtime_file(&paths, &runtime).expect("global runtime file should write");

    let allowed_input = serde_json::json!({
        "session_id": "s1",
        "cwd": repo_root,
        "hook_event_name": "PreToolUse",
        "tool_name": "mcp__codex_apps__github__merge_pull_request",
        "tool_input": {}
    })
    .to_string();
    let output = run_hook_subprocess(
        &repo_root,
        &paths,
        &["hook", "codex", "pre-tool-use"],
        &allowed_input,
    );
    assert!(
        output.status.success(),
        "stateful hook failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        output.stdout.is_empty(),
        "allowed hook outcome should not print a denial, got {}",
        String::from_utf8_lossy(&output.stdout)
    );

    let denied_input = serde_json::json!({
        "session_id": "s1",
        "cwd": repo_root,
        "hook_event_name": "PreToolUse",
        "tool_name": "mcp__filesystem__write_file",
        "tool_input": {}
    })
    .to_string();
    let output = run_hook_subprocess(
        &repo_root,
        &paths,
        &["hook", "codex", "pre-tool-use"],
        &denied_input,
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
            .contains("filesystem MCP")
    );

    fs::remove_dir_all(&temp_root).expect("temp root should be removable");
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
fn omp_write_authorize_records_runtime_lineage_without_commit_policy_input() {
    let (runtime, rx) = spawn_fake_stateful_server(r#"{"decision":"allow","message":"ok"}"#);
    let input = serde_json::json!({
        "runtime": "omp",
        "session_id": "omp-parent",
        "parent_session_id": serde_json::Value::Null,
        "omp_agent_id": "main",
        "workspace_id": runtime.workspace_id,
        "cwd": "/repo",
        "yolo": false,
        "commit_id": "abc123",
        "tool_name": "write",
        "tool_input": { "path": "docs/a.md", "content": "hello" }
    })
    .to_string();

    let outcome = handle_omp_pre_tool_use_with_runtime(
        &input,
        Some(&runtime),
        Some(Path::new("/repo")),
        Some(Path::new("/repo")),
    )
    .expect("omp pre-tool should parse");

    assert_eq!(outcome, OmpHookOutcome::Allow);
    let request = rx.recv().expect("authorize request should arrive");
    let body = request_json_body(&request);
    assert_eq!(body["source"]["kind"], "hook");
    assert_eq!(body["source"]["event"], "omp_pre_tool_use");
    assert_eq!(body["session"]["session_id"], "omp-parent");
    assert_eq!(body["payload"]["action"], "write_file");
    assert_eq!(body["payload"]["path"], "docs/a.md");
    assert!(body["payload"].get("commit_id").is_none());
    assert_eq!(body["metadata"]["runtime"], "omp");
    assert_eq!(body["metadata"]["commit_id"], "abc123");
}

#[test]
fn run_hook_omp_pre_tool_use_prints_extension_decision() {
    let temp_root =
        std::env::temp_dir().join(format!("stateful-hook-omp-runtime-{}", std::process::id()));
    if temp_root.exists() {
        fs::remove_dir_all(&temp_root).expect("old temp root should be removable");
    }
    let paths = GlobalPaths::new(temp_root.join("home"));
    let repo_root = temp_root.join("repo");
    fs::create_dir_all(repo_root.join("docs")).expect("repo docs should create");
    enable_test_repo(&paths, &repo_root);
    let (runtime, rx) = spawn_fake_stateful_server(r#"{"decision":"allow","message":"ok"}"#);
    write_global_runtime_file(&paths, &runtime).expect("global runtime file should write");

    let input = serde_json::json!({
        "session_id": "omp-parent",
        "workspace_id": runtime.workspace_id,
        "cwd": repo_root,
        "yolo": false,
        "tool_name": "write",
        "tool_input": { "path": "docs/a.md", "content": "hello" }
    })
    .to_string();

    let output = run_hook_subprocess(&repo_root, &paths, &["hook", "omp", "pre-tool-use"], &input);
    assert!(
        output.status.success(),
        "stateful hook failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("OMP hook should print JSON");
    assert_eq!(stdout["decision"], "allow");
    let body = request_json_body(&rx.recv().expect("authorize request should arrive"));
    assert_eq!(body["session"]["session_id"], "omp-parent");

    fs::remove_dir_all(&temp_root).expect("temp root should be removable");
}

#[test]
fn omp_edit_extracts_hashline_file_targets() {
    let (runtime, rx) = spawn_fake_stateful_server(r#"{"decision":"allow","message":"ok"}"#);
    let input = serde_json::json!({
        "session_id": "omp-parent",
        "workspace_id": runtime.workspace_id,
        "cwd": "/repo",
        "yolo": false,
        "tool_name": "edit",
        "tool_input": { "input": "[docs/a.md#ABCD]\nSWAP 1.=1:\n+new\n" }
    })
    .to_string();

    let outcome = handle_omp_pre_tool_use_with_runtime(
        &input,
        Some(&runtime),
        Some(Path::new("/repo")),
        Some(Path::new("/repo")),
    )
    .expect("omp edit should authorize");

    assert_eq!(outcome, OmpHookOutcome::Allow);
    let body = request_json_body(&rx.recv().expect("authorize request should arrive"));
    assert_eq!(body["payload"]["action"], "write_file");
    assert_eq!(body["payload"]["path"], "docs/a.md");
}

#[test]
fn omp_bash_with_explicit_write_target_authorizes_target() {
    let (runtime, rx) = spawn_fake_stateful_server(r#"{"decision":"allow","message":"ok"}"#);
    let stateful = trusted_stateful_path();
    let input = serde_json::json!({
        "session_id": "omp-parent",
        "workspace_id": runtime.workspace_id,
        "cwd": "/repo",
        "yolo": false,
        "tool_name": "bash",
        "tool_input": {
            "command": format!("{stateful} sandbox run --fs write-targets --write-target docs/a.md --command 'printf ok > docs/a.md'")
        }
    })
    .to_string();

    let outcome = handle_omp_pre_tool_use_with_runtime(
        &input,
        Some(&runtime),
        Some(Path::new("/repo")),
        Some(Path::new("/repo")),
    )
    .expect("omp bash should authorize explicit target");

    assert_eq!(outcome, OmpHookOutcome::Allow);
    let body = request_json_body(&rx.recv().expect("authorize request should arrive"));
    assert_eq!(body["payload"]["action"], "write_file");
    assert_eq!(body["payload"]["path"], "docs/a.md");
}

#[test]
fn omp_bash_with_explicit_write_dir_authorizes_directory_target() {
    let (runtime, rx) = spawn_fake_stateful_server(r#"{"decision":"allow","message":"ok"}"#);
    let stateful = trusted_stateful_path();
    let input = serde_json::json!({
        "session_id": "omp-parent",
        "workspace_id": runtime.workspace_id,
        "cwd": "/repo",
        "yolo": false,
        "tool_name": "bash",
        "tool_input": {
            "command": format!("{stateful} sandbox run --fs write-targets --write-dir reports --command 'python gen.py'")
        }
    })
    .to_string();

    let outcome = handle_omp_pre_tool_use_with_runtime(
        &input,
        Some(&runtime),
        Some(Path::new("/repo")),
        Some(Path::new("/repo")),
    )
    .expect("omp bash should authorize explicit directory target");

    assert_eq!(outcome, OmpHookOutcome::Allow);
    let body = request_json_body(&rx.recv().expect("authorize request should arrive"));
    assert_eq!(body["payload"]["action"], "write_directory");
    assert_eq!(body["payload"]["path"], "reports");
}

#[test]
fn omp_repo_internal_raw_bash_rejects_shell_writes_and_unsafe_find_actions() {
    for command in ["ls > docs/a.md", "find . -delete"] {
        let input = serde_json::json!({
            "session_id": "omp-parent",
            "cwd": "/repo",
            "yolo": false,
            "tool_name": "bash",
            "tool_input": { "command": command }
        })
        .to_string();
        assert!(matches!(
            handle_omp_pre_tool_use_with_runtime(
                &input,
                None,
                Some(Path::new("/repo")),
                Some(Path::new("/repo"))
            )
            .unwrap(),
            OmpHookOutcome::Block { .. }
        ));
    }
}

#[test]
fn omp_repo_internal_bash_requires_sandbox_run_for_reads_and_write_targets() {
    let raw_read_input = serde_json::json!({
        "session_id": "omp-parent",
        "cwd": "/repo",
        "yolo": false,
        "tool_name": "bash",
        "tool_input": { "command": "pwd" }
    })
    .to_string();
    assert!(matches!(
        handle_omp_pre_tool_use_with_runtime(
            &raw_read_input,
            None,
            Some(Path::new("/repo")),
            Some(Path::new("/repo"))
        )
        .unwrap(),
        OmpHookOutcome::Block { .. }
    ));

    let stateful = trusted_stateful_path();
    let sandboxed_read_input = serde_json::json!({
        "session_id": "omp-parent",
        "cwd": "/repo",
        "yolo": false,
        "tool_name": "bash",
        "tool_input": {
            "command": format!("{stateful} sandbox run --fs read-only --network disabled --command 'pwd'")
        }
    })
    .to_string();
    assert_eq!(
        handle_omp_pre_tool_use_with_runtime(
            &sandboxed_read_input,
            None,
            Some(Path::new("/repo")),
            Some(Path::new("/repo"))
        )
        .unwrap(),
        OmpHookOutcome::Allow
    );

    let write_like_input = serde_json::json!({
        "session_id": "omp-parent",
        "cwd": "/repo",
        "yolo": false,
        "tool_name": "bash",
        "tool_input": { "command": "python scripts/gen.py" }
    })
    .to_string();
    assert!(matches!(
        handle_omp_pre_tool_use_with_runtime(
            &write_like_input,
            None,
            Some(Path::new("/repo")),
            Some(Path::new("/repo"))
        )
        .unwrap(),
        OmpHookOutcome::Block { .. }
    ));
}

#[test]
fn omp_allows_classified_read_only_and_stateful_activation_tools() {
    for tool_name in [
        "read",
        "find",
        "grep",
        "search",
        "web_search",
        "browser",
        "search_tool_bm25",
        "mcp__stateful_state_current_read",
        "mcp__stateful_state_intent_declare",
        "state_current_read",
        "state_intent_declare",
    ] {
        let input = serde_json::json!({
            "session_id": "omp-parent",
            "cwd": "/repo",
            "yolo": false,
            "tool_name": tool_name,
            "tool_input": { "path": "README.md" }
        })
        .to_string();

        assert_eq!(
            handle_omp_pre_tool_use_with_runtime(
                &input,
                None,
                Some(Path::new("/repo")),
                Some(Path::new("/repo"))
            )
            .unwrap(),
            OmpHookOutcome::Allow
        );
    }
}

#[test]
fn omp_repo_external_raw_bash_warns_for_omp_approval_handoff() {
    let input = serde_json::json!({
        "session_id": "omp-parent",
        "cwd": "/tmp/outside",
        "yolo": false,
        "tool_name": "bash",
        "tool_input": { "command": "python /tmp/tool.py" }
    })
    .to_string();

    let outcome = handle_omp_pre_tool_use_with_runtime(
        &input,
        None,
        Some(Path::new("/repo")),
        Some(Path::new("/tmp/outside")),
    )
    .unwrap();
    let OmpHookOutcome::WarnAllow { reason } = outcome else {
        panic!("repo-external targetless bash should warn and hand off to OMP approval");
    };
    assert!(reason.contains("approval handoff"));
}

#[test]
fn omp_yolo_does_not_downgrade_server_denial() {
    let (runtime, _rx) =
        spawn_fake_stateful_server(r#"{"decision":"deny","message":"missing lease"}"#);
    let input = serde_json::json!({
        "session_id": "omp-parent",
        "workspace_id": runtime.workspace_id,
        "cwd": "/repo",
        "yolo": true,
        "tool_name": "write",
        "tool_input": { "path": "docs/a.md", "content": "hello" }
    })
    .to_string();

    assert!(matches!(
        handle_omp_pre_tool_use_with_runtime(
            &input,
            Some(&runtime),
            Some(Path::new("/repo")),
            Some(Path::new("/repo"))
        )
        .unwrap(),
        OmpHookOutcome::Block { .. }
    ));
}

#[test]
fn omp_session_start_posts_parent_session_register() {
    let (runtime, rx) = spawn_fake_stateful_server(r#"{"status":"ok"}"#);
    let input = serde_json::json!({
        "session_id": "omp-parent",
        "workspace_id": runtime.workspace_id,
        "cwd": "/repo",
        "omp_agent_id": "main",
        "commit_id": "abc123"
    })
    .to_string();

    handle_omp_session_start_with_runtime(&input, &runtime)
        .expect("omp session start should post register");

    let body = request_json_body(&rx.recv().expect("session register request should arrive"));
    assert_eq!(body["session_id"], "omp-parent");
    assert_eq!(body["workspace_id"], runtime.workspace_id);
    assert_eq!(body["metadata"]["runtime"], "omp");
    assert_eq!(body["metadata"]["omp_agent_id"], "main");
    assert_eq!(body["metadata"]["commit_id"], "abc123");
}

#[test]
fn omp_subagent_post_tool_uses_child_session_and_parent_metadata() {
    let (runtime, rx) = spawn_fake_stateful_server(r#"{"status":"ok"}"#);
    let input = serde_json::json!({
        "session_id": "omp-child",
        "parent_session_id": "omp-parent",
        "omp_agent_id": "WorkerA",
        "workspace_id": runtime.workspace_id,
        "cwd": "/repo",
        "tool_name": "write",
        "tool_input": { "path": "docs/a.md", "content": "hello" }
    })
    .to_string();

    handle_omp_post_tool_use_with_runtime(&input, &runtime)
        .expect("omp post tool should post heartbeat");

    let body = request_json_body(&rx.recv().expect("heartbeat request should arrive"));
    assert_eq!(body["session_id"], "omp-child");
    assert_eq!(body["workspace_id"], runtime.workspace_id);
    assert_eq!(body["metadata"]["parent_session_id"], "omp-parent");
    assert_eq!(body["metadata"]["omp_agent_id"], "WorkerA");
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
    fs::create_dir_all(repo_root.join("src")).expect("repo src should be creatable");
    fs::write(repo_root.join("src/auth.ts"), b"old contents\n")
        .expect("observed file should be writable");
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

    let output = run_hook_subprocess(
        &repo_root,
        &paths,
        &["hook", "codex", "pre-tool-use"],
        &input,
    );

    assert!(
        output.status.success(),
        "stateful hook failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let request = rx.recv().expect("captured request should arrive");
    assert!(request.contains("POST /v1/authorize HTTP/1.1"));
    assert!(request.contains("\"action\":\"write_file\""));
    assert!(request.contains("\"path\":\"src/auth.ts\""));
    let body = request_json_body(&request);
    let observations = body["payload"]["base_observations"]
        .as_array()
        .expect("base_observations should be present");
    assert_eq!(observations.len(), 1);
    assert_eq!(observations[0]["path"], "src/auth.ts");
    assert_eq!(observations[0]["exists"], true);
    assert_eq!(
        observations[0]["content_hash"],
        test_content_hash(b"old contents\n")
    );
    let json: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("deny outcome should serialize");
    assert_eq!(json["hookSpecificOutput"]["permissionDecision"], "deny");

    fs::remove_dir_all(&temp_root).expect("temp root should be removable");
}

#[test]
fn pre_tool_use_edit_denies_when_authorize_connection_drops() {
    let temp_root = std::env::temp_dir().join(format!(
        "stateful-hook-edit-authorize-drop-test-{}",
        std::process::id()
    ));
    if temp_root.exists() {
        fs::remove_dir_all(&temp_root).expect("old temp root should be removable");
    }
    let paths = GlobalPaths::new(temp_root.join("home"));
    let repo_root = temp_root.join("repo");
    fs::create_dir_all(&repo_root).expect("repo root should be creatable");
    enable_test_repo(&paths, &repo_root);
    let (runtime, rx) = spawn_fake_stateful_server_dropping_authorize();
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

    let output = run_hook_subprocess(
        &repo_root,
        &paths,
        &["hook", "codex", "pre-tool-use"],
        &input,
    );

    assert!(
        output.status.success(),
        "stateful hook should return a structured denial, not crash: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let request = rx.recv().expect("captured request should arrive");
    assert!(request.contains("POST /v1/authorize HTTP/1.1"));
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("\"permissionDecision\":\"deny\""));
    assert!(stdout.contains("server_unavailable"));
    assert!(stdout.contains("Writes fail closed"));

    fs::remove_dir_all(&temp_root).expect("temp root should be removable");
}

#[test]
fn pre_tool_use_edit_posts_authorize_and_renders_live_context_when_server_allows() {
    let temp_root = std::env::temp_dir().join(format!(
        "stateful-hook-edit-allow-context-test-{}",
        std::process::id()
    ));
    if temp_root.exists() {
        fs::remove_dir_all(&temp_root).expect("old temp root should be removable");
    }
    let paths = GlobalPaths::new(temp_root.join("home"));
    let repo_root = temp_root.join("repo");
    fs::create_dir_all(repo_root.join("src")).expect("repo src should be creatable");
    fs::write(repo_root.join("src/auth.ts"), b"old contents\n")
        .expect("observed file should be writable");
    enable_test_repo(&paths, &repo_root);
    let (runtime, rx) = spawn_fake_stateful_server_sequence(vec![
        r#"{"decision":"allow","reason_code":"authorized","message":"ok","required_next_action":null}"#,
        r#"{"status":"ok","items":[{"severity":"info"}],"prompt_text":"Nearby Activity\n- [info] src/auth.ts: Session s2 declared intent for src/auth.ts."}"#,
    ]);
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

    let output = run_hook_subprocess(
        &repo_root,
        &paths,
        &["hook", "codex", "pre-tool-use"],
        &input,
    );

    assert!(
        output.status.success(),
        "stateful hook failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let request = rx.recv().expect("captured request should arrive");
    assert!(request.contains("POST /v1/authorize HTTP/1.1"));
    assert!(request.contains("\"action\":\"write_file\""));
    assert!(request.contains("\"path\":\"src/auth.ts\""));
    let context_request = rx
        .recv_timeout(Duration::from_secs(2))
        .expect("Edit should render live context after authorization");
    assert!(context_request.contains("POST /v1/context/render HTTP/1.1"));
    assert!(context_request.contains("\"session_id\":\"s1\""));
    assert!(context_request.contains("\"mode\":\"brief\""));
    assert!(
        output.stdout.is_empty(),
        "info-only context should not be injected for allowed Edit writes: {}",
        String::from_utf8_lossy(&output.stdout)
    );

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

    let output = run_hook_subprocess_from(
        &temp_root,
        &paths,
        &["hook", "codex", "pre-tool-use"],
        &input,
    );

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

    let output =
        run_hook_subprocess_from(&outside, &paths, &["hook", "codex", "pre-tool-use"], &input);

    assert!(
        output.status.success(),
        "stateful hook failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let request = rx.recv().expect("captured request should arrive");
    assert!(request.contains("POST /v1/authorize HTTP/1.1"));
    let session = read_legacy_current_session_file(&repo_root);
    assert_eq!(session.session_id, "s-cwd");
    assert!(!outside.join(".stateful_core/runtime/session.json").exists());

    fs::remove_dir_all(&temp_root).expect("temp root should be removable");
}

#[test]
fn pre_tool_use_apply_patch_omits_queue_without_matching_current_intent_purpose() {
    let temp_root = std::env::temp_dir().join(format!(
        "stateful-hook-no-purpose-test-{}",
        std::process::id()
    ));
    if temp_root.exists() {
        fs::remove_dir_all(&temp_root).expect("old temp root should be removable");
    }
    let paths = GlobalPaths::new(temp_root.join("home"));
    let repo_root = temp_root.join("repo");
    fs::create_dir_all(&repo_root).expect("repo root should be creatable");
    enable_test_repo(&paths, &repo_root);
    let (runtime, rx) = spawn_fake_stateful_server_sequence_with_current(
        vec![
            r#"{"decision":"allow","reason_code":"authorized","message":"ok","required_next_action":null}"#,
        ],
        Some(r#"{"status":"ok","current":{},"items":[]}"#),
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

    let output = run_hook_subprocess(
        &repo_root,
        &paths,
        &["hook", "codex", "pre-tool-use"],
        input,
    );

    assert!(
        output.status.success(),
        "stateful hook failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let request = rx.recv().expect("captured request should arrive");
    let body = request_json_body(&request);
    assert_eq!(body["payload"]["action"], "write_file");
    assert_eq!(body["payload"]["path"], "src/auth.ts");
    assert!(body["payload"].get("purpose").is_none());
    assert!(body["payload"].get("queue_on_conflict").is_none());

    fs::remove_dir_all(&temp_root).expect("temp root should be removable");
}

#[test]
fn pre_tool_use_apply_patch_uses_current_intent_purpose_for_queue_even_when_target_differs() {
    let temp_root = std::env::temp_dir().join(format!(
        "stateful-hook-current-purpose-any-target-test-{}",
        std::process::id()
    ));
    if temp_root.exists() {
        fs::remove_dir_all(&temp_root).expect("old temp root should be removable");
    }
    let paths = GlobalPaths::new(temp_root.join("home"));
    let repo_root = temp_root.join("repo");
    fs::create_dir_all(&repo_root).expect("repo root should be creatable");
    enable_test_repo(&paths, &repo_root);
    let (runtime, rx) = spawn_fake_stateful_server_sequence_with_current(
        vec![
            r#"{"decision":"allow","reason_code":"authorized","message":"ok","required_next_action":null}"#,
        ],
        Some(
            r#"{"status":"ok","current":{},"items":[{
                "kind":"intent",
                "freshness":"live",
                "resource":"docs/notes.md",
                "purpose":"Continue documented retry work.",
                "session_id":"s1",
                "workspace_id":"w1"
            }]}"#,
        ),
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

    let output = run_hook_subprocess(
        &repo_root,
        &paths,
        &["hook", "codex", "pre-tool-use"],
        input,
    );

    assert!(
        output.status.success(),
        "stateful hook failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let request = rx.recv().expect("captured request should arrive");
    let body = request_json_body(&request);
    assert_eq!(body["payload"]["action"], "write_file");
    assert_eq!(body["payload"]["path"], "src/auth.ts");
    assert_eq!(body["payload"]["queue_on_conflict"], true);
    assert_eq!(
        body["payload"]["purpose"],
        "Continue documented retry work."
    );

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
    let (runtime, rx) = spawn_fake_stateful_server_sequence(vec![
        r#"{"decision":"allow","reason_code":"authorized","message":"ok","required_next_action":null}"#,
        r#"{"status":"ok","prompt_text":"Nearby Activity\n- [info] src/auth.ts: Session s2 declared intent for src/auth.ts."}"#,
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

    let output = run_hook_subprocess(
        &repo_root,
        &paths,
        &["hook", "codex", "pre-tool-use"],
        input,
    );

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
    assert!(
        body["workspace"]["repo_id"]
            .as_str()
            .expect("repo_id should be a string")
            .starts_with("repo-")
    );
    assert_eq!(
        body["workspace"]["worktree_id"],
        body["workspace"]["repo_id"]
    );
    assert_eq!(
        body["workspace"]["root"],
        repo_root
            .canonicalize()
            .expect("repo root should canonicalize")
            .to_string_lossy()
            .as_ref()
    );
    assert!(
        !body["workspace"]["branch"]
            .as_str()
            .expect("branch should be a string")
            .is_empty()
    );
    assert_eq!(body["source"]["kind"], "hook");
    assert_eq!(body["source"]["event"], "pre_tool_use");
    assert_eq!(body["payload"]["action"], "write_file");
    assert_eq!(body["payload"]["path"], "src/auth.ts");
    assert_eq!(body["payload"]["queue_on_conflict"], true);
    assert_eq!(body["payload"]["purpose"], "Fix auth validation behavior.");
    assert!(body.get("action").is_none());
    let context_request = rx
        .recv_timeout(Duration::from_secs(2))
        .expect("apply_patch should render live context after authorization");
    assert!(context_request.contains("POST /v1/context/render HTTP/1.1"));
    assert!(context_request.contains("\"session_id\":\"s1\""));
    assert!(context_request.contains("\"mode\":\"brief\""));
    assert!(
        output.stdout.is_empty(),
        "info-only context should not be injected for allowed writes: {}",
        String::from_utf8_lossy(&output.stdout)
    );

    fs::remove_dir_all(&temp_root).expect("temp root should be removable");
}

#[test]
fn pre_tool_use_apply_patch_injects_warn_context_when_server_allows() {
    let temp_root = std::env::temp_dir().join(format!(
        "stateful-hook-warn-context-test-{}",
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
        r#"{"status":"ok","items":[{"severity":"warn"}],"prompt_text":"Warnings\n- [warn] src/auth.ts: related active work."}"#,
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

    let output = run_hook_subprocess(
        &repo_root,
        &paths,
        &["hook", "codex", "pre-tool-use"],
        input,
    );

    assert!(
        output.status.success(),
        "stateful hook failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let _authorize_request = rx.recv().expect("authorize request should arrive");
    let context_request = rx
        .recv_timeout(Duration::from_secs(2))
        .expect("apply_patch should render live context after authorization");
    assert!(context_request.contains("POST /v1/context/render HTTP/1.1"));
    let rendered: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("allow outcome should serialize");
    assert!(
        rendered["hookSpecificOutput"]["additionalContext"]
            .as_str()
            .expect("additional context should be present")
            .contains("Warnings")
    );

    fs::remove_dir_all(&temp_root).expect("temp root should be removable");
}

#[test]
fn pre_tool_use_apply_patch_injects_block_context_when_server_allows() {
    let temp_root = std::env::temp_dir().join(format!(
        "stateful-hook-block-context-test-{}",
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
        r#"{"status":"ok","items":[{"severity":"block"}],"prompt_text":"Blocking\n- [block] src/auth.ts: another session has a lease."}"#,
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

    let output = run_hook_subprocess(
        &repo_root,
        &paths,
        &["hook", "codex", "pre-tool-use"],
        input,
    );

    assert!(
        output.status.success(),
        "stateful hook failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let _authorize_request = rx.recv().expect("authorize request should arrive");
    let context_request = rx
        .recv_timeout(Duration::from_secs(2))
        .expect("apply_patch should render live context after authorization");
    assert!(context_request.contains("POST /v1/context/render HTTP/1.1"));
    let rendered: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("allow outcome should serialize");
    assert!(
        rendered["hookSpecificOutput"]["additionalContext"]
            .as_str()
            .expect("additional context should be present")
            .contains("Blocking")
    );

    fs::remove_dir_all(&temp_root).expect("temp root should be removable");
}

#[test]
fn pre_tool_use_apply_patch_denial_keeps_info_context() {
    let temp_root = std::env::temp_dir().join(format!(
        "stateful-hook-deny-info-context-test-{}",
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
        r#"{"decision":"deny","reason_code":"scope_mismatch","message":"Write target is outside active intent scope.","required_next_action":"Declare matching intent."}"#,
        r#"{"status":"ok","items":[{"severity":"info"}],"prompt_text":"Nearby Activity\n- [info] src/session.ts: another session declared intent."}"#,
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

    let output = run_hook_subprocess(
        &repo_root,
        &paths,
        &["hook", "codex", "pre-tool-use"],
        input,
    );

    assert!(
        output.status.success(),
        "stateful hook failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let _authorize_request = rx.recv().expect("authorize request should arrive");
    let context_request = rx
        .recv_timeout(Duration::from_secs(2))
        .expect("apply_patch should render live context after denial");
    assert!(context_request.contains("POST /v1/context/render HTTP/1.1"));
    let json: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("deny outcome should serialize");
    assert_eq!(json["hookSpecificOutput"]["permissionDecision"], "deny");
    assert!(
        json["hookSpecificOutput"]["permissionDecisionReason"]
            .as_str()
            .expect("deny reason should be text")
            .contains("Nearby Activity")
    );

    fs::remove_dir_all(&temp_root).expect("temp root should be removable");
}

#[test]
fn pre_tool_use_apply_patch_sends_base_observation_for_existing_file() {
    let temp_root = std::env::temp_dir().join(format!(
        "stateful-hook-base-observation-test-{}",
        std::process::id()
    ));
    if temp_root.exists() {
        fs::remove_dir_all(&temp_root).expect("old temp root should be removable");
    }
    let paths = GlobalPaths::new(temp_root.join("home"));
    let repo_root = temp_root.join("repo");
    fs::create_dir_all(repo_root.join("src")).expect("repo src should be creatable");
    fs::write(repo_root.join("src/auth.ts"), b"original contents\n")
        .expect("observed file should be writable");
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
        "tool_input": {
            "command": "*** Begin Patch\n*** Update File: src/auth.ts\n@@\n-original contents\n+updated contents\n*** End Patch\n"
        }
    })
    .to_string();

    let output = run_hook_subprocess(
        &repo_root,
        &paths,
        &["hook", "codex", "pre-tool-use"],
        &input,
    );

    assert!(
        output.status.success(),
        "stateful hook failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let request = rx.recv().expect("captured request should arrive");
    let body = request_json_body(&request);
    let observations = body["payload"]["base_observations"]
        .as_array()
        .expect("base_observations should be present");
    assert_eq!(observations.len(), 1);
    assert_eq!(observations[0]["path"], "src/auth.ts");
    assert_eq!(observations[0]["exists"], true);
    assert_eq!(
        observations[0]["content_hash"],
        test_content_hash(b"original contents\n")
    );

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

    let output = run_hook_subprocess(
        &repo_root,
        &paths,
        &["hook", "codex", "pre-tool-use"],
        &input,
    );

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
fn pre_tool_use_apply_patch_move_authorizes_source_and_destination() {
    let temp_root = std::env::temp_dir().join(format!(
        "stateful-hook-move-patch-test-{}",
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
        r#"{"decision":"deny","reason_code":"scope_mismatch","message":"Move target is outside exact intent scope.","required_next_action":"Declare exact source and destination intent and acquire both leases."}"#,
    );
    write_global_runtime_file(&paths, &runtime).expect("global runtime file should write");

    let input = serde_json::json!({
        "session_id": "s1",
        "cwd": repo_root,
        "hook_event_name": "PreToolUse",
        "tool_name": "apply_patch",
        "tool_input": {
            "patch": "*** Begin Patch\n*** Update File: old.txt\n*** Move to: new.txt\n@@\n-old\n+new\n*** End Patch\n"
        }
    })
    .to_string();

    let output = run_hook_subprocess(
        &repo_root,
        &paths,
        &["hook", "codex", "pre-tool-use"],
        &input,
    );

    assert!(
        output.status.success(),
        "stateful hook failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let request = rx
        .recv_timeout(Duration::from_secs(2))
        .expect("move authorize request should arrive");
    assert!(request.contains("\"action\":\"move_file\""));
    assert!(request.contains("\"path\":\"old.txt\""));
    assert!(request.contains("\"old_path\":\"old.txt\""));
    assert!(request.contains("\"new_path\":\"new.txt\""));
    let body = request_json_body(&request);
    assert_eq!(body["payload"]["purpose"], "Fix auth validation behavior.");
    let json: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("deny outcome should serialize");
    assert_eq!(json["hookSpecificOutput"]["permissionDecision"], "deny");
    assert_eq!(
        json["hookSpecificOutput"]["permissionDecisionReason"],
        "Declare exact source and destination intent and acquire both leases."
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

    let output = run_hook_subprocess(
        &repo_root,
        &paths,
        &["hook", "codex", "pre-tool-use"],
        &input,
    );

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
    assert!(request.contains("\"tool_name\":\"apply_patch\""));

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

    let output = run_hook_subprocess(
        &repo_root,
        &paths,
        &["hook", "codex", "pre-tool-use"],
        &input,
    );

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
        .args(["hook", "codex", "pre-tool-use"])
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

    let output = run_hook_subprocess(
        &repo_root,
        &paths,
        &["hook", "codex", "pre-tool-use"],
        input,
    );

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
fn pre_tool_use_denies_new_dependency_shadowing_python_root_before_authorize() {
    let temp_root = std::env::temp_dir().join(format!(
        "stateful-hook-shadow-dependency-test-{}",
        std::process::id()
    ));
    if temp_root.exists() {
        fs::remove_dir_all(&temp_root).expect("old temp root should be removable");
    }
    let paths = GlobalPaths::new(temp_root.join("home"));
    let repo_root = temp_root.join("repo");
    fs::create_dir_all(&repo_root).expect("repo root should be creatable");
    fs::write(
        repo_root.join("pyproject.toml"),
        r#"
[project]
name = "example-project"
dependencies = ["langchain-core>=0.3"]
"#,
    )
    .expect("pyproject should write");
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
        "tool_input": {
            "command": "*** Begin Patch\n*** Add File: langchain_core/__init__.py\n+\"\"\"local shim\"\"\"\n*** End Patch\n"
        }
    })
    .to_string();

    let output = run_hook_subprocess(
        &repo_root,
        &paths,
        &["hook", "codex", "pre-tool-use"],
        &input,
    );

    assert!(
        output.status.success(),
        "stateful hook should return a structured denial, not crash: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("\"permissionDecision\":\"deny\""));
    assert!(stdout.contains("dependency shadowing guard"));
    assert!(stdout.contains("langchain_core"));
    assert!(stdout.contains("langchain-core"));
    let context_request = rx
        .recv_timeout(Duration::from_secs(2))
        .expect("apply_patch should render live context even when locally denied");
    assert!(context_request.contains("POST /v1/context/render HTTP/1.1"));
    assert!(
        !context_request.contains("POST /v1/authorize HTTP/1.1"),
        "shadowing guard should deny before posting /v1/authorize"
    );
    assert!(
        rx.recv_timeout(Duration::from_millis(200)).is_err(),
        "shadowing guard should not post /v1/authorize after live context render"
    );

    fs::remove_dir_all(&temp_root).expect("temp root should be removable");
}

#[test]
fn pre_tool_use_apply_patch_denial_includes_wait_id_guidance() {
    let temp_root = std::env::temp_dir().join(format!(
        "stateful-hook-deny-wait-id-test-{}",
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
        r#"{"decision":"deny","reason_code":"active_lease_conflict","message":"Write target is covered by another active session lease.","required_next_action":"Wait for the active lease to release, then claim the reservation before writing.","wait":{"wait_id":"wait-123","session_id":"s1","workspace_id":"w1","path":"src/auth.ts","action":"write_file","status":"queued","queue_position":2,"blocking_session_id":"s2"}}"#,
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

    let output = run_hook_subprocess(
        &repo_root,
        &paths,
        &["hook", "codex", "pre-tool-use"],
        input,
    );

    assert!(
        output.status.success(),
        "stateful hook failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let _request = rx.recv().expect("captured request should arrive");
    let json: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("deny outcome should serialize");
    let reason = json["hookSpecificOutput"]["permissionDecisionReason"]
        .as_str()
        .expect("denial reason should be text");
    assert!(reason.contains("wait-123"));
    assert!(reason.contains("queue position 2"));
    assert!(reason.contains("blocked by session s2"));
    assert!(reason.contains("state_notifications_poll"));
    assert!(reason.contains("state_resume_next"));
    assert!(reason.contains("reread"));
    assert!(reason.contains("state_intent_claim"));

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

    let output = run_hook_subprocess(
        &repo_root,
        &paths,
        &["hook", "codex", "pre-tool-use"],
        input,
    );

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

    let output = run_hook_subprocess(
        &repo_root,
        &paths,
        &["hook", "codex", "session-start"],
        input,
    );
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

    let output = run_hook_subprocess(
        &repo_root,
        &paths,
        &["hook", "codex", "post-tool-use"],
        input,
    );
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
fn post_tool_use_edit_refreshes_file_lease_observation() {
    let temp_root = std::env::temp_dir().join(format!(
        "stateful-hook-post-refresh-test-{}",
        std::process::id()
    ));
    if temp_root.exists() {
        fs::remove_dir_all(&temp_root).expect("old temp root should be removable");
    }
    let paths = GlobalPaths::new(temp_root.join("home"));
    let repo_root = temp_root.join("repo");
    fs::create_dir_all(repo_root.join("src")).expect("repo src should be creatable");
    fs::write(repo_root.join("src/auth.ts"), b"updated contents\n")
        .expect("observed file should be writable");
    enable_test_repo(&paths, &repo_root);
    let (runtime, rx) =
        spawn_fake_stateful_server_sequence(vec![r#"{"status":"ok"}"#, r#"{"status":"ok"}"#]);
    write_global_runtime_file(&paths, &runtime).expect("global runtime file should write");

    let input = serde_json::json!({
        "session_id": "s1",
        "cwd": repo_root,
        "hook_event_name": "PostToolUse",
        "tool_name": "Edit",
        "tool_input": {
            "file_path": "src/auth.ts",
            "old_string": "old",
            "new_string": "updated"
        }
    })
    .to_string();

    let output = run_hook_subprocess(
        &repo_root,
        &paths,
        &["hook", "codex", "post-tool-use"],
        &input,
    );
    assert!(
        output.status.success(),
        "stateful hook failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let heartbeat = rx.recv().expect("heartbeat request should arrive");
    assert!(heartbeat.contains("POST /v1/session/heartbeat HTTP/1.1"));

    let refresh = rx
        .recv_timeout(Duration::from_secs(2))
        .expect("lease observation refresh request should arrive");
    assert!(refresh.contains("POST /v1/lease/refresh-observation HTTP/1.1"));
    let body = request_json_body(&refresh);
    assert_eq!(body["session_id"], "s1");
    assert_eq!(body["workspace_id"], "w1");
    assert_eq!(body["path"], "src/auth.ts");
    assert_eq!(
        body["root"],
        fs::canonicalize(&repo_root)
            .expect("repo root should canonicalize")
            .to_string_lossy()
            .to_string()
    );

    fs::remove_dir_all(&temp_root).expect("temp root should be removable");
}

#[test]
fn post_tool_use_edit_releases_file_lease_after_refresh() {
    let temp_root = std::env::temp_dir().join(format!(
        "stateful-hook-post-release-test-{}",
        std::process::id()
    ));
    if temp_root.exists() {
        fs::remove_dir_all(&temp_root).expect("old temp root should be removable");
    }
    let paths = GlobalPaths::new(temp_root.join("home"));
    let repo_root = temp_root.join("repo");
    fs::create_dir_all(repo_root.join("src")).expect("repo src should be creatable");
    fs::write(repo_root.join("src/auth.ts"), b"updated contents\n")
        .expect("observed file should be writable");
    enable_test_repo(&paths, &repo_root);
    let (runtime, rx) = spawn_fake_stateful_server_sequence(vec![
        r#"{"status":"ok"}"#,
        r#"{"status":"ok"}"#,
        r#"{"status":"ok"}"#,
    ]);
    write_global_runtime_file(&paths, &runtime).expect("global runtime file should write");

    let input = serde_json::json!({
        "session_id": "s1",
        "cwd": repo_root,
        "hook_event_name": "PostToolUse",
        "tool_name": "Edit",
        "tool_input": {
            "file_path": "src/auth.ts",
            "old_string": "old",
            "new_string": "updated"
        }
    })
    .to_string();

    let output = run_hook_subprocess(
        &repo_root,
        &paths,
        &["hook", "codex", "post-tool-use"],
        &input,
    );
    assert!(
        output.status.success(),
        "stateful hook failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let heartbeat = rx.recv().expect("heartbeat request should arrive");
    assert!(heartbeat.contains("POST /v1/session/heartbeat HTTP/1.1"));

    let refresh = rx
        .recv_timeout(Duration::from_secs(2))
        .expect("lease observation refresh request should arrive");
    assert!(refresh.contains("POST /v1/lease/refresh-observation HTTP/1.1"));

    let release = rx
        .recv_timeout(Duration::from_secs(2))
        .expect("lease release request should arrive after refresh");
    assert!(release.contains("POST /v1/lease/release HTTP/1.1"));
    let body = request_json_body(&release);
    assert_eq!(body["session_id"], "s1");
    assert_eq!(body["workspace_id"], "w1");
    assert_eq!(body["path"], "src/auth.ts");

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
fn post_tool_use_outbox_fallback_records_current_created_at() {
    let temp_root = std::env::temp_dir().join(format!(
        "stateful-hook-outbox-created-at-test-{}",
        std::process::id()
    ));
    if temp_root.exists() {
        fs::remove_dir_all(&temp_root).expect("old temp root should be removable");
    }
    let paths = GlobalPaths::new(temp_root.join("home"));
    let repo_root = temp_root.join("repo");
    fs::create_dir_all(&repo_root).expect("repo root should be creatable");
    enable_test_repo(&paths, &repo_root);

    let listener = TcpListener::bind("127.0.0.1:0").expect("listener should bind");
    let addr = listener.local_addr().expect("listener addr should load");
    thread::spawn(move || {
        for _ in 0..8 {
            let (mut stream, _) = listener.accept().expect("connection should arrive");
            let request = read_http_request_maybe_body(&mut stream);
            if request.contains("GET /health HTTP/1.1") {
                write_json_response(&mut stream, r#"{"status":"ok"}"#);
            } else if request.contains("GET /v1/runtime/identity HTTP/1.1") {
                write_json_response(
                    &mut stream,
                    r#"{"status":"ok","pid":42,"protocol_version":"stateful.v1","capabilities":["authorize.write_directory"]}"#,
                );
            } else {
                stream
                    .write_all(
                        b"HTTP/1.1 500 Internal Server Error\r\nContent-Length: 18\r\n\r\n{\"status\":\"error\"}",
                    )
                    .expect("error response should write");
            }
        }
    });

    let runtime = ServerRuntime::new(format!("http://{addr}"), "secret-token", "w1", 42);
    write_global_runtime_file(&paths, &runtime).expect("global runtime file should write");

    let input = r#"{
      "session_id": "s1",
      "hook_event_name": "PostToolUse",
      "tool_name": "Bash",
      "tool_input": {"command": "rg auth src"}
    }"#;

    let before_hook = OffsetDateTime::now_utc();
    let output = run_hook_subprocess_with_extra_env(
        &repo_root,
        &paths,
        &["hook", "codex", "post-tool-use"],
        input,
        &[
            ("STATEFUL_SERVER_URL", runtime.base_url.as_str()),
            ("STATEFUL_SERVER_TOKEN", runtime.token.as_str()),
        ],
    );
    let after_hook = OffsetDateTime::now_utc();

    assert!(
        output.status.success(),
        "stateful hook failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let outbox_file = repo_root.join(".stateful_core/outbox/s1.jsonl");
    let outbox = fs::read_to_string(&outbox_file).expect("outbox fallback should write");
    let record: serde_json::Value =
        serde_json::from_str(outbox.trim()).expect("outbox record should be json");
    assert_eq!(record["event_type"], "SessionHeartbeatQueued");
    let created_at = record["created_at"]
        .as_str()
        .expect("created_at should be a string");
    assert_ne!(
        created_at, "2026-05-31T00:00:00Z",
        "created_at should describe the queued record, not a fixture constant"
    );
    let created_at = OffsetDateTime::parse(created_at, &Rfc3339)
        .expect("created_at should be an RFC3339 timestamp");
    assert!(created_at >= before_hook);
    assert!(created_at <= after_hook);

    fs::remove_dir_all(&temp_root).expect("temp root should be removable");
}

#[test]
fn codex_stateful_lifecycle_posts_expected_server_requests() {
    let temp_root = std::env::temp_dir().join(format!(
        "stateful-hook-codex-lifecycle-test-{}",
        std::process::id()
    ));
    if temp_root.exists() {
        fs::remove_dir_all(&temp_root).expect("old temp root should be removable");
    }
    let paths = GlobalPaths::new(temp_root.join("home"));
    let repo_root = temp_root.join("repo");
    fs::create_dir_all(repo_root.join("src")).expect("repo src should be creatable");
    fs::write(repo_root.join("src/auth.ts"), b"old contents\n")
        .expect("observed file should be writable");
    enable_test_repo(&paths, &repo_root);
    let (runtime, rx) = spawn_fake_stateful_server_sequence(vec![
        r#"{"status":"ok"}"#,
        r#"{"status":"ok","prompt_text":"Lifecycle Prompt\n- none"}"#,
        r#"{"decision":"allow","reason_code":"authorized","message":"ok","required_next_action":null}"#,
        r#"{"status":"ok","items":[],"prompt_text":"Lifecycle Pre\n- none"}"#,
        r#"{"status":"ok"}"#,
        r#"{"status":"ok"}"#,
    ]);
    write_global_runtime_file(&paths, &runtime).expect("global runtime file should write");

    let session_start = serde_json::json!({
        "session_id": "codex-session",
        "thread_id": "codex-session",
        "transcript_path": "/tmp/transcript.jsonl",
        "cwd": repo_root,
        "hook_event_name": "SessionStart"
    })
    .to_string();
    let output = run_hook_subprocess(
        &repo_root,
        &paths,
        &["hook", "codex", "session-start"],
        &session_start,
    );
    assert!(
        output.status.success(),
        "stateful hook failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let register = rx.recv().expect("session register request should arrive");
    assert!(register.contains("POST /v1/session/register HTTP/1.1"));
    let register_body = request_json_body(&register);
    assert_eq!(register_body["session_id"], "codex-session");
    assert_eq!(register_body["workspace_id"], "w1");

    let user_prompt = serde_json::json!({
        "session_id": "codex-session",
        "cwd": repo_root,
        "hook_event_name": "UserPromptSubmit",
        "prompt": "work on auth"
    })
    .to_string();
    let output = run_hook_subprocess(
        &repo_root,
        &paths,
        &["hook", "codex", "user-prompt-submit"],
        &user_prompt,
    );
    assert!(
        output.status.success(),
        "stateful hook failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let rendered_prompt = String::from_utf8(output.stdout).expect("prompt output should be utf8");
    assert!(rendered_prompt.contains("Lifecycle Prompt"));
    let prompt_context = rx.recv().expect("context render request should arrive");
    assert!(prompt_context.contains("POST /v1/context/render HTTP/1.1"));
    let prompt_body = request_json_body(&prompt_context);
    assert_eq!(prompt_body["session_id"], "codex-session");
    assert_eq!(prompt_body["mode"], "brief");

    let pre_tool = serde_json::json!({
        "session_id": "codex-session",
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
    let output = run_hook_subprocess(
        &repo_root,
        &paths,
        &["hook", "codex", "pre-tool-use"],
        &pre_tool,
    );
    assert!(
        output.status.success(),
        "stateful hook failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let authorize = rx.recv().expect("authorize request should arrive");
    assert!(authorize.contains("POST /v1/authorize HTTP/1.1"));
    let authorize_body = request_json_body(&authorize);
    assert_eq!(authorize_body["session"]["session_id"], "codex-session");
    assert_eq!(authorize_body["payload"]["action"], "write_file");
    assert_eq!(authorize_body["payload"]["path"], "src/auth.ts");
    let pre_context = rx
        .recv_timeout(Duration::from_secs(2))
        .expect("pre-tool context render request should arrive");
    assert!(pre_context.contains("POST /v1/context/render HTTP/1.1"));

    let post_tool = serde_json::json!({
        "session_id": "codex-session",
        "cwd": repo_root,
        "hook_event_name": "PostToolUse",
        "tool_name": "Bash",
        "tool_input": {"command": "rg auth src"}
    })
    .to_string();
    let output = run_hook_subprocess(
        &repo_root,
        &paths,
        &["hook", "codex", "post-tool-use"],
        &post_tool,
    );
    assert!(
        output.status.success(),
        "stateful hook failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let heartbeat = rx.recv().expect("heartbeat request should arrive");
    assert!(heartbeat.contains("POST /v1/session/heartbeat HTTP/1.1"));
    assert_eq!(request_json_body(&heartbeat)["session_id"], "codex-session");

    let stop = serde_json::json!({
        "session_id": "codex-session",
        "cwd": repo_root,
        "hook_event_name": "Stop"
    })
    .to_string();
    let output = run_hook_subprocess(&repo_root, &paths, &["hook", "codex", "stop"], &stop);
    assert!(
        output.status.success(),
        "stateful hook failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let finalize = rx.recv().expect("finalize request should arrive");
    assert!(finalize.contains("POST /v1/activity/finalize HTTP/1.1"));
    assert_eq!(request_json_body(&finalize)["session_id"], "codex-session");

    fs::remove_dir_all(&temp_root).expect("temp root should be removable");
}

#[test]
fn omp_stateful_lifecycle_posts_expected_server_requests() {
    let temp_root = std::env::temp_dir().join(format!(
        "stateful-hook-omp-lifecycle-test-{}",
        std::process::id()
    ));
    if temp_root.exists() {
        fs::remove_dir_all(&temp_root).expect("old temp root should be removable");
    }
    let paths = GlobalPaths::new(temp_root.join("home"));
    let repo_root = temp_root.join("repo");
    fs::create_dir_all(repo_root.join("docs")).expect("repo docs should create");
    enable_test_repo(&paths, &repo_root);
    let (runtime, rx) = spawn_fake_stateful_server_sequence(vec![
        r#"{"status":"ok"}"#,
        r#"{"decision":"allow","message":"ok"}"#,
        r#"{"status":"ok"}"#,
        r#"{"status":"ok"}"#,
    ]);
    write_global_runtime_file(&paths, &runtime).expect("global runtime file should write");

    let session_start = serde_json::json!({
        "session_id": "omp-parent",
        "workspace_id": runtime.workspace_id,
        "cwd": repo_root,
        "omp_agent_id": "main",
        "commit_id": "abc123"
    })
    .to_string();
    let output = run_hook_subprocess(
        &repo_root,
        &paths,
        &["hook", "omp", "session-start"],
        &session_start,
    );
    assert!(
        output.status.success(),
        "stateful hook failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let register = rx.recv().expect("session register request should arrive");
    assert!(register.contains("POST /v1/session/register HTTP/1.1"));
    let register_body = request_json_body(&register);
    assert_eq!(register_body["session_id"], "omp-parent");
    assert_eq!(register_body["source"]["event"], "omp_session_start");
    assert_eq!(register_body["metadata"]["runtime"], "omp");
    assert_eq!(register_body["metadata"]["omp_agent_id"], "main");

    let pre_tool = serde_json::json!({
        "session_id": "omp-parent",
        "workspace_id": runtime.workspace_id,
        "cwd": repo_root,
        "yolo": false,
        "omp_agent_id": "main",
        "commit_id": "abc123",
        "tool_name": "write",
        "tool_input": { "path": "docs/a.md", "content": "hello" }
    })
    .to_string();
    let output = run_hook_subprocess(
        &repo_root,
        &paths,
        &["hook", "omp", "pre-tool-use"],
        &pre_tool,
    );
    assert!(
        output.status.success(),
        "stateful hook failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let decision: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("OMP pre-tool output should be JSON");
    assert_eq!(decision["decision"], "allow");
    let authorize = rx.recv().expect("authorize request should arrive");
    assert!(authorize.contains("POST /v1/authorize HTTP/1.1"));
    let authorize_body = request_json_body(&authorize);
    assert_eq!(authorize_body["session"]["session_id"], "omp-parent");
    assert_eq!(authorize_body["source"]["event"], "omp_pre_tool_use");
    assert_eq!(authorize_body["payload"]["action"], "write_file");
    assert_eq!(authorize_body["payload"]["path"], "docs/a.md");

    let post_tool = serde_json::json!({
        "session_id": "omp-parent",
        "workspace_id": runtime.workspace_id,
        "cwd": repo_root,
        "omp_agent_id": "main",
        "commit_id": "abc123",
        "tool_name": "write",
        "tool_input": { "path": "docs/a.md", "content": "hello" }
    })
    .to_string();
    let output = run_hook_subprocess(
        &repo_root,
        &paths,
        &["hook", "omp", "post-tool-use"],
        &post_tool,
    );
    assert!(
        output.status.success(),
        "stateful hook failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let heartbeat = rx.recv().expect("heartbeat request should arrive");
    assert!(heartbeat.contains("POST /v1/session/heartbeat HTTP/1.1"));
    let heartbeat_body = request_json_body(&heartbeat);
    assert_eq!(heartbeat_body["session_id"], "omp-parent");
    assert_eq!(heartbeat_body["source"]["event"], "omp_post_tool_use");

    let stop = serde_json::json!({
        "session_id": "omp-parent",
        "workspace_id": runtime.workspace_id,
        "cwd": repo_root,
        "omp_agent_id": "main",
        "commit_id": "abc123"
    })
    .to_string();
    let output = run_hook_subprocess(&repo_root, &paths, &["hook", "omp", "stop"], &stop);
    assert!(
        output.status.success(),
        "stateful hook failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let finalize = rx.recv().expect("finalize request should arrive");
    assert!(finalize.contains("POST /v1/activity/finalize HTTP/1.1"));
    let finalize_body = request_json_body(&finalize);
    assert_eq!(finalize_body["session_id"], "omp-parent");
    assert_eq!(finalize_body["source"]["event"], "omp_stop");

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
    let (runtime, rx) = spawn_fake_stateful_server_sequence(vec![
        r#"{"status":"ok","prompt_text":"Nearby Activity\n- none"}"#,
        r#"{"status":"ok","prompt_text":"Nearby Activity\n- should not render twice"}"#,
    ]);
    write_global_runtime_file(&paths, &runtime).expect("global runtime file should write");

    let input = r#"{
      "session_id": "s1",
      "hook_event_name": "UserPromptSubmit",
      "prompt": "work on auth"
    }"#;

    let output = run_hook_subprocess(
        &repo_root,
        &paths,
        &["hook", "codex", "user-prompt-submit"],
        input,
    );

    assert!(
        output.status.success(),
        "stateful hook failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let rendered = String::from_utf8(output.stdout).expect("prompt output should be utf8");
    assert!(rendered.contains("Nearby Activity"));
    assert!(rendered.contains("Before using Bash"));
    assert!(rendered.contains("stateful-command-policy"));
    assert!(rendered.contains("Use canonical Stateful MCP tool names"));
    assert!(rendered.contains("state_intent_declare"));
    assert!(rendered.contains("state_lease_acquire"));
    assert!(rendered.contains("runtime-specific tool names"));
    assert!(rendered.contains("Do not run `stateful intent declare`"));
    assert!(rendered.contains("--fs read-only --network disabled"));
    assert!(rendered.contains("--fs build --network enabled"));
    assert!(rendered.contains("--fs git --network disabled"));
    assert!(rendered.contains("--fs github-pr --network enabled"));
    assert!(rendered.contains("--fs write-targets --write-target <file>"));
    let request = rx.recv().expect("captured request should arrive");
    assert!(request.contains("POST /v1/context/render HTTP/1.1"));
    assert!(request.contains("\"mode\":\"brief\""));
    assert!(request.contains("\"workspace_id\":\"w1\""));
    assert!(request.contains("\"repo_id\""));
    assert!(request.contains("\"worktree_id\""));
    assert!(request.contains("\"root\""));

    let second_output = run_hook_subprocess(
        &repo_root,
        &paths,
        &["hook", "codex", "user-prompt-submit"],
        input,
    );
    assert!(
        second_output.status.success(),
        "stateful hook failed: {}",
        String::from_utf8_lossy(&second_output.stderr)
    );
    assert!(
        second_output.stdout.is_empty(),
        "second UserPromptSubmit for the same session should not print context: {}",
        String::from_utf8_lossy(&second_output.stdout)
    );
    assert!(
        rx.recv_timeout(Duration::from_millis(200)).is_err(),
        "second UserPromptSubmit should not call /v1/context/render"
    );

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

    let output = run_hook_subprocess(&repo_root, &paths, &["hook", "codex", "stop"], input);
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
    enable_repo(paths, repo_root).expect("repo should enable");
}

fn fake_current_response(request: &str) -> String {
    let resource = request
        .strip_prefix("GET /v1/current?resource=")
        .and_then(|rest| rest.split_once(" HTTP/1.1").map(|(resource, _)| resource))
        .unwrap_or("src/auth.ts");
    serde_json::json!({
        "status": "ok",
        "current": {},
        "items": [{
            "kind": "intent",
            "freshness": "live",
            "resource": resource,
            "purpose": "Fix auth validation behavior.",
            "session_id": "s1",
            "workspace_id": "w1"
        }]
    })
    .to_string()
}

fn spawn_fake_stateful_server(
    actual_response: &'static str,
) -> (ServerRuntime, mpsc::Receiver<String>) {
    spawn_fake_stateful_server_sequence(vec![actual_response])
}

fn spawn_fake_stateful_server_sequence(
    actual_responses: Vec<&'static str>,
) -> (ServerRuntime, mpsc::Receiver<String>) {
    spawn_fake_stateful_server_sequence_with_current(actual_responses, None)
}

fn spawn_fake_stateful_server_sequence_with_current(
    actual_responses: Vec<&'static str>,
    current_response: Option<&'static str>,
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
            } else if request.starts_with("GET /v1/current") {
                let response = current_response
                    .map(str::to_string)
                    .unwrap_or_else(|| fake_current_response(&request));
                write_json_response(&mut stream, &response);
            } else if request.contains("GET /v1/runtime/identity HTTP/1.1") {
                write_json_response(
                    &mut stream,
                    r#"{"status":"ok","pid":42,"protocol_version":"stateful.v1","capabilities":["authorize.write_directory"]}"#,
                );
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

fn spawn_fake_stateful_server_dropping_authorize() -> (ServerRuntime, mpsc::Receiver<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("listener should bind");
    let addr = listener.local_addr().expect("listener addr should load");
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        while let Ok((mut stream, _)) = listener.accept() {
            let request = read_http_request_maybe_body(&mut stream);
            if request.contains("GET /health HTTP/1.1") {
                write_json_response(&mut stream, r#"{"status":"ok"}"#);
            } else if request.starts_with("GET /v1/current") {
                let response = fake_current_response(&request);
                write_json_response(&mut stream, &response);
            } else if request.contains("GET /v1/runtime/identity HTTP/1.1") {
                write_json_response(
                    &mut stream,
                    r#"{"status":"ok","pid":42,"protocol_version":"stateful.v1","capabilities":["authorize.write_directory"]}"#,
                );
            } else {
                tx.send(request).expect("request should send to test");
                drop(stream);
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

fn test_content_hash(bytes: &[u8]) -> String {
    let mut hash = 0xcbf29ce484222325u64;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("fnv1a64:{hash:016x}")
}
