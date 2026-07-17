use std::{
    fs,
    io::{Read, Write},
    net::TcpListener,
    process::Command,
    sync::mpsc,
    thread,
    time::Duration,
};

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

use stateful_cli::{
    GlobalPaths, ReservationCancelArgs, ReservationClaimArgs, ReservationDeclareArgs,
    ReservationRequestArgs, ServerRuntime, cancel_reservation_via_http, claim_reservation_via_http,
    declare_reservation_via_http, discover_runtime, discover_runtime_with_global, enable_repo,
    get_json, global_state_db_path, post_json, repo_identity_for_enabled_repo,
    request_reservation_via_http, runtime_has_required_identity, runtime_status, state_db_path,
    validate_agent_id, write_global_runtime_file, write_runtime_file,
};

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
    write_runtime_file(temp_root, &runtime).expect("runtime file should write");

    let discovered = discover_runtime(temp_root).expect("runtime should be discoverable");

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
fn cli_current_and_events_serialize_enabled_repo_identity() {
    let temp = tempfile::tempdir().expect("temp dir should create");
    let temp_root = temp.path();
    Command::new("git")
        .args(["init", "--quiet"])
        .current_dir(temp_root)
        .status()
        .expect("git init should run")
        .success()
        .then_some(())
        .expect("git init should succeed");
    Command::new("git")
        .args(["checkout", "--orphan", "task9-identity"])
        .current_dir(temp_root)
        .status()
        .expect("git checkout should run")
        .success()
        .then_some(())
        .expect("git checkout should succeed");

    let paths = GlobalPaths::new(temp_root.join("stateful-home"));
    enable_repo(&paths, temp_root).expect("repo should enable");
    let identity =
        repo_identity_for_enabled_repo(&paths, temp_root).expect("enabled identity should load");

    let listener = TcpListener::bind("127.0.0.1:0").expect("listener should bind");
    let addr = listener.local_addr().expect("listener addr should load");
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        for response in [r#"{"presence":null,"resources":[]}"#, r#"{"events":[]}"#] {
            let (mut stream, _) = listener.accept().expect("connection should arrive");
            tx.send(read_http_request_without_body(&mut stream))
                .expect("request should send");
            write_http_response(&mut stream, response);
        }
    });
    let runtime = ServerRuntime::new(format!("http://{addr}"), "secret-token", "w1", 42);
    write_global_runtime_file(&paths, &runtime).expect("runtime should write");

    for command in ["current", "events"] {
        let output = Command::new(env!("CARGO_BIN_EXE_stateful"))
            .arg(command)
            .current_dir(temp_root)
            .env_clear()
            .env(
                "PATH",
                std::env::var_os("PATH").expect("PATH should be set"),
            )
            .env("STATEFUL_HOME", &paths.home)
            .output()
            .expect("stateful query should run");
        assert!(
            output.status.success(),
            "stateful {command} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    for (request, route) in [
        (
            rx.recv_timeout(Duration::from_secs(1))
                .expect("current request should arrive"),
            "/v2/current?",
        ),
        (
            rx.recv_timeout(Duration::from_secs(1))
                .expect("events request should arrive"),
            "/v2/events?",
        ),
    ] {
        assert!(request.contains(route), "request: {request}");
        for identity_field in [
            format!("repo_id={}", identity.repo_id),
            format!("worktree_id={}", identity.worktree_id),
            format!("root={}", identity.root.replace('/', "%2F")),
            format!("branch={}", identity.branch),
        ] {
            assert!(
                request.contains(&identity_field),
                "missing {identity_field} from {request}"
            );
        }
    }
}

#[cfg(unix)]
#[test]
fn runtime_files_are_owner_read_write_only() {
    let temp = tempfile::tempdir().expect("temp dir should create");
    let temp_root = temp.path();
    let paths = GlobalPaths::new(temp_root.join("home"));
    let runtime = ServerRuntime::new("http://127.0.0.1:43875", "secret-token", "w1", 44);

    write_runtime_file(temp_root, &runtime).expect("repo runtime file should write");
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
        assert!(request.contains("GET /v2/runtime/identity?"));
        write_http_response(
            &mut stream,
            r#"{"protocol_version":"stateful.v2","journal_schema_version":2,"coordination_mode":"awareness","pid":1,"capabilities":["presence"]}"#,
        );
    });

    let runtime = ServerRuntime::new(format!("http://{addr}"), "secret-token", "shared", 0);

    assert!(stateful_cli::runtime_has_required_identity(&runtime));
}

