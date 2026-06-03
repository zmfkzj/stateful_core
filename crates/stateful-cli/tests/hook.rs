use std::{
    fs,
    io::{Read, Write},
    net::TcpListener,
    process::{Command, Stdio},
    sync::mpsc,
    thread,
};

use stateful_cli::{
    GlobalPaths, HookOutcome, ServerRuntime, handle_post_tool_use_in_repo, handle_pre_tool_use,
    handle_pre_tool_use_in_repo, handle_session_start_in_repo, handle_stop_in_repo,
    handle_user_prompt_submit_in_repo, read_current_session_file, write_global_runtime_file,
    write_runtime_file,
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
    fs::create_dir_all(&temp_root).expect("temp root should be creatable");
    let runtime = ServerRuntime::new("http://127.0.0.1:9", "secret-token", "w1", 42);
    write_runtime_file(&temp_root, &runtime).expect("runtime file should write");

    let input = r#"{
      "session_id": "s-current",
      "cwd": "/repo",
      "hook_event_name": "PreToolUse",
      "tool_name": "Bash",
      "tool_input": {
        "command": "rg auth src"
      }
    }"#;

    let outcome = handle_pre_tool_use_in_repo(input, &temp_root).expect("hook input should parse");

    assert_eq!(outcome, HookOutcome::Allow);
    let session = read_current_session_file(&temp_root).expect("current session should read");
    assert_eq!(session.session_id, "s-current");
    assert_eq!(session.workspace_id, "w1");

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
fn pre_tool_use_apply_patch_posts_authorize_and_allows_when_server_allows() {
    let temp_root = std::env::temp_dir().join(format!("stateful-hook-test-{}", std::process::id()));
    if temp_root.exists() {
        fs::remove_dir_all(&temp_root).expect("old temp root should be removable");
    }
    fs::create_dir_all(&temp_root).expect("temp root should be creatable");

    let listener = TcpListener::bind("127.0.0.1:0").expect("listener should bind");
    let addr = listener.local_addr().expect("listener addr should load");
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("connection should arrive");
        let request = read_http_request(&mut stream);
        tx.send(request).expect("request should send to test");
        write_json_response(
            &mut stream,
            r#"{"decision":"allow","reason_code":"authorized","message":"ok","required_next_action":null}"#,
        );
    });

    let runtime = ServerRuntime::new(format!("http://{addr}"), "secret-token", "w1", 42);
    write_runtime_file(&temp_root, &runtime).expect("runtime file should write");

    let input = r#"{
      "session_id": "s1",
      "cwd": "/repo",
      "hook_event_name": "PreToolUse",
      "tool_name": "apply_patch",
      "tool_input": {
        "command": "*** Begin Patch\n*** Update File: src/auth.ts\n*** End Patch\n"
      }
    }"#;

    let outcome = handle_pre_tool_use_in_repo(input, &temp_root).expect("hook input should parse");

    assert_eq!(outcome, HookOutcome::Allow);
    let request = rx.recv().expect("captured request should arrive");
    assert!(request.contains("POST /v1/authorize HTTP/1.1"));
    assert!(request.contains("Authorization: Bearer secret-token"));
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

    let listener = TcpListener::bind("127.0.0.1:0").expect("listener should bind");
    let addr = listener.local_addr().expect("listener addr should load");
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("connection should arrive");
        let request = read_http_request(&mut stream);
        tx.send(request).expect("request should send to test");
        write_json_response(
            &mut stream,
            r#"{"decision":"allow","reason_code":"authorized","message":"ok","required_next_action":null}"#,
        );
    });

    let runtime = ServerRuntime::new(format!("http://{addr}"), "secret-token", "global-w", 42);
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
    child
        .stdin
        .as_mut()
        .expect("stdin should be piped")
        .write_all(input.as_bytes())
        .expect("hook input should write");
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
    assert!(request.contains("\"workspace_id\":\"global-w\""));

    fs::remove_dir_all(&temp_root).expect("temp root should be removable");
}

