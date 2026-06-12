use std::{
    fs,
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    path::{Path, PathBuf},
    process::Command,
    thread,
};

use stateful_cli::{
    GlobalPaths, RepoRegistry, ServerJoinOptions, ServerRuntime, ServerStartRuntimeOptions,
    join_server_runtime, server_join_commands, start_server_runtime, write_global_runtime_file,
};

#[test]
fn server_join_writes_global_runtime_without_enabling_repo_by_default() {
    let fixture = TestFixture::new("join-global-only");
    let host = FakeHttpServer::start(vec![identity_response(200)]);

    let result = join_server_runtime(ServerJoinOptions {
        paths: fixture.paths.clone(),
        codex_config_path: fixture.codex_config.clone(),
        binary_path: "/opt/stateful/bin/stateful".to_string(),
        base_url: host.base_url(),
        token: "secret-token".to_string(),
        workspace_id: "shared".to_string(),
        enable_repo_root: None,
    })
    .expect("server join should succeed");

    assert_eq!(result.status, "ok");
    assert!(!result.repo_enabled);
    assert_eq!(result.runtime.base_url, host.base_url());
    assert_eq!(result.runtime.token, "secret-token");
    assert_eq!(result.runtime.pid, 0);
    assert_eq!(result.runtime.workspace_id, "shared");
    assert!(fixture.paths.server_json.is_file());
    assert!(fixture.codex_config.is_file());
    assert!(!fixture.repo.join(".stateful_core").exists());
    assert_eq!(
        RepoRegistry::load(&fixture.paths).expect("registry should load"),
        RepoRegistry::default()
    );
}

#[test]
fn server_join_to_localhost_preserves_joined_runtime_pid_zero() {
    let fixture = TestFixture::new("join-localhost-pid");
    let host = FakeHttpServer::start(vec![identity_response_with_pid(200, 4321)]);

    let result = join_server_runtime(ServerJoinOptions {
        paths: fixture.paths.clone(),
        codex_config_path: fixture.codex_config.clone(),
        binary_path: "/opt/stateful/bin/stateful".to_string(),
        base_url: host.base_url(),
        token: "secret-token".to_string(),
        workspace_id: "shared".to_string(),
        enable_repo_root: None,
    })
    .expect("server join should succeed");

    assert_eq!(result.runtime.pid, 0);
    let contents =
        fs::read_to_string(&fixture.paths.server_json).expect("runtime should be written");
    assert!(contents.contains("\"pid\": 0"));
}

#[test]
fn server_join_enable_repo_only_when_requested() {
    let fixture = TestFixture::new("join-enable-repo");
    let host = FakeHttpServer::start(vec![identity_response(200)]);
    init_git_repo(&fixture.repo);

    let result = join_server_runtime(ServerJoinOptions {
        paths: fixture.paths.clone(),
        codex_config_path: fixture.codex_config.clone(),
        binary_path: "/opt/stateful/bin/stateful".to_string(),
        base_url: host.base_url(),
        token: "secret-token".to_string(),
        workspace_id: "shared".to_string(),
        enable_repo_root: Some(fixture.repo.clone()),
    })
    .expect("server join should enable repo");

    assert!(result.repo_enabled);
    let registry = RepoRegistry::load(&fixture.paths).expect("registry should load");
    assert!(registry.enabled_entry(&fixture.repo).is_some());
}

#[test]
fn server_join_invalid_token_fails_before_writing() {
    let fixture = TestFixture::new("join-invalid-token");
    let host = FakeHttpServer::start(vec![identity_response(401)]);

    let error = join_server_runtime(ServerJoinOptions {
        paths: fixture.paths.clone(),
        codex_config_path: fixture.codex_config.clone(),
        binary_path: "/opt/stateful/bin/stateful".to_string(),
        base_url: host.base_url(),
        token: "bad-token".to_string(),
        workspace_id: "shared".to_string(),
        enable_repo_root: None,
    })
    .expect_err("invalid token should fail");

    assert!(
        error
            .to_string()
            .contains("valid stateful runtime identity")
    );
    assert!(!fixture.paths.server_json.exists());
    assert!(!fixture.codex_config.exists());
}

#[test]
fn server_join_missing_capability_fails_before_writing() {
    let fixture = TestFixture::new("join-missing-capability");
    let host = FakeHttpServer::start(vec![identity_response_without_write_dir_capability()]);

    let error = join_server_runtime(ServerJoinOptions {
        paths: fixture.paths.clone(),
        codex_config_path: fixture.codex_config.clone(),
        binary_path: "/opt/stateful/bin/stateful".to_string(),
        base_url: host.base_url(),
        token: "secret-token".to_string(),
        workspace_id: "shared".to_string(),
        enable_repo_root: None,
    })
    .expect_err("missing capability should fail");

    assert!(error.to_string().contains("authorize.write_directory"));
    assert!(!fixture.paths.server_json.exists());
    assert!(!fixture.codex_config.exists());
}

