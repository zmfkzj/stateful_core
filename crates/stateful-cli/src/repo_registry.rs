use std::{
    fs,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::Context;

use crate::{GlobalPaths, default_config_yml, default_validation_yml, install_repo_local};

#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct RepoRegistry {
    #[serde(default)]
    pub repos: Vec<RepoEntry>,
}

impl RepoRegistry {
    pub fn load(paths: &GlobalPaths) -> anyhow::Result<Self> {
        if !paths.config_yml.exists() {
            return Ok(Self::default());
        }

        let config = fs::read_to_string(&paths.config_yml).with_context(|| {
            format!(
                "failed to read repo registry config {}",
                paths.config_yml.display()
            )
        })?;
        if config.trim().is_empty() {
            return Ok(Self::default());
        }

        serde_yaml::from_str(&config).with_context(|| {
            format!(
                "failed to parse repo registry config {}",
                paths.config_yml.display()
            )
        })
    }

    pub fn save(&self, paths: &GlobalPaths) -> anyhow::Result<()> {
        if let Some(parent) = paths.config_yml.parent() {
            fs::create_dir_all(parent).with_context(|| {
                format!(
                    "failed to create global config directory {}",
                    parent.display()
                )
            })?;
        }

        let config = serde_yaml::to_string(self).context("failed to serialize repo registry")?;
        let temp_path = registry_temp_path(paths)?;
        fs::write(&temp_path, config).with_context(|| {
            format!("failed to write temporary registry {}", temp_path.display())
        })?;
        fs::rename(&temp_path, &paths.config_yml).with_context(|| {
            format!(
                "failed to replace repo registry config {}",
                paths.config_yml.display()
            )
        })?;

        Ok(())
    }