#[test]
fn pre_tool_use_apply_patch_denies_when_server_denies() {
    let temp_root =
        std::env::temp_dir().join(format!("stateful-hook-deny-test-{}", std::process::id()));
    if temp_root.exists() {
        fs::remove_dir_all(&temp_root).expect("old temp root should be removable");
    }
    fs::create_dir_all(&temp_root).expect("temp root should be creatable");

    let listener = TcpListener::bind("127.0.0.1:0").expect("listener should bind");
    let addr = listener.local_addr().expect("listener addr should load");
    thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("connection should arrive");
        let _request = read_http_request(&mut stream);
        write_json_response(
            &mut stream,
            r#"{"decision":"deny","reason_code":"scope_mismatch","message":"Write target is outside active intent scope.","required_next_action":"Declare matching intent."}"#,
        );
    });

    let runtime = ServerRuntime::new(format!("http://{addr}"), "secret-token", "w1", 42);
    write_runtime_file(&temp_root, &runtime).expect("runtime file should write");

    let input = r#"{
      "session_id": "s1",
      "cwd": "/repo",
      "hook_event_name": "PreToolUse",
      "tool_name": "apply_patch",
      "tool_input": {
        "command": "*** Begin Patch\n*** Update File: src/auth.ts\n*** End Patch\n"
      }
    }"#;

    let outcome = handle_pre_tool_use_in_repo(input, &temp_root).expect("hook input should parse");

    assert!(matches!(outcome, HookOutcome::Deny { .. }));
    let json = outcome
        .to_stdout_json()
        .expect("deny outcome should serialize");
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
    fs::create_dir_all(&temp_root).expect("temp root should be creatable");

    let listener = TcpListener::bind("127.0.0.1:0").expect("listener should bind");
    let addr = listener.local_addr().expect("listener addr should load");
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("connection should arrive");
        let request = read_http_request(&mut stream);
        tx.send(request).expect("request should send to test");
        write_json_response(
            &mut stream,
            r#"{"decision":"allow","reason_code":"authorized","message":"ok","required_next_action":null}"#,
        );
    });

    let runtime = ServerRuntime::new(format!("http://{addr}"), "secret-token", "w1", 42);
    write_runtime_file(&temp_root, &runtime).expect("runtime file should write");

    let input = r#"{
      "session_id": "s1",
      "cwd": "/repo",
      "hook_event_name": "PreToolUse",
      "tool_name": "apply_patch",
      "tool_input": {
        "command": "*** Begin Patch\n*** Delete File: src/auth.ts\n*** End Patch\n"
      }
    }"#;

    let outcome = handle_pre_tool_use_in_repo(input, &temp_root).expect("hook input should parse");

    assert_eq!(outcome, HookOutcome::Allow);
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
    fs::create_dir_all(&temp_root).expect("temp root should be creatable");

    let listener = TcpListener::bind("127.0.0.1:0").expect("listener should bind");
    let addr = listener.local_addr().expect("listener addr should load");
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("connection should arrive");
        let request = read_http_request(&mut stream);
        tx.send(request).expect("request should send to test");
        write_json_response(&mut stream, r#"{"status":"ok"}"#);
    });

    let runtime = ServerRuntime::new(format!("http://{addr}"), "secret-token", "w1", 42);
    write_runtime_file(&temp_root, &runtime).expect("runtime file should write");

    let input = r#"{
      "session_id": "s1",
      "hook_event_name": "SessionStart"
    }"#;

    handle_session_start_in_repo(input, &temp_root).expect("session start should register");

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
    fs::create_dir_all(&temp_root).expect("temp root should be creatable");

    let listener = TcpListener::bind("127.0.0.1:0").expect("listener should bind");
    let addr = listener.local_addr().expect("listener addr should load");
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("connection should arrive");
        let request = read_http_request(&mut stream);
        tx.send(request).expect("request should send to test");
        write_json_response(&mut stream, r#"{"status":"ok"}"#);
    });

    let runtime = ServerRuntime::new(format!("http://{addr}"), "secret-token", "w1", 42);
    write_runtime_file(&temp_root, &runtime).expect("runtime file should write");

    let input = r#"{
      "session_id": "s1",
      "hook_event_name": "PostToolUse",
      "tool_name": "Bash",
      "tool_input": {"command": "rg auth src"}
    }"#;

    handle_post_tool_use_in_repo(input, &temp_root).expect("post tool use should heartbeat");

    let request = rx.recv().expect("captured request should arrive");
    assert!(request.contains("POST /v1/session/heartbeat HTTP/1.1"));
    assert!(request.contains("\"session_id\":\"s1\""));
    assert!(request.contains("\"workspace_id\":\"w1\""));

    fs::remove_dir_all(&temp_root).expect("temp root should be removable");
}

