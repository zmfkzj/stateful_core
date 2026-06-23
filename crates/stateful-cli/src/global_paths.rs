use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GlobalPaths {
    pub home: PathBuf,
    pub config_yml: PathBuf,
    pub state_db: PathBuf,
    pub runtime_dir: PathBuf,
    pub server_json: PathBuf,
    pub server_lock: PathBuf,
    pub server_log: PathBuf,
    pub repos_dir: PathBuf,
    pub outbox_dir: PathBuf,
}

impl GlobalPaths {
    pub fn new(home: impl AsRef<Path>) -> Self {
        let home = home.as_ref().to_path_buf();
        let runtime_dir = home.join("runtime");
        Self {
            config_yml: home.join("config.yml"),
            state_db: home.join("state.db"),
            server_json: runtime_dir.join("server.json"),
            server_lock: runtime_dir.join("server.lock"),
            server_log: runtime_dir.join("server.log"),
            runtime_dir,
            repos_dir: home.join("repos"),
            outbox_dir: home.join("outbox"),
            home,
        }
    }

    pub fn from_env() -> anyhow::Result<Self> {
        if let Some(path) = std::env::var_os("STATEFUL_HOME") {
            if path.as_os_str().is_empty() {
                anyhow::bail!("STATEFUL_HOME is set but empty");
            }
            return Ok(Self::new(path));
        }
        let home = std::env::var_os("HOME")
            .ok_or_else(|| anyhow::anyhow!("HOME is not set; set STATEFUL_HOME"))?;
        if home.as_os_str().is_empty() {
            anyhow::bail!("HOME is set but empty; set STATEFUL_HOME");
        }
        Ok(Self::new(PathBuf::from(home).join(".stateful_core")))
    }
}
