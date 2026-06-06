use std::{
    collections::BTreeSet,
    ffi::OsString,
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};

#[cfg(unix)]
use std::os::unix::fs::MetadataExt;
#[cfg(unix)]
use std::os::unix::process::CommandExt;

use serde_json::Value;
use stateful_mcp::{ToolCall, map_tool_to_http, protocol_tool_name, tool_descriptors};

use crate::{
    CurrentSession, GlobalPaths, HttpResponse, IntentDeclareArgs, ProtocolEnvelopeArgs, RepoGate,
    RepoIdentity, ServerRuntime, discover_runtime_with_global, ensure_server, get_json,
    intent_declare_protocol_body, post_json, protocol_envelope, read_current_session_file,
    repo_gate, repo_identity_for_enabled_repo,
};

pub fn call_mcp_tool_in_repo(
    repo_root: impl AsRef<Path>,
    tool_name: impl Into<String>,
    arguments: Value,
) -> anyhow::Result<HttpResponse> {
    let start = repo_root.as_ref();
    let paths = GlobalPaths::from_env()?;
    let tool_name = tool_name.into();
    let protocol_name = protocol_tool_name(&tool_name).map_err(anyhow::Error::msg)?;
    let repo_root = match repo_gate(&paths, start)? {
        RepoGate::Enabled { repo_root } => {
            match reject_mismatched_current_session(protocol_name, &arguments, &repo_root) {
                Ok(Some(response)) => return Ok(response),
                Ok(None) => {}
                Err(error) => return Ok(error_response(400, error.to_string())),
            }
            ensure_server(&paths)?;
            repo_root
        }
        RepoGate::Disabled | RepoGate::OutsideGitRepo => {
            return Ok(HttpResponse {
                status_code: 409,
                body: serde_json::json!({
                    "status": "error",
                    "message": "repo not enabled"
                })
                .to_string(),
            });
        }
    };
    let runtime = discover_runtime_with_global(&repo_root, &paths)?;
    call_mcp_tool(&runtime, &repo_root, &paths, tool_name, arguments)
}

fn call_mcp_tool(
    runtime: &ServerRuntime,
    repo_root: &Path,
    paths: &GlobalPaths,
    tool_name: impl Into<String>,
    arguments: Value,
) -> anyhow::Result<HttpResponse> {
    let tool_name = tool_name.into();
    let protocol_name = protocol_tool_name(&tool_name).map_err(anyhow::Error::msg)?;
    if protocol_name == "state.bash.write" {
        return call_bash_write_tool(runtime, repo_root, paths, arguments);
    }
    match reject_mismatched_current_session(protocol_name, &arguments, repo_root) {
        Ok(Some(response)) => return Ok(response),
        Ok(None) => {}
        Err(error) => return Ok(error_response(400, error.to_string())),
    }

    let tool = ToolCall::new(
        protocol_name,
        enrich_arguments(protocol_name, arguments, runtime, repo_root, paths),
    );
    let request = map_tool_to_http(tool).map_err(anyhow::Error::msg)?;

    match request.method {
        "GET" => get_json(runtime, request.path),
        "POST" => {
            let body = if protocol_name == "state.intent.declare" {
                intent_declare_mcp_body(runtime, request.body)?
            } else {
                request.body
            };
            post_json(runtime, request.path, &body)
        }
        method => anyhow::bail!("unsupported MCP HTTP method: {method}"),
    }
}

fn call_bash_write_tool(
    runtime: &ServerRuntime,
    repo_root: &Path,
    paths: &GlobalPaths,
    arguments: Value,
) -> anyhow::Result<HttpResponse> {
    let args = match serde_json::from_value::<BashWriteArguments>(arguments) {
        Ok(args) => args,
        Err(error) => {
            return Ok(error_response(
                400,
                format!("invalid state.bash.write arguments: {error}"),
            ));
        }
    };
    if args.command.trim().is_empty() {
        return Ok(error_response(400, "state.bash.write command is required"));
    }
    if let Err(error) = reject_detached_bash_constructs(&args.command) {
        return Ok(error_response(400, error.to_string()));
    }
    let timeout = match bash_write_timeout(args.timeout_seconds) {
        Ok(timeout) => timeout,
        Err(error) => return Ok(error_response(400, error.to_string())),
    };
    let write_targets = match normalize_bash_target_paths("write_targets", &args.write_targets) {
        Ok(paths) => paths,
        Err(error) => return Ok(error_response(400, error.to_string())),
    };
    let create_targets = match normalize_bash_target_paths("create_targets", &args.create_targets) {
        Ok(paths) => paths,
        Err(error) => return Ok(error_response(400, error.to_string())),
    };
    let current_session = match read_current_session_file_for_mcp(repo_root) {
        Ok(current_session) => current_session,
        Err(error) => return Ok(error_response(400, error.to_string())),
    };
    if let Some(response) = reject_argument_session_mismatch(
        "state.bash.write",
        args.session_id.as_deref(),
        args.workspace_id.as_deref(),
        current_session.as_ref(),
    ) {
        return Ok(response);
    }
    let session_id = match current_session
        .as_ref()
        .map(|session| session.session_id.clone())
        .or(args.session_id)
    {
        Some(session_id) => session_id,
        None => {
            return Ok(error_response(
                400,
                "state.bash.write requires session_id or a current stateful session file",
            ));
        }
    };
    let workspace_id = current_session
        .map(|session| session.workspace_id)
        .or(args.workspace_id)
        .unwrap_or_else(|| runtime.workspace_id.clone());

    let cwd = match resolve_bash_cwd(repo_root, args.cwd.as_deref()) {
        Ok(cwd) => cwd,
        Err(error) => return Ok(error_response(400, error.to_string())),
    };
    if let Err(error) = validate_bash_filesystem_targets(repo_root, &write_targets, &create_targets)
    {
        return Ok(error_response(400, error.to_string()));
    }

    let mut allowed = Vec::new();
    let mut denied = Vec::new();
    for path in write_targets.iter().chain(create_targets.iter()) {
        let response =
            authorize_file_write(runtime, repo_root, paths, &session_id, &workspace_id, path)?;
        let body = serde_json::from_str::<Value>(&response.body)
            .unwrap_or_else(|_| serde_json::json!({ "message": response.body }));
        if (200..300).contains(&response.status_code)
            && body.get("decision").and_then(Value::as_str) == Some("allow")
        {
            allowed.push(path.clone());
        } else {
            denied.push(serde_json::json!({
                "path": path,
                "authorization": body,
            }));
        }
    }

    if !denied.is_empty() {
        return Ok(HttpResponse {
            status_code: 403,
            body: serde_json::json!({
                "status": "error",
                "message": "state.bash.write target authorization denied",
                "allowed_write_targets": allowed,
                "denied_write_targets": denied,
            })
            .to_string(),
        });
    }

    let writable_files =
        match prepare_bash_writable_files(repo_root, &write_targets, &create_targets) {
            Ok(paths) => paths,
            Err(error) => return Ok(error_response(400, error.to_string())),
        };
    let run = match run_sandboxed_bash(&args.command, &cwd, &writable_files, timeout) {
        Ok(run) => run,
        Err(error) => return Ok(error_response(500, error.to_string())),
    };

    Ok(HttpResponse {
        status_code: 200,
        body: run.to_json(allowed).to_string(),
    })
}

