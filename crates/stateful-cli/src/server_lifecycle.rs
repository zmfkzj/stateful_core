use std::{
    fs,
    fs::OpenOptions,
    io::{Read, Seek, SeekFrom, Write},
    path::Path,
    process::{Child, Command, Stdio},
    thread,
    time::{Duration, Instant, SystemTime},
};

use crate::{
    global_paths::GlobalPaths,
    runtime::{
        ServerRuntime, get_payload, get_response, runtime_base_url_is_loopback,
        validate_runtime_process_identity, write_global_runtime_file,
    },
};

#[cfg(test)]
use crate::process_start_identity_for_pid;

const START_LOCK_TIMEOUT: Duration = Duration::from_secs(2);
const START_LOCK_RETRY_DELAY: Duration = Duration::from_millis(50);
const START_LOCK_STALE_AFTER: Duration = Duration::from_secs(30);
const STARTUP_HEALTH_TIMEOUT: Duration = Duration::from_secs(5);
const STARTUP_HEALTH_RETRY_DELAY: Duration = Duration::from_millis(50);
#[derive(Clone, PartialEq, Eq)]
pub struct ServerStartOptions {
    pub host: String,
    pub port: u16,
    pub token: Option<String>,
    pub workspace_id: String,
}

impl std::fmt::Debug for ServerStartOptions {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ServerStartOptions")
            .field("host", &self.host)
            .field("port", &self.port)
            .field("token", &self.token.as_ref().map(|_| "<redacted>"))
            .field("workspace_id", &self.workspace_id)
            .finish()
    }
}

impl Default for ServerStartOptions {
    fn default() -> Self {
        Self {
            host: "127.0.0.1".to_string(),
            port: 43873,
            token: None,
            workspace_id: "local".to_string(),
        }
    }
}

pub fn ensure_server_with<H, S>(
    paths: &GlobalPaths,
    health: H,
    start: S,
) -> anyhow::Result<ServerRuntime>
where
    H: Fn(&ServerRuntime) -> bool,
    S: Fn() -> anyhow::Result<ServerRuntime>,
{
    if let Some(runtime) = read_runtime_file(paths)? {
        if validate_runtime_process_identity(&runtime).is_ok() && health(&runtime) {
            return Ok(runtime);
        }
    }

    let _lock = acquire_start_lock(paths)?;
    if let Some(runtime) = read_runtime_file(paths)? {
        if validate_runtime_process_identity(&runtime).is_ok() && health(&runtime) {
            return Ok(runtime);
        }
    }

    let runtime = start()?;
    write_global_runtime_file(paths, &runtime)?;
    Ok(runtime)
}

pub fn ensure_server(paths: &GlobalPaths) -> anyhow::Result<ServerRuntime> {
    if let Some(runtime) = read_runtime_file(paths)? {
        if runtime_is_healthy(&runtime) {
            return Ok(runtime);
        }
    }

    {
        let _lock = acquire_start_lock(paths)?;
        if let Some(runtime) = read_runtime_file(paths)? {
            if runtime_is_healthy(&runtime) {
                return Ok(runtime);
            }
            retire_incompatible_runtime(paths, &runtime)?;
        }
    }

    let options = ServerStartOptions::default();
    let runtime = start_detached_server(paths, &options)?;
    write_global_runtime_file(paths, &runtime)?;
    Ok(runtime)
}

pub fn ensure_server_with_options(
    paths: &GlobalPaths,
    options: ServerStartOptions,
) -> anyhow::Result<ServerRuntime> {
    validate_server_start_options(&options)?;
    ensure_current_server_with_options(paths, &options)
}

