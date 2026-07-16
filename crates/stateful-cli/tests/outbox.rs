use std::{
    fs,
    io::{Read, Write},
    net::TcpListener,
    path::Path,
    process::Command,
    sync::mpsc,
    thread,
    time::Duration,
};

use stateful_cli::{
    GlobalPaths, ServerRuntime, sync_outbox_with_runtime, write_global_runtime_file,
};

fn paths_for_temp_root(temp_root: &Path) -> GlobalPaths {
    GlobalPaths::new(temp_root.join("home"))
}

#[test]
fn sync_outbox_posts_pending_events_in_sequence_order_and_removes_file() {
    let temp = tempfile::tempdir().expect("temp dir should create");
    let temp_root = temp.path();
    fs::create_dir_all(temp_root.join(".git")).expect("git marker should write");
    let paths = paths_for_temp_root(temp_root);
    fs::create_dir_all(&paths.outbox_dir).expect("outbox dir should be creatable");

    let listener = TcpListener::bind("127.0.0.1:0").expect("listener should bind");
    let addr = listener.local_addr().expect("listener addr should load");
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        for _ in 0..2 {
            let (mut stream, request) = accept_v2_request(&listener);
            tx.send(request).expect("request should send to test");
            write_json_response(&mut stream, r#"{"status":"ok","sync_status":"synced"}"#);
        }
    });

    let runtime = ServerRuntime::new(format!("http://{addr}"), "secret-token", "w1", 42);
    let outbox_file = paths.outbox_dir.join("s1.jsonl");
    write_pending_records(
        &outbox_file,
        &[("outbox-2", "s1", "w1", 2), ("outbox-1", "s1", "w1", 1)],
    );
    let synced = sync_outbox_with_runtime(&paths, &runtime).expect("outbox should sync");

    assert_eq!(synced, 2);
    assert!(!outbox_file.exists());

    let first = rx
        .recv_timeout(Duration::from_secs(2))
        .expect("first request should arrive");
    let second = rx
        .recv_timeout(Duration::from_secs(2))
        .expect("second request should arrive");
    assert!(first.contains("POST /v2/outbox/sync HTTP/1.1"));
    assert!(first.contains("\"sequence\":1"));
    assert!(second.contains("\"outbox_id\":\"outbox-2\""));
    assert!(second.contains("\"sequence\":2"));
}

#[test]
fn recovery_outbox_preserves_original_request_id_and_retries_idempotently() {
    let temp = tempfile::tempdir().expect("temp dir should create");
    let paths = paths_for_temp_root(temp.path());
    fs::create_dir_all(&paths.outbox_dir).expect("outbox dir should be creatable");
    let listener = TcpListener::bind("127.0.0.1:0").expect("listener should bind");
    let addr = listener.local_addr().expect("listener address should load");
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let (first, request) = accept_v2_request(&listener);
        tx.send(request).expect("first request should send to test");
        drop(first);

        let (mut second, request) = accept_v2_request(&listener);
        tx.send(request)
            .expect("second request should send to test");
        write_json_response(
            &mut second,
            r#"{"outbox_id":"outbox-1","status":"ok","sync_status":"synced","duplicate":true}"#,
        );
    });

    let original = serde_json::json!({
        "protocol_version": "stateful.v2",
        "request_id": "018f1a33-e3c1-7000-b2a6-d16cc4f05a52",
        "observed_at": "2026-05-31T00:00:01Z",
        "agent": {
            "agent_id": "s1",
            "actor_id": "s1",
            "actor_type": "agent"
        },
        "workspace": {
            "root": "unknown",
            "workspace_id": "w1",
            "repo_id": "unknown",
            "worktree_id": "unknown",
            "branch": "unknown"
        },
        "source": {
            "kind": "cli",
            "event": "outbox_sync",
            "source_ref": "stateful-cli"
        },
        "payload": {
            "event_type": "AgentHeartbeatQueued",
            "outbox_id": "outbox-1",
            "sequence": 1,
            "payload": {"reason": "offline"}
        }
    });
    let frozen_original = serde_json::to_string(&original).expect("request should serialize");
    let outbox_file = paths.outbox_dir.join("s1.jsonl");
    fs::write(
        &outbox_file,
        serde_json::json!({
            "outbox_id": "outbox-1",
            "agent_id": "s1",
            "workspace_id": "w1",
            "sequence": 1,
            "route": "/v2/outbox/sync",
            "request_id": "018f1a33-e3c1-7000-b2a6-d16cc4f05a52",
            "request_envelope": frozen_original,
            "sync_status": "pending"
        })
        .to_string(),
    )
    .expect("outbox fixture should write");

    let runtime = ServerRuntime::new(format!("http://{addr}"), "secret-token", "w1", 42);
    sync_outbox_with_runtime(&paths, &runtime)
        .expect_err("disconnected replay should remain pending for retry");
    assert!(outbox_file.exists(), "failed replay should remain pending");
    let pending: serde_json::Value = serde_json::from_str(
        &fs::read_to_string(&outbox_file).expect("pending outbox should read"),
    )
    .expect("pending record should parse");
    assert_eq!(pending["attempts"], 1);
    assert_eq!(pending["sync_status"], "pending");
    assert_eq!(pending["request_id"], original["request_id"]);
    assert_eq!(pending["request_envelope"], frozen_original);

    assert_eq!(
        sync_outbox_with_runtime(&paths, &runtime).expect("duplicate receipt should sync"),
        1
    );
    assert!(
        !outbox_file.exists(),
        "success receipt should clear pending outbox"
    );

    let first = rx
        .recv_timeout(Duration::from_secs(2))
        .expect("first request should arrive");
    let second = rx
        .recv_timeout(Duration::from_secs(2))
        .expect("second request should arrive");
    assert!(first.contains("POST /v2/outbox/sync HTTP/1.1"));
    assert!(second.contains("POST /v2/outbox/sync HTTP/1.1"));
    for request in [&first, &second] {
        let body = request.split_once("\r\n\r\n").expect("body separator").1;
        assert_eq!(body, frozen_original);
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(body).expect("request body should parse")["request_id"],
            original["request_id"]
        );
    }
}