const DEFAULT_BASH_WRITE_TIMEOUT_SECONDS: u64 = 300;
const MAX_BASH_WRITE_TIMEOUT_SECONDS: u64 = 600;
const MAX_BASH_WRITE_OUTPUT_BYTES: usize = 1024 * 1024;
const BASH_WRITE_SAFE_PATH: &str = "/usr/bin:/bin:/usr/sbin:/sbin";

fn bash_write_timeout(timeout_seconds: Option<u64>) -> anyhow::Result<Duration> {
    let seconds = timeout_seconds.unwrap_or(DEFAULT_BASH_WRITE_TIMEOUT_SECONDS);
    if seconds == 0 || seconds > MAX_BASH_WRITE_TIMEOUT_SECONDS {
        anyhow::bail!(
            "state.bash.write timeout_seconds must be between 1 and {MAX_BASH_WRITE_TIMEOUT_SECONDS}"
        );
    }
    Ok(Duration::from_secs(seconds))
}

fn reject_detached_bash_constructs(command: &str) -> anyhow::Result<()> {
    if has_detached_shell_syntax(command) {
        anyhow::bail!("state.bash.write refuses detached process constructs");
    }
    for segment in split_bash_segments(command) {
        let words = bash_words(&segment);
        if !words.is_empty() && words.iter().all(|word| is_env_assignment(word)) {
            anyhow::bail!("state.bash.write refuses detached process constructs");
        }
        if has_detached_sensitive_env_assignment(&words) {
            anyhow::bail!("state.bash.write refuses detached process constructs");
        }
        let Some(command_name) = bash_segment_command_name(&words) else {
            continue;
        };
        if command_name.contains('/') {
            anyhow::bail!("state.bash.write refuses detached process constructs");
        }
        let command_basename = command_name
            .rsplit('/')
            .next()
            .unwrap_or(command_name)
            .to_ascii_lowercase();
        if command_basename == "find" && find_invokes_external_command(&words) {
            anyhow::bail!("state.bash.write refuses detached process constructs");
        }
        if is_detached_capable_command(&command_basename) {
            anyhow::bail!("state.bash.write refuses detached process constructs");
        }
        if !is_bash_write_allowed_command(&command_basename) {
            anyhow::bail!("state.bash.write refuses detached process constructs");
        }
    }
    Ok(())
}

fn has_detached_shell_syntax(command: &str) -> bool {
    let mut quote = BashQuote::None;
    let mut escaped = false;
    let mut chars = command.char_indices().peekable();
    while let Some((index, ch)) = chars.next() {
        if escaped {
            if ch == '\n' || ch == '\r' {
                return true;
            }
            escaped = false;
            continue;
        }
        if ch == '\\' && quote != BashQuote::Single {
            escaped = true;
            continue;
        }
        update_bash_quote(ch, &mut quote);
        if quote == BashQuote::Single {
            continue;
        }
        if ch == '\n' || ch == '\r' || ch == '`' {
            return true;
        }
        if ch == '$'
            && (command[index..].starts_with("$(")
                || command[index..].starts_with("$'")
                || command[index..].starts_with("$\""))
        {
            return true;
        }
        if quote == BashQuote::None {
            if matches!(ch, '(' | ')' | '{' | '}') {
                return true;
            }
            if matches!(ch, '*' | '?' | '[') {
                return true;
            }
            if ch == '&' {
                if command[index..].starts_with("&&") {
                    let _ = chars.next();
                    continue;
                }
                return true;
            }
            if ch == '<' && command[index..].starts_with("<(") {
                return true;
            }
        }
    }
    false
}

fn is_detached_capable_command(command: &str) -> bool {
    matches!(
        command,
        "setsid"
            | "setpgid"
            | "setpgrp"
            | "disown"
            | "eval"
            | "source"
            | "."
            | "command"
            | "builtin"
            | "exec"
            | "time"
            | "env"
            | "trap"
            | "set"
            | "unset"
            | "export"
            | "readonly"
            | "alias"
            | "unalias"
            | "if"
            | "then"
            | "else"
            | "elif"
            | "fi"
            | "for"
            | "select"
            | "while"
            | "until"
            | "do"
            | "done"
            | "case"
            | "esac"
            | "coproc"
            | "("
            | "{"
            | "nohup"
            | "launchctl"
            | "crontab"
            | "at"
            | "batch"
            | "daemon"
            | "xargs"
            | "parallel"
            | "git"
            | "tar"
            | "rsync"
            | "ssh"
            | "scp"
            | "sftp"
            | "sqlite"
            | "sqlite3"
            | "arch"
            | "tclsh"
            | "expect"
            | "stdbuf"
            | "caffeinate"
            | "sh"
            | "bash"
            | "zsh"
            | "fish"
            | "perl"
            | "ruby"
            | "node"
            | "deno"
            | "bun"
            | "php"
            | "osascript"
            | "swift"
            | "lua"
            | "awk"
            | "gawk"
            | "mawk"
            | "tmux"
            | "screen"
    ) || command.starts_with("python")
}

