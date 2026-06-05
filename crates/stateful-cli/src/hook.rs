use std::{
    io::{self, Read},
    path::{Component, Path, PathBuf},
};

use serde::Deserialize;
use serde_json::json;
use stateful_core::{BashKind, classify_bash};

use crate::codex_wrapper::STATEFUL_TRUSTED_SANDBOX_ENV;
use crate::outbox::queue_session_heartbeat_outbox;
use crate::{
    CurrentSession, GlobalPaths, HookCommand, RepoGate, RepoIdentity, ServerRuntime,
    discover_runtime_with_global, ensure_server, post_json, protocol_envelope, repo_gate,
    repo_identity_for_enabled_repo, write_current_session_file,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HookOutcome {
    Allow,
    Deny { reason: String },
}

impl HookOutcome {
    pub fn to_stdout_json(&self) -> serde_json::Result<serde_json::Value> {
        match self {
            Self::Allow => Ok(json!({})),
            Self::Deny { reason } => Ok(json!({
                "hookSpecificOutput": {
                    "hookEventName": "PreToolUse",
                    "permissionDecision": "deny",
                    "permissionDecisionReason": reason,
                }
            })),
        }
    }
}

pub fn run_hook(command: HookCommand) -> anyhow::Result<()> {
    match command {
        HookCommand::SessionStart => {
            let mut input = String::new();
            io::stdin().read_to_string(&mut input)?;
            if let Err(error) = handle_session_start_in_repo(&input, hook_start_dir(&input)?) {
                eprintln!("stateful session-start warning: {error}");
            }
        }
        HookCommand::PostToolUse => {
            let mut input = String::new();
            io::stdin().read_to_string(&mut input)?;
            if let Err(error) = handle_post_tool_use_in_repo(&input, hook_start_dir(&input)?) {
                eprintln!("stateful post-tool-use warning: {error}");
            }
        }
        HookCommand::UserPromptSubmit => {
            let mut input = String::new();
            io::stdin().read_to_string(&mut input)?;
            match handle_user_prompt_submit_in_repo(&input, hook_start_dir(&input)?) {
                Ok(prompt_text) if !prompt_text.is_empty() => println!("{prompt_text}"),
                Ok(_) => {}
                Err(error) => eprintln!("stateful user-prompt-submit warning: {error}"),
            }
        }
        HookCommand::Stop => {
            let mut input = String::new();
            io::stdin().read_to_string(&mut input)?;
            if let Err(error) = handle_stop_in_repo(&input, hook_start_dir(&input)?) {
                eprintln!("stateful stop warning: {error}");
            }
        }
        HookCommand::PreToolUse => {
            let mut input = String::new();
            io::stdin().read_to_string(&mut input)?;
            let outcome = handle_pre_tool_use_in_repo(&input, hook_start_dir(&input)?)?;
            if !matches!(outcome, HookOutcome::Allow) {
                println!("{}", serde_json::to_string(&outcome.to_stdout_json()?)?);
            }
        }
    }

    Ok(())
}

pub fn handle_pre_tool_use(input: &str) -> anyhow::Result<HookOutcome> {
    handle_pre_tool_use_with_runtime(input, None, None, None)
}

pub fn handle_pre_tool_use_with_trusted_sandbox(
    input: &str,
    trusted_sandbox: Option<serde_json::Value>,
) -> anyhow::Result<HookOutcome> {
    handle_pre_tool_use_with_runtime_and_sandbox(input, None, None, None, trusted_sandbox.as_ref())
}

pub fn handle_pre_tool_use_in_repo(
    input: &str,
    repo_root: impl AsRef<Path>,
) -> anyhow::Result<HookOutcome> {
    let start = hook_start_dir_or(input, repo_root.as_ref());
    let paths = GlobalPaths::from_env()?;
    let repo_root = match repo_gate(&paths, &start)? {
        RepoGate::Enabled { repo_root } => {
            ensure_server(&paths)?;
            repo_root
        }
        RepoGate::Disabled | RepoGate::OutsideGitRepo => return Ok(HookOutcome::Allow),
    };
    let runtime = discover_runtime_with_global(&repo_root, &paths)?;
    remember_current_session(&repo_root, &runtime, input)?;
    handle_pre_tool_use_with_runtime(
        input,
        Some(&runtime),
        Some(&repo_root),
        Some(start.as_path()),
    )
}

pub fn handle_session_start_in_repo(
    input: &str,
    repo_root: impl AsRef<Path>,
) -> anyhow::Result<()> {
    let start = hook_start_dir_or(input, repo_root.as_ref());
    let paths = GlobalPaths::from_env()?;
    let repo_root = match repo_gate(&paths, start)? {
        RepoGate::Enabled { repo_root } => {
            ensure_server(&paths)?;
            repo_root
        }
        RepoGate::Disabled | RepoGate::OutsideGitRepo => return Ok(()),
    };
    let runtime = discover_runtime_with_global(&repo_root, &paths)?;
    remember_current_session(&repo_root, &runtime, input)?;
    let identity = repo_identity(&paths, &repo_root)?;
    handle_session_start_with_runtime(input, &runtime, Some(&identity))
}

pub fn handle_post_tool_use_in_repo(
    input: &str,
    repo_root: impl AsRef<Path>,
) -> anyhow::Result<()> {
    let start = hook_start_dir_or(input, repo_root.as_ref());
    let paths = GlobalPaths::from_env()?;
    let repo_root = match repo_gate(&paths, start)? {
        RepoGate::Enabled { repo_root } => {
            ensure_server(&paths)?;
            repo_root
        }
        RepoGate::Disabled | RepoGate::OutsideGitRepo => return Ok(()),
    };
    let runtime = discover_runtime_with_global(&repo_root, &paths)?;
    remember_current_session(&repo_root, &runtime, input)?;
    let identity = repo_identity(&paths, &repo_root)?;
    if let Err(error) = handle_post_tool_use_with_runtime(input, &runtime, Some(&identity)) {
        let input: SessionEventInput = serde_json::from_str(input)?;
        queue_session_heartbeat_outbox(
            &repo_root,
            &runtime.workspace_id,
            &input.session_id,
            &error.to_string(),
        )?;
    }

    Ok(())
}

pub fn handle_user_prompt_submit_in_repo(
    input: &str,
    repo_root: impl AsRef<Path>,
) -> anyhow::Result<String> {
    let start = hook_start_dir_or(input, repo_root.as_ref());
    let paths = GlobalPaths::from_env()?;
    let repo_root = match repo_gate(&paths, start)? {
        RepoGate::Enabled { repo_root } => {
            ensure_server(&paths)?;
            repo_root
        }
        RepoGate::Disabled | RepoGate::OutsideGitRepo => return Ok(String::new()),
    };
    let runtime = discover_runtime_with_global(&repo_root, &paths)?;
    remember_current_session(&repo_root, &runtime, input)?;
    handle_user_prompt_submit_with_runtime(input, &runtime)
}

pub fn handle_stop_in_repo(input: &str, repo_root: impl AsRef<Path>) -> anyhow::Result<()> {
    let start = hook_start_dir_or(input, repo_root.as_ref());
    let paths = GlobalPaths::from_env()?;
    let repo_root = match repo_gate(&paths, start)? {
        RepoGate::Enabled { repo_root } => {
            ensure_server(&paths)?;
            repo_root
        }
        RepoGate::Disabled | RepoGate::OutsideGitRepo => return Ok(()),
    };
    let runtime = discover_runtime_with_global(&repo_root, &paths)?;
    remember_current_session(&repo_root, &runtime, input)?;
    let identity = repo_identity(&paths, &repo_root)?;
    handle_stop_with_runtime(input, &runtime, Some(&identity))
}

fn remember_current_session(
    repo_root: &Path,
    runtime: &ServerRuntime,
    input: &str,
) -> anyhow::Result<()> {
    let input: SessionEventInput = serde_json::from_str(input)?;
    write_current_session_file(
        repo_root,
        &CurrentSession::new(input.session_id, runtime.workspace_id.clone()),
    )
}

fn handle_session_start_with_runtime(
    input: &str,
    runtime: &ServerRuntime,
    identity: Option<&RepoIdentity>,
) -> anyhow::Result<()> {
    let input: SessionStartInput = serde_json::from_str(input)?;
    post_session_event(runtime, "/v1/session/register", &input.session_id, identity)
}

fn handle_post_tool_use_with_runtime(
    input: &str,
    runtime: &ServerRuntime,
    identity: Option<&RepoIdentity>,
) -> anyhow::Result<()> {
    let input: SessionEventInput = serde_json::from_str(input)?;
    post_session_event(
        runtime,
        "/v1/session/heartbeat",
        &input.session_id,
        identity,
    )
}

fn handle_user_prompt_submit_with_runtime(
    input: &str,
    runtime: &ServerRuntime,
) -> anyhow::Result<String> {
    let input: UserPromptSubmitInput = serde_json::from_str(input)?;
    let response = post_json(
        runtime,
        "/v1/context/render",
        &json!({
            "session_id": input.session_id,
            "mode": "brief"
        }),
    )?;

    if !(200..300).contains(&response.status_code) {
        anyhow::bail!(
            "context render failed with HTTP {}: {}",
            response.status_code,
            response.body
        );
    }

    let response: ContextRenderResponse = serde_json::from_str(&response.body)?;
    Ok(response.prompt_text)
}

fn handle_stop_with_runtime(
    input: &str,
    runtime: &ServerRuntime,
    identity: Option<&RepoIdentity>,
) -> anyhow::Result<()> {
    let input: SessionEventInput = serde_json::from_str(input)?;
    post_session_event(
        runtime,
        "/v1/activity/finalize",
        &input.session_id,
        identity,
    )
}

fn post_session_event(
    runtime: &ServerRuntime,
    path: &str,
    session_id: &str,
    identity: Option<&RepoIdentity>,
) -> anyhow::Result<()> {
    let mut body = json!({
        "session_id": session_id,
        "workspace_id": runtime.workspace_id,
    });
    if let Some(identity) = identity {
        body["repo_id"] = json!(&identity.repo_id);
        body["worktree_id"] = json!(&identity.worktree_id);
        body["root"] = json!(&identity.root);
        body["branch"] = json!(&identity.branch);
    }

    let response = post_json(runtime, path, &body)?;

    if !(200..300).contains(&response.status_code) {
        anyhow::bail!(
            "session event failed with HTTP {}: {}",
            response.status_code,
            response.body
        );
    }

    Ok(())
}

fn handle_pre_tool_use_with_runtime(
    input: &str,
    runtime: Option<&ServerRuntime>,
    repo_root: Option<&Path>,
    cwd: Option<&Path>,
) -> anyhow::Result<HookOutcome> {
    let trusted_sandbox = trusted_sandbox_from_env();
    handle_pre_tool_use_with_runtime_and_sandbox(
        input,
        runtime,
        repo_root,
        cwd,
        trusted_sandbox.as_ref(),
    )
}

fn handle_pre_tool_use_with_runtime_and_sandbox(
    input: &str,
    runtime: Option<&ServerRuntime>,
    repo_root: Option<&Path>,
    cwd: Option<&Path>,
    trusted_sandbox: Option<&serde_json::Value>,
) -> anyhow::Result<HookOutcome> {
    let input: PreToolUseInput = serde_json::from_str(input)?;

    match input.tool_name.as_str() {
        "Bash" => authorize_bash(&input, trusted_sandbox),
        "apply_patch" => authorize_apply_patch(&input, runtime, repo_root, cwd),
        "file_change" => authorize_file_change_tool(&input, runtime, repo_root, cwd),
        "Edit" | "Write" => authorize_file_write_tool(&input, runtime, repo_root, cwd),
        tool_name if tool_name.starts_with("mcp__filesystem__") => Ok(HookOutcome::Deny {
            reason: "filesystem MCP writes require stateful authorization; read-only MCP calls are not yet classified".to_string(),
        }),
        _ => Ok(HookOutcome::Allow),
    }
}

fn authorize_bash(
    input: &PreToolUseInput,
    trusted_sandbox: Option<&serde_json::Value>,
) -> anyhow::Result<HookOutcome> {
    let command = input.command().unwrap_or_default();
    let classification = classify_bash(command);
    match classification.kind {
        BashKind::ReadOnly => Ok(HookOutcome::Allow),
        BashKind::ValidationBypass => Ok(HookOutcome::Deny {
            reason: "Raw test commands are blocked; run tests with state.validation.run or `stateful validate <profile>`.".to_string(),
        }),
        BashKind::Mutating | BashKind::Unknown => {
            if let Some(outcome) = authorize_read_only_sandbox_bash(input, trusted_sandbox) {
                return Ok(outcome);
            }
            Ok(HookOutcome::Deny {
                reason: format!(
                    "Bash command blocked by stateful policy: {}. Use apply_patch or a structured tool after declaring intent.",
                    classification.reason
                ),
            })
        }
    }
}

fn authorize_read_only_sandbox_bash(
    input: &PreToolUseInput,
    trusted_sandbox: Option<&serde_json::Value>,
) -> Option<HookOutcome> {
    let sandbox = effective_sandbox(&input.sandbox, trusted_sandbox)?;

    if !declares_read_only_sandbox(sandbox) {
        return None;
    }

    if !declares_network_access_disabled(sandbox) {
        return Some(HookOutcome::Deny {
            reason:
                "Bash read-only sandbox relaxation requires explicit network access disabled metadata"
                    .to_string(),
        });
    }

    if let Some(root) = sandbox_writable_roots(sandbox)
        .iter()
        .find(|root| !is_trusted_tmp_writable_root(root))
    {
        return Some(HookOutcome::Deny {
            reason: format!(
                "Bash read-only sandbox writable root `{root}` is outside the trusted tmp writable roots"
            ),
        });
    }

    Some(HookOutcome::Allow)
}

fn effective_sandbox<'a>(
    payload_sandbox: &'a serde_json::Value,
    trusted_sandbox: Option<&'a serde_json::Value>,
) -> Option<&'a serde_json::Value> {
    if !payload_sandbox.is_null() {
        return Some(payload_sandbox);
    }

    trusted_sandbox
}

