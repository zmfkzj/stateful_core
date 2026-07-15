use std::{
    fs::{self, File, FileTimes},
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    process::Command,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    thread,
    time::{Duration, SystemTime},
};

use stateful_cli::{
    GlobalPaths, ServerRuntime, ServerStartOptions, detached_server_args, ensure_server_with,
    ensure_server_with_options, restart_server, runtime_is_healthy,
    server_start_options_from_runtime, stop_server,
};

#[test]
fn ensure_server_returns_existing_runtime_when_health_is_ok() {
    let home = temp_home("stateful-server-existing");
    let paths = GlobalPaths::new(&home);
    let runtime = ServerRuntime::new("http://127.0.0.1:43873", "token", "w1", 123);
    stateful_cli::write_global_runtime_file(&paths, &runtime).expect("runtime should write");
    let starts = Arc::new(AtomicUsize::new(0));
    let starts_for_closure = starts.clone();

    let discovered = ensure_server_with(
        &paths,
        |_| true,
        || {
            starts_for_closure.fetch_add(1, Ordering::SeqCst);
            Ok(runtime.clone())
        },
    )
    .expect("server should ensure");

    assert_eq!(discovered.pid, 123);
    assert_eq!(starts.load(Ordering::SeqCst), 0);
}

#[test]
fn ensure_server_reuses_remote_pid_zero_runtime_when_health_is_ok() {
    let home = temp_home("stateful-server-remote-runtime");
    let paths = GlobalPaths::new(&home);
    let fake = FakeHttpServer::start(vec![fake_response(
        200,
        r#"{"protocol_version":"stateful.v2","journal_schema_version":2,"coordination_mode":"awareness","capabilities":["presence"]}"#,
    )]);
    let runtime = ServerRuntime::new(fake.base_url(), "secret-token", "shared", 0);
    stateful_cli::write_global_runtime_file(&paths, &runtime).expect("runtime should write");

    let discovered = ensure_server_with(&paths, runtime_is_healthy, || {
        panic!("remote runtime should be reused without starting a local server")
    })
    .expect("remote runtime should be accepted");

    assert_eq!(discovered.pid, 0);
    assert_eq!(discovered.workspace_id, "shared");
}

#[test]
fn ensure_server_with_options_preserves_unreachable_remote_pid_zero_runtime() {
    let home = temp_home("stateful-server-remote-runtime-unreachable");
    let paths = GlobalPaths::new(&home);
    let listener = TcpListener::bind("127.0.0.1:0").expect("test listener should bind");
    let remote_url = format!(
        "http://{}",
        listener
            .local_addr()
            .expect("test listener address should be known")
    );
    drop(listener);
    let runtime = ServerRuntime::new(&remote_url, "secret-token", "shared", 0);
    stateful_cli::write_global_runtime_file(&paths, &runtime).expect("runtime should write");

    let error = ensure_server_with_options(&paths, ServerStartOptions::default())
        .expect_err("unreachable remote runtime should not be replaced");

    assert!(
        error.to_string().contains("preserving runtime file"),
        "unexpected error: {error}"
    );
    let contents = fs::read_to_string(&paths.server_json).expect("runtime should remain readable");
    let preserved: ServerRuntime =
        serde_json::from_str(&contents).expect("runtime should remain valid JSON");
    assert_eq!(preserved.pid, 0);
    assert_eq!(preserved.base_url, remote_url);
}

#[test]
fn ensure_server_starts_when_runtime_is_missing() {
    let home = temp_home("stateful-server-missing");
    let paths = GlobalPaths::new(&home);
    let starts = Arc::new(AtomicUsize::new(0));
    let starts_for_closure = starts.clone();

    let discovered = ensure_server_with(
        &paths,
        |_| true,
        || {
            starts_for_closure.fetch_add(1, Ordering::SeqCst);
            Ok(ServerRuntime::new(
                "http://127.0.0.1:43873",
                "token",
                "w1",
                456,
            ))
        },
    )
    .expect("server should start");

    assert_eq!(discovered.pid, 456);
    assert_eq!(starts.load(Ordering::SeqCst), 1);
    assert!(paths.server_json.is_file());
}