fn ensure_current_server_with_options(
    paths: &GlobalPaths,
    options: &ServerStartOptions,
) -> anyhow::Result<ServerRuntime> {
    if let Some(runtime) = read_runtime_file(paths)? {
        if runtime_is_healthy(&runtime) {
            ensure_runtime_matches_options(&runtime, options)?;
            return Ok(runtime);
        }
    }

    {
        let _lock = acquire_start_lock(paths)?;
        if let Some(runtime) = read_runtime_file(paths)? {
            if runtime_is_healthy(&runtime) {
                ensure_runtime_matches_options(&runtime, options)?;
                return Ok(runtime);
            }
            retire_incompatible_runtime(paths, &runtime)?;
        }
    }

    let runtime = start_detached_server(paths, options)?;
    write_global_runtime_file(paths, &runtime)?;
    Ok(runtime)
}

pub(crate) fn register_foreground_runtime(
    paths: &GlobalPaths,
    runtime: &ServerRuntime,
    options: &ServerStartOptions,
) -> anyhow::Result<()> {
    validate_server_start_options(options)?;
    register_foreground_runtime_with_health(paths, runtime, options, runtime_is_healthy)
}

fn register_foreground_runtime_with_health<H>(
    paths: &GlobalPaths,
    runtime: &ServerRuntime,
    options: &ServerStartOptions,
    health: H,
) -> anyhow::Result<()>
where
    H: Fn(&ServerRuntime) -> bool,
{
    let _lock = acquire_start_lock(paths)?;
    if let Some(existing) = read_runtime_file(paths)?
        && validate_runtime_process_identity(&existing).is_ok()
        && health(&existing)
    {
        ensure_runtime_matches_options(&existing, options)?;
        anyhow::bail!(
            "stateful server is already running at {} workspace {} pid {}; stop it with `stateful server stop` before starting a foreground server",
            existing.base_url,
            existing.workspace_id,
            existing.pid
        );
    }

    write_global_runtime_file(paths, runtime)?;
    Ok(())
}

pub fn runtime_is_healthy(runtime: &ServerRuntime) -> bool {
    validate_runtime_process_identity(runtime).is_ok()
        && get_response(runtime, "/health").is_ok_and(|health| health.status_code == 200)
        && get_payload::<stateful_store::StatusSnapshot>(runtime, "/v2/status").is_ok()
}

pub fn stop_server(paths: &GlobalPaths) -> anyhow::Result<()> {
    let contents = fs::read_to_string(&paths.server_json)?;
    let runtime: ServerRuntime = serde_json::from_str(&contents)?;
    if !runtime_file_is_local_and_complete(&runtime)
        || validate_runtime_process_identity(&runtime).is_err()
        || !pid_matches_current_exe(runtime.pid)?
    {
        anyhow::bail!(
            "refusing to stop unverified stateful server pid {}",
            runtime.pid
        );
    }

    terminate_runtime(paths, &runtime)
}

pub fn restart_server(paths: &GlobalPaths) -> anyhow::Result<ServerRuntime> {
    let runtime = read_runtime_file(paths)?
        .ok_or_else(|| anyhow::anyhow!("no stateful server runtime file found to restart"))?;
    let options = server_start_options_from_runtime(&runtime)?;
    stop_server(paths)?;
    ensure_server_with_options(paths, options)
}

pub fn server_start_options_from_runtime(
    runtime: &ServerRuntime,
) -> anyhow::Result<ServerStartOptions> {
    if !runtime_base_url_is_loopback(&runtime.base_url) {
        anyhow::bail!("only loopback stateful server URLs can be restarted");
    }
    let (host, port) = parse_runtime_base_url(&runtime.base_url)?;
    Ok(ServerStartOptions {
        host,
        port,
        token: Some(runtime.token.clone()),
        workspace_id: runtime.workspace_id.clone(),
    })
}