fn trusted_sandbox_from_env() -> Option<serde_json::Value> {
    let value = std::env::var(STATEFUL_TRUSTED_SANDBOX_ENV).ok()?;
    let parsed = serde_json::from_str::<serde_json::Value>(&value).ok()?;
    parsed.is_object().then_some(parsed)
}

fn authorize_apply_patch(
    input: &PreToolUseInput,
    runtime: Option<&ServerRuntime>,
    repo_root: Option<&Path>,
    cwd: Option<&Path>,
) -> anyhow::Result<HookOutcome> {
    let Some(patch) = input.patch_text() else {
        return Ok(HookOutcome::Deny {
            reason:
                "apply_patch writes require patch text with file targets for stateful authorization"
                    .to_string(),
        });
    };
    let targets = extract_apply_patch_write_targets(patch);
    if targets.is_empty() {
        return Ok(HookOutcome::Deny {
            reason:
                "apply_patch writes require at least one file target for stateful authorization"
                    .to_string(),
        });
    }
    let Some(targets) = normalize_targets(targets, repo_root, cwd)? else {
        return Ok(HookOutcome::Deny {
            reason: "apply_patch target is outside the enabled repo".to_string(),
        });
    };
    authorize_targets(input, runtime, targets)
}

fn authorize_file_change_tool(
    input: &PreToolUseInput,
    runtime: Option<&ServerRuntime>,
    repo_root: Option<&Path>,
    cwd: Option<&Path>,
) -> anyhow::Result<HookOutcome> {
    let targets = extract_file_change_targets(&input.tool_input);
    if targets.is_empty() {
        return Ok(HookOutcome::Deny {
            reason: "file_change writes require changed file targets for stateful authorization"
                .to_string(),
        });
    }
    let Some(targets) = normalize_targets(targets, repo_root, cwd)? else {
        return Ok(HookOutcome::Deny {
            reason: "file_change target is outside the enabled repo".to_string(),
        });
    };
    authorize_targets(input, runtime, targets)
}