#[test]
fn post_tool_use_queues_outbox_when_server_is_unavailable() {
    let temp_root =
        std::env::temp_dir().join(format!("stateful-hook-outbox-test-{}", std::process::id()));
    if temp_root.exists() {
        fs::remove_dir_all(&temp_root).expect("old temp root should be removable");
    }
    fs::create_dir_all(&temp_root).expect("temp root should be creatable");

    let listener = TcpListener::bind("127.0.0.1:0").expect("listener should bind");
    let addr = listener.local_addr().expect("listener addr should load");
    drop(listener);
    let runtime = ServerRuntime::new(format!("http://{addr}"), "secret-token", "w1", 42);
    write_runtime_file(&temp_root, &runtime).expect("runtime file should write");

    let input = r#"{
      "session_id": "s1",
      "hook_event_name": "PostToolUse",
      "tool_name": "Bash",
      "tool_input": {"command": "rg auth src"}
    }"#;

    handle_post_tool_use_in_repo(input, &temp_root).expect("post tool use should queue outbox");

    let outbox = fs::read_to_string(temp_root.join(".stateful_core/outbox/s1.jsonl"))
        .expect("outbox file should exist");
    assert!(outbox.contains("\"event_type\":\"SessionHeartbeatQueued\""));
    assert!(outbox.contains("\"sync_status\":\"pending\""));
    assert!(outbox.contains("\"session_id\":\"s1\""));
    assert!(outbox.contains("\"workspace_id\":\"w1\""));

    fs::remove_dir_all(&temp_root).expect("temp root should be removable");
}

#[test]
fn user_prompt_submit_posts_context_render() {
    let temp_root =
        std::env::temp_dir().join(format!("stateful-hook-prompt-test-{}", std::process::id()));
    if temp_root.exists() {
        fs::remove_dir_all(&temp_root).expect("old temp root should be removable");
    }
    fs::create_dir_all(&temp_root).expect("temp root should be creatable");

    let listener = TcpListener::bind("127.0.0.1:0").expect("listener should bind");
    let addr = listener.local_addr().expect("listener addr should load");
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("connection should arrive");
        let request = read_http_request(&mut stream);
        tx.send(request).expect("request should send to test");
        write_json_response(
            &mut stream,
            r#"{"status":"ok","prompt_text":"Nearby Activity\n- none"}"#,
        );
    });

    let runtime = ServerRuntime::new(format!("http://{addr}"), "secret-token", "w1", 42);
    write_runtime_file(&temp_root, &runtime).expect("runtime file should write");

    let input = r#"{
      "session_id": "s1",
      "hook_event_name": "UserPromptSubmit",
      "prompt": "work on auth"
    }"#;

    let rendered =
        handle_user_prompt_submit_in_repo(input, &temp_root).expect("prompt submit should render");

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
    fs::create_dir_all(&temp_root).expect("temp root should be creatable");

    let listener = TcpListener::bind("127.0.0.1:0").expect("listener should bind");
    let addr = listener.local_addr().expect("listener addr should load");
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("connection should arrive");
        let request = read_http_request(&mut stream);
        tx.send(request).expect("request should send to test");
        write_json_response(&mut stream, r#"{"status":"ok"}"#);
    });

    let runtime = ServerRuntime::new(format!("http://{addr}"), "secret-token", "w1", 42);
    write_runtime_file(&temp_root, &runtime).expect("runtime file should write");

    let input = r#"{
      "session_id": "s1",
      "hook_event_name": "Stop"
    }"#;

    handle_stop_in_repo(input, &temp_root).expect("stop should finalize activity");

    let request = rx.recv().expect("captured request should arrive");
    assert!(request.contains("POST /v1/activity/finalize HTTP/1.1"));
    assert!(request.contains("\"session_id\":\"s1\""));
    assert!(request.contains("\"workspace_id\":\"w1\""));

    fs::remove_dir_all(&temp_root).expect("temp root should be removable");
}

fn read_http_request(stream: &mut std::net::TcpStream) -> String {
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
        .expect("content length should exist")
        .parse::<usize>()
        .expect("content length should parse");

    let mut body = vec![0_u8; content_length];
    stream
        .read_exact(&mut body)
        .expect("request body should read");
    buffer.extend_from_slice(&body);

    String::from_utf8(buffer).expect("request should be utf8")
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
