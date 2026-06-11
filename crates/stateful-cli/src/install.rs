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

fn merge_codex_config_contents(existing: &str, binary_path: &str) -> anyhow::Result<String> {
    let stripped = strip_stateful_block(existing)?;
    ensure_no_unmarked_stateful_mcp(&stripped)?;
    let feature_update = ensure_hooks_feature_enabled(&stripped)?;
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

    normalized == "features" || normalized == "mcp_servers.stateful"
}

fn quoted_header_may_affect_stateful(header: &str) -> bool {
    toml_table_key_segments(header)
        .map(|segments| segments == ["features"] || segments == ["mcp_servers", "stateful"])
        .unwrap_or_else(|| unsupported_header_may_affect_stateful(header))
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

fn ensure_no_unmarked_stateful_mcp(contents: &str) -> anyhow::Result<()> {
    for line in contents.lines() {
        if toml_table_header(line.trim())?.and_then(TomlTableHeader::simple_name)
            == Some("mcp_servers.stateful")
        {
            anyhow::bail!(
                "Codex config already contains unmarked [mcp_servers.stateful]; remove it or install into a config without that conflict"
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
    let hook_prefix = format!("{quoted_binary} hook");
    let features_section = if include_features_section {
        "[features]\nhooks = true\n\n"
    } else {
        ""
    };

    Ok(format!(
        r#"{GLOBAL_CODEX_BLOCK_START}
{features_section}[mcp_servers.stateful]
command = {}
args = ["mcp", "serve"]
env_vars = ["STATEFUL_SESSION_ID", "STATEFUL_SERVER_URL", "STATEFUL_SERVER_TOKEN"]
startup_timeout_sec = 20

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
statusMessage = "Recording stateful activity"
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
