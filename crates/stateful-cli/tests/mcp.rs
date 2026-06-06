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
    CurrentSession, GlobalPaths, ServerRuntime, enable_repo, handle_mcp_jsonrpc_in_repo,
    serve_mcp_stdio_in_repo, write_current_session_file, write_global_runtime_file,
};

#[test]
fn mcp_session_heartbeat_executes_post_request() {
    let temp_root = temp_root("stateful-mcp-heartbeat");
    let paths = GlobalPaths::new(temp_root.join("home"));
    let repo_root = temp_root.join("repo");
    fs::create_dir_all(&repo_root).expect("repo root should be creatable");
    enable_test_repo(&paths, &repo_root);
    let (runtime, rx) = spawn_fake_stateful_server(r#"{"status":"ok"}"#);
    write_global_runtime_file(&paths, &runtime).expect("global runtime file should write");

    let output = run_stateful_in_repo(
        &repo_root,
        &paths,
        &[
            "mcp",
            "call",
            "state.session.heartbeat",
            r#"{"session_id":"s1","workspace_id":"w1"}"#,
        ],
    );

    assert!(
        output.status.success(),
        "stateful mcp call failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("\"status\":\"ok\""));
    let request = rx.recv().expect("captured request should arrive");
    assert!(request.contains("POST /v1/session/heartbeat HTTP/1.1"));
    assert!(request.contains("Authorization: Bearer secret-token"));
    assert!(request.contains("\"session_id\":\"s1\""));

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
    let (runtime, rx) = spawn_fake_stateful_server(r#"{"status":"ok"}"#);
    write_global_runtime_file(&paths, &runtime).expect("global runtime file should write");

    let output = Command::new(env!("CARGO_BIN_EXE_stateful"))
        .args([
            "mcp",
            "call",
            "state.session.heartbeat",
            r#"{"session_id":"s1","workspace_id":"w1"}"#,
        ])
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
    assert!(request.contains("POST /v1/session/heartbeat HTTP/1.1"));
    assert!(request.contains("Authorization: Bearer secret-token"));

    fs::remove_dir_all(&temp_root).expect("temp root should be removable");
}

#[test]
fn mcp_call_rejects_removed_replaceable_tools() {
    let temp_root = temp_root("stateful-mcp-removed-tool");
    let paths = GlobalPaths::new(temp_root.join("home"));
    let repo_root = temp_root.join("repo");
    fs::create_dir_all(&repo_root).expect("repo root should be creatable");
    enable_test_repo(&paths, &repo_root);
    let (runtime, _rx) = spawn_fake_stateful_server(r#"{"status":"ok"}"#);
    write_global_runtime_file(&paths, &runtime).expect("global runtime file should write");

    for tool_name in [
        "state.current.read",
        "state.events.read",
        "state.context.render",
        "state.validation.run",
        "state.file.write",
        "state_activity_observe",
        "state_activity_finalize",
    ] {
        let output = run_stateful_in_repo(&repo_root, &paths, &["mcp", "call", tool_name]);
        assert!(!output.status.success(), "{tool_name} should be rejected");
        assert!(
            String::from_utf8_lossy(&output.stderr).contains("unknown stateful MCP tool"),
            "stderr for {tool_name}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fs::remove_dir_all(&temp_root).expect("temp root should be removable");
}

#[test]
fn mcp_stdio_returns_error_response_for_bad_tool_without_terminating() {
    let temp_root = temp_root("stateful-mcp-stdio-tool-error");
    let paths = GlobalPaths::new(temp_root.join("home"));
    let repo_root = temp_root.join("repo");
    fs::create_dir_all(&repo_root).expect("repo root should be creatable");
    enable_test_repo(&paths, &repo_root);
    let (runtime, rx) = spawn_fake_stateful_server(r#"{"status":"ok"}"#);
    write_global_runtime_file(&paths, &runtime).expect("global runtime file should write");

    let request = r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"state.missing.tool","arguments":{}}}
{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"state_session_heartbeat","arguments":{"session_id":"s1","workspace_id":"w1"}}}
"#;
    let output = run_mcp_jsonrpc_lines_in_repo(&repo_root, &paths, request);
    assert!(output.contains("\"id\":1"));
    assert!(output.contains("\"isError\":true") || output.contains("\"error\""));
    assert!(output.contains("\"id\":2"));
    assert!(
        output.contains("\"isError\":false"),
        "second request should succeed, output: {output}"
    );
    let request = rx.recv().expect("second request should reach server");
    assert!(request.contains("POST /v1/session/heartbeat HTTP/1.1"));

    fs::remove_dir_all(&temp_root).expect("temp root should be removable");
}

#[test]
fn mcp_stdio_returns_jsonrpc_error_for_malformed_tool_call_without_terminating() {
    let temp_root = temp_root("stateful-mcp-stdio-malformed-tool-call");
    let paths = GlobalPaths::new(temp_root.join("home"));
    let repo_root = temp_root.join("repo");
    fs::create_dir_all(&repo_root).expect("repo root should be creatable");
    enable_test_repo(&paths, &repo_root);
    let (runtime, rx) = spawn_fake_stateful_server(r#"{"status":"ok"}"#);
    write_global_runtime_file(&paths, &runtime).expect("global runtime file should write");

    let request = r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"arguments":{}}}
{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"state_session_heartbeat","arguments":{"session_id":"s1","workspace_id":"w1"}}}
"#;
    let output = run_mcp_jsonrpc_lines_in_repo(&repo_root, &paths, request);
    assert!(output.contains("\"id\":1"));
    assert!(output.contains("\"code\":-32602"));
    assert!(output.contains("missing params.name"));
    assert!(output.contains("\"id\":2"));
    assert!(
        output.contains("\"isError\":false"),
        "second request should succeed, output: {output}"
    );
    let request = rx.recv().expect("second request should reach server");
    assert!(request.contains("POST /v1/session/heartbeat HTTP/1.1"));

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
    fs::write(repo_root.join("src/denied.ts"), "old denied\n").expect("denied file should seed");
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
fn mcp_bash_write_rejects_eval_before_authorization() {
    let temp_root = temp_root("stateful-mcp-bash-write-eval-detached");
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
            r#"{"command":"eval \"nohup sleep 5\"; printf changed > src/allowed.ts","write_targets":["src/allowed.ts"]}"#,
        ],
    );

    assert!(!output.status.success(), "eval construct should fail");
    assert!(String::from_utf8_lossy(&output.stdout).contains("detached process"));
    assert!(
        rx.recv_timeout(Duration::from_millis(100)).is_err(),
        "detached eval should fail before authorization"
    );
    assert_eq!(
        fs::read_to_string(repo_root.join("src/allowed.ts")).expect("allowed file should read"),
        "old\n"
    );

    fs::remove_dir_all(&temp_root).expect("temp root should be removable");
}