#[test]
fn runtime_status_includes_server_workspace_identity_and_version() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("listener should bind");
    let addr = listener.local_addr().expect("listener addr should load");
    thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("connection should arrive");
        let request = read_http_request_without_body(&mut stream);
        assert!(request.contains("GET /v2/runtime/identity?"));
        write_http_response(
            &mut stream,
            r#"{"protocol_version":"stateful.v2","journal_schema_version":2,"coordination_mode":"enforcement","pid":42,"workspace_id":"server-workspace","workspace_version":17,"capabilities":["presence"]}"#,
        );
    });

    let runtime = ServerRuntime::new(format!("http://{addr}"), "secret-token", "w1", 42);
    let status = serde_json::to_value(runtime_status(&runtime).expect("status should load"))
        .expect("status should serialize");

    assert_eq!(status["protocol_version"], "stateful.v2");
    assert_eq!(status["journal_schema_version"], 2);
    assert_eq!(status["coordination_mode"], "enforcement");
    assert_eq!(status["workspace_id"], "server-workspace");
    assert_eq!(status["workspace_version"], 17);
    assert_eq!(status["capabilities"], serde_json::json!(["presence"]));
}

#[test]
fn runtime_identity_matches_pid_rejects_different_nonzero_server_pid() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("listener should bind");
    let addr = listener.local_addr().expect("listener addr should load");
    thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("connection should arrive");
        let request = read_http_request_without_body(&mut stream);
        assert!(request.contains("GET /v2/runtime/identity?"));
        write_http_response(
            &mut stream,
            r#"{"protocol_version":"stateful.v2","journal_schema_version":2,"coordination_mode":"awareness","pid":43,"capabilities":["presence"]}"#,
        );
    });

    let runtime = ServerRuntime::new(format!("http://{addr}"), "secret-token", "shared", 42);

    assert!(
        !stateful_cli::runtime_identity_matches_pid(&runtime)
            .expect("identity check should succeed")
    );
}

#[test]
fn runtime_identity_matches_pid_accepts_exact_server_pid() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("listener should bind");
    let addr = listener.local_addr().expect("listener addr should load");
    thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("connection should arrive");
        let request = read_http_request_without_body(&mut stream);
        assert!(request.contains("GET /v2/runtime/identity?"));
        write_http_response(
            &mut stream,
            r#"{"protocol_version":"stateful.v2","journal_schema_version":2,"coordination_mode":"awareness","pid":42,"capabilities":["presence"]}"#,
        );
    });

    let runtime = ServerRuntime::new(format!("http://{addr}"), "secret-token", "shared", 42);

    assert!(
        stateful_cli::runtime_identity_matches_pid(&runtime)
            .expect("identity check should succeed")
    );
}

#[test]
fn runtime_identity_matches_pid_rejects_zero_server_pid() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("listener should bind");
    let addr = listener.local_addr().expect("listener addr should load");
    thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("connection should arrive");
        let request = read_http_request_without_body(&mut stream);
        assert!(request.contains("GET /v2/runtime/identity?"));
        write_http_response(
            &mut stream,
            r#"{"protocol_version":"stateful.v2","journal_schema_version":2,"coordination_mode":"awareness","pid":0,"capabilities":["presence"]}"#,
        );
    });

    let runtime = ServerRuntime::new(format!("http://{addr}"), "secret-token", "shared", 42);

    assert!(
        !stateful_cli::runtime_identity_matches_pid(&runtime)
            .expect("identity check should succeed")
    );
}

