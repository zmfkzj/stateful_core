use std::{
    fs,
    io::{Read, Write},
    net::TcpListener,
    process::Command,
    sync::mpsc,
    thread,
};

use stateful_cli::{
    CurrentSession, GlobalPaths, IntentDeclareArgs, ServerRuntime, declare_intent_via_http,
    discover_runtime, discover_runtime_with_global, get_json, global_state_db_path, post_json,
    read_current_session_file, state_db_path, write_current_session_file,
    write_global_runtime_file, write_runtime_file,
};

#[test]
fn runtime_file_round_trips_server_discovery() {
    let temp_root =
        std::env::temp_dir().join(format!("stateful-runtime-test-{}", std::process::id()));
    if temp_root.exists() {
        fs::remove_dir_all(&temp_root).expect("old temp root should be removable");
    }
    fs::create_dir_all(&temp_root).expect("temp root should be creatable");

    let runtime = ServerRuntime::new("http://127.0.0.1:43873", "secret-token", "w1", 42);
    write_runtime_file(&temp_root, &runtime).expect("runtime file should write");

    let discovered = discover_runtime(&temp_root).expect("runtime should be discoverable");

    assert_eq!(discovered.base_url, "http://127.0.0.1:43873");
    assert_eq!(discovered.token, "secret-token");
    assert_eq!(discovered.workspace_id, "w1");

    fs::remove_dir_all(&temp_root).expect("temp root should be removable");
}

#[test]
fn global_runtime_file_round_trips_server_discovery() {
    let temp_root = std::env::temp_dir().join(format!(
        "stateful-global-runtime-test-{}",
        std::process::id()
    ));
    if temp_root.exists() {
        fs::remove_dir_all(&temp_root).expect("old temp root should be removable");
    }
    fs::create_dir_all(&temp_root).expect("temp root should be creatable");
    let repo_root = temp_root.join("repo");
    let paths = GlobalPaths::new(temp_root.join("home"));

    let runtime = ServerRuntime::new("http://127.0.0.1:43874", "global-token", "global-w", 43);
    write_global_runtime_file(&paths, &runtime).expect("global runtime file should write");

    let discovered =
        discover_runtime_with_global(&repo_root, &paths).expect("global runtime should discover");

    assert_eq!(discovered.base_url, "http://127.0.0.1:43874");
    assert_eq!(discovered.token, "global-token");
    assert_eq!(discovered.workspace_id, "global-w");

    fs::remove_dir_all(&temp_root).expect("temp root should be removable");
}

#[test]
fn runtime_discovery_keeps_repo_local_compatibility_fallback() {
    let temp_root = std::env::temp_dir().join(format!(
        "stateful-runtime-compat-test-{}",
        std::process::id()
    ));
    if temp_root.exists() {
        fs::remove_dir_all(&temp_root).expect("old temp root should be removable");
    }
    fs::create_dir_all(&temp_root).expect("temp root should be creatable");
    let repo_root = temp_root.join("repo");
    let paths = GlobalPaths::new(temp_root.join("home"));

    let runtime = ServerRuntime::new("http://127.0.0.1:43875", "repo-token", "repo-w", 44);
    write_runtime_file(&repo_root, &runtime).expect("repo runtime file should write");

    let discovered =
        discover_runtime_with_global(&repo_root, &paths).expect("repo runtime should discover");

    assert_eq!(discovered.base_url, "http://127.0.0.1:43875");
    assert_eq!(discovered.token, "repo-token");
    assert_eq!(discovered.workspace_id, "repo-w");

    fs::remove_dir_all(&temp_root).expect("temp root should be removable");
}

#[test]
fn cli_current_uses_repo_local_runtime_when_global_paths_are_unavailable() {
    let temp_root = std::env::temp_dir().join(format!(
        "stateful-runtime-no-home-test-{}",
        std::process::id()
    ));
    if temp_root.exists() {
        fs::remove_dir_all(&temp_root).expect("old temp root should be removable");
    }
    fs::create_dir_all(&temp_root).expect("temp root should be creatable");

    let listener = TcpListener::bind("127.0.0.1:0").expect("listener should bind");
    let addr = listener.local_addr().expect("listener addr should load");
    thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("connection should arrive");
        let _request = read_http_request_without_body(&mut stream);
        stream
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 15\r\n\r\n{\"status\":\"ok\"}")
            .expect("response should write");
    });

    let runtime = ServerRuntime::new(format!("http://{addr}"), "secret-token", "w1", 42);
    write_runtime_file(&temp_root, &runtime).expect("runtime file should write");

    let output = Command::new(env!("CARGO_BIN_EXE_stateful"))
        .arg("current")
        .current_dir(&temp_root)
        .env_clear()
        .output()
        .expect("stateful current should run");

    assert!(
        output.status.success(),
        "stateful current failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("\"status\":\"ok\""));

    fs::remove_dir_all(&temp_root).expect("temp root should be removable");
}

#[test]
fn current_session_file_round_trips_for_mcp_enrichment() {
    let temp_root = std::env::temp_dir().join(format!(
        "stateful-current-session-test-{}",
        std::process::id()
    ));
    if temp_root.exists() {
        fs::remove_dir_all(&temp_root).expect("old temp root should be removable");
    }
    fs::create_dir_all(&temp_root).expect("temp root should be creatable");

    write_current_session_file(&temp_root, &CurrentSession::new("s1", "w1"))
        .expect("current session file should write");

    let session = read_current_session_file(&temp_root).expect("current session should read");
    assert_eq!(session.session_id, "s1");
    assert_eq!(session.workspace_id, "w1");

    fs::remove_dir_all(&temp_root).expect("temp root should be removable");
}

