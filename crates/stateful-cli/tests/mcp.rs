use std::{
    fs,
    io::{Read, Write},
    net::TcpListener,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::mpsc,
    thread,
    time::Duration,
};

use stateful_cli::{
    CurrentSession, GlobalPaths, ServerRuntime, enable_repo, handle_mcp_jsonrpc_in_repo,
    serve_mcp_stdio_in_repo, write_current_session_file, write_current_session_file_for_session,
    write_global_runtime_file,
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
    let request = rx
        .recv_timeout(Duration::from_secs(5))
        .expect("captured request should arrive");
    assert!(request.contains("GET /v1/current HTTP/1.1"));
    assert!(request.contains("Authorization: Bearer secret-token"));

    fs::remove_dir_all(&temp_root).expect("temp root should be removable");
}

#[test]
fn mcp_call_env_runtime_skips_managed_server_ensure() {
    let temp_root = temp_root("stateful-mcp-env-runtime-skip-ensure");
    let paths = GlobalPaths::new(temp_root.join("home"));
    let repo_root = temp_root.join("repo");
    fs::create_dir_all(&repo_root).expect("repo root should be creatable");
    enable_test_repo(&paths, &repo_root);
    fs::create_dir_all(&paths.runtime_dir).expect("runtime dir should be creatable");
    fs::write(&paths.server_lock, "other-process").expect("server lock should be writable");
    let (runtime, rx) = spawn_fake_stateful_server(r#"{"status":"ok","current":{"source":"env"}}"#);

    let output = run_stateful_in_repo_with_env(
        &repo_root,
        &paths,
        &[
            ("STATEFUL_SERVER_URL", runtime.base_url.as_str()),
            ("STATEFUL_SERVER_TOKEN", runtime.token.as_str()),
        ],
        &["mcp", "call", "state.current.read"],
    );

    assert!(
        output.status.success(),
        "env runtime mcp call should skip managed ensure: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("\"source\":\"env\""));
    let request = rx.recv().expect("captured request should arrive");
    assert!(request.contains("GET /v1/current HTTP/1.1"));

    fs::remove_dir_all(&temp_root).expect("temp root should be removable");
}

#[test]
fn mcp_session_register_bootstraps_current_codex_thread_session() {
    let temp_root = temp_root("stateful-mcp-session-register-bootstrap");
    let paths = GlobalPaths::new(temp_root.join("home"));
    let repo_root = temp_root.join("repo");
    fs::create_dir_all(&repo_root).expect("repo root should be creatable");
    enable_test_repo(&paths, &repo_root);
    let (runtime, rx) = spawn_fake_stateful_server(r#"{"status":"ok"}"#);
    write_global_runtime_file(&paths, &runtime).expect("global runtime file should write");

    let response = run_mcp_jsonrpc_in_repo_with_env(
        &repo_root,
        &paths,
        &[("CODEX_THREAD_ID", "thread-a")],
        r#"{
          "jsonrpc":"2.0",
          "id":16,
          "method":"tools/call",
          "params":{
            "name":"state_session_register",
            "arguments":{}
          }
        }"#,
    );

    let request = rx
        .recv_timeout(Duration::from_secs(1))
        .expect("session register request should arrive");
    assert!(request.contains("POST /v1/session/register HTTP/1.1"));
    let body = request_json_body(&request);
    assert_eq!(body["session_id"], "thread-a");
    assert_eq!(body["workspace_id"], "w1");

    let session_file = repo_root
        .join(".stateful_core")
        .join("runtime")
        .join("sessions")
        .join("thread-a.json");
    assert!(
        session_file.is_file(),
        "session register should create a session-bound current session file"
    );
    let current_session: CurrentSession = serde_json::from_str(
        &fs::read_to_string(
            repo_root
                .join(".stateful_core")
                .join("runtime")
                .join("session.json"),
        )
        .expect("current session alias should be readable"),
    )
    .expect("current session alias should be json");
    assert_eq!(current_session, CurrentSession::new("thread-a", "w1"));

    let json: serde_json::Value = serde_json::from_str(&response).expect("response should be json");
    assert_eq!(json["jsonrpc"], "2.0");
    assert_eq!(json["id"], 16);
    assert_eq!(json["result"]["isError"], false);

    fs::remove_dir_all(&temp_root).expect("temp root should be removable");
}

#[test]
fn mcp_session_register_prefers_verified_legacy_session_for_codex_run_when_thread_env_is_absent() {
    let temp_root = temp_root("stateful-mcp-session-register-codex-run-legacy");
    let paths = GlobalPaths::new(temp_root.join("home"));
    let repo_root = temp_root.join("repo");
    fs::create_dir_all(&repo_root).expect("repo root should be creatable");
    enable_test_repo(&paths, &repo_root);
    let (runtime, rx) = spawn_fake_stateful_server(r#"{"status":"ok"}"#);
    write_global_runtime_file(&paths, &runtime).expect("global runtime file should write");
    write_current_session_file_for_session(
        &repo_root,
        "session-a",
        &CurrentSession::new("session-a", "workspace-a"),
    )
    .expect("stateful env session-bound current session should write");
    write_current_session_file_for_session(
        &repo_root,
        "thread-a",
        &CurrentSession::new("thread-a", "workspace-thread"),
    )
    .expect("codex thread session-bound current session should write");
    write_current_session_file(
        &repo_root,
        &CurrentSession::new("thread-a", "workspace-thread"),
    )
    .expect("verified legacy current session alias should write");

    let response = run_mcp_jsonrpc_in_repo_with_env(
        &repo_root,
        &paths,
        &[
            ("STATEFUL_SESSION_ID", "session-a"),
            ("STATEFUL_CODEX_RUN_ID", "session-a"),
        ],
        r#"{
          "jsonrpc":"2.0",
          "id":18,
          "method":"tools/call",
          "params":{
            "name":"state_session_register",
            "arguments":{}
          }
        }"#,
    );

    let request = rx
        .recv_timeout(Duration::from_secs(1))
        .expect("session register request should arrive");
    assert!(request.contains("POST /v1/session/register HTTP/1.1"));
    let body = request_json_body(&request);
    assert_eq!(body["session_id"], "thread-a");
    assert_eq!(body["workspace_id"], "workspace-thread");
    let current_session: CurrentSession = serde_json::from_str(
        &fs::read_to_string(
            repo_root
                .join(".stateful_core")
                .join("runtime")
                .join("session.json"),
        )
        .expect("current session alias should be readable"),
    )
    .expect("current session alias should be json");
    assert_eq!(
        current_session,
        CurrentSession::new("thread-a", "workspace-thread")
    );

    let json: serde_json::Value = serde_json::from_str(&response).expect("response should be json");
    assert_eq!(json["jsonrpc"], "2.0");
    assert_eq!(json["id"], 18);
    assert_eq!(json["result"]["isError"], false);

    fs::remove_dir_all(&temp_root).expect("temp root should be removable");
}

#[test]
fn mcp_session_register_refreshes_stale_current_session_alias() {
    let temp_root = temp_root("stateful-mcp-session-register-refresh-stale");
    let paths = GlobalPaths::new(temp_root.join("home"));
    let repo_root = temp_root.join("repo");
    fs::create_dir_all(&repo_root).expect("repo root should be creatable");
    enable_test_repo(&paths, &repo_root);
    write_current_session_file_for_session(
        &repo_root,
        "parent-session",
        &CurrentSession::new("parent-session", "w-parent"),
    )
    .expect("parent session-bound file should write");
    write_current_session_file(
        &repo_root,
        &CurrentSession::new("parent-session", "w-parent"),
    )
    .expect("stale current session alias should write");
    let (runtime, rx) = spawn_fake_stateful_server(r#"{"status":"ok"}"#);
    write_global_runtime_file(&paths, &runtime).expect("global runtime file should write");

    let response = run_mcp_jsonrpc_in_repo_with_env(
        &repo_root,
        &paths,
        &[("CODEX_THREAD_ID", "thread-a")],
        r#"{
          "jsonrpc":"2.0",
          "id":17,
          "method":"tools/call",
          "params":{
            "name":"state_session_register",
            "arguments":{}
          }
        }"#,
    );

    let request = rx
        .recv_timeout(Duration::from_secs(1))
        .expect("session register request should arrive");
    let body = request_json_body(&request);
    assert_eq!(body["session_id"], "thread-a");
    assert_eq!(body["workspace_id"], "w1");
    let current_session: CurrentSession = serde_json::from_str(
        &fs::read_to_string(
            repo_root
                .join(".stateful_core")
                .join("runtime")
                .join("session.json"),
        )
        .expect("current session alias should be readable"),
    )
    .expect("current session alias should be json");
    assert_eq!(current_session, CurrentSession::new("thread-a", "w1"));

    let json: serde_json::Value = serde_json::from_str(&response).expect("response should be json");
    assert_eq!(json["jsonrpc"], "2.0");
    assert_eq!(json["id"], 17);
    assert_eq!(json["result"]["isError"], false);

    fs::remove_dir_all(&temp_root).expect("temp root should be removable");
}

#[test]
fn mcp_stale_file_write_call_returns_removed_guidance() {
    for tool_name in ["state_file_write", "state.file.write"] {
        let temp_root = temp_root(&format!("stateful-mcp-stale-file-write-{tool_name}"));
        let paths = GlobalPaths::new(temp_root.join("home"));
        let repo_root = temp_root.join("repo");
        fs::create_dir_all(&repo_root).expect("repo root should be creatable");
        enable_test_repo(&paths, &repo_root);
        let (runtime, _rx) = spawn_fake_stateful_server(r#"{"status":"ok"}"#);
        write_global_runtime_file(&paths, &runtime).expect("global runtime file should write");

        let response = run_mcp_jsonrpc_in_repo(
            &repo_root,
            &paths,
            &format!(
                r#"{{
                  "jsonrpc":"2.0",
                  "id":8,
                  "method":"tools/call",
                  "params":{{
                    "name":"{tool_name}",
                    "arguments":{{"path":"src/auth.ts","contents":"export const ok = true;\n"}}
                  }}
                }}"#
            ),
        );

        let json: serde_json::Value =
            serde_json::from_str(&response).expect("response should be json");
        assert_eq!(json["result"]["isError"], true, "{tool_name}");
        assert!(
            json["result"]["content"][0]["text"]
                .as_str()
                .unwrap_or_default()
                .contains(
                    "state_file_write was removed; use native edit tools with hook-visible targets, such as Codex apply_patch or Edit, after task-level reservation covers the target and a successful same-session file claim"
                ),
            "{tool_name}"
        );
        assert!(
            !repo_root.join("src/auth.ts").exists(),
            "removed file write tool should not write the file"
        );

        fs::remove_dir_all(&temp_root).expect("temp root should be removable");
    }
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
        r#"{"decision":"deny","reason_code":"scope_mismatch","message":"Target is outside active reservation scope.","required_next_action":"Declare matching reservation."}"#,
        r#"{"status":"ok"}"#,
        r#"{"status":"ok"}"#,
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
        request_json_body(&first)["payload"]["purpose"],
        "Run sandbox command for write target `src/allowed.ts`."
    );
    assert_eq!(
        request_json_body(&second)["payload"]["path"],
        "src/denied.ts"
    );
    assert_eq!(
        request_json_body(&second)["payload"]["purpose"],
        "Run sandbox command for write target `src/denied.ts`."
    );
    let first_release = rx
        .recv_timeout(Duration::from_secs(1))
        .expect("first release request should arrive");
    let second_release = rx
        .recv_timeout(Duration::from_secs(1))
        .expect("second release request should arrive");
    assert!(first_release.contains("POST /v1/claim/release HTTP/1.1"));
    assert!(second_release.contains("POST /v1/claim/release HTTP/1.1"));
    assert!(
        rx.recv_timeout(Duration::from_millis(200)).is_err(),
        "sandbox denial should not render current state context"
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
    assert!(!stdout.contains("\"current_state\""));
    assert!(!stdout.contains("Your Active Scope"));

    fs::remove_dir_all(&temp_root).expect("temp root should be removable");
}

