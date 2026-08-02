#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::{
    fs,
    io::{Read, Write},
    net::TcpListener,
    sync::mpsc,
    thread,
    time::Duration,
};

use serde::{Deserialize, Serialize};
use stateful_cli::{
    CommandIdentity, GlobalPaths, ServerRuntime, discover_runtime, discover_runtime_with_global,
    get_payload, post_command, process_start_identity_for_pid, write_global_runtime_file,
    write_runtime_file,
};
use stateful_core::{ActorType, AgentIdentity, SourceKind, SourceRef, WorkspaceIdentity};

#[derive(Debug, Deserialize, PartialEq, Eq)]
struct Reply {
    accepted: bool,
}

#[derive(Debug, Serialize)]
struct Payload {
    label: &'static str,
}

#[test]
fn local_runtime_file_round_trips_server_discovery() {
    let temp = tempfile::tempdir().expect("temp dir should create");
    let runtime = local_runtime("http://127.0.0.1:43873");

    write_runtime_file(temp.path(), &runtime).expect("runtime file should write");

    assert_eq!(
        discover_runtime(temp.path()).expect("runtime should be discoverable"),
        runtime
    );
}

#[test]
fn global_runtime_file_precedes_repo_runtime() {
    let temp = tempfile::tempdir().expect("temp dir should create");
    let repo_root = temp.path().join("repo");
    let paths = GlobalPaths::new(temp.path().join("home"));
    let repo_runtime = local_runtime("http://127.0.0.1:43873");
    let global_runtime = local_runtime("http://127.0.0.1:43874");
    write_runtime_file(&repo_root, &repo_runtime).expect("repo runtime should write");
    write_global_runtime_file(&paths, &global_runtime).expect("global runtime should write");

    assert_eq!(
        discover_runtime_with_global(&repo_root, &paths).expect("runtime should be discoverable"),
        global_runtime
    );
}

#[cfg(unix)]
#[test]
fn runtime_files_are_owner_read_write_only() {
    let temp = tempfile::tempdir().expect("temp dir should create");
    let paths = GlobalPaths::new(temp.path().join("home"));
    let runtime = local_runtime("http://127.0.0.1:43873");

    write_runtime_file(temp.path(), &runtime).expect("repo runtime file should write");
    write_global_runtime_file(&paths, &runtime).expect("global runtime file should write");

    let repo_mode = fs::metadata(temp.path().join(".stateful_core/runtime/server.json"))
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
fn non_loopback_runtime_file_is_rejected() {
    let temp = tempfile::tempdir().expect("temp dir should create");
    let runtime = local_runtime("http://192.0.2.1:43873");

    let error = write_runtime_file(temp.path(), &runtime)
        .expect_err("non-loopback runtime should not be persisted");

    assert!(error.to_string().contains("loopback"));
}

#[test]
fn post_command_serializes_v2_envelope_and_decodes_payload() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("listener should bind");
    let addr = listener.local_addr().expect("listener addr should load");
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("connection should arrive");
        tx.send(read_http_request(&mut stream))
            .expect("request should send");
        write_http_response(
            &mut stream,
            200,
            r#"{"protocol_version":"stateful.v2","contract_revision":"lease-1","request_id":"request-1","payload":{"accepted":true}}"#,
        );
    });

    let reply: Reply = post_command(
        &local_runtime(format!("http://{addr}")),
        "/v2/tasks/start",
        &command_identity(),
        &Payload { label: "start" },
    )
    .expect("command should succeed");

    assert_eq!(reply, Reply { accepted: true });
    let request: serde_json::Value = serde_json::from_str(request_body(
        &rx.recv().expect("captured request should arrive"),
    ))
    .expect("request body should be JSON");
    assert_eq!(request["protocol_version"], "stateful.v2");
    assert_eq!(request["contract_revision"], "lease-1");
    assert_eq!(request["task_id"], "task-1");
    assert_eq!(request["request_id"], "request-1");
    assert_eq!(request["payload"]["label"], "start");
}

#[test]
fn post_command_rejects_a_mismatched_response_request_id() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("listener should bind");
    let addr = listener.local_addr().expect("listener addr should load");
    thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("connection should arrive");
        let _ = read_http_request(&mut stream);
        write_http_response(
            &mut stream,
            200,
            r#"{"protocol_version":"stateful.v2","contract_revision":"lease-1","request_id":"other-request","payload":{"accepted":true}}"#,
        );
    });

    let error = post_command::<_, Reply>(
        &local_runtime(format!("http://{addr}")),
        "/v2/tasks/start",
        &command_identity(),
        &Payload { label: "start" },
    )
    .expect_err("mismatched response request id should be rejected");

    assert_eq!(error.reason_code, "protocol_mismatch");
    assert!(error.message.contains("request_id"));
}

#[test]
fn post_command_preserves_v2_error_fields() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("listener should bind");
    let addr = listener.local_addr().expect("listener addr should load");
    thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("connection should arrive");
        let _ = read_http_request(&mut stream);
        write_http_response(
            &mut stream,
            409,
            r#"{"protocol_version":"stateful.v2","contract_revision":"lease-1","decision":"error","reason_code":"reread_required","message":"read again"}"#,
        );
    });

    let error = post_command::<_, Reply>(
        &local_runtime(format!("http://{addr}")),
        "/v2/writes/prepare",
        &command_identity(),
        &Payload { label: "prepare" },
    )
    .expect_err("server error should be returned");

    assert_eq!(error.status_code, Some(409));
    assert_eq!(error.reason_code, "reread_required");
    assert_eq!(error.message, "read again");
    assert_eq!(error.request_id, None);
}

