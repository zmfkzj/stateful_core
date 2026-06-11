use std::{
    fs,
    fs::OpenOptions,
    io::{Read, Seek, SeekFrom},
    path::Path,
    process::{Command, Stdio},
    thread,
    time::{Duration, SystemTime},
};

use crate::runtime::{runtime_base_url_is_localhost, runtime_identity_pid};
use crate::{
    GlobalPaths, ServerRuntime, get_json, runtime_has_required_identity,
    runtime_identity_matches_pid, write_global_runtime_file,
};

const START_LOCK_TIMEOUT: Duration = Duration::from_secs(2);
const START_LOCK_RETRY_DELAY: Duration = Duration::from_millis(50);
const START_LOCK_STALE_AFTER: Duration = Duration::from_secs(30);
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerStartOptions {
    pub host: String,
    pub port: u16,
    pub token: Option<String>,
    pub workspace_id: String,
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
        if health(&runtime) {
            return Ok(runtime);
        }
    }

    let _lock = acquire_start_lock(paths)?;
    if let Some(runtime) = read_runtime_file(paths)? {
        if health(&runtime) {
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

    let _lock = acquire_start_lock(paths)?;
    if let Some(runtime) = read_runtime_file(paths)? {
        if runtime_is_healthy(&runtime) {
            return Ok(runtime);
        }
        retire_incompatible_runtime(paths, &runtime)?;
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

    let _lock = acquire_start_lock(paths)?;
    if let Some(runtime) = read_runtime_file(paths)? {
        if runtime_is_healthy(&runtime) {
            ensure_runtime_matches_options(&runtime, options)?;
            return Ok(runtime);
        }
        retire_incompatible_runtime(paths, &runtime)?;
    }

    let runtime = start_detached_server(paths, options)?;
    write_global_runtime_file(paths, &runtime)?;
    Ok(runtime)
}

pub fn runtime_is_healthy(runtime: &ServerRuntime) -> bool {
    runtime_is_basic_healthy(runtime) && runtime_has_required_identity(runtime)
}

fn runtime_is_basic_healthy(runtime: &ServerRuntime) -> bool {
    let Ok(health) = get_json(runtime, "/health") else {
        return false;
    };
    if health.status_code != 200 {
        return false;
    }

    let Ok(current) = get_json(runtime, "/v1/current") else {
        return false;
    };
    if current.status_code != 200 {
        return false;
    }

    serde_json::from_str::<serde_json::Value>(&current.body)
        .ok()
        .and_then(|body| {
            let status = body.get("status")?.as_str()?;
            let current = body.get("current")?;
            Some(status == "ok" && current.is_object())
        })
        .unwrap_or(false)
}

pub fn stop_server(paths: &GlobalPaths) -> anyhow::Result<()> {
    let contents = fs::read_to_string(&paths.server_json)?;
    let runtime: ServerRuntime = serde_json::from_str(&contents)?;
    if runtime.pid == 0
        || !runtime_is_basic_healthy(&runtime)
        || !runtime_identity_matches_pid(&runtime)?
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
    let runtime = runtime_for_local_restart(paths, runtime)?;
    let options = server_start_options_from_runtime(&runtime)?;
    stop_server(paths)?;
    ensure_server_with_options(paths, options)
}

fn runtime_for_local_restart(
    paths: &GlobalPaths,
    mut runtime: ServerRuntime,
) -> anyhow::Result<ServerRuntime> {
    if runtime.pid != 0 {
        return Ok(runtime);
    }
    if !runtime_base_url_is_localhost(&runtime.base_url) {
        return Err(remote_runtime_cannot_be_killed(&runtime));
    }
    let Some(identity_pid) = runtime_identity_pid(&runtime)? else {
        return Err(remote_runtime_cannot_be_killed(&runtime));
    };
    if identity_pid == 0 {
        return Err(remote_runtime_cannot_be_killed(&runtime));
    }
    match pid_matches_current_exe(identity_pid) {
        Ok(true) => {}
        Ok(false) | Err(_) => return Err(remote_runtime_cannot_be_killed(&runtime)),
    }

    runtime.pid = identity_pid;
    write_global_runtime_file(paths, &runtime)?;
    Ok(runtime)
}

fn remote_runtime_cannot_be_killed(runtime: &ServerRuntime) -> anyhow::Error {
    anyhow::anyhow!(
        "remote stateful server cannot be killed from this machine: {}; restart it on the host running that server or start a local stateful server",
        runtime.base_url
    )
}

pub fn server_start_options_from_runtime(
    runtime: &ServerRuntime,
) -> anyhow::Result<ServerStartOptions> {
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
    if !runtime_is_basic_healthy(runtime) {
        if runtime.pid == 0 {
            anyhow::bail!(
                "remote stateful runtime at {} is unreachable or unavailable; preserving runtime file and not starting a local server",
                runtime.base_url
            );
        }
        let _ = fs::remove_file(&paths.server_json);
        return Ok(());
    }
    if runtime.pid != 0
        && runtime_identity_matches_pid(runtime)?
        && pid_matches_current_exe(runtime.pid)?
    {
        return terminate_runtime(paths, runtime);
    }

    anyhow::bail!(
        "existing stateful server pid {} is reachable but does not support required runtime capabilities; stop it with the matching stateful binary and retry",
        runtime.pid
    )
}

fn terminate_runtime(paths: &GlobalPaths, runtime: &ServerRuntime) -> anyhow::Result<()> {
    let status = Command::new("kill").arg(runtime.pid.to_string()).status()?;
    if !status.success() {
        anyhow::bail!("failed to stop stateful server pid {}", runtime.pid);
    }
    for _ in 0..20 {
        if !runtime_is_basic_healthy(runtime) {
            break;
        }
        thread::sleep(Duration::from_millis(50));
    }
    if runtime_is_basic_healthy(runtime) {
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
        .stdin(Stdio::null())
        .stdout(Stdio::from(log))
        .stderr(Stdio::from(log_err))
        .spawn()?;

    for _ in 0..20 {
        thread::sleep(Duration::from_millis(50));
        if let Some(runtime) = read_runtime_file(paths)? {
            if runtime_is_healthy(&runtime) {
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

    if let Some(token) = &options.token {
        args.push("--token".to_string());
        args.push(token.clone());
    }

    args.extend(["--workspace-id".to_string(), options.workspace_id.clone()]);
    args
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
            Ok(runtime) => Ok(Some(runtime)),
            Err(_) => Ok(None),
        },
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
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
