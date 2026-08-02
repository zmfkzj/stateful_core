use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::Context;

use crate::{GlobalPaths, default_config_yml};

#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
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
        self.enabled_entry(repo_root).is_some()
    }

    pub fn enabled_entry(&self, repo_root: impl AsRef<Path>) -> Option<&RepoEntry> {
        let Ok(repo_root) = repo_root.as_ref().canonicalize() else {
            return None;
        };

        self.repos
            .iter()
            .find(|entry| entry.enabled && entry.root == repo_root)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RepoEntry {
    pub repo_id: String,
    pub root: PathBuf,
    pub enabled: bool,
    pub enabled_at: String,
    pub policy_config_path: PathBuf,
    #[serde(default)]
    pub policy_revision: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepoIdentity {
    pub repo_id: String,
    pub worktree_id: String,
    pub root: String,
    pub branch: String,
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

pub fn repo_identity_for_enabled_repo(
    paths: &GlobalPaths,
    repo_root: impl AsRef<Path>,
) -> anyhow::Result<RepoIdentity> {
    let repo_root = detect_git_root(repo_root)?;
    let registry = RepoRegistry::load(paths)?;
    let entry = registry
        .enabled_entry(&repo_root)
        .ok_or_else(|| anyhow::anyhow!("enabled repo metadata not found"))?;
    let branch = current_branch(&repo_root).unwrap_or_else(|| "unknown".to_string());

    Ok(RepoIdentity {
        repo_id: entry.repo_id.clone(),
        worktree_id: entry.repo_id.clone(),
        root: entry.root.to_string_lossy().into_owned(),
        branch,
    })
}

pub fn workspace_id_for_enabled_repo(
    paths: &GlobalPaths,
    repo_root: impl AsRef<Path>,
) -> anyhow::Result<String> {
    let repo_root = detect_git_root(repo_root)?;
    let registry = RepoRegistry::load(paths)?;
    let entry = registry
        .enabled_entry(&repo_root)
        .ok_or_else(|| anyhow::anyhow!("enabled repo metadata not found"))?;

    Ok(workspace_id_for_repo_id(&entry.repo_id))
}

pub fn enable_repo(paths: &GlobalPaths, repo: impl AsRef<Path>) -> anyhow::Result<RepoEntry> {
    let root = detect_git_root(repo)?;
    ensure_repo_configs(&root)?;
    let repo_id = repo_id_for_root(&root);
    let mut registry = RepoRegistry::load(paths)?;
    let policy_revision = registry
        .repos
        .iter()
        .find(|existing| existing.repo_id == repo_id || existing.root == root)
        .map_or(1, |existing| existing.policy_revision.saturating_add(1));

    let entry = RepoEntry {
        repo_id,
        root: root.clone(),
        enabled: true,
        enabled_at: current_unix_timestamp()?,
        policy_config_path: root.join(".stateful/config.yml"),
        policy_revision,
    };

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
    let digest = stateful_core::digest_bytes(root.to_string_lossy().as_bytes());
    format!("repo-{}", digest.value)
}

fn workspace_id_for_repo_id(repo_id: &str) -> String {
    let suffix = repo_id.strip_prefix("repo-").unwrap_or(repo_id);
    format!("workspace-{suffix}")
}

fn current_branch(repo_root: &Path) -> Option<String> {
    let output = Command::new("git")
        .args(["branch", "--show-current"])
        .current_dir(repo_root)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let branch = String::from_utf8(output.stdout).ok()?.trim().to_string();
    if branch.is_empty() {
        None
    } else {
        Some(branch)
    }
}