fn is_bash_write_allowed_command(command: &str) -> bool {
    matches!(
        command,
        "printf"
            | "echo"
            | "cat"
            | "true"
            | "false"
            | "test"
            | "sleep"
            | "yes"
            | "head"
            | "tail"
            | "wc"
            | "pwd"
            | "date"
            | "mkdir"
            | "touch"
            | "rm"
            | "mv"
            | "chmod"
            | "dd"
            | "truncate"
            | "find"
    )
}

fn find_invokes_external_command(words: &[String]) -> bool {
    words.iter().any(|word| {
        word.contains('$') || matches!(word.as_str(), "-exec" | "-execdir" | "-ok" | "-okdir")
    })
}

fn split_bash_segments(command: &str) -> Vec<String> {
    let mut segments = Vec::new();
    let mut current = String::new();
    let mut quote = BashQuote::None;
    let mut escaped = false;
    let mut chars = command.char_indices().peekable();
    while let Some((index, ch)) = chars.next() {
        if escaped {
            current.push(ch);
            escaped = false;
            continue;
        }
        if ch == '\\' && quote != BashQuote::Single {
            current.push(ch);
            escaped = true;
            continue;
        }
        update_bash_quote(ch, &mut quote);
        if quote == BashQuote::None {
            if command[index..].starts_with("&&") || command[index..].starts_with("||") {
                segments.push(current);
                current = String::new();
                let _ = chars.next();
                continue;
            }
            if matches!(ch, ';' | '|') {
                segments.push(current);
                current = String::new();
                continue;
            }
        }
        current.push(ch);
    }
    segments.push(current);
    segments
}

fn bash_segment_command_name(words: &[String]) -> Option<&str> {
    let mut index = words.iter().position(|word| !is_env_assignment(word))?;
    loop {
        let word = words.get(index)?;
        let basename = word.rsplit('/').next().unwrap_or(word).to_ascii_lowercase();
        if basename == "nice" {
            index += 1;
            index = skip_nice_options(words, index);
            continue;
        }
        return Some(word);
    }
}

fn skip_nice_options(words: &[String], mut index: usize) -> usize {
    while let Some(word) = words.get(index) {
        if word == "-n" || word == "--adjustment" {
            index += 2;
            continue;
        }
        if word.starts_with("--adjustment=") {
            index += 1;
            continue;
        }
        if word.starts_with('-')
            && word
                .trim_start_matches('-')
                .chars()
                .all(|ch| ch.is_ascii_digit())
        {
            index += 1;
            continue;
        }
        break;
    }
    index
}

fn bash_words(input: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut current = String::new();
    let mut quote = BashQuote::None;
    let mut escaped = false;
    for ch in input.chars() {
        if escaped {
            current.push(ch);
            escaped = false;
            continue;
        }
        if ch == '\\' && quote != BashQuote::Single {
            escaped = true;
            continue;
        }
        update_bash_quote(ch, &mut quote);
        if quote == BashQuote::None && ch.is_ascii_whitespace() {
            if !current.is_empty() {
                words.push(current);
                current = String::new();
            }
            continue;
        }
        if matches!(ch, '\'' | '"') {
            continue;
        }
        current.push(ch);
    }
    if !current.is_empty() {
        words.push(current);
    }
    words
}

fn is_env_assignment(word: &str) -> bool {
    let Some((name, _value)) = word.split_once('=') else {
        return false;
    };
    !name.is_empty()
        && name
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
}

