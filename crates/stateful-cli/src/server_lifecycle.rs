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
    ensure_server_with(paths, runtime_is_healthy, || start_detached_server(paths))
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

fn start_detached_server(paths: &GlobalPaths) -> anyhow::Result<ServerRuntime> {
    fs::create_dir_all(&paths.runtime_dir)?;
    let log = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&paths.server_log)?;
    let log_err = log.try_clone()?;
    let child = Command::new(std::env::current_exe()?)
        .args(["server", "start", "--foreground"])
        .env("STATEFUL_HOME", &paths.home)
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