#[test]
fn ensure_server_does_not_steal_existing_start_lock() {
    let home = temp_home("stateful-server-locked");
    let paths = GlobalPaths::new(&home);
    fs::create_dir_all(&paths.runtime_dir).expect("runtime dir should be creatable");
    fs::write(&paths.server_lock, "other-process").expect("lock should be writable");
    let starts = Arc::new(AtomicUsize::new(0));
    let starts_for_closure = starts.clone();

    let error = ensure_server_with(
        &paths,
        |_| true,
        || {
            starts_for_closure.fetch_add(1, Ordering::SeqCst);
            Ok(ServerRuntime::new(
                "http://127.0.0.1:43873",
                "token",
                "w1",
                789,
            ))
        },
    )
    .expect_err("server should not start while another start lock exists");

    assert!(
        error.to_string().contains("start lock"),
        "unexpected error: {error}"
    );
    assert_eq!(starts.load(Ordering::SeqCst), 0);
    assert!(paths.server_lock.is_file());
    assert_eq!(
        fs::read_to_string(&paths.server_lock).expect("lock should remain readable"),
        "other-process"
    );
}

#[test]
fn ensure_server_recovers_stale_start_lock() {
    let home = temp_home("stateful-server-stale-lock");
    let paths = GlobalPaths::new(&home);
    fs::create_dir_all(&paths.runtime_dir).expect("runtime dir should be creatable");
    fs::write(&paths.server_lock, "crashed-process").expect("lock should be writable");
    let stale_time = SystemTime::now()
        .checked_sub(Duration::from_secs(60))
        .expect("stale time should be representable");
    File::open(&paths.server_lock)
        .expect("lock should be openable")
        .set_times(FileTimes::new().set_modified(stale_time))
        .expect("lock timestamp should be adjustable");
    let starts = Arc::new(AtomicUsize::new(0));
    let starts_for_closure = starts.clone();

    let discovered = ensure_server_with(
        &paths,
        |_| true,
        || {
            starts_for_closure.fetch_add(1, Ordering::SeqCst);
            Ok(ServerRuntime::new(
                "http://127.0.0.1:43873",
                "token",
                "w1",
                890,
            ))
        },
    )
    .expect("stale lock should be recovered");

    assert_eq!(discovered.pid, 890);
    assert_eq!(starts.load(Ordering::SeqCst), 1);
    assert!(!paths.server_lock.exists());
}

#[test]
fn ensure_server_treats_malformed_runtime_as_stale() {
    let home = temp_home("stateful-server-malformed");
    let paths = GlobalPaths::new(&home);
    fs::create_dir_all(&paths.runtime_dir).expect("runtime dir should be creatable");
    fs::write(&paths.server_json, "{not-json").expect("malformed runtime should be writable");
    let starts = Arc::new(AtomicUsize::new(0));
    let starts_for_closure = starts.clone();

    let discovered = ensure_server_with(
        &paths,
        |_| true,
        || {
            starts_for_closure.fetch_add(1, Ordering::SeqCst);
            Ok(ServerRuntime::new(
                "http://127.0.0.1:43873",
                "token",
                "w1",
                901,
            ))
        },
    )
    .expect("malformed runtime should be replaced by started runtime");

    assert_eq!(discovered.pid, 901);
    assert_eq!(starts.load(Ordering::SeqCst), 1);
}

#[test]
fn ensure_server_rejects_unidentified_http_service_and_overwrites_runtime() {
    let home = temp_home("stateful-server-unidentified");
    let paths = GlobalPaths::new(&home);
    let fake = FakeHttpServer::start(vec![
        fake_response(200, "not stateful"),
        fake_response(404, "missing"),
    ]);
    let stale_runtime = ServerRuntime::new(fake.base_url(), "token", "old", 111);
    stateful_cli::write_global_runtime_file(&paths, &stale_runtime).expect("runtime should write");
    let starts = Arc::new(AtomicUsize::new(0));
    let starts_for_closure = starts.clone();

    let discovered = ensure_server_with(&paths, runtime_is_healthy, || {
        starts_for_closure.fetch_add(1, Ordering::SeqCst);
        Ok(ServerRuntime::new(
            "http://127.0.0.1:43873",
            "token",
            "w1",
            902,
        ))
    })
    .expect("unidentified runtime should be replaced by started runtime");

    assert_eq!(discovered.pid, 902);
    assert_eq!(starts.load(Ordering::SeqCst), 1);
    let contents = fs::read_to_string(&paths.server_json).expect("runtime should be readable");
    assert!(contents.contains("\"pid\": 902"));
}