fn has_detached_sensitive_env_assignment(words: &[String]) -> bool {
    words
        .iter()
        .take_while(|word| is_env_assignment(word))
        .filter_map(|word| word.split_once('=').map(|(name, _value)| name))
        .any(|name| {
            name.starts_with("GIT_")
                || name.starts_with("LD_")
                || name.starts_with("DYLD_")
                || matches!(
                    name,
                    "PATH" | "BASH_ENV" | "ENV" | "IFS" | "SHELLOPTS" | "BASHOPTS" | "CDPATH"
                )
        })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BashQuote {
    None,
    Single,
    Double,
}

fn update_bash_quote(ch: char, quote: &mut BashQuote) {
    match (*quote, ch) {
        (BashQuote::None, '\'') => *quote = BashQuote::Single,
        (BashQuote::Single, '\'') => *quote = BashQuote::None,
        (BashQuote::None, '"') => *quote = BashQuote::Double,
        (BashQuote::Double, '"') => *quote = BashQuote::None,
        _ => {}
    }
}

fn reject_mismatched_current_session(
    protocol_name: &str,
    arguments: &Value,
    repo_root: &Path,
) -> anyhow::Result<Option<HttpResponse>> {
    if !is_session_bound_mcp_tool(protocol_name) {
        return Ok(None);
    }
    let Some(current_session) = read_current_session_file_for_mcp(repo_root)? else {
        return Ok(None);
    };
    let Some(object) = arguments.as_object() else {
        return Ok(None);
    };
    Ok(reject_argument_session_mismatch(
        protocol_name,
        object.get("session_id").and_then(Value::as_str),
        object.get("workspace_id").and_then(Value::as_str),
        Some(&current_session),
    ))
}

fn read_current_session_file_for_mcp(repo_root: &Path) -> anyhow::Result<Option<CurrentSession>> {
    match read_current_session_file(repo_root) {
        Ok(session) => Ok(Some(session)),
        Err(error) if is_not_found_error(&error) => Ok(None),
        Err(error) => Err(error),
    }
}

fn is_not_found_error(error: &anyhow::Error) -> bool {
    error
        .downcast_ref::<std::io::Error>()
        .is_some_and(|error| error.kind() == std::io::ErrorKind::NotFound)
}

fn reject_argument_session_mismatch(
    tool_name: &str,
    session_id: Option<&str>,
    workspace_id: Option<&str>,
    current_session: Option<&CurrentSession>,
) -> Option<HttpResponse> {
    let current_session = current_session?;
    if let Some(session_id) = session_id
        && session_id != current_session.session_id
    {
        return Some(current_session_mismatch_response(
            tool_name,
            "session_id",
            session_id,
            &current_session.session_id,
        ));
    }
    if let Some(workspace_id) = workspace_id
        && workspace_id != current_session.workspace_id
    {
        return Some(current_session_mismatch_response(
            tool_name,
            "workspace_id",
            workspace_id,
            &current_session.workspace_id,
        ));
    }
    None
}

fn current_session_mismatch_response(
    tool_name: &str,
    field: &str,
    requested: &str,
    current: &str,
) -> HttpResponse {
    error_response(
        403,
        format!(
            "{tool_name} cannot use {field} `{requested}` while the current stateful session uses `{current}`. This mismatch does not resolve active lease or reservation conflicts; wait, poll notifications, resume a reservation, or coordinate with the blocking session instead."
        ),
    )
}

fn is_session_bound_mcp_tool(protocol_name: &str) -> bool {
    matches!(
        protocol_name,
        "state.session.register"
            | "state.session.heartbeat"
            | "state.intent.declare"
            | "state.lease.acquire"
            | "state.lease.release"
            | "state.conflicts.check"
            | "state.reconcile.ack"
            | "state.bash.write"
            | "state.notifications.poll"
            | "state.resume.next"
    )
}

fn authorize_file_write(
    runtime: &ServerRuntime,
    repo_root: &Path,
    paths: &GlobalPaths,
    session_id: &str,
    workspace_id: &str,
    path: &str,
) -> anyhow::Result<HttpResponse> {
    let body = protocol_envelope(ProtocolEnvelopeArgs {
        runtime,
        request_id: uuid::Uuid::new_v4().to_string(),
        session_id: session_id.to_string(),
        workspace_id: workspace_id.to_string(),
        identity: repo_identity_for_enabled_repo(paths, repo_root).ok(),
        source_kind: "mcp",
        event: "file_write",
        source_ref: "state.bash.write",
        payload: serde_json::json!({
            "action": "write_file",
            "path": path,
            "queue_on_conflict": true,
        }),
    });

    post_json(runtime, "/v1/authorize", &body)
}

fn ensure_repo_file_target(repo_root: &Path, relative_path: &str) -> anyhow::Result<()> {
    let canonical_repo = repo_root
        .canonicalize()
        .unwrap_or_else(|_| repo_root.to_path_buf());
    let target = repo_root.join(relative_path);
    let Some(parent) = Path::new(relative_path).parent() else {
        anyhow::bail!("state.bash.write target has no parent directory");
    };

    let mut cursor = repo_root.to_path_buf();
    for component in parent.components() {
        cursor.push(component);
        if let Ok(metadata) = fs::symlink_metadata(&cursor) {
            if metadata.file_type().is_symlink() {
                anyhow::bail!("state.bash.write refuses symlinked parent directories");
            }
            if !metadata.is_dir() {
                anyhow::bail!("state.bash.write parent path is not a directory");
            }
        }
    }

    if let Ok(metadata) = fs::symlink_metadata(&target) {
        if metadata.file_type().is_symlink() {
            anyhow::bail!("state.bash.write refuses symlink file targets");
        }
        if metadata.is_dir() {
            anyhow::bail!("state.bash.write target is a directory");
        }
        if is_hard_linked_file_target(&metadata) {
            anyhow::bail!("state.bash.write refuses hard-linked file targets");
        }
    }

    if let Some(parent) = target.parent()
        && parent.exists()
    {
        let canonical_parent = parent.canonicalize()?;
        if !canonical_parent.starts_with(canonical_repo) {
            anyhow::bail!("state.bash.write parent path escapes the repo");
        }
    }

    Ok(())
}

#[cfg(unix)]
fn is_hard_linked_file_target(metadata: &fs::Metadata) -> bool {
    metadata.is_file() && metadata.nlink() > 1
}

#[cfg(not(unix))]
fn is_hard_linked_file_target(_metadata: &fs::Metadata) -> bool {
    false
}

fn normalize_bash_target_paths(field: &str, paths: &[String]) -> anyhow::Result<Vec<String>> {
    if field == "write_targets" && paths.is_empty() {
        anyhow::bail!("state.bash.write write_targets is required");
    }
    let mut seen = BTreeSet::new();
    let mut normalized = Vec::new();
    for path in paths {
        let path = normalize_bash_target_path(field, path)?;
        if seen.insert(path.clone()) {
            normalized.push(path);
        }
    }

    Ok(normalized)
}

fn normalize_bash_target_path(field: &str, path: &str) -> anyhow::Result<String> {
    if path.is_empty() {
        anyhow::bail!("state.bash.write {field} entries must not be empty");
    }
    if Path::new(path).is_absolute() {
        anyhow::bail!("state.bash.write {field} entries must be repo-relative");
    }

    let normalized = path.replace('\\', "/");
    let mut segments = Vec::new();
    for segment in normalized.split('/') {
        if segment.is_empty() || segment == "." {
            continue;
        }
        if segment == ".." {
            anyhow::bail!("state.bash.write {field} entries must stay inside the repo");
        }
        if segment.eq_ignore_ascii_case(".git") {
            anyhow::bail!("state.bash.write refuses Git internals");
        }
        if segment.chars().any(char::is_control) {
            anyhow::bail!("state.bash.write paths must not contain control characters");
        }
        segments.push(segment);
    }

    if segments.is_empty() {
        anyhow::bail!("state.bash.write {field} entries must not be empty");
    }

    Ok(segments.join("/"))
}

fn resolve_bash_cwd(repo_root: &Path, cwd: Option<&str>) -> anyhow::Result<PathBuf> {
    let canonical_repo = repo_root
        .canonicalize()
        .unwrap_or_else(|_| repo_root.to_path_buf());
    let Some(cwd) = cwd else {
        return Ok(canonical_repo);
    };
    if cwd.is_empty() || cwd == "." {
        return Ok(canonical_repo);
    }
    if Path::new(cwd).is_absolute() {
        anyhow::bail!("state.bash.write cwd must be repo-relative");
    }

    let normalized = cwd.replace('\\', "/");
    let mut relative = PathBuf::new();
    for segment in normalized.split('/') {
        if segment.is_empty() || segment == "." {
            continue;
        }
        if segment == ".." {
            anyhow::bail!("state.bash.write cwd must stay inside the repo");
        }
        if segment.eq_ignore_ascii_case(".git") {
            anyhow::bail!("state.bash.write refuses Git internals as cwd");
        }
        if segment.chars().any(char::is_control) {
            anyhow::bail!("state.bash.write cwd must not contain control characters");
        }
        relative.push(segment);
    }

    if relative.as_os_str().is_empty() {
        return Ok(canonical_repo);
    }

    let target = repo_root.join(relative);
    let canonical = target
        .canonicalize()
        .map_err(|error| anyhow::anyhow!("state.bash.write cwd must exist: {error}"))?;
    if !canonical.starts_with(&canonical_repo) {
        anyhow::bail!("state.bash.write cwd escapes the repo");
    }
    if !canonical.is_dir() {
        anyhow::bail!("state.bash.write cwd must be a directory");
    }

    Ok(canonical)
}

fn validate_bash_filesystem_targets(
    repo_root: &Path,
    write_targets: &[String],
    create_targets: &[String],
) -> anyhow::Result<()> {
    let create_set = create_targets
        .iter()
        .map(String::as_str)
        .collect::<BTreeSet<_>>();

    for path in create_targets {
        ensure_repo_file_target(repo_root, path).map_err(|error| {
            anyhow::anyhow!("state.bash.write create target `{path}` is unsafe: {error}")
        })?;
    }

    for path in write_targets {
        ensure_repo_file_target(repo_root, path).map_err(|error| {
            anyhow::anyhow!("state.bash.write write target `{path}` is unsafe: {error}")
        })?;
        let target = repo_root.join(path);
        if !target.exists() && !create_set.contains(path.as_str()) {
            anyhow::bail!(
                "state.bash.write write target `{path}` must already exist or be listed in create_targets"
            );
        }
    }

    Ok(())
}

fn prepare_bash_writable_files(
    repo_root: &Path,
    write_targets: &[String],
    create_targets: &[String],
) -> anyhow::Result<Vec<PathBuf>> {
    validate_bash_filesystem_targets(repo_root, write_targets, create_targets)?;

    for path in create_targets {
        let target = repo_root.join(path);
        let Some(parent) = target.parent() else {
            anyhow::bail!("state.bash.write create target `{path}` has no parent directory");
        };
        fs::create_dir_all(parent)?;
        if !target.exists() {
            fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&target)?;
        }
    }

    let mut seen = BTreeSet::new();
    let mut writable_files = Vec::new();
    for path in write_targets.iter().chain(create_targets.iter()) {
        if !seen.insert(path.clone()) {
            continue;
        }
        let canonical = repo_root.join(path).canonicalize().map_err(|error| {
            anyhow::anyhow!("state.bash.write target `{path}` must exist: {error}")
        })?;
        writable_files.push(canonical);
    }

    Ok(writable_files)
}