#[test]
fn state_db_path_uses_local_runtime_directory() {
    let temp_root =
        std::env::temp_dir().join(format!("stateful-runtime-db-test-{}", std::process::id()));

    assert_eq!(
        state_db_path(&temp_root),
        temp_root.join(".stateful_core").join("state.db")
    );
}

#[test]
fn global_state_db_path_uses_user_level_state_db() {
    let temp_root = std::env::temp_dir().join(format!(
        "stateful-global-runtime-db-test-{}",
        std::process::id()
    ));
    let paths = GlobalPaths::new(temp_root.join("home"));

    assert_eq!(global_state_db_path(&paths), paths.state_db);
}

#[test]
fn post_json_sends_bearer_token_and_payload() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("listener should bind");
    let addr = listener.local_addr().expect("listener addr should load");
    let (tx, rx) = mpsc::channel();

    thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("connection should arrive");
        let request = read_http_request(&mut stream);
        tx.send(request).expect("request should send to test");
        stream
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 15\r\n\r\n{\"status\":\"ok\"}")
            .expect("response should write");
    });

    let runtime = ServerRuntime::new(format!("http://{addr}"), "secret-token", "w1", 42);
    let response = post_json(
        &runtime,
        "/v1/intent/declare",
        &serde_json::json!({
            "session_id": "s1",
            "workspace_id": "w1",
            "files_planned": ["src/auth.ts"]
        }),
    )
    .expect("post should succeed");

    assert_eq!(response.status_code, 200);

    let request = rx.recv().expect("captured request should arrive");
    assert!(request.contains("POST /v1/intent/declare HTTP/1.1"));
    assert!(request.contains("Authorization: Bearer secret-token"));
    assert!(request.contains("\"files_planned\":[\"src/auth.ts\"]"));
}

#[test]
fn get_json_sends_bearer_token_without_body() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("listener should bind");
    let addr = listener.local_addr().expect("listener addr should load");
    let (tx, rx) = mpsc::channel();

    thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("connection should arrive");
        let request = read_http_request_without_body(&mut stream);
        tx.send(request).expect("request should send to test");
        stream
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 15\r\n\r\n{\"status\":\"ok\"}")
            .expect("response should write");
    });

    let runtime = ServerRuntime::new(format!("http://{addr}"), "secret-token", "w1", 42);
    let response = get_json(&runtime, "/v1/current").expect("get should succeed");

    assert_eq!(response.status_code, 200);

    let request = rx.recv().expect("captured request should arrive");
    assert!(request.contains("GET /v1/current HTTP/1.1"));
    assert!(request.contains("Authorization: Bearer secret-token"));
    assert!(!request.contains("Content-Length:"));
}

#[test]
fn declare_intent_via_http_posts_expected_payload() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("listener should bind");
    let addr = listener.local_addr().expect("listener addr should load");
    let (tx, rx) = mpsc::channel();

    thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("connection should arrive");
        let request = read_http_request(&mut stream);
        tx.send(request).expect("request should send to test");
        stream
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 15\r\n\r\n{\"status\":\"ok\"}")
            .expect("response should write");
    });

    let runtime = ServerRuntime::new(format!("http://{addr}"), "secret-token", "w1", 42);

    declare_intent_via_http(
        &runtime,
        IntentDeclareArgs {
            session_id: "s1".to_string(),
            workspace_id: "w1".to_string(),
            files_planned: vec!["src/auth.ts".to_string()],
            identity: None,
        },
    )
    .expect("intent declaration should post");

    let request = rx.recv().expect("captured request should arrive");
    assert!(request.contains("POST /v1/intent/declare HTTP/1.1"));
    let body = request_json_body(&request);
    assert_eq!(body["protocol_version"], "stateful.v1");
    assert!(
        body["request_id"]
            .as_str()
            .is_some_and(|value| !value.is_empty())
    );
    assert_eq!(body["observed_at"], "2026-05-31T00:00:00Z");
    assert_eq!(body["session"]["session_id"], "s1");
    assert_eq!(body["session"]["actor_id"], "stateful-cli:42");
    assert_eq!(body["session"]["actor_type"], "agent");
    assert_eq!(body["workspace"]["workspace_id"], "w1");
    assert_eq!(body["workspace"]["repo_id"], "");
    assert_eq!(body["workspace"]["worktree_id"], "");
    assert_eq!(body["workspace"]["root"], "");
    assert_eq!(body["workspace"]["branch"], "");
    assert_eq!(body["source"]["kind"], "cli");
    assert_eq!(body["source"]["event"], "intent_declare");
    assert_eq!(body["source"]["source_ref"], "stateful-cli");
    assert_eq!(
        body["payload"],
        serde_json::json!({
            "files_planned": ["src/auth.ts"]
        })
    );
    assert!(body.get("files_planned").is_none());
}

fn request_json_body(request: &str) -> serde_json::Value {
    let (_, body) = request
        .split_once("\r\n\r\n")
        .expect("request should contain a body separator");
    serde_json::from_str(body).expect("request body should be json")
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

fn read_http_request_without_body(stream: &mut std::net::TcpStream) -> String {
    let mut buffer = Vec::new();
    let mut byte = [0_u8; 1];
    while !buffer.ends_with(b"\r\n\r\n") {
        stream
            .read_exact(&mut byte)
            .expect("request header byte should read");
        buffer.push(byte[0]);
    }

    String::from_utf8(buffer).expect("request should be utf8")
}
