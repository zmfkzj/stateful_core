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
    let temp_root =
        std::env::temp_dir().join(format!("stateful-outbox-test-{}", std::process::id()));
    if temp_root.exists() {
        fs::remove_dir_all(&temp_root).expect("old temp root should be removable");
    }
    fs::create_dir_all(temp_root.join(".git")).expect("git marker should write");
    let paths = paths_for_temp_root(&temp_root);
    fs::create_dir_all(&paths.outbox_dir).expect("outbox dir should be creatable");

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
    let outbox_file = paths.outbox_dir.join("s1.jsonl");
    fs::write(
        &outbox_file,
        r#"{"outbox_id":"outbox-2","event_type":"HeartbeatObserved","agent_id":"s1","actor_id":"a1","workspace_id":"w1","sequence":2,"created_at":"2026-05-31T00:00:02Z","payload":{"n":2},"sync_status":"pending"}
{"outbox_id":"outbox-1","event_type":"HeartbeatObserved","agent_id":"s1","actor_id":"a1","workspace_id":"w1","sequence":1,"created_at":"2026-05-31T00:00:01Z","payload":{"n":1},"sync_status":"pending"}
"#,
    )
    .expect("outbox file should write");

    let synced = sync_outbox_with_runtime(&paths, &runtime).expect("outbox should sync");

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

#[cfg(unix)]
#[test]
fn sync_outbox_refuses_symlinked_outbox_directory() {
    let temp_root = temp_root("stateful-outbox-symlink-test");
    let paths = paths_for_temp_root(&temp_root);
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

    fs::remove_dir_all(&temp_root).expect("temp root should be removable");
}

#[cfg(unix)]
#[test]
fn sync_outbox_refuses_symlinked_outbox_file() {
    let temp_root = temp_root("stateful-outbox-file-symlink-test");
    let paths = paths_for_temp_root(&temp_root);
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

    fs::remove_dir_all(&temp_root).expect("temp root should be removable");
}

#[cfg(unix)]
#[test]
fn sync_outbox_refuses_hard_linked_outbox_file() {
    let temp_root = temp_root("stateful-outbox-file-hardlink-test");
    let paths = paths_for_temp_root(&temp_root);
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

    fs::remove_dir_all(&temp_root).expect("temp root should be removable");
}

#[test]
fn sync_outbox_skips_malformed_lines_and_posts_valid_pending_records() {
    let temp_root = temp_root("stateful-outbox-malformed-line-test");
    let paths = paths_for_temp_root(&temp_root);
    fs::create_dir_all(&paths.outbox_dir).expect("outbox dir should be creatable");

    let listener = TcpListener::bind("127.0.0.1:0").expect("listener should bind");
    let addr = listener.local_addr().expect("listener addr should load");
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("connection should arrive");
        let request = read_http_request(&mut stream);
        tx.send(request).expect("request should send to test");
        write_json_response(&mut stream, r#"{"status":"ok","sync_status":"synced"}"#);
    });

    let runtime = ServerRuntime::new(format!("http://{addr}"), "secret-token", "w1", 42);
    let outbox_file = paths.outbox_dir.join("s1.jsonl");
    fs::write(
        &outbox_file,
        r#"not-json
{"outbox_id":"outbox-valid","event_type":"HeartbeatObserved","agent_id":"s1","actor_id":"a1","workspace_id":"w1","sequence":1,"created_at":"2026-05-31T00:00:01Z","payload":{"n":1},"sync_status":"pending"}
"#,
    )
    .expect("outbox file should write");

    let synced = sync_outbox_with_runtime(&paths, &runtime).expect("outbox should sync");

    assert_eq!(synced, 1);
    assert!(!outbox_file.exists());
    let request = rx
        .recv_timeout(Duration::from_secs(2))
        .expect("request should arrive");
    assert!(request.contains("\"outbox_id\":\"outbox-valid\""));

    fs::remove_dir_all(&temp_root).expect("temp root should be removable");
}

