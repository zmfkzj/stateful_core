use std::{
    collections::VecDeque,
    fs,
    io::{Read, Write},
    net::TcpListener,
    process::Command as ProcessCommand,
    sync::mpsc,
    thread,
    time::Duration,
};

use stateful_cli::{
    Cli, Command, GlobalPaths, SandboxCommand, SandboxFsProfile, SandboxNetworkPolicy,
    ServerRuntime, write_global_runtime_file,
};

#[test]
fn external_run_subcommand_is_removed() {
    let error = Cli::try_parse_from([
        "stateful",
        "external-run",
        "request",
        "--purpose",
        "install rebuilt binaries",
        "--write-dir",
        "/opt/stateful/bin",
        "--command",
        "true",
    ])
    .expect_err("external-run should no longer parse");

    assert!(
        error.to_string().contains("unrecognized subcommand"),
        "unexpected parse error: {error}"
    );
}

#[test]
fn sandbox_external_profile_parses_external_scopes() {
    let cli = Cli::try_parse_from([
        "stateful",
        "sandbox",
        "run",
        "--fs",
        "external",
        "--purpose",
        "install rebuilt binaries",
        "--create-target",
        "/opt/stateful/bin/stateful",
        "--write-dir",
        "/opt/stateful/bin",
        "--connect-socket",
        "/private/tmp/tmux-501/default",
        "--allow-signal",
        "--network",
        "enabled",
        "--command",
        "install -m 755 target/release/stateful /opt/stateful/bin/stateful",
    ])
    .expect("sandbox external profile should parse");

    match cli.command {
        Command::Sandbox(SandboxCommand::Run {
            fs,
            network,
            purpose,
            create_targets,
            write_dirs,
            connect_sockets,
            allow_signal,
            ..
        }) => {
            assert_eq!(fs, SandboxFsProfile::External);
            assert_eq!(network, SandboxNetworkPolicy::Enabled);
            assert_eq!(purpose, Some("install rebuilt binaries".to_string()));
            assert_eq!(create_targets, vec!["/opt/stateful/bin/stateful"]);
            assert_eq!(write_dirs, vec!["/opt/stateful/bin"]);
            assert_eq!(connect_sockets, vec!["/private/tmp/tmux-501/default"]);
            assert!(allow_signal);
        }
        other => panic!("expected sandbox external run, got {other:?}"),
    }
}

