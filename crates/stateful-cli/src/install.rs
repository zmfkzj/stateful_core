use std::{
    fs,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::Context;

use crate::{GlobalPaths, RepoRegistry};

const GLOBAL_CODEX_BLOCK_START: &str = "# stateful-core-global-install";
const GLOBAL_CODEX_BLOCK_END: &str = "# /stateful-core-global-install";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstallOptions {
    pub yes: bool,
    pub paths: GlobalPaths,
    pub codex_config_path: PathBuf,
    pub binary_path: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstallPlan {
    pub summary: String,
    pub files: Vec<PathBuf>,
}

pub fn plan_global_install(options: &InstallOptions) -> anyhow::Result<InstallPlan> {
    let mode = if options.yes { "apply" } else { "dry-run" };
    Ok(InstallPlan {
        summary: format!(
            "{mode}: install stateful global files under {} and merge Codex config {}",
            options.paths.home.display(),
            options.codex_config_path.display()
        ),
        files: vec![
            options.paths.home.clone(),
            options.paths.runtime_dir.clone(),
            options.paths.repos_dir.clone(),
            options.paths.config_yml.clone(),
            options.paths.state_db.clone(),
            options.codex_config_path.clone(),
        ],
    })
}

pub fn apply_global_install(options: InstallOptions) -> anyhow::Result<InstallPlan> {
    let mut plan = plan_global_install(&options)?;
    if !options.yes {
        return Ok(plan);
    }

    fs::create_dir_all(&options.paths.home).with_context(|| {
        format!(
            "failed to create stateful home {}",
            options.paths.home.display()
        )
    })?;
    fs::create_dir_all(&options.paths.runtime_dir).with_context(|| {
        format!(
            "failed to create stateful runtime directory {}",
            options.paths.runtime_dir.display()
        )
    })?;
    fs::create_dir_all(&options.paths.repos_dir).with_context(|| {
        format!(
            "failed to create stateful repos directory {}",
            options.paths.repos_dir.display()
        )
    })?;

    if !options.paths.config_yml.exists() {
        RepoRegistry::default().save(&options.paths)?;
    }

    let _store = stateful_store::Store::open(&options.paths.state_db).with_context(|| {
        format!(
            "failed to initialize state database {}",
            options.paths.state_db.display()
        )
    })?;

    merge_codex_config(&options.codex_config_path, &options.binary_path)?;
    plan.summary = format!(
        "apply: installed stateful global files under {} and merged Codex config {}",
        options.paths.home.display(),
        options.codex_config_path.display()
    );

    Ok(plan)
}

pub fn default_codex_config_path() -> anyhow::Result<PathBuf> {
    let home = std::env::var_os("HOME")
        .ok_or_else(|| anyhow::anyhow!("HOME is not set; pass --codex-config"))?;
    if home.is_empty() {
        anyhow::bail!("HOME is set but empty; pass --codex-config");
    }

    Ok(PathBuf::from(home).join(".codex").join("config.toml"))
}

pub fn current_stateful_binary_path() -> anyhow::Result<String> {
    let binary_path =
        std::env::current_exe().context("failed to resolve current executable path")?;
    let binary_path = binary_path.canonicalize().unwrap_or(binary_path);

    binary_path
        .to_str()
        .map(str::to_owned)
        .ok_or_else(|| anyhow::anyhow!("current executable path is not valid UTF-8"))
}

fn merge_codex_config(config_path: &Path, binary_path: &str) -> anyhow::Result<()> {
    let existing = if config_path.exists() {
        Some(fs::read_to_string(config_path).with_context(|| {
            format!(
                "failed to read existing Codex config {}",
                config_path.display()
            )
        })?)
    } else {
        None
    };
    let merged = append_stateful_block(
        existing.as_deref().unwrap_or_default(),
        &global_codex_config_block(binary_path),
    );

    if existing.as_deref() == Some(merged.as_str()) {
        return Ok(());
    }

    if let Some(parent) = config_path.parent() {
        fs::create_dir_all(parent).with_context(|| {
            format!("failed to create Codex config directory {}", parent.display())
        })?;
    }

    if let Some(existing) = existing {
        let backup_path = codex_backup_path(config_path)?;
        fs::write(&backup_path, existing).with_context(|| {
            format!(
                "failed to write Codex config backup {}",
                backup_path.display()
            )
        })?;
    }

    fs::write(config_path, merged).with_context(|| {
        format!(
            "failed to write merged Codex config {}",
            config_path.display()
        )
    })?;

    Ok(())
}

fn append_stateful_block(existing: &str, block: &str) -> String {
    let stripped = strip_stateful_block(existing);
    let mut merged = ensure_hooks_feature_enabled(&stripped);
    if !merged.is_empty() && !merged.ends_with("\n\n") {
        if merged.ends_with('\n') {
            merged.push('\n');
        } else {
            merged.push_str("\n\n");
        }
    }
    merged.push_str(block);
    if !merged.ends_with('\n') {
        merged.push('\n');
    }
    merged
}

fn ensure_hooks_feature_enabled(contents: &str) -> String {
    let mut lines = contents.lines().map(str::to_owned).collect::<Vec<_>>();
    let mut first_table_index = None;
    let mut features_table_end = None;
    let mut in_features_table = false;

    for index in 0..lines.len() {
        let trimmed = lines[index].trim();
        if toml_key_equals(trimmed, "features.hooks") {
            lines[index] = "features.hooks = true".to_string();
            return join_lines(lines, contents.ends_with('\n'));
        }
        if in_features_table && toml_key_equals(trimmed, "hooks") {
            lines[index] = "hooks = true".to_string();
            return join_lines(lines, contents.ends_with('\n'));
        }

        if is_toml_table_header(trimmed) {
            first_table_index.get_or_insert(index);
            if in_features_table {
                features_table_end = Some(index);
                in_features_table = false;
            }
            if trimmed == "[features]" {
                in_features_table = true;
                features_table_end = Some(lines.len());
            }
        }
    }

    if let Some(index) = features_table_end {
        lines.insert(index, "hooks = true".to_string());
    } else {
        let index = first_table_index.unwrap_or(lines.len());
        lines.insert(index, "features.hooks = true".to_string());
    }

    join_lines(lines, contents.ends_with('\n'))
}

fn toml_key_equals(line: &str, key: &str) -> bool {
    if line.starts_with('#') {
        return false;
    }
    line.split_once('=')
        .map(|(left, _right)| left.trim() == key)
        .unwrap_or(false)
}

fn is_toml_table_header(line: &str) -> bool {
    line.starts_with('[') && line.ends_with(']')
}

fn join_lines(lines: Vec<String>, had_trailing_newline: bool) -> String {
    let mut joined = lines.join("\n");
    if had_trailing_newline && !joined.is_empty() {
        joined.push('\n');
    }
    joined
}

fn strip_stateful_block(contents: &str) -> String {
    let mut lines = Vec::new();
    let mut in_stateful_block = false;

    for line in contents.lines() {
        let trimmed = line.trim();
        if trimmed == GLOBAL_CODEX_BLOCK_START {
            in_stateful_block = true;
            continue;
        }
        if in_stateful_block {
            if trimmed == GLOBAL_CODEX_BLOCK_END {
                in_stateful_block = false;
            }
            continue;
        }
        lines.push(line);
    }

    let mut stripped = lines.join("\n");
    if contents.ends_with('\n') && !stripped.is_empty() {
        stripped.push('\n');
    }
    stripped
}

fn codex_backup_path(config_path: &Path) -> anyhow::Result<PathBuf> {
    let parent = config_path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("Codex config path has no parent"))?;
    let file_name = config_path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| anyhow::anyhow!("Codex config file name is not valid UTF-8"))?;
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .context("system clock is before unix epoch")?
        .as_nanos();

    Ok(parent.join(format!(
        "{file_name}.stateful-backup-{}-{nonce}",
        std::process::id()
    )))
}

