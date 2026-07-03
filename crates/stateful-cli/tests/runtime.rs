use std::{
    fs,
    io::{Read, Write},
    net::TcpListener,
    process::Command,
    sync::mpsc,
    thread,
};

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

use stateful_cli::{
    GlobalPaths, ReservationCancelArgs, ReservationClaimArgs, ReservationDeclareArgs,
    ReservationRequestArgs, ServerRuntime, cancel_reservation_via_http, claim_reservation_via_http,
    declare_reservation_via_http, discover_runtime, discover_runtime_with_global, get_json,
    global_state_db_path, post_json, request_reservation_via_http, state_db_path,
    validate_agent_id, write_global_runtime_file, write_runtime_file,
};
use time::{OffsetDateTime, format_description::well_known::Rfc3339};

#[test]
fn validate_agent_id_accepts_safe_agent_identifiers() {
    for agent_id in ["agent-1", "sub_agent_2", "A0_b-C9"] {
        validate_agent_id(agent_id, "agent_id").expect("safe agent id should be accepted");
    }
}

#[test]
fn validate_agent_id_rejects_empty_agent_identifier() {
    let error = validate_agent_id("", "agent_id").expect_err("empty agent id should fail");

    assert!(error.to_string().contains("agent_id is set but empty"));
}

#[test]
fn validate_agent_id_rejects_unsupported_agent_identifier_characters() {
    for agent_id in ["agent/1", "agent 1", "agent.1"] {
        let error =
            validate_agent_id(agent_id, "agent_id").expect_err("unsupported agent id should fail");
        assert!(
            error
                .to_string()
                .contains("agent_id contains unsupported characters")
        );
    }
}

#[test]
fn runtime_file_round_trips_server_discovery() {
    let temp = tempfile::tempdir().expect("temp dir should create");
    let temp_root = temp.path();

    let runtime = ServerRuntime::new("http://127.0.0.1:43873", "secret-token", "w1", 42);
    write_runtime_file(&temp_root, &runtime).expect("runtime file should write");

    let discovered = discover_runtime(&temp_root).expect("runtime should be discoverable");

    assert_eq!(discovered.base_url, "http://127.0.0.1:43873");
    assert_eq!(discovered.token, "secret-token");
    assert_eq!(discovered.workspace_id, "w1");
}

#[test]
fn global_runtime_file_round_trips_server_discovery() {
    let temp = tempfile::tempdir().expect("temp dir should create");
    let temp_root = temp.path();
    let repo_root = temp_root.join("repo");
    let paths = GlobalPaths::new(temp_root.join("home"));

    let runtime = ServerRuntime::new("http://127.0.0.1:43874", "global-token", "global-w", 43);
    write_global_runtime_file(&paths, &runtime).expect("global runtime file should write");

    let discovered =
        discover_runtime_with_global(&repo_root, &paths).expect("global runtime should discover");

    assert_eq!(discovered.base_url, "http://127.0.0.1:43874");
    assert_eq!(discovered.token, "global-token");
    assert_eq!(discovered.workspace_id, "global-w");
}

#[test]
fn runtime_discovery_keeps_local_runtime_compatibility_fallback() {
    let temp = tempfile::tempdir().expect("temp dir should create");
    let temp_root = temp.path();
    let repo_root = temp_root.join("repo");
    let paths = GlobalPaths::new(temp_root.join("home"));

    let runtime = ServerRuntime::new("http://127.0.0.1:43875", "repo-token", "repo-w", 44);
    write_runtime_file(&repo_root, &runtime).expect("repo runtime file should write");

    let discovered =
        discover_runtime_with_global(&repo_root, &paths).expect("repo runtime should discover");

    assert_eq!(discovered.base_url, "http://127.0.0.1:43875");
    assert_eq!(discovered.token, "repo-token");
    assert_eq!(discovered.workspace_id, "repo-w");
}

#[test]
fn cli_current_uses_local_runtime_when_global_paths_are_unavailable() {
    let temp = tempfile::tempdir().expect("temp dir should create");
    let temp_root = temp.path();
    fs::create_dir_all(temp_root.join(".git")).expect("git marker should write");

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
}