#[cfg(unix)]
#[test]
fn sync_outbox_refuses_symlinked_outbox_directory() {
    let temp = temp_root("stateful-outbox-symlink-test");
    let temp_root = temp.path();
    let paths = paths_for_temp_root(temp_root);
    let victim_outbox = temp_root.join("victim-outbox");
    fs::create_dir_all(paths.home.clone()).expect("stateful dir should be creatable");
    fs::create_dir_all(&victim_outbox).expect("victim outbox should be creatable");
    let victim_file = victim_outbox.join("s1.jsonl");
    fs::write(
        &victim_file,
        r#"{"outbox_id":"outbox-victim","event_type":"HeartbeatObserved","agent_id":"s1","actor_id":"a1","workspace_id":"w1","sequence":1,"created_at":"2026-05-31T00:00:01Z","payload":{"n":1},"sync_status":"pending"}
"#,
    )
    .expect("victim outbox file should write");
    std::os::unix::fs::symlink(&victim_outbox, paths.outbox_dir.clone())
        .expect("outbox symlink should be creatable");

    let runtime = ServerRuntime::new("http://127.0.0.1:9", "secret-token", "w1", 42);
    let error =
        sync_outbox_with_runtime(&paths, &runtime).expect_err("symlinked outbox should be refused");

    assert!(
        error
            .to_string()
            .contains("symlinked global outbox directory")
    );
    assert!(
        victim_file.exists(),
        "sync should not claim or remove files outside the repo"
    );
    assert!(
        victim_outbox
            .read_dir()
            .expect("victim outbox should list")
            .all(|entry| !entry
                .expect("victim entry should read")
                .file_name()
                .to_string_lossy()
                .contains(".syncing-")),
        "sync should not rename files outside the repo"
    );
}

#[cfg(unix)]
#[test]
fn sync_outbox_refuses_symlinked_outbox_file() {
    let temp = temp_root("stateful-outbox-file-symlink-test");
    let temp_root = temp.path();
    let paths = paths_for_temp_root(temp_root);
    let victim_dir = temp_root.join("victim");
    fs::create_dir_all(paths.outbox_dir.clone()).expect("outbox dir should be creatable");
    fs::create_dir_all(&victim_dir).expect("victim dir should be creatable");
    let victim_file = victim_dir.join("victim.jsonl");
    fs::write(
        &victim_file,
        r#"{"outbox_id":"outbox-victim","event_type":"HeartbeatObserved","agent_id":"s1","actor_id":"a1","workspace_id":"w1","sequence":1,"created_at":"2026-05-31T00:00:01Z","payload":{"n":1},"sync_status":"pending"}
"#,
    )
    .expect("victim file should write");
    let outbox_link = paths.outbox_dir.join("s1.jsonl");
    std::os::unix::fs::symlink(&victim_file, &outbox_link)
        .expect("outbox file symlink should be creatable");

    let runtime = ServerRuntime::new("http://127.0.0.1:9", "secret-token", "w1", 42);
    let error = sync_outbox_with_runtime(&paths, &runtime)
        .expect_err("symlinked outbox file should be refused");

    assert!(error.to_string().contains("symlinked outbox file"));
    assert!(
        victim_file.exists(),
        "sync should not remove the symlink target"
    );
    assert!(
        fs::symlink_metadata(outbox_link)
            .expect("outbox symlink should remain")
            .file_type()
            .is_symlink(),
        "sync should not rename the symlink"
    );
}