#[test]
fn sandbox_run_write_dir_authorizes_directory_and_allows_artifact_write() {
    if macos_stateful_sandbox_is_active() {
        return;
    }

    let temp_root = temp_root("stateful-sandbox-run-write-dir");
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
            "sandbox",
            "run",
            "--fs",
            "write-targets",
            "--write-dir",
            "tmp/run-1",
            "--command",
            "printf artifact > tmp/run-1/out.txt && printf tmp > \"$TMPDIR/out.tmp\"",
        ],
    );

    assert!(
        output.status.success(),
        "write-dir sandbox run failed: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let request = rx
        .recv_timeout(Duration::from_secs(1))
        .expect("authorize request should arrive");
    assert_eq!(
        request_json_body(&request)["payload"]["action"],
        "write_directory"
    );
    assert_eq!(request_json_body(&request)["payload"]["path"], "tmp/run-1/");
    assert_eq!(
        request_json_body(&request)["payload"]["purpose"],
        "Run sandbox command for write directory `tmp/run-1/`."
    );
    assert_eq!(
        fs::read_to_string(repo_root.join("tmp/run-1/out.txt")).expect("artifact should read"),
        "artifact"
    );
    assert_eq!(
        fs::read_to_string(repo_root.join("tmp/run-1/.stateful-tmp/out.tmp"))
            .expect("temp artifact should read"),
        "tmp"
    );
    assert!(
        String::from_utf8_lossy(&output.stdout)
            .contains("\"allowed_write_targets\":[\"tmp/run-1/\"]")
    );

    fs::remove_dir_all(&temp_root).expect("temp root should be removable");
}

#[test]
fn sandbox_run_write_targets_reclaims_file_lease_after_command() {
    if macos_stateful_sandbox_is_active() {
        return;
    }

    let temp_root = temp_root("stateful-sandbox-run-write-target-release");
    let paths = GlobalPaths::new(temp_root.join("home"));
    let repo_root = temp_root.join("repo");
    fs::create_dir_all(repo_root.join("src")).expect("repo src should be creatable");
    fs::write(repo_root.join("src/allowed.ts"), b"old\n").expect("target file should write");
    enable_test_repo(&paths, &repo_root);
    write_current_session_file(&repo_root, &CurrentSession::new("s-current", "w1"))
        .expect("current session should write");
    let (runtime, rx) = spawn_fake_stateful_server_sequence(vec![
        r#"{"decision":"allow","reason_code":"authorized","message":"ok","required_next_action":null}"#,
        r#"{"status":"ok"}"#,
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
            "--write-target",
            "src/allowed.ts",
            "--command",
            "printf updated > src/allowed.ts",
        ],
    );

    assert!(
        output.status.success(),
        "write-target sandbox run failed: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let authorize = rx
        .recv_timeout(Duration::from_secs(1))
        .expect("authorize request should arrive");
    assert_eq!(
        request_json_body(&authorize)["payload"]["action"],
        "write_file"
    );
    assert_eq!(
        request_json_body(&authorize)["payload"]["path"],
        "src/allowed.ts"
    );

    let release = rx
        .recv_timeout(Duration::from_secs(1))
        .expect("release request should arrive after command completes");
    assert!(release.contains("POST /v1/claim/release HTTP/1.1"));
    let body = request_json_body(&release);
    assert_eq!(body["session_id"], "s-current");
    assert_eq!(body["workspace_id"], "w1");
    assert_eq!(body["path"], "src/allowed.ts");
    assert_eq!(
        fs::read_to_string(repo_root.join("src/allowed.ts")).expect("target file should read"),
        "updated"
    );

    fs::remove_dir_all(&temp_root).expect("temp root should be removable");
}

#[test]
fn sandbox_run_write_dir_treats_repo_tmp_as_normal_authorized_directory() {
    if macos_stateful_sandbox_is_active() {
        return;
    }

    let temp_root = temp_root("stateful-sandbox-run-write-dir-root-tmp");
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
            "sandbox",
            "run",
            "--fs",
            "write-targets",
            "--write-dir",
            "tmp",
            "--command",
            "printf repo-tmp > tmp/out.txt && printf tmp > \"$TMPDIR/out.tmp\"",
        ],
    );

    assert!(
        output.status.success(),
        "direct tmp write-dir sandbox run failed: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let request = rx
        .recv_timeout(Duration::from_secs(1))
        .expect("authorize request should arrive");
    assert_eq!(
        request_json_body(&request)["payload"]["action"],
        "write_directory"
    );
    assert_eq!(request_json_body(&request)["payload"]["path"], "tmp/");
    assert_eq!(
        fs::read_to_string(repo_root.join("tmp/out.txt")).expect("tmp artifact should read"),
        "repo-tmp"
    );
    assert_eq!(
        fs::read_to_string(repo_root.join("tmp/.stateful-tmp/out.tmp"))
            .expect("temp artifact should read"),
        "tmp"
    );
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("\"allowed_write_targets\":[\"tmp/\"]")
    );

    fs::remove_dir_all(&temp_root).expect("temp root should be removable");
}

#[test]
fn sandbox_run_build_profile_uses_external_tmp_stateful_scratch_without_authorize() {
    if macos_stateful_sandbox_is_active() {
        return;
    }

    let temp_root = temp_root("stateful-sandbox-run-build-profile");
    let paths = GlobalPaths::new(temp_root.join("home"));
    let repo_root = temp_root.join("repo");
    fs::create_dir_all(&repo_root).expect("repo root should be creatable");
    enable_test_repo(&paths, &repo_root);
    let session_id = format!("s-build-profile-{}", std::process::id());
    let scratch_session_root = Path::new("/tmp/stateful").join(&session_id);
    if scratch_session_root.exists() {
        fs::remove_dir_all(&scratch_session_root).expect("old scratch root should be removable");
    }
    write_current_session_file(&repo_root, &CurrentSession::new(&session_id, "w1"))
        .expect("current session should write");
    let (runtime, rx) = spawn_fake_stateful_server(
        r#"{"decision":"allow","reason_code":"authorized","message":"should not authorize","required_next_action":null}"#,
    );
    write_global_runtime_file(&paths, &runtime).expect("global runtime file should write");

    let output = run_stateful_in_repo(
        &repo_root,
        &paths,
        &[
            "sandbox",
            "run",
            "--fs",
            "build",
            "--network",
            "enabled",
            "--write-dir",
            "build-profile",
            "--command",
            "printf '%s\n' \"$TMPDIR\" && printf tmp > \"$TMPDIR/build.tmp\" && printf '%s\n' \"$CARGO_TARGET_DIR\"",
        ],
    );

    assert!(
        output.status.success(),
        "build profile sandbox run failed: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        rx.recv_timeout(Duration::from_millis(100)).is_err(),
        "build scratch should not request repo write authorization"
    );
    let scratch_root = scratch_session_root.join("build-profile");
    let tmp_dir = scratch_root.join(".stateful-tmp");
    assert_eq!(
        fs::read_to_string(tmp_dir.join("build.tmp")).expect("temp artifact should read"),
        "tmp"
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains(&format!(
        "/tmp/stateful/{session_id}/build-profile/.stateful-tmp"
    )));
    assert!(
        String::from_utf8_lossy(&output.stdout)
            .contains(&format!("/tmp/stateful/{session_id}/build-profile/target"))
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains(&format!(
        "\"allowed_write_targets\":[\"/tmp/stateful/{session_id}/build-profile/\"]"
    )));
    assert!(!repo_root.join("tmp/build-profile").exists());

    fs::remove_dir_all(&scratch_session_root).expect("scratch root should be removable");
    fs::remove_dir_all(&temp_root).expect("temp root should be removable");
}

#[test]
fn sandbox_run_build_profile_requires_scoped_tmp_write_dir() {
    let temp_root = temp_root("stateful-sandbox-run-build-requires-write-dir");
    let paths = GlobalPaths::new(temp_root.join("home"));
    let repo_root = temp_root.join("repo");
    fs::create_dir_all(&repo_root).expect("repo root should be creatable");
    enable_test_repo(&paths, &repo_root);
    write_current_session_file(&repo_root, &CurrentSession::new("s-current", "w1"))
        .expect("current session should write");
    let (runtime, rx) = spawn_fake_stateful_server(
        r#"{"decision":"allow","reason_code":"authorized","message":"should not authorize","required_next_action":null}"#,
    );
    write_global_runtime_file(&paths, &runtime).expect("global runtime file should write");

    let output = run_stateful_in_repo(
        &repo_root,
        &paths,
        &[
            "sandbox",
            "run",
            "--fs",
            "build",
            "--network",
            "enabled",
            "--command",
            "printf bypass > tmp/build.out",
        ],
    );

    assert!(
        !output.status.success(),
        "build profile should require write-dir"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("--write-dir <scratch-purpose>"),
        "stderr was {stderr}"
    );
    assert!(
        rx.recv_timeout(Duration::from_millis(100)).is_err(),
        "missing build write-dir should fail before authorization"
    );
    assert!(
        !repo_root.join("tmp/build.out").exists(),
        "build command must not run without a scratch-purpose write-dir"
    );

    fs::remove_dir_all(&temp_root).expect("temp root should be removable");
}

#[test]
fn sandbox_run_build_profile_sets_cargo_target_dir_under_scoped_tmp() {
    if macos_stateful_sandbox_is_active() {
        return;
    }

    let temp_root = temp_root("stateful-sandbox-run-build-cargo-target");
    let paths = GlobalPaths::new(temp_root.join("home"));
    let repo_root = temp_root.join("repo");
    fs::create_dir_all(&repo_root).expect("repo root should be creatable");
    enable_test_repo(&paths, &repo_root);
    let session_id = format!("s-build-cargo-{}", std::process::id());
    let scratch_session_root = Path::new("/tmp/stateful").join(&session_id);
    if scratch_session_root.exists() {
        fs::remove_dir_all(&scratch_session_root).expect("old scratch root should be removable");
    }
    write_current_session_file(&repo_root, &CurrentSession::new(&session_id, "w1"))
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
            "build",
            "--network",
            "enabled",
            "--write-dir",
            "build-cargo",
            "--command",
            "printf '%s' \"$CARGO_TARGET_DIR\"",
        ],
    );

    assert!(
        output.status.success(),
        "build profile sandbox run failed: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        String::from_utf8_lossy(&output.stdout)
            .contains(&format!("/tmp/stateful/{session_id}/build-cargo/target"))
    );
    assert!(!repo_root.join("tmp/build-cargo").exists());

    fs::remove_dir_all(&scratch_session_root).expect("scratch root should be removable");
    fs::remove_dir_all(&temp_root).expect("temp root should be removable");
}

