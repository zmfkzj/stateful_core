use std::{
    fs::{self, File, FileTimes},
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    thread,
    time::{Duration, SystemTime},
};

use stateful_cli::{
    GlobalPaths, ServerRuntime, ensure_server_with, runtime_is_healthy, stop_server,
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

    let discovered = ensure_server_with(
        &paths,
        runtime_is_healthy,
        || {
            starts_for_closure.fetch_add(1, Ordering::SeqCst);
            Ok(ServerRuntime::new(
                "http://127.0.0.1:43873",
                "token",
                "w1",
                902,
            ))
        },
    )
    .expect("unidentified runtime should be replaced by started runtime");

    assert_eq!(discovered.pid, 902);
    assert_eq!(starts.load(Ordering::SeqCst), 1);
    let contents = fs::read_to_string(&paths.server_json).expect("runtime should be readable");
    assert!(contents.contains("\"pid\": 902"));
}

#[test]
fn runtime_health_requires_status_ok_and_authenticated_current() {
    let health_not_ok = FakeHttpServer::start(vec![fake_response(503, "not ready")]);
    let runtime = ServerRuntime::new(health_not_ok.base_url(), "token", "w1", 1);
    assert!(!runtime_is_healthy(&runtime));

    let current_not_ok = FakeHttpServer::start(vec![
        fake_response(200, "ok"),
        fake_response(401, r#"{"error":"unauthorized"}"#),
    ]);
    let runtime = ServerRuntime::new(current_not_ok.base_url(), "token", "w1", 1);
    assert!(!runtime_is_healthy(&runtime));
}

#[test]
fn stop_server_refuses_to_kill_unverified_pid() {
    let home = temp_home("stateful-server-stop-unverified");
    let paths = GlobalPaths::new(&home);
    let fake = FakeHttpServer::start(vec![
        fake_response(200, "ok"),
        fake_response(200, r#"{"status":"ok","current":{}}"#),
    ]);
    let runtime = ServerRuntime::new(fake.base_url(), "token", "w1", 1);
    stateful_cli::write_global_runtime_file(&paths, &runtime).expect("runtime should write");

    let error = stop_server(&paths).expect_err("unverified pid must not be killed");

    assert!(
        error.to_string().contains("refusing to stop"),
        "unexpected error: {error}"
    );
    assert!(paths.server_json.is_file());
}

fn temp_home(name: &str) -> std::path::PathBuf {
    let home = std::env::temp_dir().join(format!("{name}-{}", std::process::id()));
    if home.exists() {
        fs::remove_dir_all(&home).expect("old home should be removable");
    }
    home
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
}

impl FakeHttpServer {
    fn start(responses: Vec<String>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("fake server should bind");
        let addr = listener.local_addr().expect("fake server addr should be known");
        thread::spawn(move || {
            for response in responses {
                if let Ok((mut stream, _addr)) = listener.accept() {
                    read_request(&mut stream);
                    let _ = stream.write_all(response.as_bytes());
                }
            }
        });
        Self { addr }
    }

    fn base_url(&self) -> String {
        format!("http://{}", self.addr)
    }
}

fn read_request(stream: &mut TcpStream) {
    let mut buffer = [0; 1024];
    let _ = stream.read(&mut buffer);
}