#[test]
fn sync_outbox_recovers_stale_lock_before_wait_timeout() {
    let temp_root = temp_root("stateful-outbox-stale-lock-test");
    let paths = paths_for_temp_root(&temp_root);
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
        let (mut stream, _) = listener.accept().expect("connection should arrive");
        let _request = read_http_request(&mut stream);
        write_json_response(&mut stream, r#"{"status":"ok","sync_status":"synced"}"#);
    });

    let runtime = ServerRuntime::new(format!("http://{addr}"), "secret-token", "w1", 42);
    fs::write(
        paths.outbox_dir.join("s1.jsonl"),
        r#"{"outbox_id":"outbox-lock","event_type":"HeartbeatObserved","agent_id":"s1","actor_id":"a1","workspace_id":"w1","sequence":1,"created_at":"2026-05-31T00:00:01Z","payload":{"n":1},"sync_status":"pending"}
"#,
    )
    .expect("outbox file should write");

    let synced = sync_outbox_with_runtime(&paths, &runtime).expect("outbox should sync");

    assert_eq!(synced, 1);
    assert!(!paths.outbox_dir.join(".lock").exists());

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
    fs::create_dir_all(temp_root.join(".git")).expect("git marker should write");
    let paths = paths_for_temp_root(&temp_root);
    fs::create_dir_all(&paths.outbox_dir).expect("outbox dir should be creatable");

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
    let outbox_file = paths.outbox_dir.join("s1.jsonl");
    fs::write(
        &outbox_file,
        r#"{"outbox_id":"outbox-global","event_type":"HeartbeatObserved","agent_id":"s1","actor_id":"a1","workspace_id":"global-w","sequence":1,"created_at":"2026-05-31T00:00:01Z","payload":{"n":1},"sync_status":"pending"}
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

#[test]
fn sync_outbox_preserves_records_queued_while_file_is_in_flight() {
    let temp_root = temp_root("stateful-outbox-race-test");
    let paths = paths_for_temp_root(&temp_root);
    fs::create_dir_all(&paths.outbox_dir).expect("outbox dir should be creatable");

    let listener = TcpListener::bind("127.0.0.1:0").expect("listener should bind");
    let addr = listener.local_addr().expect("listener addr should load");
    let outbox_file = paths.outbox_dir.join("s1.jsonl");
    let outbox_file_for_server = outbox_file.clone();
    thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("connection should arrive");
        let _request = read_http_request(&mut stream);
        fs::write(
            &outbox_file_for_server,
            r#"{"outbox_id":"outbox-late","event_type":"HeartbeatObserved","agent_id":"s1","actor_id":"a1","workspace_id":"w1","sequence":2,"created_at":"2026-05-31T00:00:02Z","payload":{"n":2},"sync_status":"pending"}
"#,
        )
        .expect("late queued record should write");
        write_json_response(&mut stream, r#"{"status":"ok","sync_status":"synced"}"#);
    });

    let runtime = ServerRuntime::new(format!("http://{addr}"), "secret-token", "w1", 42);
    fs::write(
        &outbox_file,
        r#"{"outbox_id":"outbox-1","event_type":"HeartbeatObserved","agent_id":"s1","actor_id":"a1","workspace_id":"w1","sequence":1,"created_at":"2026-05-31T00:00:01Z","payload":{"n":1},"sync_status":"pending"}
"#,
    )
    .expect("outbox file should write");

    let synced = sync_outbox_with_runtime(&paths, &runtime).expect("outbox should sync");

    assert_eq!(synced, 1);
    let remaining = fs::read_to_string(&outbox_file).expect("late record should remain pending");
    assert!(remaining.contains("\"outbox_id\":\"outbox-late\""));

    fs::remove_dir_all(&temp_root).expect("temp root should be removable");
}