#[test]
fn runtime_has_required_identity_rejects_missing_pid() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("listener should bind");
    let addr = listener.local_addr().expect("listener addr should load");
    thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("connection should arrive");
        let request = read_http_request_without_body(&mut stream);
        assert!(request.contains("GET /v2/runtime/identity?"));
        write_http_response(
            &mut stream,
            r#"{"protocol_version":"stateful.v2","journal_schema_version":2,"coordination_mode":"awareness","capabilities":["presence"]}"#,
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
        assert!(request.contains("GET /v2/runtime/identity?"));
        write_http_response(
            &mut stream,
            r#"{"protocol_version":"stateful.v2","journal_schema_version":2,"coordination_mode":"awareness","pid":1,"capabilities":[]}"#,
        );
    });

    let output = Command::new(env!("CARGO_BIN_EXE_stateful"))
        .arg("current")
        .current_dir(temp_root)
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
        assert!(request.contains("GET /v2/runtime/identity?"));
        write_http_response(
            &mut stream,
            r#"{"protocol_version":"stateful.v2","journal_schema_version":2,"coordination_mode":"awareness","pid":1,"capabilities":["presence"]}"#,
        );

        let (mut stream, _) = listener.accept().expect("current connection should arrive");
        let request = read_http_request_without_body(&mut stream);
        assert!(request.contains("GET /v2/current?"));
        write_http_response(&mut stream, r#"{"presence":null,"resources":[]}"#);
    });

    let output = Command::new(env!("CARGO_BIN_EXE_stateful"))
        .arg("current")
        .current_dir(temp_root)
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
    assert!(String::from_utf8_lossy(&output.stdout).contains("\"resources\":[]"));
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
fn declare_reservation_via_http_posts_one_task_envelope_and_returns_its_identity() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("listener should bind");
    let addr = listener.local_addr().expect("listener addr should load");
    let (tx, rx) = mpsc::channel();

    thread::spawn(move || {
        for reservation_id in ["reservation-123", "reservation-456"] {
            let (mut stream, _) = listener
                .accept()
                .expect("handshake connection should arrive");
            let handshake = read_http_request_without_body(&mut stream);
            assert!(handshake.contains("GET /v2/runtime/identity?"));
            write_v2_runtime_identity(&mut stream);

            let (mut stream, _) = listener
                .accept()
                .expect("declaration connection should arrive");
            tx.send(read_http_request(&mut stream))
                .expect("request should send to test");
            write_http_response(
                &mut stream,
                &format!(r#"{{"reservation_id":"{reservation_id}"}}"#),
            );
        }
    });

    let runtime = ServerRuntime::new(format!("http://{addr}"), "secret-token", "w1", 42);
    let response = declare_reservation_via_http(
        &runtime,
        ReservationDeclareArgs {
            request_id: uuid::Uuid::parse_str("00000000-0000-4000-8000-000000000100")
                .expect("request id should parse"),
            agent_id: "s1".to_string(),
            workspace_id: "w1".to_string(),
            purpose: "Fix auth validation behavior.".to_string(),
            files_planned: vec![
                "./src/auth.ts".to_string(),
                "src/api.rs".to_string(),
                "src/generated/".to_string(),
            ],
            identity: None,
        },
    )
    .expect("reservation declaration should post");

    assert_eq!(response.body, r#"{"reservation_id":"reservation-123"}"#);
    let request = rx.recv().expect("captured request should arrive");
    assert!(
        rx.recv_timeout(Duration::from_millis(100)).is_err(),
        "a task declaration must send exactly one envelope"
    );
    assert!(request.contains("POST /v2/reservation/declare HTTP/1.1"));
    let body = request_json_body(&request);
    assert_eq!(body["protocol_version"], "stateful.v2");
    assert_eq!(body["request_id"], "00000000-0000-4000-8000-000000000100");
    assert_eq!(body["agent"]["agent_id"], "s1");
    assert_eq!(body["workspace"]["workspace_id"], "w1");
    assert_eq!(body["source"]["kind"], "cli");
    assert_eq!(body["source"]["event"], "reservation_declare");
    assert_eq!(
        body["payload"],
        serde_json::json!({
            "scopes": [
                {"kind": "file", "path": "src/auth.ts"},
                {"kind": "file", "path": "src/api.rs"},
                {"kind": "directory", "path": "src/generated/"}
            ],
            "action": "write",
            "purpose": "Fix auth validation behavior."
        })
    );
}

#[test]
fn claim_reservation_via_http_posts_granted_path_and_validates_wait_identity() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("listener should bind");
    let addr = listener.local_addr().expect("listener addr should load");
    let (tx, rx) = mpsc::channel();

    thread::spawn(move || {
        let (mut stream, _) = listener
            .accept()
            .expect("handshake connection should arrive");
        let handshake = read_http_request_without_body(&mut stream);
        assert!(handshake.contains("GET /v2/runtime/identity?"));
        write_v2_runtime_identity(&mut stream);
        let (mut stream, _) = listener.accept().expect("connection should arrive");
        let request = read_http_request(&mut stream);
        tx.send(request).expect("request should send to test");
        write_http_response(
            &mut stream,
            r#"{"wait_id":"wait-1","relative_path":"src/auth.ts"}"#,
        );
    });

    let runtime = ServerRuntime::new(format!("http://{addr}"), "secret-token", "w1", 42);

    claim_reservation_via_http(
        &runtime,
        ReservationClaimArgs {
            request_id: uuid::Uuid::parse_str("00000000-0000-4000-8000-000000000103")
                .expect("request id should parse"),
            agent_id: "s1".to_string(),
            workspace_id: "w1".to_string(),
            wait_id: "wait-1".to_string(),
            relative_path: "src/auth.ts".to_string(),
            reservation_id: None,
            identity: None,
        },
    )
    .expect("reservation claim should post");
    let request = rx.recv().expect("captured request should arrive");

    assert!(request.contains("POST /v2/reservation/claim HTTP/1.1"));
    let body = request_json_body(&request);
    assert_eq!(body["protocol_version"], "stateful.v2");
    assert_eq!(body["agent"]["agent_id"], "s1");
    assert_eq!(body["workspace"]["workspace_id"], "w1");
    assert_eq!(body["source"]["kind"], "cli");
    assert_eq!(body["source"]["event"], "reservation_claim");
    assert_eq!(body["source"]["source_ref"], "stateful-cli");
    assert_eq!(body["request_id"], "00000000-0000-4000-8000-000000000103");
    assert_eq!(
        body["payload"],
        serde_json::json!({
            "relative_path": "src/auth.ts"
        })
    );
}