fn run_sandboxed_bash(
    command: &str,
    cwd: &Path,
    writable_files: &[PathBuf],
    timeout: Duration,
) -> anyhow::Result<BashWriteRun> {
    #[cfg(target_os = "macos")]
    {
        run_command_with_timeout(seatbelt_command(command, cwd, writable_files), timeout)
    }

    #[cfg(target_os = "linux")]
    {
        run_command_with_timeout(bubblewrap_command(command, cwd, writable_files), timeout)
    }

    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        let _ = (command, cwd, writable_files, timeout);
        anyhow::bail!("state.bash.write is only supported on macOS and Linux");
    }
}

#[cfg(target_os = "macos")]
fn seatbelt_command(command: &str, cwd: &Path, writable_files: &[PathBuf]) -> Command {
    let profile = seatbelt_profile(writable_files);
    let mut sandbox = Command::new("/usr/bin/sandbox-exec");
    sandbox
        .arg("-p")
        .arg(profile)
        .arg("/bin/sh")
        .arg("-c")
        .arg(command)
        .current_dir(cwd);
    configure_bash_write_environment(&mut sandbox);
    sandbox
}

#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn seatbelt_profile(writable_files: &[PathBuf]) -> String {
    let mut profile = String::from(
        "(version 1)\n(deny default)\n(allow process*)\n(allow file-read*)\n(allow file-write*",
    );
    profile.push_str(" (literal \"/dev/null\")");
    for file in writable_files {
        profile.push_str(" (literal \"");
        profile.push_str(&seatbelt_escape(&file.to_string_lossy()));
        profile.push_str("\")");
    }
    profile.push_str(")\n");
    profile
}

#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn seatbelt_escape(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}