#[test]
fn sync_outbox_requeues_only_unsent_records_after_failure() {
    let temp_root = temp_root("stateful-outbox-partial-failure-test");
    let paths = paths_for_temp_root(&temp_root);
    fs::create_dir_all(&paths.outbox_dir).expect("outbox dir should be creatable");

    let listener = TcpListener::bind("127.0.0.1:0").expect("listener should bind");
    let addr = listener.local_addr().expect("listener addr should load");
    thread::spawn(move || {
        let (mut first, _) = listener.accept().expect("first connection should arrive");
        let _first_request = read_http_request(&mut first);
        write_json_response(&mut first, r#"{"status":"ok","sync_status":"synced"}"#);

        let (mut second, _) = listener.accept().expect("second connection should arrive");
        let _second_request = read_http_request(&mut second);
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
    fs::write(
        &outbox_file,
        r#"{"outbox_id":"outbox-1","event_type":"HeartbeatObserved","agent_id":"s1","actor_id":"a1","workspace_id":"w1","sequence":1,"created_at":"2026-05-31T00:00:01Z","payload":{"n":1},"sync_status":"pending"}
{"outbox_id":"outbox-2","event_type":"HeartbeatObserved","agent_id":"s1","actor_id":"a1","workspace_id":"w1","sequence":2,"created_at":"2026-05-31T00:00:02Z","payload":{"n":2},"sync_status":"pending"}
"#,
    )
    .expect("outbox file should write");

    let error = sync_outbox_with_runtime(&paths, &runtime)
        .expect_err("outbox sync should fail on server error");

    assert!(error.to_string().contains("outbox sync failed"));
    let remaining = fs::read_to_string(&outbox_file).expect("failed record should remain pending");
    assert!(!remaining.contains("\"outbox_id\":\"outbox-1\""));
    assert!(remaining.contains("\"outbox_id\":\"outbox-2\""));

    fs::remove_dir_all(&temp_root).expect("temp root should be removable");
}

#[test]
fn sync_outbox_recovers_stranded_claimed_files() {
    let temp_root = temp_root("stateful-outbox-stranded-claim-test");
    let paths = paths_for_temp_root(&temp_root);
    fs::create_dir_all(&paths.outbox_dir).expect("outbox dir should be creatable");

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
    let outbox_file = paths.outbox_dir.join("s1.jsonl");
    let claimed_file = paths.outbox_dir.join("s1.jsonl.syncing-old");
    fs::write(
        &outbox_file,
        r#"{"outbox_id":"outbox-base","event_type":"HeartbeatObserved","agent_id":"s1","actor_id":"a1","workspace_id":"w1","sequence":2,"created_at":"2026-05-31T00:00:02Z","payload":{"n":2},"sync_status":"pending"}
"#,
    )
    .expect("base outbox file should write");
    fs::write(
        &claimed_file,
        r#"{"outbox_id":"outbox-claimed","event_type":"HeartbeatObserved","agent_id":"s1","actor_id":"a1","workspace_id":"w1","sequence":1,"created_at":"2026-05-31T00:00:01Z","payload":{"n":1},"sync_status":"pending"}
"#,
    )
    .expect("claimed outbox file should write");

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

    fs::remove_dir_all(&temp_root).expect("temp root should be removable");
}

#[cfg(unix)]
#[test]
fn sync_outbox_does_not_trust_symlinked_active_claim_marker() {
    let temp_root = temp_root("stateful-outbox-symlink-active-claim-test");
    let paths = paths_for_temp_root(&temp_root);
    fs::create_dir_all(&paths.outbox_dir).expect("outbox dir should be creatable");

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
    let outbox_file = paths.outbox_dir.join("s1.jsonl");
    let claimed_file = paths.outbox_dir.join("s1.jsonl.syncing-old");
    let active_marker = paths.outbox_dir.join("s1.jsonl.syncing-old.active");
    fs::write(
        &outbox_file,
        r#"{"outbox_id":"outbox-base","event_type":"HeartbeatObserved","agent_id":"s1","actor_id":"a1","workspace_id":"w1","sequence":2,"created_at":"2026-05-31T00:00:02Z","payload":{"n":2},"sync_status":"pending"}
"#,
    )
    .expect("base outbox file should write");
    fs::write(
        &claimed_file,
        r#"{"outbox_id":"outbox-claimed","event_type":"HeartbeatObserved","agent_id":"s1","actor_id":"a1","workspace_id":"w1","sequence":1,"created_at":"2026-05-31T00:00:01Z","payload":{"n":1},"sync_status":"pending"}
"#,
    )
    .expect("claimed outbox file should write");
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

    fs::remove_dir_all(&temp_root).expect("temp root should be removable");
}

#[test]
fn sync_outbox_does_not_let_fake_active_claim_block_base_file() {
    let temp_root = temp_root("stateful-outbox-fake-active-claim-test");
    let paths = paths_for_temp_root(&temp_root);
    fs::create_dir_all(&paths.outbox_dir).expect("outbox dir should be creatable");

    let listener = TcpListener::bind("127.0.0.1:0").expect("listener should bind");
    let addr = listener.local_addr().expect("listener addr should load");
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("connection should arrive");
        let request = read_http_request(&mut stream);
        tx.send(request).expect("request should send to test");
        write_json_response(&mut stream, r#"{"status":"ok","sync_status":"synced"}"#);
    });

    let runtime = ServerRuntime::new(format!("http://{addr}"), "secret-token", "w1", 42);
    let outbox_file = paths.outbox_dir.join("s1.jsonl");
    let claimed_file = paths.outbox_dir.join("s1.jsonl.syncing-spoof");
    let active_marker = paths.outbox_dir.join("s1.jsonl.syncing-spoof.active");
    fs::write(
        &outbox_file,
        r#"{"outbox_id":"outbox-base","event_type":"HeartbeatObserved","agent_id":"s1","actor_id":"a1","workspace_id":"w1","sequence":1,"created_at":"2026-05-31T00:00:01Z","payload":{"n":1},"sync_status":"pending"}
"#,
    )
    .expect("base outbox file should write");
    fs::write(&claimed_file, "").expect("spoof claimed file should write");
    fs::write(&active_marker, "active\n").expect("spoof active marker should write");

    let synced = sync_outbox_with_runtime(&paths, &runtime).expect("outbox should sync");

    assert_eq!(synced, 1);
    assert!(!outbox_file.exists());
    let request = rx
        .recv_timeout(Duration::from_secs(2))
        .expect("request should arrive");
    assert!(request.contains("\"outbox_id\":\"outbox-base\""));

    fs::remove_dir_all(&temp_root).expect("temp root should be removable");
}

#[cfg(unix)]
#[test]
fn sync_outbox_does_not_trust_symlinked_lock_heartbeat() {
    let temp_root = temp_root("stateful-outbox-symlink-lock-heartbeat-test");
    let paths = paths_for_temp_root(&temp_root);
    fs::create_dir_all(paths.outbox_dir.join(".lock")).expect("lock dir should be creatable");
    let fresh_target = temp_root.join("fresh-heartbeat");
    fs::write(&fresh_target, "fresh\n").expect("fresh heartbeat target should write");
    std::os::unix::fs::symlink(&fresh_target, paths.outbox_dir.join(".lock/heartbeat"))
        .expect("heartbeat symlink should create");

    let listener = TcpListener::bind("127.0.0.1:0").expect("listener should bind");
    let addr = listener.local_addr().expect("listener addr should load");
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("connection should arrive");
        let request = read_http_request(&mut stream);
        tx.send(request).expect("request should send to test");
        write_json_response(&mut stream, r#"{"status":"ok","sync_status":"synced"}"#);
    });

    let runtime = ServerRuntime::new(format!("http://{addr}"), "secret-token", "w1", 42);
    let outbox_file = paths.outbox_dir.join("s1.jsonl");
    fs::write(
        &outbox_file,
        r#"{"outbox_id":"outbox-base","event_type":"HeartbeatObserved","agent_id":"s1","actor_id":"a1","workspace_id":"w1","sequence":1,"created_at":"2026-05-31T00:00:01Z","payload":{"n":1},"sync_status":"pending"}
"#,
    )
    .expect("outbox file should write");

    let synced = sync_outbox_with_runtime(&paths, &runtime).expect("outbox should sync");

    assert_eq!(synced, 1);
    assert!(!outbox_file.exists());
    let request = rx
        .recv_timeout(Duration::from_secs(2))
        .expect("request should arrive");
    assert!(request.contains("\"outbox_id\":\"outbox-base\""));

    fs::remove_dir_all(&temp_root).expect("temp root should be removable");
}

#[cfg(unix)]
#[test]
fn sync_outbox_does_not_trust_symlinked_lock_directory() {
    let temp_root = temp_root("stateful-outbox-symlink-lock-dir-test");
    let paths = paths_for_temp_root(&temp_root);
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
        let (mut stream, _) = listener.accept().expect("connection should arrive");
        let request = read_http_request(&mut stream);
        tx.send(request).expect("request should send to test");
        write_json_response(&mut stream, r#"{"status":"ok","sync_status":"synced"}"#);
    });

    let runtime = ServerRuntime::new(format!("http://{addr}"), "secret-token", "w1", 42);
    let outbox_file = paths.outbox_dir.join("s1.jsonl");
    fs::write(
        &outbox_file,
        r#"{"outbox_id":"outbox-base","event_type":"HeartbeatObserved","agent_id":"s1","actor_id":"a1","workspace_id":"w1","sequence":1,"created_at":"2026-05-31T00:00:01Z","payload":{"n":1},"sync_status":"pending"}
"#,
    )
    .expect("outbox file should write");

    let synced = sync_outbox_with_runtime(&paths, &runtime).expect("outbox should sync");

    assert_eq!(synced, 1);
    assert!(!outbox_file.exists());
    let request = rx
        .recv_timeout(Duration::from_secs(2))
        .expect("request should arrive");
    assert!(request.contains("\"outbox_id\":\"outbox-base\""));

    fs::remove_dir_all(&temp_root).expect("temp root should be removable");
}

