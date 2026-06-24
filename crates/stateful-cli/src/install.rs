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
use serde_json::json;

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
    let mcp_path = agent_dir.join("mcp.json");
    let skill_path = omp_command_policy_skill_path(&agent_dir);
    let rule_path = omp_required_rule_path(&agent_dir);
    plan.summary = format!(
        "{mode}: install stateful files under {} and configure the OMP stateful profile under {}",
        options.paths.home.display(),
        agent_dir.display()
    );
    plan.files.push(config_path);
    plan.files.push(extension_path);
    plan.files.push(mcp_path);
    plan.files.push(skill_path);
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
    let mcp_path = agent_dir.join("mcp.json");
    fs::create_dir_all(
        extension_path
            .parent()
            .expect("extension path should have parent"),
    )?;
    if let Some(parent) = config_path.parent() {
        fs::create_dir_all(parent)?;
    }
    if let Some(parent) = mcp_path.parent() {
        fs::create_dir_all(parent)?;
    }
    write_omp_config(&config_path, &extension_path, options.update)?;
    write_omp_extension(&extension_path, &options.binary_path)?;
    write_omp_mcp_config(&mcp_path, &options.binary_path)?;
    write_omp_command_policy_skill(&agent_dir)?;
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

{features_section}[mcp_servers.stateful]
command = {}
args = ["mcp", "serve"]
env_vars = ["CODEX_THREAD_ID", "STATEFUL_CODEX_RUN_ID", "STATEFUL_SESSION_ID", "STATEFUL_SERVER_URL", "STATEFUL_SERVER_TOKEN"]
startup_timeout_sec = 20
default_tools_approval_mode = "approve"

[[hooks.SessionStart]]
matcher = "startup|resume|clear|compact"

[[hooks.SessionStart.hooks]]
type = "command"
command = {}
statusMessage = "Loading stateful current state"

[[hooks.UserPromptSubmit]]

[[hooks.UserPromptSubmit.hooks]]
type = "command"
command = {}
statusMessage = "Checking stateful intent context"

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
        toml_string(binary_path),
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
        "{binary_path} sandbox run --fs external --purpose 'install rebuilt binaries' --write-dir /Users/me/.cargo/bin --command 'install -m 755 target/release/stateful /Users/me/.cargo/bin/stateful'"
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
    ensure_omp_child_scalar(&mut lines, "eval", "py", "false", update_existing)?;
    ensure_omp_child_scalar(&mut lines, "eval", "js", "false", update_existing)?;
    ensure_omp_child_scalar(&mut lines, "eval", "rb", "false", update_existing)?;
    ensure_omp_child_scalar(&mut lines, "eval", "jl", "false", update_existing)?;
    ensure_omp_child_scalar(&mut lines, "bash", "enabled", "false", update_existing)?;

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
        r#"import {{ spawn, spawnSync }} from "node:child_process";

const STATEFUL = {binary_json};


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

function detectSessionId(event, ctx) {{
  return event?.sessionId || ctx?.sessionManager?.session?.id || process.env.STATEFUL_SESSION_ID || "omp-session";
}}

function sessionId(event, ctx) {{
  const id = detectSessionId(event, ctx);
  process.env.STATEFUL_SESSION_ID = id;
  return id;
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
  return base + "/v1/notifications/stream?session_id=" + encodeURIComponent(stream.session_id) + "&workspace_id=" + encodeURIComponent(stream.workspace_id);
}}

function reservationResumeUrl(stream) {{
  const base = String(stream.base_url || "").replace(/\/+$/, "");
  return base + "/v1/resume/next";
}}

function reservationMessage(notification) {{
  const payload = notification?.payload || {{}};
  const target = payload.relative_path || "the reserved target";
  const waitId = payload.wait_id || "unknown";
  const action = payload.action || "write";
  const purpose = payload.purpose;
  const lines = [
    "Stateful reservation is ready for " + target + ".",
    "wait_id: " + waitId,
    "action: " + action,
  ];
  if (typeof purpose === "string" && purpose.trim().length > 0) {{
    lines.push("purpose: " + purpose.trim());
  }}
  lines.push("Next: reread the target, then call state_intent_claim with this wait_id before retrying the write.");
  return lines.join("\n");
}}