#[cfg(target_os = "linux")]
fn bubblewrap_command(command: &str, cwd: &Path, writable_files: &[PathBuf]) -> Command {
    let mut bwrap = Command::new("bwrap");
    bwrap.args(bubblewrap_args(command, cwd, writable_files));
    configure_bash_write_environment(&mut bwrap);
    bwrap
}

fn configure_bash_write_environment(command: &mut Command) {
    command
        .env_clear()
        .env("PATH", BASH_WRITE_SAFE_PATH)
        .env("LC_ALL", "C");
}

#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn bubblewrap_args(command: &str, cwd: &Path, writable_files: &[PathBuf]) -> Vec<OsString> {
    let mut args = vec![
        OsString::from("--unshare-all"),
        OsString::from("--die-with-parent"),
        OsString::from("--unshare-net"),
        OsString::from("--ro-bind"),
        OsString::from("/"),
        OsString::from("/"),
        OsString::from("--proc"),
        OsString::from("/proc"),
    ];

    for file in writable_files {
        args.push(OsString::from("--bind"));
        args.push(file.as_os_str().to_owned());
        args.push(file.as_os_str().to_owned());
    }

    args.push(OsString::from("--chdir"));
    args.push(cwd.as_os_str().to_owned());
    args.push(OsString::from("--"));
    args.push(OsString::from("/bin/sh"));
    args.push(OsString::from("-c"));
    args.push(OsString::from(command));

    args
}

fn run_command_with_timeout(
    mut command: Command,
    timeout: Duration,
) -> anyhow::Result<BashWriteRun> {
    command.stdout(Stdio::piped()).stderr(Stdio::piped());
    configure_child_process_group(&mut command);
    let mut child = command.spawn()?;
    let child_process_group = child.id();
    let mut stdout = child
        .stdout
        .take()
        .ok_or_else(|| anyhow::anyhow!("failed to capture sandbox stdout"))?;
    let mut stderr = child
        .stderr
        .take()
        .ok_or_else(|| anyhow::anyhow!("failed to capture sandbox stderr"))?;
    let stdout_reader =
        thread::spawn(move || read_limited_output(&mut stdout, MAX_BASH_WRITE_OUTPUT_BYTES));
    let stderr_reader =
        thread::spawn(move || read_limited_output(&mut stderr, MAX_BASH_WRITE_OUTPUT_BYTES));

    let deadline = Instant::now() + timeout;
    let mut timed_out = false;
    let exit_status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if Instant::now() >= deadline {
            timed_out = true;
            kill_child_process_group(child_process_group);
            match child.kill() {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::InvalidInput => {}
                Err(error) => return Err(error.into()),
            }
            break child.wait()?;
        }
        thread::sleep(Duration::from_millis(25));
    };

    kill_child_process_group(child_process_group);

    let stdout = join_output_reader(stdout_reader, "stdout", deadline)?;
    let stderr = join_output_reader(stderr_reader, "stderr", deadline)?;

    Ok(BashWriteRun {
        status: if timed_out { "timed_out" } else { "exited" },
        exit_code: exit_status.code(),
        stdout: String::from_utf8_lossy(&stdout.bytes).into_owned(),
        stderr: String::from_utf8_lossy(&stderr.bytes).into_owned(),
        stdout_truncated: stdout.truncated,
        stderr_truncated: stderr.truncated,
    })
}

fn read_limited_output(
    reader: &mut impl Read,
    max_bytes: usize,
) -> std::io::Result<CapturedOutput> {
    let mut bytes = Vec::new();
    let mut truncated = false;
    let mut buffer = [0_u8; 8192];
    loop {
        let read = reader.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        let remaining = max_bytes.saturating_sub(bytes.len());
        if remaining == 0 {
            truncated = true;
            continue;
        }
        let to_store = read.min(remaining);
        bytes.extend_from_slice(&buffer[..to_store]);
        if to_store < read {
            truncated = true;
        }
    }
    Ok(CapturedOutput { bytes, truncated })
}

#[cfg(unix)]
fn configure_child_process_group(command: &mut Command) {
    command.process_group(0);
}

#[cfg(not(unix))]
fn configure_child_process_group(_command: &mut Command) {}

fn kill_child_process_group(process_group_id: u32) {
    #[cfg(unix)]
    {
        for program in ["/bin/kill", "/usr/bin/kill"] {
            if !Path::new(program).exists() {
                continue;
            }
            let _ = Command::new(program)
                .env_clear()
                .arg("-KILL")
                .arg(format!("-{process_group_id}"))
                .status();
            break;
        }
    }

    #[cfg(not(unix))]
    {
        let _ = process_group_id;
    }
}

fn join_output_reader(
    reader: thread::JoinHandle<std::io::Result<CapturedOutput>>,
    stream_name: &str,
    deadline: Instant,
) -> anyhow::Result<CapturedOutput> {
    while !reader.is_finished() {
        if Instant::now() >= deadline {
            anyhow::bail!("sandbox {stream_name} reader did not finish before command timeout");
        }
        thread::sleep(Duration::from_millis(10));
    }
    reader
        .join()
        .map_err(|_| anyhow::anyhow!("sandbox {stream_name} reader panicked"))?
        .map_err(Into::into)
}

struct CapturedOutput {
    bytes: Vec<u8>,
    truncated: bool,
}

struct BashWriteRun {
    status: &'static str,
    exit_code: Option<i32>,
    stdout: String,
    stderr: String,
    stdout_truncated: bool,
    stderr_truncated: bool,
}

impl BashWriteRun {
    fn to_json(&self, allowed_write_targets: Vec<String>) -> Value {
        serde_json::json!({
            "status": self.status,
            "exit_code": self.exit_code,
            "stdout": self.stdout,
            "stderr": self.stderr,
            "stdout_truncated": self.stdout_truncated,
            "stderr_truncated": self.stderr_truncated,
            "allowed_write_targets": allowed_write_targets,
            "denied_write_targets": [],
        })
    }
}

