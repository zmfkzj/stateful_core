use std::{
    io::{self, Read},
    path::{Component, Path, PathBuf},
};

use serde::Deserialize;
use serde_json::json;
use stateful_core::{BashKind, classify_bash};

use crate::outbox::queue_session_heartbeat_outbox;
use crate::{
    CodexHookCommand, CurrentSession, GlobalPaths, HookCommand, ProtocolPostContext, RepoGate,
    RepoIdentity, ServerRuntime, discover_runtime_with_global, ensure_server, post_json,
    post_protocol_json, repo_gate, repo_identity_for_enabled_repo, write_current_session_file,
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
        HookCommand::Codex(command) => run_codex_hook(command)?,
        HookCommand::Run { event: _ } => {
            let mut input = String::new();
            io::stdin().read_to_string(&mut input)?;
            if let Some(outcome) = handle_normalized_hook_in_repo(&input, hook_start_dir(&input)?)?
                && !matches!(outcome, HookOutcome::Allow)
            {
                println!("{}", serde_json::to_string(&outcome.to_stdout_json()?)?);
            }
        }
        HookCommand::SessionStart => run_codex_hook(CodexHookCommand::SessionStart)?,
        HookCommand::PostToolUse => run_codex_hook(CodexHookCommand::PostToolUse)?,
        HookCommand::UserPromptSubmit => run_codex_hook(CodexHookCommand::UserPromptSubmit)?,
        HookCommand::Stop => run_codex_hook(CodexHookCommand::Stop)?,
        HookCommand::PreToolUse => run_codex_hook(CodexHookCommand::PreToolUse)?,
    }

    Ok(())
}