#[test]
fn sync_outbox_deduplicates_claimed_records_already_merged_into_base() {
    let temp_root = temp_root("stateful-outbox-duplicate-merge-test");
    let paths = paths_for_temp_root(&temp_root);
    fs::create_dir_all(&paths.outbox_dir).expect("outbox dir should be creatable");

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
    let outbox_file = paths.outbox_dir.join("s1.jsonl");
    let claimed_file = paths.outbox_dir.join("s1.jsonl.syncing-old");
    let claimed_record = r#"{"outbox_id":"outbox-claimed","event_type":"HeartbeatObserved","agent_id":"s1","actor_id":"a1","workspace_id":"w1","sequence":1,"created_at":"2026-05-31T00:00:01Z","payload":{"n":1},"sync_status":"pending"}"#;
    fs::write(
        &outbox_file,
        format!(
            "{claimed_record}\n{{\"outbox_id\":\"outbox-base\",\"event_type\":\"HeartbeatObserved\",\"agent_id\":\"s1\",\"actor_id\":\"a1\",\"workspace_id\":\"w1\",\"sequence\":2,\"created_at\":\"2026-05-31T00:00:02Z\",\"payload\":{{\"n\":2}},\"sync_status\":\"pending\"}}\n"
        ),
    )
    .expect("base outbox file should write");
    fs::write(&claimed_file, format!("{claimed_record}\n"))
        .expect("claimed outbox file should write");

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

    fs::remove_dir_all(&temp_root).expect("temp root should be removable");
}

fn temp_root(label: &str) -> std::path::PathBuf {
    let temp_root = std::env::temp_dir().join(format!("{label}-{}", std::process::id()));
    if temp_root.exists() {
        fs::remove_dir_all(&temp_root).expect("old temp root should be removable");
    }
    temp_root
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
