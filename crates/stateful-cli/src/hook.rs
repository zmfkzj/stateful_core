use std::{
    io::{self, Read},
    path::{Component, Path, PathBuf},
};

use serde::Deserialize;
use serde_json::json;
use stateful_core::{BashKind, classify_bash};

use crate::outbox::queue_session_heartbeat_outbox;
use crate::{
    CurrentSession, GlobalPaths, HookCommand, ProtocolEnvelopeArgs, RepoGate, RepoIdentity,
    ServerRuntime, discover_runtime_with_global, ensure_server, post_json, protocol_envelope,
    repo_gate, repo_identity_for_enabled_repo, write_current_session_file,
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
        BashKind::ReadOnly => Ok(authorize_read_only_sandbox_bash(input, trusted_sandbox)),
        BashKind::Mutating if is_stateful_intent_declare_command(command) => {
            Ok(HookOutcome::Deny {
                reason: format!(
                    "Bash command blocked by stateful policy: {}. Use state_intent_declare / state.intent.declare directly in Codex sessions.",
                    classification.reason
                ),
            })
        }
        BashKind::Mutating => Ok(HookOutcome::Deny {
            reason: format!(
                "Bash command blocked by stateful policy: {}. Use state_bash_write / state.bash.write with explicit write_targets, or a structured stateful write tool.",
                classification.reason
            ),
        }),
        BashKind::ValidationBypass => Ok(HookOutcome::Deny {
            reason: format!(
                "Bash command blocked by stateful policy: {}. Use state_bash_write / state.bash.write with explicit write_targets, or run a configured validation profile outside Codex hooks.",
                classification.reason
            ),
        }),
        BashKind::Unknown => Ok(HookOutcome::Deny {
            reason: format!(
                "Bash command blocked by stateful policy: {}. Use direct Bash only for known read-only inspection commands; use stateful tools for writes.",
                classification.reason
            ),
        }),
    }
}

fn is_stateful_intent_declare_command(command: &str) -> bool {
    let words = command.split_whitespace().collect::<Vec<_>>();
    words.windows(3).any(|window| {
        window[0].rsplit('/').next().unwrap_or(window[0]) == "stateful"
            && window[1] == "intent"
            && window[2] == "declare"
    })
}

fn authorize_read_only_sandbox_bash(
    input: &PreToolUseInput,
    trusted_sandbox: Option<&serde_json::Value>,
) -> HookOutcome {
    let sandbox = if input.sandbox.is_null() {
        let _ = trusted_sandbox;
        return HookOutcome::Deny {
            reason: "Bash read-only commands require top-level read-only sandbox metadata"
                .to_string(),
        };
    } else {
        &input.sandbox
    };

    if has_writable_sandbox_mode(sandbox) || !declares_read_only_sandbox(sandbox) {
        return HookOutcome::Deny {
            reason: "Bash sandbox metadata must declare read-only mode when supplied".to_string(),
        };
    }

    if !declares_network_access_disabled(sandbox) {
        return HookOutcome::Deny {
            reason:
                "Bash read-only sandbox relaxation requires explicit network access disabled metadata"
                    .to_string(),
        };
    }

    if let Some(root) = sandbox_writable_roots(sandbox)
        .iter()
        .find(|root| !is_trusted_tmp_writable_root(root))
    {
        return HookOutcome::Deny {
            reason: format!(
                "Bash read-only sandbox writable root `{root}` is outside the trusted tmp writable roots"
            ),
        };
    }

    HookOutcome::Allow
}

fn trusted_sandbox_from_env() -> Option<serde_json::Value> {
    None
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
        let old_path = target
            .old_path
            .as_deref()
            .map(|old_path| normalize_file_tool_target(old_path, repo_root, cwd))
            .transpose()?
            .flatten();
        let new_path = target
            .new_path
            .as_deref()
            .map(|new_path| normalize_file_tool_target(new_path, repo_root, cwd))
            .transpose()?
            .flatten();
        if target.old_path.is_some() && old_path.is_none()
            || target.new_path.is_some() && new_path.is_none()
        {
            return Ok(None);
        }
        normalized.push(target.with_paths(path, old_path, new_path));
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
        let mut payload = json!({
            "action": target.action,
            "path": target.path,
            "queue_on_conflict": true,
        });
        if let Some(old_path) = target.old_path {
            payload["old_path"] = json!(old_path);
        }
        if let Some(new_path) = target.new_path {
            payload["new_path"] = json!(new_path);
        }
        let body = protocol_envelope(ProtocolEnvelopeArgs {
            runtime,
            request_id: uuid::Uuid::new_v4().to_string(),
            session_id: input.session_id.clone(),
            workspace_id: runtime.workspace_id.clone(),
            identity: None,
            source_kind: "hook",
            event: "pre_tool_use",
            source_ref: &input.tool_name,
            payload,
        });
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
    let mut targets = Vec::new();
    let mut pending_update: Option<String> = None;

    for line in patch.lines() {
        if let Some(path) = line.strip_prefix("*** Update File: ") {
            if let Some(path) = pending_update.take() {
                targets.push(PatchTarget::write(&path));
            }
            pending_update = Some(path.trim().to_string());
        } else if let Some(path) = line.strip_prefix("*** Move to: ") {
            if let Some(old_path) = pending_update.take() {
                targets.push(PatchTarget::move_file(&old_path, path));
            } else {
                targets.push(PatchTarget::write(path));
            }
        } else if let Some(path) = line.strip_prefix("*** Add File: ") {
            if let Some(path) = pending_update.take() {
                targets.push(PatchTarget::write(&path));
            }
            targets.push(PatchTarget::write(path));
        } else if let Some(path) = line.strip_prefix("*** Delete File: ") {
            if let Some(path) = pending_update.take() {
                targets.push(PatchTarget::write(&path));
            }
            targets.push(PatchTarget::delete(path));
        }
    }

    if let Some(path) = pending_update {
        targets.push(PatchTarget::write(&path));
    }

    targets
        .into_iter()
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

fn has_writable_sandbox_mode(value: &serde_json::Value) -> bool {
    recursive_sandbox_mode_strings(value)
        .iter()
        .any(|mode| is_writable_sandbox_mode(mode))
}

fn recursive_sandbox_mode_strings(value: &serde_json::Value) -> Vec<String> {
    let mut modes = sandbox_mode_strings(value);
    for nested in nested_sandbox_objects(value) {
        modes.extend(sandbox_mode_strings(nested));
    }
    modes
}

fn nested_sandbox_objects(value: &serde_json::Value) -> Vec<&serde_json::Value> {
    let mut objects = Vec::new();
    collect_nested_sandbox_objects(value, &mut objects);
    objects
}

fn collect_nested_sandbox_objects<'a>(
    value: &'a serde_json::Value,
    objects: &mut Vec<&'a serde_json::Value>,
) {
    if let Some(nested) = value.get("sandbox").filter(|nested| nested.is_object()) {
        objects.push(nested);
        collect_nested_sandbox_objects(nested, objects);
    }

    for key in ["arguments", "args"] {
        if let Some(container) = value.get(key).filter(|nested| nested.is_object()) {
            objects.push(container);
            collect_nested_sandbox_objects(container, objects);
        }
    }
}