fn parse_runtime_base_url(base_url: &str) -> anyhow::Result<(String, u16)> {
    let without_scheme = base_url
        .strip_prefix("http://")
        .ok_or_else(|| anyhow::anyhow!("only http:// stateful server URLs can be restarted"))?;
    let authority = without_scheme
        .split('/')
        .next()
        .ok_or_else(|| anyhow::anyhow!("missing stateful server authority"))?;
    if authority.is_empty() {
        anyhow::bail!("missing stateful server authority");
    }
    if let Some(rest) = authority.strip_prefix('[') {
        let (host, port) = rest
            .split_once("]:")
            .ok_or_else(|| anyhow::anyhow!("invalid bracketed stateful server authority"))?;
        if host.is_empty() {
            anyhow::bail!("missing stateful server host");
        }
        return Ok((format!("[{host}]"), port.parse()?));
    }
    let (host, port) = authority
        .rsplit_once(':')
        .ok_or_else(|| anyhow::anyhow!("missing stateful server port"))?;
    if host.is_empty() {
        anyhow::bail!("missing stateful server host");
    }
    Ok((host.to_string(), port.parse()?))
}

fn retire_incompatible_runtime(paths: &GlobalPaths, runtime: &ServerRuntime) -> anyhow::Result<()> {
    if validate_runtime_process_identity(runtime).is_err() {
        let _ = fs::remove_file(&paths.server_json);
        return Ok(());
    }
    if !pid_matches_current_exe(runtime.pid)? {
        anyhow::bail!(
            "existing stateful server pid {} is not the current stateful executable",
            runtime.pid
        );
    }
    terminate_runtime(paths, runtime)
}

fn terminate_runtime(paths: &GlobalPaths, runtime: &ServerRuntime) -> anyhow::Result<()> {
    if validate_runtime_process_identity(runtime).is_err() || !pid_matches_current_exe(runtime.pid)?
    {
        anyhow::bail!(
            "refusing to terminate unverified stateful server pid {}",
            runtime.pid
        );
    }
    let status = Command::new("kill").arg(runtime.pid.to_string()).status()?;
    if !status.success() {
        anyhow::bail!("failed to stop stateful server pid {}", runtime.pid);
    }
    for _ in 0..20 {
        if !runtime_is_healthy(runtime) {
            break;
        }
        thread::sleep(Duration::from_millis(50));
    }
    if runtime_is_healthy(runtime) {
        anyhow::bail!("stateful server pid {} did not stop", runtime.pid);
    }
    let _ = fs::remove_file(&paths.server_json);
    Ok(())
}

fn start_detached_server(
    paths: &GlobalPaths,
    options: &ServerStartOptions,
) -> anyhow::Result<ServerRuntime> {
    fs::create_dir_all(&paths.runtime_dir)?;
    let log = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&paths.server_log)?;
    let log_start = log.metadata()?.len();
    let log_err = log.try_clone()?;
    let mut command = Command::new(std::env::current_exe()?);
    configure_detached_command(&mut command);
    let mut child = command
        .args(detached_server_args(options))
        .env("STATEFUL_HOME", &paths.home)
        .stdin(if options.token.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::from(log))
        .stderr(Stdio::from(log_err))
        .spawn()?;
    if let Some(token) = &options.token {
        write_detached_token(&mut child, token)?;
    }

    wait_for_detached_server_health(paths, &mut child, log_start, STARTUP_HEALTH_TIMEOUT)
}

fn wait_for_detached_server_health(
    paths: &GlobalPaths,
    child: &mut Child,
    log_start: u64,
    timeout: Duration,
) -> anyhow::Result<ServerRuntime> {
    let started_at = Instant::now();
    while started_at.elapsed() < timeout {
        thread::sleep(STARTUP_HEALTH_RETRY_DELAY);
        if let Some(runtime) = read_runtime_file(paths)? {
            if runtime.pid == child.id()
                && validate_runtime_process_identity(&runtime).is_ok()
                && runtime_is_healthy(&runtime)
            {
                return Ok(runtime);
            }
        }
        if let Some(status) = child.try_wait()? {
            anyhow::bail!(
                "stateful server exited before becoming healthy with status {} after starting pid {}{}",
                status,
                child.id(),
                startup_log_detail(paths, log_start)
            );
        }
    }

    anyhow::bail!(
        "stateful server did not become healthy after starting pid {}{}",
        child.id(),
        startup_log_detail(paths, log_start)
    )
}