#[test]
fn sandbox_run_build_profile_denies_dependency_shadowing_before_command_runs() {
    let temp_root = temp_root("stateful-sandbox-run-build-shadowing");
    let paths = GlobalPaths::new(temp_root.join("home"));
    let repo_root = temp_root.join("repo");
    fs::create_dir_all(repo_root.join("langchain_core")).expect("shadow root should be creatable");
    fs::write(
        repo_root.join("pyproject.toml"),
        r#"
[project]
name = "example-project"
dependencies = ["langchain-core>=0.3"]
"#,
    )
    .expect("pyproject should write");
    fs::write(
        repo_root.join("langchain_core/__init__.py"),
        "\"\"\"local shim\"\"\"\n",
    )
    .expect("shadow package should write");
    enable_test_repo(&paths, &repo_root);

    let output = run_stateful_in_repo(
        &repo_root,
        &paths,
        &[
            "sandbox",
            "run",
            "--fs",
            "build",
            "--write-dir",
            "build-shadowing",
            "--command",
            "printf should-not-run > \"$TMPDIR/shadow-audit-ran.txt\"",
        ],
    );

    assert!(!output.status.success(), "shadowing audit should fail");
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(combined.contains("dependency shadowing guard"));
    assert!(combined.contains("langchain_core"));
    assert!(combined.contains("langchain-core"));
    assert!(
        !repo_root.join("tmp/build-shadowing").exists(),
        "build command must not run when import resolution audit fails"
    );

    fs::remove_dir_all(&temp_root).expect("temp root should be removable");
}

#[test]
fn sandbox_run_write_dir_delegates_source_tree_directory_to_authorization() {
    let temp_root = temp_root("stateful-sandbox-run-write-dir-source");
    let paths = GlobalPaths::new(temp_root.join("home"));
    let repo_root = temp_root.join("repo");
    fs::create_dir_all(repo_root.join("src")).expect("repo src should be creatable");
    enable_test_repo(&paths, &repo_root);
    write_current_session_file(&repo_root, &CurrentSession::new("s-current", "w1"))
        .expect("current session should write");
    let (runtime, rx) = spawn_fake_stateful_server(
        r#"{"decision":"deny","reason_code":"scope_mismatch","message":"source dirs must not authorize","required_next_action":"Use native edit tools."}"#,
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
            "--write-dir",
            "src",
            "--command",
            "printf bypass > src/main.rs",
        ],
    );

    assert!(!output.status.success(), "source write-dir should fail");
    let request = rx
        .recv_timeout(Duration::from_secs(1))
        .expect("source write-dir should be checked by authorization");
    assert_eq!(
        request_json_body(&request)["payload"]["action"],
        "write_directory"
    );
    assert_eq!(request_json_body(&request)["payload"]["path"], "src/");
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(combined.contains("\"path\":\"src/\""));
    assert!(combined.contains("source dirs must not authorize"));

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

#[test]
fn sandbox_run_read_only_does_not_require_reachable_runtime() {
    if macos_stateful_sandbox_is_active() {
        return;
    }

    let temp_root = temp_root("stateful-sandbox-run-readonly-no-runtime");
    let paths = GlobalPaths::new(temp_root.join("home"));
    let repo_root = temp_root.join("repo");
    fs::create_dir_all(&repo_root).expect("repo root should be creatable");
    enable_test_repo(&paths, &repo_root);

    let output = run_stateful_in_repo_with_env(
        &repo_root,
        &paths,
        &[
            ("STATEFUL_SERVER_URL", "http://127.0.0.1:9"),
            ("STATEFUL_SERVER_TOKEN", "unreachable-token"),
        ],
        &[
            "sandbox",
            "run",
            "--fs",
            "read-only",
            "--network",
            "disabled",
            "--command",
            "printf ok",
        ],
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !stdout.contains("Connection refused") && !stderr.contains("Connection refused"),
        "read-only sandbox must not try to contact runtime: stdout={stdout} stderr={stderr}"
    );
    let body: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("sandbox output should be json");
    if macos_stateful_sandbox_is_active()
        && body["stderr"]
            .as_str()
            .is_some_and(|stderr| stderr.contains("sandbox-exec: sandbox_apply"))
    {
        assert_eq!(body["status"], "exited");
        fs::remove_dir_all(&temp_root).expect("temp root should be removable");
        return;
    }
    assert!(
        output.status.success(),
        "read-only sandbox should run without runtime: stdout={stdout} stderr={stderr}"
    );
    assert_eq!(body["status"], "exited");
    assert_eq!(body["stdout"], "ok");

    fs::remove_dir_all(&temp_root).expect("temp root should be removable");
}

#[test]
fn sandbox_run_external_profile_allows_read_only_scope_without_runtime() {
    if macos_stateful_sandbox_is_active() {
        return;
    }

    let temp_root = temp_root("stateful-sandbox-run-external-read-only");
    let paths = GlobalPaths::new(temp_root.join("home"));
    let repo_root = temp_root.join("repo");
    fs::create_dir_all(&repo_root).expect("repo root should be creatable");
    enable_test_repo(&paths, &repo_root);

    let output = run_stateful_in_repo_with_env(
        &repo_root,
        &paths,
        &[
            ("STATEFUL_SERVER_URL", "http://127.0.0.1:9"),
            ("STATEFUL_SERVER_TOKEN", "unreachable-token"),
        ],
        &[
            "sandbox",
            "run",
            "--fs",
            "external",
            "--network",
            "enabled",
            "--purpose",
            "inspect external environment",
            "--command",
            "printf external-read-only",
        ],
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !stdout.contains("Connection refused") && !stderr.contains("Connection refused"),
        "external read-only sandbox must not contact runtime: stdout={stdout} stderr={stderr}"
    );
    let body: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("sandbox output should be json");
    if macos_stateful_sandbox_is_active()
        && body["stderr"]
            .as_str()
            .is_some_and(|stderr| stderr.contains("sandbox-exec: sandbox_apply"))
    {
        assert_eq!(body["status"], "exited");
        fs::remove_dir_all(&temp_root).expect("temp root should be removable");
        return;
    }
    assert!(
        output.status.success(),
        "external read-only sandbox should run without runtime: stdout={stdout} stderr={stderr}"
    );
    assert_eq!(body["status"], "exited");
    assert_eq!(body["stdout"], "external-read-only");
    assert_eq!(body["allowed_write_targets"], serde_json::json!([]));

    fs::remove_dir_all(&temp_root).expect("temp root should be removable");
}

#[test]
fn sandbox_run_external_profile_writes_absolute_external_targets_without_runtime() {
    if macos_stateful_sandbox_is_active() {
        return;
    }

    let temp_root = external_temp_root("stateful-sandbox-run-external-no-runtime");
    let paths = GlobalPaths::new(temp_root.join("home"));
    let repo_root = temp_root.join("repo");
    let external_dir = temp_root.join("external-output");
    fs::create_dir_all(&repo_root).expect("repo root should be creatable");
    fs::create_dir_all(&external_dir).expect("external output dir should be creatable");
    enable_test_repo(&paths, &repo_root);
    let output_path = external_dir.join("artifact.txt");
    let existing_path = external_dir.join("existing.txt");
    fs::write(&existing_path, "old").expect("existing external file should seed");
    let output_arg = output_path.to_string_lossy().into_owned();
    let existing_arg = existing_path.to_string_lossy().into_owned();
    let write_dir_arg = external_dir.to_string_lossy().into_owned();
    let command = format!(
        "printf external > {} && printf updated > {}",
        shell_quote_path(&output_path),
        shell_quote_path(&existing_path)
    );

    let output = run_stateful_in_repo_with_env(
        &repo_root,
        &paths,
        &[
            ("STATEFUL_SERVER_URL", "http://127.0.0.1:9"),
            ("STATEFUL_SERVER_TOKEN", "unreachable-token"),
        ],
        &[
            "sandbox",
            "run",
            "--fs",
            "external",
            "--network",
            "enabled",
            "--purpose",
            "write external artifact",
            "--create-target",
            output_arg.as_str(),
            "--write-target",
            existing_arg.as_str(),
            "--write-dir",
            write_dir_arg.as_str(),
            "--allow-signal",
            "--timeout-seconds",
            "10",
            "--command",
            command.as_str(),
        ],
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !stdout.contains("Connection refused") && !stderr.contains("Connection refused"),
        "external sandbox must not contact runtime: stdout={stdout} stderr={stderr}"
    );
    assert!(
        output.status.success(),
        "external sandbox run failed: stdout={stdout} stderr={stderr}"
    );
    assert_eq!(
        fs::read_to_string(&output_path).expect("external artifact should read"),
        "external"
    );
    assert_eq!(
        fs::read_to_string(&existing_path).expect("existing external file should read"),
        "updated"
    );

    fs::remove_dir_all(&temp_root).expect("temp root should be removable");
}

#[test]
fn sandbox_run_external_profile_authorizes_repo_relative_targets() {
    if macos_stateful_sandbox_is_active() {
        return;
    }

    let temp_root = temp_root("stateful-sandbox-run-external-repo-target");
    let paths = GlobalPaths::new(temp_root.join("home"));
    let repo_root = temp_root.join("repo");
    fs::create_dir_all(repo_root.join("src")).expect("repo src should be creatable");
    enable_test_repo(&paths, &repo_root);
    write_current_session_file(&repo_root, &CurrentSession::new("s-current", "w1"))
        .expect("current session should write");
    fs::write(repo_root.join("src/allowed.ts"), "old\n").expect("allowed file should seed");
    let (runtime, rx) = spawn_fake_stateful_server_sequence(vec![
        r#"{"decision":"allow","reason_code":"authorized","message":"ok","required_next_action":null}"#,
        r#"{"status":"ok"}"#,
    ]);
    write_global_runtime_file(&paths, &runtime).expect("global runtime file should write");

    let output = run_stateful_in_repo(
        &repo_root,
        &paths,
        &[
            "sandbox",
            "run",
            "--fs",
            "external",
            "--purpose",
            "update generated repo file through external sandbox",
            "--write-target",
            "src/allowed.ts",
            "--command",
            "printf changed > src/allowed.ts",
        ],
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "external repo-relative sandbox run failed: stdout={stdout} stderr={stderr}"
    );
    let authorize = rx
        .recv_timeout(Duration::from_secs(1))
        .expect("authorize request should arrive");
    assert_eq!(
        request_json_body(&authorize)["payload"]["action"],
        "write_file"
    );
    assert_eq!(
        request_json_body(&authorize)["payload"]["path"],
        "src/allowed.ts"
    );
    let release = rx
        .recv_timeout(Duration::from_secs(1))
        .expect("release request should arrive after command completes");
    assert!(release.contains("POST /v1/claim/release HTTP/1.1"));
    assert_eq!(
        fs::read_to_string(repo_root.join("src/allowed.ts")).expect("allowed file should read"),
        "changed"
    );

    fs::remove_dir_all(&temp_root).expect("temp root should be removable");
}

#[cfg(unix)]
#[test]
fn sandbox_run_external_profile_accepts_connect_socket_scope_without_runtime() {
    if macos_stateful_sandbox_is_active() {
        return;
    }

    let temp_root = external_temp_root("stateful-sandbox-run-external-socket");
    let paths = GlobalPaths::new(temp_root.join("home"));
    let repo_root = temp_root.join("repo");
    let external_dir = temp_root.join("external-control");
    fs::create_dir_all(&repo_root).expect("repo root should be creatable");
    fs::create_dir_all(&external_dir).expect("external control dir should be creatable");
    enable_test_repo(&paths, &repo_root);
    let socket_path = external_dir.join("control.sock");
    let _listener =
        std::os::unix::net::UnixListener::bind(&socket_path).expect("socket should bind");
    let socket_arg = socket_path.to_string_lossy().into_owned();

    let output = run_stateful_in_repo_with_env(
        &repo_root,
        &paths,
        &[
            ("STATEFUL_SERVER_URL", "http://127.0.0.1:9"),
            ("STATEFUL_SERVER_TOKEN", "unreachable-token"),
        ],
        &[
            "sandbox",
            "run",
            "--fs",
            "external",
            "--purpose",
            "connect external control socket",
            "--connect-socket",
            socket_arg.as_str(),
            "--command",
            "true",
        ],
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !stdout.contains("Connection refused") && !stderr.contains("Connection refused"),
        "external socket sandbox must not contact runtime: stdout={stdout} stderr={stderr}"
    );
    assert!(
        output.status.success(),
        "external socket sandbox run failed: stdout={stdout} stderr={stderr}"
    );

    fs::remove_dir_all(&temp_root).expect("temp root should be removable");
}