#[cfg(unix)]
#[test]
fn sync_outbox_refuses_hard_linked_outbox_file() {
    let temp = temp_root("stateful-outbox-file-hardlink-test");
    let temp_root = temp.path();
    let paths = paths_for_temp_root(temp_root);
    let victim_dir = temp_root.join("victim");
    fs::create_dir_all(paths.outbox_dir.clone()).expect("outbox dir should be creatable");
    fs::create_dir_all(&victim_dir).expect("victim dir should be creatable");
    let victim_file = victim_dir.join("victim.jsonl");
    fs::write(
        &victim_file,
        r#"{"outbox_id":"outbox-victim","event_type":"HeartbeatObserved","agent_id":"s1","actor_id":"a1","workspace_id":"w1","sequence":1,"created_at":"2026-05-31T00:00:01Z","payload":{"n":1},"sync_status":"pending"}
"#,
    )
    .expect("victim file should write");
    let outbox_file = paths.outbox_dir.join("s1.jsonl");
    fs::hard_link(&victim_file, &outbox_file).expect("outbox hard link should be creatable");

    let runtime = ServerRuntime::new("http://127.0.0.1:9", "secret-token", "w1", 42);
    let error = sync_outbox_with_runtime(&paths, &runtime)
        .expect_err("hard-linked outbox file should be refused");

    assert!(error.to_string().contains("hard-linked outbox file"));
    assert!(
        victim_file.exists(),
        "sync should not remove the external hard link peer"
    );
    assert!(
        outbox_file.exists(),
        "sync should not rename the hard-linked outbox path"
    );
}

#[test]
fn sync_outbox_skips_malformed_lines_and_posts_valid_pending_records() {
    let temp = temp_root("stateful-outbox-malformed-line-test");
    let temp_root = temp.path();
    let paths = paths_for_temp_root(temp_root);
    fs::create_dir_all(&paths.outbox_dir).expect("outbox dir should be creatable");

    let listener = TcpListener::bind("127.0.0.1:0").expect("listener should bind");
    let addr = listener.local_addr().expect("listener addr should load");
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let (mut stream, request) = accept_v2_request(&listener);
        tx.send(request).expect("request should send to test");
        write_json_response(&mut stream, r#"{"status":"ok","sync_status":"synced"}"#);
    });

    let runtime = ServerRuntime::new(format!("http://{addr}"), "secret-token", "w1", 42);
    let outbox_file = paths.outbox_dir.join("s1.jsonl");
    write_pending_records(&outbox_file, &[("outbox-valid", "s1", "w1", 1)]);
    let valid = fs::read_to_string(&outbox_file).expect("valid outbox record should read");
    fs::write(&outbox_file, format!("not-json\n{valid}")).expect("malformed fixture should write");

    let synced = sync_outbox_with_runtime(&paths, &runtime).expect("outbox should sync");

    assert_eq!(synced, 1);
    assert!(!outbox_file.exists());
    let request = rx
        .recv_timeout(Duration::from_secs(2))
        .expect("request should arrive");
    assert!(request.contains("\"outbox_id\":\"outbox-valid\""));
}

