use std::{
    io::{self, Read},
    path::{Path, PathBuf},
    process::Command,
};

use serde::Deserialize;
use serde_json::json;
use stateful_core::{BashKind, classify_bash};

use crate::outbox::queue_session_heartbeat_outbox;
use crate::{
    CurrentSession, GlobalPaths, HookCommand, RepoGate, RepoRegistry, ServerRuntime,
    discover_runtime_with_global, ensure_server, post_json, repo_gate, write_current_session_file,
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
    handle_pre_tool_use_with_runtime(input, None, None)
}

pub fn handle_pre_tool_use_in_repo(
    input: &str,
    repo_root: impl AsRef<Path>,
) -> anyhow::Result<HookOutcome> {
    let start = repo_root.as_ref();
    let paths = GlobalPaths::from_env()?;
    let repo_root = match repo_gate(&paths, start)? {
        RepoGate::Enabled { repo_root } => {
            ensure_server(&paths)?;
            repo_root
        }
        RepoGate::Disabled | RepoGate::OutsideGitRepo => return Ok(HookOutcome::Allow),
    };
    let runtime = discover_runtime_with_global(&repo_root, &paths)?;
    remember_current_session(&repo_root, &runtime, input)?;
    handle_pre_tool_use_with_runtime(input, Some(&runtime), Some(&repo_root))
}

pub fn handle_session_start_in_repo(
    input: &str,
    repo_root: impl AsRef<Path>,
) -> anyhow::Result<()> {
    let start = repo_root.as_ref();
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
    let start = repo_root.as_ref();
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
    let start = repo_root.as_ref();
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
    let start = repo_root.as_ref();
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
) -> anyhow::Result<HookOutcome> {
    let input: PreToolUseInput = serde_json::from_str(input)?;

    match input.tool_name.as_str() {
        "Bash" => authorize_bash(input.command().unwrap_or_default()),
        "apply_patch" => authorize_apply_patch(&input, runtime),
        "Edit" | "Write" => authorize_file_write_tool(&input, runtime, repo_root),
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

    let Some(target) = normalize_file_tool_target(path, repo_root)? else {
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
        let response = post_json(
            runtime,
            "/v1/authorize",
            &json!({
                "session_id": input.session_id,
                "workspace_id": runtime.workspace_id,
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
) -> anyhow::Result<Option<String>> {
    let path = path.trim();
    let candidate = Path::new(path);
    if candidate.is_absolute() {
        let Some(repo_root) = repo_root else {
            return Ok(Some(path.replace('\\', "/")));
        };
        let repo_root = repo_root
            .canonicalize()
            .unwrap_or_else(|_| repo_root.to_path_buf());
        let canonical = candidate
            .canonicalize()
            .unwrap_or_else(|_| candidate.to_path_buf());
        let Ok(relative) = canonical.strip_prefix(&repo_root) else {
            return Ok(None);
        };
        return Ok(Some(relative.to_string_lossy().replace('\\', "/")));
    }

    Ok(Some(path.replace('\\', "/")))
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

#[derive(Debug, Clone, PartialEq, Eq)]
struct RepoIdentity {
    repo_id: String,
    worktree_id: String,
    root: String,
    branch: String,
}

fn repo_identity(paths: &GlobalPaths, repo_root: &Path) -> anyhow::Result<RepoIdentity> {
    let registry = RepoRegistry::load(paths)?;
    let entry = registry
        .enabled_entry(repo_root)
        .ok_or_else(|| anyhow::anyhow!("enabled repo metadata not found"))?;
    let branch = current_branch(repo_root).unwrap_or_else(|| "unknown".to_string());

    Ok(RepoIdentity {
        repo_id: entry.repo_id.clone(),
        worktree_id: entry.repo_id.clone(),
        root: entry.root.to_string_lossy().into_owned(),
        branch,
    })
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

fn hook_start_dir(input: &str) -> anyhow::Result<PathBuf> {
    let fallback = std::env::current_dir()?;
    let Ok(input) = serde_json::from_str::<HookCwdInput>(input) else {
        return Ok(fallback);
    };
    Ok(input
        .cwd
        .filter(|cwd| !cwd.as_os_str().is_empty() && cwd.exists())
        .unwrap_or(fallback))
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