#[test]
fn mcp_bash_write_rejects_wrapper_option_detached_processes_before_authorization() {
    let temp_root = temp_root("stateful-mcp-bash-write-wrapper-option-detached");
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
            r#"{"command":"command -- python3 -c 'import os; getattr(os, \"set\" + \"sid\")()'; printf changed > src/allowed.ts","write_targets":["src/allowed.ts"]}"#,
        ],
    );

    assert!(
        !output.status.success(),
        "wrapper option detached construct should fail"
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("detached process"));
    assert!(
        rx.recv_timeout(Duration::from_millis(100)).is_err(),
        "wrapper option detached construct should fail before authorization"
    );
    assert_eq!(
        fs::read_to_string(repo_root.join("src/allowed.ts")).expect("allowed file should read"),
        "old\n"
    );

    fs::remove_dir_all(&temp_root).expect("temp root should be removable");
}

#[test]
fn mcp_bash_write_rejects_shell_function_detached_processes_before_authorization() {
    let temp_root = temp_root("stateful-mcp-bash-write-function-detached");
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
            r#"{"command":"f(){ python3 -c 'import os; getattr(os, \"set\" + \"sid\")()'; }; f; printf changed > src/allowed.ts","write_targets":["src/allowed.ts"]}"#,
        ],
    );

    assert!(
        !output.status.success(),
        "shell function detached construct should fail"
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("detached process"));
    assert!(
        rx.recv_timeout(Duration::from_millis(100)).is_err(),
        "shell function detached construct should fail before authorization"
    );
    assert_eq!(
        fs::read_to_string(repo_root.join("src/allowed.ts")).expect("allowed file should read"),
        "old\n"
    );

    fs::remove_dir_all(&temp_root).expect("temp root should be removable");
}

#[test]
fn mcp_bash_write_rejects_trap_detached_processes_before_authorization() {
    let temp_root = temp_root("stateful-mcp-bash-write-trap-detached");
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
            r#"{"command":"trap 'python3 -c \"import os; getattr(os, \\\"set\\\" + \\\"sid\\\")()\"' EXIT; printf changed > src/allowed.ts","write_targets":["src/allowed.ts"]}"#,
        ],
    );

    assert!(
        !output.status.success(),
        "trap detached construct should fail"
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("detached process"));
    assert!(
        rx.recv_timeout(Duration::from_millis(100)).is_err(),
        "trap detached construct should fail before authorization"
    );
    assert_eq!(
        fs::read_to_string(repo_root.join("src/allowed.ts")).expect("allowed file should read"),
        "old\n"
    );

    fs::remove_dir_all(&temp_root).expect("temp root should be removable");
}

#[test]
fn mcp_bash_write_rejects_env_split_string_detached_processes_before_authorization() {
    let temp_root = temp_root("stateful-mcp-bash-write-env-s-detached");
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
            r#"{"command":"env -Spython3 -c 'import os; getattr(os, \"set\" + \"sid\")()'; printf changed > src/allowed.ts","write_targets":["src/allowed.ts"]}"#,
        ],
    );

    assert!(
        !output.status.success(),
        "env -S detached construct should fail"
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("detached process"));
    assert!(
        rx.recv_timeout(Duration::from_millis(100)).is_err(),
        "env -S detached construct should fail before authorization"
    );
    assert_eq!(
        fs::read_to_string(repo_root.join("src/allowed.ts")).expect("allowed file should read"),
        "old\n"
    );

    fs::remove_dir_all(&temp_root).expect("temp root should be removable");
}

#[test]
fn mcp_bash_write_rejects_path_qualified_commands_before_authorization() {
    let temp_root = temp_root("stateful-mcp-bash-write-path-command-detached");
    let paths = GlobalPaths::new(temp_root.join("home"));
    let repo_root = temp_root.join("repo");
    fs::create_dir_all(repo_root.join("src")).expect("repo src should be creatable");
    enable_test_repo(&paths, &repo_root);
    write_current_session_file(&repo_root, &CurrentSession::new("s-current", "w1"))
        .expect("current session should write");
    fs::write(repo_root.join("src/allowed.ts"), "old\n").expect("allowed file should seed");
    fs::write(
        repo_root.join("src/runner"),
        "#!/bin/sh\nprintf late > src/allowed.ts\n",
    )
    .expect("runner should seed");
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
            r#"{"command":"src/runner; printf changed > src/allowed.ts","write_targets":["src/allowed.ts","src/runner"]}"#,
        ],
    );

    assert!(
        !output.status.success(),
        "path-qualified command should fail"
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("detached process"));
    assert!(
        rx.recv_timeout(Duration::from_millis(100)).is_err(),
        "path-qualified command should fail before authorization"
    );
    assert_eq!(
        fs::read_to_string(repo_root.join("src/allowed.ts")).expect("allowed file should read"),
        "old\n"
    );

    fs::remove_dir_all(&temp_root).expect("temp root should be removable");
}

#[test]
fn mcp_bash_write_rejects_path_env_assignment_before_authorization() {
    let temp_root = temp_root("stateful-mcp-bash-write-path-env-detached");
    let paths = GlobalPaths::new(temp_root.join("home"));
    let repo_root = temp_root.join("repo");
    fs::create_dir_all(repo_root.join("src")).expect("repo src should be creatable");
    enable_test_repo(&paths, &repo_root);
    write_current_session_file(&repo_root, &CurrentSession::new("s-current", "w1"))
        .expect("current session should write");
    fs::write(repo_root.join("src/allowed.ts"), "old\n").expect("allowed file should seed");
    fs::write(
        repo_root.join("src/runner"),
        "#!/bin/sh\nprintf late > src/allowed.ts\n",
    )
    .expect("runner should seed");
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
            r#"{"command":"PATH=src:$PATH runner; printf changed > src/allowed.ts","write_targets":["src/allowed.ts","src/runner"]}"#,
        ],
    );

    assert!(
        !output.status.success(),
        "PATH assignment command should fail"
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("detached process"));
    assert!(
        rx.recv_timeout(Duration::from_millis(100)).is_err(),
        "PATH assignment command should fail before authorization"
    );
    assert_eq!(
        fs::read_to_string(repo_root.join("src/allowed.ts")).expect("allowed file should read"),
        "old\n"
    );

    fs::remove_dir_all(&temp_root).expect("temp root should be removable");
}

