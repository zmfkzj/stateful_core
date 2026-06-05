use std::{
    fs,
    io::{Read, Write},
    net::TcpListener,
    process::{Command, Stdio},
    sync::mpsc,
    thread,
    time::Duration,
};

use stateful_cli::{
    CurrentSession, GlobalPaths, STATEFUL_CODEX_RUN_ID_ENV, ServerRuntime, enable_repo,
    handle_mcp_jsonrpc_in_repo, serve_mcp_stdio_in_repo, write_current_session_file,
    write_current_session_file_for_codex_run, write_global_runtime_file,
};

#[test]
fn mcp_current_read_executes_get_request() {
    let temp_root = temp_root("stateful-mcp-current");
    let paths = GlobalPaths::new(temp_root.join("home"));
    let repo_root = temp_root.join("repo");
    fs::create_dir_all(&repo_root).expect("repo root should be creatable");
    enable_test_repo(&paths, &repo_root);
    let (runtime, rx) = spawn_fake_stateful_server(r#"{"status":"ok","current":{}}"#);
    write_global_runtime_file(&paths, &runtime).expect("global runtime file should write");

    let output = run_stateful_in_repo(&repo_root, &paths, &["mcp", "call", "state.current.read"]);

    assert!(
        output.status.success(),
        "stateful mcp call failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("\"status\":\"ok\""));
    let request = rx.recv().expect("captured request should arrive");
    assert!(request.contains("GET /v1/current HTTP/1.1"));
    assert!(request.contains("Authorization: Bearer secret-token"));

    fs::remove_dir_all(&temp_root).expect("temp root should be removable");
}

#[test]
fn mcp_call_discovers_global_runtime_file() {
    let temp_root = temp_root("stateful-mcp-global-runtime");
    fs::create_dir_all(&temp_root).expect("temp root should be creatable");
    let repo_root = temp_root.join("repo");
    fs::create_dir_all(&repo_root).expect("repo root should be creatable");
    let paths = GlobalPaths::new(temp_root.join("home"));
    enable_test_repo(&paths, &repo_root);
    let (runtime, rx) = spawn_fake_stateful_server(r#"{"status":"ok","current":{}}"#);
    write_global_runtime_file(&paths, &runtime).expect("global runtime file should write");

    let output = Command::new(env!("CARGO_BIN_EXE_stateful"))
        .args(["mcp", "call", "state.current.read"])
        .current_dir(&repo_root)
        .env_clear()
        .env("STATEFUL_HOME", &paths.home)
        .output()
        .expect("stateful mcp call should run");

    assert!(
        output.status.success(),
        "stateful mcp call failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("\"status\":\"ok\""));
    let request = rx.recv().expect("captured request should arrive");
    assert!(request.contains("GET /v1/current HTTP/1.1"));
    assert!(request.contains("Authorization: Bearer secret-token"));

    fs::remove_dir_all(&temp_root).expect("temp root should be removable");
}