#[cfg(unix)]
#[test]
fn runtime_files_are_owner_read_write_only() {
    let temp = tempfile::tempdir().expect("temp dir should create");
    let temp_root = temp.path();
    let paths = GlobalPaths::new(temp_root.join("home"));
    let runtime = ServerRuntime::new("http://127.0.0.1:43875", "secret-token", "w1", 44);

    write_runtime_file(&temp_root, &runtime).expect("repo runtime file should write");
    write_global_runtime_file(&paths, &runtime).expect("global runtime file should write");

    let repo_mode = fs::metadata(temp_root.join(".stateful_core/runtime/server.json"))
        .expect("repo runtime metadata should read")
        .permissions()
        .mode()
        & 0o777;
    let global_mode = fs::metadata(&paths.server_json)
        .expect("global runtime metadata should read")
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(repo_mode, 0o600);
    assert_eq!(global_mode, 0o600);
}

#[test]
fn remote_runtime_with_pid_zero_accepts_matching_identity_capabilities() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("listener should bind");
    let addr = listener.local_addr().expect("listener addr should load");
    thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("connection should arrive");
        let request = read_http_request_without_body(&mut stream);
        assert!(request.contains("GET /v1/runtime/identity HTTP/1.1"));
        write_http_response(
            &mut stream,
            r#"{"status":"ok","pid":9876,"protocol_version":"stateful.v1","capabilities":["authorize.write_directory"]}"#,
        );
    });

    let runtime = ServerRuntime::new(format!("http://{addr}"), "secret-token", "shared", 0);

    assert!(stateful_cli::runtime_has_required_identity(&runtime));
}

#[test]
fn runtime_identity_matches_pid_requires_exact_pid_for_pid_zero_runtime() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("listener should bind");
    let addr = listener.local_addr().expect("listener addr should load");
    thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("connection should arrive");
        let request = read_http_request_without_body(&mut stream);
        assert!(request.contains("GET /v1/runtime/identity HTTP/1.1"));
        write_http_response(
            &mut stream,
            r#"{"status":"ok","pid":9876,"protocol_version":"stateful.v1","capabilities":["authorize.write_directory"]}"#,
        );
    });

    let runtime = ServerRuntime::new(format!("http://{addr}"), "secret-token", "shared", 0);

    assert!(
        !stateful_cli::runtime_identity_matches_pid(&runtime)
            .expect("identity check should succeed")
    );
}

#[test]
fn runtime_has_required_identity_rejects_mismatched_nonzero_pid() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("listener should bind");
    let addr = listener.local_addr().expect("listener addr should load");
    thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("connection should arrive");
        let request = read_http_request_without_body(&mut stream);
        assert!(request.contains("GET /v1/runtime/identity HTTP/1.1"));
        write_http_response(
            &mut stream,
            r#"{"status":"ok","pid":9876,"protocol_version":"stateful.v1","capabilities":["authorize.write_directory"]}"#,
        );
    });

    let runtime = ServerRuntime::new(format!("http://{addr}"), "secret-token", "shared", 42);

    assert!(!stateful_cli::runtime_has_required_identity(&runtime));
}