#[test]
fn mcp_bash_write_rejects_dynamic_loader_env_before_authorization() {
    let temp_root = temp_root("stateful-mcp-bash-write-loader-env-detached");
    let paths = GlobalPaths::new(temp_root.join("home"));
    let repo_root = temp_root.join("repo");
    fs::create_dir_all(repo_root.join("src")).expect("repo src should be creatable");
    enable_test_repo(&paths, &repo_root);
    write_current_session_file(&repo_root, &CurrentSession::new("s-current", "w1"))
        .expect("current session should write");
    fs::write(repo_root.join("src/allowed.ts"), "old\n").expect("allowed file should seed");
    fs::write(repo_root.join("src/libdetach.so"), "not a real library\n")
        .expect("library placeholder should seed");
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
            r#"{"command":"LD_PRELOAD=src/libdetach.so cat /dev/null; printf changed > src/allowed.ts","write_targets":["src/allowed.ts","src/libdetach.so"]}"#,
        ],
    );

    assert!(!output.status.success(), "dynamic loader env should fail");
    assert!(String::from_utf8_lossy(&output.stdout).contains("detached process"));
    assert!(
        rx.recv_timeout(Duration::from_millis(100)).is_err(),
        "dynamic loader env should fail before authorization"
    );
    assert_eq!(
        fs::read_to_string(repo_root.join("src/allowed.ts")).expect("allowed file should read"),
        "old\n"
    );

    fs::remove_dir_all(&temp_root).expect("temp root should be removable");
}

#[cfg(target_os = "macos")]
#[test]
fn mcp_bash_write_uses_safe_path_for_allowed_commands() {
    let temp_root = temp_root("stateful-mcp-bash-write-safe-path");
    let paths = GlobalPaths::new(temp_root.join("home"));
    let repo_root = temp_root.join("repo");
    fs::create_dir_all(repo_root.join("src")).expect("repo src should be creatable");
    fs::create_dir_all(repo_root.join("bin")).expect("repo bin should be creatable");
    enable_test_repo(&paths, &repo_root);
    write_current_session_file(&repo_root, &CurrentSession::new("s-current", "w1"))
        .expect("current session should write");
    fs::write(repo_root.join("src/allowed.ts"), "old\n").expect("allowed file should seed");
    let fake_cat = repo_root.join("bin/cat");
    fs::write(&fake_cat, "#!/bin/sh\nprintf malicious\n").expect("fake cat should seed");
    let mut permissions = fs::metadata(&fake_cat)
        .expect("fake cat metadata should read")
        .permissions();
    std::os::unix::fs::PermissionsExt::set_mode(&mut permissions, 0o755);
    fs::set_permissions(&fake_cat, permissions).expect("fake cat should be executable");
    let fake_kill = repo_root.join("bin/kill");
    fs::write(&fake_kill, "#!/bin/sh\nprintf kill-ran > src/kill-ran\n")
        .expect("fake kill should seed");
    let mut permissions = fs::metadata(&fake_kill)
        .expect("fake kill metadata should read")
        .permissions();
    std::os::unix::fs::PermissionsExt::set_mode(&mut permissions, 0o755);
    fs::set_permissions(&fake_kill, permissions).expect("fake kill should be executable");
    let (runtime, rx) = spawn_fake_stateful_server(
        r#"{"decision":"allow","reason_code":"authorized","message":"ok","required_next_action":null}"#,
    );
    write_global_runtime_file(&paths, &runtime).expect("global runtime file should write");
    let inherited_path = format!("{}:/usr/bin:/bin", repo_root.join("bin").display());

    let output = run_stateful_in_repo_with_env(
        &repo_root,
        &paths,
        &[("PATH", inherited_path.as_str())],
        &[
            "mcp",
            "call",
            "state_bash_write",
            r#"{"command":"cat /dev/null > src/allowed.ts","write_targets":["src/allowed.ts"]}"#,
        ],
    );

    assert!(
        output.status.success(),
        "state.bash.write should run with safe PATH: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let request = rx
        .recv_timeout(Duration::from_secs(1))
        .expect("authorization request should arrive");
    assert_eq!(
        request_json_body(&request)["payload"]["path"],
        "src/allowed.ts"
    );
    assert_eq!(
        fs::read_to_string(repo_root.join("src/allowed.ts")).expect("allowed file should read"),
        ""
    );
    assert!(
        !repo_root.join("src/kill-ran").exists(),
        "cleanup kill should not resolve through inherited PATH"
    );

    fs::remove_dir_all(&temp_root).expect("temp root should be removable");
}

#[test]
fn mcp_bash_write_rejects_git_external_diff_before_authorization() {
    let temp_root = temp_root("stateful-mcp-bash-write-git-external-diff");
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
            r#"{"command":"GIT_EXTERNAL_DIFF=python3 git diff --no-index /dev/null src/allowed.ts; printf changed > src/allowed.ts","write_targets":["src/allowed.ts"]}"#,
        ],
    );

    assert!(
        !output.status.success(),
        "GIT_EXTERNAL_DIFF command should fail"
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("detached process"));
    assert!(
        rx.recv_timeout(Duration::from_millis(100)).is_err(),
        "GIT_EXTERNAL_DIFF command should fail before authorization"
    );
    assert_eq!(
        fs::read_to_string(repo_root.join("src/allowed.ts")).expect("allowed file should read"),
        "old\n"
    );

    fs::remove_dir_all(&temp_root).expect("temp root should be removable");
}

#[test]
fn mcp_bash_write_rejects_tar_external_program_before_authorization() {
    let temp_root = temp_root("stateful-mcp-bash-write-tar-external-program");
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
            r#"{"command":"tar -cf /dev/null --use-compress-program python3 src/allowed.ts; printf changed > src/allowed.ts","write_targets":["src/allowed.ts"]}"#,
        ],
    );

    assert!(
        !output.status.success(),
        "tar external program command should fail"
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("detached process"));
    assert!(
        rx.recv_timeout(Duration::from_millis(100)).is_err(),
        "tar external program command should fail before authorization"
    );
    assert_eq!(
        fs::read_to_string(repo_root.join("src/allowed.ts")).expect("allowed file should read"),
        "old\n"
    );

    fs::remove_dir_all(&temp_root).expect("temp root should be removable");
}