#[test]
fn request_reservation_via_http_posts_expected_payload() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("listener should bind");
    let addr = listener.local_addr().expect("listener addr should load");
    let (tx, rx) = mpsc::channel();

    thread::spawn(move || {
        let (mut stream, _) = listener
            .accept()
            .expect("handshake connection should arrive");
        let handshake = read_http_request_without_body(&mut stream);
        assert!(handshake.contains("GET /v2/runtime/identity?"));
        write_v2_runtime_identity(&mut stream);
        let (mut stream, _) = listener.accept().expect("connection should arrive");
        let request = read_http_request(&mut stream);
        tx.send(request).expect("request should send to test");
        write_http_response(&mut stream, r#"{}"#);
    });

    let runtime = ServerRuntime::new(format!("http://{addr}"), "secret-token", "w1", 42);

    let response = request_reservation_via_http(
        &runtime,
        ReservationRequestArgs {
            agent_id: "s1".to_string(),
            workspace_id: "w1".to_string(),
            request_id: uuid::Uuid::parse_str("00000000-0000-4000-8000-000000000101")
                .expect("request id should parse"),
            reservation_id: None,
            action: "write_file".to_string(),
            path: "src/auth.ts".to_string(),
            purpose: "Queue auth file changes.".to_string(),
            identity: None,
        },
    )
    .expect("reservation request should post");
    assert_eq!(response.status_code, 200);
    assert_eq!(response.body, "{}");
    let request = rx.recv().expect("captured request should arrive");

    assert!(request.contains("POST /v2/reservation/request HTTP/1.1"));
    let body = request_json_body(&request);
    assert_eq!(body["protocol_version"], "stateful.v2");
    assert_eq!(body["agent"]["agent_id"], "s1");
    assert_eq!(body["workspace"]["workspace_id"], "w1");
    assert_eq!(body["source"]["kind"], "cli");
    assert_eq!(body["source"]["event"], "reservation_request");
    assert_eq!(body["source"]["source_ref"], "stateful-cli");
    assert_eq!(body["request_id"], "00000000-0000-4000-8000-000000000101");
    assert_eq!(
        body["payload"],
        serde_json::json!({
            "relative_path": "src/auth.ts",
            "action": "write_file",
            "purpose": "Queue auth file changes.",
            "blocking_agent_id": null
        })
    );
}