    pub fn is_enabled(&self, repo_root: impl AsRef<Path>) -> bool {
        let Ok(repo_root) = repo_root.as_ref().canonicalize() else {
            return false;
        };

        self.repos
            .iter()
            .any(|entry| entry.enabled && entry.root == repo_root)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct RepoEntry {
    pub repo_id: String,
    pub root: PathBuf,
    pub enabled: bool,
    pub enabled_at: String,
    pub validation_config_path: PathBuf,
    pub policy_config_path: PathBuf,
    pub codex_mode: CodexMode,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CodexMode {
    #[default]
    Global,
    RepoLocal,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RepoGate {
    Enabled { repo_root: PathBuf },
    Disabled,
    OutsideGitRepo,
}

pub fn repo_gate(paths: &GlobalPaths, start: impl AsRef<Path>) -> anyhow::Result<RepoGate> {
    let root = match detect_git_root(start) {
        Ok(root) => root,
        Err(_) => return Ok(RepoGate::OutsideGitRepo),
    };
    let registry = RepoRegistry::load(paths)?;
    if registry.is_enabled(&root) {
        Ok(RepoGate::Enabled { repo_root: root })
    } else {
        Ok(RepoGate::Disabled)
    }
}

pub fn enable_repo(
    paths: &GlobalPaths,
    repo: impl AsRef<Path>,
    repo_local_codex: bool,
) -> anyhow::Result<RepoEntry> {
    let root = detect_git_root(repo)?;
    if repo_local_codex {
        crate::ensure_repo_local_install_can_write(&root)?;
    }

    ensure_repo_configs(&root)?;
    if repo_local_codex {
        let policy_config = root.join(".stateful/config.yml");
        let validation_config = root.join(".stateful/validation.yml");
        let policy_contents = fs::read(&policy_config)
            .with_context(|| format!("failed to read {}", policy_config.display()))?;
        let validation_contents = fs::read(&validation_config)
            .with_context(|| format!("failed to read {}", validation_config.display()))?;
        let binary_path = current_stateful_binary_path()?;

        install_repo_local(&root, &binary_path)?;
        fs::write(&policy_config, policy_contents)
            .with_context(|| format!("failed to restore {}", policy_config.display()))?;
        fs::write(&validation_config, validation_contents)
            .with_context(|| format!("failed to restore {}", validation_config.display()))?;
    }

    let entry = RepoEntry {
        repo_id: repo_id_for_root(&root),
        root: root.clone(),
        enabled: true,
        enabled_at: current_unix_timestamp()?,
        validation_config_path: root.join(".stateful/validation.yml"),
        policy_config_path: root.join(".stateful/config.yml"),
        codex_mode: if repo_local_codex {
            CodexMode::RepoLocal
        } else {
            CodexMode::Global
        },
    };

    let mut registry = RepoRegistry::load(paths)?;
    if let Some(existing) = registry
        .repos
        .iter_mut()
        .find(|existing| existing.repo_id == entry.repo_id || existing.root == entry.root)
    {
        *existing = entry.clone();
    } else {
        registry.repos.push(entry.clone());
    }
    registry.save(paths)?;
    write_repo_metadata(paths, &entry)?;

    Ok(entry)
}

pub fn disable_repo(paths: &GlobalPaths, repo: impl AsRef<Path>) -> anyhow::Result<RepoEntry> {
    let root = detect_git_root(repo)?;
    let mut registry = RepoRegistry::load(paths)?;
    let entry = registry
        .repos
        .iter_mut()
        .find(|entry| entry.root == root)
        .ok_or_else(|| anyhow::anyhow!("repo is not registered: {}", root.display()))?;

    entry.enabled = false;
    let disabled = entry.clone();
    registry.save(paths)?;
    write_repo_metadata(paths, &disabled)?;

    Ok(disabled)
}

pub fn detect_git_root(start: impl AsRef<Path>) -> anyhow::Result<PathBuf> {
    let start = start.as_ref();
    let canonical_start = start
        .canonicalize()
        .with_context(|| format!("failed to canonicalize {}", start.display()))?;
    let mut current = if canonical_start.is_file() {
        canonical_start
            .parent()
            .map(Path::to_path_buf)
            .ok_or_else(|| anyhow::anyhow!("{} has no parent directory", start.display()))?
    } else {
        canonical_start
    };

    loop {
        if current.join(".git").exists() {
            return Ok(current);
        }
        if !current.pop() {
            anyhow::bail!("no git root found from {}", start.display());
        }
    }
}

fn ensure_repo_configs(root: &Path) -> anyhow::Result<()> {
    let stateful_dir = root.join(".stateful");
    fs::create_dir_all(&stateful_dir)
        .with_context(|| format!("failed to create {}", stateful_dir.display()))?;

    let policy_config = stateful_dir.join("config.yml");
    if !policy_config.exists() {
        fs::write(&policy_config, default_config_yml())
            .with_context(|| format!("failed to write {}", policy_config.display()))?;
    }

    let validation_config = stateful_dir.join("validation.yml");
    if !validation_config.exists() {
        fs::write(&validation_config, default_validation_yml())
            .with_context(|| format!("failed to write {}", validation_config.display()))?;
    }

    Ok(())
}

fn write_repo_metadata(paths: &GlobalPaths, entry: &RepoEntry) -> anyhow::Result<()> {
    fs::create_dir_all(&paths.repos_dir).with_context(|| {
        format!(
            "failed to create repo metadata directory {}",
            paths.repos_dir.display()
        )
    })?;
    let metadata_path = paths.repos_dir.join(format!("{}.json", entry.repo_id));
    let metadata =
        serde_json::to_string_pretty(entry).context("failed to serialize repo metadata")?;
    fs::write(&metadata_path, format!("{metadata}\n"))
        .with_context(|| format!("failed to write {}", metadata_path.display()))?;

    Ok(())
}

fn current_stateful_binary_path() -> anyhow::Result<String> {
    let binary_path =
        std::env::current_exe().context("failed to resolve current executable path")?;
    let binary_path = binary_path.canonicalize().unwrap_or(binary_path);

    if !binary_path.is_absolute() {
        anyhow::bail!(
            "current executable path is not absolute: {}",
            binary_path.display()
        );
    }

    binary_path
        .to_str()
        .map(str::to_owned)
        .ok_or_else(|| anyhow::anyhow!("current executable path is not valid UTF-8"))
}

fn current_unix_timestamp() -> anyhow::Result<String> {
    Ok(SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .context("system clock is before unix epoch")?
        .as_secs()
        .to_string())
}

fn registry_temp_path(paths: &GlobalPaths) -> anyhow::Result<PathBuf> {
    let parent = paths
        .config_yml
        .parent()
        .ok_or_else(|| anyhow::anyhow!("global config path has no parent"))?;
    let file_name = paths
        .config_yml
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| anyhow::anyhow!("global config file name is not valid UTF-8"))?;
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .context("system clock is before unix epoch")?
        .as_nanos();

    Ok(parent.join(format!(".{file_name}.{}.{}.tmp", std::process::id(), nonce)))
}

fn repo_id_for_root(root: &Path) -> String {
    let bytes = root.to_string_lossy();
    let first = fnv1a64(0xcbf29ce484222325, bytes.as_bytes());
    let second = fnv1a64(0x84222325cbf29ce4, bytes.as_bytes());
    format!("repo-{first:016x}{second:016x}")
}

fn fnv1a64(seed: u64, bytes: &[u8]) -> u64 {
    let mut hash = seed;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}