fn global_codex_config_block(binary_path: &str) -> String {
    let hook_prefix = format!("\"{binary_path}\" hook");

    format!(
        r#"{GLOBAL_CODEX_BLOCK_START}
[mcp_servers.stateful]
command = "{}"
args = ["mcp", "serve"]
startup_timeout_sec = 20

[[hooks.SessionStart]]
matcher = "startup|resume|clear|compact"

[[hooks.SessionStart.hooks]]
type = "command"
command = "{} session-start"
statusMessage = "Loading stateful current state"

[[hooks.UserPromptSubmit]]

[[hooks.UserPromptSubmit.hooks]]
type = "command"
command = "{} user-prompt-submit"
statusMessage = "Checking stateful intent context"

[[hooks.PreToolUse]]
matcher = "Bash|apply_patch|Edit|Write|mcp__filesystem__.*"

[[hooks.PreToolUse.hooks]]
type = "command"
command = "{} pre-tool-use"
statusMessage = "Authorizing stateful tool use"

[[hooks.PostToolUse]]
matcher = "Bash|apply_patch|Edit|Write|mcp__filesystem__.*"

[[hooks.PostToolUse.hooks]]
type = "command"
command = "{} post-tool-use"
statusMessage = "Recording stateful activity"

[[hooks.Stop]]

[[hooks.Stop.hooks]]
type = "command"
command = "{} stop"
statusMessage = "Finalizing stateful activity"
{GLOBAL_CODEX_BLOCK_END}
"#,
        escape_toml_string(binary_path),
        escape_toml_string(&hook_prefix),
        escape_toml_string(&hook_prefix),
        escape_toml_string(&hook_prefix),
        escape_toml_string(&hook_prefix),
        escape_toml_string(&hook_prefix)
    )
}

fn escape_toml_string(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}