#[test]
fn mcp_bash_write_rejects_rsync_remote_shell_before_authorization() {
    let temp_root = temp_root("stateful-mcp-bash-write-rsync-remote-shell");
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
            r#"{"command":"rsync -e python3 /dev/null example.invalid:/tmp/out; printf changed > src/allowed.ts","write_targets":["src/allowed.ts"]}"#,
        ],
    );

    assert!(
        !output.status.success(),
        "rsync remote shell command should fail"
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("detached process"));
    assert!(
        rx.recv_timeout(Duration::from_millis(100)).is_err(),
        "rsync remote shell command should fail before authorization"
    );
    assert_eq!(
        fs::read_to_string(repo_root.join("src/allowed.ts")).expect("allowed file should read"),
        "old\n"
    );

    fs::remove_dir_all(&temp_root).expect("temp root should be removable");
}

#[test]
fn mcp_bash_write_rejects_sqlite_shell_before_authorization() {
    let temp_root = temp_root("stateful-mcp-bash-write-sqlite-shell");
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
            r#"{"command":"sqlite3 ':memory:' '.shell python3'; printf changed > src/allowed.ts","write_targets":["src/allowed.ts"]}"#,
        ],
    );

    assert!(!output.status.success(), "sqlite shell command should fail");
    assert!(String::from_utf8_lossy(&output.stdout).contains("detached process"));
    assert!(
        rx.recv_timeout(Duration::from_millis(100)).is_err(),
        "sqlite shell command should fail before authorization"
    );
    assert_eq!(
        fs::read_to_string(repo_root.join("src/allowed.ts")).expect("allowed file should read"),
        "old\n"
    );

    fs::remove_dir_all(&temp_root).expect("temp root should be removable");
}

#[test]
fn mcp_bash_write_rejects_arch_wrapper_before_authorization() {
    let temp_root = temp_root("stateful-mcp-bash-write-arch-wrapper");
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
            r#"{"command":"arch -arm64 sh -c 'printf late > src/allowed.ts'; printf changed > src/allowed.ts","write_targets":["src/allowed.ts"]}"#,
        ],
    );

    assert!(!output.status.success(), "arch wrapper command should fail");
    assert!(String::from_utf8_lossy(&output.stdout).contains("detached process"));
    assert!(
        rx.recv_timeout(Duration::from_millis(100)).is_err(),
        "arch wrapper command should fail before authorization"
    );
    assert_eq!(
        fs::read_to_string(repo_root.join("src/allowed.ts")).expect("allowed file should read"),
        "old\n"
    );

    fs::remove_dir_all(&temp_root).expect("temp root should be removable");
}

#[test]
fn mcp_bash_write_rejects_tcl_entrypoint_before_authorization() {
    let temp_root = temp_root("stateful-mcp-bash-write-tcl-entrypoint");
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
            r#"{"command":"printf 'exec python3' | tclsh; printf changed > src/allowed.ts","write_targets":["src/allowed.ts"]}"#,
        ],
    );

    assert!(!output.status.success(), "tcl entrypoint should fail");
    assert!(String::from_utf8_lossy(&output.stdout).contains("detached process"));
    assert!(
        rx.recv_timeout(Duration::from_millis(100)).is_err(),
        "tcl entrypoint should fail before authorization"
    );
    assert_eq!(
        fs::read_to_string(repo_root.join("src/allowed.ts")).expect("allowed file should read"),
        "old\n"
    );

    fs::remove_dir_all(&temp_root).expect("temp root should be removable");
}

#[test]
fn mcp_bash_write_rejects_command_prefix_wrappers_before_authorization() {
    let temp_root = temp_root("stateful-mcp-bash-write-prefix-wrapper");
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
            r#"{"command":"stdbuf -o0 sh -c 'printf late > src/allowed.ts'; printf changed > src/allowed.ts","write_targets":["src/allowed.ts"]}"#,
        ],
    );

    assert!(
        !output.status.success(),
        "command prefix wrapper should fail"
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("detached process"));
    assert!(
        rx.recv_timeout(Duration::from_millis(100)).is_err(),
        "command prefix wrapper should fail before authorization"
    );
    assert_eq!(
        fs::read_to_string(repo_root.join("src/allowed.ts")).expect("allowed file should read"),
        "old\n"
    );

    fs::remove_dir_all(&temp_root).expect("temp root should be removable");
}

#[test]
fn mcp_bash_write_rejects_shell_compound_detached_processes_before_authorization() {
    let temp_root = temp_root("stateful-mcp-bash-write-compound-detached");
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
            r#"{"command":"if true; then python3 -c 'import os; getattr(os, \"set\" + \"sid\")()'; fi; printf changed > src/allowed.ts","write_targets":["src/allowed.ts"]}"#,
        ],
    );

    assert!(
        !output.status.success(),
        "shell compound detached construct should fail"
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("detached process"));
    assert!(
        rx.recv_timeout(Duration::from_millis(100)).is_err(),
        "shell compound detached construct should fail before authorization"
    );
    assert_eq!(
        fs::read_to_string(repo_root.join("src/allowed.ts")).expect("allowed file should read"),
        "old\n"
    );

    fs::remove_dir_all(&temp_root).expect("temp root should be removable");
}

#[test]
fn mcp_bash_write_rejects_find_exec_before_authorization() {
    let temp_root = temp_root("stateful-mcp-bash-write-find-exec-detached");
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
            r#"{"command":"find . -maxdepth 0 -exec python3 -c 'import os; getattr(os, \"set\" + \"sid\")()' \\; ; printf changed > src/allowed.ts","write_targets":["src/allowed.ts"]}"#,
        ],
    );

    assert!(!output.status.success(), "find -exec construct should fail");
    assert!(String::from_utf8_lossy(&output.stdout).contains("detached process"));
    assert!(
        rx.recv_timeout(Duration::from_millis(100)).is_err(),
        "find -exec construct should fail before authorization"
    );
    assert_eq!(
        fs::read_to_string(repo_root.join("src/allowed.ts")).expect("allowed file should read"),
        "old\n"
    );

    fs::remove_dir_all(&temp_root).expect("temp root should be removable");
}

#[test]
fn mcp_bash_write_rejects_ansi_c_quoted_find_exec_before_authorization() {
    let temp_root = temp_root("stateful-mcp-bash-write-ansi-c-find-exec");
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
            r#"{"command":"find . -maxdepth 0 -e$'xec' sh -c 'printf late > src/allowed.ts' ';'; printf changed > src/allowed.ts","write_targets":["src/allowed.ts"]}"#,
        ],
    );

    assert!(
        !output.status.success(),
        "ANSI-C quoted find -exec should fail"
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("detached process"));
    assert!(
        rx.recv_timeout(Duration::from_millis(100)).is_err(),
        "ANSI-C quoted find -exec should fail before authorization"
    );
    assert_eq!(
        fs::read_to_string(repo_root.join("src/allowed.ts")).expect("allowed file should read"),
        "old\n"
    );

    fs::remove_dir_all(&temp_root).expect("temp root should be removable");
}