fn authorize_file_write_tool(
    input: &PreToolUseInput,
    runtime: Option<&ServerRuntime>,
    repo_root: Option<&Path>,
    cwd: Option<&Path>,
) -> anyhow::Result<HookOutcome> {
    let Some(path) = input
        .tool_input
        .get("file_path")
        .or_else(|| input.tool_input.get("path"))
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|path| !path.is_empty())
    else {
        return Ok(HookOutcome::Deny {
            reason: format!(
                "{} writes require a file_path target for stateful authorization",
                input.tool_name
            ),
        });
    };

    let Some(target) = normalize_file_tool_target(path, repo_root, cwd)? else {
        return Ok(HookOutcome::Deny {
            reason: format!("{} target is outside the enabled repo", input.tool_name),
        });
    };

    authorize_targets(input, runtime, vec![PatchTarget::write(&target)])
}

fn normalize_targets(
    targets: Vec<PatchTarget>,
    repo_root: Option<&Path>,
    cwd: Option<&Path>,
) -> anyhow::Result<Option<Vec<PatchTarget>>> {
    let mut normalized = Vec::with_capacity(targets.len());
    for target in targets {
        let Some(path) = normalize_file_tool_target(&target.path, repo_root, cwd)? else {
            return Ok(None);
        };
        normalized.push(target.with_path(path));
    }

    Ok(Some(normalized))
}