fn run_codex_hook(command: CodexHookCommand) -> anyhow::Result<()> {
    match command {
        CodexHookCommand::SessionStart => {
            let mut input = String::new();
            io::stdin().read_to_string(&mut input)?;
            if let Err(error) = handle_session_start_in_repo(&input, hook_start_dir(&input)?) {
                eprintln!("stateful session-start warning: {error}");
            }
        }
        CodexHookCommand::PostToolUse => {
            let mut input = String::new();
            io::stdin().read_to_string(&mut input)?;
            if let Err(error) = handle_post_tool_use_in_repo(&input, hook_start_dir(&input)?) {
                eprintln!("stateful post-tool-use warning: {error}");
            }
        }
        CodexHookCommand::UserPromptSubmit => {
            let mut input = String::new();
            io::stdin().read_to_string(&mut input)?;
            match handle_user_prompt_submit_in_repo(&input, hook_start_dir(&input)?) {
                Ok(prompt_text) if !prompt_text.is_empty() => println!("{prompt_text}"),
                Ok(_) => {}
                Err(error) => eprintln!("stateful user-prompt-submit warning: {error}"),
            }
        }
        CodexHookCommand::Stop => {
            let mut input = String::new();
            io::stdin().read_to_string(&mut input)?;
            if let Err(error) = handle_stop_in_repo(&input, hook_start_dir(&input)?) {
                eprintln!("stateful stop warning: {error}");
            }
        }
        CodexHookCommand::PreToolUse => {
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

pub fn handle_codex_pre_tool_use(input: &str) -> anyhow::Result<HookOutcome> {
    handle_pre_tool_use(input)
}

pub fn handle_pre_tool_use(input: &str) -> anyhow::Result<HookOutcome> {
    handle_pre_tool_use_with_runtime(input, None, None, None)
}

pub fn handle_normalized_hook_in_repo(
    input: &str,
    repo_root: impl AsRef<Path>,
) -> anyhow::Result<Option<HookOutcome>> {
    let normalized: NormalizedHookInput = serde_json::from_str(input)?;
    match normalized.event.as_str() {
        "pre_tool_use" | "pre-tool-use" => {
            let codex_shape = normalized.to_pre_tool_use_input().to_string();
            handle_pre_tool_use_in_repo(&codex_shape, repo_root).map(Some)
        }
        "post_tool_use" | "post-tool-use" => {
            let codex_shape = normalized.to_session_event_input("PostToolUse").to_string();
            handle_post_tool_use_in_repo(&codex_shape, repo_root)?;
            Ok(None)
        }
        "session_start" | "session-start" => {
            let codex_shape = normalized
                .to_session_event_input("SessionStart")
                .to_string();
            handle_session_start_in_repo(&codex_shape, repo_root)?;
            Ok(None)
        }
        "stop" => {
            let codex_shape = normalized.to_session_event_input("Stop").to_string();
            handle_stop_in_repo(&codex_shape, repo_root)?;
            Ok(None)
        }
        _ => Ok(None),
    }
}

#[derive(Debug, Deserialize)]
struct NormalizedHookInput {
    event: String,
    session_id: String,
    cwd: Option<PathBuf>,
    #[serde(default)]
    tool_name: Option<String>,
    #[serde(default)]
    tool_input: serde_json::Value,
    #[serde(default, rename = "source")]
    _source: serde_json::Value,
}

impl NormalizedHookInput {
    fn to_pre_tool_use_input(&self) -> serde_json::Value {
        serde_json::json!({
            "session_id": &self.session_id,
            "cwd": self.cwd_string(),
            "hook_event_name": "PreToolUse",
            "tool_name": self.tool_name.as_deref().unwrap_or(""),
            "tool_input": self.tool_input.clone(),
        })
    }

    fn to_session_event_input(&self, hook_event_name: &str) -> serde_json::Value {
        serde_json::json!({
            "session_id": &self.session_id,
            "cwd": self.cwd_string(),
            "hook_event_name": hook_event_name,
            "tool_name": self.tool_name.as_deref().unwrap_or(""),
            "tool_input": self.tool_input.clone(),
        })
    }

    fn cwd_string(&self) -> Option<String> {
        self.cwd
            .as_ref()
            .map(|path| path.to_string_lossy().to_string())
    }
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
    let response = post_protocol_json(
        runtime,
        path,
        ProtocolPostContext {
            session_id,
            workspace_id: &runtime.workspace_id,
            source_kind: "hook",
            source_event: session_event_source(path),
            source_ref: "stateful hook session event",
            tool_name: None,
            identity,
        },
        &json!({}),
    )?;

    if !(200..300).contains(&response.status_code) {
        anyhow::bail!(
            "session event failed with HTTP {}: {}",
            response.status_code,
            response.body
        );
    }

    Ok(())
}

fn session_event_source(path: &str) -> &'static str {
    match path {
        "/v1/session/register" => "session_start",
        "/v1/session/heartbeat" => "post_tool_use",
        "/v1/activity/finalize" => "stop",
        _ => "session_event",
    }
}

fn handle_pre_tool_use_with_runtime(
    input: &str,
    runtime: Option<&ServerRuntime>,
    repo_root: Option<&Path>,
    cwd: Option<&Path>,
) -> anyhow::Result<HookOutcome> {
    let input: PreToolUseInput = serde_json::from_str(input)?;

    match input.tool_name.as_str() {
        "Bash" => authorize_bash(input.command().unwrap_or_default()),
        "apply_patch" => authorize_apply_patch(&input, runtime),
        "Edit" | "Write" => authorize_file_write_tool(&input, runtime, repo_root, cwd),
        tool_name if tool_name.starts_with("mcp__filesystem__") => Ok(HookOutcome::Deny {
            reason: "filesystem MCP writes require stateful authorization; read-only MCP calls are not yet classified".to_string(),
        }),
        _ => Ok(HookOutcome::Allow),
    }
}

fn authorize_bash(command: &str) -> anyhow::Result<HookOutcome> {
    let classification = classify_bash(command);
    match classification.kind {
        BashKind::ReadOnly => Ok(HookOutcome::Allow),
        BashKind::ValidationBypass => Ok(HookOutcome::Deny {
            reason: "Raw test commands are blocked; run tests with state.validation.run or `stateful validate <profile>`.".to_string(),
        }),
        BashKind::Mutating | BashKind::Unknown => Ok(HookOutcome::Deny {
            reason: format!(
                "Bash command blocked by stateful policy: {}. Use apply_patch or a structured tool after declaring intent.",
                classification.reason
            ),
        }),
    }
}

fn authorize_apply_patch(
    input: &PreToolUseInput,
    runtime: Option<&ServerRuntime>,
) -> anyhow::Result<HookOutcome> {
    let targets = extract_apply_patch_write_targets(input.command().unwrap_or_default());
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
        let response = post_protocol_json(
            runtime,
            "/v1/authorize",
            ProtocolPostContext {
                session_id: &input.session_id,
                workspace_id: &runtime.workspace_id,
                source_kind: "hook",
                source_event: "pre_tool_use",
                source_ref: "stateful hook pre-tool-use",
                tool_name: Some(&input.tool_name),
                identity: None,
            },
            &json!({
                "action": target.action,
                "path": target.path,
                "queue_on_conflict": true,
            }),
        )?;

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