fn startup_log_detail(paths: &GlobalPaths, start_offset: u64) -> String {
    match startup_log_excerpt(paths, start_offset) {
        Ok(Some(excerpt)) => format!("; recent server log: {excerpt}"),
        Ok(None) | Err(_) => String::new(),
    }
}

fn startup_log_excerpt(paths: &GlobalPaths, start_offset: u64) -> anyhow::Result<Option<String>> {
    let mut file = fs::File::open(&paths.server_log)?;
    file.seek(SeekFrom::Start(start_offset))?;
    let mut contents = String::new();
    file.read_to_string(&mut contents)?;
    let lines = contents
        .lines()
        .filter(|line| !line.trim().is_empty())
        .rev()
        .take(3)
        .collect::<Vec<_>>();
    if lines.is_empty() {
        return Ok(None);
    }

    Ok(Some(
        lines.into_iter().rev().collect::<Vec<_>>().join(" | "),
    ))
}

pub fn detached_server_args(options: &ServerStartOptions) -> Vec<String> {
    let mut args = vec![
        "server".to_string(),
        "start".to_string(),
        "--foreground".to_string(),
        "--host".to_string(),
        options.host.clone(),
        "--port".to_string(),
        options.port.to_string(),
    ];

    if options.token.is_some() {
        args.push("--token-stdin".to_string());
    }

    args.extend(["--workspace-id".to_string(), options.workspace_id.clone()]);
    args
}

fn write_detached_token(child: &mut Child, token: &str) -> anyhow::Result<()> {
    let Some(mut stdin) = child.stdin.take() else {
        anyhow::bail!("detached stateful server stdin is unavailable");
    };
    let result = stdin
        .write_all(token.as_bytes())
        .and_then(|_| stdin.flush());
    drop(stdin);
    if let Err(error) = result
        && error.kind() != std::io::ErrorKind::BrokenPipe
        && child.try_wait()?.is_none()
    {
        return Err(error.into());
    }
    Ok(())
}
#[cfg(unix)]
fn configure_detached_command(command: &mut Command) {
    use std::os::unix::process::CommandExt;

    command.process_group(0);
}

#[cfg(not(unix))]
fn configure_detached_command(_command: &mut Command) {}

fn ensure_runtime_matches_options(
    runtime: &ServerRuntime,
    options: &ServerStartOptions,
) -> anyhow::Result<()> {
    let expected_base_url = format!("http://{}:{}", options.host, options.port);
    let token_matches = options
        .token
        .as_ref()
        .is_none_or(|token| token == &runtime.token);
    if runtime.base_url == expected_base_url
        && token_matches
        && runtime.workspace_id == options.workspace_id
    {
        return Ok(());
    }

    anyhow::bail!(
        "existing stateful server runtime does not match requested server options: existing {} workspace {} pid {}, requested {} workspace {}; stop the existing server with `stateful server stop`, then retry the requested command",
        runtime.base_url,
        runtime.workspace_id,
        runtime.pid,
        expected_base_url,
        options.workspace_id
    )
}

fn validate_server_start_options(options: &ServerStartOptions) -> anyhow::Result<()> {
    let base_url = format!("http://{}:{}", options.host, options.port);
    if !runtime_base_url_is_loopback(&base_url) {
        anyhow::bail!("stateful server may only bind a loopback host");
    }
    Ok(())
}

fn acquire_start_lock(paths: &GlobalPaths) -> anyhow::Result<StartLock> {
    fs::create_dir_all(&paths.runtime_dir)?;
    let started_at = SystemTime::now();

    loop {
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&paths.server_lock)
        {
            Ok(_) => {
                return Ok(StartLock {
                    path: paths.server_lock.clone(),
                });
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                if remove_stale_lock(&paths.server_lock)? {
                    continue;
                }

                if started_at.elapsed().unwrap_or_default() >= START_LOCK_TIMEOUT {
                    anyhow::bail!(
                        "timed out acquiring stateful server start lock at {}",
                        paths.server_lock.display()
                    );
                }
                thread::sleep(START_LOCK_RETRY_DELAY);
            }
            Err(error) => return Err(error.into()),
        }
    }
}