#[test]
fn sync_outbox_recovers_stale_lock_before_wait_timeout() {
    let temp = temp_root("stateful-outbox-stale-lock-test");
    let temp_root = temp.path();
    let paths = paths_for_temp_root(temp_root);
    fs::create_dir_all(paths.outbox_dir.join(".lock")).expect("stale lock dir should be creatable");
    let heartbeat = paths.outbox_dir.join(".lock/heartbeat");
    fs::write(&heartbeat, "stale\n").expect("stale heartbeat should write");
    let _ = Command::new("touch")
        .args(["-t", "200001010000"])
        .arg(&heartbeat)
        .status();

    let listener = TcpListener::bind("127.0.0.1:0").expect("listener should bind");
    let addr = listener.local_addr().expect("listener addr should load");
    thread::spawn(move || {
        let (mut stream, _request) = accept_v2_request(&listener);
        write_json_response(&mut stream, r#"{"status":"ok","sync_status":"synced"}"#);
    });

    let runtime = ServerRuntime::new(format!("http://{addr}"), "secret-token", "w1", 42);
    write_pending_records(
        &paths.outbox_dir.join("s1.jsonl"),
        &[("outbox-lock", "s1", "w1", 1)],
    );

    let synced = sync_outbox_with_runtime(&paths, &runtime).expect("outbox should sync");

    assert_eq!(synced, 1);
    assert!(!paths.outbox_dir.join(".lock").exists());
}

#[test]
fn sync_outbox_command_discovers_global_runtime_file() {
    let temp = tempfile::tempdir().expect("temp dir should create");
    let temp_root = temp.path();
    fs::create_dir_all(temp_root.join(".git")).expect("git marker should write");
    let paths = paths_for_temp_root(temp_root);
    fs::create_dir_all(&paths.outbox_dir).expect("outbox dir should be creatable");

    let listener = TcpListener::bind("127.0.0.1:0").expect("listener should bind");
    let addr = listener.local_addr().expect("listener addr should load");
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let (mut stream, request) = accept_v2_request(&listener);
        tx.send(request).expect("request should send to test");
        write_json_response(&mut stream, r#"{"status":"ok","sync_status":"synced"}"#);
    });

    let runtime = ServerRuntime::new(format!("http://{addr}"), "secret-token", "global-w", 42);
    write_global_runtime_file(&paths, &runtime).expect("global runtime file should write");
    let outbox_file = paths.outbox_dir.join("s1.jsonl");
    write_pending_records(&outbox_file, &[("outbox-global", "s1", "global-w", 1)]);

    let output = Command::new(env!("CARGO_BIN_EXE_stateful"))
        .arg("sync-outbox")
        .current_dir(temp_root)
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
    assert!(request.contains("POST /v2/outbox/sync HTTP/1.1"));
    assert!(request.contains("Authorization: Bearer secret-token"));
    assert!(request.contains("\"outbox_id\":\"outbox-global\""));
}