#[test]
fn server_join_writes_requested_workspace_id() {
    let fixture = TestFixture::new("join-workspace");
    let host = FakeHttpServer::start(vec![identity_response(200)]);

    let result = join_server_runtime(ServerJoinOptions {
        paths: fixture.paths.clone(),
        codex_config_path: fixture.codex_config.clone(),
        binary_path: "/opt/stateful/bin/stateful".to_string(),
        base_url: host.base_url(),
        token: "secret-token".to_string(),
        workspace_id: "w1".to_string(),
        enable_repo_root: None,
    })
    .expect("server join should succeed");

    assert_eq!(result.runtime.workspace_id, "w1");
}

#[test]
fn server_join_rejects_non_loopback_plain_http_before_writing() {
    let fixture = TestFixture::new("join-non-loopback-http");

    let error = join_server_runtime(ServerJoinOptions {
        paths: fixture.paths.clone(),
        codex_config_path: fixture.codex_config.clone(),
        binary_path: "/opt/stateful/bin/stateful".to_string(),
        base_url: "http://0.0.0.0:1".to_string(),
        token: "secret-token".to_string(),
        workspace_id: "shared".to_string(),
        enable_repo_root: None,
    })
    .expect_err("non-loopback plain http joins should fail");

    assert!(
        error
            .to_string()
            .contains("plain http joins are only allowed for loopback addresses"),
        "unexpected error: {error}"
    );
    assert!(!fixture.paths.server_json.exists());
    assert!(!fixture.codex_config.exists());
}

#[test]
fn server_join_commands_render_loopback_tunnel_command_for_non_loopback_addresses() {
    let commands = server_join_commands(
        &[
            "192.168.0.23".parse().expect("ip should parse"),
            "10.0.0.7".parse().expect("ip should parse"),
        ],
        43873,
        "secret-token",
    );

    assert_eq!(
        commands,
        vec!["stateful server join http://127.0.0.1:43873 --token secret-token"]
    );
}

#[test]
fn server_join_commands_bracket_loopback_ipv6_addresses() {
    let commands = server_join_commands(
        &["::1".parse().expect("ip should parse")],
        43873,
        "secret-token",
    );

    assert_eq!(
        commands,
        vec!["stateful server join http://[::1]:43873 --token secret-token"]
    );
}

#[test]
fn server_start_without_token_reuses_existing_runtime_token() {
    let fixture = TestFixture::new("serve-reuse-token");
    let host = FakeRuntimeServer::start();
    let runtime = ServerRuntime::new(host.base_url(), "existing-token", "shared", 0);
    write_global_runtime_file(&fixture.paths, &runtime).expect("runtime should write");

    let result = start_server_runtime(ServerStartRuntimeOptions {
        paths: fixture.paths.clone(),
        host: "127.0.0.1".to_string(),
        port: host.port(),
        token: None,
        workspace_id: "shared".to_string(),
    })
    .expect("server start should reuse existing runtime");

    assert_eq!(result.runtime.token, "existing-token");
    assert_eq!(
        result.join_commands,
        vec![format!(
            "stateful server join http://127.0.0.1:{} --token existing-token",
            host.port()
        )]
    );
}

#[test]
fn server_start_explicit_host_and_workspace_are_reflected_in_join_command() {
    let fixture = TestFixture::new("serve-explicit-host-workspace");
    let host = FakeRuntimeServer::start();
    let runtime = ServerRuntime::new(host.base_url(), "secret-token", "w1", 0);
    write_global_runtime_file(&fixture.paths, &runtime).expect("runtime should write");

    let result = start_server_runtime(ServerStartRuntimeOptions {
        paths: fixture.paths.clone(),
        host: "127.0.0.1".to_string(),
        port: host.port(),
        token: Some("secret-token".to_string()),
        workspace_id: "w1".to_string(),
    })
    .expect("server start should reuse existing runtime");

    assert_eq!(
        result.join_commands,
        vec![format!(
            "stateful server join http://127.0.0.1:{} --token secret-token --workspace-id w1",
            host.port()
        )]
    );
}