fn read_runtime_file(paths: &GlobalPaths) -> anyhow::Result<Option<ServerRuntime>> {
    match fs::read_to_string(&paths.server_json) {
        Ok(contents) => match serde_json::from_str(&contents) {
            Ok(runtime) if runtime_file_is_local_and_complete(&runtime) => Ok(Some(runtime)),
            Ok(_) | Err(_) => Ok(None),
        },
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}

fn runtime_file_is_local_and_complete(runtime: &ServerRuntime) -> bool {
    runtime_base_url_is_loopback(&runtime.base_url)
        && !runtime.token.is_empty()
        && !runtime.workspace_id.is_empty()
        && runtime.pid != 0
        && !runtime.process_start_identity.trim().is_empty()
}

fn remove_stale_lock(path: &Path) -> anyhow::Result<bool> {
    let metadata = match fs::metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error.into()),
    };
    let modified = metadata.modified()?;
    if modified.elapsed().unwrap_or_default() < START_LOCK_STALE_AFTER {
        return Ok(false);
    }

    match fs::remove_file(path) {
        Ok(()) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error.into()),
    }
}

fn pid_matches_current_exe(pid: u32) -> anyhow::Result<bool> {
    let output = Command::new("ps")
        .args(["-p", &pid.to_string(), "-o", "command="])
        .output()?;
    if !output.status.success() {
        return Ok(false);
    }

    let command = String::from_utf8(output.stdout)?;
    let current_exe = std::env::current_exe()?;
    let Some(current_exe_name) = current_exe.file_name().and_then(|name| name.to_str()) else {
        return Ok(false);
    };

    Ok(command.contains(current_exe.to_string_lossy().as_ref())
        || command.split_whitespace().next().is_some_and(|program| {
            Path::new(program)
                .file_name()
                .and_then(|name| name.to_str())
                == Some(current_exe_name)
        }))
}

struct StartLock {
    path: std::path::PathBuf,
}