#[test]
fn sync_outbox_preserves_records_queued_while_file_is_in_flight() {
    let temp = temp_root("stateful-outbox-race-test");
    let temp_root = temp.path();
    let paths = paths_for_temp_root(temp_root);
    fs::create_dir_all(&paths.outbox_dir).expect("outbox dir should be creatable");

    let listener = TcpListener::bind("127.0.0.1:0").expect("listener should bind");
    let addr = listener.local_addr().expect("listener addr should load");
    let outbox_file = paths.outbox_dir.join("s1.jsonl");
    let outbox_file_for_server = outbox_file.clone();
    thread::spawn(move || {
        let (mut stream, _request) = accept_v2_request(&listener);
        write_pending_records(&outbox_file_for_server, &[("outbox-late", "s1", "w1", 2)]);
        write_json_response(&mut stream, r#"{"status":"ok","sync_status":"synced"}"#);
    });

    let runtime = ServerRuntime::new(format!("http://{addr}"), "secret-token", "w1", 42);
    write_pending_records(&outbox_file, &[("outbox-1", "s1", "w1", 1)]);

    let synced = sync_outbox_with_runtime(&paths, &runtime).expect("outbox should sync");

    assert_eq!(synced, 1);
    let remaining = fs::read_to_string(&outbox_file).expect("late record should remain pending");
    assert!(remaining.contains("\"outbox_id\":\"outbox-late\""));
}

#[test]
fn sync_outbox_requeues_only_unsent_records_after_failure() {
    let temp = temp_root("stateful-outbox-partial-failure-test");
    let temp_root = temp.path();
    let paths = paths_for_temp_root(temp_root);
    fs::create_dir_all(&paths.outbox_dir).expect("outbox dir should be creatable");

    let listener = TcpListener::bind("127.0.0.1:0").expect("listener should bind");
    let addr = listener.local_addr().expect("listener addr should load");
    thread::spawn(move || {
        let (mut first, _first_request) = accept_v2_request(&listener);
        write_json_response(&mut first, r#"{"status":"ok","sync_status":"synced"}"#);

        let (mut second, _second_request) = accept_v2_request(&listener);
        let body = r#"{"status":"error","message":"boom"}"#;
        let response = format!(
            "HTTP/1.1 500 Internal Server Error\r\nContent-Length: {}\r\n\r\n{}",
            body.len(),
            body
        );
        second
            .write_all(response.as_bytes())
            .expect("failure response should write");
    });

    let runtime = ServerRuntime::new(format!("http://{addr}"), "secret-token", "w1", 42);
    let outbox_file = paths.outbox_dir.join("s1.jsonl");
    write_pending_records(
        &outbox_file,
        &[("outbox-1", "s1", "w1", 1), ("outbox-2", "s1", "w1", 2)],
    );

    let error = sync_outbox_with_runtime(&paths, &runtime)
        .expect_err("outbox sync should fail on server error");

    assert!(error.to_string().contains("outbox sync failed with HTTP 500"));
    let remaining = fs::read_to_string(&outbox_file).expect("failed record should remain pending");
    assert!(!remaining.contains("\"outbox_id\":\"outbox-1\""));
    assert!(remaining.contains("\"outbox_id\":\"outbox-2\""));
}

#[test]
fn sync_outbox_discards_deterministic_client_rejections() {
    let temp = temp_root("stateful-outbox-client-rejection-test");
    let paths = paths_for_temp_root(temp.path());
    fs::create_dir_all(&paths.outbox_dir).expect("outbox dir should be creatable");
    let listener = TcpListener::bind("127.0.0.1:0").expect("listener should bind");
    let addr = listener.local_addr().expect("listener addr should load");
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let (mut stream, request) = accept_v2_request(&listener);
        tx.send(request).expect("request should send to test");
        let body = r#"{"protocol_version":"stateful.v2","request_id":"018f1a33-e3c1-7000-b2a6-000000000001","error":{"code":"not_found","message":"missing"}}"#;
        let response = format!(
            "HTTP/1.1 404 Not Found\r\nContent-Length: {}\r\n\r\n{}",
            body.len(),
            body
        );
        stream
            .write_all(response.as_bytes())
            .expect("client rejection should write");
    });
    let runtime = ServerRuntime::new(format!("http://{addr}"), "secret-token", "w1", 42);
    let outbox_file = paths.outbox_dir.join("s1.jsonl");
    write_pending_records(&outbox_file, &[("outbox-rejected", "s1", "w1", 1)]);
    let frozen = serde_json::from_str::<serde_json::Value>(
        &fs::read_to_string(&outbox_file).expect("outbox should read"),
    )
    .expect("outbox record should parse")["request_envelope"]
        .as_str()
        .expect("frozen request should be a string")
        .to_string();

    assert_eq!(
        sync_outbox_with_runtime(&paths, &runtime)
            .expect("deterministic client rejection should not retry forever"),
        0
    );
    assert!(
        !outbox_file.exists(),
        "a deterministic client rejection must be removed rather than replayed forever"
    );
    assert_eq!(
        rx.recv_timeout(Duration::from_secs(2))
            .expect("client rejection request should arrive")
            .split_once("\r\n\r\n")
            .expect("request should contain a body")
            .1,
        frozen
    );
}

#[test]
fn sync_outbox_discards_a_structured_404_heartbeat_and_sends_the_next_record() {
    let temp = temp_root("stateful-outbox-heartbeat-404-test");
    let paths = paths_for_temp_root(temp.path());
    fs::create_dir_all(&paths.outbox_dir).expect("outbox dir should be creatable");
    let listener = TcpListener::bind("127.0.0.1:0").expect("listener should bind");
    let addr = listener.local_addr().expect("listener addr should load");
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        for (status, body) in [
            (
                "404 Not Found",
                r#"{"protocol_version":"stateful.v2","request_id":"018f1a33-e3c1-7000-b2a6-000000000001","error":{"code":"not_found","message":"missing"}}"#,
            ),
            ("200 OK", r#"{"status":"ok"}"#),
        ] {
            let (mut stream, request) = accept_v2_request(&listener);
            tx.send(request).expect("request should send to test");
            let response = format!(
                "HTTP/1.1 {status}\r\nContent-Length: {}\r\n\r\n{body}",
                body.len()
            );
            stream.write_all(response.as_bytes()).expect("response should write");
        }
    });
    let runtime = ServerRuntime::new(format!("http://{addr}"), "secret-token", "w1", 42);
    let outbox_file = paths.outbox_dir.join("s1.jsonl");
    write_pending_records(
        &outbox_file,
        &[("outbox-heartbeat", "s1", "w1", 1), ("outbox-next", "s1", "w1", 2)],
    );

    assert_eq!(sync_outbox_with_runtime(&paths, &runtime).expect("next record should sync"), 1);
    assert!(rx.recv_timeout(Duration::from_secs(2)).is_ok(), "heartbeat should arrive");
    assert!(rx.recv_timeout(Duration::from_secs(2)).is_ok(), "next record should arrive");
    assert!(!outbox_file.exists(), "404 heartbeat must not retain the outbox file");
}