#[test]
fn cli_current_rejects_env_runtime_without_required_capabilities() {
    let temp = tempfile::tempdir().expect("temp dir should create");
    let temp_root = temp.path();

    let listener = TcpListener::bind("127.0.0.1:0").expect("listener should bind");
    let addr = listener.local_addr().expect("listener addr should load");
    thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("connection should arrive");
        let request = read_http_request_without_body(&mut stream);
        assert!(request.contains("GET /v1/runtime/identity HTTP/1.1"));
        write_http_response(
            &mut stream,
            r#"{"status":"ok","pid":77,"protocol_version":"stateful.v1"}"#,
        );
    });

    let output = Command::new(env!("CARGO_BIN_EXE_stateful"))
        .arg("current")
        .current_dir(&temp_root)
        .env_clear()
        .env("STATEFUL_SERVER_URL", format!("http://{addr}"))
        .env("STATEFUL_SERVER_TOKEN", "secret-token")
        .output()
        .expect("stateful current should run");

    assert!(
        !output.status.success(),
        "old env runtime should fail capability validation"
    );
    assert!(
        String::from_utf8_lossy(&output.stderr)
            .contains("does not support required runtime capabilities"),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn cli_current_accepts_env_runtime_with_required_capabilities() {
    let temp = tempfile::tempdir().expect("temp dir should create");
    let temp_root = temp.path();

    let listener = TcpListener::bind("127.0.0.1:0").expect("listener should bind");
    let addr = listener.local_addr().expect("listener addr should load");
    thread::spawn(move || {
        let (mut stream, _) = listener
            .accept()
            .expect("identity connection should arrive");
        let request = read_http_request_without_body(&mut stream);
        assert!(request.contains("GET /v1/runtime/identity HTTP/1.1"));
        write_http_response(
            &mut stream,
            r#"{"status":"ok","pid":77,"protocol_version":"stateful.v1","capabilities":["authorize.write_directory"]}"#,
        );

        let (mut stream, _) = listener.accept().expect("current connection should arrive");
        let request = read_http_request_without_body(&mut stream);
        assert!(request.contains("GET /v1/current HTTP/1.1"));
        write_http_response(&mut stream, r#"{"status":"ok","current":{}}"#);
    });

    let output = Command::new(env!("CARGO_BIN_EXE_stateful"))
        .arg("current")
        .current_dir(&temp_root)
        .env_clear()
        .env("STATEFUL_SERVER_URL", format!("http://{addr}"))
        .env("STATEFUL_SERVER_TOKEN", "secret-token")
        .output()
        .expect("stateful current should run");

    assert!(
        output.status.success(),
        "capable env runtime should work: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("\"status\":\"ok\""));
}

#[test]
fn state_db_path_uses_local_runtime_directory() {
    let temp = tempfile::tempdir().expect("temp dir should create");
    let temp_root = temp.path();
    assert_eq!(
        state_db_path(temp_root),
        temp_root.join(".stateful_core").join("state.db")
    );
}

#[test]
fn global_state_db_path_uses_user_level_state_db() {
    let temp = tempfile::tempdir().expect("temp dir should create");
    let temp_root = temp.path();
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
        "/v1/reservation/declare",
        &serde_json::json!({
            "agent_id": "s1",
            "workspace_id": "w1",
            "purpose": "Fix auth validation behavior.",
            "files_planned": ["src/auth.ts"]
        }),
    )
    .expect("post should succeed");

    assert_eq!(response.status_code, 200);

    let request = rx.recv().expect("captured request should arrive");
    assert!(request.contains("POST /v1/reservation/declare HTTP/1.1"));
    assert!(request.contains("Authorization: Bearer secret-token"));
    assert!(request.contains("\"purpose\":\"Fix auth validation behavior.\""));
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
fn declare_reservation_via_http_posts_expected_payload() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("listener should bind");
    let addr = listener.local_addr().expect("listener addr should load");
    let (tx, rx) = mpsc::channel();

    thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("connection should arrive");
        let request = read_http_request(&mut stream);
        tx.send(request).expect("request should send to test");
        write_http_response(
            &mut stream,
            r#"{"status":"ok","reservation_id":"reservation-123"}"#,
        );
    });

    let runtime = ServerRuntime::new(format!("http://{addr}"), "secret-token", "w1", 42);

    let before_request = OffsetDateTime::now_utc();
    let response = declare_reservation_via_http(
        &runtime,
        ReservationDeclareArgs {
            agent_id: "s1".to_string(),
            workspace_id: "w1".to_string(),
            purpose: "Fix auth validation behavior.".to_string(),
            files_planned: vec!["src/auth.ts".to_string()],
            identity: None,
        },
    )
    .expect("reservation declaration should post");
    assert_eq!(
        response.body,
        r#"{"status":"ok","reservation_id":"reservation-123"}"#
    );

    let request = rx.recv().expect("captured request should arrive");
    assert!(request.contains("POST /v1/reservation/declare HTTP/1.1"));
    let body = request_json_body(&request);
    assert_eq!(body["protocol_version"], "stateful.v1");
    assert!(
        body["request_id"]
            .as_str()
            .is_some_and(|value| !value.is_empty())
    );
    let observed_at = body["observed_at"]
        .as_str()
        .expect("observed_at should be a string");
    assert_ne!(
        observed_at, runtime.started_at,
        "observed_at should describe the request, not the runtime start"
    );
    let observed_at = OffsetDateTime::parse(observed_at, &Rfc3339)
        .expect("observed_at should be an RFC3339 timestamp");
    let after_request = OffsetDateTime::now_utc();
    assert!(observed_at >= before_request);
    assert!(observed_at <= after_request);
    assert_eq!(body["agent"]["agent_id"], "s1");
    assert_eq!(body["agent"]["actor_id"], "s1");
    assert_eq!(body["agent"]["actor_type"], "agent");
    assert_eq!(body["workspace"]["workspace_id"], "w1");
    assert_eq!(body["workspace"]["repo_id"], "");
    assert_eq!(body["workspace"]["worktree_id"], "");
    assert_eq!(body["workspace"]["root"], "");
    assert_eq!(body["workspace"]["branch"], "");
    assert_eq!(body["source"]["kind"], "cli");
    assert_eq!(body["source"]["event"], "reservation_declare");
    assert_eq!(body["source"]["source_ref"], "stateful-cli");
    assert_eq!(
        body["payload"],
        serde_json::json!({
            "purpose": "Fix auth validation behavior.",
            "files_planned": ["src/auth.ts"]
        })
    );
    assert!(body.get("files_planned").is_none());
}