fn sandbox_mode_strings(value: &serde_json::Value) -> Vec<String> {
    let mut modes = Vec::new();
    if let Some(mode) = value.as_str() {
        modes.push(mode.to_string());
    }
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
        &mut modes,
    );
    modes
}

fn is_read_only_sandbox_mode(value: &str) -> bool {
    let normalized = value.trim().to_ascii_lowercase().replace(['_', ' '], "-");
    matches!(normalized.as_str(), "read-only" | "readonly")
}

fn is_writable_sandbox_mode(value: &str) -> bool {
    let normalized = value.trim().to_ascii_lowercase().replace(['_', ' '], "-");
    matches!(
        normalized.as_str(),
        "workspace-write"
            | "danger-full-access"
            | "read-write"
            | "write"
            | "writable"
            | "full-access"
            | "unrestricted"
            | "require-escalated"
            | "write-targets"
    )
}

fn declares_network_access_disabled(value: &serde_json::Value) -> bool {
    let keys = &[
        "network_access",
        "networkAccess",
        "network_enabled",
        "networkEnabled",
    ];
    let top_level = bool_payload_fields(value, keys);
    top_level.iter().any(|enabled| !enabled)
        && !recursive_bool_payload_values(value, keys)
            .into_iter()
            .any(|enabled| enabled)
}

fn recursive_bool_payload_values(value: &serde_json::Value, keys: &[&str]) -> Vec<bool> {
    let mut values = Vec::new();
    values.extend(bool_payload_fields(value, keys));
    for nested in nested_sandbox_objects(value) {
        values.extend(bool_payload_fields(nested, keys));
    }
    values
}

fn bool_payload_fields(value: &serde_json::Value, keys: &[&str]) -> Vec<bool> {
    let mut values = Vec::new();
    for key in keys {
        if let Some(value) = value.get(key) {
            if let Some(boolean) = value.as_bool() {
                values.push(boolean);
                continue;
            }
            if let Some(text) = value.as_str() {
                let normalized = text.trim().to_ascii_lowercase();
                if matches!(normalized.as_str(), "true" | "enabled" | "yes" | "on") {
                    values.push(true);
                    continue;
                }
                if matches!(normalized.as_str(), "false" | "disabled" | "no" | "off") {
                    values.push(false);
                    continue;
                }
            }
        }
    }

    values
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
    for nested in nested_sandbox_objects(value) {
        collect_string_array_fields(
            nested,
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
    if matches!(root, "$TMPDIR" | "${TMPDIR}") {
        return true;
    }
    if let Some(suffix) = root
        .strip_prefix("$TMPDIR/")
        .or_else(|| root.strip_prefix("${TMPDIR}/"))
    {
        return is_safe_tmpdir_suffix(suffix);
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

fn is_safe_tmpdir_suffix(suffix: &str) -> bool {
    suffix.is_empty()
        || Path::new(suffix).components().all(|component| {
            matches!(component, std::path::Component::Normal(part) if !part.is_empty())
        })
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PatchTarget {
    action: &'static str,
    path: String,
    old_path: Option<String>,
    new_path: Option<String>,
}

impl PatchTarget {
    fn write(path: &str) -> Self {
        Self {
            action: "write_file",
            path: path.trim().to_string(),
            old_path: None,
            new_path: None,
        }
    }

    fn delete(path: &str) -> Self {
        Self {
            action: "delete_file",
            path: path.trim().to_string(),
            old_path: None,
            new_path: None,
        }
    }

    fn move_file(old_path: &str, new_path: &str) -> Self {
        let old_path = old_path.trim().to_string();
        let new_path = new_path.trim().to_string();
        Self {
            action: "move_file",
            path: old_path.clone(),
            old_path: Some(old_path),
            new_path: Some(new_path),
        }
    }

    fn with_paths(
        mut self,
        path: String,
        old_path: Option<String>,
        new_path: Option<String>,
    ) -> Self {
        self.path = path;
        self.old_path = old_path;
        self.new_path = new_path;
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
        string_payload_field(&self.tool_input, &["command", "cmd", "input"])
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
