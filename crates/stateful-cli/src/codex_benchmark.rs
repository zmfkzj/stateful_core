#[cfg(unix)]
use std::os::unix::fs::FileTypeExt;
#[cfg(any(target_os = "macos", test))]
use std::process::Command;
use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
    time::Duration,
};

#[cfg(any(target_os = "macos", test))]
use crate::sandbox::STATEFUL_ALLOW_NESTED_SANDBOX_RUN_ENV;
#[cfg(any(target_os = "macos", test))]
use crate::sandbox::apply_sandbox_temp_env;
#[cfg(target_os = "macos")]
use crate::sandbox::run_command_with_timeout;
use crate::sandbox::{
    STATEFUL_SANDBOX_RUN_ACTIVE_ENV, SandboxAuthorizationDenied, SandboxAuthorizeContext,
    SandboxAuthorizeDecision, SandboxCommandResult, SandboxRunOutput, SandboxWritablePath,
    SandboxWritablePathKind, agent_context_for_sandbox_profile, authorize_sandbox_write,
    complete_sandbox_write_authorization, enrich_sandbox_write_dir_denial, ensure_repo_dir_target,
    normalize_sandbox_target_path, push_seatbelt_device_read_allows, resolve_sandbox_cwd,
    sandbox_temp_dir, sandbox_write_dir_display_path, seatbelt_escape,
};
use crate::{
    GlobalPaths, RepoGate, discover_runtime_with_global, ensure_server, repo_gate,
    runtime_env_override_is_configured,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NestedCodexBenchmarkSandboxRequest {
    pub purpose: String,
    pub agent_id: String,
    pub workspace_id: Option<String>,
    pub write_dir: String,
    pub codex_home_root: String,
    pub docker_socket: Option<PathBuf>,
    pub command: String,
    pub timeout_seconds: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct NestedCodexBenchmarkSandboxPaths {
    write_dir: PathBuf,
    codex_home_root: PathBuf,
    write_dir_relative: String,
}

pub fn run_nested_codex_benchmark_sandbox_in_repo(
    repo_root: &Path,
    paths: &GlobalPaths,
    request: NestedCodexBenchmarkSandboxRequest,
) -> anyhow::Result<SandboxRunOutput> {
    if request.purpose.trim().is_empty() {
        anyhow::bail!("stateful sandbox run-nested-codex-benchmark requires --purpose");
    }
    if request.command.trim().is_empty() {
        anyhow::bail!("stateful sandbox run-nested-codex-benchmark requires a non-empty --command");
    }
    if !cfg!(test) && std::env::var_os(STATEFUL_SANDBOX_RUN_ACTIVE_ENV).is_some() {
        anyhow::bail!(
            "stateful sandbox run-nested-codex-benchmark must be the outermost sandbox command"
        );
    }

    let repo_root = match repo_gate(paths, repo_root)? {
        RepoGate::Enabled { repo_root } => {
            if !runtime_env_override_is_configured() {
                ensure_server(paths)?;
            }
            repo_root
        }
        RepoGate::Disabled => {
            anyhow::bail!("stateful sandbox run-nested-codex-benchmark requires an enabled repo")
        }
        RepoGate::OutsideGitRepo => {
            anyhow::bail!("stateful sandbox run-nested-codex-benchmark requires a Git repo")
        }
    };
    let runtime = discover_runtime_with_global(&repo_root, paths)?;
    let nested_paths = validate_nested_codex_benchmark_paths(
        &repo_root,
        &request.write_dir,
        &request.codex_home_root,
    )?;
    let docker_socket = request
        .docker_socket
        .as_deref()
        .map(validate_nested_codex_benchmark_docker_socket)
        .transpose()?;

    let agent_context = agent_context_for_sandbox_profile(
        &repo_root,
        paths,
        &runtime,
        Some(&request.agent_id),
        request.workspace_id.as_deref(),
        "sandbox run-nested-codex-benchmark requires --agent-id",
    )?;
    let authorize_context = SandboxAuthorizeContext {
        runtime: &runtime,
        repo_root: &repo_root,
        paths,
        agent_id: &agent_context.agent_id,
        workspace_id: &agent_context.workspace_id,
        reservation_id: None,
    };
    let authorization_path = sandbox_write_dir_display_path(&nested_paths.write_dir_relative);
    let authorization =
        authorize_sandbox_write(&authorize_context, "write_directory", &authorization_path)?;
    let operation_id = authorization.operation_id.clone();
    let mut allowed_write_targets = Vec::new();
    let mut denied_write_targets = Vec::new();
    match authorization.decision {
        SandboxAuthorizeDecision::Allow => allowed_write_targets.push(authorization_path),
        SandboxAuthorizeDecision::Warn(_) => allowed_write_targets.push(authorization_path),
        SandboxAuthorizeDecision::Deny(body) => {
            denied_write_targets.push(serde_json::json!({
                "path": sandbox_write_dir_display_path(&nested_paths.write_dir_relative),
                "authorization": enrich_sandbox_write_dir_denial(body),
            }));
        }
    }
    if !denied_write_targets.is_empty() {
        let body = serde_json::json!({
            "status": "error",
            "message": "stateful sandbox run-nested-codex-benchmark target authorization denied",
            "allowed_write_targets": allowed_write_targets,
            "denied_write_targets": denied_write_targets,
        })
        .to_string();
        if let Some(operation_id) = operation_id.as_deref() {
            complete_sandbox_write_authorization(&authorize_context, operation_id, true)?;
        }
        return Err(SandboxAuthorizationDenied::new(body).into());
    }

    let timeout = Duration::from_secs(request.timeout_seconds.unwrap_or(300).max(1));
    let result = (|| -> anyhow::Result<_> {
        let writable_paths = prepare_nested_codex_benchmark_writable_paths(&nested_paths)?;
        let cwd = resolve_sandbox_cwd(&repo_root)?;
        run_nested_codex_benchmark_sandboxed_command(
            &request.command,
            &cwd,
            &writable_paths,
            &nested_paths.codex_home_root,
            docker_socket.as_deref(),
            &runtime,
            timeout,
        )
    })();
    if let Some(operation_id) = operation_id {
        complete_sandbox_write_authorization(&authorize_context, &operation_id, result.is_err())?;
    }
    let result = result?;

    Ok(SandboxRunOutput {
        status: result.status,
        exit_code: result.exit_code,
        stdout: result.stdout,
        stderr: result.stderr,
        allowed_write_targets,
        denied_write_targets: Vec::new(),
        warnings: Vec::new(),
    })
}

fn validate_nested_codex_benchmark_paths(
    repo_root: &Path,
    write_dir: &str,
    codex_home_root: &str,
) -> anyhow::Result<NestedCodexBenchmarkSandboxPaths> {
    let write_dir = normalize_sandbox_target_path("write_dirs", write_dir)?;
    if write_dir != "target" {
        anyhow::bail!("stateful sandbox run-nested-codex-benchmark requires --write-dir target");
    }

    let codex_home_root =
        normalize_sandbox_target_path("codex_home_root", codex_home_root).map_err(|error| {
            anyhow::anyhow!(
                "stateful sandbox run-nested-codex-benchmark requires --codex-home-root under target: {error}"
            )
        })?;
    if !codex_home_root.starts_with("target/") {
        anyhow::bail!(
            "stateful sandbox run-nested-codex-benchmark requires --codex-home-root under target"
        );
    }

    ensure_repo_dir_target(repo_root, &write_dir).map_err(|error| {
        anyhow::anyhow!(
            "stateful sandbox run-nested-codex-benchmark write dir `{write_dir}` is unsafe: {error}"
        )
    })?;
    ensure_repo_dir_target(repo_root, &codex_home_root).map_err(|error| {
        anyhow::anyhow!(
            "stateful sandbox run-nested-codex-benchmark codex home root `{codex_home_root}` is unsafe: {error}"
        )
    })?;

    Ok(NestedCodexBenchmarkSandboxPaths {
        write_dir: repo_root.join(&write_dir),
        codex_home_root: repo_root.join(&codex_home_root),
        write_dir_relative: write_dir,
    })
}

fn validate_nested_codex_benchmark_docker_socket(docker_socket: &Path) -> anyhow::Result<PathBuf> {
    if !docker_socket.is_absolute() {
        anyhow::bail!(
            "stateful sandbox run-nested-codex-benchmark requires --docker-socket to be an absolute path"
        );
    }

    let metadata = fs::metadata(docker_socket).map_err(|error| {
        anyhow::anyhow!(
            "stateful sandbox run-nested-codex-benchmark Docker socket {} is unavailable: {error}",
            docker_socket.display()
        )
    })?;
    #[cfg(unix)]
    {
        if !metadata.file_type().is_socket() {
            anyhow::bail!(
                "stateful sandbox run-nested-codex-benchmark Docker socket {} is not a Unix socket",
                docker_socket.display()
            );
        }
    }
    #[cfg(not(unix))]
    {
        let _ = metadata;
        anyhow::bail!(
            "stateful sandbox run-nested-codex-benchmark --docker-socket is supported only on Unix hosts"
        );
    }

    Ok(docker_socket.to_path_buf())
}

fn prepare_nested_codex_benchmark_writable_paths(
    paths: &NestedCodexBenchmarkSandboxPaths,
) -> anyhow::Result<Vec<SandboxWritablePath>> {
    fs::create_dir_all(&paths.write_dir)?;
    fs::create_dir_all(paths.write_dir.join(".stateful-tmp"))?;
    fs::create_dir_all(&paths.codex_home_root)?;

    let mut seen = BTreeSet::new();
    let mut writable_paths = Vec::new();
    for path in [&paths.write_dir, &paths.codex_home_root] {
        let canonical = path.canonicalize()?;
        if seen.insert(canonical.clone()) {
            writable_paths.push(SandboxWritablePath::directory(canonical));
        }
    }

    Ok(writable_paths)
}

fn run_nested_codex_benchmark_sandboxed_command(
    command: &str,
    cwd: &Path,
    writable_paths: &[SandboxWritablePath],
    codex_home_root: &Path,
    docker_socket: Option<&Path>,
    runtime: &crate::ServerRuntime,
    timeout: Duration,
) -> anyhow::Result<SandboxCommandResult> {
    let temp_dir = sandbox_temp_dir(writable_paths)
        .ok_or_else(|| anyhow::anyhow!("nested Codex benchmark sandbox requires a temp dir"))?;
    #[cfg(target_os = "macos")]
    {
        run_command_with_timeout(
            nested_codex_benchmark_seatbelt_command(
                command,
                cwd,
                writable_paths,
                &temp_dir,
                codex_home_root,
                docker_socket,
                runtime,
            ),
            timeout,
            false,
        )
    }

    #[cfg(target_os = "linux")]
    {
        let _ = (
            command,
            cwd,
            writable_paths,
            codex_home_root,
            docker_socket,
            runtime,
            timeout,
            temp_dir,
        );
        anyhow::bail!(
            "stateful sandbox run-nested-codex-benchmark is currently supported only on macOS"
        );
    }

    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        let _ = (
            command,
            cwd,
            writable_paths,
            codex_home_root,
            docker_socket,
            runtime,
            timeout,
            temp_dir,
        );
        anyhow::bail!(
            "stateful sandbox run-nested-codex-benchmark is currently supported only on macOS"
        );
    }
}

#[cfg(any(target_os = "macos", test))]
fn apply_nested_codex_benchmark_env(
    command: &mut Command,
    temp_dir: &Path,
    codex_home_root: &Path,
    docker_socket: Option<&Path>,
    runtime: &crate::ServerRuntime,
) {
    apply_sandbox_temp_env(command, Some(temp_dir));
    command.env("STATEFUL_NESTED_CODEX_HOME_ROOT", codex_home_root);
    command.env(STATEFUL_ALLOW_NESTED_SANDBOX_RUN_ENV, "1");
    command.env("STATEFUL_SERVER_URL", &runtime.base_url);
    command.env("STATEFUL_SERVER_TOKEN", &runtime.token);
    if let Some(docker_socket) = docker_socket {
        command.env("DOCKER_HOST", format!("unix://{}", docker_socket.display()));
    }
}

#[cfg(target_os = "macos")]
fn nested_codex_benchmark_seatbelt_command(
    command: &str,
    cwd: &Path,
    writable_paths: &[SandboxWritablePath],
    temp_dir: &Path,
    codex_home_root: &Path,
    docker_socket: Option<&Path>,
    runtime: &crate::ServerRuntime,
) -> Command {
    let profile = nested_codex_benchmark_seatbelt_profile(writable_paths, docker_socket);
    let mut sandbox = Command::new("/usr/bin/sandbox-exec");
    sandbox
        .arg("-p")
        .arg(profile)
        .arg("/bin/sh")
        .arg("-c")
        .arg(command)
        .current_dir(cwd);
    apply_nested_codex_benchmark_env(
        &mut sandbox,
        temp_dir,
        codex_home_root,
        docker_socket,
        runtime,
    );
    sandbox
}

#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn nested_codex_benchmark_seatbelt_profile(
    writable_paths: &[SandboxWritablePath],
    docker_socket: Option<&Path>,
) -> String {
    let mut profile =
        String::from("(version 1)\n(deny default)\n(allow process*)\n(allow file-read*)\n");
    push_seatbelt_device_read_allows(&mut profile);
    profile.push_str(
        "(allow sysctl-read)\n(allow network*)\n(allow file-write* (literal \"/dev/null\")",
    );
    if let Some(docker_socket) = docker_socket {
        profile.push_str(" (literal \"");
        profile.push_str(&seatbelt_escape(&docker_socket.to_string_lossy()));
        profile.push_str("\")");
    }
    for writable_path in writable_paths {
        profile.push_str(match writable_path.kind {
            SandboxWritablePathKind::File => " (literal \"",
            SandboxWritablePathKind::Directory => " (subpath \"",
        });
        profile.push_str(&seatbelt_escape(&writable_path.path.to_string_lossy()));
        profile.push_str("\")");
    }
    profile.push_str(")\n");
    profile
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{GlobalPaths, ServerRuntime, enable_repo, write_global_runtime_file};
    use std::{
        fs,
        io::{Read, Write},
        net::TcpListener,
        sync::mpsc,
        thread,
        time::Duration,
    };

    fn spawn_benchmark_fake_server() -> (ServerRuntime, mpsc::Receiver<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("listener should bind");
        let address = listener.local_addr().expect("listener address should resolve");
        let (tx, rx) = mpsc::channel();
        thread::spawn(move || {
            while let Ok((mut stream, _)) = listener.accept() {
                let mut request = Vec::new();
                let mut byte = [0_u8; 1];
                while !request.ends_with(b"\r\n\r\n") {
                    stream.read_exact(&mut byte).expect("headers should read");
                    request.push(byte[0]);
                }
                let request = String::from_utf8(request).expect("request should be UTF-8");
                let content_length = request
                    .lines()
                    .find_map(|line| line.strip_prefix("Content-Length: "))
                    .and_then(|length| length.parse::<usize>().ok())
                    .unwrap_or_default();
                let mut request_body = vec![0; content_length];
                stream
                    .read_exact(&mut request_body)
                    .expect("request body should read");
                let request = format!(
                    "{}{}",
                    request,
                    String::from_utf8(request_body).expect("body should be UTF-8")
                );
                tx.send(request.clone()).expect("request should send");
                let body = if request.starts_with("GET /v2/runtime/identity?") {
                    r#"{"protocol_version":"stateful.v2","journal_schema_version":2,"coordination_mode":"awareness","pid":42,"workspace_id":"w1","workspace_version":1,"capabilities":["presence"]}"#
                } else if request.starts_with("POST /v2/authorize ") {
                    r#"{"intent_id":"intent-1","decision":{"decision":"deny","reason_code":"denied","message":"blocked"}}"#
                } else {
                    r#"{"status":"completed"}"#
                };
                write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                )
                .expect("response should write");
            }
        });
        (
            ServerRuntime::new(format!("http://{address}"), "token", "w1", 1),
            rx,
        )
    }

    #[test]
    fn denied_benchmark_intent_completes_as_failed() {
        let temp = tempfile::tempdir().expect("temp dir should create");
        let paths = GlobalPaths::new(temp.path().join("home"));
        let repo_root = temp.path().join("repo");
        fs::create_dir_all(repo_root.join(".git")).expect("git marker should create");
        fs::create_dir_all(repo_root.join("target/nested")).expect("nested target should create");
        fs::create_dir_all(repo_root.join("target")).expect("target should create");
        enable_repo(&paths, &repo_root).expect("repo should enable");
        let (runtime, requests) = spawn_benchmark_fake_server();
        write_global_runtime_file(&paths, &runtime).expect("runtime file should write");
        let error = run_nested_codex_benchmark_sandbox_in_repo(
            &repo_root,
            &paths,
            NestedCodexBenchmarkSandboxRequest {
                purpose: "test".to_string(),
                agent_id: "agent-1".to_string(),
                workspace_id: Some("w1".to_string()),
                write_dir: "target".to_string(),
                codex_home_root: "target/nested".to_string(),
                docker_socket: None,
                command: "true".to_string(),
                timeout_seconds: Some(1),
            },
        )
        .expect_err("denied benchmark should fail");
        assert!(error.to_string().contains("target authorization denied"), "{error}");
        for expected in ["/v2/runtime/identity?", "/v2/runtime/identity?"] {
            let request = requests
                .recv_timeout(Duration::from_secs(2))
                .expect("authorization identity request should arrive");
            assert!(request.contains(expected), "expected {expected}, got {request}");
        }
        let authorize = requests
            .recv_timeout(Duration::from_secs(2))
            .expect("authorization request should arrive");
        assert!(authorize.contains("POST /v2/authorize"), "{authorize}");
        let authorize_body = serde_json::from_str::<serde_json::Value>(
            authorize
                .split_once("\r\n\r\n")
                .expect("authorization should have a body")
                .1,
        )
        .expect("authorization body should be JSON");
        assert!(authorize_body["payload"]["operation_id"].is_string());
        let identity = requests
            .recv_timeout(Duration::from_millis(500))
            .expect("completion identity should arrive");
        assert!(identity.contains("/v2/runtime/identity?"));
        let completion = requests
            .recv_timeout(Duration::from_millis(500))
            .expect("failed completion should arrive");
        assert!(completion.contains("POST /v2/write/complete "), "{completion}");
        let body = serde_json::from_str::<serde_json::Value>(
            completion
                .split_once("\r\n\r\n")
                .expect("completion should have a body")
                .1,
        )
        .expect("completion body should be JSON");
        assert_eq!(body["payload"]["intent_id"], "intent-1");
        assert_eq!(body["payload"]["outcome"], "failed");
    }

    #[test]
    fn nested_codex_benchmark_paths_must_stay_under_target() {
        let repo_root = Path::new("/repo");
        let paths = validate_nested_codex_benchmark_paths(
            repo_root,
            "target",
            "target/nested-codex-homes/run-1",
        )
        .expect("valid nested Codex benchmark paths should pass");
        assert_eq!(paths.write_dir, PathBuf::from("/repo/target"));
        assert_eq!(
            paths.codex_home_root,
            PathBuf::from("/repo/target/nested-codex-homes/run-1")
        );

        for (write_dir, codex_home_root, expected) in [
            (
                "crates",
                "target/nested-codex-homes/run-1",
                "requires --write-dir target",
            ),
            (
                "target/bench",
                "target/nested-codex-homes/run-1",
                "requires --write-dir target",
            ),
            (
                "target",
                "target/../codex-home",
                "requires --codex-home-root under target",
            ),
        ] {
            let error =
                validate_nested_codex_benchmark_paths(repo_root, write_dir, codex_home_root)
                    .expect_err("invalid nested Codex benchmark paths should fail");
            assert!(
                error.to_string().contains(expected),
                "unexpected error for {write_dir} {codex_home_root}: {error}"
            );
        }
    }

    #[test]
    fn nested_codex_benchmark_env_sets_isolated_codex_home_root() {
        let mut command = Command::new("printenv");
        let runtime = ServerRuntime::new("http://127.0.0.1:43873", "token-123", "w1", 1234);
        apply_nested_codex_benchmark_env(
            &mut command,
            Path::new("/repo/target/.stateful-tmp"),
            Path::new("/repo/target/nested-codex-homes/run-1"),
            None,
            &runtime,
        );

        let env = command
            .get_envs()
            .map(|(key, value)| {
                (
                    key.to_string_lossy().to_string(),
                    value.map(|value| value.to_string_lossy().to_string()),
                )
            })
            .collect::<Vec<_>>();

        assert!(env.contains(&(
            "STATEFUL_SANDBOX_RUN_ACTIVE".to_string(),
            Some("1".to_string())
        )));
        assert_eq!(
            env.iter()
                .find(|(key, _)| key == "STATEFUL_NESTED_CODEX_HOME_ROOT")
                .map(|(_, value)| value),
            Some(&Some("/repo/target/nested-codex-homes/run-1".to_string()))
        );
        assert_eq!(
            env.iter()
                .find(|(key, _)| key == "STATEFUL_ALLOW_NESTED_SANDBOX_RUN")
                .map(|(_, value)| value),
            Some(&Some("1".to_string()))
        );
        assert_eq!(
            env.iter()
                .find(|(key, _)| key == "STATEFUL_SERVER_URL")
                .map(|(_, value)| value),
            Some(&Some("http://127.0.0.1:43873".to_string()))
        );
        assert_eq!(
            env.iter()
                .find(|(key, _)| key == "STATEFUL_SERVER_TOKEN")
                .map(|(_, value)| value),
            Some(&Some("token-123".to_string()))
        );
    }

    #[test]
    fn nested_codex_benchmark_env_sets_docker_host_when_socket_is_explicit() {
        let mut command = Command::new("printenv");
        let runtime = ServerRuntime::new("http://127.0.0.1:43873", "token-123", "w1", 1234);
        apply_nested_codex_benchmark_env(
            &mut command,
            Path::new("/repo/target/.stateful-tmp"),
            Path::new("/repo/target/nested-codex-homes/run-1"),
            Some(Path::new("/tmp/colima/default/docker.sock")),
            &runtime,
        );

        let env = command
            .get_envs()
            .map(|(key, value)| {
                (
                    key.to_string_lossy().to_string(),
                    value.map(|value| value.to_string_lossy().to_string()),
                )
            })
            .collect::<Vec<_>>();

        assert_eq!(
            env.iter()
                .find(|(key, _)| key == "DOCKER_HOST")
                .map(|(_, value)| value),
            Some(&Some("unix:///tmp/colima/default/docker.sock".to_string()))
        );
    }

    #[test]
    fn nested_codex_benchmark_seatbelt_profile_is_target_only_with_network() {
        let profile = nested_codex_benchmark_seatbelt_profile(
            &[
                SandboxWritablePath::directory(PathBuf::from("/repo/target")),
                SandboxWritablePath::directory(PathBuf::from(
                    "/repo/target/nested-codex-homes/run-1",
                )),
            ],
            None,
        );

        assert!(profile.contains("(allow network*)"));
        assert!(profile.contains(
            "(allow file-read* (literal \"/dev/null\") (literal \"/dev/zero\") (literal \"/dev/urandom\"))"
        ));
        let write_rules = profile
            .lines()
            .filter(|line| line.starts_with("(allow file-write*"))
            .collect::<Vec<_>>();
        assert!(
            write_rules
                .iter()
                .any(|line| line.contains("(literal \"/dev/null\")"))
        );
        assert!(!write_rules.iter().any(|line| line.contains("/dev/zero")));
        assert!(!write_rules.iter().any(|line| line.contains("/dev/urandom")));
        assert!(profile.contains("(subpath \"/repo/target\")"));
        assert!(profile.contains("(subpath \"/repo/target/nested-codex-homes/run-1\")"));
        assert!(!profile.contains("(subpath \"/repo\")"));
    }

    #[test]
    fn nested_codex_benchmark_seatbelt_profile_allows_explicit_docker_socket() {
        let profile = nested_codex_benchmark_seatbelt_profile(
            &[SandboxWritablePath::directory(PathBuf::from(
                "/repo/target",
            ))],
            Some(Path::new("/tmp/colima/default/docker.sock")),
        );

        let write_rules = profile
            .lines()
            .filter(|line| line.starts_with("(allow file-write*"))
            .collect::<Vec<_>>();
        assert!(
            write_rules
                .iter()
                .any(|line| line.contains("(literal \"/tmp/colima/default/docker.sock\")"))
        );
    }
}