#[test]
fn server_start_command_always_prints_join_command() {
    let fixture = TestFixture::new("server-start-prints-join");
    let host = FakeRuntimeServer::start();
    let runtime = ServerRuntime::new(host.base_url(), "existing-token", "shared", 0);
    write_global_runtime_file(&fixture.paths, &runtime).expect("runtime should write");

    let output = Command::new(env!("CARGO_BIN_EXE_stateful"))
        .args([
            "server",
            "start",
            "--host",
            "127.0.0.1",
            "--port",
            &host.port().to_string(),
            "--workspace-id",
            "shared",
        ])
        .env("STATEFUL_HOME", &fixture.paths.home)
        .output()
        .expect("stateful binary should run");

    assert!(output.status.success(), "server start should succeed");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains(&format!(
            "stateful server join http://127.0.0.1:{} --token existing-token",
            host.port()
        )),
        "server start output should include join command, got: {stdout}"
    );
}

struct TestFixture {
    root: PathBuf,
    paths: GlobalPaths,
    codex_config: PathBuf,
    repo: PathBuf,
}

impl TestFixture {
    fn new(name: &str) -> Self {
        let root = std::env::temp_dir().join(format!("stateful-lan-{name}-{}", std::process::id()));
        if root.exists() {
            fs::remove_dir_all(&root).expect("old temp root should remove");
        }
        fs::create_dir_all(&root).expect("temp root should create");
        let paths = GlobalPaths::new(root.join("home"));
        let codex_config = root.join("codex").join("config.toml");
        let repo = root.join("repo");
        fs::create_dir_all(&repo).expect("repo dir should create");
        Self {
            root,
            paths,
            codex_config,
            repo,
        }
    }
}

impl Drop for TestFixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

struct FakeHttpServer {
    addr: std::net::SocketAddr,
}

impl FakeHttpServer {
    fn start(responses: Vec<String>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("fake server should bind");
        let addr = listener.local_addr().expect("fake addr should be known");
        thread::spawn(move || {
            for response in responses {
                if let Ok((mut stream, _)) = listener.accept() {
                    read_request(&mut stream);
                    stream
                        .write_all(response.as_bytes())
                        .expect("fake response should write");
                }
            }
        });
        Self { addr }
    }

    fn base_url(&self) -> String {
        format!("http://{}", self.addr)
    }
}

struct FakeRuntimeServer {
    addr: std::net::SocketAddr,
}

impl FakeRuntimeServer {
    fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("fake runtime should bind");
        let addr = listener.local_addr().expect("fake addr should be known");
        thread::spawn(move || {
            for response in [
                http_response(200, r#"{"status":"ok"}"#),
                http_response(200, r#"{"status":"ok","current":{}}"#),
                identity_response(200),
            ] {
                if let Ok((mut stream, _)) = listener.accept() {
                    read_any_request(&mut stream);
                    stream
                        .write_all(response.as_bytes())
                        .expect("fake response should write");
                }
            }
        });
        Self { addr }
    }

    fn base_url(&self) -> String {
        format!("http://{}", self.addr)
    }

    fn port(&self) -> u16 {
        self.addr.port()
    }
}

fn read_request(stream: &mut TcpStream) {
    let mut buffer = [0_u8; 1024];
    let bytes = stream.read(&mut buffer).expect("request should read");
    let request = String::from_utf8_lossy(&buffer[..bytes]);
    assert!(request.contains("GET /v1/runtime/identity HTTP/1.1"));
    assert!(
        request.contains("Authorization: Bearer secret-token")
            || request.contains("Authorization: Bearer bad-token")
    );
}

fn read_any_request(stream: &mut TcpStream) {
    let mut buffer = [0_u8; 1024];
    let _ = stream.read(&mut buffer).expect("request should read");
}

fn identity_response(status: u16) -> String {
    identity_response_with_pid(status, 9876)
}

fn identity_response_with_pid(status: u16, pid: u32) -> String {
    if status == 401 {
        return http_response(401, r#"{"error":"unauthorized"}"#);
    }
    http_response(200, &identity_body(pid))
}

fn identity_body(pid: u32) -> String {
    format!(
        "{{\"status\":\"ok\",\"pid\":{pid},\"protocol_version\":\"stateful.v1\",\"capabilities\":[\"authorize.write_directory\"]}}"
    )
}

fn identity_response_without_write_dir_capability() -> String {
    http_response(
        200,
        r#"{"status":"ok","pid":9876,"protocol_version":"stateful.v1","capabilities":[]}"#,
    )
}

fn http_response(status: u16, body: &str) -> String {
    let reason = match status {
        200 => "OK",
        401 => "Unauthorized",
        _ => "Unknown",
    };
    format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
}

fn init_git_repo(path: &Path) {
    let status = std::process::Command::new("git")
        .arg("init")
        .arg(path)
        .status()
        .expect("git init should run");
    assert!(status.success());
}
