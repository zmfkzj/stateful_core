use std::{
    fs,
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    process::{Command, Stdio},
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant},
};

use stateful_cli::{
    GlobalPaths, ServerRuntime, ServerStartOptions, detached_server_args, ensure_server_with,
    ensure_server_with_options, process_start_identity_for_pid, runtime_is_healthy,
    server_start_options_from_runtime, write_global_runtime_file,
};

#[test]
fn runtime_health_requires_health_and_v2_status() {
    let (server, requests) = FakeHttpServer::start(healthy_responses());
    let runtime = local_runtime(server.base_url());

    assert!(runtime_is_healthy(&runtime));
    assert_eq!(
        requests.recv().expect("health request should arrive"),
        "GET /health HTTP/1.1"
    );
    assert_eq!(
        requests.recv().expect("status request should arrive"),
        "GET /v2/status HTTP/1.1"
    );
}

#[test]
fn ensure_server_reuses_healthy_local_v2_runtime() {
    let temp = tempfile::tempdir().expect("temp dir should create");
    let paths = GlobalPaths::new(temp.path());
    let (server, _requests) = FakeHttpServer::start(healthy_responses());
    let runtime = local_runtime(server.base_url());
    write_global_runtime_file(&paths, &runtime).expect("runtime should write");
    let starts = Arc::new(AtomicUsize::new(0));
    let starts_for_closure = starts.clone();

    let discovered = ensure_server_with(&paths, runtime_is_healthy, move || {
        starts_for_closure.fetch_add(1, Ordering::SeqCst);
        Ok(local_runtime("http://127.0.0.1:43873"))
    })
    .expect("healthy V2 runtime should be reused");

    assert_eq!(discovered, runtime);
    assert_eq!(starts.load(Ordering::SeqCst), 0);
}

#[test]
fn stale_runtime_is_replaced_without_reuse() {
    let temp = tempfile::tempdir().expect("temp dir should create");
    let paths = GlobalPaths::new(temp.path());
    let mut stale = local_runtime("http://127.0.0.1:43873");
    stale.process_start_identity = "mismatched-process-start-identity".to_string();
    write_global_runtime_file(&paths, &stale).expect("stale runtime should write");
    let starts = Arc::new(AtomicUsize::new(0));
    let starts_for_closure = starts.clone();
    let replacement = local_runtime("http://127.0.0.1:43874");

    let discovered = ensure_server_with(
        &paths,
        |_| true,
        move || {
            starts_for_closure.fetch_add(1, Ordering::SeqCst);
            Ok(replacement.clone())
        },
    )
    .expect("stale runtime should start a replacement");

    assert_eq!(starts.load(Ordering::SeqCst), 1);
    assert_eq!(discovered.base_url, "http://127.0.0.1:43874");
}

#[test]
fn restart_arguments_use_token_stdin_without_secret_or_debug_leakage() {
    let pid = std::process::id();
    let runtime = ServerRuntime::new(
        "http://127.0.0.1:43873",
        "restart-secret",
        "workspace-1",
        pid,
        process_start_identity_for_pid(pid).expect("current process should have an identity"),
    );
    let options =
        server_start_options_from_runtime(&runtime).expect("runtime should yield restart options");
    let args = detached_server_args(&options);

    assert!(args.iter().any(|arg| arg == "--token-stdin"));
    assert!(!args.iter().any(|arg| arg == "--token"));
    assert!(!args.iter().any(|arg| arg == "restart-secret"));
    assert!(!format!("{runtime:?}").contains("restart-secret"));
    assert!(!format!("{options:?}").contains("restart-secret"));
}