fn error_response(status_code: u16, message: impl Into<String>) -> HttpResponse {
    HttpResponse {
        status_code,
        body: serde_json::json!({
            "status": "error",
            "message": message.into()
        })
        .to_string(),
    }
}

fn intent_declare_mcp_body(runtime: &ServerRuntime, body: Value) -> anyhow::Result<Value> {
    let Value::Object(mut object) = body else {
        anyhow::bail!("state.intent.declare arguments must be an object");
    };

    let files_planned = object
        .remove("files_planned")
        .ok_or_else(|| anyhow::anyhow!("state.intent.declare requires files_planned"))
        .and_then(|value| serde_json::from_value::<Vec<String>>(value).map_err(Into::into))?;
    let session_id = take_string(&mut object, "session_id")
        .unwrap_or_else(|| format!("stateful-mcp:{}", runtime.pid));
    let workspace_id =
        take_string(&mut object, "workspace_id").unwrap_or_else(|| runtime.workspace_id.clone());
    let identity = repo_identity_from_object(&mut object);

    Ok(intent_declare_protocol_body(
        runtime,
        IntentDeclareArgs {
            session_id,
            workspace_id,
            files_planned,
            identity,
        },
        "mcp",
        "state.intent.declare",
    ))
}

fn repo_identity_from_object(object: &mut serde_json::Map<String, Value>) -> Option<RepoIdentity> {
    Some(RepoIdentity {
        repo_id: take_string(object, "repo_id")?,
        worktree_id: take_string(object, "worktree_id")?,
        root: take_string(object, "root")?,
        branch: take_string(object, "branch")?,
    })
}

fn take_string(object: &mut serde_json::Map<String, Value>, key: &str) -> Option<String> {
    object
        .remove(key)
        .and_then(|value| value.as_str().map(ToOwned::to_owned))
}

fn enrich_arguments(
    tool_name: &str,
    arguments: Value,
    runtime: &ServerRuntime,
    repo_root: &Path,
    paths: &GlobalPaths,
) -> Value {
    let Value::Object(mut object) = arguments else {
        return arguments;
    };

    if tool_name == "state.intent.declare" {
        if !object.contains_key("session_id")
            && let Ok(session) = read_current_session_file(repo_root)
        {
            object.insert("session_id".to_string(), Value::String(session.session_id));
            object
                .entry("workspace_id")
                .or_insert_with(|| Value::String(session.workspace_id));
        }
        object
            .entry("workspace_id")
            .or_insert_with(|| Value::String(runtime.workspace_id.clone()));
        add_repo_identity(&mut object, paths, repo_root);
    }

    Value::Object(object)
}

fn add_repo_identity(
    object: &mut serde_json::Map<String, Value>,
    paths: &GlobalPaths,
    repo_root: &Path,
) {
    let Ok(identity) = repo_identity_for_enabled_repo(paths, repo_root) else {
        return;
    };
    object
        .entry("repo_id")
        .or_insert_with(|| Value::String(identity.repo_id));
    object
        .entry("worktree_id")
        .or_insert_with(|| Value::String(identity.worktree_id));
    object
        .entry("root")
        .or_insert_with(|| Value::String(identity.root));
    object
        .entry("branch")
        .or_insert_with(|| Value::String(identity.branch));
}

#[derive(Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct BashWriteArguments {
    #[serde(default)]
    session_id: Option<String>,
    #[serde(default)]
    workspace_id: Option<String>,
    command: String,
    write_targets: Vec<String>,
    #[serde(default)]
    create_targets: Vec<String>,
    #[serde(default)]
    cwd: Option<String>,
    #[serde(default)]
    timeout_seconds: Option<u64>,
}

pub fn handle_mcp_jsonrpc_in_repo(
    repo_root: impl AsRef<Path>,
    message: &str,
) -> anyhow::Result<Option<String>> {
    let repo_root = repo_root.as_ref();
    let request: Value = match serde_json::from_str(message) {
        Ok(request) => request,
        Err(error) => {
            let response = jsonrpc_error(Value::Null, -32700, format!("invalid JSON: {error}"));
            return Ok(Some(serde_json::to_string(&response)?));
        }
    };
    let id = request.get("id").cloned().unwrap_or(Value::Null);
    let Some(method) = request.get("method").and_then(Value::as_str) else {
        let response = jsonrpc_error(id, -32600, "MCP request missing method".to_string());
        return Ok(Some(serde_json::to_string(&response)?));
    };

    let response = match method {
        "initialize" => jsonrpc_result(
            id,
            serde_json::json!({
                "protocolVersion": "2024-11-05",
                "capabilities": {
                    "tools": {}
                },
                "serverInfo": {
                    "name": "stateful",
                    "version": env!("CARGO_PKG_VERSION")
                }
            }),
        ),
        "notifications/initialized" => return Ok(None),
        "tools/list" => jsonrpc_result(
            id,
            serde_json::json!({
                "tools": tool_descriptors()
                    .into_iter()
                    .map(|tool| serde_json::json!({
                        "name": tool.name,
                        "description": tool.description,
                        "inputSchema": tool.input_schema
                    }))
                    .collect::<Vec<_>>()
            }),
        ),
        "tools/call" => {
            let params = request
                .get("params")
                .cloned()
                .unwrap_or_else(|| serde_json::json!({}));
            let Some(name) = params.get("name").and_then(Value::as_str) else {
                return Ok(Some(serde_json::to_string(&jsonrpc_error(
                    id,
                    -32602,
                    "MCP tools/call missing params.name".to_string(),
                ))?));
            };
            let arguments = params
                .get("arguments")
                .cloned()
                .unwrap_or_else(|| serde_json::json!({}));
            let response = match call_mcp_tool_in_repo(repo_root, name, arguments) {
                Ok(response) => response,
                Err(error) => error_response(500, error.to_string()),
            };
            jsonrpc_result(
                id,
                serde_json::json!({
                    "content": [{
                        "type": "text",
                        "text": response.body
                    }],
                    "isError": !(200..300).contains(&response.status_code)
                }),
            )
        }
        _ => jsonrpc_error(id, -32601, format!("unknown MCP method: {method}")),
    };

    Ok(Some(serde_json::to_string(&response)?))
}