impl Drop for StartLock {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        io::{Read, Write},
        net::{TcpListener, TcpStream},
    };

    #[test]
    fn foreground_registration_respects_existing_start_lock() {
        let home = temp_home("stateful-foreground-start-lock");
        let paths = GlobalPaths::new(&home);
        fs::create_dir_all(&paths.runtime_dir).expect("runtime dir should be creatable");
        fs::write(&paths.server_lock, "other-process").expect("lock should be writable");
        let options = ServerStartOptions::default();
        let runtime = ServerRuntime::new(
            "http://127.0.0.1:43873",
            "token",
            options.workspace_id.clone(),
            123,
            "process-start-123",
        );

        let error = register_foreground_runtime_with_health(&paths, &runtime, &options, |_| false)
            .expect_err("foreground registration should respect the start lock");

        assert!(
            error.to_string().contains("start lock"),
            "unexpected error: {error}"
        );
        assert!(!paths.server_json.exists());
        assert_eq!(
            fs::read_to_string(&paths.server_lock).expect("lock should remain readable"),
            "other-process"
        );
        let _ = fs::remove_dir_all(home);
    }

    #[test]
    fn foreground_registration_rejects_conflicting_healthy_runtime() {
        let home = temp_home("stateful-foreground-runtime-conflict");
        let paths = GlobalPaths::new(&home);
        let pid = std::process::id();
        let existing = ServerRuntime::new(
            "http://127.0.0.1:43874",
            "token",
            "local",
            pid,
            process_start_identity_for_pid(pid).expect("current process should have an identity"),
        );
        write_global_runtime_file(&paths, &existing).expect("existing runtime should write");
        let options = ServerStartOptions::default();
        let runtime = ServerRuntime::new(
            "http://127.0.0.1:43873",
            "token",
            options.workspace_id.clone(),
            123,
            "process-start-123",
        );

        let error = register_foreground_runtime_with_health(&paths, &runtime, &options, |_| true)
            .expect_err("foreground registration should reject a healthy conflicting runtime");

        assert!(
            error
                .to_string()
                .contains("does not match requested server options"),
            "unexpected error: {error}"
        );
        let contents = fs::read_to_string(&paths.server_json).expect("runtime should remain");
        let preserved: ServerRuntime =
            serde_json::from_str(&contents).expect("runtime should remain valid JSON");
        assert_eq!(preserved.base_url, existing.base_url);
        assert_eq!(preserved.pid, existing.pid);
        let _ = fs::remove_dir_all(home);
    }

    #[test]
    fn runtime_process_identity_requires_matching_start_identity() {
        let pid = std::process::id();
        let identity =
            process_start_identity_for_pid(pid).expect("current process should have an identity");
        let mut runtime =
            ServerRuntime::new("http://127.0.0.1:43873", "token", "w1", pid, identity);

        assert!(validate_runtime_process_identity(&runtime).is_ok());
        runtime.process_start_identity = "different-start-identity".to_string();
        assert!(validate_runtime_process_identity(&runtime).is_err());
    }

    fn temp_home(name: &str) -> std::path::PathBuf {
        let root = std::env::temp_dir().join(format!("{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).expect("temp home should be creatable");
        root
    }

    #[test]
    fn detached_start_waits_for_matching_process_identity_and_v2_health() {
        let home = temp_home("stateful-detached-slow-health");
        let paths = GlobalPaths::new(&home);
        fs::create_dir_all(&paths.runtime_dir).expect("runtime dir should be creatable");
        fs::write(&paths.server_log, "").expect("server log should be creatable");
        let mut child = Command::new("sleep")
            .arg("5")
            .spawn()
            .expect("sleep child should spawn");
        let fake = FakeHttpServer::start(vec![
            fake_response(200, "ok"),
            fake_response(
                200,
                r#"{"protocol_version":"stateful.v2","contract_revision":"lease-1","request_id":null,"payload":{"active_tasks":0,"draining_tasks":0,"active_leases":0,"draining_leases":0,"queued_requests":0,"offered_requests":0,"executing_writes":0,"uncertain_writes":0}}"#,
            ),
        ]);
        let delayed_runtime = ServerRuntime::new(
            fake.base_url(),
            "token",
            "w1",
            child.id(),
            process_start_identity_for_pid(child.id())
                .expect("child process should have a start identity"),
        );
        let delayed_paths = paths.clone();
        thread::spawn(move || {
            thread::sleep(Duration::from_millis(1200));
            write_global_runtime_file(&delayed_paths, &delayed_runtime)
                .expect("delayed runtime should write");
        });

        let result = wait_for_detached_server_health(&paths, &mut child, 0, Duration::from_secs(3))
            .expect("detached startup should tolerate slow health");

        assert_eq!(result.pid, child.id());
        let _ = child.kill();
        let _ = child.wait();
        let _ = fs::remove_dir_all(home);
    }

    fn fake_response(status: u16, body: impl AsRef<str>) -> String {
        let body = body.as_ref();
        format!(
            "HTTP/1.1 {status} OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        )
    }

    struct FakeHttpServer {
        addr: std::net::SocketAddr,
    }

    impl FakeHttpServer {
        fn start(responses: Vec<String>) -> Self {
            let listener = TcpListener::bind("127.0.0.1:0").expect("fake server should bind");
            let addr = listener
                .local_addr()
                .expect("fake server addr should be known");
            thread::spawn(move || {
                for response in responses {
                    if let Ok((mut stream, _)) = listener.accept() {
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
}