#[test]
fn mcp_bash_write_rejects_escaped_newline_find_exec_before_authorization() {
    let temp_root = temp_root("stateful-mcp-bash-write-escaped-newline-find-exec");
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
            r#"{"command":"find . -maxdepth 0 -e\\\nxec sh -c 'printf late > src/allowed.ts' ';'; printf changed > src/allowed.ts","write_targets":["src/allowed.ts"]}"#,
        ],
    );

    assert!(
        !output.status.success(),
        "escaped-newline find -exec should fail"
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("detached process"));
    assert!(
        rx.recv_timeout(Duration::from_millis(100)).is_err(),
        "escaped-newline find -exec should fail before authorization"
    );
    assert_eq!(
        fs::read_to_string(repo_root.join("src/allowed.ts")).expect("allowed file should read"),
        "old\n"
    );

    fs::remove_dir_all(&temp_root).expect("temp root should be removable");
}

#[test]
fn mcp_bash_write_rejects_hard_link_retarget_before_authorization() {
    let temp_root = temp_root("stateful-mcp-bash-write-hardlink-retarget");
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
            r#"{"command":"rm src/allowed.ts; ln .git/config src/allowed.ts; printf broken > src/allowed.ts","write_targets":["src/allowed.ts"]}"#,
        ],
    );

    assert!(!output.status.success(), "hard-link retarget should fail");
    assert!(String::from_utf8_lossy(&output.stdout).contains("detached process"));
    assert!(
        rx.recv_timeout(Duration::from_millis(100)).is_err(),
        "hard-link retarget should fail before authorization"
    );
    assert_eq!(
        fs::read_to_string(repo_root.join("src/allowed.ts")).expect("allowed file should read"),
        "old\n"
    );

    fs::remove_dir_all(&temp_root).expect("temp root should be removable");
}

#[cfg(unix)]
#[test]
fn mcp_bash_write_rejects_preexisting_hard_link_target_before_authorization() {
    let temp_root = temp_root("stateful-mcp-bash-write-preexisting-hardlink");
    let paths = GlobalPaths::new(temp_root.join("home"));
    let repo_root = temp_root.join("repo");
    fs::create_dir_all(repo_root.join("src")).expect("repo src should be creatable");
    enable_test_repo(&paths, &repo_root);
    write_current_session_file(&repo_root, &CurrentSession::new("s-current", "w1"))
        .expect("current session should write");
    fs::create_dir_all(repo_root.join(".git")).expect("git dir should be creatable");
    let git_config = repo_root.join(".git/config");
    fs::write(&git_config, "safe\n").expect("git config should seed");
    fs::hard_link(&git_config, repo_root.join("src/allowed.ts"))
        .expect("hard-linked target should be creatable");
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
            r#"{"command":"printf broken > src/allowed.ts","write_targets":["src/allowed.ts"]}"#,
        ],
    );

    assert!(!output.status.success(), "hard-linked target should fail");
    assert!(String::from_utf8_lossy(&output.stdout).contains("hard-linked"));
    assert!(
        rx.recv_timeout(Duration::from_millis(100)).is_err(),
        "hard-linked target should fail before authorization"
    );
    assert_eq!(
        fs::read_to_string(git_config).expect("git config should read"),
        "safe\n"
    );

    fs::remove_dir_all(&temp_root).expect("temp root should be removable");
}

#[test]
fn mcp_bash_write_rejects_cp_hard_link_retarget_before_authorization() {
    let temp_root = temp_root("stateful-mcp-bash-write-cp-hardlink-retarget");
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
            r#"{"command":"rm src/allowed.ts; cp -l .git/config src/allowed.ts; printf broken > src/allowed.ts","write_targets":["src/allowed.ts"]}"#,
        ],
    );

    assert!(
        !output.status.success(),
        "cp hard-link retarget should fail"
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("detached process"));
    assert!(
        rx.recv_timeout(Duration::from_millis(100)).is_err(),
        "cp hard-link retarget should fail before authorization"
    );
    assert_eq!(
        fs::read_to_string(repo_root.join("src/allowed.ts")).expect("allowed file should read"),
        "old\n"
    );

    fs::remove_dir_all(&temp_root).expect("temp root should be removable");
}

#[test]
fn mcp_bash_write_rejects_globbed_find_exec_before_authorization() {
    let temp_root = temp_root("stateful-mcp-bash-write-globbed-find-exec");
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
            r#"{"command":"find . -maxdepth 0 -* sh -c 'printf late > src/allowed.ts' ';'; printf changed > src/allowed.ts","write_targets":["src/allowed.ts"],"create_targets":["-exec"]}"#,
        ],
    );

    assert!(!output.status.success(), "globbed find -exec should fail");
    assert!(String::from_utf8_lossy(&output.stdout).contains("detached process"));
    assert!(
        rx.recv_timeout(Duration::from_millis(100)).is_err(),
        "globbed find -exec should fail before authorization"
    );
    assert_eq!(
        fs::read_to_string(repo_root.join("src/allowed.ts")).expect("allowed file should read"),
        "old\n"
    );

    fs::remove_dir_all(&temp_root).expect("temp root should be removable");
}

#[test]
fn mcp_bash_write_rejects_assignment_assembled_find_exec_before_authorization() {
    let temp_root = temp_root("stateful-mcp-bash-write-assignment-find-exec");
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
            r#"{"command":"E=-exec; find . -maxdepth 0 $E sh -c 'printf late > src/allowed.ts' ';'; printf changed > src/allowed.ts","write_targets":["src/allowed.ts"]}"#,
        ],
    );

    assert!(
        !output.status.success(),
        "assignment-assembled find -exec should fail"
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("detached process"));
    assert!(
        rx.recv_timeout(Duration::from_millis(100)).is_err(),
        "assignment-assembled find -exec should fail before authorization"
    );
    assert_eq!(
        fs::read_to_string(repo_root.join("src/allowed.ts")).expect("allowed file should read"),
        "old\n"
    );

    fs::remove_dir_all(&temp_root).expect("temp root should be removable");
}