fn authorize_targets(
    input: &PreToolUseInput,
    runtime: Option<&ServerRuntime>,
    targets: Vec<PatchTarget>,
) -> anyhow::Result<HookOutcome> {
    let Some(runtime) = runtime else {
        return Ok(HookOutcome::Deny {
            reason: format!(
                "{} writes require a reachable stateful server and active file or directory intent",
                input.tool_name
            ),
        });
    };

    if targets.is_empty() {
        return Ok(HookOutcome::Allow);
    }

    for target in targets {
        let body = protocol_envelope(
            runtime,
            uuid::Uuid::new_v4().to_string(),
            input.session_id.clone(),
            runtime.workspace_id.clone(),
            None,
            "hook",
            "pre_tool_use",
            &input.tool_name,
            json!({
                "action": target.action,
                "path": target.path,
                "queue_on_conflict": true,
            }),
        );
        let response = post_json(runtime, "/v1/authorize", &body)?;

        if !(200..300).contains(&response.status_code) {
            return Ok(HookOutcome::Deny {
                reason: format!(
                    "stateful authorization failed with HTTP {}: {}",
                    response.status_code, response.body
                ),
            });
        }

        let decision: AuthorizeDecision = serde_json::from_str(&response.body)?;
        if decision.decision != "allow" {
            return Ok(HookOutcome::Deny {
                reason: decision.required_next_action.unwrap_or(decision.message),
            });
        }
    }

    Ok(HookOutcome::Allow)
}