#[test]
fn sync_outbox_recovers_stranded_claimed_files() {
    let temp = temp_root("stateful-outbox-stranded-claim-test");
    let temp_root = temp.path();
    let paths = paths_for_temp_root(temp_root);
    fs::create_dir_all(&paths.outbox_dir).expect("outbox dir should be creatable");

    let listener = TcpListener::bind("127.0.0.1:0").expect("listener should bind");
    let addr = listener.local_addr().expect("listener addr should load");
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        for _ in 0..2 {
            let (mut stream, request) = accept_v2_request(&listener);
            tx.send(request).expect("request should send to test");
            write_json_response(&mut stream, r#"{"status":"ok","sync_status":"synced"}"#);
        }
    });

    let runtime = ServerRuntime::new(format!("http://{addr}"), "secret-token", "w1", 42);
    let outbox_file = paths.outbox_dir.join("s1.jsonl");
    let claimed_file = paths.outbox_dir.join("s1.jsonl.syncing-old");
    write_pending_records(&outbox_file, &[("outbox-base", "s1", "w1", 2)]);
    write_pending_records(&claimed_file, &[("outbox-claimed", "s1", "w1", 1)]);

    let synced = sync_outbox_with_runtime(&paths, &runtime).expect("outbox should sync");

    assert_eq!(synced, 2);
    assert!(!outbox_file.exists());
    assert!(!claimed_file.exists());
    let first = rx
        .recv_timeout(Duration::from_secs(2))
        .expect("first request should arrive");
    let second = rx
        .recv_timeout(Duration::from_secs(2))
        .expect("second request should arrive");
    assert!(first.contains("\"outbox_id\":\"outbox-claimed\""));
    assert!(second.contains("\"outbox_id\":\"outbox-base\""));
}

#[cfg(unix)]
#[test]
fn sync_outbox_does_not_trust_symlinked_active_claim_marker() {
    let temp = temp_root("stateful-outbox-symlink-active-claim-test");
    let temp_root = temp.path();
    let paths = paths_for_temp_root(temp_root);
    fs::create_dir_all(&paths.outbox_dir).expect("outbox dir should be creatable");

    let listener = TcpListener::bind("127.0.0.1:0").expect("listener should bind");
    let addr = listener.local_addr().expect("listener addr should load");
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        for _ in 0..2 {
            let (mut stream, request) = accept_v2_request(&listener);
            tx.send(request).expect("request should send to test");
            write_json_response(&mut stream, r#"{"status":"ok","sync_status":"synced"}"#);
        }
    });

    let runtime = ServerRuntime::new(format!("http://{addr}"), "secret-token", "w1", 42);
    let outbox_file = paths.outbox_dir.join("s1.jsonl");
    let claimed_file = paths.outbox_dir.join("s1.jsonl.syncing-old");
    let active_marker = paths.outbox_dir.join("s1.jsonl.syncing-old.active");
    write_pending_records(&outbox_file, &[("outbox-base", "s1", "w1", 2)]);
    write_pending_records(&claimed_file, &[("outbox-claimed", "s1", "w1", 1)]);
    std::os::unix::fs::symlink(&outbox_file, active_marker)
        .expect("active marker symlink should create");

    let synced = sync_outbox_with_runtime(&paths, &runtime).expect("outbox should sync");

    assert_eq!(synced, 2);
    assert!(!outbox_file.exists());
    assert!(!claimed_file.exists());
    let first = rx
        .recv_timeout(Duration::from_secs(2))
        .expect("first request should arrive");
    let second = rx
        .recv_timeout(Duration::from_secs(2))
        .expect("second request should arrive");
    assert!(first.contains("\"outbox_id\":\"outbox-claimed\""));
    assert!(second.contains("\"outbox_id\":\"outbox-base\""));
}