#[test]
fn mcp_bash_write_rejects_parameter_assembled_find_exec_before_authorization() {
    let temp_root = temp_root("stateful-mcp-bash-write-parameter-find-exec");
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
            r#"{"command":"find . -maxdepth 0 -ex${STATEFUL_MISSING}ec sh -c 'printf late > src/allowed.ts' ';'; printf changed > src/allowed.ts","write_targets":["src/allowed.ts"]}"#,
        ],
    );

    assert!(
        !output.status.success(),
        "parameter-assembled find -exec should fail"
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("detached process"));
    assert!(
        rx.recv_timeout(Duration::from_millis(100)).is_err(),
        "parameter-assembled find -exec should fail before authorization"
    );
    assert_eq!(
        fs::read_to_string(repo_root.join("src/allowed.ts")).expect("allowed file should read"),
        "old\n"
    );

    fs::remove_dir_all(&temp_root).expect("temp root should be removable");
}

#[test]
fn mcp_bash_write_allows_regular_shell_variable_expansion_to_reach_authorization() {
    let temp_root = temp_root("stateful-mcp-bash-write-shell-variable");
    let paths = GlobalPaths::new(temp_root.join("home"));
    let repo_root = temp_root.join("repo");
    fs::create_dir_all(repo_root.join("src")).expect("repo src should be creatable");
    enable_test_repo(&paths, &repo_root);
    write_current_session_file(&repo_root, &CurrentSession::new("s-current", "w1"))
        .expect("current session should write");
    fs::write(repo_root.join("src/allowed.ts"), "old\n").expect("allowed file should seed");
    let (runtime, rx) = spawn_fake_stateful_server(
        r#"{"decision":"deny","reason_code":"scope_mismatch","message":"Target is outside active intent scope.","required_next_action":"Declare matching intent."}"#,
    );
    write_global_runtime_file(&paths, &runtime).expect("global runtime file should write");

    let output = run_stateful_in_repo(
        &repo_root,
        &paths,
        &[
            "mcp",
            "call",
            "state_bash_write",
            r#"{"command":"printf \"%s\" \"$PWD\" > src/allowed.ts","write_targets":["src/allowed.ts"]}"#,
        ],
    );

    assert!(
        !output.status.success(),
        "denied authorization should fail after command validation"
    );
    let request = rx
        .recv_timeout(Duration::from_secs(1))
        .expect("authorize request should arrive");
    assert_eq!(
        request_json_body(&request)["payload"]["path"],
        "src/allowed.ts"
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("target authorization denied"));
    assert!(!stdout.contains("detached process"));
    assert_eq!(
        fs::read_to_string(repo_root.join("src/allowed.ts")).expect("allowed file should read"),
        "old\n"
    );

    fs::remove_dir_all(&temp_root).expect("temp root should be removable");
}

#[test]
fn mcp_bash_write_preserves_whitespace_in_write_targets() {
    let temp_root = temp_root("stateful-mcp-bash-write-whitespace-target");
    let paths = GlobalPaths::new(temp_root.join("home"));
    let repo_root = temp_root.join("repo");
    fs::create_dir_all(repo_root.join("src")).expect("repo src should be creatable");
    enable_test_repo(&paths, &repo_root);
    write_current_session_file(&repo_root, &CurrentSession::new("s-current", "w1"))
        .expect("current session should write");
    fs::write(repo_root.join("src/report.ts "), "old\n").expect("spaced file should seed");
    let (runtime, rx) = spawn_fake_stateful_server(
        r#"{"decision":"deny","reason_code":"scope_mismatch","message":"Target is outside active intent scope.","required_next_action":"Declare matching intent."}"#,
    );
    write_global_runtime_file(&paths, &runtime).expect("global runtime file should write");

    let output = run_stateful_in_repo(
        &repo_root,
        &paths,
        &[
            "mcp",
            "call",
            "state_bash_write",
            r#"{"command":"printf changed > \"src/report.ts \"","write_targets":["src/report.ts "]}"#,
        ],
    );

    assert!(
        !output.status.success(),
        "denied authorization should fail after target validation"
    );
    let request = rx
        .recv_timeout(Duration::from_secs(1))
        .expect("authorize request should arrive");
    assert_eq!(
        request_json_body(&request)["payload"]["path"],
        "src/report.ts "
    );

    fs::remove_dir_all(&temp_root).expect("temp root should be removable");
}

