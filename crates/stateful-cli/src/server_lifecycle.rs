use std::{
    fs,
    fs::OpenOptions,
    path::Path,
    process::{Command, Stdio},
    thread,
    time::{Duration, SystemTime},
};

use crate::{GlobalPaths, ServerRuntime, get_json, write_global_runtime_file};

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
    let options = ServerStartOptions::default();
    ensure_server_with(paths, runtime_is_healthy, || {
        start_detached_server(paths, &options)
    })
}

pub fn ensure_server_with_options(
    paths: &GlobalPaths,
    options: ServerStartOptions,
) -> anyhow::Result<ServerRuntime> {
    if let Some(runtime) = read_runtime_file(paths)? {
        if runtime_is_healthy(&runtime) {
            ensure_runtime_matches_options(&runtime, &options)?;
            return Ok(runtime);
        }
    }

    let _lock = acquire_start_lock(paths)?;
    if let Some(runtime) = read_runtime_file(paths)? {
        if runtime_is_healthy(&runtime) {
            ensure_runtime_matches_options(&runtime, &options)?;
            return Ok(runtime);
        }
    }

    let runtime = start_detached_server(paths, &options)?;
    write_global_runtime_file(paths, &runtime)?;
    Ok(runtime)
}

pub fn runtime_is_healthy(runtime: &ServerRuntime) -> bool {
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
        || !runtime_is_healthy(&runtime)
        || !runtime_identity_matches_pid(&runtime)?
        || !pid_matches_current_exe(runtime.pid)?
    {
        anyhow::bail!(
            "refusing to stop unverified stateful server pid {}",
            runtime.pid
        );
    }

    let status = Command::new("kill").arg(runtime.pid.to_string()).status()?;
    if !status.success() {
        anyhow::bail!("failed to stop stateful server pid {}", runtime.pid);
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
    let log_err = log.try_clone()?;
    let mut command = Command::new(std::env::current_exe()?);
    configure_detached_command(&mut command);
    let child = command
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
    }

    anyhow::bail!(
        "stateful server did not become healthy after starting pid {}",
        child.id()
    )
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
        "existing stateful server runtime does not match requested server options: existing {} workspace {} pid {}, requested {} workspace {}",
        runtime.base_url,
        runtime.workspace_id,
        runtime.pid,
        expected_base_url,
        options.workspace_id
    )
}

fn runtime_identity_matches_pid(runtime: &ServerRuntime) -> anyhow::Result<bool> {
    let response = get_json(runtime, "/v1/runtime/identity")?;
    if response.status_code != 200 {
        return Ok(false);
    }

    let identity: RuntimeIdentity = serde_json::from_str(&response.body)?;
    Ok(identity.status == "ok"
        && identity.protocol_version == runtime.protocol_version
        && identity.pid == runtime.pid)
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

#[derive(Debug, serde::Deserialize)]
struct RuntimeIdentity {
    status: String,
    pid: u32,
    protocol_version: String,
}

struct StartLock {
    path: std::path::PathBuf,
}

impl Drop for StartLock {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}