#[test]
fn sync_outbox_does_not_let_fake_active_claim_block_base_file() {
    let temp = temp_root("stateful-outbox-fake-active-claim-test");
    let temp_root = temp.path();
    let paths = paths_for_temp_root(temp_root);
    fs::create_dir_all(&paths.outbox_dir).expect("outbox dir should be creatable");

    let listener = TcpListener::bind("127.0.0.1:0").expect("listener should bind");
    let addr = listener.local_addr().expect("listener addr should load");
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let (mut stream, request) = accept_v2_request(&listener);
        tx.send(request).expect("request should send to test");
        write_json_response(&mut stream, r#"{"status":"ok","sync_status":"synced"}"#);
    });

    let runtime = ServerRuntime::new(format!("http://{addr}"), "secret-token", "w1", 42);
    let outbox_file = paths.outbox_dir.join("s1.jsonl");
    let claimed_file = paths.outbox_dir.join("s1.jsonl.syncing-spoof");
    let active_marker = paths.outbox_dir.join("s1.jsonl.syncing-spoof.active");
    write_pending_records(&outbox_file, &[("outbox-base", "s1", "w1", 1)]);
    fs::write(&claimed_file, "").expect("spoof claimed file should write");
    fs::write(&active_marker, "active\n").expect("spoof active marker should write");

    let synced = sync_outbox_with_runtime(&paths, &runtime).expect("outbox should sync");

    assert_eq!(synced, 1);
    assert!(!outbox_file.exists());
    let request = rx
        .recv_timeout(Duration::from_secs(2))
        .expect("request should arrive");
    assert!(request.contains("\"outbox_id\":\"outbox-base\""));
}

#[cfg(unix)]
#[test]
fn sync_outbox_does_not_trust_symlinked_lock_heartbeat() {
    let temp = temp_root("stateful-outbox-symlink-lock-heartbeat-test");
    let temp_root = temp.path();
    let paths = paths_for_temp_root(temp_root);
    fs::create_dir_all(paths.outbox_dir.join(".lock")).expect("lock dir should be creatable");
    let fresh_target = temp_root.join("fresh-heartbeat");
    fs::write(&fresh_target, "fresh\n").expect("fresh heartbeat target should write");
    std::os::unix::fs::symlink(&fresh_target, paths.outbox_dir.join(".lock/heartbeat"))
        .expect("heartbeat symlink should create");

    let listener = TcpListener::bind("127.0.0.1:0").expect("listener should bind");
    let addr = listener.local_addr().expect("listener addr should load");
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let (mut stream, request) = accept_v2_request(&listener);
        tx.send(request).expect("request should send to test");
        write_json_response(&mut stream, r#"{"status":"ok","sync_status":"synced"}"#);
    });

    let runtime = ServerRuntime::new(format!("http://{addr}"), "secret-token", "w1", 42);
    let outbox_file = paths.outbox_dir.join("s1.jsonl");
    write_pending_records(&outbox_file, &[("outbox-base", "s1", "w1", 1)]);

    let synced = sync_outbox_with_runtime(&paths, &runtime).expect("outbox should sync");

    assert_eq!(synced, 1);
    assert!(!outbox_file.exists());
    let request = rx
        .recv_timeout(Duration::from_secs(2))
        .expect("request should arrive");
    assert!(request.contains("\"outbox_id\":\"outbox-base\""));
}

#[cfg(unix)]
#[test]
fn sync_outbox_does_not_trust_symlinked_lock_directory() {
    let temp = temp_root("stateful-outbox-symlink-lock-dir-test");
    let temp_root = temp.path();
    let paths = paths_for_temp_root(temp_root);
    fs::create_dir_all(paths.outbox_dir.join("fake-lock"))
        .expect("fake lock dir should be creatable");
    fs::write(paths.outbox_dir.join("fake-lock/heartbeat"), "fresh\n")
        .expect("fake heartbeat should write");
    std::os::unix::fs::symlink("fake-lock", paths.outbox_dir.join(".lock"))
        .expect("lock symlink should create");

    let listener = TcpListener::bind("127.0.0.1:0").expect("listener should bind");
    let addr = listener.local_addr().expect("listener addr should load");
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let (mut stream, request) = accept_v2_request(&listener);
        tx.send(request).expect("request should send to test");
        write_json_response(&mut stream, r#"{"status":"ok","sync_status":"synced"}"#);
    });

    let runtime = ServerRuntime::new(format!("http://{addr}"), "secret-token", "w1", 42);
    let outbox_file = paths.outbox_dir.join("s1.jsonl");
    write_pending_records(&outbox_file, &[("outbox-base", "s1", "w1", 1)]);

    let synced = sync_outbox_with_runtime(&paths, &runtime).expect("outbox should sync");

    assert_eq!(synced, 1);
    assert!(!outbox_file.exists());
    let request = rx
        .recv_timeout(Duration::from_secs(2))
        .expect("request should arrive");
    assert!(request.contains("\"outbox_id\":\"outbox-base\""));
}