#[test]
fn get_payload_decodes_v2_status_payload() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("listener should bind");
    let addr = listener.local_addr().expect("listener addr should load");
    thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("connection should arrive");
        let request = read_http_request(&mut stream);
        assert!(request.starts_with("GET /v2/status HTTP/1.1"));
        write_http_response(
            &mut stream,
            200,
            r#"{"protocol_version":"stateful.v2","contract_revision":"lease-1","request_id":null,"payload":{"accepted":true}}"#,
        );
    });

    let reply: Reply = get_payload(&local_runtime(format!("http://{addr}")), "/v2/status")
        .expect("status should decode");

    assert_eq!(reply, Reply { accepted: true });
}

#[test]
fn stale_runtime_does_not_connect_or_send_bearer_for_get_or_post() {
    assert_stale_runtime_blocks_request(|runtime| {
        get_payload::<Reply>(runtime, "/v2/status").map(|_| ())
    });
    assert_stale_runtime_blocks_request(|runtime| {
        post_command::<_, Reply>(
            runtime,
            "/v2/tasks/start",
            &command_identity(),
            &Payload { label: "start" },
        )
        .map(|_| ())
    });
}

fn assert_stale_runtime_blocks_request(
    request: impl FnOnce(&ServerRuntime) -> Result<(), stateful_cli::CommandError>,
) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("listener should bind");
    let addr = listener.local_addr().expect("listener addr should load");
    listener
        .set_nonblocking(true)
        .expect("listener should become nonblocking");
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let started = std::time::Instant::now();
        while started.elapsed() < Duration::from_millis(250) {
            match listener.accept() {
                Ok((mut stream, _)) => {
                    stream
                        .set_read_timeout(Some(Duration::from_millis(50)))
                        .expect("stream timeout should set");
                    let mut request = String::new();
                    let _ = stream.read_to_string(&mut request);
                    tx.send(Some(request)).expect("connection should report");
                    return;
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_millis(5));
                }
                Err(error) => panic!("listener accept should succeed: {error}"),
            }
        }
        tx.send(None).expect("no connection should report");
    });

    let runtime = ServerRuntime::new(
        format!("http://{addr}"),
        "secret-token",
        "workspace-1",
        std::process::id(),
        "mismatched-process-start-identity",
    );
    let error = request(&runtime).expect_err("stale runtime must fail closed");

    assert!(error.message.contains("process identity"));
    assert_eq!(
        rx.recv_timeout(Duration::from_secs(1))
            .expect("listener outcome should arrive"),
        None,
        "stale listener must not receive a connection or bearer bytes"
    );
}

fn local_runtime(base_url: impl Into<String>) -> ServerRuntime {
    let pid = std::process::id();
    ServerRuntime::new(
        base_url,
        "secret-token",
        "workspace-1",
        pid,
        process_start_identity_for_pid(pid).expect("current process should have an identity"),
    )
}

fn command_identity() -> CommandIdentity {
    CommandIdentity::new(
        "task-1",
        "request-1",
        "2026-08-02T00:00:00Z",
        AgentIdentity {
            agent_id: "agent-1".to_string(),
            turn_id: Some("turn-1".to_string()),
            actor_id: "actor-1".to_string(),
            actor_type: ActorType::Agent,
            owner_id: None,
            parent_agent_id: None,
            parent_actor_id: None,
        },
        WorkspaceIdentity {
            root: "/workspace".to_string(),
            workspace_id: "workspace-1".to_string(),
            repo_id: "repo-1".to_string(),
            worktree_id: "worktree-1".to_string(),
            branch: "main".to_string(),
        },
        SourceRef {
            kind: SourceKind::Cli,
            event: "test".to_string(),
            tool_name: Some("runtime-test".to_string()),
            source_ref: "test".to_string(),
        },
    )
}

fn read_http_request(stream: &mut std::net::TcpStream) -> String {
    let mut request = Vec::new();
    let mut byte = [0_u8; 1];
    while !request.ends_with(b"\r\n\r\n") {
        stream
            .read_exact(&mut byte)
            .expect("request headers should arrive");
        request.push(byte[0]);
    }
    let headers = String::from_utf8(request.clone()).expect("request headers should be UTF-8");
    let content_length = headers
        .lines()
        .find_map(|line| line.strip_prefix("Content-Length: "))
        .and_then(|length| length.parse::<usize>().ok())
        .unwrap_or(0);
    let mut body = vec![0_u8; content_length];
    stream
        .read_exact(&mut body)
        .expect("request body should arrive");
    request.extend(body);
    String::from_utf8(request).expect("request should be UTF-8")
}

fn request_body(request: &str) -> &str {
    request
        .split_once("\r\n\r\n")
        .map(|(_, body)| body)
        .expect("request should include a body delimiter")
}

fn write_http_response(stream: &mut std::net::TcpStream, status: u16, body: &str) {
    let reason = match status {
        200 => "OK",
        409 => "Conflict",
        _ => "Unexpected",
    };
    write!(
        stream,
        "HTTP/1.1 {status} {reason}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
    .expect("response should write");
}