#[test]
fn sandbox_run_external_profile_requires_purpose() {
    let temp_root = temp_root("stateful-sandbox-run-external-purpose");
    let paths = GlobalPaths::new(temp_root.join("home"));
    let repo_root = temp_root.join("repo");
    let external_dir = temp_root.join("external-output");
    fs::create_dir_all(&repo_root).expect("repo root should be creatable");
    fs::create_dir_all(&external_dir).expect("external output dir should be creatable");
    enable_test_repo(&paths, &repo_root);
    let write_dir_arg = external_dir.to_string_lossy().into_owned();

    let output = run_stateful_in_repo(
        &repo_root,
        &paths,
        &[
            "sandbox",
            "run",
            "--fs",
            "external",
            "--write-dir",
            write_dir_arg.as_str(),
            "--command",
            "true",
        ],
    );

    assert!(
        !output.status.success(),
        "external profile should require purpose"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("--purpose"), "stderr was {stderr}");

    fs::remove_dir_all(&temp_root).expect("temp root should be removable");
}

#[test]
fn sandbox_run_external_profile_rejects_repo_internal_targets() {
    let temp_root = temp_root("stateful-sandbox-run-external-internal-target");
    let paths = GlobalPaths::new(temp_root.join("home"));
    let repo_root = temp_root.join("repo");
    fs::create_dir_all(&repo_root).expect("repo root should be creatable");
    enable_test_repo(&paths, &repo_root);
    let internal_path = repo_root.join("generated.txt");
    let internal_arg = internal_path.to_string_lossy().into_owned();

    let output = run_stateful_in_repo(
        &repo_root,
        &paths,
        &[
            "sandbox",
            "run",
            "--fs",
            "external",
            "--purpose",
            "try to mutate repo through external sandbox",
            "--create-target",
            internal_arg.as_str(),
            "--command",
            "true",
        ],
    );

    assert!(
        !output.status.success(),
        "external profile should reject repo-internal targets"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("outside the repo") || stderr.contains("inside the repo"),
        "stderr was {stderr}"
    );
    assert!(
        !internal_path.exists(),
        "repo-internal target must not be created by external profile"
    );

    fs::remove_dir_all(&temp_root).expect("temp root should be removable");
}

#[test]
fn mcp_initialize_advertises_resources_capability() {
    let temp_root = temp_root("stateful-mcp-initialize-resources");
    fs::create_dir_all(&temp_root).expect("temp root should be creatable");

    let response = handle_mcp_jsonrpc_in_repo(
        &temp_root,
        r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}"#,
    )
    .expect("initialize should handle")
    .expect("initialize should produce response");
    let json: serde_json::Value = serde_json::from_str(&response).expect("response should be json");

    assert_eq!(json["jsonrpc"], "2.0");
    assert_eq!(json["id"], 1);
    assert_eq!(
        json["result"]["capabilities"]["resources"],
        serde_json::json!({})
    );

    fs::remove_dir_all(&temp_root).expect("temp root should be removable");
}

#[test]
fn mcp_resources_list_returns_stateful_read_resources() {
    let temp_root = temp_root("stateful-mcp-resources-list");
    fs::create_dir_all(&temp_root).expect("temp root should be creatable");

    let response = handle_mcp_jsonrpc_in_repo(
        &temp_root,
        r#"{"jsonrpc":"2.0","id":1,"method":"resources/list","params":{}}"#,
    )
    .expect("resources/list should handle")
    .expect("resources/list should produce response");
    let json: serde_json::Value = serde_json::from_str(&response).expect("response should be json");

    assert_eq!(json["jsonrpc"], "2.0");
    assert_eq!(json["id"], 1);
    let resources = json["result"]["resources"]
        .as_array()
        .expect("resources should be array");
    let current = resources
        .iter()
        .find(|resource| resource["uri"] == "stateful/current")
        .expect("current resource should be listed");
    assert_eq!(current["mimeType"], "application/json");
    let events = resources
        .iter()
        .find(|resource| resource["uri"] == "stateful/events")
        .expect("events resource should be listed");
    assert_eq!(events["mimeType"], "application/json");
    let context = resources
        .iter()
        .find(|resource| resource["uri"] == "stateful/context")
        .expect("context resource should be listed");
    assert_eq!(context["mimeType"], "text/plain");

    fs::remove_dir_all(&temp_root).expect("temp root should be removable");
}

#[test]
fn mcp_resources_read_current_forwards_to_current_endpoint() {
    let temp_root = temp_root("stateful-mcp-resources-read-current");
    let paths = GlobalPaths::new(temp_root.join("home"));
    let repo_root = temp_root.join("repo");
    fs::create_dir_all(&repo_root).expect("repo root should be creatable");
    enable_test_repo(&paths, &repo_root);
    let current_body = r#"{"status":"ok","current":{"source":"resource"}}"#;
    let (runtime, rx) = spawn_fake_stateful_server(current_body);

    let response = run_mcp_jsonrpc_in_repo_with_env(
        &repo_root,
        &paths,
        &[
            ("STATEFUL_SERVER_URL", runtime.base_url.as_str()),
            ("STATEFUL_SERVER_TOKEN", runtime.token.as_str()),
        ],
        r#"{
          "jsonrpc":"2.0",
          "id":2,
          "method":"resources/read",
          "params":{
            "uri":"stateful/current"
          }
        }"#,
    );
    let json: serde_json::Value = serde_json::from_str(&response).expect("response should be json");

    assert_eq!(json["jsonrpc"], "2.0");
    assert_eq!(json["id"], 2);
    let contents = json["result"]["contents"]
        .as_array()
        .expect("contents should be array");
    assert_eq!(contents.len(), 1);
    assert_eq!(contents[0]["uri"], "stateful/current");
    assert_eq!(contents[0]["mimeType"], "application/json");
    assert_eq!(contents[0]["text"], current_body);
    let request = rx
        .recv_timeout(Duration::from_secs(5))
        .expect("captured request should arrive");
    assert!(request.contains("GET /v1/current HTTP/1.1"));
    assert!(request.contains("Authorization: Bearer secret-token"));

    fs::remove_dir_all(&temp_root).expect("temp root should be removable");
}

#[test]
fn mcp_resources_read_context_forwards_brief_context_with_empty_resource_filter() {
    let temp_root = temp_root("stateful-mcp-resources-read-context");
    let paths = GlobalPaths::new(temp_root.join("home"));
    let repo_root = temp_root.join("repo");
    fs::create_dir_all(&repo_root).expect("repo root should be creatable");
    enable_test_repo(&paths, &repo_root);
    let context_body = r#"{"status":"ok","prompt_text":"brief context"}"#;
    let (runtime, rx) = spawn_fake_stateful_server(context_body);

    let response = run_mcp_jsonrpc_in_repo_with_env(
        &repo_root,
        &paths,
        &[
            ("STATEFUL_SERVER_URL", runtime.base_url.as_str()),
            ("STATEFUL_SERVER_TOKEN", runtime.token.as_str()),
            ("STATEFUL_SESSION_ID", "s-context"),
        ],
        r#"{
          "jsonrpc":"2.0",
          "id":3,
          "method":"resources/read",
          "params":{
            "uri":"stateful/context"
          }
        }"#,
    );
    let json: serde_json::Value = serde_json::from_str(&response).expect("response should be json");

    assert_eq!(json["jsonrpc"], "2.0");
    assert_eq!(json["id"], 3);
    let contents = json["result"]["contents"]
        .as_array()
        .expect("contents should be array");
    assert_eq!(contents.len(), 1);
    assert_eq!(contents[0]["uri"], "stateful/context");
    assert_eq!(contents[0]["mimeType"], "text/plain");
    assert_eq!(contents[0]["text"], context_body);
    let request = rx
        .recv_timeout(Duration::from_secs(5))
        .expect("captured request should arrive");
    assert!(request.contains("POST /v1/context/render HTTP/1.1"));
    assert!(request.contains("Authorization: Bearer secret-token"));
    let body = request_json_body(&request);
    assert_eq!(body["mode"], "brief");
    assert_eq!(body["resource"], "");

    fs::remove_dir_all(&temp_root).expect("temp root should be removable");
}

#[test]
fn mcp_resources_read_rejects_unknown_resource_uri() {
    let temp_root = temp_root("stateful-mcp-resources-read-unknown");
    fs::create_dir_all(&temp_root).expect("temp root should be creatable");

    let response = handle_mcp_jsonrpc_in_repo(
        &temp_root,
        r#"{
          "jsonrpc":"2.0",
          "id":3,
          "method":"resources/read",
          "params":{
            "uri":"stateful/missing"
          }
        }"#,
    )
    .expect("resources/read should handle")
    .expect("resources/read should produce response");
    let json: serde_json::Value = serde_json::from_str(&response).expect("response should be json");

    assert_eq!(json["jsonrpc"], "2.0");
    assert_eq!(json["id"], 3);
    assert_eq!(json["error"]["code"], -32602);
    assert!(
        json["error"]["message"]
            .as_str()
            .unwrap_or_default()
            .contains("unknown Stateful MCP resource URI")
    );

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
            .any(|tool| tool["name"] == "state_reservation_declare")
    );
    assert!(!tools.iter().any(|tool| tool["name"] == "state_file_write"));
    assert!(!tools.iter().any(|tool| tool["name"] == "state_bash_write"));
    let reservation_tool = tools
        .iter()
        .find(|tool| tool["name"] == "state_reservation_declare")
        .expect("reservation tool should be listed");
    assert_eq!(
        reservation_tool["description"],
        "Declare a task-level repo-internal reservation with the known file or directory set before repo write actions."
    );
    assert_eq!(
        reservation_tool["inputSchema"]["required"],
        serde_json::json!(["purpose", "files_planned"])
    );
    assert_eq!(
        reservation_tool["inputSchema"]["properties"]["purpose"]["type"],
        "string"
    );
    assert_eq!(
        reservation_tool["inputSchema"]["properties"]["purpose"]["minLength"],
        1
    );
    assert_eq!(
        reservation_tool["inputSchema"]["properties"]["files_planned"]["items"]["type"],
        "string"
    );
    assert_eq!(
        reservation_tool["inputSchema"]["properties"]["files_planned"]["minItems"],
        1
    );
    let reconcile_tool = tools
        .iter()
        .find(|tool| tool["name"] == "state_reconcile_ack")
        .expect("reconcile tool should be listed");
    assert_eq!(
        reconcile_tool["inputSchema"]["properties"]["files_reread"]["items"]["type"],
        "string"
    );
    assert!(
        reconcile_tool["inputSchema"]["properties"]["files_reread"]
            .get("minItems")
            .is_none()
    );

    fs::remove_dir_all(&temp_root).expect("temp root should be removable");
}

