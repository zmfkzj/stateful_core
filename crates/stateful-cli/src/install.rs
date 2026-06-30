use std::{
    fs,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

#[cfg(unix)]
use std::{
    fs::OpenOptions,
    io::Write,
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
};

use anyhow::Context;

use crate::{GlobalPaths, RepoRegistry};

const GLOBAL_CODEX_BLOCK_START: &str = "# stateful-core-global-install";
const GLOBAL_CODEX_BLOCK_END: &str = "# /stateful-core-global-install";
const STATEFUL_APPROVAL_POLICY: &str = "approval_policy = { granular = { sandbox_approval = false, rules = true, mcp_elicitations = false, request_permissions = false, skill_approval = false } }";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstallOptions {
    pub yes: bool,
    pub paths: GlobalPaths,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodexInstallOptions {
    pub yes: bool,
    pub paths: GlobalPaths,
    pub codex_config_path: PathBuf,
    pub binary_path: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OmpInstallOptions {
    pub yes: bool,
    pub paths: GlobalPaths,
    pub binary_path: String,
    pub project_config_path: Option<PathBuf>,
    pub omp_agent_dir: Option<PathBuf>,
    pub update: bool,
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
            "{mode}: install stateful global files under {}",
            options.paths.home.display()
        ),
        files: vec![
            options.paths.home.clone(),
            options.paths.runtime_dir.clone(),
            options.paths.repos_dir.clone(),
            options.paths.config_yml.clone(),
            options.paths.state_db.clone(),
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

    initialize_state_database_if_missing(&options.paths.state_db)?;

    plan.summary = format!(
        "apply: installed stateful global files under {}",
        options.paths.home.display()
    );

    Ok(plan)
}

fn initialize_state_database_if_missing(path: &Path) -> anyhow::Result<()> {
    match path.metadata() {
        Ok(metadata) if metadata.is_file() => return Ok(()),
        Ok(_) => anyhow::bail!(
            "state database path exists but is not a file: {}",
            path.display()
        ),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(error)
                .with_context(|| format!("failed to inspect state database {}", path.display()));
        }
    }

    let _store = stateful_store::Store::open(path)
        .with_context(|| format!("failed to initialize state database {}", path.display()))?;
    Ok(())
}

pub fn plan_codex_install(options: &CodexInstallOptions) -> anyhow::Result<InstallPlan> {
    let mode = if options.yes { "apply" } else { "dry-run" };
    let mut plan = plan_global_install(&InstallOptions {
        yes: options.yes,
        paths: options.paths.clone(),
    })?;
    plan.summary = format!(
        "{mode}: install stateful global files under {} and merge Codex config {}",
        options.paths.home.display(),
        options.codex_config_path.display()
    );
    plan.files.push(options.codex_config_path.clone());
    plan.files
        .push(global_command_policy_skill_path(&options.codex_config_path));
    plan.files.extend(global_command_policy_support_file_paths(
        &options.codex_config_path,
    ));
    plan.files
        .push(global_dispatching_parallel_agents_skill_path(
            &options.codex_config_path,
        ));
    plan.files.push(global_external_sandbox_rules_path(
        &options.codex_config_path,
    ));
    Ok(plan)
}

pub fn apply_codex_install(options: CodexInstallOptions) -> anyhow::Result<InstallPlan> {
    let mut plan = plan_codex_install(&options)?;
    if !options.yes {
        return Ok(plan);
    }
    let codex_update = plan_codex_config_update(&options.codex_config_path, &options.binary_path)?;

    apply_global_install(InstallOptions {
        yes: true,
        paths: options.paths.clone(),
    })?;
    write_codex_config_update(&options.codex_config_path, codex_update)?;
    write_global_command_policy_skill(&options.codex_config_path)?;
    write_global_command_policy_support_files(&options.codex_config_path)?;
    write_global_dispatching_parallel_agents_skill(&options.codex_config_path)?;
    write_global_external_sandbox_rules(&options.codex_config_path, &options.binary_path)?;
    plan.summary = format!(
        "apply: installed stateful global files under {} and merged Codex config {}",
        options.paths.home.display(),
        options.codex_config_path.display()
    );

    Ok(plan)
}

pub fn plan_omp_install(options: &OmpInstallOptions) -> anyhow::Result<InstallPlan> {
    let mode = if options.yes { "apply" } else { "dry-run" };
    let mut plan = plan_global_install(&InstallOptions {
        yes: options.yes,
        paths: options.paths.clone(),
    })?;
    let agent_dir = omp_agent_dir(options)?;
    let config_path = options
        .project_config_path
        .clone()
        .unwrap_or_else(|| agent_dir.join("config.yml"));
    let extension_path = agent_dir
        .join("extensions")
        .join("stateful-omp-extension.js");
    let command_policy_skill_path = omp_command_policy_skill_path(&agent_dir);
    let rule_path = omp_required_rule_path(&agent_dir);
    plan.summary = format!(
        "{mode}: install stateful files under {} and configure the OMP stateful profile under {}",
        options.paths.home.display(),
        agent_dir.display()
    );
    plan.files.push(config_path);
    plan.files.push(extension_path);
    plan.files.push(command_policy_skill_path);
    plan.files
        .extend(omp_command_policy_support_file_paths(&agent_dir));
    plan.files.push(rule_path);
    Ok(plan)
}

pub fn apply_omp_install(options: OmpInstallOptions) -> anyhow::Result<InstallPlan> {
    validate_no_control_chars(&options.binary_path)?;
    let mut plan = plan_omp_install(&options)?;
    if !options.yes {
        return Ok(plan);
    }

    apply_global_install(InstallOptions {
        yes: true,
        paths: options.paths.clone(),
    })?;
    let agent_dir = omp_agent_dir(&options)?;
    let config_path = options
        .project_config_path
        .clone()
        .unwrap_or_else(|| agent_dir.join("config.yml"));
    let extension_path = agent_dir
        .join("extensions")
        .join("stateful-omp-extension.js");
    fs::create_dir_all(
        extension_path
            .parent()
            .expect("extension path should have parent"),
    )?;
    if let Some(parent) = config_path.parent() {
        fs::create_dir_all(parent)?;
    }
    write_omp_config(&config_path, &extension_path, options.update)?;
    write_omp_extension(&extension_path, &options.binary_path)?;
    write_omp_command_policy_skill(&agent_dir)?;
    write_omp_command_policy_support_files(&agent_dir)?;
    write_omp_required_rule(&agent_dir)?;
    plan.summary = format!(
        "apply: installed stateful files under {} and configured the OMP stateful profile under {}",
        options.paths.home.display(),
        agent_dir.display()
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

enum CodexConfigUpdate {
    Unchanged,
    Write {
        existing: Option<String>,
        merged: String,
    },
}

fn plan_codex_config_update(
    config_path: &Path,
    binary_path: &str,
) -> anyhow::Result<CodexConfigUpdate> {
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
    let merged = merge_codex_config_contents(existing.as_deref().unwrap_or_default(), binary_path)?;

    if existing.as_deref() == Some(merged.as_str()) {
        return Ok(CodexConfigUpdate::Unchanged);
    }

    Ok(CodexConfigUpdate::Write { existing, merged })
}

fn write_codex_config_update(config_path: &Path, update: CodexConfigUpdate) -> anyhow::Result<()> {
    let CodexConfigUpdate::Write { existing, merged } = update else {
        return Ok(());
    };
    let existing_mode = file_mode(config_path)?;

    let parent = containing_dir(config_path);
    fs::create_dir_all(parent).with_context(|| {
        format!(
            "failed to create Codex config directory {}",
            parent.display()
        )
    })?;

    if let Some(existing) = existing.as_ref() {
        let backup_path = codex_backup_path(config_path)?;
        write_text_file_with_mode(&backup_path, existing, existing_mode).with_context(|| {
            format!(
                "failed to write Codex config backup {}",
                backup_path.display()
            )
        })?;
        set_file_mode(&backup_path, existing_mode)?;
    }

    let temp_path = codex_temp_path(config_path)?;
    write_text_file_with_mode(&temp_path, &merged, existing_mode).with_context(|| {
        format!(
            "failed to write temporary Codex config {}",
            temp_path.display()
        )
    })?;
    set_file_mode(&temp_path, existing_mode)?;
    fs::rename(&temp_path, config_path).with_context(|| {
        format!(
            "failed to write merged Codex config {}",
            config_path.display()
        )
    })?;

    Ok(())
}

fn write_global_command_policy_skill(codex_config_path: &Path) -> anyhow::Result<()> {
    let path = global_command_policy_skill_path(codex_config_path);
    let parent = containing_dir(&path);
    fs::create_dir_all(parent).with_context(|| {
        format!(
            "failed to create Codex skills directory {}",
            parent.display()
        )
    })?;
    fs::write(&path, stateful_command_policy_skill())
        .with_context(|| format!("failed to write {}", path.display()))
}

fn global_command_policy_skill_path(codex_config_path: &Path) -> PathBuf {
    containing_dir(codex_config_path)
        .join("skills")
        .join("stateful-command-policy")
        .join("SKILL.md")
}

fn global_command_policy_skill_dir(codex_config_path: &Path) -> PathBuf {
    containing_dir(codex_config_path)
        .join("skills")
        .join("stateful-command-policy")
}

fn global_command_policy_support_file_paths(codex_config_path: &Path) -> Vec<PathBuf> {
    command_policy_support_file_paths(&global_command_policy_skill_dir(codex_config_path))
}

fn write_global_command_policy_support_files(codex_config_path: &Path) -> anyhow::Result<()> {
    write_command_policy_support_files(&global_command_policy_skill_dir(codex_config_path))
}

fn write_omp_command_policy_skill(agent_dir: &Path) -> anyhow::Result<()> {
    let path = omp_command_policy_skill_path(agent_dir);
    let parent = containing_dir(&path);
    fs::create_dir_all(parent)
        .with_context(|| format!("failed to create OMP skills directory {}", parent.display()))?;
    fs::write(&path, stateful_command_policy_skill())
        .with_context(|| format!("failed to write {}", path.display()))
}

fn omp_command_policy_skill_path(agent_dir: &Path) -> PathBuf {
    agent_dir
        .join("skills")
        .join("stateful-command-policy")
        .join("SKILL.md")
}

fn omp_command_policy_support_file_paths(agent_dir: &Path) -> Vec<PathBuf> {
    command_policy_support_file_paths(&omp_command_policy_skill_dir(agent_dir))
}

fn write_omp_command_policy_support_files(agent_dir: &Path) -> anyhow::Result<()> {
    write_command_policy_support_files(&omp_command_policy_skill_dir(agent_dir))
}

fn omp_command_policy_skill_dir(agent_dir: &Path) -> PathBuf {
    agent_dir.join("skills").join("stateful-command-policy")
}

fn command_policy_support_file_paths(skill_dir: &Path) -> Vec<PathBuf> {
    stateful_command_policy_support_files()
        .iter()
        .map(|(name, _)| skill_dir.join(name))
        .collect()
}

fn write_command_policy_support_files(skill_dir: &Path) -> anyhow::Result<()> {
    fs::create_dir_all(skill_dir).with_context(|| {
        format!(
            "failed to create Stateful command policy skill directory {}",
            skill_dir.display()
        )
    })?;
    for (name, contents) in stateful_command_policy_support_files() {
        let path = skill_dir.join(name);
        fs::write(&path, contents)
            .with_context(|| format!("failed to write {}", path.display()))?;
    }
    Ok(())
}

fn write_global_dispatching_parallel_agents_skill(codex_config_path: &Path) -> anyhow::Result<()> {
    let path = global_dispatching_parallel_agents_skill_path(codex_config_path);
    let parent = containing_dir(&path);
    fs::create_dir_all(parent).with_context(|| {
        format!(
            "failed to create Codex dispatching skill directory {}",
            parent.display()
        )
    })?;
    fs::write(&path, dispatching_parallel_agents_skill())
        .with_context(|| format!("failed to write {}", path.display()))
}

fn global_dispatching_parallel_agents_skill_path(codex_config_path: &Path) -> PathBuf {
    containing_dir(codex_config_path)
        .join("skills")
        .join("dispatching-parallel-agents")
        .join("SKILL.md")
}

fn write_omp_required_rule(agent_dir: &Path) -> anyhow::Result<()> {
    let path = omp_required_rule_path(agent_dir);
    let parent = containing_dir(&path);
    fs::create_dir_all(parent)
        .with_context(|| format!("failed to create OMP rules directory {}", parent.display()))?;
    fs::write(&path, omp_stateful_required_rule())
        .with_context(|| format!("failed to write {}", path.display()))
}

fn omp_required_rule_path(agent_dir: &Path) -> PathBuf {
    agent_dir.join("rules").join("stateful-required.md")
}

fn write_global_external_sandbox_rules(
    codex_config_path: &Path,
    binary_path: &str,
) -> anyhow::Result<()> {
    let path = global_external_sandbox_rules_path(codex_config_path);
    let parent = containing_dir(&path);
    fs::create_dir_all(parent).with_context(|| {
        format!(
            "failed to create Codex rules directory {}",
            parent.display()
        )
    })?;
    fs::write(&path, sandbox_external_prompt_rules(binary_path)?)
        .with_context(|| format!("failed to write {}", path.display()))
}

fn global_external_sandbox_rules_path(codex_config_path: &Path) -> PathBuf {
    containing_dir(codex_config_path)
        .join("rules")
        .join("stateful.rules")
}

fn merge_codex_config_contents(existing: &str, binary_path: &str) -> anyhow::Result<String> {
    let stripped = strip_stateful_block(existing)?;
    ensure_no_unmarked_stateful_mcp(&stripped)?;
    let without_approval_policy = strip_top_level_approval_policy(&stripped)?;
    let feature_update = ensure_hooks_feature_enabled(&without_approval_policy)?;
    let block = global_codex_config_block(binary_path, feature_update.include_features_section)?;

    Ok(append_stateful_block(&feature_update.contents, &block))
}

fn append_stateful_block(existing: &str, block: &str) -> String {
    let mut merged = existing.to_string();
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

struct FeatureUpdate {
    contents: String,
    include_features_section: bool,
}

fn ensure_hooks_feature_enabled(contents: &str) -> anyhow::Result<FeatureUpdate> {
    let mut lines = contents.lines().map(str::to_owned).collect::<Vec<_>>();
    let mut section = TomlSection::TopLevel;
    let mut features_table_count = 0;
    let mut features_table_end = None;
    let mut feature_hooks_indices = Vec::new();

    for index in 0..lines.len() {
        let trimmed = lines[index].trim();
        if let Some(header) = toml_table_header(trimmed)? {
            if matches!(section, TomlSection::Features) {
                features_table_end = Some(index);
            }
            if header.simple_name() == Some("features") {
                features_table_count += 1;
                section = TomlSection::Features;
                features_table_end = Some(lines.len());
            } else {
                section = TomlSection::Other;
            }
            continue;
        }

        match section {
            TomlSection::TopLevel if toml_key_equals(trimmed, "features.hooks") => {
                feature_hooks_indices.push((index, FeatureHookLocation::TopLevelDotted));
            }
            TomlSection::Features if toml_key_equals(trimmed, "hooks") => {
                feature_hooks_indices.push((index, FeatureHookLocation::FeaturesTable));
            }
            _ => {}
        }
    }

    if features_table_count > 1 {
        anyhow::bail!("Codex config contains multiple [features] tables");
    }
    if feature_hooks_indices.len() > 1 {
        anyhow::bail!("Codex config contains multiple features hooks settings");
    }

    if let Some((index, location)) = feature_hooks_indices.first().copied() {
        lines[index] = match location {
            FeatureHookLocation::TopLevelDotted => "features.hooks = true".to_string(),
            FeatureHookLocation::FeaturesTable => "hooks = true".to_string(),
        };
        return Ok(FeatureUpdate {
            contents: join_lines(lines, contents.ends_with('\n')),
            include_features_section: false,
        });
    }

    if let Some(index) = features_table_end {
        lines.insert(index, "hooks = true".to_string());
        return Ok(FeatureUpdate {
            contents: join_lines(lines, contents.ends_with('\n')),
            include_features_section: false,
        });
    }

    Ok(FeatureUpdate {
        contents: contents.to_string(),
        include_features_section: true,
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TomlSection {
    TopLevel,
    Features,
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FeatureHookLocation {
    TopLevelDotted,
    FeaturesTable,
}

fn toml_key_equals(line: &str, key: &str) -> bool {
    if line.starts_with('#') {
        return false;
    }
    line.split_once('=')
        .map(|(left, _right)| left.trim() == key)
        .unwrap_or(false)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TomlTableHeader<'a> {
    Simple { name: &'a str },
    Other,
}

impl<'a> TomlTableHeader<'a> {
    fn simple_name(self) -> Option<&'a str> {
        match self {
            Self::Simple { name } => Some(name),
            Self::Other => None,
        }
    }
}

fn toml_table_header(line: &str) -> anyhow::Result<Option<TomlTableHeader<'_>>> {
    let line = line.trim();
    if !line.starts_with('[') {
        return Ok(None);
    }

    let is_array = line.starts_with("[[");
    let body_start = if is_array { 2 } else { 1 };
    let Some((body_end, tail_start)) = find_toml_table_header_end(line, is_array) else {
        if line.contains('"') || line.contains('\'') {
            anyhow::bail!(
                "unsupported Codex config quoted table header; stateful install supports only bare dotted table headers: {line}"
            );
        }
        if unsupported_header_may_affect_stateful(line) {
            anyhow::bail!(
                "unsupported Codex config table header could conflict with stateful install: {line}"
            );
        }
        return Ok(Some(TomlTableHeader::Other));
    };

    let body = &line[body_start..body_end];
    if body.contains('"') || body.contains('\'') {
        if quoted_header_may_affect_stateful(body) {
            anyhow::bail!(
                "unsupported Codex config quoted table header; stateful install supports only bare dotted table headers: {line}"
            );
        }
        return Ok(Some(TomlTableHeader::Other));
    }

    let tail = line[tail_start..].trim();
    let supported_tail = tail.is_empty() || tail.starts_with('#');
    if is_array || !supported_tail || !is_supported_simple_table_name(body) {
        if unsupported_header_may_affect_stateful(body) {
            anyhow::bail!(
                "unsupported Codex config table header could conflict with stateful install: {line}"
            );
        }
        return Ok(Some(TomlTableHeader::Other));
    }

    Ok(Some(TomlTableHeader::Simple { name: body }))
}

fn find_toml_table_header_end(line: &str, is_array: bool) -> Option<(usize, usize)> {
    let mut in_basic_string = false;
    let mut in_literal_string = false;
    let mut escaped = false;
    let body_start = if is_array { 2 } else { 1 };
    let mut chars = line.char_indices().peekable();

    while let Some((index, character)) = chars.next() {
        if index < body_start {
            continue;
        }

        if in_basic_string {
            if escaped {
                escaped = false;
            } else if character == '\\' {
                escaped = true;
            } else if character == '"' {
                in_basic_string = false;
            }
            continue;
        }
        if in_literal_string {
            if character == '\'' {
                in_literal_string = false;
            }
            continue;
        }

        match character {
            '"' => in_basic_string = true,
            '\'' => in_literal_string = true,
            ']' if is_array => {
                if chars
                    .peek()
                    .map(|(_next_index, next)| *next == ']')
                    .unwrap_or(false)
                {
                    let tail_start = index + 2;
                    return Some((index, tail_start));
                }
            }
            ']' => {
                let tail_start = index + 1;
                return Some((index, tail_start));
            }
            _ => {}
        }
    }

    None
}

fn is_supported_simple_table_name(name: &str) -> bool {
    if name.is_empty() || name.starts_with('.') || name.ends_with('.') || name.contains("..") {
        return false;
    }

    name.split('.').all(|segment| {
        !segment.is_empty()
            && segment.chars().all(|character| {
                character.is_ascii_alphanumeric() || character == '_' || character == '-'
            })
    })
}

fn unsupported_header_may_affect_stateful(header: &str) -> bool {
    let normalized: String = header
        .chars()
        .filter(|character| {
            !character.is_whitespace()
                && *character != '"'
                && *character != '\''
                && *character != '['
                && *character != ']'
        })
        .collect();

    normalized == "features" || is_stateful_mcp_table_name(&normalized)
}

fn quoted_header_may_affect_stateful(header: &str) -> bool {
    toml_table_key_segments(header)
        .map(|segments| segments == ["features"] || is_stateful_mcp_table_segments(&segments))
        .unwrap_or_else(|| unsupported_header_may_affect_stateful(header))
}

fn is_stateful_mcp_table_name(name: &str) -> bool {
    name == "mcp_servers.stateful" || name.starts_with("mcp_servers.stateful.")
}

fn is_stateful_mcp_table_segments(segments: &[String]) -> bool {
    segments.len() >= 2 && segments[0] == "mcp_servers" && segments[1] == "stateful"
}

fn toml_table_key_segments(header: &str) -> Option<Vec<String>> {
    let mut segments = Vec::new();
    let mut current = String::new();
    let mut chars = header.chars().peekable();

    while let Some(character) = chars.next() {
        match character {
            '"' => {
                current.push_str(&parse_basic_toml_string(&mut chars)?);
            }
            '\'' => {
                current.push_str(&parse_literal_toml_string(&mut chars)?);
            }
            '.' => {
                if current.is_empty() {
                    return None;
                }
                segments.push(std::mem::take(&mut current));
            }
            character if character.is_whitespace() => {}
            character => current.push(character),
        }
    }

    if current.is_empty() {
        return None;
    }
    segments.push(current);
    Some(segments)
}

fn parse_basic_toml_string<I>(chars: &mut std::iter::Peekable<I>) -> Option<String>
where
    I: Iterator<Item = char>,
{
    let mut output = String::new();
    while let Some(character) = chars.next() {
        match character {
            '"' => return Some(output),
            '\\' => output.push(parse_basic_toml_escape(chars)?),
            character => output.push(character),
        }
    }
    None
}

fn parse_basic_toml_escape<I>(chars: &mut std::iter::Peekable<I>) -> Option<char>
where
    I: Iterator<Item = char>,
{
    match chars.next()? {
        'b' => Some('\u{0008}'),
        't' => Some('\t'),
        'n' => Some('\n'),
        'f' => Some('\u{000c}'),
        'r' => Some('\r'),
        '"' => Some('"'),
        '\\' => Some('\\'),
        'u' => parse_toml_unicode_escape(chars, 4),
        'U' => parse_toml_unicode_escape(chars, 8),
        _ => None,
    }
}

fn parse_toml_unicode_escape<I>(chars: &mut std::iter::Peekable<I>, digits: usize) -> Option<char>
where
    I: Iterator<Item = char>,
{
    let mut value = 0_u32;
    for _ in 0..digits {
        value = (value << 4) + chars.next()?.to_digit(16)?;
    }
    char::from_u32(value)
}

fn parse_literal_toml_string<I>(chars: &mut std::iter::Peekable<I>) -> Option<String>
where
    I: Iterator<Item = char>,
{
    let mut output = String::new();
    for character in chars.by_ref() {
        if character == '\'' {
            return Some(output);
        }
        output.push(character);
    }
    None
}

fn join_lines(lines: Vec<String>, had_trailing_newline: bool) -> String {
    let mut joined = lines.join("\n");
    if had_trailing_newline && !joined.is_empty() {
        joined.push('\n');
    }
    joined
}

fn strip_stateful_block(contents: &str) -> anyhow::Result<String> {
    let mut lines = Vec::new();
    let mut in_stateful_block = false;

    for line in contents.lines() {
        let trimmed = line.trim();
        if trimmed == GLOBAL_CODEX_BLOCK_START {
            if in_stateful_block {
                anyhow::bail!("Codex config contains nested stateful install marker");
            }
            in_stateful_block = true;
            continue;
        }
        if in_stateful_block {
            if trimmed == GLOBAL_CODEX_BLOCK_END {
                in_stateful_block = false;
            }
            continue;
        }
        if trimmed == GLOBAL_CODEX_BLOCK_END {
            anyhow::bail!("Codex config contains stateful install end marker without start marker");
        }
        lines.push(line);
    }

    if in_stateful_block {
        anyhow::bail!("Codex config stateful install block is missing end marker");
    }

    let mut stripped = lines.join("\n");
    if contents.ends_with('\n') && !stripped.is_empty() {
        stripped.push('\n');
    }
    Ok(stripped)
}

fn strip_top_level_approval_policy(contents: &str) -> anyhow::Result<String> {
    let mut lines = Vec::new();
    let mut section = TomlSection::TopLevel;

    for line in contents.lines() {
        let trimmed = line.trim();
        if let Some(header) = toml_table_header(trimmed)? {
            section = if header.simple_name() == Some("features") {
                TomlSection::Features
            } else {
                TomlSection::Other
            };
            lines.push(line);
            continue;
        }
        if matches!(section, TomlSection::TopLevel) && toml_key_equals(trimmed, "approval_policy") {
            continue;
        }
        lines.push(line);
    }

    let mut stripped = lines.join("\n");
    if contents.ends_with('\n') && !stripped.is_empty() {
        stripped.push('\n');
    }
    Ok(stripped)
}

fn ensure_no_unmarked_stateful_mcp(contents: &str) -> anyhow::Result<()> {
    for line in contents.lines() {
        if toml_table_header(line.trim())?
            .and_then(TomlTableHeader::simple_name)
            .is_some_and(is_stateful_mcp_table_name)
        {
            anyhow::bail!(
                "Codex config already contains unmarked [mcp_servers.stateful] configuration; remove it or install into a config without that conflict"
            );
        }
    }

    Ok(())
}

fn codex_backup_path(config_path: &Path) -> anyhow::Result<PathBuf> {
    let file_name = config_path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| anyhow::anyhow!("Codex config file name is not valid UTF-8"))?;
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .context("system clock is before unix epoch")?
        .as_nanos();

    Ok(containing_dir(config_path).join(format!(
        "{file_name}.stateful-backup-{}-{nonce}",
        std::process::id()
    )))
}

fn codex_temp_path(config_path: &Path) -> anyhow::Result<PathBuf> {
    let file_name = config_path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| anyhow::anyhow!("Codex config file name is not valid UTF-8"))?;
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .context("system clock is before unix epoch")?
        .as_nanos();

    Ok(containing_dir(config_path).join(format!(
        ".{file_name}.stateful-tmp-{}-{nonce}",
        std::process::id()
    )))
}

fn containing_dir(path: &Path) -> &Path {
    match path.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent,
        _ => Path::new("."),
    }
}

#[cfg(unix)]
fn write_text_file_with_mode(path: &Path, contents: &str, mode: Option<u32>) -> anyhow::Result<()> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    if let Some(mode) = mode {
        options.mode(mode);
    }

    let mut file = options
        .open(path)
        .with_context(|| format!("failed to create {}", path.display()))?;
    file.write_all(contents.as_bytes())
        .with_context(|| format!("failed to write {}", path.display()))
}

#[cfg(not(unix))]
fn write_text_file_with_mode(path: &Path, contents: &str, _mode: Option<()>) -> anyhow::Result<()> {
    fs::write(path, contents).with_context(|| format!("failed to write {}", path.display()))
}

#[cfg(unix)]
fn private_file_mode() -> Option<u32> {
    Some(0o600)
}

#[cfg(not(unix))]
fn private_file_mode() -> Option<()> {
    None
}

#[cfg(unix)]
fn write_or_replace_text_file_with_mode(
    path: &Path,
    contents: &str,
    mode: Option<u32>,
) -> anyhow::Result<()> {
    let mut options = OpenOptions::new();
    options.write(true).create(true).truncate(true);
    if let Some(mode) = mode {
        options.mode(mode);
    }

    let mut file = options
        .open(path)
        .with_context(|| format!("failed to create {}", path.display()))?;
    file.write_all(contents.as_bytes())
        .with_context(|| format!("failed to write {}", path.display()))?;
    if let Some(mode) = mode {
        fs::set_permissions(path, fs::Permissions::from_mode(mode))
            .with_context(|| format!("failed to set permissions on {}", path.display()))?;
    }
    Ok(())
}

#[cfg(not(unix))]
fn write_or_replace_text_file_with_mode(
    path: &Path,
    contents: &str,
    _mode: Option<()>,
) -> anyhow::Result<()> {
    fs::write(path, contents).with_context(|| format!("failed to write {}", path.display()))
}

#[cfg(unix)]
fn file_mode(path: &Path) -> anyhow::Result<Option<u32>> {
    if !path.exists() {
        return Ok(None);
    }

    let mode = fs::metadata(path)
        .with_context(|| format!("failed to read file mode for {}", path.display()))?
        .permissions()
        .mode()
        & 0o7777;
    Ok(Some(mode))
}

#[cfg(not(unix))]
fn file_mode(_path: &Path) -> anyhow::Result<Option<()>> {
    Ok(None)
}

#[cfg(unix)]
fn set_file_mode(path: &Path, mode: Option<u32>) -> anyhow::Result<()> {
    let Some(mode) = mode else {
        return Ok(());
    };

    let mut permissions = fs::metadata(path)
        .with_context(|| format!("failed to read file mode for {}", path.display()))?
        .permissions();
    permissions.set_mode(mode);
    fs::set_permissions(path, permissions)
        .with_context(|| format!("failed to preserve file mode for {}", path.display()))
}

#[cfg(not(unix))]
fn set_file_mode(_path: &Path, _mode: Option<()>) -> anyhow::Result<()> {
    Ok(())
}

fn global_codex_config_block(
    binary_path: &str,
    include_features_section: bool,
) -> anyhow::Result<String> {
    let quoted_binary = shell_quote_posix(binary_path)?;
    let hook_prefix = format!("{quoted_binary} hook codex");
    let features_section = if include_features_section {
        "[features]\nhooks = true\n\n"
    } else {
        ""
    };

    Ok(format!(
        r#"{GLOBAL_CODEX_BLOCK_START}
{STATEFUL_APPROVAL_POLICY}

{features_section}[[hooks.SessionStart]]
matcher = "startup|resume|clear|compact"

[[hooks.SessionStart.hooks]]
type = "command"
command = {}
statusMessage = "Loading stateful current state"

[[hooks.UserPromptSubmit]]

[[hooks.UserPromptSubmit.hooks]]
type = "command"
command = {}
statusMessage = "Checking stateful reservation context"

[[hooks.PreToolUse]]
matcher = ".*"

[[hooks.PreToolUse.hooks]]
type = "command"
command = {}
statusMessage = "Authorizing stateful tool use"

[[hooks.PostToolUse]]
matcher = "Bash|apply_patch|Edit|Write|file_change|mcp__filesystem__.*"

[[hooks.PostToolUse.hooks]]
type = "command"
command = {}
statusMessage = "Recording stateful activity"

[[hooks.Stop]]

[[hooks.Stop.hooks]]
type = "command"
command = {}
statusMessage = "Finalizing stateful activity"
{GLOBAL_CODEX_BLOCK_END}
"#,
        toml_string(&format!("{hook_prefix} session-start")),
        toml_string(&format!("{hook_prefix} user-prompt-submit")),
        toml_string(&format!("{hook_prefix} pre-tool-use")),
        toml_string(&format!("{hook_prefix} post-tool-use")),
        toml_string(&format!("{hook_prefix} stop"))
    ))
}

fn sandbox_external_prompt_rules(binary_path: &str) -> anyhow::Result<String> {
    validate_no_control_chars(binary_path)?;
    let binary = toml_string(binary_path);
    let request_match = toml_string(&format!(
        "{binary_path} sandbox run --fs external --purpose 'install rebuilt binaries' --write-dir /opt/stateful/bin --command 'install -m 755 target/release/stateful /opt/stateful/bin/stateful'"
    ));
    Ok(format!(
        r#"{GLOBAL_CODEX_BLOCK_START}
prefix_rule(
    pattern = [{binary}, "sandbox", "run", "--fs", "external"],
    decision = "prompt",
    justification = "Require explicit approval before running stateful sandbox run --fs external.",
    match = [
        {request_match},
    ],
)
{GLOBAL_CODEX_BLOCK_END}
"#
    ))
}

fn omp_agent_dir(options: &OmpInstallOptions) -> anyhow::Result<PathBuf> {
    if let Some(agent_dir) = &options.omp_agent_dir {
        return Ok(agent_dir.clone());
    }

    default_omp_agent_dir()
}

fn default_omp_agent_dir() -> anyhow::Result<PathBuf> {
    let home = std::env::var_os("HOME")
        .ok_or_else(|| anyhow::anyhow!("HOME is not set; pass an OMP agent directory"))?;
    if home.is_empty() {
        anyhow::bail!("HOME is set but empty; pass an OMP agent directory");
    }

    Ok(default_omp_agent_dir_from_home(PathBuf::from(home)))
}

fn default_omp_agent_dir_from_home(home: impl AsRef<Path>) -> PathBuf {
    home.as_ref()
        .join(".omp")
        .join("profiles")
        .join("stateful")
        .join("agent")
}

fn write_or_create_text_file(config_path: &Path, contents: &str) -> anyhow::Result<()> {
    if config_path.exists() {
        fs::write(config_path, contents).with_context(|| {
            format!(
                "failed to write existing OMP config {}",
                config_path.display()
            )
        })
    } else {
        write_text_file_with_mode(config_path, contents, private_file_mode())
    }
}

fn write_omp_config(
    config_path: &Path,
    extension_path: &Path,
    update_existing: bool,
) -> anyhow::Result<()> {
    let extension = extension_path.to_string_lossy();
    let entry = format!("  - {extension}");
    let mut contents = if config_path.exists() {
        fs::read_to_string(config_path).with_context(|| {
            format!(
                "failed to read existing OMP config {}",
                config_path.display()
            )
        })?
    } else {
        String::new()
    };

    validate_omp_config_yml(config_path, &contents)?;
    contents = ensure_omp_extension(contents, &entry);
    contents = ensure_omp_required_config(contents, update_existing)?;
    validate_omp_config_yml(config_path, &contents)?;
    write_or_create_text_file(config_path, &contents)
}

fn validate_omp_config_yml(config_path: &Path, contents: &str) -> anyhow::Result<()> {
    if contents.trim().is_empty() {
        return Ok(());
    }

    let value: serde_yaml::Value = serde_yaml::from_str(contents)
        .with_context(|| format!("invalid OMP config YAML {}", config_path.display()))?;
    if !matches!(
        value,
        serde_yaml::Value::Mapping(_) | serde_yaml::Value::Null
    ) {
        anyhow::bail!(
            "OMP config {} must be a YAML mapping",
            config_path.display()
        );
    }

    Ok(())
}

fn ensure_omp_extension(mut contents: String, entry: &str) -> String {
    if contents.lines().any(|line| line.trim() == entry.trim()) {
        return contents;
    }

    if let Some(offset) = contents
        .lines()
        .position(|line| line.trim() == "extensions:")
    {
        let mut lines: Vec<String> = contents.lines().map(ToString::to_string).collect();
        lines.insert(offset + 1, entry.to_string());
        contents = lines.join("\n");
        contents.push('\n');
    } else {
        if !contents.is_empty() && !contents.ends_with('\n') {
            contents.push('\n');
        }
        contents.push_str("extensions:\n");
        contents.push_str(entry);
        contents.push('\n');
    }

    contents
}

fn ensure_omp_required_config(contents: String, update_existing: bool) -> anyhow::Result<String> {
    let mut lines: Vec<String> = contents.lines().map(ToString::to_string).collect();

    ensure_omp_child_scalar(&mut lines, "tools", "approvalMode", "yolo", update_existing)?;
    remove_omp_child_mapping(&mut lines, "tools", "approval")?;
    ensure_omp_child_scalar(
        &mut lines,
        "stateful",
        "autoApprove",
        "false",
        update_existing,
    )?;
    ensure_omp_child_scalar(&mut lines, "eval", "py", "false", update_existing)?;
    ensure_omp_child_scalar(&mut lines, "eval", "js", "false", update_existing)?;
    ensure_omp_child_scalar(&mut lines, "eval", "rb", "false", update_existing)?;
    ensure_omp_child_scalar(&mut lines, "eval", "jl", "false", update_existing)?;
    ensure_omp_child_scalar(&mut lines, "bash", "enabled", "true", true)?;

    Ok(finish_omp_yaml_lines(lines))
}

fn ensure_omp_child_scalar(
    lines: &mut Vec<String>,
    section: &str,
    key: &str,
    value: &str,
    update_existing: bool,
) -> anyhow::Result<()> {
    let (section_offset, section_end) = ensure_omp_top_level_mapping(lines, section)?;
    if let Some(offset) = find_omp_yaml_key(lines, section_offset + 1, section_end, 2, key) {
        if update_existing {
            lines[offset] = format!("  {key}: {value}");
        }
    } else {
        lines.insert(section_end, format!("  {key}: {value}"));
    }

    Ok(())
}

fn remove_omp_child_mapping(
    lines: &mut Vec<String>,
    section: &str,
    child: &str,
) -> anyhow::Result<()> {
    let Some(section_offset) = find_omp_top_level_mapping(lines, section)? else {
        return Ok(());
    };
    let section_end = find_omp_yaml_mapping_end(lines, section_offset, 0, lines.len());
    if let Some(child_offset) = find_omp_yaml_key(lines, section_offset + 1, section_end, 2, child)
    {
        if !omp_yaml_key_is_block_mapping(&lines[child_offset], 2, child) {
            anyhow::bail!("OMP config `{section}.{child}` must be a block mapping");
        }
        let child_end = find_omp_yaml_mapping_end(lines, child_offset, 2, section_end);
        lines.drain(child_offset..child_end);
    }

    Ok(())
}

fn ensure_omp_top_level_mapping(
    lines: &mut Vec<String>,
    section: &str,
) -> anyhow::Result<(usize, usize)> {
    if let Some(offset) = find_omp_top_level_mapping(lines, section)? {
        let end = find_omp_yaml_mapping_end(lines, offset, 0, lines.len());
        return Ok((offset, end));
    }

    lines.push(format!("{section}:"));
    let offset = lines.len() - 1;
    Ok((offset, lines.len()))
}

fn find_omp_top_level_mapping(lines: &[String], section: &str) -> anyhow::Result<Option<usize>> {
    for (offset, line) in lines.iter().enumerate() {
        if omp_yaml_line_is_blank_or_comment(line) || omp_yaml_indent(line) != 0 {
            continue;
        }
        if !omp_yaml_key_matches(line, 0, section) {
            continue;
        }
        if !omp_yaml_key_is_block_mapping(line, 0, section) {
            anyhow::bail!("OMP config top-level `{section}` must be a block mapping");
        }
        return Ok(Some(offset));
    }

    Ok(None)
}

fn find_omp_yaml_key(
    lines: &[String],
    start: usize,
    end: usize,
    indent: usize,
    key: &str,
) -> Option<usize> {
    lines[start..end]
        .iter()
        .position(|line| {
            !omp_yaml_line_is_blank_or_comment(line)
                && omp_yaml_indent(line) == indent
                && omp_yaml_key_matches(line, indent, key)
        })
        .map(|offset| start + offset)
}

fn find_omp_yaml_mapping_end(lines: &[String], start: usize, indent: usize, limit: usize) -> usize {
    lines[start + 1..limit]
        .iter()
        .position(|line| {
            !omp_yaml_line_is_blank_or_comment(line) && omp_yaml_indent(line) <= indent
        })
        .map_or(limit, |offset| start + 1 + offset)
}

fn omp_yaml_key_matches(line: &str, indent: usize, key: &str) -> bool {
    let prefix = format!("{key}:");
    line.chars()
        .take_while(|character| character.is_whitespace())
        .count()
        == indent
        && line.trim_start().starts_with(&prefix)
}

fn omp_yaml_key_is_block_mapping(line: &str, indent: usize, key: &str) -> bool {
    let prefix = format!("{key}:");
    if !omp_yaml_key_matches(line, indent, key) {
        return false;
    }
    let tail = line.trim_start()[prefix.len()..].trim_start();
    tail.is_empty() || tail.starts_with('#')
}

fn omp_yaml_indent(line: &str) -> usize {
    line.chars()
        .take_while(|character| character.is_whitespace())
        .count()
}

fn omp_yaml_line_is_blank_or_comment(line: &str) -> bool {
    let trimmed = line.trim_start();
    trimmed.is_empty() || trimmed.starts_with('#')
}

fn finish_omp_yaml_lines(lines: Vec<String>) -> String {
    let mut contents = lines.join("\n");
    contents.push('\n');
    contents
}

fn write_omp_extension(extension_path: &Path, binary_path: &str) -> anyhow::Result<()> {
    let binary_json = serde_json::to_string(binary_path)?;
    let contents = format!(
        r#"import {{ spawnSync }} from "node:child_process";
import {{ createHash }} from "node:crypto";
import {{ existsSync, mkdirSync, readFileSync, statSync, writeFileSync }} from "node:fs";
import {{ delimiter, dirname, resolve }} from "node:path";
import {{ fileURLToPath }} from "node:url";

const STATEFUL = {binary_json};
const EXTENSION_DIR = dirname(fileURLToPath(import.meta.url));
const OMP_AGENT_CONFIG = resolve(EXTENSION_DIR, "..", "config.yml");
const BENCHMARK_SOURCE_BLOCK_ENV = "STATEFUL_BENCHMARK_SOURCE_BLOCK_PATTERNS";
 
let verifiedBareStatefulPath = null;

function statefulBinaryDigest(path) {{
  return createHash("sha256").update(readFileSync(path)).digest("hex");
}}

function executableFile(path) {{
  try {{
    const stat = statSync(path);
    return stat.isFile() && (stat.mode & 0o111) !== 0;
  }} catch {{
    return false;
  }}
}}

function firstPathStateful(cwd) {{
  const base = cwd || process.cwd();
  for (const entry of String(process.env.PATH || "").split(delimiter)) {{
    const directory = entry ? resolve(base, entry) : base;
    const candidate = resolve(directory, "stateful");
    if (executableFile(candidate)) return candidate;
  }}
  return null;
}}

function verifyBareStateful(cwd) {{
  verifiedBareStatefulPath = null;
  const candidate = firstPathStateful(cwd);
  if (!candidate) return false;
  try {{
    if (statefulBinaryDigest(candidate) !== statefulBinaryDigest(STATEFUL)) return false;
    verifiedBareStatefulPath = candidate;
    return true;
  }} catch {{
    verifiedBareStatefulPath = null;
    return false;
  }}
}}

function bareStatefulStillVerified() {{
  if (!verifiedBareStatefulPath) return false;
  try {{
    return statefulBinaryDigest(verifiedBareStatefulPath) === statefulBinaryDigest(STATEFUL);
  }} catch {{
    return false;
  }}
}}

function isTrustedStatefulCommand(word) {{
  if (word === STATEFUL) return true;
  if (word !== "stateful") return false;
  return bareStatefulStillVerified();
}}


function runStatefulHook(event, payload) {{
  const result = spawnSync(STATEFUL, ["hook", "omp", event], {{
    input: JSON.stringify(payload),
    encoding: "utf8",
  }});
  if (result.status !== 0) {{
    return {{ decision: "block", reason: result.stderr || "stateful hook failed" }};
  }}
  const text = (result.stdout || "").trim();
  return text ? JSON.parse(text) : {{ decision: "allow" }};
}}

function isYolo(event, ctx) {{
  const values = [
    event?.yolo,
    event?.autoApprove,
    event?.approvalMode,
    ctx?.yolo,
    ctx?.autoApprove,
    ctx?.approvalMode,
    ctx?.config?.approvalMode,
    ctx?.config?.tools?.approvalMode,
  ];
  return values.some((value) => value === true || value === "yolo" || value === "auto-approve");
}}

function firstString(...values) {{
  for (const value of values) {{
    if (typeof value === "string" && value.trim().length > 0) return value;
  }}
  return undefined;
}}

function agentIdFromString(value) {{
  if (typeof value !== "string") return undefined;
  const id = value.trim();
  if (!id) return undefined;
  if (!/^[A-Za-z0-9_-]+$/.test(id)) return undefined;
  return id;
}}

function workspaceIdFromString(value) {{
  if (typeof value !== "string") return undefined;
  const id = value.trim();
  return id.length > 0 ? id : undefined;
}}

function agentIdFromOmpSessionValue(value) {{
  const id = agentIdFromString(value);
  if (!id) return undefined;
  return id.startsWith("omp-") ? id : "omp-" + id;
}}


function detectOmpSessionAgentId(event, ctx) {{
  return firstString(
    agentIdFromOmpSessionValue(event?.sessionId),
    agentIdFromOmpSessionValue(event?.session_id),
    agentIdFromOmpSessionValue(event?.session?.id),
    agentIdFromOmpSessionValue(event?.session?.sessionId),
    agentIdFromOmpSessionValue(event?.session?.session_id),
    agentIdFromOmpSessionValue(ctx?.sessionId),
    agentIdFromOmpSessionValue(ctx?.session_id),
    agentIdFromOmpSessionValue(ctx?.session?.id),
    agentIdFromOmpSessionValue(ctx?.session?.sessionId),
    agentIdFromOmpSessionValue(ctx?.session?.session_id),
    agentIdFromOmpSessionValue(ctx?.runtime?.sessionId),
    agentIdFromOmpSessionValue(ctx?.runtime?.session_id),
    agentIdFromOmpSessionValue(ctx?.runtime?.session?.id),
    agentIdFromOmpSessionValue(ctx?.runtime?.session?.sessionId),
    agentIdFromOmpSessionValue(ctx?.runtime?.session?.session_id)
  );
}}

function detectAdapterAgentId(event, ctx) {{
  return firstString(
    agentIdFromString(event?.agentId),
    agentIdFromString(event?.agent_id),
    agentIdFromString(event?.agent?.id),
    agentIdFromString(event?.agent?.agentId),
    agentIdFromString(event?.agent?.agent_id),
    agentIdFromString(ctx?.agentId),
    agentIdFromString(ctx?.agent_id),
    agentIdFromString(ctx?.agent?.id),
    agentIdFromString(ctx?.agent?.agentId),
    agentIdFromString(ctx?.agent?.agent_id)
  );
}}

function detectAgentId(event, ctx) {{
  return detectAdapterAgentId(event, ctx)
    || detectOmpSessionAgentId(event, ctx);
}}

function detectWorkspaceId(event, ctx) {{
  return firstString(
    workspaceIdFromString(event?.workspaceId),
    workspaceIdFromString(event?.workspace_id),
    workspaceIdFromString(event?.workspace?.id),
    workspaceIdFromString(event?.workspace?.workspaceId),
    workspaceIdFromString(event?.workspace?.workspace_id),
    workspaceIdFromString(ctx?.workspaceId),
    workspaceIdFromString(ctx?.workspace_id),
    workspaceIdFromString(ctx?.workspace?.id),
    workspaceIdFromString(ctx?.workspace?.workspaceId),
    workspaceIdFromString(ctx?.workspace?.workspace_id)
  );
}}

function missingAgentIdReason() {{
  return "Stateful requires OMP-provided agent/session identity for the active agent; no agent_id was available, so Stateful actions are disabled for this agent.";
}}


function agentId(event, ctx) {{
  const id = detectAgentId(event, ctx);
  if (!id) throw new Error(missingAgentIdReason());
  return id;
}}

function reservationIdFromValue(value) {{
  if (typeof value !== "string") return undefined;
  const id = value.trim();
  return id.length > 0 ? id : undefined;
}}

function reservationId(event, decision) {{
  return firstString(
    reservationIdFromValue(event?.reservation_id),
    reservationIdFromValue(event?.reservationId),
    reservationIdFromValue(event?.input?.reservation_id),
    reservationIdFromValue(event?.input?.reservationId),
    reservationIdFromValue(decision?.reservation_id),
    reservationIdFromValue(decision?.wait?.reservation_id),
    reservationIdFromValue(decision?.reservation?.reservation_id),
    reservationIdFromValue(decision?.reservation?.wait_id),
    reservationIdFromValue(decision?.reservation?.id)
  );
}}

let reservationStreamAbort;
const seenReservationWaitIds = new Set();

function stopReservationStream() {{
  if (reservationStreamAbort) {{
    reservationStreamAbort.abort();
    reservationStreamAbort = undefined;
  }}
}}

function sleepWithAbort(ms, signal) {{
  return new Promise((resolve) => {{
    const timer = setTimeout(resolve, ms);
    if (signal) {{
      signal.addEventListener("abort", () => {{
        clearTimeout(timer);
        resolve();
      }}, {{ once: true }});
    }}
  }});
}}

function reservationStreamUrl(stream) {{
  const base = String(stream.base_url || "").replace(/\/+$/, "");
  return base + "/v1/notifications/stream?agent_id=" + encodeURIComponent(stream.agent_id) + "&workspace_id=" + encodeURIComponent(stream.workspace_id);
}}

function reservationResumeUrl(stream) {{
  const base = String(stream.base_url || "").replace(/\/+$/, "");
  return base + "/v1/resume/next";
}}

function reservationMessage(notification) {{
  const payload = notification?.payload || {{}};
  const target = payload.relative_path || "the reserved target";
  const waitId = payload.wait_id || "unknown";
  const reservationId = payload.reservation_id || waitId;
  const action = payload.action || "write";
  const purpose = payload.purpose;
  const lines = [
    "Stateful reservation is ready for " + target + ".",
    "wait_id: " + waitId,
    "reservation_id: " + reservationId,
    "action: " + action,
  ];
  if (typeof purpose === "string" && purpose.trim().length > 0) {{
    lines.push("purpose: " + purpose.trim());
  }}
  lines.push("Next: reread the target, then call state_reservation_claim with this reservation_id before retrying the write.");
  return lines.join("\n");
}}

function notificationTargetsStreamAgent(notification, stream) {{
  const targetAgentId = property(notification, "target_agent_id")
    || property(notification, "agent_id")
    || propertyPath(notification, "payload", "agent_id");
  if (!targetAgentId) return true;
  return targetAgentId === property(stream, "agent_id");
}}


function deliverReservationNotification(pi, notification, stream) {{
  if (!notificationTargetsStreamAgent(notification, stream)) return;
  const payload = notification?.payload || {{}};
  const waitId = payload.wait_id;
  if (!waitId || seenReservationWaitIds.has(waitId)) {{
    return;
  }}
  if (typeof pi?.sendMessage !== "function") {{
    return;
  }}
  const text = reservationMessage(notification);
  try {{
    pi.sendMessage(
      {{
        customType: "stateful_reservation_ready",
        content: text,
        display: true,
        details: notification,
      }},
      {{ triggerTurn: true, deliverAs: "nextTurn" }}
    );
    seenReservationWaitIds.add(waitId);
  }} catch (_) {{}}
}}

async function checkReservationResume(pi, stream, signal) {{
  if (typeof fetch !== "function" || signal?.aborted) return;
  try {{
    const response = await fetch(reservationResumeUrl(stream), {{
      method: "POST",
      headers: {{
        authorization: stream.authorization,
        "content-type": "application/json",
      }},
      body: JSON.stringify({{ agent_id: stream.agent_id, workspace_id: stream.workspace_id }}),
      signal,
    }});
    if (!response.ok) return;
    const body = await response.json();
    if (body?.resume_available && body?.reservation) {{
      deliverReservationNotification(pi, {{
        notification_id: "resume:" + body.reservation.wait_id,
        workspace_id: stream.workspace_id,
        kind: "reservation_granted",
        payload: {{
          wait_id: body.reservation.wait_id,
          reservation_id: body.reservation.reservation_id || body.reservation.wait_id,
          relative_path: body.reservation.relative_path,
          action: body.reservation.action,
          purpose: body.reservation.purpose,
          reservation_expires_at: body.reservation.reservation_expires_at,
        }},
        required_next_action: body.required_next_action,
      }}, stream);
    }}
  }} catch (_) {{}}
}}

function processReservationSseBlock(pi, block, stream) {{
  let event = "message";
  const data = [];
  for (const rawLine of block.split(/\r?\n/)) {{
    const line = rawLine.trimEnd();
    if (line.startsWith("event:")) event = line.slice(6).trim();
    if (line.startsWith("data:")) data.push(line.slice(5).trimStart());
  }}
  if (event !== "reservation_granted" || data.length === 0) return;
  try {{
    deliverReservationNotification(pi, JSON.parse(data.join("\n")), stream);
  }} catch (_) {{}}
}}

function processReservationSseBuffer(pi, buffer, stream) {{
  buffer = buffer.replace(/\r\n/g, "\n");
  let cursor = 0;
  for (;;) {{
    const next = buffer.indexOf("\n\n", cursor);
    if (next === -1) break;
    processReservationSseBlock(pi, buffer.slice(cursor, next), stream);
    cursor = next + 2;
  }}
  return buffer.slice(cursor);
}}

function startReservationStream(pi, stream) {{
  if (!stream?.base_url || !stream?.authorization || !stream?.agent_id || !stream?.workspace_id) return;
  if (typeof fetch !== "function" || typeof TextDecoder !== "function") return;
  stopReservationStream();
  const controller = new AbortController();
  reservationStreamAbort = controller;
  const signal = controller.signal;
  const run = async () => {{
    let backoffMs = 1000;
    await checkReservationResume(pi, stream, signal);
    while (!signal.aborted) {{
      try {{
        const response = await fetch(reservationStreamUrl(stream), {{
          headers: {{ authorization: stream.authorization, accept: "text/event-stream" }},
          signal,
        }});
        if (!response.ok || !response.body?.getReader) throw new Error("reservation stream unavailable");
        backoffMs = 1000;
        const reader = response.body.getReader();
        const decoder = new TextDecoder();
        let buffer = "";
        for (;;) {{
          const {{ done, value }} = await reader.read();
          if (done || signal.aborted) break;
          buffer = processReservationSseBuffer(pi, buffer + decoder.decode(value, {{ stream: true }}), stream);
        }}
      }} catch (_) {{
        if (signal.aborted) return;
        await checkReservationResume(pi, stream, signal);
        await sleepWithAbort(backoffMs, signal);
        backoffMs = Math.min(backoffMs * 2, 30000);
      }}
    }}
  }};
  run().catch(() => {{}});
}}


const EXTERNAL_GRANT_DEFAULT_MAX_USES = 5;
const EXTERNAL_GRANT_MAX_USES_LIMIT = 20;
const EXTERNAL_GRANT_DEFAULT_TTL_MS = 10 * 60 * 1000;
const EXTERNAL_GRANT_MAX_TTL_MS = 60 * 60 * 1000;
const externalBashGrants = new Map();


function stringList(value) {{
  if (Array.isArray(value)) {{
    return value.filter((item) => typeof item === "string" && item.trim().length > 0);
  }}
  if (typeof value === "string" && value.trim().length > 0) {{
    return [value];
  }}
  return [];
}}


const lazyEditOperations = new Map();
let lazyEditOperationCounter = 0;
const lazyWriteOperations = new Map();
let lazyWriteOperationCounter = 0;
const lazyBashOperations = new Map();
let lazyBashOperationCounter = 0;

function extractWaitId(reason) {{
  const match = String(reason || "").match(/wait_id ([A-Za-z0-9_-]+)/);
  return match ? match[1] : "";
}}

function extractReservationId(reason) {{
  const match = String(reason || "").match(/reservation_id[: ]+([A-Za-z0-9_-]+)/);
  return match ? match[1] : "";
}}

function structuredLazyEditOperationId(decision) {{
  return decision?.wait?.wait_id
    || decision?.reservation?.wait_id
    || decision?.reservation?.id
    || extractWaitId(decision?.reason)
    || extractWaitId(decision?.message);
}}

function structuredLazyWriteOperationId(decision) {{
  return decision?.wait?.wait_id
    || decision?.reservation?.wait_id
    || decision?.reservation?.id
    || extractWaitId(decision?.reason)
    || extractWaitId(decision?.message);
}}

function structuredLazyReservationId(event, decision) {{
  return reservationId(event, decision)
    || extractReservationId(decision?.reason)
    || extractReservationId(decision?.message)
    || "";
}}

function nextLazyEditOperationId() {{
  lazyEditOperationCounter += 1;
  return "lazy-edit-" + Date.now().toString(36) + "-" + lazyEditOperationCounter.toString(36);
}}

function nextLazyWriteOperationId() {{
  lazyWriteOperationCounter += 1;
  return "lazy-write-" + Date.now().toString(36) + "-" + lazyWriteOperationCounter.toString(36);
}}

function nextLazyBashOperationId() {{
  lazyBashOperationCounter += 1;
  return "lazy-bash-" + Date.now().toString(36) + "-" + lazyBashOperationCounter.toString(36);
}}

function editPatchTargets(input) {{
  const patch = String(input?.input || "");
  const targets = [];
  for (const line of patch.split(/\r?\n/)) {{
    const match = line.match(/^\[([^#\]\r\n]+)#[0-9A-Fa-f]{{4}}\]$/);
    if (match) targets.push(match[1]);
  }}
  return [...new Set(targets)];
}}

function safeLazyOperationTarget(target) {{
  return typeof target === "string"
    && target.length > 0
    && !target.startsWith("/")
    && !target.includes("\\")
    && !target.includes(":")
    && !target.split("/").some((part) => part === "" || part === "." || part === "..");
}}

function readOperationBases(cwd, targets) {{
  const bases = new Map();
  for (const target of targets) {{
    const path = resolve(cwd, target);
    bases.set(target, existsSync(path) ? readFileSync(path, "utf8") : null);
  }}
  return bases;
}}

function rememberLazyEditOperation(event, ctx, decision) {{
  if (event?.toolName !== "edit") return "";
  const targets = editPatchTargets(event.input || {{}});
  if (targets.length === 0 || !targets.every(safeLazyOperationTarget)) return "";
  const operationId = structuredLazyEditOperationId(decision) || nextLazyEditOperationId();
  lazyEditOperations.set(operationId, {{
    operation_id: operationId,
    agent_id: agentId(event, ctx),
    reservation_id: structuredLazyReservationId(event, decision),
    cwd: ctx.cwd,
    tool_name: event.toolName,
    tool_input: event.input || {{}},
    targets,
    bases: readOperationBases(ctx.cwd, targets),
    blocked_reason: decision?.reason || "",
  }});
  return operationId;
}}

function writeToolTarget(input) {{
  const target = String(input?.path || "").trim();
  return safeLazyOperationTarget(target) ? target : "";
}}

function rememberLazyWriteOperation(event, ctx, decision) {{
  if (event?.toolName !== "write") return "";
  const target = writeToolTarget(event.input || {{}});
  if (!target) return "";
  const operationId = structuredLazyWriteOperationId(decision) || nextLazyWriteOperationId();
  const targets = [target];
  lazyWriteOperations.set(operationId, {{
    operation_id: operationId,
    agent_id: agentId(event, ctx),
    reservation_id: structuredLazyReservationId(event, decision),
    cwd: ctx.cwd,
    tool_name: event.toolName,
    tool_input: event.input || {{}},
    targets,
    bases: readOperationBases(ctx.cwd, targets),
    blocked_reason: decision?.reason || "",
  }});
  return operationId;
}}

function normalizedStatefulCommandWords(words) {{
  const normalized = [...words];
  if (normalized[0] === "stateful") normalized[0] = STATEFUL;
  return normalized;
}}

function rememberLazyBashOperation(event, ctx, decision) {{
  if (event?.toolName !== "bash" && event?.toolName !== "functions.bash") return "";
  if (!decision?.externalGrantParams || !Array.isArray(decision?.words)) return "";
  const operationId = nextLazyBashOperationId();
  lazyBashOperations.set(operationId, {{
    operation_id: operationId,
    agent_id: agentId(event, ctx),
    cwd: ctx.cwd,
    tool_name: event.toolName,
    tool_input: event.input || {{}},
    command: String(event?.input?.command || ""),
    command_words: normalizedStatefulCommandWords(decision.words),
    grant_params: decision.externalGrantParams,
  }});
  return operationId;
}}

function textToLines(text) {{
  if (text === "") return {{ lines: [], trailing: false }};
  const trailing = text.endsWith("\n");
  const body = trailing ? text.slice(0, -1) : text;
  return {{ lines: body.length ? body.split("\n") : [], trailing }};
}}

function linesToText(lines, trailing) {{
  return lines.join("\n") + (trailing && lines.length ? "\n" : "");
}}

function readPatchBody(lines, cursor) {{
  const body = [];
  while (cursor < lines.length && lines[cursor].startsWith("+")) {{
    body.push(lines[cursor].slice(1));
    cursor += 1;
  }}
  return {{ body, cursor }};
}}

function parseOmpLinePatch(patch) {{
  const lines = String(patch || "").replace(/\r\n/g, "\n").split("\n");
  const files = new Map();
  let current = null;
  for (let i = 0; i < lines.length;) {{
    const line = lines[i];
    if (line === "*** Begin Patch" || line === "*** End Patch") {{ i += 1; continue; }}
    if (!line) {{ i += 1; continue; }}
    const header = line.match(/^\[([^#\]\r\n]+)#[0-9A-Fa-f]{{4}}\]$/);
    if (header) {{
      current = header[1];
      if (!files.has(current)) files.set(current, []);
      i += 1;
      continue;
    }}
    if (!current) throw new Error("lazy_edit_resume patch missing file header");
    if (/^(SWAP|DEL)\.BLK |^INS\.BLK\.POST /.test(line)) {{
      throw new Error("lazy_edit_resume supports line edits only; regenerate patch for block operations");
    }}
    let match = line.match(/^SWAP ([1-9]\d*)\.=([1-9]\d*):$/);
    if (match) {{
      const read = readPatchBody(lines, i + 1);
      files.get(current).push({{ kind: "swap", start: Number(match[1]), end: Number(match[2]), body: read.body }});
      i = read.cursor;
      continue;
    }}
    match = line.match(/^DEL ([1-9]\d*)(?:\.=([1-9]\d*))?$/);
    if (match) {{
      files.get(current).push({{ kind: "del", start: Number(match[1]), end: Number(match[2] || match[1]), body: [] }});
      i += 1;
      continue;
    }}
    match = line.match(/^INS\.(HEAD|TAIL):$/);
    if (match) {{
      const read = readPatchBody(lines, i + 1);
      files.get(current).push({{ kind: "ins", pos: match[1].toLowerCase(), line: 0, body: read.body }});
      i = read.cursor;
      continue;
    }}
    match = line.match(/^INS\.(PRE|POST) ([1-9]\d*):$/);
    if (match) {{
      const read = readPatchBody(lines, i + 1);
      files.get(current).push({{ kind: "ins", pos: match[1].toLowerCase(), line: Number(match[2]), body: read.body }});
      i = read.cursor;
      continue;
    }}
    throw new Error("unsupported lazy_edit_resume patch line: " + line);
  }}
  return files;
}}

function validateOmpLinePatchBases(cwd, editsByFile, bases) {{
  for (const target of editsByFile.keys()) {{
    const filePath = resolve(cwd, target);
    const current = existsSync(filePath) ? readFileSync(filePath, "utf8") : null;
    if (current !== (bases.get(target) ?? null)) {{
      return {{ status: "stale", message: target + " changed since operation was queued" }};
    }}
  }}
  return null;
}}

function applyOmpLinePatch(cwd, patch, bases) {{
  const editsByFile = parseOmpLinePatch(patch);
  const stale = validateOmpLinePatchBases(cwd, editsByFile, bases);
  if (stale) return stale;
  for (const [target, edits] of editsByFile.entries()) {{
    const filePath = resolve(cwd, target);
    const current = existsSync(filePath) ? readFileSync(filePath, "utf8") : null;
    const text = current || "";
    const split = textToLines(text);
    const applied = split.lines.slice();
    const ordered = edits.slice().sort((a, b) => {{
      const aLine = a.kind === "ins" ? (a.pos === "tail" ? Number.MAX_SAFE_INTEGER : a.line) : a.start;
      const bLine = b.kind === "ins" ? (b.pos === "tail" ? Number.MAX_SAFE_INTEGER : b.line) : b.start;
      return bLine - aLine;
    }});
    for (const edit of ordered) {{
      if (edit.kind === "swap") {{
        if (edit.start < 1 || edit.end < edit.start || edit.end > applied.length) throw new Error("invalid SWAP range for " + target);
        applied.splice(edit.start - 1, edit.end - edit.start + 1, ...edit.body);
      }} else if (edit.kind === "del") {{
        if (edit.start < 1 || edit.end < edit.start || edit.end > applied.length) throw new Error("invalid DEL range for " + target);
        applied.splice(edit.start - 1, edit.end - edit.start + 1);
      }} else if (edit.kind === "ins") {{
        const index = edit.pos === "head" ? 0 : edit.pos === "tail" ? applied.length : edit.pos === "pre" ? edit.line - 1 : edit.line;
        if (index < 0 || index > applied.length) throw new Error("invalid INS anchor for " + target);
        applied.splice(index, 0, ...edit.body);
      }}
    }}
    writeFileSync(filePath, linesToText(applied, split.trailing || text === ""), "utf8");
  }}
  return {{ status: "applied", message: "lazy edit applied" }};
}}

function applyOmpWrite(cwd, operation) {{
  const target = operation.targets[0];
  const stale = validateOmpLinePatchBases(cwd, new Map([[target, []]]), operation.bases);
  if (stale) return stale;
  const filePath = resolve(cwd, target);
  mkdirSync(dirname(filePath), {{ recursive: true }});
  writeFileSync(filePath, String(operation.tool_input?.content ?? ""), "utf8");
  return {{ status: "applied", message: "lazy write applied" }};
}}

function emptyToolOutputText(text) {{
  if (!String(text || "").trim()) return "No output.";
  return String(text);
}}

function lazyToolResult(status, text, details) {{
  return {{
    isError: status !== "applied",
    content: [{{ type: "text", text: emptyToolOutputText(text) }}],
    details,
  }};
}}

function bashResumeText(result) {{
  if (result.error) return result.error.message || String(result.error);
  const stdout = String(result.stdout || "");
  const stderr = String(result.stderr || "");
  const text = stdout + stderr;
  return text.trim().length ? text : "Command exited with code " + result.status;
}}




function hasExternalWriteScope(params) {{
  return stringList(params.write_targets).length > 0
    || stringList(params.create_targets).length > 0
    || stringList(params.write_dirs).length > 0;
}}


function normalizedStringList(value) {{
  return stringList(value).map((item) => item.trim()).sort();
}}

function externalGrantSettings(params) {{
  const requestedMaxUses = params.grant_max_uses ?? params.grantMaxUses;
  const requestedTtlSeconds = params.grant_expires_seconds ?? params.grantExpiresSeconds;
  let maxUses = EXTERNAL_GRANT_DEFAULT_MAX_USES;
  if (requestedMaxUses !== undefined) {{
    if (!Number.isInteger(requestedMaxUses) || requestedMaxUses < 1 || requestedMaxUses > EXTERNAL_GRANT_MAX_USES_LIMIT) {{
      throw new Error("external sandbox grant grant_max_uses must be an integer from 1 to " + EXTERNAL_GRANT_MAX_USES_LIMIT);
    }}
    maxUses = requestedMaxUses;
  }}
  let ttlMs = EXTERNAL_GRANT_DEFAULT_TTL_MS;
  if (requestedTtlSeconds !== undefined) {{
    if (!Number.isInteger(requestedTtlSeconds) || requestedTtlSeconds < 1 || requestedTtlSeconds > EXTERNAL_GRANT_MAX_TTL_MS / 1000) {{
      throw new Error("external sandbox grant grant_expires_seconds must be an integer from 1 to " + (EXTERNAL_GRANT_MAX_TTL_MS / 1000));
    }}
    ttlMs = requestedTtlSeconds * 1000;
  }}
  return {{ maxUses, ttlMs }};
}}

function externalGrantDescriptor(params) {{
  return {{
    purpose: params.purpose.trim(),
    write_targets: normalizedStringList(params.write_targets),
    create_targets: normalizedStringList(params.create_targets),
    write_dirs: normalizedStringList(params.write_dirs),
    connect_sockets: normalizedStringList(params.connect_sockets),
    allow_signal: params.allow_signal === true,
    network: typeof params.network === "string" ? params.network : "default",
  }};
}}


function externalGrantKey(params) {{
  return JSON.stringify(externalGrantDescriptor(params));
}}

function pruneExternalBashGrants(now) {{
  for (const [key, grant] of externalBashGrants) {{
    if (grant.expiresAt <= now || grant.uses >= grant.maxUses) {{
      externalBashGrants.delete(key);
    }}
  }}
}}

function configBool(value) {{
  return value === true || value === "true" || value === "1" || value === "yes" || value === "on";
}}

function benchmarkSourceBlockPatterns() {{
  const raw = process.env[BENCHMARK_SOURCE_BLOCK_ENV];
  if (!raw) return [];
  try {{
    const parsed = JSON.parse(raw);
    if (Array.isArray(parsed)) {{
      return parsed.map((item) => String(item || "").trim()).filter(Boolean);
    }}
  }} catch (_) {{}}
  return String(raw).split(/[\r\n,]+/).map((item) => item.trim()).filter(Boolean);
}}

function benchmarkSourcePatternMatches(text, pattern) {{
  const lowerPattern = pattern.toLowerCase();
  if (lowerPattern === "upstream" || lowerPattern === "upstream/") {{
    return /(^|[^a-z0-9_-])upstream(?:\/|[^a-z0-9_-]|$)/.test(text);
  }}
  return text.includes(lowerPattern);
}}


function benchmarkSourceBlockReason(event) {{
  const patterns = benchmarkSourceBlockPatterns();
  if (patterns.length === 0) return "";
  const text = (String(event?.toolName || "") + "\n" + JSON.stringify(event?.input || {{}})).toLowerCase();
  for (const pattern of patterns) {{
    if (benchmarkSourcePatternMatches(text, pattern)) {{
      return "DeNovo benchmark blocked target upstream source access before tool execution: " + pattern;
    }}
  }}
  return "";
}}

function configTextAutoApprove(text) {{
  const value = "(?:true|\\\"true\\\"|'true'|1|\\\"1\\\"|'1'|yes|\\\"yes\\\"|'yes'|on|\\\"on\\\"|'on')";
  const body = String(text || "");
  return new RegExp("(^|\\n)\\s*stateful\\.autoApprove\\s*:\\s*" + value + "\\s*(?:#.*)?(?:\\n|$)", "i").test(body)
    || new RegExp("(^|\\n)stateful\\s*:\\s*\\n(?:[ \\t]+[^\\n]*\\n)*?[ \\t]+autoApprove\\s*:\\s*" + value + "\\s*(?:#.*)?(?:\\n|$)", "i").test(body);
}}

function statefulConfigFileAutoApprove() {{
  const configPaths = [
    OMP_AGENT_CONFIG,
    process.env.HOME ? resolve(process.env.HOME, ".omp/profiles/stateful/agent/config.yml") : "",
  ].filter(Boolean);
  for (const configPath of configPaths) {{
    try {{
      if (configTextAutoApprove(readFileSync(configPath, "utf8"))) return true;
    }} catch (_) {{}}
  }}
  return false;
}}

function statefulPromptAutoApproveConfig(ctx) {{
  return configBool(ctx?.config?.stateful?.autoApprove)
    || configBool(ctx?.config?.stateful?.auto_approve)
    || configBool(ctx?.config?.["stateful.autoApprove"])
    || configBool(ctx?.config?.["stateful.auto_approve"])
    || configBool(ctx?.stateful?.autoApprove)
    || configBool(ctx?.stateful?.auto_approve)
    || statefulConfigFileAutoApprove();
}}

function shouldAutoApproveStatefulPrompt(ctx, _params) {{
  return statefulPromptAutoApproveConfig(ctx);
}}

function recordExternalBashGrant(params, now) {{
  const key = externalGrantKey(params);
  const settings = externalGrantSettings(params);
  const approvedAt = now ?? Date.now();
  externalBashGrants.set(key, {{
    expiresAt: approvedAt + settings.ttlMs,
    maxUses: settings.maxUses,
    uses: 1,
  }});
}}

function approveExternalBashGrantWithoutPrompt(params) {{
  const now = Date.now();
  pruneExternalBashGrants(now);
  const key = externalGrantKey(params);
  const existing = externalBashGrants.get(key);
  if (existing && existing.expiresAt > now && existing.uses < existing.maxUses) {{
    existing.uses += 1;
    return true;
  }}
  recordExternalBashGrant(params, now);
  return true;
}}

function externalBashApprovalMessage(params) {{
  const descriptor = externalGrantDescriptor(params);
  const settings = externalGrantSettings(params);
  const scope = [
    ...descriptor.write_targets.map((path) => "write-target: " + path),
    ...descriptor.create_targets.map((path) => "create-target: " + path),
    ...descriptor.write_dirs.map((path) => "write-dir: " + path),
    ...descriptor.connect_sockets.map((path) => "connect-socket: " + path),
    ...(descriptor.allow_signal ? ["allow-signal"] : []),
    "network: " + descriptor.network,
  ];
  const examples = stringList(params.approval_examples);
  return [
    "Stateful is requesting a scoped repo-external sandbox grant.",
    "",
    "Purpose:",
    descriptor.purpose,
    "",
    "Allowed external write/socket/signal scope:",
    scope.length ? scope.join("\n") : "No declared external write/socket/signal scope.",
    "",
    "Grant limits:",
    "max uses: " + settings.maxUses,
    "expires in seconds: " + Math.floor(settings.ttlMs / 1000),
    "",
    "Command examples:",
    examples.length ? examples.map((example) => "- " + example).join("\n") : "- Commands may vary, but must stay within the purpose and scope above.",
    "",
    "Raw command text is intentionally hidden from this approval prompt.",
  ].join("\n");
}}

async function confirmExternalBashGrant(ctx, params, signal) {{
  if (signal?.aborted) return false;
  let abortHandler;
  const abortPromise = signal ? new Promise((resolve) => {{
    abortHandler = () => resolve(false);
    signal.addEventListener("abort", abortHandler, {{ once: true }});
  }}) : undefined;
  try {{
    const confirmPromise = ctx.ui.confirm(
      "Approve external sandbox grant",
      externalBashApprovalMessage(params)
    );
    return abortPromise
      ? await Promise.race([confirmPromise, abortPromise])
      : await confirmPromise;
  }} finally {{
    if (signal && abortHandler) {{
      signal.removeEventListener("abort", abortHandler);
    }}
  }}
}}

async function ensureExternalBashGrant(ctx, params, signal) {{
  const now = Date.now();
  pruneExternalBashGrants(now);
  const key = externalGrantKey(params);
  const existing = externalBashGrants.get(key);
  if (existing && existing.expiresAt > now && existing.uses < existing.maxUses) {{
    existing.uses += 1;
    return true;
  }}
  const approved = await confirmExternalBashGrant(ctx, params, signal);
  if (!approved) return false;
  recordExternalBashGrant(params, Date.now());
  return true;
}}

function quoteStatefulCommandWord(word) {{
  const value = String(word || "");
  if (/^[A-Za-z0-9_@%+=:,./-]+$/.test(value)) return value;
  return "'" + value.replace(/'/g, "'\\''") + "'";
}}

function commandWordsToShell(words) {{
  return words.map(quoteStatefulCommandWord).join(" ");
}}

function flagValue(words, flag) {{
  const index = words.indexOf(flag);
  if (index < 0) return undefined;
  return words[index + 1];
}}

function insertSandboxIdentityFlag(words, flag, value) {{
  if (flagValue(words, flag) !== undefined) return words;
  words.splice(3, 0, flag, value);
  return words;
}}

function commandWithActiveSandboxIdentity(words, event, ctx) {{
  if (!Array.isArray(words) || words[1] !== "sandbox" || words[2] !== "run") {{
    return {{ words, command: undefined }};
  }}
  const activeAgentId = agentId(event, ctx);
  const suppliedAgentId = flagValue(words, "--agent-id");
  if (suppliedAgentId !== undefined && suppliedAgentId !== activeAgentId) {{
    throw new Error("stateful sandbox run --agent-id must match the active agent_id");
  }}
  const rewritten = [...words];
  insertSandboxIdentityFlag(rewritten, "--agent-id", activeAgentId);

  const activeWorkspaceId = detectWorkspaceId(event, ctx);
  const suppliedWorkspaceId = flagValue(words, "--workspace-id");
  if (activeWorkspaceId) {{
    if (suppliedWorkspaceId !== undefined && suppliedWorkspaceId !== activeWorkspaceId) {{
      throw new Error("stateful sandbox run --workspace-id must match the active workspace_id");
    }}
    insertSandboxIdentityFlag(rewritten, "--workspace-id", activeWorkspaceId);
  }}
  return {{ words: rewritten, command: commandWordsToShell(rewritten) }};
}}


function splitStatefulCommandWords(command) {{
  const words = [];
  let current = "";
  let quote = null;
  for (let index = 0; index < command.length; index += 1) {{
    const ch = command[index];
    if (quote) {{
      if (ch === quote) {{
        quote = null;
      }} else {{
        current += ch;
      }}
      continue;
    }}
    if (ch === "'" || ch === "\"") {{
      quote = ch;
      continue;
    }}
    if (ch === "\\" || ch === "`" || ch === "\n" || ch === "\r") {{
      throw new Error("Bash wrapper must be a single stateful sandbox command");
    }}
    if (ch === "$" && command[index + 1] === "(") {{
      throw new Error("Bash wrapper must not use command substitution");
    }}
    if (";|&<>".includes(ch)) {{
      throw new Error("Bash wrapper must be a single stateful sandbox command");
    }}
    if (/\s/.test(ch)) {{
      if (current) {{
        words.push(current);
        current = "";
      }}
      continue;
    }}
    current += ch;
  }}
  if (quote) throw new Error("Bash wrapper command has unterminated quotes");
  if (current) words.push(current);
  return words;
}}

function parseStatefulSandboxRunWords(words) {{
  if (words.length < 4 || words[1] !== "sandbox" || words[2] !== "run") {{
    return {{ allow: false, reason: "Bash commands must use stateful sandbox run" }};
  }}
  const params = {{
    fs: "read-only",
    purpose: "",
    write_targets: [],
    create_targets: [],
    write_dirs: [],
    connect_sockets: [],
    allow_signal: false,
    network: undefined,
    agent_id: undefined,
    workspace_id: undefined,
    command: "",
  }};
  for (let index = 3; index < words.length; index += 1) {{
    const arg = words[index];
    const nextValue = (name) => {{
      index += 1;
      if (index >= words.length || !words[index]) throw new Error("stateful sandbox run " + name + " requires a value");
      return words[index];
    }};
    if (arg === "--fs") params.fs = nextValue("--fs");
    else if (arg === "--purpose") params.purpose = nextValue("--purpose");
    else if (arg === "--write-target") params.write_targets.push(nextValue("--write-target"));
    else if (arg === "--create-target") params.create_targets.push(nextValue("--create-target"));
    else if (arg === "--write-dir") params.write_dirs.push(nextValue("--write-dir"));
    else if (arg === "--connect-socket") params.connect_sockets.push(nextValue("--connect-socket"));
    else if (arg === "--network") params.network = nextValue("--network");
    else if (arg === "--timeout-seconds") nextValue("--timeout-seconds");
    else if (arg === "--stream-events") continue;
    else if (arg === "--allow-signal") params.allow_signal = true;
    else if (arg === "--agent-id") params.agent_id = nextValue("--agent-id");
    else if (arg === "--workspace-id") params.workspace_id = nextValue("--workspace-id");
    else if (arg === "--command") params.command = nextValue("--command");
    else throw new Error("unsupported stateful sandbox run argument `" + arg + "`");
  }}
  if (!params.command) return {{ allow: false, reason: "stateful sandbox run requires exactly one --command" }};
  if (params.fs === "external" && !params.purpose.trim()) {{
    return {{ allow: false, reason: "stateful sandbox run --fs external requires --purpose" }};
  }}
  if (params.fs === "external" && (hasExternalWriteScope(params) || stringList(params.connect_sockets).length > 0 || params.allow_signal === true)) {{
    return {{ allow: true, externalGrantParams: params }};
  }}
  return {{ allow: true }};
}}

function parseStatefulProcessFindWords(words) {{
  if (words.length < 5 || words[1] !== "sandbox" || words[2] !== "process" || words[3] !== "find") {{
    return {{ allow: false, reason: "Bash commands must use stateful sandbox process find" }};
  }}
  return {{ allow: true }};
}}

function statefulBashPassthroughDecision(command) {{
  try {{
    const words = splitStatefulCommandWords(String(command || "").trim());
    if (words.length === 0) return {{ allow: false, reason: "Bash command is empty" }};
    if (!isTrustedStatefulCommand(words[0])) {{
      return {{ allow: false, reason: "OMP raw Bash is denied; use the trusted stateful sandbox command" }};
    }}
    let decision;
    if (words[1] === "sandbox" && words[2] === "run") decision = parseStatefulSandboxRunWords(words);
    else if (words[1] === "sandbox" && words[2] === "process" && words[3] === "find") decision = parseStatefulProcessFindWords(words);
    else decision = {{ allow: false, reason: "Bash commands must use stateful sandbox run or stateful sandbox process find" }};
    if (decision.allow) decision.words = words;
    return decision;
  }} catch (error) {{
    return {{ allow: false, reason: error instanceof Error ? error.message : String(error) }};
  }}
}}


export default function statefulOmpExtension(pi) {{
  pi.setLabel("Stateful");
  pi.on("tool_call", async (event, ctx) => {{
    if (event?.toolName !== "bash" && event?.toolName !== "functions.bash") return;
    const decision = statefulBashPassthroughDecision(event?.input?.command);
    if (!decision.allow) return {{ block: true, reason: decision.reason }};
    try {{
      const rewritten = commandWithActiveSandboxIdentity(decision.words, event, ctx);
      decision.words = rewritten.words;
      if (rewritten.command && event?.input) event.input.command = rewritten.command;
    }} catch (error) {{
      return {{ block: true, reason: error instanceof Error ? error.message : String(error) }};
    }}
    if (decision.externalGrantParams) {{
      const params = decision.externalGrantParams;
      if (shouldAutoApproveStatefulPrompt(ctx, params)) {{
        approveExternalBashGrantWithoutPrompt(params);
        return;
      }}
      if (typeof ctx?.ui?.confirm !== "function") {{
        const operationId = rememberLazyBashOperation(event, ctx, decision);
        const suffix = operationId
          ? "\n\nQueued lazy bash operation_id: " + operationId + "\nNext: approve the external sandbox grant, then call lazy_bash_resume with this operation_id."
          : "";
        return {{ block: true, reason: "Built-in Bash external sandbox command requires OMP UI confirmation; use stateful.autoApprove to skip this prompt." + suffix }};
      }}
      const signal = undefined;
      const approved = await ensureExternalBashGrant(ctx, params, signal);
      if (!approved) return {{ block: true, reason: "user denied stateful external sandbox grant" }};
    }}
  }});
  pi.registerTool({{
    name: "lazy_edit_resume",
    label: "Lazy Edit Resume",
    description: "Resume a blocked OMP edit operation after the needed reservation or claim is ready. Applies only strict line-based OMP edit patches captured in this live extension session.",
    parameters: {{
      type: "object",
      properties: {{
        operation_id: {{ type: "string", description: "Queued lazy edit operation id; either a Stateful wait_id or a generated live-session id printed in the block message." }},
      }},
      required: ["operation_id"],
    }},
    async execute(_toolCallId, params, _signal, _onUpdate, ctx) {{
      const operationId = String(params?.operation_id || "").trim();
      const operation = lazyEditOperations.get(operationId);
      if (!operation) {{
        return lazyToolResult("failed", "lazy edit operation not found in this live OMP extension session", {{ operation_id: operationId }});
      }}
      const authorization = runStatefulHook("pre-tool-use", {{
        agent_id: operation.agent_id,
        reservation_id: operation.reservation_id || undefined,
        cwd: operation.cwd || ctx.cwd,
        yolo: true,
        tool_name: operation.tool_name,
        tool_input: operation.tool_input,
      }});
      if (authorization.decision !== "allow") {{
        return lazyToolResult("failed", authorization.reason || "stateful authorization denied lazy edit resume", {{ operation_id: operationId, authorization }});
      }}
      let result;
      try {{
        result = applyOmpLinePatch(operation.cwd || ctx.cwd, operation.tool_input?.input || "", operation.bases);
      }} catch (error) {{
        result = {{ status: "failed", message: error instanceof Error ? error.message : String(error) }};
      }}
      if (result.status === "applied") {{
        lazyEditOperations.delete(operationId);
        runStatefulHook("post-tool-use", {{
          agent_id: operation.agent_id,
          cwd: operation.cwd || ctx.cwd,
          tool_name: operation.tool_name,
          tool_input: operation.tool_input,
        }});
      }}
      return lazyToolResult(result.status, result.message, {{ operation_id: operationId, targets: operation.targets }});
    }},
  }});
  pi.registerTool({{
    name: "lazy_write_resume",
    label: "Lazy Write Resume",
    description: "Resume a blocked OMP write operation after the needed reservation or claim is ready. Replays only write operations captured in this live extension session and fails if the target changed while queued.",
    parameters: {{
      type: "object",
      properties: {{
        operation_id: {{ type: "string", description: "Queued lazy write operation id; either a Stateful wait_id or a generated live-session id printed in the block message." }},
      }},
      required: ["operation_id"],
    }},
    async execute(_toolCallId, params, _signal, _onUpdate, ctx) {{
      const operationId = String(params?.operation_id || "").trim();
      const operation = lazyWriteOperations.get(operationId);
      if (!operation) {{
        return lazyToolResult("failed", "lazy write operation not found in this live OMP extension session", {{ operation_id: operationId }});
      }}
      const authorization = runStatefulHook("pre-tool-use", {{
        agent_id: operation.agent_id,
        reservation_id: operation.reservation_id || undefined,
        cwd: operation.cwd || ctx.cwd,
        yolo: true,
        tool_name: operation.tool_name,
        tool_input: operation.tool_input,
      }});
      if (authorization.decision !== "allow") {{
        return lazyToolResult("failed", authorization.reason || "stateful authorization denied lazy write resume", {{ operation_id: operationId, authorization }});
      }}
      let result;
      try {{
        result = applyOmpWrite(operation.cwd || ctx.cwd, operation);
      }} catch (error) {{
        result = {{ status: "failed", message: error instanceof Error ? error.message : String(error) }};
      }}
      if (result.status === "applied") {{
        lazyWriteOperations.delete(operationId);
        runStatefulHook("post-tool-use", {{
          agent_id: operation.agent_id,
          cwd: operation.cwd || ctx.cwd,
          tool_name: operation.tool_name,
          tool_input: operation.tool_input,
        }});
      }}
      return lazyToolResult(result.status, result.message, {{ operation_id: operationId, targets: operation.targets }});
    }},
  }});
  pi.registerTool({{
    name: "lazy_bash_resume",
    label: "Lazy Bash Resume",
    description: "Resume a blocked OMP Bash command after approving an external sandbox grant.",
    parameters: {{
      type: "object",
      properties: {{
        operation_id: {{ type: "string", description: "Queued lazy bash operation id printed in the block message." }},
      }},
      required: ["operation_id"],
    }},
    async execute(_toolCallId, params, signal, _onUpdate, ctx) {{
      const operationId = String(params?.operation_id || "").trim();
      const operation = lazyBashOperations.get(operationId);
      if (!operation) {{
        return lazyToolResult("failed", "lazy bash operation not found in this live OMP extension session", {{ operation_id: operationId }});
      }}
      if (typeof ctx?.ui?.confirm !== "function" && !shouldAutoApproveStatefulPrompt(ctx, operation.grant_params)) {{
        return lazyToolResult("failed", "lazy bash resume requires OMP UI confirmation or stateful.autoApprove", {{ operation_id: operationId }});
      }}
      const approved = shouldAutoApproveStatefulPrompt(ctx, operation.grant_params)
        ? approveExternalBashGrantWithoutPrompt(operation.grant_params)
        : await ensureExternalBashGrant(ctx, operation.grant_params, signal);
      if (!approved) {{
        return lazyToolResult("failed", "user denied stateful external sandbox grant", {{ operation_id: operationId }});
      }}
      const authorization = runStatefulHook("pre-tool-use", {{
        agent_id: operation.agent_id,
        cwd: operation.cwd || ctx.cwd,
        yolo: true,
        tool_name: operation.tool_name,
        tool_input: operation.tool_input,
      }});
      if (authorization.decision !== "allow") {{
        return lazyToolResult("failed", authorization.reason || "stateful authorization denied lazy bash resume", {{ operation_id: operationId, authorization }});
      }}
      const words = operation.command_words || [];
      const result = spawnSync(words[0], words.slice(1), {{
        cwd: operation.cwd || ctx.cwd,
        encoding: "utf8",
      }});
      if (result.status === 0) {{
        lazyBashOperations.delete(operationId);
        runStatefulHook("post-tool-use", {{
          agent_id: operation.agent_id,
          cwd: operation.cwd || ctx.cwd,
          tool_name: operation.tool_name,
          tool_input: operation.tool_input,
        }});
      }}
      return lazyToolResult(result.status === 0 ? "applied" : "failed", bashResumeText(result), {{ operation_id: operationId, exit_code: result.status }});
    }},
  }});
  pi.on("session_start", async (event, ctx) => {{
    verifyBareStateful(ctx.cwd);
    const activeAgentId = detectAgentId(event, ctx);
    if (!activeAgentId) {{
      stopReservationStream();
      return;
    }}
    const result = runStatefulHook("session-start", {{
      agent_id: activeAgentId,
      cwd: ctx.cwd,
    }});
    startReservationStream(pi, result?.notifications_stream);
  }});
  pi.on("tool_call", async (event, ctx) => {{
    const benchmarkBlockReason = benchmarkSourceBlockReason(event);
    if (benchmarkBlockReason) return {{ block: true, reason: benchmarkBlockReason }};
    const activeAgentId = detectAgentId(event, ctx);
    if (!activeAgentId) return {{ block: true, reason: missingAgentIdReason() }};
    const decision = runStatefulHook("pre-tool-use", {{
      agent_id: activeAgentId,
      reservation_id: reservationId(event),
      cwd: ctx.cwd,
      yolo: isYolo(event, ctx),
      tool_name: event.toolName,
      tool_input: event.input || {{}},
    }});
    if (decision.decision === "prompt" && !shouldAutoApproveStatefulPrompt(ctx, event.input || {{}})) {{
      if (typeof ctx?.ui?.confirm !== "function") {{
        return {{
          block: true,
          reason: "Stateful requested approval, but OMP UI confirmation is unavailable.",
        }};
      }}
      const approved = await ctx.ui.confirm(
        decision.title || "Approve stateful action",
        decision.message || decision.reason || "Approve this stateful action?"
      );
      if (!approved) {{
        return {{ block: true, reason: decision.reason || "Blocked by user" }};
      }}
    }}
    if (decision.decision === "block") {{
      const editOperationId = rememberLazyEditOperation(event, ctx, decision);
      const writeOperationId = rememberLazyWriteOperation(event, ctx, decision);
      const suffix = editOperationId
        ? "\n\nQueued lazy edit operation_id: " + editOperationId + "\nNext: when reservation or claim is ready, call lazy_edit_resume with this operation_id."
        : writeOperationId
          ? "\n\nQueued lazy write operation_id: " + writeOperationId + "\nNext: when reservation or claim is ready, call lazy_write_resume with this operation_id."
          : "";
      return {{ block: true, reason: decision.reason + suffix }};
    }}
  }});
  pi.on("tool_result", async (event, ctx) => {{
    const activeAgentId = detectAgentId(event, ctx);
    if (!activeAgentId) return;
    runStatefulHook("post-tool-use", {{
      agent_id: activeAgentId,
      cwd: ctx.cwd,
      tool_name: event.toolName,
      tool_input: event.input || {{}},
    }});
  }});
  pi.on("session_shutdown", async (event, ctx) => {{
    stopReservationStream();
    const activeAgentId = detectAgentId(event, ctx);
    if (!activeAgentId) return;
    runStatefulHook("stop", {{
      agent_id: activeAgentId,
      cwd: ctx.cwd,
    }});
  }});
}};
"#
    );
    write_or_replace_text_file_with_mode(extension_path, &contents, private_file_mode())
}

fn shell_quote_posix(value: &str) -> anyhow::Result<String> {
    validate_no_control_chars(value)?;

    let mut quoted = String::from("'");
    for character in value.chars() {
        if character == '\'' {
            quoted.push_str("'\\''");
        } else {
            quoted.push(character);
        }
    }
    quoted.push('\'');

    Ok(quoted)
}

fn validate_no_control_chars(value: &str) -> anyhow::Result<()> {
    if value.chars().any(char::is_control) {
        anyhow::bail!("binary path contains a control character");
    }

    Ok(())
}

fn stateful_command_policy_skill() -> &'static str {
    include_str!("../assets/stateful-command-policy/SKILL.md")
}

fn stateful_command_policy_support_files() -> &'static [(&'static str, &'static str)] {
    &[
        (
            "omp-tools.md",
            include_str!("../assets/stateful-command-policy/omp-tools.md"),
        ),
        (
            "sandbox-tools.md",
            include_str!("../assets/stateful-command-policy/sandbox-tools.md"),
        ),
        (
            "denial-recovery.md",
            include_str!("../assets/stateful-command-policy/denial-recovery.md"),
        ),
        (
            "subagent-write-recovery.md",
            include_str!("../assets/stateful-command-policy/subagent-write-recovery.md"),
        ),
    ]
}

fn dispatching_parallel_agents_skill() -> &'static str {
    include_str!("../assets/dispatching-parallel-agents/SKILL.md")
}

fn omp_stateful_required_rule() -> &'static str {
    include_str!("../assets/omp-stateful-required-rule.md")
}

fn toml_string(value: &str) -> String {
    let mut escaped = String::from("\"");
    for character in value.chars() {
        match character {
            '\\' => escaped.push_str("\\\\"),
            '"' => escaped.push_str("\\\""),
            '\u{08}' => escaped.push_str("\\b"),
            '\t' => escaped.push_str("\\t"),
            '\n' => escaped.push_str("\\n"),
            '\u{0c}' => escaped.push_str("\\f"),
            '\r' => escaped.push_str("\\r"),
            character if character.is_control() => {
                escaped.push_str(&format!("\\u{:04X}", character as u32));
            }
            character => escaped.push(character),
        }
    }
    escaped.push('"');
    escaped
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_omp_agent_dir_uses_user_omp_profile() {
        assert_eq!(
            default_omp_agent_dir_from_home("home"),
            PathBuf::from("home/.omp/profiles/stateful/agent")
        );
    }

    #[test]
    fn omp_install_places_stateful_rule_in_agent_rules_dir() {
        assert_eq!(
            omp_required_rule_path(Path::new("home/.omp/profiles/stateful/agent")),
            PathBuf::from("home/.omp/profiles/stateful/agent/rules/stateful-required.md")
        );
    }

    #[test]
    fn command_policy_support_files_are_installed_with_skill() {
        assert_eq!(
            global_command_policy_support_file_paths(Path::new("home/.codex/config.toml")),
            vec![
                PathBuf::from("home/.codex/skills/stateful-command-policy/omp-tools.md"),
                PathBuf::from("home/.codex/skills/stateful-command-policy/sandbox-tools.md"),
                PathBuf::from("home/.codex/skills/stateful-command-policy/denial-recovery.md"),
                PathBuf::from(
                    "home/.codex/skills/stateful-command-policy/subagent-write-recovery.md"
                ),
            ]
        );
        assert_eq!(
            omp_command_policy_support_file_paths(Path::new("home/.omp/profiles/stateful/agent")),
            vec![
                PathBuf::from(
                    "home/.omp/profiles/stateful/agent/skills/stateful-command-policy/omp-tools.md"
                ),
                PathBuf::from(
                    "home/.omp/profiles/stateful/agent/skills/stateful-command-policy/sandbox-tools.md"
                ),
                PathBuf::from(
                    "home/.omp/profiles/stateful/agent/skills/stateful-command-policy/denial-recovery.md"
                ),
                PathBuf::from(
                    "home/.omp/profiles/stateful/agent/skills/stateful-command-policy/subagent-write-recovery.md"
                ),
            ]
        );

        let support_files = stateful_command_policy_support_files();
        assert_eq!(support_files.len(), 4);
        assert!(stateful_command_policy_skill().contains("Support Files"));
    }

    #[test]
    fn omp_extension_compacts_empty_tool_output() {
        let temp_dir = std::env::temp_dir().join(format!(
            "stateful-omp-extension-empty-output-test-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&temp_dir);
        fs::create_dir_all(&temp_dir).expect("temp dir should be creatable");
        let extension_path = temp_dir.join("stateful.js");

        write_omp_extension(&extension_path, "/usr/local/bin/stateful")
            .expect("extension should be written");
        let contents = fs::read_to_string(&extension_path).expect("extension should be readable");

        let helper_start = contents
            .find("function emptyToolOutputText")
            .expect("empty output helper should be generated");
        let helper_end = contents[helper_start..]
            .find("\n\nfunction lazyToolResult")
            .map(|offset| helper_start + offset)
            .expect("empty output helper should end before lazyToolResult");
        let script = format!(
            "{}\nconsole.log(JSON.stringify([emptyToolOutputText(''), emptyToolOutputText('  '), emptyToolOutputText('kept')]))",
            &contents[helper_start..helper_end]
        );
        let output = std::process::Command::new("node")
            .arg("-e")
            .arg(script)
            .output()
            .expect("node should execute generated extension helper");

        assert!(
            output.status.success(),
            "node failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(
            String::from_utf8(output.stdout).expect("stdout should be utf8"),
            "[\"No output.\",\"No output.\",\"kept\"]\n"
        );

        fs::remove_dir_all(&temp_dir).expect("temp dir should be removable");
    }

    #[test]
    fn omp_extension_uses_adapter_or_session_agent_id_only() {
        let temp_dir = std::env::temp_dir().join(format!(
            "stateful-omp-extension-agent-id-test-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&temp_dir);
        fs::create_dir_all(&temp_dir).expect("temp dir should be creatable");
        let extension_path = temp_dir.join("stateful.js");

        write_omp_extension(&extension_path, "/usr/local/bin/stateful")
            .expect("extension should be written");
        let contents = fs::read_to_string(&extension_path).expect("extension should be readable");

        let helper_start = contents
            .find("function firstString")
            .expect("identity helpers should be generated");
        let helper_end = contents[helper_start..]
            .find("\n\nfunction reservationIdFromValue")
            .map(|offset| helper_start + offset)
            .expect("identity helpers should end before reservationIdFromValue");
        let script = format!(
            "{}\nlet missing;\ntry {{ agentId({{}}, {{}}); }} catch (error) {{ missing = error.message; }}\nconst values = [agentId({{ agent_id: 'adapter-agent' }}, {{}}), agentId({{ session: {{ id: '019f1a33-e3c1-7000-b2a6-d16cc4f05a52' }} }}, {{}}), agentId({{}}, {{ runtime: {{ session: {{ id: 'session_x' }} }} }}), missing];\nconsole.log(JSON.stringify(values));",
            &contents[helper_start..helper_end]
        );
        let output = std::process::Command::new("node")
            .arg("-e")
            .arg(script)
            .output()
            .expect("node should execute generated extension helper");

        assert!(
            output.status.success(),
            "node failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(
            String::from_utf8(output.stdout).expect("stdout should be utf8"),
            "[\"adapter-agent\",\"omp-019f1a33-e3c1-7000-b2a6-d16cc4f05a52\",\"omp-session_x\",\"Stateful requires OMP-provided agent/session identity for the active agent; no agent_id was available, so Stateful actions are disabled for this agent.\"]\n"
        );

        fs::remove_dir_all(&temp_dir).expect("temp dir should be removable");
    }

    #[test]
    fn omp_stateful_required_rule_is_always_apply() {
        let rule = omp_stateful_required_rule();
        assert!(rule.contains("alwaysApply: true"));
        assert!(rule.contains("skill://stateful-command-policy"));
    }
}