#[test]
fn mcp_bash_write_rejects_removed_wait_argument() {
    let temp_root = temp_root("stateful-mcp-bash-write-removed-wait");
    let paths = GlobalPaths::new(temp_root.join("home"));
    let repo_root = temp_root.join("repo");
    fs::create_dir_all(repo_root.join("src")).expect("repo src should be creatable");
    enable_test_repo(&paths, &repo_root);
    write_current_session_file(&repo_root, &CurrentSession::new("s-current", "w1"))
        .expect("current session should write");
    let (runtime, _rx) = spawn_fake_stateful_server(r#"{"status":"ok"}"#);
    write_global_runtime_file(&paths, &runtime).expect("global runtime file should write");

    let output = run_stateful_in_repo(
        &repo_root,
        &paths,
        &[
            "mcp",
            "call",
            "state_bash_write",
            r#"{"command":"true","write_targets":["src/allowed.ts"],"mcp_wait_ms":1}"#,
        ],
    );

    assert!(
        !output.status.success(),
        "removed wait argument should fail"
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("invalid state.bash.write arguments"));
    assert!(stdout.contains("mcp_wait_ms"));

    fs::remove_dir_all(&temp_root).expect("temp root should be removable");
}

#[test]
fn mcp_bash_write_rejects_case_insensitive_git_internals() {
    let temp_root = temp_root("stateful-mcp-bash-write-git-internals-case");
    let paths = GlobalPaths::new(temp_root.join("home"));
    let repo_root = temp_root.join("repo");
    fs::create_dir_all(&repo_root).expect("repo root should be creatable");
    enable_test_repo(&paths, &repo_root);
    write_current_session_file(&repo_root, &CurrentSession::new("s-current", "w1"))
        .expect("current session should write");
    let (runtime, _rx) = spawn_fake_stateful_server(r#"{"status":"ok"}"#);
    write_global_runtime_file(&paths, &runtime).expect("global runtime file should write");

    let output = run_stateful_in_repo(
        &repo_root,
        &paths,
        &[
            "mcp",
            "call",
            "state_bash_write",
            r#"{"command":"printf broken > .Git/config","write_targets":[".Git/config"]}"#,
        ],
    );

    assert!(!output.status.success(), "Git internals target should fail");
    assert!(String::from_utf8_lossy(&output.stdout).contains("Git internals"));

    fs::remove_dir_all(&temp_root).expect("temp root should be removable");
}

#[test]
fn mcp_bash_write_rejects_detached_process_constructs() {
    let temp_root = temp_root("stateful-mcp-bash-write-detached-process");
    let paths = GlobalPaths::new(temp_root.join("home"));
    let repo_root = temp_root.join("repo");
    fs::create_dir_all(repo_root.join("src")).expect("repo src should be creatable");
    enable_test_repo(&paths, &repo_root);
    write_current_session_file(&repo_root, &CurrentSession::new("s-current", "w1"))
        .expect("current session should write");
    let (runtime, _rx) = spawn_fake_stateful_server(r#"{"status":"ok"}"#);
    write_global_runtime_file(&paths, &runtime).expect("global runtime file should write");

    let output = run_stateful_in_repo(
        &repo_root,
        &paths,
        &[
            "mcp",
            "call",
            "state_bash_write",
            r#"{"command":"nice python3 -c 'import os; getattr(os, \"set\" + \"sid\")()'","write_targets":["src/allowed.ts"]}"#,
        ],
    );

    assert!(
        !output.status.success(),
        "detached process construct should fail"
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("detached process"));

    fs::remove_dir_all(&temp_root).expect("temp root should be removable");
}

#[cfg(target_os = "macos")]
#[test]
fn mcp_bash_write_allows_daemon_in_authorized_filename() {
    let temp_root = temp_root("stateful-mcp-bash-write-daemon-filename");
    let paths = GlobalPaths::new(temp_root.join("home"));
    let repo_root = temp_root.join("repo");
    fs::create_dir_all(repo_root.join("src")).expect("repo src should be creatable");
    enable_test_repo(&paths, &repo_root);
    write_current_session_file(&repo_root, &CurrentSession::new("s-current", "w1"))
        .expect("current session should write");
    let (runtime, _rx) = spawn_fake_stateful_server_sequence(vec![
        r#"{"decision":"allow","reason_code":"authorized","message":"ok","required_next_action":null}"#,
        r#"{"decision":"allow","reason_code":"authorized","message":"ok","required_next_action":null}"#,
    ]);
    write_global_runtime_file(&paths, &runtime).expect("global runtime file should write");

    let output = run_stateful_in_repo(
        &repo_root,
        &paths,
        &[
            "mcp",
            "call",
            "state_bash_write",
            r#"{"command":"printf changed > src/daemon.ts","write_targets":["src/daemon.ts"],"create_targets":["src/daemon.ts"]}"#,
        ],
    );

    assert!(
        output.status.success(),
        "daemon filename should be allowed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        fs::read_to_string(repo_root.join("src/daemon.ts")).expect("daemon file should read"),
        "changed"
    );

    fs::remove_dir_all(&temp_root).expect("temp root should be removable");
}

#[test]
fn mcp_bash_write_rejects_case_insensitive_git_internals_as_cwd() {
    let temp_root = temp_root("stateful-mcp-bash-write-git-internals-cwd-case");
    let paths = GlobalPaths::new(temp_root.join("home"));
    let repo_root = temp_root.join("repo");
    fs::create_dir_all(repo_root.join("src")).expect("repo src should be creatable");
    fs::create_dir_all(repo_root.join(".Git")).expect("case variant git dir should write");
    fs::write(repo_root.join("src/allowed.ts"), "old\n").expect("allowed file should seed");
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
            r#"{"cwd":".Git","command":"printf changed > ../src/allowed.ts","write_targets":["src/allowed.ts"]}"#,
        ],
    );

    assert!(!output.status.success(), "Git internals cwd should fail");
    assert!(String::from_utf8_lossy(&output.stdout).contains("Git internals"));

    fs::remove_dir_all(&temp_root).expect("temp root should be removable");
}

#[cfg(unix)]
#[test]
fn mcp_bash_write_refuses_untrusted_current_session_file_before_authorization() {
    let temp_root = temp_root("stateful-mcp-bash-write-session-symlink");
    let paths = GlobalPaths::new(temp_root.join("home"));
    let repo_root = temp_root.join("repo");
    fs::create_dir_all(repo_root.join("src")).expect("repo src should be creatable");
    fs::create_dir_all(repo_root.join(".stateful_core/runtime"))
        .expect("runtime dir should be creatable");
    enable_test_repo(&paths, &repo_root);
    fs::write(repo_root.join("src/allowed.ts"), "old\n").expect("allowed file should seed");
    let victim = temp_root.join("victim-session.json");
    fs::write(&victim, r#"{"session_id":"s-current","workspace_id":"w1"}"#)
        .expect("victim session should write");
    std::os::unix::fs::symlink(
        &victim,
        repo_root.join(".stateful_core/runtime/session.json"),
    )
    .expect("current session symlink should create");
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
            r#"{"session_id":"s-other","workspace_id":"w1","command":"printf changed > src/allowed.ts","write_targets":["src/allowed.ts"]}"#,
        ],
    );

    assert!(
        !output.status.success(),
        "untrusted current session should fail"
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("symlinked current session file"));
    assert!(
        rx.recv_timeout(Duration::from_millis(100)).is_err(),
        "untrusted current session should fail before authorization"
    );
    assert_eq!(
        fs::read_to_string(repo_root.join("src/allowed.ts")).expect("allowed file should read"),
        "old\n"
    );

    fs::remove_dir_all(&temp_root).expect("temp root should be removable");
}

#[test]
fn mcp_bash_write_rejects_invalid_cwd_before_authorization() {
    let temp_root = temp_root("stateful-mcp-bash-write-invalid-cwd");
    let paths = GlobalPaths::new(temp_root.join("home"));
    let repo_root = temp_root.join("repo");
    fs::create_dir_all(repo_root.join("src")).expect("repo src should be creatable");
    fs::write(repo_root.join("src/allowed.ts"), "old\n").expect("allowed file should seed");
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
            "state_bash_write",
            r#"{"cwd":"does-not-exist","command":"printf changed > src/allowed.ts","write_targets":["src/allowed.ts"]}"#,
        ],
    );

    assert!(!output.status.success(), "invalid cwd should fail");
    assert!(String::from_utf8_lossy(&output.stdout).contains("cwd must exist"));
    assert!(
        rx.recv_timeout(Duration::from_millis(100)).is_err(),
        "invalid cwd should fail before authorization"
    );

    fs::remove_dir_all(&temp_root).expect("temp root should be removable");
}