fn normalize_file_tool_target(
    path: &str,
    repo_root: Option<&Path>,
    cwd: Option<&Path>,
) -> anyhow::Result<Option<String>> {
    let path = path.trim();
    let candidate = Path::new(path);
    let Some(repo_root) = repo_root else {
        return Ok(Some(path.replace('\\', "/")));
    };
    let repo_root = repo_root
        .canonicalize()
        .unwrap_or_else(|_| repo_root.to_path_buf());

    if candidate.is_absolute() {
        let canonical = candidate
            .canonicalize()
            .unwrap_or_else(|_| candidate.to_path_buf());
        let candidate = normalize_path(canonical);
        let Ok(relative) = candidate.strip_prefix(&repo_root) else {
            return Ok(None);
        };
        return Ok(Some(relative.to_string_lossy().replace('\\', "/")));
    }

    let base = cwd.unwrap_or(repo_root.as_path());
    let base = base.canonicalize().unwrap_or_else(|_| base.to_path_buf());
    let candidate = normalize_path(base.join(candidate));
    let Ok(relative) = candidate.strip_prefix(&repo_root) else {
        return Ok(None);
    };

    Ok(Some(relative.to_string_lossy().replace('\\', "/")))
}

fn extract_apply_patch_write_targets(patch: &str) -> Vec<PatchTarget> {
    patch
        .lines()
        .filter_map(|line| {
            if let Some(path) = line.strip_prefix("*** Update File: ") {
                Some(PatchTarget::write(path))
            } else if let Some(path) = line.strip_prefix("*** Add File: ") {
                Some(PatchTarget::write(path))
            } else {
                line.strip_prefix("*** Delete File: ")
                    .map(PatchTarget::delete)
            }
        })
        .filter(|target| !target.path.is_empty())
        .collect()
}