#[test]
fn claim_reservation_via_http_posts_expected_payload() {
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

    claim_reservation_via_http(
        &runtime,
        ReservationClaimArgs {
            agent_id: "s1".to_string(),
            workspace_id: "w1".to_string(),
            wait_id: "wait-1".to_string(),
            reservation_id: None,
            identity: None,
        },
    )
    .expect("reservation claim should post");

    let request = rx.recv().expect("captured request should arrive");
    assert!(request.contains("POST /v1/reservation/claim HTTP/1.1"));
    let body = request_json_body(&request);
    assert_eq!(body["protocol_version"], "stateful.v1");
    assert_eq!(body["agent"]["agent_id"], "s1");
    assert_eq!(body["workspace"]["workspace_id"], "w1");
    assert_eq!(body["source"]["kind"], "cli");
    assert_eq!(body["source"]["event"], "reservation_claim");
    assert_eq!(body["source"]["source_ref"], "stateful-cli");
    assert_eq!(
        body["payload"],
        serde_json::json!({
            "wait_id": "wait-1"
        })
    );
    assert!(body.get("wait_id").is_none());
}

#[test]
fn request_reservation_via_http_posts_expected_payload() {
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

    let response = request_reservation_via_http(
        &runtime,
        ReservationRequestArgs {
            agent_id: "s1".to_string(),
            workspace_id: "w1".to_string(),
            request_id: "request-1".to_string(),
            reservation_id: None,
            action: "write_file".to_string(),
            path: "src/auth.ts".to_string(),
            purpose: "Queue auth file changes.".to_string(),
            identity: None,
        },
    )
    .expect("reservation request should post");
    assert_eq!(response.status_code, 200);
    assert_eq!(response.body, "{\"status\":\"ok\"}");

    let request = rx.recv().expect("captured request should arrive");
    assert!(request.contains("POST /v1/reservation/request HTTP/1.1"));
    let body = request_json_body(&request);
    assert_eq!(body["protocol_version"], "stateful.v1");
    assert_eq!(body["agent"]["agent_id"], "s1");
    assert_eq!(body["workspace"]["workspace_id"], "w1");
    assert_eq!(body["source"]["kind"], "cli");
    assert_eq!(body["source"]["event"], "reservation_request");
    assert_eq!(body["source"]["source_ref"], "stateful-cli");
    assert_eq!(
        body["payload"],
        serde_json::json!({
            "request_id": "request-1",
            "action": "write_file",
            "path": "src/auth.ts",
            "purpose": "Queue auth file changes."
        })
    );
    assert!(body.get("request_id").is_some());
}

#[test]
fn cancel_reservation_via_http_posts_expected_payload() {
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

    cancel_reservation_via_http(
        &runtime,
        ReservationCancelArgs {
            agent_id: "s1".to_string(),
            workspace_id: "w1".to_string(),
            request_id: "request-1".to_string(),
            identity: None,
        },
    )
    .expect("reservation cancel should post");

    let request = rx.recv().expect("captured request should arrive");
    assert!(request.contains("POST /v1/reservation/cancel HTTP/1.1"));
    let body = request_json_body(&request);
    assert_eq!(body["protocol_version"], "stateful.v1");
    assert_eq!(body["agent"]["agent_id"], "s1");
    assert_eq!(body["workspace"]["workspace_id"], "w1");
    assert_eq!(body["source"]["kind"], "cli");
    assert_eq!(body["source"]["event"], "reservation_cancel");
    assert_eq!(body["source"]["source_ref"], "stateful-cli");
    assert_eq!(
        body["payload"],
        serde_json::json!({
            "request_id": "request-1"
        })
    );
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

fn write_http_response(stream: &mut std::net::TcpStream, body: &str) {
    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n{body}",
        body.len()
    );
    stream
        .write_all(response.as_bytes())
        .expect("response should write");
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