#[test]
fn mcp_bash_write_rejects_missing_write_target_before_authorization() {
    let temp_root = temp_root("stateful-mcp-bash-write-missing-target");
    let paths = GlobalPaths::new(temp_root.join("home"));
    let repo_root = temp_root.join("repo");
    fs::create_dir_all(repo_root.join("src")).expect("repo src should be creatable");
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
            "state_bash_write",
            r#"{"command":"true","write_targets":["src/missing.ts"]}"#,
        ],
    );

    assert!(!output.status.success(), "missing write target should fail");
    assert!(String::from_utf8_lossy(&output.stdout).contains("must already exist"));
    assert!(
        rx.recv_timeout(Duration::from_millis(100)).is_err(),
        "missing write target should fail before authorization"
    );

    fs::remove_dir_all(&temp_root).expect("temp root should be removable");
}

#[test]
fn mcp_bash_write_rejects_unbounded_timeout() {
    let temp_root = temp_root("stateful-mcp-bash-write-timeout-bound");
    let paths = GlobalPaths::new(temp_root.join("home"));
    let repo_root = temp_root.join("repo");
    fs::create_dir_all(repo_root.join("src")).expect("repo src should be creatable");
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
            "state_bash_write",
            r#"{"command":"true","write_targets":["src/allowed.ts"],"timeout_seconds":18446744073709551615}"#,
        ],
    );

    assert!(!output.status.success(), "unbounded timeout should fail");
    assert!(String::from_utf8_lossy(&output.stdout).contains("timeout_seconds"));
    assert!(
        rx.recv_timeout(Duration::from_millis(100)).is_err(),
        "invalid timeout should fail before authorization"
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

#[cfg(target_os = "macos")]
#[test]
fn mcp_bash_write_waits_for_command_completion_and_returns_result() {
    let temp_root = temp_root("stateful-mcp-bash-write-waits");
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
            r#"{"command":"sleep 0.05; printf changed > src/allowed.ts","write_targets":["src/allowed.ts"]}"#,
        ],
    );

    assert!(
        output.status.success(),
        "stateful mcp call failed: {}",
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
        "changed"
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains(r#""status":"exited""#));
    assert!(stdout.contains(r#""exit_code":0"#));

    fs::remove_dir_all(&temp_root).expect("temp root should be removable");
}

#[cfg(target_os = "macos")]
#[test]
fn mcp_bash_write_rejects_background_child_constructs() {
    let temp_root = temp_root("stateful-mcp-bash-write-background-child");
    let paths = GlobalPaths::new(temp_root.join("home"));
    let repo_root = temp_root.join("repo");
    fs::create_dir_all(repo_root.join("src")).expect("repo src should be creatable");
    enable_test_repo(&paths, &repo_root);
    write_current_session_file(&repo_root, &CurrentSession::new("s-current", "w1"))
        .expect("current session should write");
    fs::write(repo_root.join("src/allowed.ts"), "old\n").expect("allowed file should seed");
    let (runtime, _rx) = spawn_fake_stateful_server(r#"{"status":"ok"}"#);
    write_global_runtime_file(&paths, &runtime).expect("global runtime file should write");

    let output = run_stateful_in_repo(
        &repo_root,
        &paths,
        &[
            "mcp",
            "call",
            "state_bash_write",
            r#"{"command":"sleep 5 & printf changed > src/allowed.ts","write_targets":["src/allowed.ts"],"timeout_seconds":1}"#,
        ],
    );

    assert!(!output.status.success(), "background construct should fail");
    assert!(String::from_utf8_lossy(&output.stdout).contains("detached process"));

    fs::remove_dir_all(&temp_root).expect("temp root should be removable");
}

#[cfg(target_os = "macos")]
#[test]
fn mcp_bash_write_truncates_large_stdout() {
    let temp_root = temp_root("stateful-mcp-bash-write-large-stdout");
    let paths = GlobalPaths::new(temp_root.join("home"));
    let repo_root = temp_root.join("repo");
    fs::create_dir_all(repo_root.join("src")).expect("repo src should be creatable");
    enable_test_repo(&paths, &repo_root);
    write_current_session_file(&repo_root, &CurrentSession::new("s-current", "w1"))
        .expect("current session should write");
    fs::write(repo_root.join("src/allowed.ts"), "old\n").expect("allowed file should seed");
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
            r#"{"command":"yes x | head -c 1200000","write_targets":["src/allowed.ts"]}"#,
        ],
    );

    assert!(
        output.status.success(),
        "large stdout command should return: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains(r#""stdout_truncated":true"#));
    assert!(stdout.contains(r#""stderr_truncated":false"#));

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
    assert!(tools.iter().any(|tool| tool["name"] == "state_bash_write"));
    for removed in [
        "state_activity_observe",
        "state_activity_finalize",
        "state_current_read",
        "state_events_read",
        "state_context_render",
        "state_validation_run",
        "state_file_write",
    ] {
        assert!(
            !tools.iter().any(|tool| tool["name"] == removed),
            "{removed} should not be listed"
        );
    }
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
            "name":"state_session_heartbeat",
            "arguments":{"session_id":"s1","workspace_id":"w1"}
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
    assert!(
        json["result"]["content"][0]["text"]
            .as_str()
            .unwrap_or_default()
            .contains("does not resolve active lease or reservation conflicts")
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
    spawn_fake_stateful_server_sequence_with_delay(
        responses
            .into_iter()
            .map(|response| (response, Duration::ZERO))
            .collect(),
    )
}

fn spawn_fake_stateful_server_sequence_with_delay(
    responses: Vec<(&'static str, Duration)>,
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
                let (response, delay) = responses
                    .next()
                    .unwrap_or((r#"{"status":"ok"}"#, Duration::ZERO));
                thread::sleep(delay);
                write_json_response(&mut stream, response);
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
    let message = serde_json::to_string(
        &serde_json::from_str::<serde_json::Value>(message).expect("message should be json"),
    )
    .expect("message should serialize");
    run_mcp_jsonrpc_lines_in_repo(repo_root, paths, &format!("{message}\n"))
}

fn run_mcp_jsonrpc_lines_in_repo(
    repo_root: &std::path::Path,
    paths: &GlobalPaths,
    messages: &str,
) -> String {
    let mut child = spawn_mcp_server(repo_root, paths);
    let mut stdin = child.stdin.take().expect("stdin should be piped");
    stdin
        .write_all(messages.as_bytes())
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

fn spawn_mcp_server(repo_root: &std::path::Path, paths: &GlobalPaths) -> std::process::Child {
    Command::new(env!("CARGO_BIN_EXE_stateful"))
        .args(["mcp", "serve"])
        .current_dir(repo_root)
        .env_clear()
        .env("STATEFUL_HOME", &paths.home)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("stateful mcp serve should spawn")
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
    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n{}",
        body.len(),
        body
    );
    stream
        .write_all(response.as_bytes())
        .expect("response should write");
}