fn extract_file_change_targets(input: &serde_json::Value) -> Vec<PatchTarget> {
    let Some(changes) = input.get("changes").and_then(serde_json::Value::as_array) else {
        return Vec::new();
    };

    changes
        .iter()
        .filter_map(|change| {
            let path = change
                .get("path")
                .and_then(serde_json::Value::as_str)?
                .trim();
            if path.is_empty() {
                return None;
            }
            let kind = change
                .get("kind")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("update");
            if kind.eq_ignore_ascii_case("delete")
                || kind.eq_ignore_ascii_case("deleted")
                || kind.eq_ignore_ascii_case("remove")
                || kind.eq_ignore_ascii_case("removed")
            {
                Some(PatchTarget::delete(path))
            } else {
                Some(PatchTarget::write(path))
            }
        })
        .collect()
}

fn string_payload_field<'a>(value: &'a serde_json::Value, keys: &[&str]) -> Option<&'a str> {
    if let Some(text) = value.as_str() {
        return Some(text);
    }

    for key in keys {
        if let Some(text) = value.get(key).and_then(serde_json::Value::as_str) {
            return Some(text);
        }
    }

    for nested_key in ["arguments", "args"] {
        if let Some(nested) = value.get(nested_key)
            && let Some(text) = string_payload_field(nested, keys)
        {
            return Some(text);
        }
    }

    None
}

fn declares_read_only_sandbox(value: &serde_json::Value) -> bool {
    sandbox_mode_strings(value)
        .iter()
        .any(|mode| is_read_only_sandbox_mode(mode))
}

fn sandbox_mode_strings(value: &serde_json::Value) -> Vec<String> {
    let mut modes = Vec::new();
    collect_sandbox_mode_strings(value, &mut modes);
    modes
}

fn collect_sandbox_mode_strings(value: &serde_json::Value, modes: &mut Vec<String>) {
    collect_string_fields(
        value,
        &[
            "sandbox",
            "sandbox_mode",
            "sandboxMode",
            "sandbox_permissions",
            "sandboxPermissions",
            "filesystem_sandbox",
            "filesystemSandbox",
            "filesystem_mode",
            "filesystemMode",
            "mode",
        ],
        modes,
    );

    for nested_key in ["sandbox", "arguments", "args"] {
        if let Some(nested) = value.get(nested_key)
            && nested.is_object()
        {
            collect_sandbox_mode_strings(nested, modes);
        }
    }
}

fn is_read_only_sandbox_mode(value: &str) -> bool {
    let normalized = value.trim().to_ascii_lowercase().replace(['_', ' '], "-");
    matches!(normalized.as_str(), "read-only" | "readonly")
}

fn declares_network_access_disabled(value: &serde_json::Value) -> bool {
    matches!(
        bool_payload_field(
            value,
            &[
                "network_access",
                "networkAccess",
                "network_enabled",
                "networkEnabled",
            ],
        ),
        Some(false)
    )
}

fn bool_payload_field(value: &serde_json::Value, keys: &[&str]) -> Option<bool> {
    for key in keys {
        if let Some(value) = value.get(key) {
            if let Some(boolean) = value.as_bool() {
                return Some(boolean);
            }
            if let Some(text) = value.as_str() {
                let normalized = text.trim().to_ascii_lowercase();
                if matches!(normalized.as_str(), "true" | "enabled" | "yes" | "on") {
                    return Some(true);
                }
                if matches!(normalized.as_str(), "false" | "disabled" | "no" | "off") {
                    return Some(false);
                }
            }
        }
    }

    for nested_key in ["sandbox", "arguments", "args"] {
        if let Some(nested) = value.get(nested_key)
            && let Some(boolean) = bool_payload_field(nested, keys)
        {
            return Some(boolean);
        }
    }

    None
}

fn sandbox_writable_roots(value: &serde_json::Value) -> Vec<String> {
    let mut roots = Vec::new();
    collect_string_array_fields(
        value,
        &[
            "writable_roots",
            "writableRoots",
            "writable_paths",
            "writablePaths",
            "writable_dirs",
            "writableDirs",
        ],
        &mut roots,
    );

    for nested_key in ["sandbox", "arguments", "args"] {
        if let Some(nested) = value.get(nested_key)
            && nested.is_object()
        {
            roots.extend(sandbox_writable_roots(nested));
        }
    }

    roots
}

