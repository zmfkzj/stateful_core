use std::{
    fs,
    fs::OpenOptions,
    process::{Command, Stdio},
    thread,
    time::Duration,
};

use crate::{GlobalPaths, ServerRuntime, get_json, write_global_runtime_file};

pub fn ensure_server_with<H, S>(
    paths: &GlobalPaths,
    health: H,
    start: S,
) -> anyhow::Result<ServerRuntime>
where
    H: Fn(&ServerRuntime) -> bool,
    S: Fn() -> anyhow::Result<ServerRuntime>,
{
    if let Ok(contents) = fs::read_to_string(&paths.server_json) {
        let runtime: ServerRuntime = serde_json::from_str(&contents)?;
        if health(&runtime) {
            return Ok(runtime);
        }
    }

    let _lock = acquire_start_lock(paths)?;
    if let Ok(contents) = fs::read_to_string(&paths.server_json) {
        let runtime: ServerRuntime = serde_json::from_str(&contents)?;
        if health(&runtime) {
            return Ok(runtime);
        }
    }

    let runtime = start()?;
    write_global_runtime_file(paths, &runtime)?;
    Ok(runtime)
}

pub fn ensure_server(paths: &GlobalPaths) -> anyhow::Result<ServerRuntime> {
    ensure_server_with(
        paths,
        |runtime| get_json(runtime, "/health").is_ok(),
        || start_detached_server(paths),
    )
}

pub fn stop_server(paths: &GlobalPaths) -> anyhow::Result<()> {
    let contents = fs::read_to_string(&paths.server_json)?;
    let runtime: ServerRuntime = serde_json::from_str(&contents)?;
    if runtime.pid > 0 {
        let status = Command::new("kill").arg(runtime.pid.to_string()).status()?;
        if !status.success() {
            anyhow::bail!("failed to stop stateful server pid {}", runtime.pid);
        }
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
        if let Ok(contents) = fs::read_to_string(&paths.server_json) {
            let runtime: ServerRuntime = serde_json::from_str(&contents)?;
            if get_json(&runtime, "/health").is_ok() {
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
    let attempts = 20;
    let retry_delay = Duration::from_millis(10);

    for attempt in 0..attempts {
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
                if attempt + 1 == attempts {
                    anyhow::bail!(
                        "timed out acquiring stateful server start lock at {}",
                        paths.server_lock.display()
                    );
                }
                thread::sleep(retry_delay);
            }
            Err(error) => return Err(error.into()),
        }
    }

    anyhow::bail!(
        "timed out acquiring stateful server start lock at {}",
        paths.server_lock.display()
    )
}

struct StartLock {
    path: std::path::PathBuf,
}

impl Drop for StartLock {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}