#[test]
fn runtime_health_requires_v2_identity_and_capabilities() {
    let health_not_ok = FakeHttpServer::start(vec![fake_response(503, "not ready")]);
    let runtime = ServerRuntime::new(health_not_ok.base_url(), "token", "w1", 1);
    assert!(!runtime_is_healthy(&runtime));

    let identity_not_ok = FakeHttpServer::start(vec![fake_response(401, r#"{"error":"unauthorized"}"#)]);
    let runtime = ServerRuntime::new(identity_not_ok.base_url(), "token", "w1", 1);
    assert!(!runtime_is_healthy(&runtime));

    let missing_capabilities = FakeHttpServer::start(vec![fake_response(
        200,
        r#"{"protocol_version":"stateful.v2","journal_schema_version":2,"coordination_mode":"awareness","capabilities":[]}"#,
    )]);
    let runtime = ServerRuntime::new(missing_capabilities.base_url(), "token", "w1", 1);
    assert!(!runtime_is_healthy(&runtime));

    let healthy = FakeHttpServer::start(vec![fake_response(
        200,
        r#"{"protocol_version":"stateful.v2","journal_schema_version":2,"coordination_mode":"awareness","capabilities":["presence"]}"#,
    )]);
    let runtime = ServerRuntime::new(healthy.base_url(), "token", "w1", 1);
    assert!(runtime_is_healthy(&runtime));
}

#[test]
fn stop_server_refuses_to_kill_unverified_pid() {
    let home = temp_home("stateful-server-stop-unverified");
    let paths = GlobalPaths::new(&home);
    let fake = FakeHttpServer::start(vec![fake_response(
        200,
        r#"{"protocol_version":"stateful.v2","journal_schema_version":2,"coordination_mode":"awareness","capabilities":[]}"#,
    )]);
    let runtime = ServerRuntime::new(fake.base_url(), "token", "w1", 1);
    stateful_cli::write_global_runtime_file(&paths, &runtime).expect("runtime should write");

    let error = stop_server(&paths).expect_err("unverified pid must not be killed");

    assert!(
        error.to_string().contains("refusing to stop"),
        "unexpected error: {error}"
    );
    assert!(paths.server_json.is_file());
}

#[test]
fn restart_refuses_remote_pid_zero_runtime_that_cannot_be_killed() {
    let home = temp_home("stateful-server-restart-remote-pid-zero");
    let paths = GlobalPaths::new(&home);
    let fake = FakeHttpServer::start(vec![fake_response(
        200,
        r#"{"protocol_version":"stateful.v2","journal_schema_version":2,"coordination_mode":"awareness","capabilities":["presence"]}"#,
    )]);
    let runtime = ServerRuntime::new(fake.base_url(), "token", "w1", 0);
    stateful_cli::write_global_runtime_file(&paths, &runtime).expect("runtime should write");

    let error = restart_server(&paths).expect_err("remote runtime cannot be restarted locally");

    assert!(
        error
            .to_string()
            .contains("remote stateful server cannot be killed"),
        "unexpected error: {error}"
    );
    assert!(paths.server_json.is_file());
}

#[test]
fn restart_refuses_joined_pid_zero_runtime_without_identity_probe() {
    let home = temp_home("stateful-server-restart-joined-pid-zero-without-probe");
    let paths = GlobalPaths::new(&home);
    let fake = FakeHttpServer::start(vec![fake_response(
        200,
        r#"{"protocol_version":"stateful.v2","journal_schema_version":2,"coordination_mode":"awareness","capabilities":["presence"]}"#,
    )]);
    let runtime = ServerRuntime::new(fake.base_url(), "token", "w1", 0);
    stateful_cli::write_global_runtime_file(&paths, &runtime).expect("runtime should write");

    let error = restart_server(&paths).expect_err("joined runtime restart should fail locally");

    assert!(
        error
            .to_string()
            .contains("remote stateful server cannot be killed"),
        "unexpected error: {error}"
    );
    assert_eq!(fake.request_count(), 0);
    let contents = fs::read_to_string(&paths.server_json).expect("runtime should remain readable");
    let preserved: ServerRuntime =
        serde_json::from_str(&contents).expect("runtime should remain valid JSON");
    assert_eq!(preserved.pid, 0);
    assert_eq!(preserved.base_url, runtime.base_url);
}


#[test]
fn ensure_server_with_options_rejects_healthy_runtime_on_different_port() {
    let home = temp_home("stateful-server-option-mismatch");
    let paths = GlobalPaths::new(&home);
    let fake = FakeHttpServer::start(vec![fake_response(
        200,
        r#"{"protocol_version":"stateful.v2","journal_schema_version":2,"coordination_mode":"awareness","capabilities":["presence"]}"#,
    )]);
    let runtime = ServerRuntime::new(fake.base_url(), "token", "w1", 123);
    stateful_cli::write_global_runtime_file(&paths, &runtime).expect("runtime should write");

    let error = ensure_server_with_options(
        &paths,
        ServerStartOptions {
            host: "127.0.0.1".to_string(),
            port: 1,
            token: None,
            workspace_id: "w1".to_string(),
            coordination_mode: "enforcement".to_string(),
        },
    )
    .expect_err("healthy runtime with different port should be rejected");

    assert!(
        error
            .to_string()
            .contains("does not match requested server options"),
        "unexpected error: {error}"
    );
    assert!(
        error.to_string().contains("stateful server stop"),
        "unexpected error: {error}"
    );
}

#[test]
fn server_start_options_from_runtime_preserves_previous_start_options() {
    let runtime = ServerRuntime::new("http://0.0.0.0:43874", "secret-token", "shared", 123);

    let options =
        server_start_options_from_runtime(&runtime).expect("runtime options should parse");

    assert_eq!(
        options,
        ServerStartOptions {
            host: "0.0.0.0".to_string(),
            port: 43874,
            token: Some("secret-token".to_string()),
            workspace_id: "shared".to_string(),
            coordination_mode: "awareness".to_string(),
        }
    );
}

#[test]
fn server_start_options_from_runtime_preserves_bracketed_ipv6_host() {
    let runtime = ServerRuntime::new("http://[::1]:43875", "secret-token", "w1", 123);

    let options =
        server_start_options_from_runtime(&runtime).expect("runtime options should parse");

    assert_eq!(options.host, "[::1]");
    assert_eq!(options.port, 43875);
    assert_eq!(options.token.as_deref(), Some("secret-token"));
    assert_eq!(options.workspace_id, "w1");
}

#[test]
fn detached_server_args_include_start_options() {
    let args = detached_server_args(&ServerStartOptions {
        host: "127.0.0.2".to_string(),
        port: 43874,
        token: Some("secret-token".to_string()),
        workspace_id: "w2".to_string(),
        coordination_mode: "awareness".to_string(),
    });

    assert_eq!(
        args,
        vec![
            "server",
            "start",
            "--foreground",
            "--host",
            "127.0.0.2",
            "--port",
            "43874",
            "--token",
            "secret-token",
            "--workspace-id",
            "w2",
            "--coordination-mode",
            "awareness"
        ]
    );
}

#[test]
fn server_start_and_install_default_to_awareness() {
    assert_eq!(
        ServerStartOptions::default().coordination_mode,
        "awareness",
        "new server installations must default to awareness"
    );
    let runtime = ServerRuntime::new("http://127.0.0.1:43873", "token", "w1", 0);
    assert_eq!(
        server_start_options_from_runtime(&runtime)
            .expect("runtime options should parse")
            .coordination_mode,
        "awareness"
    );
}

#[test]
fn explicit_enforcement_flag_is_preserved() {
    let args = detached_server_args(&ServerStartOptions {
        coordination_mode: "enforcement".to_string(),
        ..ServerStartOptions::default()
    });
    assert_eq!(
        args.windows(2)
            .find(|pair| pair[0] == "--coordination-mode")
            .expect("coordination mode argument"),
        ["--coordination-mode", "enforcement"]
    );
}

#[test]
fn foreground_server_does_not_write_runtime_when_bind_fails() {
    let home = temp_home("stateful-server-foreground-bind-fail");
    let paths = GlobalPaths::new(&home);
    let listener = TcpListener::bind("127.0.0.1:0").expect("test listener should reserve a port");
    let port = listener
        .local_addr()
        .expect("listener should expose local address")
        .port();

    let output = Command::new(env!("CARGO_BIN_EXE_stateful"))
        .args([
            "server",
            "start",
            "--foreground",
            "--port",
            &port.to_string(),
        ])
        .env("STATEFUL_HOME", home.path())
        .output()
        .expect("stateful binary should run");

    assert!(
        !output.status.success(),
        "server should fail to bind occupied port"
    );
    assert!(
        !paths.server_json.exists(),
        "failed foreground start must not publish a runtime file"
    );
}

#[test]
fn detached_server_reports_child_startup_error_when_bind_fails() {
    let home = temp_home("stateful-server-detached-bind-fail");
    let listener = TcpListener::bind("127.0.0.1:0").expect("test listener should reserve a port");
    let port = listener
        .local_addr()
        .expect("listener should expose local address")
        .port();

    let output = Command::new(env!("CARGO_BIN_EXE_stateful"))
        .args(["server", "start", "--port", &port.to_string()])
        .env("STATEFUL_HOME", home.path())
        .output()
        .expect("stateful binary should run");

    assert!(
        !output.status.success(),
        "server should fail to bind occupied port"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("Address already in use"),
        "startup error should include child bind failure, got: {stderr}"
    );
}

#[test]
fn detached_server_start_registers_runtime_without_parent_lock_timeout() {
    if cfg!(target_os = "macos") && std::env::var_os("STATEFUL_SANDBOX_RUN_ACTIVE").is_some() {
        return;
    }

    let mut last_bind_race = String::new();
    for attempt in 0..8 {
        let home = temp_home(&format!(
            "stateful-server-detached-start-lock-handoff-{attempt}"
        ));
        let listener =
            TcpListener::bind("127.0.0.1:0").expect("test listener should reserve a port");
        let port = listener
            .local_addr()
            .expect("listener should expose local address")
            .port();
        drop(listener);

        let output = Command::new(env!("CARGO_BIN_EXE_stateful"))
            .args([
                "server",
                "start",
                "--host",
                "127.0.0.1",
                "--port",
                &port.to_string(),
                "--workspace-id",
                "share",
            ])
            .env("STATEFUL_HOME", home.path())
            .output()
            .expect("stateful binary should run");

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            if stderr.contains("Address already in use") {
                last_bind_race = stderr.into_owned();
                continue;
            }
            panic!("detached start should succeed, stderr: {stderr}");
        }

        let contents = fs::read_to_string(home.path().join("runtime/server.json"))
            .expect("runtime file should exist");
        let runtime: ServerRuntime =
            serde_json::from_str(&contents).expect("runtime file should be valid JSON");
        assert_eq!(runtime.base_url, format!("http://127.0.0.1:{port}"));
        assert_eq!(runtime.workspace_id, "share");

        let stop_output = Command::new(env!("CARGO_BIN_EXE_stateful"))
            .args(["server", "stop"])
            .env("STATEFUL_HOME", home.path())
            .output()
            .expect("stateful stop should run");
        assert!(
            stop_output.status.success(),
            "server stop should succeed, stderr: {}",
            String::from_utf8_lossy(&stop_output.stderr)
        );
        return;
    }

    panic!("detached start exhausted port-race retries; last stderr: {last_bind_race}");
}

fn temp_home(name: &str) -> tempfile::TempDir {
    tempfile::Builder::new()
        .prefix(&format!("{name}-"))
        .tempdir()
        .expect("temp dir should create")
}

fn fake_response(status: u16, body: &'static str) -> String {
    let reason = match status {
        200 => "OK",
        401 => "Unauthorized",
        404 => "Not Found",
        503 => "Service Unavailable",
        _ => "OK",
    };
    format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
}

struct FakeHttpServer {
    addr: std::net::SocketAddr,
    requests: Arc<AtomicUsize>,
}

impl FakeHttpServer {
    fn start(responses: Vec<String>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("fake server should bind");
        let addr = listener
            .local_addr()
            .expect("fake server addr should be known");
        let requests = Arc::new(AtomicUsize::new(0));
        let requests_for_thread = requests.clone();
        thread::spawn(move || {
            for response in responses {
                if let Ok((mut stream, _addr)) = listener.accept() {
                    requests_for_thread.fetch_add(1, Ordering::SeqCst);
                    read_request(&mut stream);
                    let _ = stream.write_all(response.as_bytes());
                }
            }
        });
        Self { addr, requests }
    }

    fn base_url(&self) -> String {
        format!("http://{}", self.addr)
    }

    fn request_count(&self) -> usize {
        self.requests.load(Ordering::SeqCst)
    }
}

fn read_request(stream: &mut TcpStream) {
    let mut buffer = [0; 1024];
    let _ = stream.read(&mut buffer);
}