function deliverReservationNotification(pi, notification) {{
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
      body: JSON.stringify({{ session_id: stream.session_id, workspace_id: stream.workspace_id }}),
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
          relative_path: body.reservation.relative_path,
          action: body.reservation.action,
          purpose: body.reservation.purpose,
          reservation_expires_at: body.reservation.reservation_expires_at,
        }},
        required_next_action: body.required_next_action,
      }});
    }}
  }} catch (_) {{}}
}}

function processReservationSseBlock(pi, block) {{
  let event = "message";
  const data = [];
  for (const rawLine of block.split(/\r?\n/)) {{
    const line = rawLine.trimEnd();
    if (line.startsWith("event:")) event = line.slice(6).trim();
    if (line.startsWith("data:")) data.push(line.slice(5).trimStart());
  }}
  if (event !== "reservation_granted" || data.length === 0) return;
  try {{
    deliverReservationNotification(pi, JSON.parse(data.join("\n")));
  }} catch (_) {{}}
}}

function processReservationSseBuffer(pi, buffer) {{
  buffer = buffer.replace(/\r\n/g, "\n");
  let cursor = 0;
  for (;;) {{
    const next = buffer.indexOf("\n\n", cursor);
    if (next === -1) break;
    processReservationSseBlock(pi, buffer.slice(cursor, next));
    cursor = next + 2;
  }}
  return buffer.slice(cursor);
}}