fn collect_string_fields(value: &serde_json::Value, keys: &[&str], output: &mut Vec<String>) {
    for key in keys {
        if let Some(text) = value.get(key).and_then(serde_json::Value::as_str) {
            output.push(text.to_string());
        }
    }
}

fn collect_string_array_fields(value: &serde_json::Value, keys: &[&str], output: &mut Vec<String>) {
    for key in keys {
        if let Some(array) = value.get(key).and_then(serde_json::Value::as_array) {
            output.extend(
                array
                    .iter()
                    .filter_map(serde_json::Value::as_str)
                    .map(ToString::to_string),
            );
        }
    }
}

fn is_trusted_tmp_writable_root(root: &str) -> bool {
    let root = root.trim();
    if matches!(root, "$TMPDIR" | "${TMPDIR}") || root.starts_with("$TMPDIR/") {
        return true;
    }

    let path = normalize_path(PathBuf::from(root));
    if !path.is_absolute() {
        return false;
    }

    let trusted_roots = [
        Path::new("/tmp"),
        Path::new("/private/tmp"),
        Path::new("/var/tmp"),
    ];
    if trusted_roots
        .iter()
        .any(|trusted| path.starts_with(trusted))
    {
        return true;
    }

    let temp_dir = normalize_path(std::env::temp_dir());
    path.starts_with(temp_dir)
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PatchTarget {
    action: &'static str,
    path: String,
}

impl PatchTarget {
    fn write(path: &str) -> Self {
        Self {
            action: "write_file",
            path: path.trim().to_string(),
        }
    }

    fn delete(path: &str) -> Self {
        Self {
            action: "delete_file",
            path: path.trim().to_string(),
        }
    }

    fn with_path(mut self, path: String) -> Self {
        self.path = path;
        self
    }
}

fn repo_identity(paths: &GlobalPaths, repo_root: &Path) -> anyhow::Result<RepoIdentity> {
    repo_identity_for_enabled_repo(paths, repo_root)
}

fn hook_start_dir(input: &str) -> anyhow::Result<PathBuf> {
    let fallback = std::env::current_dir()?;
    Ok(hook_start_dir_or(input, &fallback))
}

fn hook_start_dir_or(input: &str, fallback: &Path) -> PathBuf {
    let Ok(input) = serde_json::from_str::<HookCwdInput>(input) else {
        return fallback.to_path_buf();
    };
    input
        .cwd
        .filter(|cwd| !cwd.as_os_str().is_empty() && cwd.exists())
        .unwrap_or_else(|| fallback.to_path_buf())
}

fn normalize_path(path: PathBuf) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Prefix(prefix) => normalized.push(prefix.as_os_str()),
            Component::RootDir => normalized.push(component.as_os_str()),
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            Component::Normal(segment) => normalized.push(segment),
        }
    }
    normalized
}

#[derive(Debug, Deserialize)]
struct PreToolUseInput {
    session_id: String,
    tool_name: String,
    #[serde(default)]
    sandbox: serde_json::Value,
    #[serde(default)]
    tool_input: serde_json::Value,
}

#[derive(Debug, Deserialize)]
struct HookCwdInput {
    #[serde(default)]
    cwd: Option<PathBuf>,
}

impl PreToolUseInput {
    fn command(&self) -> Option<&str> {
        self.tool_input.get("command")?.as_str()
    }

    fn patch_text(&self) -> Option<&str> {
        string_payload_field(
            &self.tool_input,
            &["command", "patch", "input", "cmd", "diff"],
        )
    }
}

#[derive(Debug, Deserialize)]
struct AuthorizeDecision {
    decision: String,
    message: String,
    #[serde(default)]
    required_next_action: Option<String>,
}

#[derive(Debug, Deserialize)]
struct SessionStartInput {
    session_id: String,
}

#[derive(Debug, Deserialize)]
struct SessionEventInput {
    session_id: String,
}

#[derive(Debug, Deserialize)]
struct UserPromptSubmitInput {
    session_id: String,
}

#[derive(Debug, Deserialize)]
struct ContextRenderResponse {
    prompt_text: String,
}