pub fn serve_mcp_stdio_in_repo(
    repo_root: impl AsRef<Path>,
    mut input: impl Read,
    mut output: impl Write,
) -> anyhow::Result<()> {
    let repo_root = repo_root.as_ref();
    while let Some(message) = read_mcp_message(&mut input)? {
        if let Some(response) = handle_mcp_jsonrpc_in_repo(repo_root, &message.body)? {
            write_mcp_message(&mut output, &response, message.framing)?;
        }
    }

    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum McpFraming {
    ContentLength,
    JsonLine,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct McpMessage {
    body: String,
    framing: McpFraming,
}

fn jsonrpc_result(id: Value, result: Value) -> Value {
    serde_json::json!({
        "jsonrpc": "2.0",
        "id": id,
        "result": result
    })
}

fn jsonrpc_error(id: Value, code: i64, message: impl Into<String>) -> Value {
    serde_json::json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": {
            "code": code,
            "message": message.into()
        }
    })
}

fn read_mcp_message(input: &mut impl Read) -> anyhow::Result<Option<McpMessage>> {
    let Some(first_line) = read_line(input)? else {
        return Ok(None);
    };
    let first_line_trimmed = first_line.trim_end_matches(&['\r', '\n'][..]);

    if !first_line_trimmed
        .to_ascii_lowercase()
        .starts_with("content-length:")
    {
        return Ok(Some(McpMessage {
            body: first_line_trimmed.to_string(),
            framing: McpFraming::JsonLine,
        }));
    }

    let mut headers = first_line;
    loop {
        let Some(line) = read_line(input)? else {
            anyhow::bail!("unexpected EOF while reading MCP headers");
        };
        let is_blank = line == "\n" || line == "\r\n";
        headers.push_str(&line);
        if is_blank {
            break;
        }
    }

    let content_length = headers
        .lines()
        .find_map(|line| {
            line.to_ascii_lowercase()
                .strip_prefix("content-length:")
                .map(|value| value.trim().to_string())
        })
        .ok_or_else(|| anyhow::anyhow!("missing MCP Content-Length header"))?
        .parse::<usize>()?;
    let mut body = vec![0_u8; content_length];
    input.read_exact(&mut body)?;

    Ok(Some(McpMessage {
        body: String::from_utf8(body)?,
        framing: McpFraming::ContentLength,
    }))
}

fn read_line(input: &mut impl Read) -> anyhow::Result<Option<String>> {
    let mut line = Vec::new();
    let mut byte = [0_u8; 1];
    loop {
        let read = input.read(&mut byte)?;
        if read == 0 {
            return if line.is_empty() {
                Ok(None)
            } else {
                Ok(Some(String::from_utf8(line)?))
            };
        }
        line.push(byte[0]);
        if byte[0] == b'\n' {
            return Ok(Some(String::from_utf8(line)?));
        }
    }
}

fn write_mcp_message(
    output: &mut impl Write,
    message: &str,
    framing: McpFraming,
) -> anyhow::Result<()> {
    match framing {
        McpFraming::ContentLength => {
            write!(
                output,
                "Content-Length: {}\r\n\r\n{}",
                message.len(),
                message
            )?;
        }
        McpFraming::JsonLine => {
            writeln!(output, "{message}")?;
        }
    }
    output.flush()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bubblewrap_args_mount_root_read_only_and_bind_authorized_files() {
        let cwd = Path::new("/repo");
        let writable_files = vec![
            PathBuf::from("/repo/src/allowed.ts"),
            PathBuf::from("/repo/src/new.ts"),
        ];

        let args = bubblewrap_args("printf ok", cwd, &writable_files);
        let args = args
            .into_iter()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect::<Vec<_>>();

        assert!(args.starts_with(&[
            "--unshare-all".to_string(),
            "--die-with-parent".to_string(),
            "--unshare-net".to_string(),
            "--ro-bind".to_string(),
            "/".to_string(),
            "/".to_string(),
            "--proc".to_string(),
            "/proc".to_string(),
        ]));
        assert!(args.windows(3).any(|window| {
            window == ["--bind", "/repo/src/allowed.ts", "/repo/src/allowed.ts"]
        }));
        assert!(
            args.windows(3)
                .any(|window| { window == ["--bind", "/repo/src/new.ts", "/repo/src/new.ts"] })
        );
        assert!(args.windows(2).any(|window| window == ["--chdir", "/repo"]));
        assert!(args.ends_with(&[
            "--".to_string(),
            "/bin/sh".to_string(),
            "-c".to_string(),
            "printf ok".to_string(),
        ]));
    }

    #[test]
    fn seatbelt_profile_allows_only_literal_write_targets() {
        let profile = seatbelt_profile(&[
            PathBuf::from("/repo/src/allowed.ts"),
            PathBuf::from("/repo/src/quoted\"path.ts"),
        ]);

        assert!(profile.contains("(deny default)"));
        assert!(profile.contains("(allow file-read*)"));
        assert!(profile.contains("(literal \"/repo/src/allowed.ts\")"));
        assert!(profile.contains("(literal \"/repo/src/quoted\\\"path.ts\")"));
        assert!(!profile.contains("subpath \"/repo/src\""));
    }

    #[test]
    fn seatbelt_profile_allows_dev_null_as_a_literal_write_target() {
        let profile = seatbelt_profile(&[PathBuf::from("/repo/src/allowed.ts")]);

        assert!(profile.contains("(literal \"/dev/null\")"));
        assert!(!profile.contains("subpath \"/dev\""));
    }
}
