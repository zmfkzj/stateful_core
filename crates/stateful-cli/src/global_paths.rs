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
}

impl GlobalPaths {
    pub fn new(home: impl AsRef<Path>) -> Self {
        let home = home.as_ref().to_path_buf();
        Self {
            config_yml: home.join("config.yml"),
            state_db: home.join("state.db"),
            runtime_dir: home.join("runtime"),
            server_json: home.join("runtime").join("server.json"),
            server_lock: home.join("runtime").join("server.lock"),
            server_log: home.join("runtime").join("server.log"),
            repos_dir: home.join("repos"),
            home,
        }
    }

    pub fn from_env() -> anyhow::Result<Self> {
        if let Some(path) = std::env::var_os("STATEFUL_HOME") {
            return Ok(Self::new(path));
        }
        let home = std::env::var_os("HOME")
            .ok_or_else(|| anyhow::anyhow!("HOME is not set; set STATEFUL_HOME"))?;
        Ok(Self::new(PathBuf::from(home).join(".stateful_core")))
    }
}