#[test]
fn mcp_stale_bash_write_call_returns_removed_guidance() {
    for tool_name in ["state_bash_write", "state.bash.write"] {
        let temp_root = temp_root(&format!("stateful-mcp-stale-bash-write-{tool_name}"));
        let paths = GlobalPaths::new(temp_root.join("home"));
        let repo_root = temp_root.join("repo");
        fs::create_dir_all(&repo_root).expect("repo root should be creatable");
        enable_test_repo(&paths, &repo_root);
        let (runtime, _rx) = spawn_fake_stateful_server(r#"{"status":"ok"}"#);
        write_global_runtime_file(&paths, &runtime).expect("global runtime file should write");

        let response = run_mcp_jsonrpc_in_repo(
            &repo_root,
            &paths,
            &format!(
                r#"{{
                  "jsonrpc":"2.0",
                  "id":4,
                  "method":"tools/call",
                  "params":{{
                    "name":"{tool_name}",
                    "arguments":{{"command":"true","write_targets":["README.md"]}}
                  }}
                }}"#
            ),
        );

        let json: serde_json::Value =
            serde_json::from_str(&response).expect("response should be json");
        assert_eq!(json["result"]["isError"], true, "{tool_name}");
        assert!(
            json["result"]["content"][0]["text"]
                .as_str()
                .unwrap_or_default()
                .contains(
                    "state_bash_write was removed; use stateful sandbox run --fs write-targets --write-target <repo-path> ... --command <cmd> after repo reservation/claim, or stateful sandbox run --fs external --purpose <purpose> [--write-target <repo-path-or-absolute-external-path>] [--create-target <repo-path-or-absolute-external-path>] [--write-dir <repo-path-or-absolute-external-dir>] [--connect-socket <absolute-socket>] [--allow-signal] [--network disabled|enabled] --command <cmd> for approved external commands; repo-relative scopes still require repo reservation/claim."
                ),
            "{tool_name}"
        );

        fs::remove_dir_all(&temp_root).expect("temp root should be removable");
    }
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
fn mcp_context_render_defaults_to_current_session_workspace() {
    let temp_root = temp_root("stateful-mcp-context-render-session");
    let paths = GlobalPaths::new(temp_root.join("home"));
    let repo_root = temp_root.join("repo");
    fs::create_dir_all(&repo_root).expect("repo root should be creatable");
    enable_test_repo(&paths, &repo_root);
    let identity = stateful_cli::repo_identity_for_enabled_repo(&paths, &repo_root)
        .expect("repo identity should resolve");
    let (runtime, rx) = spawn_fake_stateful_server(r#"{"status":"ok","prompt_text":""}"#);
    write_global_runtime_file(&paths, &runtime).expect("global runtime file should write");
    write_current_session_file_for_session(&repo_root, "s1", &CurrentSession::new("s1", "w1"))
        .expect("session-bound current session should write");

    let response = run_mcp_jsonrpc_in_repo_with_env(
        &repo_root,
        &paths,
        &[("STATEFUL_SESSION_ID", "s1")],
        r#"{
          "jsonrpc":"2.0",
          "id":2,
          "method":"tools/call",
          "params":{
            "name":"state_context_render",
            "arguments":{
              "mode":"brief",
              "resource":"src/auth.ts"
            }
          }
        }"#,
    );

    let request = rx.recv().expect("captured request should arrive");
    assert!(request.contains("POST /v1/context/render HTTP/1.1"));
    assert!(request.contains("Authorization: Bearer secret-token"));
    let body = request_json_body(&request);
    assert_eq!(body["session_id"], "s1");
    assert_eq!(body["workspace_id"], "w1");
    assert_eq!(body["mode"], "brief");
    assert_eq!(body["resource"], "src/auth.ts");
    assert_eq!(body["repo_id"], identity.repo_id);
    assert_eq!(body["worktree_id"], identity.worktree_id);
    assert_eq!(body["root"], identity.root);

    let json: serde_json::Value = serde_json::from_str(&response).expect("response should be json");
    assert_eq!(json["jsonrpc"], "2.0");
    assert_eq!(json["id"], 2);
    assert_eq!(json["result"]["isError"], false);

    fs::remove_dir_all(&temp_root).expect("temp root should be removable");
}

#[test]
fn mcp_tools_call_for_reservation_declare_posts_to_state_server() {
    let temp_root = temp_root("stateful-mcp-reservation-declare");
    let paths = GlobalPaths::new(temp_root.join("home"));
    let repo_root = temp_root.join("repo");
    fs::create_dir_all(&repo_root).expect("repo root should be creatable");
    enable_test_repo(&paths, &repo_root);
    let (runtime, rx) = spawn_fake_stateful_server(r#"{"status":"ok"}"#);
    write_global_runtime_file(&paths, &runtime).expect("global runtime file should write");
    write_current_session_file_for_session(&repo_root, "s1", &CurrentSession::new("s1", "w1"))
        .expect("session-bound current session should write");

    let response = run_mcp_jsonrpc_in_repo_with_env(
        &repo_root,
        &paths,
        &[("STATEFUL_SESSION_ID", "s1")],
        r#"{
          "jsonrpc":"2.0",
          "id":2,
          "method":"tools/call",
          "params":{
            "name":"state_reservation_declare",
            "arguments":{
              "session_id":"s1",
              "workspace_id":"w1",
              "purpose":"Fix auth validation behavior.",
              "files_planned":["src/auth.ts"]
            }
          }
        }"#,
    );

    let request = rx.recv().expect("captured request should arrive");
    assert!(request.contains("POST /v1/reservation/declare HTTP/1.1"));
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
            "purpose": "Fix auth validation behavior.",
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
fn reservation_declare_command_posts_repo_identity() {
    let temp_root = temp_root("stateful-cli-reservation-identity");
    let paths = GlobalPaths::new(temp_root.join("home"));
    let repo_root = temp_root.join("repo");
    fs::create_dir_all(&repo_root).expect("repo root should be creatable");
    enable_test_repo(&paths, &repo_root);
    let (runtime, rx) = spawn_fake_stateful_server(r#"{"status":"ok"}"#);
    write_global_runtime_file(&paths, &runtime).expect("global runtime file should write");
    write_current_session_file(&repo_root, &CurrentSession::new("s-current", "w1"))
        .expect("current session should write");

    let output = run_stateful_in_repo(
        &repo_root,
        &paths,
        &[
            "reservation",
            "declare",
            "--purpose",
            "Fix auth validation behavior.",
            "src/auth.ts",
        ],
    );

    assert!(
        output.status.success(),
        "stateful reservation declare failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let request = rx.recv().expect("captured request should arrive");
    assert!(request.contains("POST /v1/reservation/declare HTTP/1.1"));
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
            "purpose": "Fix auth validation behavior.",
            "files_planned": ["src/auth.ts"]
        })
    );

    fs::remove_dir_all(&temp_root).expect("temp root should be removable");
}

#[test]
fn reservation_request_command_prints_wait_id_from_server_response() {
    let temp_root = temp_root("stateful-cli-reservation-request-wait-id");
    let paths = GlobalPaths::new(temp_root.join("home"));
    let repo_root = temp_root.join("repo");
    fs::create_dir_all(&repo_root).expect("repo root should be creatable");
    enable_test_repo(&paths, &repo_root);
    let (runtime, rx) = spawn_fake_stateful_server(
        r#"{"status":"queued","wait":{"wait_id":"wait-123","resource":"src/auth.ts"}}"#,
    );
    write_global_runtime_file(&paths, &runtime).expect("global runtime file should write");
    write_current_session_file(&repo_root, &CurrentSession::new("s-current", "w1"))
        .expect("current session should write");

    let output = run_stateful_in_repo(
        &repo_root,
        &paths,
        &[
            "reservation",
            "request",
            "--request-id",
            "request-1",
            "--action",
            "write_file",
            "--path",
            "src/auth.ts",
            "--purpose",
            "Queue auth file changes.",
        ],
    );

    assert!(
        output.status.success(),
        "stateful reservation request failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("\"wait_id\":\"wait-123\""));
    let request = rx.recv().expect("captured request should arrive");
    assert!(request.contains("POST /v1/reservation/request HTTP/1.1"));
    let body = request_json_body(&request);
    assert_eq!(body["session"]["session_id"], "s-current");
    assert_eq!(body["workspace"]["workspace_id"], "w1");
    assert_eq!(body["payload"]["request_id"], "request-1");
    assert_eq!(body["payload"]["action"], "write_file");
    assert_eq!(body["payload"]["path"], "src/auth.ts");

    fs::remove_dir_all(&temp_root).expect("temp root should be removable");
}