#[test]
fn foreground_token_stdin_and_restart_preserve_exact_secret_without_argv_exposure() {
    let home = tempfile::tempdir().expect("temporary stateful home should create");
    let paths = GlobalPaths::new(home.path());
    let port = unused_loopback_port();
    let port_arg = port.to_string();
    let token = "token-from-private-stdin";
    let mut child = Command::new(env!("CARGO_BIN_EXE_stateful"))
        .args([
            "server",
            "start",
            "--foreground",
            "--token-stdin",
            "--host",
            "127.0.0.1",
            "--port",
            &port_arg,
            "--workspace-id",
            "pipe-test",
        ])
        .env("STATEFUL_HOME", &paths.home)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("foreground server should start");
    let mut stdin = child.stdin.take().expect("child stdin should be piped");
    stdin
        .write_all(token.as_bytes())
        .expect("token should write to child");
    drop(stdin);

    let runtime = wait_for_runtime_file(&paths, &mut child);
    assert_eq!(runtime.token, token);
    let pid = child.id().to_string();
    let command = String::from_utf8(
        Command::new("ps")
            .args(["-p", &pid, "-o", "command="])
            .output()
            .expect("child command should be inspectable")
            .stdout,
    )
    .expect("child command should be UTF-8");
    assert!(!command.contains(token));
    assert!(command.contains("--token-stdin"));
    let restart = Command::new(env!("CARGO_BIN_EXE_stateful"))
        .args(["server", "restart"])
        .env("STATEFUL_HOME", &paths.home)
        .output()
        .expect("server restart should run");
    assert!(
        restart.status.success(),
        "server restart should succeed: {}",
        String::from_utf8_lossy(&restart.stderr)
    );
    child
        .wait()
        .expect("restarted foreground server should reap");

    let restarted = fs::read_to_string(&paths.server_json)
        .ok()
        .and_then(|contents| serde_json::from_str::<ServerRuntime>(&contents).ok())
        .expect("restarted server should write a runtime file");
    assert_eq!(restarted.token, token);
    let restarted_pid = restarted.pid.to_string();
    let restarted_command = String::from_utf8(
        Command::new("ps")
            .args(["-p", &restarted_pid, "-o", "command="])
            .output()
            .expect("restarted command should be inspectable")
            .stdout,
    )
    .expect("restarted command should be UTF-8");
    assert!(!restarted_command.contains(token));
    assert!(restarted_command.contains("--token-stdin"));

    let stop = Command::new(env!("CARGO_BIN_EXE_stateful"))
        .args(["server", "stop"])
        .env("STATEFUL_HOME", &paths.home)
        .output()
        .expect("server stop should run");
    assert!(
        stop.status.success(),
        "server stop should succeed: {}",
        String::from_utf8_lossy(&stop.stderr)
    );
}

#[test]
fn foreground_token_stdin_rejects_empty_input() {
    let home = tempfile::tempdir().expect("temporary stateful home should create");
    let port = unused_loopback_port();
    let port_arg = port.to_string();
    let mut child = Command::new(env!("CARGO_BIN_EXE_stateful"))
        .args([
            "server",
            "start",
            "--foreground",
            "--token-stdin",
            "--host",
            "127.0.0.1",
            "--port",
            &port_arg,
        ])
        .env("STATEFUL_HOME", home.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("foreground server should start");
    drop(child.stdin.take());

    let started = Instant::now();
    loop {
        if let Some(status) = child.try_wait().expect("child status should read") {
            assert!(!status.success(), "empty token stdin must fail closed");
            break;
        }
        if started.elapsed() > Duration::from_secs(1) {
            let _ = child.kill();
            panic!("empty token stdin must not start a server");
        }
        thread::sleep(Duration::from_millis(10));
    }
}

fn unused_loopback_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .expect("loopback listener should bind")
        .local_addr()
        .expect("loopback address should load")
        .port()
}