#[test]
fn cancel_reservation_via_http_posts_expected_payload() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("listener should bind");
    let addr = listener.local_addr().expect("listener addr should load");
    let (tx, rx) = mpsc::channel();

    thread::spawn(move || {
        let (mut stream, _) = listener
            .accept()
            .expect("handshake connection should arrive");
        let handshake = read_http_request_without_body(&mut stream);
        assert!(handshake.contains("GET /v2/runtime/identity?"));
        write_v2_runtime_identity(&mut stream);
        let (mut stream, _) = listener.accept().expect("connection should arrive");
        let request = read_http_request(&mut stream);
        tx.send(request).expect("request should send to test");
        write_http_response(&mut stream, r#"{}"#);
    });

    let runtime = ServerRuntime::new(format!("http://{addr}"), "secret-token", "w1", 42);

    cancel_reservation_via_http(
        &runtime,
        ReservationCancelArgs {
            agent_id: "s1".to_string(),
            workspace_id: "w1".to_string(),
            request_id: uuid::Uuid::parse_str("00000000-0000-4000-8000-000000000102")
                .expect("request id should parse"),
            wait_id: "wait-123".to_string(),
            identity: None,
        },
    )
    .expect("reservation cancel should post");
    let request = rx.recv().expect("captured request should arrive");

    assert!(request.contains("POST /v2/reservation/cancel HTTP/1.1"));
    let body = request_json_body(&request);
    assert_eq!(body["protocol_version"], "stateful.v2");
    assert_eq!(body["agent"]["agent_id"], "s1");
    assert_eq!(body["workspace"]["workspace_id"], "w1");
    assert_eq!(body["source"]["kind"], "cli");
    assert_eq!(body["source"]["event"], "reservation_cancel");
    assert_eq!(body["source"]["source_ref"], "stateful-cli");
    assert_eq!(body["request_id"], "00000000-0000-4000-8000-000000000102");
    assert_eq!(
        body["payload"],
        serde_json::json!({
            "wait_id": "wait-123"
        })
    );
}

#[test]
fn runtime_post_wraps_typed_payload_in_v2_envelope() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("listener should bind");
    let addr = listener.local_addr().expect("listener address should load");
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let (mut stream, _) = listener
            .accept()
            .expect("handshake connection should arrive");
        let handshake = read_http_request_without_body(&mut stream);
        assert!(handshake.contains("GET /v2/runtime/identity?"));
        write_v2_runtime_identity(&mut stream);
        let (mut stream, _) = listener.accept().expect("connection should arrive");
        let request = read_http_request(&mut stream);
        tx.send(request).expect("request should send to test");
        write_http_response(&mut stream, r#"{"reservation_id":"reservation-123"}"#);
    });

    let runtime = ServerRuntime::new(format!("http://{addr}"), "secret-token", "w1", 42);
    declare_reservation_via_http(
        &runtime,
        ReservationDeclareArgs {
            request_id: uuid::Uuid::parse_str("00000000-0000-4000-8000-000000000104")
                .expect("request id should parse"),
            agent_id: "s1".to_string(),
            workspace_id: "w1".to_string(),
            purpose: "Fix auth validation behavior.".to_string(),
            files_planned: vec!["src/auth.ts".to_string()],
            identity: None,
        },
    )
    .expect("reservation declaration should post");

    let request = rx.recv().expect("captured request should arrive");
    assert!(request.contains("POST /v2/reservation/declare HTTP/1.1"));
    let body = request_json_body(&request);
    assert_eq!(body["protocol_version"], "stateful.v2");
    assert!(uuid::Uuid::parse_str(body["request_id"].as_str().expect("request id")).is_ok());
}

