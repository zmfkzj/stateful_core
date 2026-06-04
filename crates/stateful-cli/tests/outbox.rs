use std::{
    fs,
    io::{Read, Write},
    net::TcpListener,
    process::Command,
    sync::mpsc,
    thread,
    time::Duration,
};

use stateful_cli::{
    GlobalPaths, ServerRuntime, sync_outbox_in_repo_with_runtime, write_global_runtime_file,
};

#[test]
fn sync_outbox_posts_pending_events_in_sequence_order_and_removes_file() {
    let temp_root =
        std::env::temp_dir().join(format!("stateful-outbox-test-{}", std::process::id()));
    if temp_root.exists() {
        fs::remove_dir_all(&temp_root).expect("old temp root should be removable");
    }
    fs::create_dir_all(temp_root.join(".stateful_core/outbox"))
        .expect("outbox dir should be creatable");

    let listener = TcpListener::bind("127.0.0.1:0").expect("listener should bind");
    let addr = listener.local_addr().expect("listener addr should load");
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        for _ in 0..2 {
            let (mut stream, _) = listener.accept().expect("connection should arrive");
            let request = read_http_request(&mut stream);
            tx.send(request).expect("request should send to test");
            write_json_response(&mut stream, r#"{"status":"ok","sync_status":"synced"}"#);
        }
    });

    let runtime = ServerRuntime::new(format!("http://{addr}"), "secret-token", "w1", 42);
    let outbox_file = temp_root.join(".stateful_core/outbox/s1.jsonl");
    fs::write(
        &outbox_file,
        r#"{"outbox_id":"outbox-2","event_type":"HeartbeatObserved","session_id":"s1","actor_id":"a1","workspace_id":"w1","sequence":2,"created_at":"2026-05-31T00:00:02Z","payload":{"n":2},"sync_status":"pending"}
{"outbox_id":"outbox-1","event_type":"HeartbeatObserved","session_id":"s1","actor_id":"a1","workspace_id":"w1","sequence":1,"created_at":"2026-05-31T00:00:01Z","payload":{"n":1},"sync_status":"pending"}
"#,
    )
    .expect("outbox file should write");

    let synced =
        sync_outbox_in_repo_with_runtime(&temp_root, &runtime).expect("outbox should sync");

    assert_eq!(synced, 2);
    assert!(!outbox_file.exists());

    let first = rx
        .recv_timeout(Duration::from_secs(2))
        .expect("first request should arrive");
    let second = rx
        .recv_timeout(Duration::from_secs(2))
        .expect("second request should arrive");
    assert!(first.contains("POST /v1/outbox/sync HTTP/1.1"));
    assert!(first.contains("\"outbox_id\":\"outbox-1\""));
    assert!(first.contains("\"sequence\":1"));
    assert!(second.contains("\"outbox_id\":\"outbox-2\""));
    assert!(second.contains("\"sequence\":2"));

    fs::remove_dir_all(&temp_root).expect("temp root should be removable");
}

#[test]
fn sync_outbox_command_discovers_global_runtime_file() {
    let temp_root = std::env::temp_dir().join(format!(
        "stateful-outbox-global-runtime-test-{}",
        std::process::id()
    ));
    if temp_root.exists() {
        fs::remove_dir_all(&temp_root).expect("old temp root should be removable");
    }
    fs::create_dir_all(temp_root.join(".stateful_core/outbox"))
        .expect("outbox dir should be creatable");
    let paths = GlobalPaths::new(temp_root.join("home"));

    let listener = TcpListener::bind("127.0.0.1:0").expect("listener should bind");
    let addr = listener.local_addr().expect("listener addr should load");
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("connection should arrive");
        let request = read_http_request(&mut stream);
        tx.send(request).expect("request should send to test");
        write_json_response(&mut stream, r#"{"status":"ok","sync_status":"synced"}"#);
    });

    let runtime = ServerRuntime::new(format!("http://{addr}"), "secret-token", "global-w", 42);
    write_global_runtime_file(&paths, &runtime).expect("global runtime file should write");
    let outbox_file = temp_root.join(".stateful_core/outbox/s1.jsonl");
    fs::write(
        &outbox_file,
        r#"{"outbox_id":"outbox-global","event_type":"HeartbeatObserved","session_id":"s1","actor_id":"a1","workspace_id":"global-w","sequence":1,"created_at":"2026-05-31T00:00:01Z","payload":{"n":1},"sync_status":"pending"}
"#,
    )
    .expect("outbox file should write");

    let output = Command::new(env!("CARGO_BIN_EXE_stateful"))
        .arg("sync-outbox")
        .current_dir(&temp_root)
        .env_clear()
        .env("STATEFUL_HOME", &paths.home)
        .output()
        .expect("stateful sync-outbox should run");

    assert!(
        output.status.success(),
        "stateful sync-outbox failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!outbox_file.exists());
    assert!(String::from_utf8_lossy(&output.stdout).contains("\"synced\":1"));

    let request = rx
        .recv_timeout(Duration::from_secs(2))
        .expect("captured request should arrive");
    assert!(request.contains("POST /v1/outbox/sync HTTP/1.1"));
    assert!(request.contains("Authorization: Bearer secret-token"));
    assert!(request.contains("\"outbox_id\":\"outbox-global\""));

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