#[test]
fn sync_outbox_deduplicates_claimed_records_already_merged_into_base() {
    let temp = temp_root("stateful-outbox-duplicate-merge-test");
    let temp_root = temp.path();
    let paths = paths_for_temp_root(temp_root);
    fs::create_dir_all(&paths.outbox_dir).expect("outbox dir should be creatable");

    let listener = TcpListener::bind("127.0.0.1:0").expect("listener should bind");
    let addr = listener.local_addr().expect("listener addr should load");
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        for _ in 0..2 {
            let (mut stream, request) = accept_v2_request(&listener);
            tx.send(request).expect("request should send to test");
            write_json_response(&mut stream, r#"{"status":"ok","sync_status":"synced"}"#);
        }
    });

    let runtime = ServerRuntime::new(format!("http://{addr}"), "secret-token", "w1", 42);
    let outbox_file = paths.outbox_dir.join("s1.jsonl");
    let claimed_file = paths.outbox_dir.join("s1.jsonl.syncing-old");
    write_pending_records(
        &outbox_file,
        &[
            ("outbox-claimed", "s1", "w1", 1),
            ("outbox-base", "s1", "w1", 2),
        ],
    );
    write_pending_records(&claimed_file, &[("outbox-claimed", "s1", "w1", 1)]);

    let synced = sync_outbox_with_runtime(&paths, &runtime).expect("outbox should sync");

    assert_eq!(synced, 2);
    assert!(!outbox_file.exists());
    assert!(!claimed_file.exists());
    let first = rx
        .recv_timeout(Duration::from_secs(2))
        .expect("first request should arrive");
    let second = rx
        .recv_timeout(Duration::from_secs(2))
        .expect("second request should arrive");
    assert!(first.contains("\"outbox_id\":\"outbox-claimed\""));
    assert!(second.contains("\"outbox_id\":\"outbox-base\""));
    assert!(rx.recv_timeout(Duration::from_millis(100)).is_err());
}

fn write_pending_records(path: &Path, records: &[(&str, &str, &str, u64)]) {
    let contents = records
        .iter()
        .map(|(outbox_id, agent_id, workspace_id, sequence)| {
            let request_id = format!("018f1a33-e3c1-7000-b2a6-{sequence:012x}");
            let request = serde_json::json!({
                "protocol_version": "stateful.v2",
                "request_id": request_id,
                "observed_at": "2026-05-31T00:00:01Z",
                "agent": {"agent_id": agent_id, "actor_id": agent_id, "actor_type": "agent"},
                "workspace": {
                    "root": "unknown",
                    "workspace_id": workspace_id,
                    "repo_id": "unknown",
                    "worktree_id": "unknown",
                    "branch": "unknown"
                },
                "source": {"kind": "cli", "event": "outbox_sync", "source_ref": "stateful-cli"},
                "payload": {
                    "outbox_id": outbox_id,
                    "sequence": sequence,
                    "event_type": "HeartbeatObserved",
                    "payload": {"n": sequence}
                }
            });
            serde_json::json!({
                "outbox_id": outbox_id,
                "agent_id": agent_id,
                "workspace_id": workspace_id,
                "sequence": sequence,
                "route": "/v2/outbox/sync",
                "request_id": request_id,
                "request_envelope": serde_json::to_string(&request).expect("request should serialize"),
                "sync_status": "pending"
            })
            .to_string()
        })
        .collect::<Vec<_>>()
        .join("\n");
    fs::write(path, format!("{contents}\n")).expect("outbox file should write");
}

fn accept_v2_request(listener: &TcpListener) -> (std::net::TcpStream, String) {
    loop {
        let (mut stream, _) = listener.accept().expect("connection should arrive");
        let request = read_http_request(&mut stream);
        if request.starts_with("GET /v2/runtime/identity?") {
            write_json_response(
                &mut stream,
                r#"{"protocol_version":"stateful.v2","journal_schema_version":2,"coordination_mode":"awareness","pid":42,"workspace_id":"w1","workspace_version":1,"capabilities":["presence"]}"#,
            );
            continue;
        }
        return (stream, request);
    }
}

fn temp_root(label: &str) -> tempfile::TempDir {
    tempfile::Builder::new()
        .prefix(label)
        .tempdir()
        .expect("temp dir should create")
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
    let Some(content_length) = headers
        .lines()
        .find_map(|line| line.strip_prefix("Content-Length: "))
        .map(|value| value.parse::<usize>().expect("content length should parse"))
    else {
        return headers;
    };
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