fn wait_for_runtime_file(paths: &GlobalPaths, child: &mut std::process::Child) -> ServerRuntime {
    let started = Instant::now();
    loop {
        if let Ok(contents) = fs::read_to_string(&paths.server_json) {
            return serde_json::from_str(&contents).expect("runtime file should be valid");
        }
        if let Some(status) = child.try_wait().expect("child status should read") {
            panic!("foreground server exited before writing runtime: {status}");
        }
        if started.elapsed() > Duration::from_secs(2) {
            let _ = child.kill();
            panic!("foreground server did not write its runtime file");
        }
        thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn lifecycle_rejects_non_loopback_start_options_before_starting() {
    let temp = tempfile::tempdir().expect("temp dir should create");
    let paths = GlobalPaths::new(temp.path());
    let error = ensure_server_with_options(
        &paths,
        ServerStartOptions {
            host: "192.0.2.1".to_string(),
            ..ServerStartOptions::default()
        },
    )
    .expect_err("non-loopback start should be rejected");

    assert!(error.to_string().contains("loopback"));
    assert!(!paths.server_json.exists());
}

#[test]
fn runtime_start_options_preserve_local_binding() {
    let runtime = local_runtime("http://127.0.0.1:43873");

    let options = server_start_options_from_runtime(&runtime)
        .expect("local runtime should yield start options");

    assert_eq!(options.host, "127.0.0.1");
    assert_eq!(options.port, 43873);
    assert_eq!(options.token.as_deref(), Some("token"));
    assert_eq!(options.workspace_id, "workspace-1");
}

#[test]
fn runtime_start_options_reject_non_loopback_runtime() {
    let runtime = local_runtime("http://192.0.2.1:43873");

    let error = server_start_options_from_runtime(&runtime)
        .expect_err("non-loopback runtime should not yield start options");

    assert!(error.to_string().contains("loopback"));
}

fn local_runtime(base_url: impl Into<String>) -> ServerRuntime {
    let pid = std::process::id();
    ServerRuntime::new(
        base_url,
        "token",
        "workspace-1",
        pid,
        process_start_identity_for_pid(pid).expect("current process should have an identity"),
    )
}

fn healthy_responses() -> Vec<FakeResponse> {
    vec![
        FakeResponse::new(200, "ok"),
        FakeResponse::new(
            200,
            r#"{"protocol_version":"stateful.v2","contract_revision":"lease-1","request_id":null,"payload":{"active_tasks":0,"draining_tasks":0,"active_leases":0,"draining_leases":0,"queued_requests":0,"offered_requests":0,"executing_writes":0,"uncertain_writes":0}}"#,
        ),
    ]
}

struct FakeResponse {
    status: u16,
    body: String,
}

impl FakeResponse {
    fn new(status: u16, body: impl Into<String>) -> Self {
        Self {
            status,
            body: body.into(),
        }
    }
}

struct FakeHttpServer {
    addr: std::net::SocketAddr,
}

impl FakeHttpServer {
    fn start(responses: Vec<FakeResponse>) -> (Self, mpsc::Receiver<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("fake server should bind");
        let addr = listener
            .local_addr()
            .expect("fake server address should be known");
        let (tx, rx) = mpsc::channel();
        thread::spawn(move || {
            for response in responses {
                let (mut stream, _) = listener.accept().expect("connection should arrive");
                tx.send(request_line(&mut stream))
                    .expect("request line should send");
                write_response(&mut stream, response.status, &response.body);
            }
        });
        (Self { addr }, rx)
    }

    fn base_url(&self) -> String {
        format!("http://{}", self.addr)
    }
}

fn request_line(stream: &mut TcpStream) -> String {
    let mut request = Vec::new();
    let mut byte = [0_u8; 1];
    while !request.ends_with(b"\r\n\r\n") {
        stream
            .read_exact(&mut byte)
            .expect("request headers should arrive");
        request.push(byte[0]);
    }
    String::from_utf8(request)
        .expect("request should be UTF-8")
        .lines()
        .next()
        .expect("request should have a request line")
        .to_string()
}

fn write_response(stream: &mut TcpStream, status: u16, body: &str) {
    let reason = match status {
        200 => "OK",
        _ => "Unexpected",
    };
    write!(
        stream,
        "HTTP/1.1 {status} {reason}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
    .expect("response should write");
}