function startReservationStream(pi, stream) {{
  if (!stream?.base_url || !stream?.authorization || !stream?.session_id || !stream?.workspace_id) return;
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
          buffer = processReservationSseBuffer(pi, buffer + decoder.decode(value, {{ stream: true }}));
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

const MAX_SANDBOX_TOOL_OUTPUT_BYTES = 50 * 1024;
const SANDBOX_BASH_FS_PROFILES = new Set(["read-only", "write-targets", "build", "git", "github-pr"]);

let sandboxJobCounter = 0;

function nextSandboxJobId(label) {{
  sandboxJobCounter += 1;
  return label.replace(/[^a-z0-9]+/gi, "-").replace(/^-+|-+$/g, "").toLowerCase() + "-" + Date.now().toString(36) + "-" + sandboxJobCounter.toString(36);
}}

function stringList(value) {{
  if (Array.isArray(value)) {{
    return value.filter((item) => typeof item === "string" && item.trim().length > 0);
  }}
  if (typeof value === "string" && value.trim().length > 0) {{
    return [value];
  }}
  return [];
}}

function truncateSandboxToolText(value, label) {{
  const text = value || "";
  if (Buffer.byteLength(text, "utf8") <= MAX_SANDBOX_TOOL_OUTPUT_BYTES) {{
    return text;
  }}
  return Buffer
    .from(text, "utf8")
    .subarray(0, MAX_SANDBOX_TOOL_OUTPUT_BYTES)
    .toString("utf8") + "\n\n[" + label + " output truncated to 51200 bytes]";
}}

function addCommonSandboxArgs(args, params, toolName) {{
  for (const target of stringList(params.write_targets)) args.push("--write-target", target);
  for (const target of stringList(params.create_targets)) args.push("--create-target", target);
  for (const dir of stringList(params.write_dirs)) args.push("--write-dir", dir);
  for (const socket of stringList(params.connect_sockets)) args.push("--connect-socket", socket);
  if (params.allow_signal === true) args.push("--allow-signal");
  if (params.network !== undefined) {{
    if (params.network !== "enabled" && params.network !== "disabled") {{
      throw new Error(toolName + " network must be 'enabled' or 'disabled'");
    }}
    args.push("--network", params.network);
  }}
  const timeoutSeconds = params.timeout_seconds ?? params.timeoutSeconds;
  if (timeoutSeconds !== undefined) {{
    if (!Number.isInteger(timeoutSeconds) || timeoutSeconds < 1) {{
      throw new Error(toolName + " timeout_seconds must be a positive integer");
    }}
    args.push("--timeout-seconds", String(timeoutSeconds));
  }}
}}

function sandboxBashArgs(params) {{
  if (typeof params?.fs !== "string" || params.fs.trim().length === 0) {{
    throw new Error("sandbox_bash requires a non-empty fs profile");
  }}
  const fs = params.fs.trim();
  if (fs === "external") {{
    throw new Error("sandbox_bash does not support --fs external; use ext_ro_bash for read-only external operations or ext_rw_bash for external writes");
  }}
  if (!SANDBOX_BASH_FS_PROFILES.has(fs)) {{
    throw new Error("sandbox_bash fs must be one of: read-only, write-targets, build, git, github-pr");
  }}
  if (typeof params?.command !== "string" || params.command.trim().length === 0) {{
    throw new Error("sandbox_bash requires a non-empty command");
  }}
  const args = ["sandbox", "run", "--fs", fs];
  addCommonSandboxArgs(args, params, "sandbox_bash");
  args.push("--command", params.command);
  return args;
}}

function validateExternalPurposeAndCommand(params, toolName) {{
  if (typeof params?.purpose !== "string" || params.purpose.trim().length === 0) {{
    throw new Error(toolName + " requires a non-empty purpose");
  }}
  if (typeof params?.command !== "string" || params.command.trim().length === 0) {{
    throw new Error(toolName + " requires a non-empty command");
  }}
}}

function hasExternalWriteScope(params) {{
  return stringList(params.write_targets).length > 0
    || stringList(params.create_targets).length > 0
    || stringList(params.write_dirs).length > 0;
}}

function externalReadOnlyBashArgs(params) {{
  validateExternalPurposeAndCommand(params, "ext_ro_bash");
  if (hasExternalWriteScope(params) || stringList(params.connect_sockets).length > 0 || params.allow_signal === true) {{
    throw new Error("ext_ro_bash does not accept write, socket, or signal scope; use ext_rw_bash for scoped external operations");
  }}
  const args = ["sandbox", "run", "--fs", "external", "--purpose", params.purpose];
  addCommonSandboxArgs(args, params, "ext_ro_bash");
  args.push("--command", params.command);
  return args;
}}

function externalReadWriteBashArgs(params) {{
  validateExternalPurposeAndCommand(params, "ext_rw_bash");
  if (!hasExternalWriteScope(params)) {{
    throw new Error("ext_rw_bash requires at least one write_targets, create_targets, or write_dirs entry");
  }}
  const args = ["sandbox", "run", "--fs", "external", "--purpose", params.purpose];
  addCommonSandboxArgs(args, params, "ext_rw_bash");
  args.push("--command", params.command);
  return args;
}}

function externalBashApprovalMessage(params, args) {{
  const scope = [
    ...stringList(params.write_targets).map((path) => "write-target: " + path),
    ...stringList(params.create_targets).map((path) => "create-target: " + path),
    ...stringList(params.write_dirs).map((path) => "write-dir: " + path),
    ...stringList(params.connect_sockets).map((path) => "connect-socket: " + path),
    ...(params.allow_signal === true ? ["allow-signal"] : []),
  ];
  return [
    "Stateful is requesting a read/write repo-external sandbox operation.",
    "",
    "Purpose:",
    params.purpose,
    "",
    "Declared external write/socket/signal scope:",
    scope.length ? scope.join("\n") : "No declared external write/socket/signal scope.",
    "",
    "Command:",
    params.command,
    "",
    "Sandbox invocation:",
    STATEFUL + " " + args.join(" "),
  ].join("\n");
}}

function sandboxToolResultText(exitCode, stdout, stderr, error) {{
  if (!stderr && !error) {{
    return stdout || "";
  }}
  const sections = [];
  if (stdout) sections.push(stdout);
  const diagnostics = [];
  diagnostics.push("exit_code: " + exitCode);
  if (stderr) diagnostics.push("stderr:\n" + stderr);
  if (error) diagnostics.push("error:\n" + error);
  sections.push(diagnostics.join("\n\n"));
  return sections.join("\n\n");
}}

function sandboxToolError(error) {{
  return {{
    isError: true,
    content: [{{ type: "text", text: error instanceof Error ? error.message : String(error) }}],
    details: {{ error: error instanceof Error ? error.message : String(error) }},
  }};
}}


function parseSandboxRunOutput(rawStdout) {{
  const text = String(rawStdout || "").trim();
  if (!text || !text.startsWith("{{")) {{
    return null;
  }}
  try {{
    const parsed = JSON.parse(text);
    if (!parsed || typeof parsed !== "object" || !("stdout" in parsed) || !("stderr" in parsed)) {{
      return null;
    }}
    return parsed;
  }} catch (_) {{
    return null;
  }}
}}

function buildSandboxToolResult(params, args, exitCode, stdout, stderr, error) {{
  const text = sandboxToolResultText(exitCode, stdout, stderr, error);
  return {{
    isError: Boolean(error) || exitCode !== 0,
    content: [{{ type: "text", text }}],
    details: {{
      exitCode,
      stdout,
      stderr,
      error,
      command: params.command,
      sandboxArgs: args,
    }},
  }};
}}

function deliverSandboxStdoutChunk(pi, jobId, label, chunk) {{
  if (!chunk || typeof pi?.sendMessage !== "function") {{
    return Promise.resolve();
  }}
  try {{
    return Promise.resolve(pi.sendMessage(
      {{
        customType: "stateful_sandbox_bash_stdout",
        content: chunk,
        display: true,
        details: {{
          runId: jobId,
          stream: "stdout",
          label,
          collapsible: true,
          collapseShortcut: "Ctrl+O",
        }},
      }},
      {{ triggerTurn: false }}
    )).catch(() => {{}});
  }} catch (_) {{
    return Promise.resolve();
  }}
}}
 
function createSandboxStdoutStreamer(pi, jobId, label) {{
  let buffer = "";
  let timer = null;
  let pending = [];
  const flush = () => {{
    if (timer) {{
      clearTimeout(timer);
      timer = null;
    }}
    if (!buffer) return;
    const chunk = buffer;
    buffer = "";
    pending.push(deliverSandboxStdoutChunk(pi, jobId, label, chunk));
  }};
  return {{
    push(chunk) {{
      if (!chunk) return;
      buffer = truncateSandboxToolText(buffer + chunk, label);
      if (!timer) {{
        timer = setTimeout(flush, 200);
      }}
    }},
    flush,
    async drain() {{
      flush();
      const deliveries = pending;
      pending = [];
      await Promise.allSettled(deliveries);
    }},
  }};
}}

function runSandboxToolProcess(params, args, ctx, label, onStdout) {{
  return new Promise((resolve) => {{
    let stdout = "";
    let stderr = "";
    let processError = "";
    let settled = false;
    const finish = (exitCode, error) => {{
      if (settled) return;
      settled = true;
      const sandboxRunOutput = parseSandboxRunOutput(stdout);
      const commandStdout = sandboxRunOutput ? String(sandboxRunOutput.stdout || "") : stdout;
      const commandStderr = sandboxRunOutput ? String(sandboxRunOutput.stderr || "") || stderr : stderr;
      const commandExitCode = typeof sandboxRunOutput?.exit_code === "number" ? sandboxRunOutput.exit_code : exitCode;
      if (commandStdout) {{
        onStdout(commandStdout);
      }}
      const result = buildSandboxToolResult(params, args, commandExitCode, commandStdout, commandStderr, error);
      if (sandboxRunOutput) {{
        result.details.sandboxRunOutput = sandboxRunOutput;
      }}
      resolve(result);
    }};
    let child;
    try {{
      child = spawn(STATEFUL, args, {{
        cwd: ctx.cwd,
        stdio: ["ignore", "pipe", "pipe"],
        detached: false,
      }});
    }} catch (error) {{
      finish(1, error instanceof Error ? error.message : String(error));
      return;
    }}
    child.stdout?.setEncoding("utf8");
    child.stderr?.setEncoding("utf8");
    child.stdout?.on("data", (chunk) => {{
      stdout = truncateSandboxToolText(stdout + chunk, label);
    }});
    child.stderr?.on("data", (chunk) => {{
      stderr = truncateSandboxToolText(stderr + chunk, label);
    }});
    child.on("error", (error) => {{
      processError = error instanceof Error ? error.message : String(error);
    }});
    child.on("close", (code, signalName) => {{
      const exitCode = typeof code === "number" ? code : 1;
      const signalError = signalName ? "terminated by signal " + signalName : "";
      finish(exitCode, processError || signalError);
    }});
  }});
}}

async function runSandboxAwaitedTool(pi, params, args, ctx, label) {{
  const runId = nextSandboxJobId(label);
  const commandLabel = params.command.length > 120 ? params.command.slice(0, 117) + "..." : params.command;
  const stdoutStreamer = createSandboxStdoutStreamer(pi, runId, commandLabel);
  const result = await runSandboxToolProcess(params, args, ctx, label, (chunk) => {{
    stdoutStreamer.push(chunk);
  }});
  await stdoutStreamer.drain();
  return result;
}}

export default function statefulOmpExtension(pi) {{
  pi.setLabel("Stateful");
  pi.registerTool({{
    name: "sandbox_bash",
    label: "Sandbox Bash",
    description: "Run a command through stateful sandbox run. Supports all sandbox run --fs profiles except external; use ext_ro_bash for read-only external operations or ext_rw_bash for external writes.",
    parameters: {{
      type: "object",
      properties: {{
        fs: {{ type: "string", description: "Sandbox filesystem profile: read-only, write-targets, build, git, or github-pr. external is not supported here." }},
        command: {{ type: "string", description: "Shell command to run inside the stateful sandbox." }},
        write_targets: {{ type: "array", items: {{ type: "string" }}, description: "Existing repo-relative file paths the command may write." }},
        create_targets: {{ type: "array", items: {{ type: "string" }}, description: "New repo-relative file paths the command may create." }},
        write_dirs: {{ type: "array", items: {{ type: "string" }}, description: "Repo-relative directories or build scratch purpose the command may write under." }},
        connect_sockets: {{ type: "array", items: {{ type: "string" }}, description: "Unix socket paths the sandbox may connect to when the selected profile supports sockets." }},
        allow_signal: {{ type: "boolean", description: "Allow the sandboxed command to signal approved processes when the selected profile supports signaling." }},
        network: {{ type: "string", description: "Network mode: enabled or disabled." }},
        timeout_seconds: {{ type: "number", description: "Positive integer timeout in seconds." }},
        async: {{ type: "boolean", description: "Deprecated compatibility field; commands now wait for completion before returning." }},
      }},
      required: ["fs", "command"],
    }},
    async execute(_toolCallId, params, signal, _onUpdate, ctx) {{
      let args;
      try {{
        args = sandboxBashArgs(params);
      }} catch (error) {{
        return sandboxToolError(error);
      }}
      return await runSandboxAwaitedTool(pi, params, args, ctx, "sandbox_bash");
    }},
  }});
  pi.registerTool({{
    name: "ext_ro_bash",
    label: "External Read-only Bash",
    description: "Run a read-only command through stateful sandbox run --fs external without OMP UI confirmation. Write, socket, and signal scopes are rejected.",
    parameters: {{
      type: "object",
      properties: {{
        purpose: {{ type: "string", description: "Human-readable purpose for the external read-only operation." }},
        command: {{ type: "string", description: "Shell command to run inside the external sandbox." }},
        network: {{ type: "string", description: "Network mode: enabled or disabled." }},
        timeout_seconds: {{ type: "number", description: "Positive integer timeout in seconds." }},
        async: {{ type: "boolean", description: "Deprecated compatibility field; commands now wait for completion before returning." }},
      }},
      required: ["purpose", "command"],
    }},
    async execute(_toolCallId, params, signal, _onUpdate, ctx) {{
      let args;
      try {{
        args = externalReadOnlyBashArgs(params);
      }} catch (error) {{
        return sandboxToolError(error);
      }}
      return await runSandboxAwaitedTool(pi, params, args, ctx, "ext_ro_bash");
    }},
  }});
  pi.registerTool({{
    name: "ext_rw_bash",
    label: "External Read/write Bash",
    description: "Run a command through stateful sandbox run --fs external after explicit OMP UI approval. At least one write_targets, create_targets, or write_dirs entry is required.",
    parameters: {{
      type: "object",
      properties: {{
        purpose: {{ type: "string", description: "Human-readable purpose for the external write operation." }},
        command: {{ type: "string", description: "Shell command to run inside the external sandbox." }},
        write_targets: {{ type: "array", items: {{ type: "string" }}, description: "Existing repo-relative or absolute external file paths the command may write. At least one write/create/directory scope is required." }},
        create_targets: {{ type: "array", items: {{ type: "string" }}, description: "New repo-relative or absolute external file paths the command may create. At least one write/create/directory scope is required." }},
        write_dirs: {{ type: "array", items: {{ type: "string" }}, description: "Repo-relative directories or absolute external directories the command may write under. At least one write/create/directory scope is required." }},
        connect_sockets: {{ type: "array", items: {{ type: "string" }}, description: "Optional absolute Unix socket paths the sandbox may connect to." }},
        allow_signal: {{ type: "boolean", description: "Optionally allow the sandboxed command to signal approved external processes." }},
        network: {{ type: "string", description: "Network mode: enabled or disabled." }},
        timeout_seconds: {{ type: "number", description: "Positive integer timeout in seconds." }},
        async: {{ type: "boolean", description: "Deprecated compatibility field; commands now wait for completion before returning." }},
      }},
      required: ["purpose", "command"],
    }},
    async execute(_toolCallId, params, signal, _onUpdate, ctx) {{
      let args;
      try {{
        args = externalReadWriteBashArgs(params);
      }} catch (error) {{
        return sandboxToolError(error);
      }}
      if (typeof ctx?.ui?.confirm !== "function") {{
        return {{
          isError: true,
          content: [{{ type: "text", text: "ext_rw_bash requires OMP UI confirmation, but ctx.ui.confirm is unavailable." }}],
          details: {{ error: "confirmation_unavailable" }},
        }};
      }}
      const approved = await ctx.ui.confirm(
        "Approve external sandbox command",
        externalBashApprovalMessage(params, args)
      );
      if (!approved) {{
        return {{
          isError: true,
          content: [{{ type: "text", text: "ext_rw_bash blocked by user" }}],
          details: {{ blocked: true }},
        }};
      }}
      return await runSandboxAwaitedTool(pi, params, args, ctx, "ext_rw_bash");
    }},
  }});
  pi.on("session_start", async (event, ctx) => {{
    const result = runStatefulHook("session-start", {{
      session_id: sessionId(event, ctx),
      cwd: ctx.cwd,
    }});
    startReservationStream(pi, result?.notifications_stream);
  }});
  pi.on("tool_call", async (event, ctx) => {{
    const decision = runStatefulHook("pre-tool-use", {{
      session_id: sessionId(event, ctx),
      cwd: ctx.cwd,
      yolo: isYolo(event, ctx),
      tool_name: event.toolName,
      tool_input: event.input || {{}},
    }});
    if (decision.decision === "prompt") {{
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
      return {{ block: true, reason: decision.reason }};
    }}
  }});
  pi.on("tool_result", async (event, ctx) => {{
    runStatefulHook("post-tool-use", {{
      session_id: sessionId(event, ctx),
      cwd: ctx.cwd,
      tool_name: event.toolName,
      tool_input: event.input || {{}},
    }});
  }});
  pi.on("session_shutdown", async (event, ctx) => {{
    stopReservationStream();
    runStatefulHook("stop", {{
      session_id: sessionId(event, ctx),
      cwd: ctx.cwd,
    }});
  }});
}};
"#
    );
    write_or_replace_text_file_with_mode(extension_path, &contents, private_file_mode())
}

fn write_omp_mcp_config(mcp_path: &Path, binary_path: &str) -> anyhow::Result<()> {
    let contents = serde_json::to_string_pretty(&json!({
        "mcpServers": {
            "stateful": {
                "type": "stdio",
                "command": binary_path,
                "args": ["mcp", "serve"]
            }
        }
    }))?;
    write_or_replace_text_file_with_mode(mcp_path, &format!("{contents}\n"), private_file_mode())
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
            default_omp_agent_dir_from_home("/tmp/home"),
            PathBuf::from("/tmp/home/.omp/profiles/stateful/agent")
        );
    }

    #[test]
    fn omp_install_places_stateful_rule_in_agent_rules_dir() {
        assert_eq!(
            omp_required_rule_path(Path::new("/tmp/home/.omp/profiles/stateful/agent")),
            PathBuf::from("/tmp/home/.omp/profiles/stateful/agent/rules/stateful-required.md")
        );
    }

    #[test]
    fn omp_stateful_required_rule_is_always_apply() {
        let rule = omp_stateful_required_rule();
        assert!(rule.contains("alwaysApply: true"));
        assert!(rule.contains("skill://stateful-command-policy"));
    }
}