#[test]
fn mcp_validation_run_adds_repo_root_and_workspace_id() {
    let temp_root = temp_root("stateful-mcp-validation");
    let paths = GlobalPaths::new(temp_root.join("home"));
    let repo_root = temp_root.join("repo");
    fs::create_dir_all(&repo_root).expect("repo root should be creatable");
    enable_test_repo(&paths, &repo_root);
    let (runtime, rx) = spawn_fake_stateful_server(r#"{"status":"passed"}"#);
    write_global_runtime_file(&paths, &runtime).expect("global runtime file should write");

    let output = run_stateful_in_repo(
        &repo_root,
        &paths,
        &[
            "mcp",
            "call",
            "state.validation.run",
            r#"{"profile":"cargo-test"}"#,
        ],
    );

    assert!(
        output.status.success(),
        "stateful mcp call failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let request = rx.recv().expect("captured request should arrive");
    assert!(request.contains("POST /v1/validation/run HTTP/1.1"));
    assert!(request.contains("\"workspace_id\":\"w1\""));
    assert!(request.contains("\"repo_root\":"));
    assert!(request.contains("\"profile\":\"cargo-test\""));

    fs::remove_dir_all(&temp_root).expect("temp root should be removable");
}

#[test]
fn mcp_validation_run_from_enabled_subdir_sends_repo_root() {
    let temp_root = temp_root("stateful-mcp-validation-subdir");
    let paths = GlobalPaths::new(temp_root.join("home"));
    let repo_root = temp_root.join("repo");
    let subdir = repo_root.join("nested/worktree");
    fs::create_dir_all(&subdir).expect("subdir should be creatable");
    enable_test_repo(&paths, &repo_root);
    let (runtime, rx) = spawn_fake_stateful_server(r#"{"status":"passed"}"#);
    write_global_runtime_file(&paths, &runtime).expect("global runtime file should write");

    let output = run_stateful_in_repo(
        &subdir,
        &paths,
        &[
            "mcp",
            "call",
            "state.validation.run",
            r#"{"profile":"cargo-test"}"#,
        ],
    );

    assert!(
        output.status.success(),
        "stateful mcp call failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let request = rx.recv().expect("captured request should arrive");
    let canonical_repo_root = repo_root
        .canonicalize()
        .expect("repo root should canonicalize");
    let canonical_subdir = subdir.canonicalize().expect("subdir should canonicalize");
    assert!(request.contains("POST /v1/validation/run HTTP/1.1"));
    assert!(request.contains(&format!(
        "\"repo_root\":\"{}\"",
        canonical_repo_root.to_string_lossy()
    )));
    assert!(!request.contains(&format!(
        "\"repo_root\":\"{}\"",
        canonical_subdir.to_string_lossy()
    )));

    fs::remove_dir_all(&temp_root).expect("temp root should be removable");
}

#[test]
fn mcp_file_write_posts_authorize_and_writes_repo_file() {
    let temp_root = temp_root("stateful-mcp-file-write");
    let paths = GlobalPaths::new(temp_root.join("home"));
    let repo_root = temp_root.join("repo");
    fs::create_dir_all(&repo_root).expect("repo root should be creatable");
    enable_test_repo(&paths, &repo_root);
    write_current_session_file(&repo_root, &CurrentSession::new("s-current", "w1"))
        .expect("current session should write");
    let (runtime, rx) = spawn_fake_stateful_server(
        r#"{"decision":"allow","reason_code":"authorized","message":"ok","required_next_action":null}"#,
    );
    write_global_runtime_file(&paths, &runtime).expect("global runtime file should write");

    let output = run_stateful_in_repo(
        &repo_root,
        &paths,
        &[
            "mcp",
            "call",
            "state.file.write",
            r#"{"path":"src/auth.ts","contents":"export const ok = true;\n"}"#,
        ],
    );

    assert!(
        output.status.success(),
        "stateful mcp file write failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let request = rx.recv().expect("captured request should arrive");
    assert!(request.contains("POST /v1/authorize HTTP/1.1"));
    let body = request_json_body(&request);
    assert_eq!(body["protocol_version"], "stateful.v1");
    assert_eq!(body["session"]["session_id"], "s-current");
    assert_eq!(body["workspace"]["workspace_id"], "w1");
    assert_eq!(body["payload"]["action"], "write_file");
    assert_eq!(body["payload"]["path"], "src/auth.ts");
    assert_eq!(body["payload"]["queue_on_conflict"], true);
    assert_eq!(
        fs::read_to_string(repo_root.join("src/auth.ts")).expect("file should be written"),
        "export const ok = true;\n"
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("\"status\":\"ok\""));

    fs::remove_dir_all(&temp_root).expect("temp root should be removable");
}

#[test]
fn mcp_file_write_uses_codex_run_session_when_legacy_current_session_changed() {
    let temp_root = temp_root("stateful-mcp-file-write-codex-run-session");
    let paths = GlobalPaths::new(temp_root.join("home"));
    let repo_root = temp_root.join("repo");
    fs::create_dir_all(&repo_root).expect("repo root should be creatable");
    enable_test_repo(&paths, &repo_root);
    write_current_session_file(&repo_root, &CurrentSession::new("s-other", "w1"))
        .expect("legacy current session should write");
    write_current_session_file_for_codex_run(
        &repo_root,
        "run-a",
        &CurrentSession::new("s-current", "w1"),
    )
    .expect("run-bound current session should write");
    let (runtime, rx) = spawn_fake_stateful_server(
        r#"{"decision":"allow","reason_code":"authorized","message":"ok","required_next_action":null}"#,
    );
    write_global_runtime_file(&paths, &runtime).expect("global runtime file should write");

    let output = run_stateful_in_repo_with_env(
        &repo_root,
        &paths,
        &[(STATEFUL_CODEX_RUN_ID_ENV, "run-a")],
        &[
            "mcp",
            "call",
            "state.file.write",
            r#"{"path":"src/auth.ts","contents":"export const ok = true;\n"}"#,
        ],
    );

    assert!(
        output.status.success(),
        "stateful mcp file write failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let request = rx.recv().expect("captured request should arrive");
    let body = request_json_body(&request);
    assert_eq!(body["session"]["session_id"], "s-current");
    assert_eq!(
        fs::read_to_string(repo_root.join("src/auth.ts")).expect("file should be written"),
        "export const ok = true;\n"
    );

    fs::remove_dir_all(&temp_root).expect("temp root should be removable");
}

#[test]
fn mcp_file_write_refuses_session_id_that_differs_from_current_session() {
    let temp_root = temp_root("stateful-mcp-file-write-session-mismatch");
    let paths = GlobalPaths::new(temp_root.join("home"));
    let repo_root = temp_root.join("repo");
    fs::create_dir_all(&repo_root).expect("repo root should be creatable");
    enable_test_repo(&paths, &repo_root);
    write_current_session_file(&repo_root, &CurrentSession::new("s-current", "w1"))
        .expect("current session should write");

    let output = run_stateful_in_repo(
        &repo_root,
        &paths,
        &[
            "mcp",
            "call",
            "state_file_write",
            r#"{"session_id":"s-other","workspace_id":"w1","path":"src/auth.ts","contents":"wrong session\n"}"#,
        ],
    );

    assert!(
        !output.status.success(),
        "mismatched session write should fail"
    );
    assert!(
        !repo_root.join("src/auth.ts").exists(),
        "mismatched session should not write the file"
    );
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("current stateful session"),
        "stdout: {}",
        String::from_utf8_lossy(&output.stdout)
    );

    fs::remove_dir_all(&temp_root).expect("temp root should be removable");
}

#[test]
fn mcp_file_write_refuses_outside_repo_path_before_writing() {
    let temp_root = temp_root("stateful-mcp-file-write-outside");
    let paths = GlobalPaths::new(temp_root.join("home"));
    let repo_root = temp_root.join("repo");
    fs::create_dir_all(&repo_root).expect("repo root should be creatable");
    enable_test_repo(&paths, &repo_root);
    write_current_session_file(&repo_root, &CurrentSession::new("s-current", "w1"))
        .expect("current session should write");
    let (runtime, _rx) = spawn_fake_stateful_server(
        r#"{"decision":"allow","reason_code":"authorized","message":"ok","required_next_action":null}"#,
    );
    write_global_runtime_file(&paths, &runtime).expect("global runtime file should write");

    let output = run_stateful_in_repo(
        &repo_root,
        &paths,
        &[
            "mcp",
            "call",
            "state_file_write",
            r#"{"path":"../outside.txt","contents":"outside\n"}"#,
        ],
    );

    assert!(!output.status.success(), "outside write should fail");
    assert!(
        !temp_root.join("outside.txt").exists(),
        "outside file should not be written"
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("\"status\":\"error\""));

    fs::remove_dir_all(&temp_root).expect("temp root should be removable");
}

#[test]
fn mcp_file_write_refuses_when_authorization_denies() {
    let temp_root = temp_root("stateful-mcp-file-write-deny");
    let paths = GlobalPaths::new(temp_root.join("home"));
    let repo_root = temp_root.join("repo");
    fs::create_dir_all(&repo_root).expect("repo root should be creatable");
    enable_test_repo(&paths, &repo_root);
    write_current_session_file(&repo_root, &CurrentSession::new("s-current", "w1"))
        .expect("current session should write");
    let (runtime, rx) = spawn_fake_stateful_server(
        r#"{"decision":"deny","reason_code":"scope_mismatch","message":"Write target is outside active intent scope.","required_next_action":"Declare matching intent."}"#,
    );
    write_global_runtime_file(&paths, &runtime).expect("global runtime file should write");

    let output = run_stateful_in_repo(
        &repo_root,
        &paths,
        &[
            "mcp",
            "call",
            "state_file_write",
            r#"{"path":"src/auth.ts","contents":"export const denied = true;\n"}"#,
        ],
    );

    assert!(!output.status.success(), "denied write should fail");
    let request = rx.recv().expect("captured request should arrive");
    assert!(request.contains("POST /v1/authorize HTTP/1.1"));
    assert!(
        !repo_root.join("src/auth.ts").exists(),
        "denied file should not be written"
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("\"decision\":\"deny\""));

    fs::remove_dir_all(&temp_root).expect("temp root should be removable");
}

#[test]
fn mcp_bash_write_reports_allowed_and_denied_targets_without_running_command() {
    let temp_root = temp_root("stateful-mcp-bash-write-deny");
    let paths = GlobalPaths::new(temp_root.join("home"));
    let repo_root = temp_root.join("repo");
    fs::create_dir_all(repo_root.join("src")).expect("repo src should be creatable");
    enable_test_repo(&paths, &repo_root);
    write_current_session_file(&repo_root, &CurrentSession::new("s-current", "w1"))
        .expect("current session should write");
    fs::write(repo_root.join("src/allowed.ts"), "old\n").expect("allowed file should seed");
    let (runtime, rx) = spawn_fake_stateful_server_sequence(vec![
        r#"{"decision":"allow","reason_code":"authorized","message":"ok","required_next_action":null}"#,
        r#"{"decision":"deny","reason_code":"scope_mismatch","message":"Target is outside active intent scope.","required_next_action":"Declare matching intent."}"#,
    ]);
    write_global_runtime_file(&paths, &runtime).expect("global runtime file should write");

    let output = run_stateful_in_repo(
        &repo_root,
        &paths,
        &[
            "mcp",
            "call",
            "state_bash_write",
            r#"{"command":"printf changed > src/allowed.ts","write_targets":["src/allowed.ts","src/denied.ts"]}"#,
        ],
    );

    assert!(!output.status.success(), "denied target should fail");
    let first = rx
        .recv_timeout(Duration::from_secs(1))
        .expect("first authorize request should arrive");
    let second = rx
        .recv_timeout(Duration::from_secs(1))
        .expect("second authorize request should arrive");
    assert_eq!(
        request_json_body(&first)["payload"]["path"],
        "src/allowed.ts"
    );
    assert_eq!(
        request_json_body(&second)["payload"]["path"],
        "src/denied.ts"
    );
    assert_eq!(
        fs::read_to_string(repo_root.join("src/allowed.ts")).expect("allowed file should read"),
        "old\n",
        "command should not run when any target is denied"
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("\"allowed_write_targets\":[\"src/allowed.ts\"]"));
    assert!(stdout.contains("\"path\":\"src/denied.ts\""));
    assert!(stdout.contains("\"decision\":\"deny\""));

    fs::remove_dir_all(&temp_root).expect("temp root should be removable");
}

#[test]
fn mcp_bash_write_rejects_case_insensitive_git_targets() {
    let temp_root = temp_root("stateful-mcp-bash-write-git-case");
    let paths = GlobalPaths::new(temp_root.join("home"));
    let repo_root = temp_root.join("repo");
    fs::create_dir_all(&repo_root).expect("repo root should be creatable");
    enable_test_repo(&paths, &repo_root);
    write_current_session_file(&repo_root, &CurrentSession::new("s-current", "w1"))
        .expect("current session should write");
    let (runtime, _rx) = spawn_fake_stateful_server(
        r#"{"decision":"allow","reason_code":"authorized","message":"ok","required_next_action":null}"#,
    );
    write_global_runtime_file(&paths, &runtime).expect("global runtime file should write");

    let output = run_stateful_in_repo(
        &repo_root,
        &paths,
        &[
            "mcp",
            "call",
            "state_bash_write",
            r#"{"command":"true","write_targets":[".GIT/config"]}"#,
        ],
    );

    assert!(!output.status.success(), "Git internals target should fail");
    assert!(String::from_utf8_lossy(&output.stdout).contains("Git internals"));

    fs::remove_dir_all(&temp_root).expect("temp root should be removable");
}

#[test]
fn sandbox_run_write_targets_reports_allowed_and_denied_without_running_command() {
    let temp_root = temp_root("stateful-sandbox-run-deny");
    let paths = GlobalPaths::new(temp_root.join("home"));
    let repo_root = temp_root.join("repo");
    fs::create_dir_all(repo_root.join("src")).expect("repo src should be creatable");
    enable_test_repo(&paths, &repo_root);
    write_current_session_file(&repo_root, &CurrentSession::new("s-current", "w1"))
        .expect("current session should write");
    fs::write(repo_root.join("src/allowed.ts"), "old\n").expect("allowed file should seed");
    let (runtime, rx) = spawn_fake_stateful_server_sequence(vec![
        r#"{"decision":"allow","reason_code":"authorized","message":"ok","required_next_action":null}"#,
        r#"{"decision":"deny","reason_code":"scope_mismatch","message":"Target is outside active intent scope.","required_next_action":"Declare matching intent."}"#,
    ]);
    write_global_runtime_file(&paths, &runtime).expect("global runtime file should write");

    let output = run_stateful_in_repo(
        &repo_root,
        &paths,
        &[
            "sandbox",
            "run",
            "--fs",
            "write-targets",
            "--network",
            "enabled",
            "--write-target",
            "src/allowed.ts",
            "--write-target",
            "src/denied.ts",
            "--command",
            "printf changed > src/allowed.ts",
        ],
    );

    assert!(!output.status.success(), "denied target should fail");
    let first = rx
        .recv_timeout(Duration::from_secs(1))
        .expect("first authorize request should arrive");
    let second = rx
        .recv_timeout(Duration::from_secs(1))
        .expect("second authorize request should arrive");
    assert_eq!(
        request_json_body(&first)["payload"]["path"],
        "src/allowed.ts"
    );
    assert_eq!(
        request_json_body(&second)["payload"]["path"],
        "src/denied.ts"
    );
    assert_eq!(
        fs::read_to_string(repo_root.join("src/allowed.ts")).expect("allowed file should read"),
        "old\n",
        "command should not run when any target is denied"
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("\"allowed_write_targets\":[\"src/allowed.ts\"]"));
    assert!(stdout.contains("\"path\":\"src/denied.ts\""));
    assert!(stdout.contains("\"decision\":\"deny\""));

    fs::remove_dir_all(&temp_root).expect("temp root should be removable");
}

#[test]
fn sandbox_run_rejects_case_insensitive_git_targets() {
    let temp_root = temp_root("stateful-sandbox-run-git-case");
    let paths = GlobalPaths::new(temp_root.join("home"));
    let repo_root = temp_root.join("repo");
    fs::create_dir_all(&repo_root).expect("repo root should be creatable");
    enable_test_repo(&paths, &repo_root);
    write_current_session_file(&repo_root, &CurrentSession::new("s-current", "w1"))
        .expect("current session should write");
    let (runtime, _rx) = spawn_fake_stateful_server(
        r#"{"decision":"allow","reason_code":"authorized","message":"ok","required_next_action":null}"#,
    );
    write_global_runtime_file(&paths, &runtime).expect("global runtime file should write");

    let output = run_stateful_in_repo(
        &repo_root,
        &paths,
        &[
            "sandbox",
            "run",
            "--fs",
            "write-targets",
            "--write-target",
            ".GIT/config",
            "--command",
            "true",
        ],
    );

    assert!(!output.status.success(), "Git internals target should fail");
    assert!(String::from_utf8_lossy(&output.stderr).contains("Git internals"));

    fs::remove_dir_all(&temp_root).expect("temp root should be removable");
}

#[test]
fn sandbox_run_reports_authorize_server_errors_on_stderr() {
    let temp_root = temp_root("stateful-sandbox-run-authorize-500");
    let paths = GlobalPaths::new(temp_root.join("home"));
    let repo_root = temp_root.join("repo");
    fs::create_dir_all(repo_root.join("src")).expect("repo src should be creatable");
    enable_test_repo(&paths, &repo_root);
    write_current_session_file(&repo_root, &CurrentSession::new("s-current", "w1"))
        .expect("current session should write");
    fs::write(repo_root.join("src/allowed.ts"), "old\n").expect("allowed file should seed");
    let (runtime, _rx) =
        spawn_fake_stateful_server_sequence_with_status(vec![(500, r#"{"status":"error"}"#)]);
    write_global_runtime_file(&paths, &runtime).expect("global runtime file should write");

    let output = run_stateful_in_repo(
        &repo_root,
        &paths,
        &[
            "sandbox",
            "run",
            "--fs",
            "write-targets",
            "--write-target",
            "src/allowed.ts",
            "--command",
            "true",
        ],
    );

    assert!(
        !output.status.success(),
        "authorize server error should fail"
    );
    assert!(
        !String::from_utf8_lossy(&output.stdout).contains("denied_write_targets"),
        "non-policy authorize errors should not use denial stdout"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("authorize"));
    assert!(stderr.contains("500"));

    fs::remove_dir_all(&temp_root).expect("temp root should be removable");
}

#[test]
fn sandbox_run_read_only_rejects_write_targets() {
    let temp_root = temp_root("stateful-sandbox-run-readonly-target");
    let paths = GlobalPaths::new(temp_root.join("home"));
    let repo_root = temp_root.join("repo");
    fs::create_dir_all(&repo_root).expect("repo root should be creatable");
    enable_test_repo(&paths, &repo_root);
    let (runtime, _rx) = spawn_fake_stateful_server(r#"{"status":"ok"}"#);
    write_global_runtime_file(&paths, &runtime).expect("global runtime file should write");

    let output = run_stateful_in_repo(
        &repo_root,
        &paths,
        &[
            "sandbox",
            "run",
            "--fs",
            "read-only",
            "--write-target",
            "README.md",
            "--command",
            "rg README",
        ],
    );

    assert!(
        !output.status.success(),
        "read-only must reject write targets"
    );
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("read-only profile rejects write targets")
    );

    fs::remove_dir_all(&temp_root).expect("temp root should be removable");
}

#[cfg(target_os = "macos")]
#[test]
fn mcp_bash_write_macos_seatbelt_allows_only_authorized_targets() {
    let temp_root = temp_root("stateful-mcp-bash-write-seatbelt");
    let paths = GlobalPaths::new(temp_root.join("home"));
    let repo_root = temp_root.join("repo");
    fs::create_dir_all(repo_root.join("src")).expect("repo src should be creatable");
    enable_test_repo(&paths, &repo_root);
    write_current_session_file(&repo_root, &CurrentSession::new("s-current", "w1"))
        .expect("current session should write");
    fs::write(repo_root.join("src/allowed.ts"), "old\n").expect("allowed file should seed");
    let (runtime, rx) = spawn_fake_stateful_server(
        r#"{"decision":"allow","reason_code":"authorized","message":"ok","required_next_action":null}"#,
    );
    write_global_runtime_file(&paths, &runtime).expect("global runtime file should write");

    let output = run_stateful_in_repo(
        &repo_root,
        &paths,
        &[
            "mcp",
            "call",
            "state_bash_write",
            r#"{"command":"printf changed > src/allowed.ts; printf denied > src/denied.ts","write_targets":["src/allowed.ts"]}"#,
        ],
    );

    assert!(
        output.status.success(),
        "sandboxed command result should be returned even when command exits nonzero: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let request = rx
        .recv_timeout(Duration::from_secs(1))
        .expect("authorize request should arrive");
    assert_eq!(
        request_json_body(&request)["payload"]["path"],
        "src/allowed.ts"
    );
    assert_eq!(
        fs::read_to_string(repo_root.join("src/allowed.ts")).expect("allowed file should read"),
        "changed",
    );
    assert!(
        !repo_root.join("src/denied.ts").exists(),
        "unlisted file should not be created"
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("\"status\":\"exited\""));
    assert!(stdout.contains("\"exit_code\":1"));
    assert!(stdout.contains("Operation not permitted"));

    fs::remove_dir_all(&temp_root).expect("temp root should be removable");
}

#[test]
fn mcp_tools_list_returns_stateful_tool_descriptors() {
    let temp_root = temp_root("stateful-mcp-tools-list");
    fs::create_dir_all(&temp_root).expect("temp root should be creatable");

    let response = handle_mcp_jsonrpc_in_repo(
        &temp_root,
        r#"{"jsonrpc":"2.0","id":1,"method":"tools/list","params":{}}"#,
    )
    .expect("tools/list should handle")
    .expect("tools/list should produce response");
    let json: serde_json::Value = serde_json::from_str(&response).expect("response should be json");

    assert_eq!(json["jsonrpc"], "2.0");
    assert_eq!(json["id"], 1);
    let tools = json["result"]["tools"]
        .as_array()
        .expect("tools should be array");
    assert!(
        tools
            .iter()
            .any(|tool| tool["name"] == "state_intent_declare")
    );
    assert!(tools.iter().any(|tool| tool["name"] == "state_file_write"));
    let intent_tool = tools
        .iter()
        .find(|tool| tool["name"] == "state_intent_declare")
        .expect("intent tool should be listed");
    assert_eq!(
        intent_tool["inputSchema"]["required"],
        serde_json::json!(["files_planned"])
    );
    assert_eq!(
        intent_tool["inputSchema"]["properties"]["files_planned"]["items"]["type"],
        "string"
    );

    fs::remove_dir_all(&temp_root).expect("temp root should be removable");
}

#[test]
fn mcp_tool_call_in_disabled_repo_returns_repo_not_enabled() {
    let temp_root = temp_root("stateful-mcp-disabled");
    fs::create_dir_all(temp_root.join(".git")).expect("git marker should write");

    let response = handle_mcp_jsonrpc_in_repo(
        &temp_root,
        r#"{
          "jsonrpc":"2.0",
          "id":7,
          "method":"tools/call",
          "params":{
            "name":"state_current_read",
            "arguments":{}
          }
        }"#,
    )
    .expect("disabled repo should handle")
    .expect("tools/call should produce response");

    let json: serde_json::Value = serde_json::from_str(&response).expect("response should be json");
    assert_eq!(json["result"]["isError"], true);
    assert!(
        json["result"]["content"][0]["text"]
            .as_str()
            .unwrap_or_default()
            .contains("repo not enabled")
    );
    fs::remove_dir_all(&temp_root).expect("temp root should be removable");
}

#[test]
fn mcp_tools_call_for_intent_declare_posts_to_state_server() {
    let temp_root = temp_root("stateful-mcp-intent-declare");
    let paths = GlobalPaths::new(temp_root.join("home"));
    let repo_root = temp_root.join("repo");
    fs::create_dir_all(&repo_root).expect("repo root should be creatable");
    enable_test_repo(&paths, &repo_root);
    let (runtime, rx) = spawn_fake_stateful_server(r#"{"status":"ok"}"#);
    write_global_runtime_file(&paths, &runtime).expect("global runtime file should write");

    let response = run_mcp_jsonrpc_in_repo(
        &repo_root,
        &paths,
        r#"{
          "jsonrpc":"2.0",
          "id":2,
          "method":"tools/call",
          "params":{
            "name":"state_intent_declare",
            "arguments":{
              "session_id":"s1",
              "workspace_id":"w1",
              "files_planned":["src/auth.ts"]
            }
          }
        }"#,
    );

    let request = rx.recv().expect("captured request should arrive");
    assert!(request.contains("POST /v1/intent/declare HTTP/1.1"));
    assert!(request.contains("Authorization: Bearer secret-token"));
    let body = request_json_body(&request);
    assert_eq!(body["protocol_version"], "stateful.v1");
    assert!(
        body["request_id"]
            .as_str()
            .is_some_and(|value| !value.is_empty())
    );
    assert_eq!(body["session"]["session_id"], "s1");
    assert_eq!(body["workspace"]["workspace_id"], "w1");
    assert!(
        body["workspace"]["repo_id"]
            .as_str()
            .is_some_and(|value| value.starts_with("repo-"))
    );
    assert!(
        body["workspace"]["worktree_id"]
            .as_str()
            .is_some_and(|value| value.starts_with("repo-"))
    );
    let canonical_repo_root = repo_root
        .canonicalize()
        .expect("repo root should canonicalize");
    assert_eq!(
        body["workspace"]["root"],
        canonical_repo_root.to_string_lossy().as_ref()
    );
    assert!(
        body["workspace"]["branch"]
            .as_str()
            .is_some_and(|value| !value.is_empty())
    );
    assert_eq!(
        body["payload"],
        serde_json::json!({
            "files_planned": ["src/auth.ts"]
        })
    );
    assert!(body.get("files_planned").is_none());

    let json: serde_json::Value = serde_json::from_str(&response).expect("response should be json");
    assert_eq!(json["jsonrpc"], "2.0");
    assert_eq!(json["id"], 2);
    assert_eq!(json["result"]["isError"], false);
    assert_eq!(json["result"]["content"][0]["type"], "text");
    assert!(
        json["result"]["content"][0]["text"]
            .as_str()
            .unwrap_or_default()
            .contains("\"status\":\"ok\"")
    );

    fs::remove_dir_all(&temp_root).expect("temp root should be removable");
}

#[test]
fn intent_declare_command_posts_repo_identity() {
    let temp_root = temp_root("stateful-cli-intent-identity");
    let paths = GlobalPaths::new(temp_root.join("home"));
    let repo_root = temp_root.join("repo");
    fs::create_dir_all(&repo_root).expect("repo root should be creatable");
    enable_test_repo(&paths, &repo_root);
    let (runtime, rx) = spawn_fake_stateful_server(r#"{"status":"ok"}"#);
    write_global_runtime_file(&paths, &runtime).expect("global runtime file should write");
    write_current_session_file(&repo_root, &CurrentSession::new("s-current", "w1"))
        .expect("current session should write");

    let output = run_stateful_in_repo(&repo_root, &paths, &["intent", "declare", "src/auth.ts"]);

    assert!(
        output.status.success(),
        "stateful intent declare failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let request = rx.recv().expect("captured request should arrive");
    assert!(request.contains("POST /v1/intent/declare HTTP/1.1"));
    let body = request_json_body(&request);
    assert_eq!(body["protocol_version"], "stateful.v1");
    assert_eq!(body["session"]["session_id"], "s-current");
    assert_eq!(body["workspace"]["workspace_id"], "w1");
    assert!(
        body["workspace"]["repo_id"]
            .as_str()
            .is_some_and(|value| value.starts_with("repo-"))
    );
    assert!(
        body["workspace"]["worktree_id"]
            .as_str()
            .is_some_and(|value| value.starts_with("repo-"))
    );
    let canonical_repo_root = repo_root
        .canonicalize()
        .expect("repo root should canonicalize");
    assert_eq!(
        body["workspace"]["root"],
        canonical_repo_root.to_string_lossy().as_ref()
    );
    assert!(
        body["workspace"]["branch"]
            .as_str()
            .is_some_and(|value| !value.is_empty())
    );
    assert_eq!(
        body["payload"],
        serde_json::json!({
            "files_planned": ["src/auth.ts"]
        })
    );

    fs::remove_dir_all(&temp_root).expect("temp root should be removable");
}

#[test]
fn mcp_intent_declare_defaults_to_current_hook_session() {
    let temp_root = temp_root("stateful-mcp-intent-current-session");
    let paths = GlobalPaths::new(temp_root.join("home"));
    let repo_root = temp_root.join("repo");
    fs::create_dir_all(&repo_root).expect("repo root should be creatable");
    enable_test_repo(&paths, &repo_root);
    let (runtime, rx) = spawn_fake_stateful_server(r#"{"status":"ok"}"#);
    write_global_runtime_file(&paths, &runtime).expect("global runtime file should write");
    write_current_session_file(&repo_root, &CurrentSession::new("s-current", "w1"))
        .expect("current session should write");

    let response = run_mcp_jsonrpc_in_repo(
        &repo_root,
        &paths,
        r#"{
          "jsonrpc":"2.0",
          "id":3,
          "method":"tools/call",
          "params":{
            "name":"state_intent_declare",
            "arguments":{
              "files_planned":["src/auth.ts"]
            }
          }
        }"#,
    );

    let request = rx.recv().expect("captured request should arrive");
    let body = request_json_body(&request);
    assert_eq!(body["protocol_version"], "stateful.v1");
    assert_eq!(body["session"]["session_id"], "s-current");
    assert_eq!(body["workspace"]["workspace_id"], "w1");
    assert_eq!(
        body["payload"],
        serde_json::json!({
            "files_planned": ["src/auth.ts"]
        })
    );

    let json: serde_json::Value = serde_json::from_str(&response).expect("response should be json");
    assert_eq!(json["jsonrpc"], "2.0");
    assert_eq!(json["id"], 3);
    assert_eq!(json["result"]["isError"], false);

    fs::remove_dir_all(&temp_root).expect("temp root should be removable");
}

#[test]
fn mcp_intent_declare_refuses_session_id_that_differs_from_current_session() {
    let temp_root = temp_root("stateful-mcp-intent-session-mismatch");
    let paths = GlobalPaths::new(temp_root.join("home"));
    let repo_root = temp_root.join("repo");
    fs::create_dir_all(&repo_root).expect("repo root should be creatable");
    enable_test_repo(&paths, &repo_root);
    write_current_session_file(&repo_root, &CurrentSession::new("s-current", "w1"))
        .expect("current session should write");

    let response = run_mcp_jsonrpc_in_repo(
        &repo_root,
        &paths,
        r#"{
          "jsonrpc":"2.0",
          "id":4,
          "method":"tools/call",
          "params":{
            "name":"state_intent_declare",
            "arguments":{
              "session_id":"s-other",
              "workspace_id":"w1",
              "files_planned":["src/auth.ts"]
            }
          }
        }"#,
    );

    let json: serde_json::Value = serde_json::from_str(&response).expect("response should be json");
    assert_eq!(json["jsonrpc"], "2.0");
    assert_eq!(json["id"], 4);
    assert_eq!(json["result"]["isError"], true);
    assert!(
        json["result"]["content"][0]["text"]
            .as_str()
            .unwrap_or_default()
            .contains("current stateful session")
    );

    fs::remove_dir_all(&temp_root).expect("temp root should be removable");
}

#[test]
fn mcp_stdio_accepts_lf_only_content_length_headers() {
    let temp_root = temp_root("stateful-mcp-lf-headers");
    fs::create_dir_all(&temp_root).expect("temp root should be creatable");
    let body = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}"#;
    let input = format!("Content-Length: {}\n\n{}", body.len(), body);
    let mut output = Vec::new();

    serve_mcp_stdio_in_repo(&temp_root, input.as_bytes(), &mut output)
        .expect("stdio server should handle LF-only headers");

    let output = String::from_utf8(output).expect("output should be utf8");
    assert!(output.starts_with("Content-Length: "));
    assert!(output.contains("\"jsonrpc\":\"2.0\""));
    assert!(output.contains("\"serverInfo\""));

    fs::remove_dir_all(&temp_root).expect("temp root should be removable");
}

#[test]
fn mcp_stdio_accepts_newline_delimited_jsonrpc() {
    let temp_root = temp_root("stateful-mcp-json-line");
    fs::create_dir_all(&temp_root).expect("temp root should be creatable");
    let input = b"{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"initialize\",\"params\":{}}\n";
    let mut output = Vec::new();

    serve_mcp_stdio_in_repo(&temp_root, &input[..], &mut output)
        .expect("stdio server should handle newline-delimited JSON-RPC");

    let output = String::from_utf8(output).expect("output should be utf8");
    assert!(output.starts_with('{'));
    assert!(output.ends_with('\n'));
    assert!(output.contains("\"id\":1"));
    assert!(output.contains("\"jsonrpc\":\"2.0\""));
    assert!(output.contains("\"serverInfo\""));

    fs::remove_dir_all(&temp_root).expect("temp root should be removable");
}

fn temp_root(name: &str) -> std::path::PathBuf {
    let root = std::env::temp_dir().join(format!("{name}-{}", std::process::id()));
    if root.exists() {
        fs::remove_dir_all(&root).expect("old temp root should be removable");
    }
    root
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
    responses: Vec<&'static str>,
) -> (ServerRuntime, mpsc::Receiver<String>) {
    spawn_fake_stateful_server_sequence_with_status(
        responses
            .into_iter()
            .map(|response| (200, response))
            .collect::<Vec<_>>(),
    )
}

fn spawn_fake_stateful_server_sequence_with_status(
    responses: Vec<(u16, &'static str)>,
) -> (ServerRuntime, mpsc::Receiver<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("listener should bind");
    let addr = listener.local_addr().expect("listener addr should load");
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let mut health_seen = false;
        let mut current_seen = false;
        let mut responses = responses.into_iter();
        for _ in 0..16 {
            let (mut stream, _) = listener.accept().expect("connection should arrive");
            let request = read_http_request_maybe_body(&mut stream);
            if request.contains("GET /health HTTP/1.1") && !health_seen {
                health_seen = true;
                write_json_response(&mut stream, r#"{"status":"ok"}"#);
            } else if request.contains("GET /v1/current HTTP/1.1") && !current_seen {
                current_seen = true;
                write_json_response(&mut stream, r#"{"status":"ok","current":{}}"#);
            } else {
                tx.send(request).expect("request should send to test");
                let (status_code, response) =
                    responses.next().unwrap_or((200, r#"{"status":"ok"}"#));
                write_json_response_with_status(&mut stream, status_code, response);
                if responses.as_slice().is_empty() {
                    break;
                }
            }
        }
    });

    (
        ServerRuntime::new(format!("http://{addr}"), "secret-token", "w1", 42),
        rx,
    )
}

fn run_stateful_in_repo(
    repo_root: &std::path::Path,
    paths: &GlobalPaths,
    args: &[&str],
) -> std::process::Output {
    run_stateful_in_repo_with_env(repo_root, paths, &[], args)
}

fn run_stateful_in_repo_with_env(
    repo_root: &std::path::Path,
    paths: &GlobalPaths,
    env: &[(&str, &str)],
    args: &[&str],
) -> std::process::Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_stateful"));
    command
        .args(args)
        .current_dir(repo_root)
        .env_clear()
        .env("STATEFUL_HOME", &paths.home);
    for (key, value) in env {
        command.env(key, value);
    }
    command.output().expect("stateful command should run")
}

fn run_mcp_jsonrpc_in_repo(
    repo_root: &std::path::Path,
    paths: &GlobalPaths,
    message: &str,
) -> String {
    let mut child = Command::new(env!("CARGO_BIN_EXE_stateful"))
        .args(["mcp", "serve"])
        .current_dir(repo_root)
        .env_clear()
        .env("STATEFUL_HOME", &paths.home)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("stateful mcp serve should spawn");
    let mut stdin = child.stdin.take().expect("stdin should be piped");
    let message = serde_json::to_string(
        &serde_json::from_str::<serde_json::Value>(message).expect("message should be json"),
    )
    .expect("message should serialize");
    stdin
        .write_all(format!("{message}\n").as_bytes())
        .expect("mcp request should write");
    drop(stdin);
    let output = child
        .wait_with_output()
        .expect("stateful mcp serve should complete");
    assert!(
        output.status.success(),
        "stateful mcp serve failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).expect("mcp output should be utf8")
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

fn write_json_response(stream: &mut std::net::TcpStream, body: &str) {
    write_json_response_with_status(stream, 200, body);
}

fn write_json_response_with_status(stream: &mut std::net::TcpStream, status_code: u16, body: &str) {
    let reason = match status_code {
        200 => "OK",
        500 => "Internal Server Error",
        _ => "Unknown",
    };
    let response = format!(
        "HTTP/1.1 {status_code} {reason}\r\nContent-Length: {}\r\n\r\n{}",
        body.len(),
        body
    );
    stream
        .write_all(response.as_bytes())
        .expect("response should write");
}