#[test]
fn mcp_reservation_declare_defaults_to_current_hook_session() {
    let temp_root = temp_root("stateful-mcp-reservation-current-session");
    let paths = GlobalPaths::new(temp_root.join("home"));
    let repo_root = temp_root.join("repo");
    fs::create_dir_all(&repo_root).expect("repo root should be creatable");
    enable_test_repo(&paths, &repo_root);
    let (runtime, rx) = spawn_fake_stateful_server(r#"{"status":"ok"}"#);
    write_global_runtime_file(&paths, &runtime).expect("global runtime file should write");
    write_current_session_file_for_session(
        &repo_root,
        "s-current",
        &CurrentSession::new("s-current", "w1"),
    )
    .expect("session-bound current session should write");

    let response = run_mcp_jsonrpc_in_repo_with_env(
        &repo_root,
        &paths,
        &[("STATEFUL_SESSION_ID", "s-current")],
        r#"{
          "jsonrpc":"2.0",
          "id":3,
          "method":"tools/call",
          "params":{
            "name":"state_reservation_declare",
            "arguments":{
              "purpose":"Fix auth validation behavior.",
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
            "purpose": "Fix auth validation behavior.",
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
fn mcp_session_intent_lease_sequence_uses_omp_current_session_without_env() {
    let temp_root = temp_root("stateful-mcp-omp-session-sequence");
    let paths = GlobalPaths::new(temp_root.join("home"));
    let repo_root = temp_root.join("repo");
    fs::create_dir_all(repo_root.join("src")).expect("repo src should be creatable");
    enable_test_repo(&paths, &repo_root);
    let (runtime, rx) = spawn_fake_stateful_server_sequence(vec![
        r#"{"status":"ok"}"#,
        r#"{"status":"ok"}"#,
        r#"{"status":"ok"}"#,
    ]);
    write_global_runtime_file(&paths, &runtime).expect("global runtime file should write");
    let runtime_env = [
        ("STATEFUL_SERVER_URL", runtime.base_url.as_str()),
        ("STATEFUL_SERVER_TOKEN", runtime.token.as_str()),
    ];
    let current_session = CurrentSession::new("omp-actual-session", "w1");
    write_current_session_file_for_session(&repo_root, "omp-actual-session", &current_session)
        .expect("session-bound current session should write");
    write_legacy_current_session_for_test(&repo_root, &current_session);

    let register_response = run_mcp_jsonrpc_in_repo_with_env(
        &repo_root,
        &paths,
        &runtime_env,
        r#"{
          "jsonrpc":"2.0",
          "id":40,
          "method":"tools/call",
          "params":{
            "name":"state_session_register",
            "arguments":{}
          }
        }"#,
    );
    let register = rx.recv().expect("session register request should arrive");
    assert!(register.contains("POST /v1/session/register HTTP/1.1"));
    let register_body = request_json_body(&register);
    assert_eq!(register_body["session_id"], "omp-actual-session");
    assert_eq!(register_body["workspace_id"], "w1");
    let json: serde_json::Value =
        serde_json::from_str(&register_response).expect("register response should be json");
    assert_eq!(json["result"]["isError"], false);

    let intent_response = run_mcp_jsonrpc_in_repo_with_env(
        &repo_root,
        &paths,
        &runtime_env,
        r#"{
          "jsonrpc":"2.0",
          "id":41,
          "method":"tools/call",
          "params":{
            "name":"state_reservation_declare",
            "arguments":{
              "purpose":"Implement benchmark package files.",
              "files_planned":["src/auth.ts"]
            }
          }
        }"#,
    );
    let reservation = rx
        .recv()
        .expect("reservation declare request should arrive");
    assert!(reservation.contains("POST /v1/reservation/declare HTTP/1.1"));
    let reservation_body = request_json_body(&reservation);
    assert_eq!(
        reservation_body["session"]["session_id"],
        "omp-actual-session"
    );
    assert_eq!(reservation_body["workspace"]["workspace_id"], "w1");
    assert_eq!(
        reservation_body["payload"],
        serde_json::json!({
            "purpose": "Implement benchmark package files.",
            "files_planned": ["src/auth.ts"]
        })
    );
    let json: serde_json::Value =
        serde_json::from_str(&intent_response).expect("reservation response should be json");
    assert_eq!(json["result"]["isError"], false);

    let claim_response = run_mcp_jsonrpc_in_repo_with_env(
        &repo_root,
        &paths,
        &runtime_env,
        r#"{
          "jsonrpc":"2.0",
          "id":42,
          "method":"tools/call",
          "params":{
            "name":"state_claim_acquire",
            "arguments":{
              "path":"src/auth.ts"
            }
          }
        }"#,
    );
    let claim = rx.recv().expect("claim acquire request should arrive");
    assert!(claim.contains("POST /v1/claim/acquire HTTP/1.1"));
    let claim_body = request_json_body(&claim);
    assert_eq!(claim_body["session_id"], "omp-actual-session");
    assert_eq!(claim_body["workspace_id"], "w1");
    assert_eq!(claim_body["path"], "src/auth.ts");
    let json: serde_json::Value =
        serde_json::from_str(&claim_response).expect("claim response should be json");
    assert_eq!(json["result"]["isError"], false);

    fs::remove_dir_all(&temp_root).expect("temp root should be removable");
}

#[test]
fn mcp_reservation_declare_bootstraps_missing_session_bound_file_from_env() {
    let temp_root = temp_root("stateful-mcp-reservation-bootstrap-session");
    let paths = GlobalPaths::new(temp_root.join("home"));
    let repo_root = temp_root.join("repo");
    fs::create_dir_all(&repo_root).expect("repo root should be creatable");
    enable_test_repo(&paths, &repo_root);
    let (runtime, rx) = spawn_fake_stateful_server(r#"{"status":"ok"}"#);
    write_global_runtime_file(&paths, &runtime).expect("global runtime file should write");

    let response = run_mcp_jsonrpc_in_repo_with_env(
        &repo_root,
        &paths,
        &[("STATEFUL_SESSION_ID", "s-bootstrap")],
        r#"{
          "jsonrpc":"2.0",
          "id":30,
          "method":"tools/call",
          "params":{
            "name":"state_reservation_declare",
            "arguments":{
              "purpose":"Fix auth validation behavior.",
              "files_planned":["src/auth.ts"]
            }
          }
        }"#,
    );

    let request = rx
        .recv_timeout(Duration::from_secs(1))
        .expect("reservation declare request should arrive");
    assert!(request.contains("POST /v1/reservation/declare HTTP/1.1"));
    let body = request_json_body(&request);
    assert_eq!(body["session"]["session_id"], "s-bootstrap");
    assert_eq!(body["workspace"]["workspace_id"], "w1");
    assert_eq!(
        body["payload"],
        serde_json::json!({
            "purpose": "Fix auth validation behavior.",
            "files_planned": ["src/auth.ts"]
        })
    );
    assert!(
        repo_root
            .join(".stateful_core")
            .join("runtime")
            .join("sessions")
            .join("s-bootstrap.json")
            .is_file(),
        "session-bound current session file should be bootstrapped"
    );

    let json: serde_json::Value = serde_json::from_str(&response).expect("response should be json");
    assert_eq!(json["jsonrpc"], "2.0");
    assert_eq!(json["id"], 30);
    assert_eq!(json["result"]["isError"], false);

    fs::remove_dir_all(&temp_root).expect("temp root should be removable");
}

#[test]
fn mcp_reservation_declare_refuses_session_id_that_differs_from_current_session() {
    let temp_root = temp_root("stateful-mcp-reservation-session-mismatch");
    let paths = GlobalPaths::new(temp_root.join("home"));
    let repo_root = temp_root.join("repo");
    fs::create_dir_all(&repo_root).expect("repo root should be creatable");
    enable_test_repo(&paths, &repo_root);
    write_current_session_file_for_session(
        &repo_root,
        "s-current",
        &CurrentSession::new("s-current", "w1"),
    )
    .expect("session-bound current session should write");

    let response = run_mcp_jsonrpc_in_repo_with_env(
        &repo_root,
        &paths,
        &[("STATEFUL_SESSION_ID", "s-current")],
        r#"{
          "jsonrpc":"2.0",
          "id":4,
          "method":"tools/call",
          "params":{
            "name":"state_reservation_declare",
            "arguments":{
              "session_id":"s-other",
              "workspace_id":"w1",
              "purpose":"Fix auth validation behavior.",
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
fn mcp_lease_acquire_defaults_to_stateful_session_bound_file_over_legacy() {
    let temp_root = temp_root("stateful-mcp-session-claim-session");
    let paths = GlobalPaths::new(temp_root.join("home"));
    let repo_root = temp_root.join("repo");
    fs::create_dir_all(&repo_root).expect("repo root should be creatable");
    enable_test_repo(&paths, &repo_root);
    let (runtime, rx) = spawn_fake_stateful_server(r#"{"status":"ok"}"#);
    write_global_runtime_file(&paths, &runtime).expect("global runtime file should write");
    write_current_session_file_for_session(
        &repo_root,
        "session-a",
        &CurrentSession::new("session-a", "workspace-a"),
    )
    .expect("session-bound current session should write");
    write_legacy_current_session_for_test(
        &repo_root,
        &CurrentSession::new("session-b", "workspace-b"),
    );

    let response = run_mcp_jsonrpc_in_repo_with_env(
        &repo_root,
        &paths,
        &[("STATEFUL_SESSION_ID", "session-a")],
        r#"{
          "jsonrpc":"2.0",
          "id":9,
          "method":"tools/call",
          "params":{
            "name":"state_claim_acquire",
            "arguments":{
              "paths":["src/auth.ts", "src/session.ts"]
            }
          }
        }"#,
    );

    let request = rx
        .recv_timeout(Duration::from_secs(1))
        .expect("claim acquire request should arrive");
    assert!(request.contains("POST /v1/claim/acquire HTTP/1.1"));
    let body = request_json_body(&request);
    assert_eq!(body["session_id"], "session-a");
    assert_eq!(body["workspace_id"], "workspace-a");
    assert_eq!(
        body["paths"],
        serde_json::json!(["src/auth.ts", "src/session.ts"])
    );
    let canonical_repo_root = repo_root
        .canonicalize()
        .expect("repo root should canonicalize");
    assert_eq!(body["root"], canonical_repo_root.to_string_lossy().as_ref());

    let json: serde_json::Value = serde_json::from_str(&response).expect("response should be json");
    assert_eq!(json["jsonrpc"], "2.0");
    assert_eq!(json["id"], 9);
    assert_eq!(json["result"]["isError"], false);

    fs::remove_dir_all(&temp_root).expect("temp root should be removable");
}

#[test]
fn mcp_lease_acquire_prefers_codex_thread_id_over_stateful_session_id() {
    let temp_root = temp_root("stateful-mcp-session-claim-codex-thread");
    let paths = GlobalPaths::new(temp_root.join("home"));
    let repo_root = temp_root.join("repo");
    fs::create_dir_all(&repo_root).expect("repo root should be creatable");
    enable_test_repo(&paths, &repo_root);
    let (runtime, rx) = spawn_fake_stateful_server(r#"{"status":"ok"}"#);
    write_global_runtime_file(&paths, &runtime).expect("global runtime file should write");
    write_current_session_file_for_session(
        &repo_root,
        "session-a",
        &CurrentSession::new("session-a", "workspace-a"),
    )
    .expect("stateful env session-bound current session should write");
    write_current_session_file_for_session(
        &repo_root,
        "thread-a",
        &CurrentSession::new("thread-a", "workspace-thread"),
    )
    .expect("codex thread session-bound current session should write");
    write_legacy_current_session_for_test(
        &repo_root,
        &CurrentSession::new("session-a", "workspace-a"),
    );

    let response = run_mcp_jsonrpc_in_repo_with_env(
        &repo_root,
        &paths,
        &[
            ("STATEFUL_SESSION_ID", "session-a"),
            ("STATEFUL_CODEX_RUN_ID", "run-a"),
            ("CODEX_THREAD_ID", "thread-a"),
        ],
        r#"{
          "jsonrpc":"2.0",
          "id":15,
          "method":"tools/call",
          "params":{
            "name":"state_claim_acquire",
            "arguments":{
              "path":"src/auth.ts"
            }
          }
        }"#,
    );

    let request = rx
        .recv_timeout(Duration::from_secs(1))
        .expect("claim acquire request should arrive");
    assert!(request.contains("POST /v1/claim/acquire HTTP/1.1"));
    let body = request_json_body(&request);
    assert_eq!(body["session_id"], "thread-a");
    assert_eq!(body["workspace_id"], "workspace-thread");
    assert_eq!(body["path"], "src/auth.ts");

    let json: serde_json::Value = serde_json::from_str(&response).expect("response should be json");
    assert_eq!(json["jsonrpc"], "2.0");
    assert_eq!(json["id"], 15);
    assert_eq!(json["result"]["isError"], false);

    fs::remove_dir_all(&temp_root).expect("temp root should be removable");
}

#[test]
fn mcp_lease_acquire_prefers_verified_legacy_session_for_codex_run_when_thread_env_is_absent() {
    let temp_root = temp_root("stateful-mcp-session-claim-codex-run-legacy");
    let paths = GlobalPaths::new(temp_root.join("home"));
    let repo_root = temp_root.join("repo");
    fs::create_dir_all(&repo_root).expect("repo root should be creatable");
    enable_test_repo(&paths, &repo_root);
    let (runtime, rx) = spawn_fake_stateful_server(r#"{"status":"ok"}"#);
    write_global_runtime_file(&paths, &runtime).expect("global runtime file should write");
    write_current_session_file_for_session(
        &repo_root,
        "session-a",
        &CurrentSession::new("session-a", "workspace-a"),
    )
    .expect("stateful env session-bound current session should write");
    write_current_session_file_for_session(
        &repo_root,
        "thread-a",
        &CurrentSession::new("thread-a", "workspace-thread"),
    )
    .expect("codex thread session-bound current session should write");
    write_legacy_current_session_for_test(
        &repo_root,
        &CurrentSession::new("thread-a", "workspace-thread"),
    );

    let response = run_mcp_jsonrpc_in_repo_with_env(
        &repo_root,
        &paths,
        &[
            ("STATEFUL_SESSION_ID", "session-a"),
            ("STATEFUL_CODEX_RUN_ID", "run-a"),
        ],
        r#"{
          "jsonrpc":"2.0",
          "id":16,
          "method":"tools/call",
          "params":{
            "name":"state_claim_acquire",
            "arguments":{
              "path":"src/auth.ts"
            }
          }
        }"#,
    );

    let request = rx
        .recv_timeout(Duration::from_secs(1))
        .expect("claim acquire request should arrive");
    assert!(request.contains("POST /v1/claim/acquire HTTP/1.1"));
    let body = request_json_body(&request);
    assert_eq!(body["session_id"], "thread-a");
    assert_eq!(body["workspace_id"], "workspace-thread");
    assert_eq!(body["path"], "src/auth.ts");

    let json: serde_json::Value = serde_json::from_str(&response).expect("response should be json");
    assert_eq!(json["jsonrpc"], "2.0");
    assert_eq!(json["id"], 16);
    assert_eq!(json["result"]["isError"], false);

    fs::remove_dir_all(&temp_root).expect("temp root should be removable");
}

#[test]
fn mcp_lease_acquire_rejects_legacy_session_when_stateful_session_is_bound_elsewhere() {
    let temp_root = temp_root("stateful-mcp-session-claim-mismatch");
    let paths = GlobalPaths::new(temp_root.join("home"));
    let repo_root = temp_root.join("repo");
    fs::create_dir_all(&repo_root).expect("repo root should be creatable");
    enable_test_repo(&paths, &repo_root);
    let (runtime, rx) = spawn_fake_stateful_server(r#"{"status":"ok"}"#);
    write_global_runtime_file(&paths, &runtime).expect("global runtime file should write");
    write_current_session_file_for_session(
        &repo_root,
        "session-a",
        &CurrentSession::new("session-a", "workspace-a"),
    )
    .expect("session-bound current session should write");
    write_legacy_current_session_for_test(
        &repo_root,
        &CurrentSession::new("session-b", "workspace-b"),
    );

    let response = run_mcp_jsonrpc_in_repo_with_env(
        &repo_root,
        &paths,
        &[("STATEFUL_SESSION_ID", "session-a")],
        r#"{
          "jsonrpc":"2.0",
          "id":10,
          "method":"tools/call",
          "params":{
            "name":"state_claim_acquire",
            "arguments":{
              "session_id":"session-b",
              "workspace_id":"workspace-b",
              "path":"src/auth.ts"
            }
          }
        }"#,
    );

    let json: serde_json::Value = serde_json::from_str(&response).expect("response should be json");
    assert_eq!(json["jsonrpc"], "2.0");
    assert_eq!(json["id"], 10);
    assert_eq!(json["result"]["isError"], true);
    assert!(
        json["result"]["content"][0]["text"]
            .as_str()
            .unwrap_or_default()
            .contains(
                "state.claim.acquire cannot use session_id `session-b` while the current stateful session uses `session-a`"
            )
    );
    assert!(
        rx.recv_timeout(Duration::from_millis(200)).is_err(),
        "mismatched session-bound session should reject before HTTP"
    );

    fs::remove_dir_all(&temp_root).expect("temp root should be removable");
}

#[test]
fn mcp_lease_acquire_with_stateful_session_id_bootstraps_session_bound_file() {
    let temp_root = temp_root("stateful-mcp-session-missing-session");
    let paths = GlobalPaths::new(temp_root.join("home"));
    let repo_root = temp_root.join("repo");
    fs::create_dir_all(&repo_root).expect("repo root should be creatable");
    enable_test_repo(&paths, &repo_root);
    let (runtime, rx) = spawn_fake_stateful_server(r#"{"status":"ok"}"#);
    write_global_runtime_file(&paths, &runtime).expect("global runtime file should write");
    write_legacy_current_session_for_test(
        &repo_root,
        &CurrentSession::new("session-b", "workspace-b"),
    );

    let response = run_mcp_jsonrpc_in_repo_with_env(
        &repo_root,
        &paths,
        &[("STATEFUL_SESSION_ID", "missing-session")],
        r#"{
          "jsonrpc":"2.0",
          "id":11,
          "method":"tools/call",
          "params":{
            "name":"state_claim_acquire",
            "arguments":{
              "path":"src/auth.ts"
            }
          }
        }"#,
    );

    let request = rx
        .recv_timeout(Duration::from_secs(1))
        .expect("claim acquire request should arrive");
    assert!(request.contains("POST /v1/claim/acquire HTTP/1.1"));
    let body = request_json_body(&request);
    assert_eq!(body["session_id"], "missing-session");
    assert_eq!(body["workspace_id"], "w1");
    assert_eq!(body["path"], "src/auth.ts");
    assert!(
        repo_root
            .join(".stateful_core")
            .join("runtime")
            .join("sessions")
            .join("missing-session.json")
            .is_file(),
        "session-bound current session file should be bootstrapped"
    );

    let json: serde_json::Value = serde_json::from_str(&response).expect("response should be json");
    assert_eq!(json["jsonrpc"], "2.0");
    assert_eq!(json["id"], 11);
    assert_eq!(json["result"]["isError"], false);

    fs::remove_dir_all(&temp_root).expect("temp root should be removable");
}

#[test]
fn mcp_lease_acquire_uses_codex_thread_id_without_stateful_session_id() {
    let temp_root = temp_root("stateful-mcp-codex-thread-session");
    let paths = GlobalPaths::new(temp_root.join("home"));
    let repo_root = temp_root.join("repo");
    fs::create_dir_all(&repo_root).expect("repo root should be creatable");
    enable_test_repo(&paths, &repo_root);
    let (runtime, rx) = spawn_fake_stateful_server(r#"{"status":"ok"}"#);
    write_global_runtime_file(&paths, &runtime).expect("global runtime file should write");
    write_current_session_file_for_session(
        &repo_root,
        "thread-a",
        &CurrentSession::new("thread-a", "workspace-a"),
    )
    .expect("session-bound current session should write");
    write_current_session_file_for_session(
        &repo_root,
        "run-a",
        &CurrentSession::new("run-a", "workspace-run"),
    )
    .expect("run-bound current session should write");
    write_legacy_current_session_for_test(
        &repo_root,
        &CurrentSession::new("legacy-a", "workspace-legacy"),
    );

    let response = run_mcp_jsonrpc_in_repo_with_env(
        &repo_root,
        &paths,
        &[
            ("STATEFUL_CODEX_RUN_ID", "run-a"),
            ("CODEX_THREAD_ID", "thread-a"),
        ],
        r#"{
          "jsonrpc":"2.0",
          "id":12,
          "method":"tools/call",
          "params":{
            "name":"state_claim_acquire",
            "arguments":{
              "path":"src/auth.ts"
            }
          }
        }"#,
    );

    let request = rx
        .recv_timeout(Duration::from_secs(1))
        .expect("claim acquire request should arrive");
    assert!(request.contains("POST /v1/claim/acquire HTTP/1.1"));
    let body = request_json_body(&request);
    assert_eq!(body["session_id"], "thread-a");
    assert_eq!(body["workspace_id"], "workspace-a");
    assert_eq!(body["path"], "src/auth.ts");

    let json: serde_json::Value = serde_json::from_str(&response).expect("response should be json");
    assert_eq!(json["jsonrpc"], "2.0");
    assert_eq!(json["id"], 12);
    assert_eq!(json["result"]["isError"], false);

    fs::remove_dir_all(&temp_root).expect("temp root should be removable");
}

#[test]
fn mcp_lease_acquire_without_stateful_session_id_uses_verified_legacy_session() {
    let temp_root = temp_root("stateful-mcp-session-env-missing-current-session");
    let paths = GlobalPaths::new(temp_root.join("home"));
    let repo_root = temp_root.join("repo");
    fs::create_dir_all(&repo_root).expect("repo root should be creatable");
    enable_test_repo(&paths, &repo_root);
    let (runtime, rx) = spawn_fake_stateful_server(r#"{"status":"ok"}"#);
    write_global_runtime_file(&paths, &runtime).expect("global runtime file should write");
    write_current_session_file_for_session(
        &repo_root,
        "session-a",
        &CurrentSession::new("session-a", "workspace-a"),
    )
    .expect("session-bound current session should write");
    write_legacy_current_session_for_test(
        &repo_root,
        &CurrentSession::new("session-a", "workspace-a"),
    );

    let response = run_mcp_jsonrpc_in_repo(
        &repo_root,
        &paths,
        r#"{
          "jsonrpc":"2.0",
          "id":13,
          "method":"tools/call",
          "params":{
            "name":"state_claim_acquire",
            "arguments":{
              "path":"src/auth.ts"
            }
          }
        }"#,
    );

    let request = rx
        .recv_timeout(Duration::from_secs(1))
        .expect("claim acquire request should arrive");
    assert!(request.contains("POST /v1/claim/acquire HTTP/1.1"));
    let body = request_json_body(&request);
    assert_eq!(body["session_id"], "session-a");
    assert_eq!(body["workspace_id"], "workspace-a");
    assert_eq!(body["path"], "src/auth.ts");

    let json: serde_json::Value = serde_json::from_str(&response).expect("response should be json");
    assert_eq!(json["jsonrpc"], "2.0");
    assert_eq!(json["id"], 13);
    assert_eq!(json["result"]["isError"], false);

    fs::remove_dir_all(&temp_root).expect("temp root should be removable");
}

#[test]
fn mcp_lease_acquire_without_stateful_session_id_uses_verified_legacy_session_with_stale_sibling() {
    let temp_root = temp_root("stateful-mcp-session-ambiguous-fallback");
    let paths = GlobalPaths::new(temp_root.join("home"));
    let repo_root = temp_root.join("repo");
    fs::create_dir_all(&repo_root).expect("repo root should be creatable");
    enable_test_repo(&paths, &repo_root);
    let (runtime, rx) = spawn_fake_stateful_server(r#"{"status":"ok"}"#);
    write_global_runtime_file(&paths, &runtime).expect("global runtime file should write");
    write_current_session_file_for_session(
        &repo_root,
        "session-a",
        &CurrentSession::new("session-a", "workspace-a"),
    )
    .expect("first session-bound current session should write");
    write_current_session_file_for_session(
        &repo_root,
        "session-b",
        &CurrentSession::new("session-b", "workspace-b"),
    )
    .expect("second session-bound current session should write");
    write_legacy_current_session_for_test(
        &repo_root,
        &CurrentSession::new("session-a", "workspace-a"),
    );

    let response = run_mcp_jsonrpc_in_repo(
        &repo_root,
        &paths,
        r#"{
          "jsonrpc":"2.0",
          "id":14,
          "method":"tools/call",
          "params":{
            "name":"state_claim_acquire",
            "arguments":{
              "path":"src/auth.ts"
            }
          }
        }"#,
    );

    let request = rx
        .recv_timeout(Duration::from_secs(1))
        .expect("claim acquire request should arrive");
    assert!(request.contains("POST /v1/claim/acquire HTTP/1.1"));
    let body = request_json_body(&request);
    assert_eq!(body["session_id"], "session-a");
    assert_eq!(body["workspace_id"], "workspace-a");
    assert_eq!(body["path"], "src/auth.ts");

    let json: serde_json::Value = serde_json::from_str(&response).expect("response should be json");
    assert_eq!(json["jsonrpc"], "2.0");
    assert_eq!(json["id"], 14);
    assert_eq!(json["result"]["isError"], false);

    fs::remove_dir_all(&temp_root).expect("temp root should be removable");
}

#[test]
fn mcp_lease_acquire_without_stateful_session_id_rejects_unverified_legacy_session_before_http() {
    let temp_root = temp_root("stateful-mcp-session-required");
    let paths = GlobalPaths::new(temp_root.join("home"));
    let repo_root = temp_root.join("repo");
    fs::create_dir_all(&repo_root).expect("repo root should be creatable");
    enable_test_repo(&paths, &repo_root);
    let (runtime, rx) = spawn_fake_stateful_server(r#"{"status":"ok"}"#);
    write_global_runtime_file(&paths, &runtime).expect("global runtime file should write");
    write_legacy_current_session_for_test(
        &repo_root,
        &CurrentSession::new("session-b", "workspace-b"),
    );

    let response = run_mcp_jsonrpc_in_repo(
        &repo_root,
        &paths,
        r#"{
          "jsonrpc":"2.0",
          "id":13,
          "method":"tools/call",
          "params":{
            "name":"state_claim_acquire",
            "arguments":{
              "path":"src/auth.ts"
            }
          }
        }"#,
    );

    let json: serde_json::Value = serde_json::from_str(&response).expect("response should be json");
    assert_eq!(json["jsonrpc"], "2.0");
    assert_eq!(json["id"], 13);
    assert_eq!(json["result"]["isError"], true);
    assert!(
        json["result"]["content"][0]["text"]
            .as_str()
            .unwrap_or_default()
            .contains("has no matching session-bound file")
    );
    assert!(
        rx.recv_timeout(Duration::from_millis(200)).is_err(),
        "unverified legacy session should fail before HTTP"
    );

    fs::remove_dir_all(&temp_root).expect("temp root should be removable");
}

#[test]
fn mcp_tools_call_for_reservation_claim_posts_to_state_server() {
    let temp_root = temp_root("stateful-mcp-reservation-claim");
    let paths = GlobalPaths::new(temp_root.join("home"));
    let repo_root = temp_root.join("repo");
    fs::create_dir_all(&repo_root).expect("repo root should be creatable");
    enable_test_repo(&paths, &repo_root);
    let (runtime, rx) = spawn_fake_stateful_server(r#"{"status":"ok"}"#);
    write_global_runtime_file(&paths, &runtime).expect("global runtime file should write");
    write_current_session_file_for_session(&repo_root, "s1", &CurrentSession::new("s1", "w1"))
        .expect("session-bound current session should write");

    let response = run_mcp_jsonrpc_in_repo_with_env(
        &repo_root,
        &paths,
        &[("STATEFUL_SESSION_ID", "s1")],
        r#"{
          "jsonrpc":"2.0",
          "id":5,
          "method":"tools/call",
          "params":{
            "name":"state_reservation_claim",
            "arguments":{
              "session_id":"s1",
              "workspace_id":"w1",
              "wait_id":"wait-1"
            }
          }
        }"#,
    );

    let request = rx.recv().expect("captured request should arrive");
    assert!(request.contains("POST /v1/reservation/claim HTTP/1.1"));
    assert!(request.contains("Authorization: Bearer secret-token"));
    let body = request_json_body(&request);
    assert_eq!(body["protocol_version"], "stateful.v1");
    assert_eq!(body["session"]["session_id"], "s1");
    assert_eq!(body["workspace"]["workspace_id"], "w1");
    assert_eq!(body["source"]["kind"], "mcp");
    assert_eq!(body["source"]["event"], "reservation_claim");
    assert_eq!(body["source"]["source_ref"], "state.reservation.claim");
    assert_eq!(
        body["payload"],
        serde_json::json!({
            "wait_id": "wait-1"
        })
    );

    let json: serde_json::Value = serde_json::from_str(&response).expect("response should be json");
    assert_eq!(json["jsonrpc"], "2.0");
    assert_eq!(json["id"], 5);
    assert_eq!(json["result"]["isError"], false);

    fs::remove_dir_all(&temp_root).expect("temp root should be removable");
}

#[test]
fn mcp_tools_call_for_reservation_request_posts_to_state_server() {
    let temp_root = temp_root("stateful-mcp-reservation-request");
    let paths = GlobalPaths::new(temp_root.join("home"));
    let repo_root = temp_root.join("repo");
    fs::create_dir_all(&repo_root).expect("repo root should be creatable");
    enable_test_repo(&paths, &repo_root);
    let (runtime, rx) = spawn_fake_stateful_server(r#"{"status":"ok"}"#);
    write_global_runtime_file(&paths, &runtime).expect("global runtime file should write");
    write_current_session_file_for_session(&repo_root, "s1", &CurrentSession::new("s1", "w1"))
        .expect("session-bound current session should write");

    let response = run_mcp_jsonrpc_in_repo_with_env(
        &repo_root,
        &paths,
        &[("STATEFUL_SESSION_ID", "s1")],
        r#"{
          "jsonrpc":"2.0",
          "id":6,
          "method":"tools/call",
          "params":{
            "name":"state_reservation_request",
            "arguments":{
              "session_id":"s1",
              "workspace_id":"w1",
              "request_id":"request-1",
              "action":"write_file",
              "path":"src/auth.ts",
              "purpose":"Queue auth file changes."
            }
          }
        }"#,
    );

    let request = rx.recv().expect("captured request should arrive");
    assert!(request.contains("POST /v1/reservation/request HTTP/1.1"));
    assert!(request.contains("Authorization: Bearer secret-token"));
    let body = request_json_body(&request);
    assert_eq!(body["protocol_version"], "stateful.v1");
    assert_eq!(body["session"]["session_id"], "s1");
    assert_eq!(body["workspace"]["workspace_id"], "w1");
    assert_eq!(body["source"]["kind"], "mcp");
    assert_eq!(body["source"]["event"], "reservation_request");
    assert_eq!(body["source"]["source_ref"], "state.reservation.request");
    assert_eq!(
        body["payload"],
        serde_json::json!({
            "request_id": "request-1",
            "action": "write_file",
            "path": "src/auth.ts",
            "purpose": "Queue auth file changes."
        })
    );

    let json: serde_json::Value = serde_json::from_str(&response).expect("response should be json");
    assert_eq!(json["jsonrpc"], "2.0");
    assert_eq!(json["id"], 6);
    assert_eq!(json["result"]["isError"], false);

    fs::remove_dir_all(&temp_root).expect("temp root should be removable");
}

#[test]
fn mcp_tools_call_for_reservation_cancel_posts_to_state_server() {
    let temp_root = temp_root("stateful-mcp-reservation-cancel");
    let paths = GlobalPaths::new(temp_root.join("home"));
    let repo_root = temp_root.join("repo");
    fs::create_dir_all(&repo_root).expect("repo root should be creatable");
    enable_test_repo(&paths, &repo_root);
    let (runtime, rx) = spawn_fake_stateful_server(r#"{"status":"ok"}"#);
    write_global_runtime_file(&paths, &runtime).expect("global runtime file should write");
    write_current_session_file_for_session(&repo_root, "s1", &CurrentSession::new("s1", "w1"))
        .expect("session-bound current session should write");

    let response = run_mcp_jsonrpc_in_repo_with_env(
        &repo_root,
        &paths,
        &[("STATEFUL_SESSION_ID", "s1")],
        r#"{
          "jsonrpc":"2.0",
          "id":7,
          "method":"tools/call",
          "params":{
            "name":"state_reservation_cancel",
            "arguments":{
              "session_id":"s1",
              "workspace_id":"w1",
              "request_id":"request-1"
            }
          }
        }"#,
    );

    let request = rx.recv().expect("captured request should arrive");
    assert!(request.contains("POST /v1/reservation/cancel HTTP/1.1"));
    assert!(request.contains("Authorization: Bearer secret-token"));
    let body = request_json_body(&request);
    assert_eq!(body["protocol_version"], "stateful.v1");
    assert_eq!(body["session"]["session_id"], "s1");
    assert_eq!(body["workspace"]["workspace_id"], "w1");
    assert_eq!(body["source"]["kind"], "mcp");
    assert_eq!(body["source"]["event"], "reservation_cancel");
    assert_eq!(body["source"]["source_ref"], "state.reservation.cancel");
    assert_eq!(
        body["payload"],
        serde_json::json!({
            "request_id": "request-1"
        })
    );

    let json: serde_json::Value = serde_json::from_str(&response).expect("response should be json");
    assert_eq!(json["jsonrpc"], "2.0");
    assert_eq!(json["id"], 7);
    assert_eq!(json["result"]["isError"], false);

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

fn shell_quote_path(path: &std::path::Path) -> String {
    let value = path.to_string_lossy();
    let mut quoted = String::from("'");
    for ch in value.chars() {
        if ch == '\'' {
            quoted.push_str("'\\''");
        } else {
            quoted.push(ch);
        }
    }
    quoted.push('\'');
    quoted
}

fn temp_root(name: &str) -> std::path::PathBuf {
    let root = std::env::temp_dir().join(format!("{name}-{}", std::process::id()));
    if root.exists() {
        fs::remove_dir_all(&root).expect("old temp root should be removable");
    }
    root
}

fn external_temp_root(name: &str) -> PathBuf {
    let base = if cfg!(target_os = "macos") {
        PathBuf::from("/private/tmp")
    } else {
        std::env::temp_dir()
            .canonicalize()
            .unwrap_or_else(|_| std::env::temp_dir())
    };
    let root = base.join(format!("{name}-{}", std::process::id()));
    if root.exists() {
        fs::remove_dir_all(&root).expect("old temp root should be removable");
    }
    root
}

fn macos_stateful_sandbox_is_active() -> bool {
    if cfg!(target_os = "macos") && std::env::var_os("STATEFUL_SANDBOX_RUN_ACTIVE").is_some() {
        return true;
    }

    cfg!(target_os = "linux")
        && !Path::new("/usr/bin/bwrap").is_file()
        && !Path::new("/bin/bwrap").is_file()
}

fn enable_test_repo(paths: &GlobalPaths, repo_root: &std::path::Path) {
    fs::create_dir_all(repo_root.join(".git")).expect("git marker should write");
    enable_repo(paths, repo_root).expect("repo should enable");
}

fn write_legacy_current_session_for_test(repo_root: &std::path::Path, session: &CurrentSession) {
    let runtime_dir = repo_root.join(".stateful_core").join("runtime");
    fs::create_dir_all(&runtime_dir).expect("runtime dir should be creatable");
    fs::write(
        runtime_dir.join("session.json"),
        serde_json::to_string_pretty(session).expect("session should serialize"),
    )
    .expect("legacy current session should write");
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
            } else if health_seen && request.contains("GET /v1/current HTTP/1.1") && !current_seen {
                current_seen = true;
                write_json_response(&mut stream, r#"{"status":"ok","current":{}}"#);
            } else if request.contains("GET /v1/runtime/identity HTTP/1.1") {
                write_json_response(
                    &mut stream,
                    r#"{"status":"ok","pid":42,"protocol_version":"stateful.v1","capabilities":["authorize.write_directory"]}"#,
                );
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
    run_mcp_jsonrpc_in_repo_with_env(repo_root, paths, &[], message)
}

fn run_mcp_jsonrpc_in_repo_with_env(
    repo_root: &std::path::Path,
    paths: &GlobalPaths,
    env: &[(&str, &str)],
    message: &str,
) -> String {
    let mut command = Command::new(env!("CARGO_BIN_EXE_stateful"));
    command
        .args(["mcp", "serve"])
        .current_dir(repo_root)
        .env_clear()
        .env("STATEFUL_HOME", &paths.home);
    for (key, value) in env {
        command.env(key, value);
    }
    let mut child = command
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