#[test]
fn sandbox_external_repo_create_target_auto_declares_claims_and_runs() {
    let temp = tempfile::tempdir().expect("temp dir should create");
    let repo_root = temp.path().join("repo");
    fs::create_dir_all(&repo_root).expect("repo root should create");
    let paths = GlobalPaths::new(temp.path().join("home"));
    let (runtime, rx) = spawn_fake_stateful_server_sequence(vec![
        r#"{
            "decision": "deny",
            "message": "Supported writes require active file reservation.",
            "reason_code": "missing_reservation"
        }"#,
        r#"{"status":"ok","reservation_id":"auto-reservation"}"#,
        r#"{"status":"ok","claim_state":"acquired","paths":["fixtures/import.txt"],"acquired":1,"already_held":0}"#,
        r#"{"decision":"allow","message":"ok"}"#,
        r#"{"status":"ok"}"#,
    ]);
    write_global_runtime_file(&paths, &runtime).expect("runtime file should write");

    let output = ProcessCommand::new(env!("CARGO_BIN_EXE_stateful"))
        .args([
            "sandbox",
            "run",
            "--fs",
            "external",
            "--purpose",
            "import fixture",
            "--agent-id",
            "agent-a",
            "--workspace-id",
            "workspace-a",
            "--create-target",
            "fixtures/import.txt",
            "--command",
            "printf 'imported\n' > fixtures/import.txt",
        ])
        .current_dir(&repo_root)
        .env_clear()
        .env("STATEFUL_HOME", &paths.home)
        .env("STATEFUL_SERVER_URL", &runtime.base_url)
        .env("STATEFUL_SERVER_TOKEN", &runtime.token)
        .env("STATEFUL_SANDBOX_RUN_ACTIVE", "1")
        .env("STATEFUL_ALLOW_NESTED_SANDBOX_RUN", "1")
        .output()
        .expect("stateful sandbox run should spawn");

    assert!(
        output.status.success(),
        "sandbox run should succeed, stdout: {}, stderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        fs::read_to_string(repo_root.join("fixtures/import.txt"))
            .expect("created target should read"),
        "imported\n"
    );

    let first_authorize_request = rx
        .recv_timeout(Duration::from_secs(2))
        .expect("first authorize request should arrive");
    assert!(first_authorize_request.contains("POST /v1/authorize HTTP/1.1"));
    let first_authorize = request_json_body(&first_authorize_request);
    assert_eq!(first_authorize["payload"]["path"], "fixtures/import.txt");
    assert!(first_authorize["payload"].get("reservation_id").is_none());

    let declare_request = rx
        .recv_timeout(Duration::from_secs(2))
        .expect("reservation declare request should arrive");
    assert!(declare_request.contains("POST /v1/reservation/declare HTTP/1.1"));
    let declare = request_json_body(&declare_request);
    assert_eq!(declare["source"]["event"], "reservation_declare");
    assert_eq!(declare["payload"]["purpose"], "import fixture");
    assert_eq!(
        declare["payload"]["files_planned"],
        serde_json::json!(["fixtures/import.txt"])
    );

    let claim_request = rx
        .recv_timeout(Duration::from_secs(2))
        .expect("claim acquire request should arrive");
    assert!(claim_request.contains("POST /v1/claim/acquire HTTP/1.1"));
    let claim = request_json_body(&claim_request);
    assert_eq!(claim["reservation_id"], "auto-reservation");
    assert_eq!(claim["paths"], serde_json::json!(["fixtures/import.txt"]));

    let retry_authorize_request = rx
        .recv_timeout(Duration::from_secs(2))
        .expect("retry authorize request should arrive");
    let retry_authorize = request_json_body(&retry_authorize_request);
    assert_eq!(
        retry_authorize["payload"]["reservation_id"],
        "auto-reservation"
    );
    assert_eq!(retry_authorize["payload"]["path"], "fixtures/import.txt");
}

fn spawn_fake_stateful_server_sequence(
    actual_responses: Vec<&'static str>,
) -> (ServerRuntime, mpsc::Receiver<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("listener should bind");
    let addr = listener.local_addr().expect("listener addr should load");
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let mut actual_responses = VecDeque::from(actual_responses);
        while !actual_responses.is_empty() {
            let (mut stream, _) = listener.accept().expect("connection should arrive");
            let request = read_http_request_maybe_body(&mut stream);
            if request.contains("GET /health HTTP/1.1") {
                write_json_response(&mut stream, r#"{"status":"ok"}"#);
            } else if request.contains("GET /v1/runtime/identity HTTP/1.1") {
                write_json_response(
                    &mut stream,
                    r#"{"status":"ok","pid":42,"protocol_version":"stateful.v1","capabilities":["authorize.write_directory"]}"#,
                );
            } else {
                tx.send(request).expect("request should send to test");
                let response = actual_responses
                    .pop_front()
                    .expect("response should exist while loop is active");
                write_json_response(&mut stream, response);
            }
        }
    });

    (
        ServerRuntime::new(format!("http://{addr}"), "secret-token", "w1", 42),
        rx,
    )
}

fn read_http_request_maybe_body(stream: &mut std::net::TcpStream) -> String {
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
        .map(|value| value.parse::<usize>().expect("content length should parse"))
        .unwrap_or(0);

    let mut body = vec![0_u8; content_length];
    if content_length > 0 {
        stream
            .read_exact(&mut body)
            .expect("request body should read");
        buffer.extend_from_slice(&body);
    }

    String::from_utf8(buffer).expect("request should be utf8")
}

fn request_json_body(request: &str) -> serde_json::Value {
    let (_, body) = request
        .split_once("\r\n\r\n")
        .expect("request should contain a body separator");
    serde_json::from_str(body).expect("request body should be json")
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
