use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::Context;

use crate::{GlobalPaths, default_config_yml};

const DEFAULT_ALLOWED_TOOLS: &[&str] = &[
    "multi_agent_v1spawn_agent",
    "multi_agent_v1wait_agent",
    "multi_agent_v1close_agent",
    "multi_agent_v1resume_agent",
    "mcp__openaiDeveloperDocs__fetch_openai_doc",
    "mcp__openaiDeveloperDocs__search_openai_docs",
    "multi_agent_v1send_input",
];

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
pub struct RepoEntry {
    pub repo_id: String,
    pub root: PathBuf,
    pub enabled: bool,
    pub enabled_at: String,
    pub policy_config_path: PathBuf,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub allowed_tools: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub unclassified_tools: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct RepoToolList {
    pub allowed_tools: Vec<String>,
    pub unclassified_tools: Vec<String>,
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

pub fn workspace_id_for_repo_identity(identity: &RepoIdentity) -> String {
    workspace_id_for_repo_id(&identity.worktree_id)
}

pub fn effective_workspace_id_for_repo(
    runtime_workspace_id: &str,
    identity: Option<&RepoIdentity>,
) -> String {
    if matches!(runtime_workspace_id, "local" | "shared" | "unknown")
        && let Some(identity) = identity
    {
        return workspace_id_for_repo_identity(identity);
    }

    runtime_workspace_id.to_string()
}

pub fn enable_repo(paths: &GlobalPaths, repo: impl AsRef<Path>) -> anyhow::Result<RepoEntry> {
    let root = detect_git_root(repo)?;
    ensure_repo_configs(&root)?;
    let repo_id = repo_id_for_root(&root);
    let mut registry = RepoRegistry::load(paths)?;
    let allowed_tools = default_allowed_tools_with_existing(
        registry
            .repos
            .iter()
            .find(|existing| existing.repo_id == repo_id || existing.root == root)
            .map(|existing| existing.allowed_tools.clone())
            .unwrap_or_default(),
    );
    let unclassified_tools = registry
        .repos
        .iter()
        .find(|existing| existing.repo_id == repo_id || existing.root == root)
        .map(|existing| existing.unclassified_tools.clone())
        .unwrap_or_default();

    let entry = RepoEntry {
        repo_id,
        root: root.clone(),
        enabled: true,
        enabled_at: current_unix_timestamp()?,
        policy_config_path: root.join(".stateful/config.yml"),
        allowed_tools,
        unclassified_tools,
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

pub fn allow_tool_for_repo(
    paths: &GlobalPaths,
    repo: impl AsRef<Path>,
    tool_name: &str,
) -> anyhow::Result<RepoEntry> {
    update_tool_allowlist(paths, repo, tool_name, ToolAllowlistUpdate::Allow)
}

pub fn deny_tool_for_repo(
    paths: &GlobalPaths,
    repo: impl AsRef<Path>,
    tool_name: &str,
) -> anyhow::Result<RepoEntry> {
    update_tool_allowlist(paths, repo, tool_name, ToolAllowlistUpdate::Deny)
}

pub fn allowed_tools_for_repo(
    paths: &GlobalPaths,
    repo: impl AsRef<Path>,
) -> anyhow::Result<Vec<String>> {
    Ok(tool_list_for_repo(paths, repo)?.allowed_tools)
}

pub fn tool_list_for_repo(
    paths: &GlobalPaths,
    repo: impl AsRef<Path>,
) -> anyhow::Result<RepoToolList> {
    let root = detect_git_root(repo)?;
    let registry = RepoRegistry::load(paths)?;
    let entry = registry
        .repos
        .iter()
        .find(|entry| entry.root == root)
        .ok_or_else(|| anyhow::anyhow!("repo is not registered: {}", root.display()))?;

    let unclassified_tools = entry
        .unclassified_tools
        .iter()
        .filter(|tool| {
            !entry
                .allowed_tools
                .iter()
                .any(|allowed_tool| allowed_tool == *tool)
        })
        .cloned()
        .collect();

    Ok(RepoToolList {
        allowed_tools: entry.allowed_tools.clone(),
        unclassified_tools,
    })
}

pub fn tool_allowed_for_enabled_repo(
    paths: &GlobalPaths,
    repo_root: impl AsRef<Path>,
    tool_name: &str,
) -> anyhow::Result<bool> {
    let root = detect_git_root(repo_root)?;
    let registry = RepoRegistry::load(paths)?;
    let Some(entry) = registry.enabled_entry(&root) else {
        return Ok(false);
    };
    Ok(entry
        .allowed_tools
        .iter()
        .any(|allowed| allowed == tool_name))
}

pub fn record_unclassified_tool_for_repo(
    paths: &GlobalPaths,
    repo: impl AsRef<Path>,
    tool_name: &str,
) -> anyhow::Result<RepoEntry> {
    let root = detect_git_root(repo)?;
    let tool_name = normalized_tool_name(tool_name)?;
    let mut registry = RepoRegistry::load(paths)?;
    let entry = registry
        .repos
        .iter_mut()
        .find(|entry| entry.root == root)
        .ok_or_else(|| anyhow::anyhow!("repo is not registered: {}", root.display()))?;

    if !entry
        .allowed_tools
        .iter()
        .any(|allowed| allowed == &tool_name)
        && !entry
            .unclassified_tools
            .iter()
            .any(|unclassified| unclassified == &tool_name)
    {
        entry.unclassified_tools.push(tool_name);
    }

    let updated = entry.clone();
    registry.save(paths)?;
    write_repo_metadata(paths, &updated)?;

    Ok(updated)
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ToolAllowlistUpdate {
    Allow,
    Deny,
}

fn update_tool_allowlist(
    paths: &GlobalPaths,
    repo: impl AsRef<Path>,
    tool_name: &str,
    update: ToolAllowlistUpdate,
) -> anyhow::Result<RepoEntry> {
    let root = detect_git_root(repo)?;
    let tool_name = normalized_tool_name(tool_name)?;
    let mut registry = RepoRegistry::load(paths)?;
    let entry = registry
        .repos
        .iter_mut()
        .find(|entry| entry.root == root)
        .ok_or_else(|| anyhow::anyhow!("repo is not registered: {}", root.display()))?;

    match update {
        ToolAllowlistUpdate::Allow => {
            if !entry
                .allowed_tools
                .iter()
                .any(|allowed| allowed == &tool_name)
            {
                entry.allowed_tools.push(tool_name.clone());
            }
            entry
                .unclassified_tools
                .retain(|unclassified| unclassified != &tool_name);
        }
        ToolAllowlistUpdate::Deny => {
            entry.allowed_tools.retain(|allowed| allowed != &tool_name);
        }
    }

    let updated = entry.clone();
    registry.save(paths)?;
    write_repo_metadata(paths, &updated)?;

    Ok(updated)
}

fn default_allowed_tools_with_existing(existing_tools: Vec<String>) -> Vec<String> {
    let mut allowed_tools: Vec<String> = DEFAULT_ALLOWED_TOOLS
        .iter()
        .map(|tool| (*tool).to_string())
        .collect();
    for tool in existing_tools {
        if !allowed_tools.iter().any(|allowed| allowed == &tool) {
            allowed_tools.push(tool);
        }
    }
    allowed_tools
}

fn normalized_tool_name(tool_name: &str) -> anyhow::Result<String> {
    let tool_name = tool_name.trim();
    if tool_name.is_empty() {
        anyhow::bail!("tool name must not be empty");
    }
    if tool_name.chars().any(char::is_control) {
        anyhow::bail!("tool name must not contain control characters");
    }

    Ok(tool_name.to_string())
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
    let bytes = root.to_string_lossy();
    let first = fnv1a64(0xcbf29ce484222325, bytes.as_bytes());
    let second = fnv1a64(0x84222325cbf29ce4, bytes.as_bytes());
    format!("repo-{first:016x}{second:016x}")
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

fn fnv1a64(seed: u64, bytes: &[u8]) -> u64 {
    let mut hash = seed;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}