#[test]
fn runtime_get_serializes_full_query_identity() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("listener should bind");
    let addr = listener.local_addr().expect("listener address should load");
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("connection should arrive");
        let request = read_http_request_without_body(&mut stream);
        tx.send(request).expect("request should send to test");
        write_v2_runtime_identity(&mut stream);
    });

    let runtime = ServerRuntime::new(format!("http://{addr}"), "secret-token", "w1", 0);
    assert!(runtime_has_required_identity(&runtime));

    let request = rx.recv().expect("captured request should arrive");
    assert!(request.contains("GET /v2/runtime/identity?"));
    for identity_field in [
        "protocol_version=stateful.v2",
        "agent_id=stateful-cli",
        "workspace_id=w1",
        "repo_id=unknown",
        "worktree_id=unknown",
        "root=unknown",
        "branch=unknown",
    ] {
        assert!(request.contains(identity_field), "missing {identity_field}");
    }
}

#[test]
fn unsupported_runtime_protocol_fails_before_mutation() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("listener should bind");
    let addr = listener.local_addr().expect("listener address should load");
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("connection should arrive");
        let request = read_http_request_without_body(&mut stream);
        tx.send(request).expect("request should send to test");
        write_http_response(
            &mut stream,
            r#"{"protocol_version":"stateful.v1","journal_schema_version":2,"coordination_mode":"awareness","pid":1,"capabilities":["presence"]}"#,
        );
    });

    let runtime = ServerRuntime::new(format!("http://{addr}"), "secret-token", "w1", 42);
    let error = declare_reservation_via_http(
        &runtime,
        ReservationDeclareArgs {
            request_id: uuid::Uuid::parse_str("00000000-0000-4000-8000-000000000105")
                .expect("request id should parse"),
            agent_id: "s1".to_string(),
            workspace_id: "w1".to_string(),
            purpose: "Fix auth validation behavior.".to_string(),
            files_planned: vec!["src/auth.ts".to_string()],
            identity: None,
        },
    )
    .expect_err("unsupported runtime protocol must reject the mutation");
    assert!(error.to_string().contains("stateful.v2"));

    let request = rx.recv().expect("handshake request should arrive");
    assert!(request.contains("GET /v2/runtime/identity?"));
    assert!(!request.starts_with("POST "), "mutation must not be posted");
}

#[test]
fn missing_runtime_capability_fails_before_mutation() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("listener should bind");
    let addr = listener.local_addr().expect("listener address should load");
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("connection should arrive");
        let request = read_http_request_without_body(&mut stream);
        tx.send(request).expect("request should send to test");
        write_http_response(
            &mut stream,
            r#"{"protocol_version":"stateful.v2","journal_schema_version":2,"coordination_mode":"awareness","pid":42,"workspace_id":"w1","workspace_version":1,"capabilities":[]}"#,
        );
    });

    let runtime = ServerRuntime::new(format!("http://{addr}"), "secret-token", "w1", 42);
    let error = declare_reservation_via_http(
        &runtime,
        ReservationDeclareArgs {
            request_id: uuid::Uuid::parse_str("00000000-0000-4000-8000-000000000106")
                .expect("request id should parse"),
            agent_id: "s1".to_string(),
            workspace_id: "w1".to_string(),
            purpose: "Fix auth validation behavior.".to_string(),
            files_planned: vec!["src/auth.ts".to_string()],
            identity: None,
        },
    )
    .expect_err("missing runtime capability must reject the mutation");
    assert!(error.to_string().contains("presence"));

    let request = rx.recv().expect("handshake request should arrive");
    assert!(request.contains("GET /v2/runtime/identity?"));
    assert!(!request.starts_with("POST "), "mutation must not be posted");
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

fn write_v2_runtime_identity(stream: &mut std::net::TcpStream) {
    write_http_response(
        stream,
        r#"{"protocol_version":"stateful.v2","journal_schema_version":2,"coordination_mode":"awareness","pid":1,"capabilities":["presence"]}"#,
    );
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
