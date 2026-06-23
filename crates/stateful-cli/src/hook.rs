use std::{
    collections::BTreeSet,
    fs,
    io::{self, Read},
    path::{Component, Path, PathBuf},
};

use serde::{Deserialize, Serialize};
use serde_json::json;
use stateful_core::normalize_relative_path;

use crate::outbox::queue_session_heartbeat_outbox;
use crate::runtime::write_current_session_file_for_explicit_session;
use crate::sandbox::{
    SandboxFsProfile, parse_sandbox_process_find_bash_invocation,
    parse_sandbox_run_bash_invocation, validate_process_find_request,
    validate_sandbox_run_request_shape,
};
use crate::shadow_guard;
use crate::shell_command::{
    first_word_is_env_assignment, reject_outer_shell_syntax, split_simple_command_words,
};
use crate::{
    CurrentSession, GlobalPaths, HookCommand, HookRuntime, ProtocolEnvelopeArgs, RepoGate,
    RepoIdentity, ServerRuntime, discover_runtime_with_global, effective_workspace_id_for_repo,
    ensure_server, get_json, post_json, protocol_envelope, record_unclassified_tool_for_repo,
    repo_gate, repo_identity_for_enabled_repo, runtime_env_override_is_configured,
    tool_allowed_for_enabled_repo,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HookOutcome {
    Allow,
    AllowWithContext { message: String },
    Deny { reason: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OmpHookOutcome {
    Allow,
    Block { reason: String },
    Prompt { title: String, message: String },
}

impl OmpHookOutcome {
    fn to_stdout_json(&self) -> serde_json::Value {
        match self {
            Self::Allow => json!({ "decision": "allow" }),
            Self::Block { reason } => json!({
                "decision": "block",
                "reason": reason,
            }),
            Self::Prompt { title, message } => json!({
                "decision": "prompt",
                "title": title,
                "message": message,
            }),
        }
    }
}

#[cfg(feature = "codex-benchmark")]
#[derive(Debug, Clone, PartialEq, Eq)]
struct NestedCodexBenchmarkSandboxInvocation {
    executable: String,
    purpose: String,
    write_dir: String,
    codex_home_root: String,
    docker_socket: Option<String>,
    command: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct StatefulControlInvocation {
    executable: String,
}

impl HookOutcome {
    pub fn to_stdout_json(&self) -> serde_json::Result<serde_json::Value> {
        match self {
            Self::Allow => Ok(json!({})),
            Self::AllowWithContext { message } => Ok(json!({
                "hookSpecificOutput": {
                    "hookEventName": "PreToolUse",
                    "additionalContext": message,
                }
            })),
            Self::Deny { reason } => Ok(json!({
                "hookSpecificOutput": {
                    "hookEventName": "PreToolUse",
                    "permissionDecision": "deny",
                    "permissionDecisionReason": reason,
                }
            })),
        }
    }

    fn emits_stdout(&self) -> bool {
        !matches!(self, Self::Allow)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct OmpSessionStartOutput {
    pub decision: &'static str,
    pub session_id: String,
    pub workspace_id: String,
    pub notifications_stream: OmpNotificationsStream,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct OmpNotificationsStream {
    pub base_url: String,
    pub authorization: String,
    pub session_id: String,
    pub workspace_id: String,
}

pub fn run_hook(runtime: HookRuntime) -> anyhow::Result<()> {
    match runtime {
        HookRuntime::Codex { command } => run_codex_hook(command),
        HookRuntime::Omp { command } => run_omp_hook(command),
    }
}

fn run_omp_hook(command: HookCommand) -> anyhow::Result<()> {
    let mut input = String::new();
    io::stdin().read_to_string(&mut input)?;
    let start = hook_start_dir(&input)?;
    let paths = GlobalPaths::from_env()?;
    let repo_root = match repo_gate(&paths, &start)? {
        RepoGate::Enabled { repo_root } => repo_root,
        RepoGate::Disabled | RepoGate::OutsideGitRepo => {
            if matches!(command, HookCommand::PreToolUse) {
                println!("{}", json!({ "decision": "allow" }));
            }
            return Ok(());
        }
    };
    if !runtime_env_override_is_configured() {
        ensure_server(&paths)?;
    }
    let runtime = discover_runtime_with_global(&repo_root, &paths)?;
    let identity = repo_identity_for_enabled_repo(&paths, &repo_root).ok();

    match command {
        HookCommand::PreToolUse => {
            let outcome = handle_omp_pre_tool_use_with_identity(
                &input,
                Some(&runtime),
                Some(&repo_root),
                Some(start.as_path()),
                identity.as_ref(),
            )?;
            println!("{}", outcome.to_stdout_json());
        }
        HookCommand::SessionStart => {
            match handle_omp_session_start_with_identity(
                &input,
                &runtime,
                &repo_root,
                identity.as_ref(),
            ) {
                Ok(output) => println!("{}", serde_json::to_string(&output)?),
                Err(error) => eprintln!("stateful omp session-start warning: {error}"),
            }
        }
        HookCommand::PostToolUse => {
            if let Err(error) = handle_omp_post_tool_use_with_identity(
                &input,
                &runtime,
                Some(&repo_root),
                identity.as_ref(),
            ) {
                eprintln!("stateful omp post-tool-use warning: {error}");
            }
        }
        HookCommand::Stop => {
            let input: OmpSessionEventInput = serde_json::from_str(&input)?;
            post_omp_session_event(
                &runtime,
                "/v1/activity/finalize",
                "omp_stop",
                &input,
                identity.as_ref(),
            )?;
        }
        HookCommand::UserPromptSubmit => {
            anyhow::bail!("OMP hook user-prompt-submit is not supported");
        }
    }
    Ok(())
}

fn run_codex_hook(command: HookCommand) -> anyhow::Result<()> {
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
            if outcome.emits_stdout() {
                println!("{}", serde_json::to_string(&outcome.to_stdout_json()?)?);
            }
        }
    }

    Ok(())
}

pub fn handle_pre_tool_use(input: &str) -> anyhow::Result<HookOutcome> {
    handle_pre_tool_use_with_runtime(input, None, None, None)
}

pub fn handle_pre_tool_use_in_repo(
    input: &str,
    repo_root: impl AsRef<Path>,
) -> anyhow::Result<HookOutcome> {
    let start = hook_start_dir_or(input, repo_root.as_ref());
    let paths = GlobalPaths::from_env()?;
    let repo_root = match repo_gate(&paths, &start)? {
        RepoGate::Enabled { repo_root } => repo_root,
        RepoGate::Disabled | RepoGate::OutsideGitRepo => return Ok(HookOutcome::Allow),
    };
    let runtime = prepare_pre_tool_use_runtime(&repo_root, &paths, input).ok();
    handle_pre_tool_use_with_runtime(
        input,
        runtime.as_ref(),
        Some(&repo_root),
        Some(start.as_path()),
    )
}

pub fn handle_omp_pre_tool_use_with_runtime(
    input: &str,
    runtime: Option<&ServerRuntime>,
    repo_root: Option<&Path>,
    cwd: Option<&Path>,
) -> anyhow::Result<OmpHookOutcome> {
    handle_omp_pre_tool_use_with_identity(input, runtime, repo_root, cwd, None)
}

fn handle_omp_pre_tool_use_with_identity(
    input: &str,
    runtime: Option<&ServerRuntime>,
    repo_root: Option<&Path>,
    cwd: Option<&Path>,
    identity: Option<&RepoIdentity>,
) -> anyhow::Result<OmpHookOutcome> {
    let input: OmpPreToolUseInput = serde_json::from_str(input)?;
    let global_paths = GlobalPaths::from_env().ok();
    match omp_pre_tool_action(&input, repo_root, cwd, global_paths.as_ref())? {
        OmpPreToolAction::Allow => Ok(OmpHookOutcome::Allow),
        OmpPreToolAction::Block { reason } => Ok(OmpHookOutcome::Block { reason }),
        OmpPreToolAction::Targets(targets) => {
            authorize_omp_targets(&input, runtime, repo_root, cwd, identity, targets)
        }
    }
}

enum OmpPreToolAction {
    Targets(Vec<PatchTarget>),
    Allow,
    Block { reason: String },
}

fn omp_pre_tool_action(
    input: &OmpPreToolUseInput,
    repo_root: Option<&Path>,
    _cwd: Option<&Path>,
    global_paths: Option<&GlobalPaths>,
) -> anyhow::Result<OmpPreToolAction> {
    let tool_name = runtime_tool_name_leaf(&input.tool_name);
    match tool_name {
        tool_name if tool_name.eq_ignore_ascii_case("write") => {
            let Some(path) = input
                .tool_input
                .get("path")
                .and_then(serde_json::Value::as_str)
                .map(str::trim)
                .filter(|path| !path.is_empty())
            else {
                return Ok(OmpPreToolAction::Block {
                    reason: "write requires a path target for stateful authorization".to_string(),
                });
            };
            Ok(OmpPreToolAction::Targets(vec![PatchTarget::write(path)]))
        }
        tool_name if tool_name.eq_ignore_ascii_case("edit") => {
            let targets = extract_omp_edit_targets(&input.tool_input);
            if targets.is_empty() {
                return Ok(OmpPreToolAction::Block {
                    reason: "edit requires hashline file targets for stateful authorization"
                        .to_string(),
                });
            }
            Ok(OmpPreToolAction::Targets(targets))
        }
        tool_name
            if tool_name.eq_ignore_ascii_case("ext_ro_bash")
                || tool_name.eq_ignore_ascii_case("ext_rw_bash")
                || tool_name.eq_ignore_ascii_case("sandbox_bash") =>
        {
            Ok(OmpPreToolAction::Allow)
        }
        tool_name if tool_name.eq_ignore_ascii_case("bash") => Ok(OmpPreToolAction::Block {
            reason: format!(
                "OMP raw {} is denied; use sandbox_bash for stateful sandbox run profiles except --fs external, ext_ro_bash for read-only external operations, or ext_rw_bash for external writes",
                input.tool_name
            ),
        }),
        tool_name if is_omp_eval_tool(tool_name) => Ok(OmpPreToolAction::Block {
            reason: format!(
                "OMP eval tool {} is denied; use sandbox_bash for stateful sandbox run profiles except --fs external, ext_ro_bash for read-only external operations, or ext_rw_bash for external writes",
                input.tool_name
            ),
        }),
        tool_name if is_omp_safe_without_repo_write_authorization(tool_name) => {
            Ok(OmpPreToolAction::Allow)
        }
        _ if is_stateful_control_plane_tool(&input.tool_name) => Ok(OmpPreToolAction::Allow),
        _ if is_user_allowed_tool(global_paths, repo_root, &input.tool_name) => {
            Ok(OmpPreToolAction::Allow)
        }
        _ => {
            record_unclassified_tool(global_paths, repo_root, &input.tool_name);
            Ok(OmpPreToolAction::Block {
                reason: format!(
                    "unclassified OMP tool {} may write or execute and requires explicit stateful classification",
                    input.tool_name
                ),
            })
        }
    }
}

fn is_omp_eval_tool(tool_name: &str) -> bool {
    [
        "eval",
        "py",
        "python",
        "javascript",
        "js",
        "rb",
        "ruby",
        "jl",
        "julia",
    ]
    .iter()
    .any(|eval_tool| tool_name.eq_ignore_ascii_case(eval_tool))
}

fn is_omp_safe_without_repo_write_authorization(tool_name: &str) -> bool {
    [
        "ask",
        "ast_grep",
        "browser",
        "find",
        "generate_image",
        "grep",
        "irc",
        "job",
        "read",
        "report_tool_issue",
        "search",
        "search_tool_bm25",
        "task",
        "todo",
        "web_search",
    ]
    .iter()
    .any(|safe_tool| tool_name.eq_ignore_ascii_case(safe_tool))
}

fn omp_sandbox_run_action(command: &str) -> Option<OmpPreToolAction> {
    let invocation = parse_sandbox_run_bash_invocation(command).ok()?;
    if !is_trusted_stateful_executable(&invocation.executable) {
        return Some(OmpPreToolAction::Block {
            reason: "stateful sandbox run requires the trusted absolute stateful binary"
                .to_string(),
        });
    }
    if let Err(error) = validate_sandbox_run_request_shape(&invocation.request) {
        return Some(OmpPreToolAction::Block {
            reason: error.to_string(),
        });
    }
    if invocation.request.fs == SandboxFsProfile::WriteTargets {
        let targets = invocation
            .request
            .write_targets
            .iter()
            .chain(invocation.request.create_targets.iter())
            .map(|path| PatchTarget::write(path))
            .chain(
                invocation
                    .request
                    .write_dirs
                    .iter()
                    .map(|path| PatchTarget::write_directory(path)),
            )
            .collect();
        return Some(OmpPreToolAction::Targets(targets));
    }
    if invocation.request.fs == SandboxFsProfile::External {
        return Some(OmpPreToolAction::Block {
            reason: "OMP raw Bash cannot run stateful sandbox run --fs external; use ext_ro_bash for read-only external operations or ext_rw_bash for external writes".to_string(),
        });
    }
    Some(OmpPreToolAction::Allow)
}

fn authorize_omp_targets(
    input: &OmpPreToolUseInput,
    runtime: Option<&ServerRuntime>,
    repo_root: Option<&Path>,
    cwd: Option<&Path>,
    identity: Option<&RepoIdentity>,
    targets: Vec<PatchTarget>,
) -> anyhow::Result<OmpHookOutcome> {
    let Some(targets) = normalize_targets(targets, repo_root, cwd)? else {
        return Ok(OmpHookOutcome::Block {
            reason: format!("{} target is outside the enabled repo", input.tool_name),
        });
    };

    let Some(runtime) = runtime else {
        let reason = format!(
            "{} writes require a reachable stateful server, exact file intent, and a same-session file lease",
            input.tool_name
        );
        return Ok(OmpHookOutcome::Block { reason });
    };

    let workspace_id = input
        .workspace_id
        .clone()
        .unwrap_or_else(|| effective_workspace_id(runtime, identity));
    for target in targets {
        let mut payload = json!({
            "action": target.action,
            "path": target.path,
        });
        if let Some(new_path) = &target.new_path {
            payload["old_path"] = json!(target.path);
            payload["new_path"] = json!(new_path);
        }
        let mut body = protocol_envelope(ProtocolEnvelopeArgs {
            runtime,
            request_id: uuid::Uuid::new_v4().to_string(),
            session_id: input.session_id.clone(),
            workspace_id: workspace_id.clone(),
            identity: identity.cloned(),
            source_kind: "hook",
            event: "omp_pre_tool_use",
            source_ref: "hook:omp_pre_tool_use",
            source_tool_name: Some(input.tool_name.as_str()),
            payload,
        });
        body["metadata"] = input.audit_metadata();

        let response = match post_json(runtime, "/v1/authorize", &body) {
            Ok(response) => response,
            Err(error) => {
                let reason = authorization_unavailable_reason(&error);
                return Ok(OmpHookOutcome::Block { reason });
            }
        };

        if !(200..300).contains(&response.status_code) {
            let reason = format!(
                "stateful authorization failed with HTTP {}: {}",
                response.status_code, response.body
            );
            return Ok(OmpHookOutcome::Block { reason });
        }

        let decision: AuthorizeDecision = match serde_json::from_str(&response.body) {
            Ok(decision) => decision,
            Err(error) => {
                let reason = authorization_unavailable_reason(&error);
                return Ok(OmpHookOutcome::Block { reason });
            }
        };
        if decision.decision != "allow" {
            let reason = authorization_denial_reason(decision);
            return Ok(OmpHookOutcome::Block { reason });
        }
    }

    Ok(OmpHookOutcome::Allow)
}

fn extract_omp_edit_targets(input: &serde_json::Value) -> Vec<PatchTarget> {
    let Some(edit_input) = input.get("input").and_then(serde_json::Value::as_str) else {
        return Vec::new();
    };
    edit_input
        .lines()
        .filter_map(|line| {
            let line = line.trim();
            let header = line.strip_prefix('[')?.strip_suffix(']')?;
            let (path, _) = header.split_once('#')?;
            let path = path.trim();
            (!path.is_empty()).then(|| PatchTarget::write(path))
        })
        .collect()
}

pub fn handle_omp_session_start_with_runtime(
    input: &str,
    runtime: &ServerRuntime,
) -> anyhow::Result<OmpSessionStartOutput> {
    let input: OmpSessionEventInput = serde_json::from_str(input)?;
    let workspace_id = input
        .workspace_id
        .clone()
        .unwrap_or_else(|| effective_workspace_id(runtime, None));
    post_omp_session_event(
        runtime,
        "/v1/session/register",
        "omp_session_start",
        &input,
        None,
    )?;
    Ok(omp_session_start_output(
        runtime,
        &input.session_id,
        workspace_id,
    ))
}

fn handle_omp_session_start_with_identity(
    input: &str,
    runtime: &ServerRuntime,
    repo_root: &Path,
    identity: Option<&RepoIdentity>,
) -> anyhow::Result<OmpSessionStartOutput> {
    let input: OmpSessionEventInput = serde_json::from_str(input)?;
    let workspace_id = input
        .workspace_id
        .clone()
        .unwrap_or_else(|| effective_workspace_id(runtime, identity));
    write_current_session_file_for_explicit_session(
        repo_root,
        &CurrentSession::new(input.session_id.clone(), workspace_id.clone()),
    )?;
    post_omp_session_event(
        runtime,
        "/v1/session/register",
        "omp_session_start",
        &input,
        identity,
    )?;
    Ok(omp_session_start_output(
        runtime,
        &input.session_id,
        workspace_id,
    ))
}

fn omp_session_start_output(
    runtime: &ServerRuntime,
    session_id: &str,
    workspace_id: String,
) -> OmpSessionStartOutput {
    OmpSessionStartOutput {
        decision: "allow",
        session_id: session_id.to_string(),
        workspace_id: workspace_id.clone(),
        notifications_stream: OmpNotificationsStream {
            base_url: runtime.base_url.clone(),
            authorization: format!("Bearer {}", runtime.token),
            session_id: session_id.to_string(),
            workspace_id,
        },
    }
}

pub fn handle_omp_post_tool_use_with_runtime(
    input: &str,
    runtime: &ServerRuntime,
) -> anyhow::Result<()> {
    let input: OmpSessionEventInput = serde_json::from_str(input)?;
    post_omp_post_tool_use_event(runtime, &input, None, None)
}

fn handle_omp_post_tool_use_with_identity(
    input: &str,
    runtime: &ServerRuntime,
    repo_root: Option<&Path>,
    identity: Option<&RepoIdentity>,
) -> anyhow::Result<()> {
    let input: OmpSessionEventInput = serde_json::from_str(input)?;
    post_omp_post_tool_use_event(runtime, &input, repo_root, identity)
}

fn post_omp_post_tool_use_event(
    runtime: &ServerRuntime,
    input: &OmpSessionEventInput,
    repo_root: Option<&Path>,
    identity: Option<&RepoIdentity>,
) -> anyhow::Result<()> {
    post_omp_session_event(
        runtime,
        "/v1/session/heartbeat",
        "omp_post_tool_use",
        input,
        identity,
    )?;
    refresh_omp_post_tool_lease_observations(input, runtime, repo_root, identity);
    release_omp_post_tool_leases(input, runtime, repo_root, identity);
    Ok(())
}

fn refresh_omp_post_tool_lease_observations(
    input: &OmpSessionEventInput,
    runtime: &ServerRuntime,
    repo_root: Option<&Path>,
    identity: Option<&RepoIdentity>,
) {
    let Some(identity) = identity else {
        return;
    };
    let Some(repo_root) = repo_root else {
        return;
    };
    let Ok(targets) = omp_post_tool_refresh_targets(input, repo_root) else {
        return;
    };
    let workspace_id = input
        .workspace_id
        .clone()
        .unwrap_or_else(|| effective_workspace_id(runtime, Some(identity)));
    let mut paths = BTreeSet::new();
    for target in targets {
        paths.insert(target.path);
        if let Some(new_path) = target.new_path {
            paths.insert(new_path);
        }
    }

    for path in paths {
        let body = json!({
            "session_id": input.session_id,
            "workspace_id": workspace_id,
            "path": path,
            "root": identity.root,
        });
        let Ok(response) = post_json(runtime, "/v1/lease/refresh-observation", &body) else {
            continue;
        };
        if !(200..300).contains(&response.status_code) {
            continue;
        }
    }
}

fn release_omp_post_tool_leases(
    input: &OmpSessionEventInput,
    runtime: &ServerRuntime,
    repo_root: Option<&Path>,
    identity: Option<&RepoIdentity>,
) {
    let Some(identity) = identity else {
        return;
    };
    let Some(repo_root) = repo_root else {
        return;
    };
    let Ok(targets) = omp_post_tool_refresh_targets(input, repo_root) else {
        return;
    };
    let workspace_id = input
        .workspace_id
        .clone()
        .unwrap_or_else(|| effective_workspace_id(runtime, Some(identity)));
    let mut paths = BTreeSet::new();
    for target in targets {
        paths.insert(target.path);
        if let Some(new_path) = target.new_path {
            paths.insert(new_path);
        }
    }

    for path in paths {
        let body = json!({
            "session_id": input.session_id,
            "workspace_id": workspace_id,
            "path": path,
        });
        let Ok(response) = post_json(runtime, "/v1/lease/release", &body) else {
            continue;
        };
        if !(200..300).contains(&response.status_code) {
            continue;
        }
    }
}

fn omp_post_tool_refresh_targets(
    input: &OmpSessionEventInput,
    repo_root: &Path,
) -> anyhow::Result<Vec<PatchTarget>> {
    let cwd = input.cwd.as_deref();
    let tool_name = input.tool_name.as_deref().map(runtime_tool_name_leaf);
    let targets = match tool_name {
        Some(tool_name) if tool_name.eq_ignore_ascii_case("write") => {
            let Some(path) = input
                .tool_input
                .get("path")
                .and_then(serde_json::Value::as_str)
                .map(str::trim)
                .filter(|path| !path.is_empty())
            else {
                return Ok(Vec::new());
            };
            vec![PatchTarget::write(path)]
        }
        Some(tool_name) if tool_name.eq_ignore_ascii_case("edit") => {
            extract_omp_edit_targets(&input.tool_input)
        }
        Some(tool_name)
            if tool_name.eq_ignore_ascii_case("bash")
                || tool_name.eq_ignore_ascii_case("python") =>
        {
            match input.command().and_then(omp_sandbox_run_action) {
                Some(OmpPreToolAction::Targets(targets)) => targets,
                _ => Vec::new(),
            }
        }
        _ => Vec::new(),
    };

    normalize_targets(targets, Some(repo_root), cwd).map(|targets| targets.unwrap_or_default())
}

fn post_omp_session_event(
    runtime: &ServerRuntime,
    path: &str,
    event: &str,
    input: &OmpSessionEventInput,
    identity: Option<&RepoIdentity>,
) -> anyhow::Result<()> {
    let workspace_id = input
        .workspace_id
        .clone()
        .unwrap_or_else(|| effective_workspace_id(runtime, identity));
    let mut body = json!({
        "session_id": &input.session_id,
        "workspace_id": workspace_id,
        "source": {
            "kind": "hook",
            "event": event,
            "ref": "hook:omp_session",
            "tool_name": &input.tool_name,
        },
        "metadata": input.audit_metadata(),
    });
    if let Some(identity) = identity {
        body["repo_id"] = json!(identity.repo_id);
        body["worktree_id"] = json!(identity.worktree_id);
        body["root"] = json!(identity.root);
        body["branch"] = json!(identity.branch);
    }

    let response = post_json(runtime, path, &body)?;
    if !(200..300).contains(&response.status_code) {
        anyhow::bail!(
            "OMP session event failed with HTTP {}: {}",
            response.status_code,
            response.body
        );
    }
    Ok(())
}

fn prepare_pre_tool_use_runtime(
    repo_root: &Path,
    paths: &GlobalPaths,
    input: &str,
) -> anyhow::Result<ServerRuntime> {
    if !runtime_env_override_is_configured() {
        ensure_server(paths)?;
    }
    let runtime = discover_runtime_with_global(repo_root, paths)?;
    remember_current_session(repo_root, &runtime, input)?;
    Ok(runtime)
}

pub fn handle_session_start_in_repo(
    input: &str,
    repo_root: impl AsRef<Path>,
) -> anyhow::Result<()> {
    let start = hook_start_dir_or(input, repo_root.as_ref());
    let paths = GlobalPaths::from_env()?;
    let repo_root = match repo_gate(&paths, start)? {
        RepoGate::Enabled { repo_root } => {
            if !runtime_env_override_is_configured() {
                ensure_server(&paths)?;
            }
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
            if !runtime_env_override_is_configured() {
                ensure_server(&paths)?;
            }
            repo_root
        }
        RepoGate::Disabled | RepoGate::OutsideGitRepo => return Ok(()),
    };
    let runtime = discover_runtime_with_global(&repo_root, &paths)?;
    remember_current_session(&repo_root, &runtime, input)?;
    let identity = repo_identity(&paths, &repo_root)?;
    if let Err(error) =
        handle_post_tool_use_with_runtime(input, &runtime, Some(&repo_root), Some(&identity))
    {
        let input: SessionEventInput = serde_json::from_str(input)?;
        let workspace_id = effective_workspace_id(&runtime, Some(&identity));
        queue_session_heartbeat_outbox(
            &paths,
            &workspace_id,
            input.stateful_session_id(),
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
            if !runtime_env_override_is_configured() {
                ensure_server(&paths)?;
            }
            repo_root
        }
        RepoGate::Disabled | RepoGate::OutsideGitRepo => return Ok(String::new()),
    };
    let runtime = discover_runtime_with_global(&repo_root, &paths)?;
    remember_current_session(&repo_root, &runtime, input)?;
    let identity = repo_identity(&paths, &repo_root)?;
    let input: UserPromptSubmitInput = serde_json::from_str(input)?;
    if user_prompt_context_rendered(&repo_root, input.stateful_session_id()) {
        return Ok(String::new());
    }
    let prompt_text = handle_user_prompt_submit_with_runtime(&input, &runtime, Some(&identity))?;
    mark_user_prompt_context_rendered(&repo_root, input.stateful_session_id());
    Ok(prompt_text)
}

pub fn handle_stop_in_repo(input: &str, repo_root: impl AsRef<Path>) -> anyhow::Result<()> {
    let start = hook_start_dir_or(input, repo_root.as_ref());
    let paths = GlobalPaths::from_env()?;
    let repo_root = match repo_gate(&paths, start)? {
        RepoGate::Enabled { repo_root } => {
            if !runtime_env_override_is_configured() {
                ensure_server(&paths)?;
            }
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
    let identity = GlobalPaths::from_env()
        .ok()
        .and_then(|paths| repo_identity_for_enabled_repo(&paths, repo_root).ok());
    let workspace_id = effective_workspace_id(runtime, identity.as_ref());
    write_current_session_file_for_explicit_session(
        repo_root,
        &CurrentSession::new(input.stateful_session_id(), workspace_id),
    )
}

fn handle_session_start_with_runtime(
    input: &str,
    runtime: &ServerRuntime,
    identity: Option<&RepoIdentity>,
) -> anyhow::Result<()> {
    let input: SessionStartInput = serde_json::from_str(input)?;
    post_session_event(
        runtime,
        "/v1/session/register",
        input.stateful_session_id(),
        identity,
    )
}

fn handle_post_tool_use_with_runtime(
    input: &str,
    runtime: &ServerRuntime,
    repo_root: Option<&Path>,
    identity: Option<&RepoIdentity>,
) -> anyhow::Result<()> {
    let input: SessionEventInput = serde_json::from_str(input)?;
    post_session_event(
        runtime,
        "/v1/session/heartbeat",
        input.stateful_session_id(),
        identity,
    )?;
    refresh_post_tool_lease_observations(&input, runtime, repo_root, identity);
    release_post_tool_leases(&input, runtime, repo_root, identity);
    Ok(())
}

fn handle_user_prompt_submit_with_runtime(
    input: &UserPromptSubmitInput,
    runtime: &ServerRuntime,
    identity: Option<&RepoIdentity>,
) -> anyhow::Result<String> {
    let prompt_text = render_context_prompt_text(runtime, input.stateful_session_id(), identity)?;
    Ok(with_stateful_command_policy_reminder(prompt_text))
}

fn render_context_prompt_text(
    runtime: &ServerRuntime,
    session_id: &str,
    identity: Option<&RepoIdentity>,
) -> anyhow::Result<String> {
    Ok(render_context_response(runtime, session_id, identity)?.prompt_text)
}

fn render_context_response(
    runtime: &ServerRuntime,
    session_id: &str,
    identity: Option<&RepoIdentity>,
) -> anyhow::Result<ContextRenderResponse> {
    let workspace_id = effective_workspace_id(runtime, identity);
    let mut body = json!({
        "session_id": session_id,
        "workspace_id": workspace_id,
        "mode": "brief"
    });
    if let Some(identity) = identity
        && let Some(object) = body.as_object_mut()
    {
        object.insert("repo_id".to_string(), json!(&identity.repo_id));
        object.insert("worktree_id".to_string(), json!(&identity.worktree_id));
        object.insert("root".to_string(), json!(&identity.root));
        object.insert("branch".to_string(), json!(&identity.branch));
    }
    let response = post_json(runtime, "/v1/context/render", &body)?;

    if !(200..300).contains(&response.status_code) {
        anyhow::bail!(
            "context render failed with HTTP {}: {}",
            response.status_code,
            response.body
        );
    }

    let response: ContextRenderResponse = serde_json::from_str(&response.body)?;
    Ok(response)
}

fn user_prompt_context_rendered(repo_root: &Path, session_id: &str) -> bool {
    user_prompt_context_marker_path(repo_root, session_id).exists()
}

fn mark_user_prompt_context_rendered(repo_root: &Path, session_id: &str) {
    let path = user_prompt_context_marker_path(repo_root, session_id);
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let _ = fs::write(path, b"rendered\n");
}

fn user_prompt_context_marker_path(repo_root: &Path, session_id: &str) -> PathBuf {
    repo_root
        .join(".stateful_core")
        .join("runtime")
        .join("prompt_context")
        .join(format!("{session_id}.sent"))
}

fn with_stateful_command_policy_reminder(prompt_text: String) -> String {
    let reminder = stateful_command_policy_reminder();
    if prompt_text.trim().is_empty() {
        reminder
    } else {
        format!("{reminder}\n\n{prompt_text}")
    }
}

fn stateful_command_policy_reminder() -> String {
    let binary = stateful_binary_for_guidance();
    format!(
        "Stateful command policy reminder:\n- First inspect current state with canonical Stateful MCP tool names such as `state_current_read` or `state_context_render` so you know who is active, what you already hold, and what may conflict.\n- Before using Bash or eval tools, use the `stateful-command-policy` skill.\n- Use canonical Stateful MCP tool names (`state_intent_declare`, `state_lease_acquire`) for coordination. If the active tool list exposes only runtime-specific tool names, call the exact shown equivalent such as Codex `mcp__stateful__state_intent_declare` or OMP `mcp__stateful_state_intent_declare`. Do not run `stateful intent declare` or `stateful mcp call` through Bash.\n- Raw Bash is denied for Codex. OMP raw Bash and Python/JavaScript/JS/Ruby/Julia eval tools are denied; use `sandbox_bash` for stateful sandbox run profiles except `--fs external`, `ext_ro_bash` for read-only external operations, and `ext_rw_bash` for external writes.\n- Use `{binary} sandbox run --fs read-only --network disabled --command <cmd>` only as the read-only shell fallback when native tools are unavailable or insufficient.\n- Use `{binary} sandbox process find <selector>` for structured process lookup instead of raw `ps` or `pgrep`.\n- Use `{binary} sandbox run --fs write-targets --write-target <file> --command <cmd>` only after declaring exact intent and acquiring the same-session file lease.\n- Use `{binary} sandbox run --fs build --network enabled --write-dir <scratch-purpose> --command <cmd>` for builds/tests with disposable artifacts.\n- Use `{binary} sandbox run --fs git --network disabled --command 'git <args>'` for local git operations; enable network only for remote git operations.\n- Use `{binary} sandbox run --fs github-pr --network enabled --command 'gh pr <list|view|status|create> ...'` for GitHub PR inspection or creation.",
    )
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
        input.stateful_session_id(),
        identity,
    )
}

fn post_session_event(
    runtime: &ServerRuntime,
    path: &str,
    session_id: &str,
    identity: Option<&RepoIdentity>,
) -> anyhow::Result<()> {
    let workspace_id = effective_workspace_id(runtime, identity);
    let mut body = json!({
        "session_id": session_id,
        "workspace_id": workspace_id,
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

fn refresh_post_tool_lease_observations(
    input: &SessionEventInput,
    runtime: &ServerRuntime,
    repo_root: Option<&Path>,
    identity: Option<&RepoIdentity>,
) {
    let Some(identity) = identity else {
        return;
    };
    let Some(repo_root) = repo_root else {
        return;
    };
    let Ok(targets) = post_tool_refresh_targets(input, repo_root) else {
        return;
    };
    let workspace_id = effective_workspace_id(runtime, Some(identity));
    let mut paths = BTreeSet::new();
    for target in targets {
        paths.insert(target.path);
        if let Some(new_path) = target.new_path {
            paths.insert(new_path);
        }
    }

    for path in paths {
        let body = json!({
            "session_id": input.stateful_session_id(),
            "workspace_id": workspace_id,
            "path": path,
            "root": identity.root,
        });
        let Ok(response) = post_json(runtime, "/v1/lease/refresh-observation", &body) else {
            continue;
        };
        if !(200..300).contains(&response.status_code) {
            continue;
        }
    }
}

fn release_post_tool_leases(
    input: &SessionEventInput,
    runtime: &ServerRuntime,
    repo_root: Option<&Path>,
    identity: Option<&RepoIdentity>,
) {
    let Some(identity) = identity else {
        return;
    };
    let Some(repo_root) = repo_root else {
        return;
    };
    let Ok(targets) = post_tool_refresh_targets(input, repo_root) else {
        return;
    };
    let workspace_id = effective_workspace_id(runtime, Some(identity));
    let mut paths = BTreeSet::new();
    for target in targets {
        paths.insert(target.path);
        if let Some(new_path) = target.new_path {
            paths.insert(new_path);
        }
    }

    for path in paths {
        let body = json!({
            "session_id": input.stateful_session_id(),
            "workspace_id": workspace_id,
            "path": path,
        });
        let Ok(response) = post_json(runtime, "/v1/lease/release", &body) else {
            continue;
        };
        if !(200..300).contains(&response.status_code) {
            continue;
        }
    }
}

fn post_tool_refresh_targets(
    input: &SessionEventInput,
    repo_root: &Path,
) -> anyhow::Result<Vec<PatchTarget>> {
    let cwd = input.cwd.as_deref();
    let targets = match input.tool_name.as_deref() {
        Some("apply_patch") => input
            .patch_text()
            .map(extract_apply_patch_write_targets)
            .unwrap_or_default(),
        Some("file_change") => extract_file_change_targets(&input.tool_input),
        Some("Edit") | Some("Write") => {
            let Some(path) = input
                .tool_input
                .get("file_path")
                .or_else(|| input.tool_input.get("path"))
                .and_then(serde_json::Value::as_str)
                .map(str::trim)
                .filter(|path| !path.is_empty())
            else {
                return Ok(Vec::new());
            };
            vec![PatchTarget::write(path)]
        }
        _ => Vec::new(),
    };

    normalize_targets(targets, Some(repo_root), cwd).map(|targets| targets.unwrap_or_default())
}

fn effective_workspace_id(runtime: &ServerRuntime, identity: Option<&RepoIdentity>) -> String {
    effective_workspace_id_for_repo(&runtime.workspace_id, identity)
}

fn handle_pre_tool_use_with_runtime(
    input: &str,
    runtime: Option<&ServerRuntime>,
    repo_root: Option<&Path>,
    cwd: Option<&Path>,
) -> anyhow::Result<HookOutcome> {
    let input: PreToolUseInput = serde_json::from_str(input)?;
    let global_paths = GlobalPaths::from_env().ok();
    let identity = repo_root.and_then(|repo_root| {
        global_paths
            .as_ref()
            .and_then(|paths| repo_identity_for_enabled_repo(paths, repo_root).ok())
    });

    let tool_name = runtime_tool_name_leaf(&input.tool_name);
    match tool_name {
        tool_name if tool_name.eq_ignore_ascii_case("bash") => {
            let outcome = authorize_bash(&input)?;
            Ok(with_file_tool_live_context(
                outcome,
                &input,
                runtime,
                identity.as_ref(),
            ))
        }
        tool_name if tool_name.eq_ignore_ascii_case("apply_patch") => {
            authorize_apply_patch(&input, runtime, repo_root, cwd, identity.as_ref())
        }
        tool_name if tool_name.eq_ignore_ascii_case("file_change") => {
            authorize_file_change_tool(&input, runtime, repo_root, cwd, identity.as_ref())
        }
        tool_name if tool_name.eq_ignore_ascii_case("edit") || tool_name.eq_ignore_ascii_case("write") => {
            authorize_file_write_tool(&input, runtime, repo_root, cwd, identity.as_ref())
        }
        tool_name if tool_name.starts_with("mcp__filesystem__") => Ok(HookOutcome::Deny {
            reason: "filesystem MCP writes require stateful authorization; read-only MCP calls are not yet classified".to_string(),
        }),
        tool_name if is_remote_repository_mutation_tool(tool_name) => Ok(HookOutcome::Deny {
            reason: format!(
                "remote repository mutation tool {tool_name} is not covered by local stateful repo authorization; use local git/stateful workflows or an explicit external approval path"
            ),
        }),
        tool_name if is_safe_without_repo_write_authorization(tool_name) => Ok(HookOutcome::Allow),
        _ if is_user_allowed_tool(global_paths.as_ref(), repo_root, &input.tool_name) => {
            Ok(HookOutcome::Allow)
        }
        _ => {
            record_unclassified_tool(global_paths.as_ref(), repo_root, &input.tool_name);
            Ok(HookOutcome::Deny {
                reason: format!(
                    "unclassified tool {} may write or execute and requires explicit stateful classification before it can run in an enabled repository",
                    input.tool_name
                ),
            })
        }
    }
}

fn runtime_tool_name_leaf(tool_name: &str) -> &str {
    tool_name
        .rsplit(['.', '/'])
        .next()
        .unwrap_or(tool_name)
        .trim_start_matches('_')
}

fn is_user_allowed_tool(
    paths: Option<&GlobalPaths>,
    repo_root: Option<&Path>,
    tool_name: &str,
) -> bool {
    let (Some(paths), Some(repo_root)) = (paths, repo_root) else {
        return false;
    };

    tool_allowed_for_enabled_repo(paths, repo_root, tool_name).unwrap_or(false)
}

fn record_unclassified_tool(
    paths: Option<&GlobalPaths>,
    repo_root: Option<&Path>,
    tool_name: &str,
) {
    let (Some(paths), Some(repo_root)) = (paths, repo_root) else {
        return;
    };

    let _ = record_unclassified_tool_for_repo(paths, repo_root, tool_name);
}

fn is_safe_without_repo_write_authorization(tool_name: &str) -> bool {
    is_builtin_safe_tool(tool_name)
        || is_stateful_control_plane_tool(tool_name)
        || is_github_metadata_tool(tool_name)
        || is_teams_connector_tool(tool_name)
}

fn is_builtin_safe_tool(tool_name: &str) -> bool {
    [
        "Read",
        "Grep",
        "Glob",
        "LS",
        "NotebookRead",
        "WebFetch",
        "WebSearch",
        "TodoWrite",
        "update_plan",
        "tool_search",
        "tool_search_tool",
        "search_tool_bm25",
        "get_goal",
        "create_goal",
        "update_goal",
        "request_user_input",
        "view_image",
        "task",
        "spawn_agent",
        "multi_agent_v1spawn_agent",
        "wait_agent",
        "multi_agent_v1wait_agent",
        "send_input",
        "multi_agent_v1send_input",
        "close_agent",
        "multi_agent_v1close_agent",
        "resume_agent",
        "multi_agent_v1resume_agent",
    ]
    .iter()
    .any(|safe_tool| tool_name.eq_ignore_ascii_case(safe_tool))
}

fn is_stateful_control_plane_tool(tool_name: &str) -> bool {
    tool_name.starts_with("mcp__stateful__")
        || tool_name.starts_with("mcp__stateful_")
        || is_canonical_stateful_mcp_tool(tool_name)
}

fn is_canonical_stateful_mcp_tool(tool_name: &str) -> bool {
    matches!(
        tool_name,
        "state_session_register"
            | "state.session.register"
            | "state_session_heartbeat"
            | "state.session.heartbeat"
            | "state_intent_declare"
            | "state.intent.declare"
            | "state_intent_request"
            | "state.intent.request"
            | "state_intent_claim"
            | "state.intent.claim"
            | "state_intent_cancel"
            | "state.intent.cancel"
            | "state_lease_acquire"
            | "state.lease.acquire"
            | "state_lease_release"
            | "state.lease.release"
            | "state_activity_observe"
            | "state.activity.observe"
            | "state_activity_finalize"
            | "state.activity.finalize"
            | "state_conflicts_check"
            | "state.conflicts.check"
            | "state_current_read"
            | "state.current.read"
            | "state_events_read"
            | "state.events.read"
            | "state_context_render"
            | "state.context.render"
            | "state_reconcile_ack"
            | "state.reconcile.ack"
            | "state_notifications_poll"
            | "state.notifications.poll"
            | "state_resume_next"
            | "state.resume.next"
    )
}

fn is_github_metadata_tool(tool_name: &str) -> bool {
    if !tool_name.starts_with("mcp__codex_apps__github__") {
        return false;
    }
    matches!(
        tool_suffix(tool_name),
        "update_pull_request"
            | "create_pull_request"
            | "add_comment_to_issue"
            | "fetch_file"
            | "search_branches"
            | "search_repositories"
    )
}

fn is_teams_connector_tool(tool_name: &str) -> bool {
    if !tool_name.starts_with("mcp__codex_apps__microsoft_teams__") {
        return false;
    }
    matches!(tool_suffix(tool_name), "resolve_channel" | "send_message")
}

fn tool_suffix(tool_name: &str) -> &str {
    tool_name
        .rsplit("__")
        .next()
        .unwrap_or(tool_name)
        .trim_start_matches('_')
}

fn is_remote_repository_mutation_tool(tool_name: &str) -> bool {
    if !tool_name.starts_with("mcp__codex_apps__github__") {
        return false;
    }
    matches!(
        tool_suffix(tool_name),
        "create_file" | "update_file" | "create_branch" | "update_ref"
    )
}

fn authorize_bash(input: &PreToolUseInput) -> anyhow::Result<HookOutcome> {
    let command = input.command().unwrap_or_default();
    if let Some(coordination) = deny_stateful_coordination_bash(command) {
        return Ok(coordination);
    }

    let sandbox = authorize_sandbox_run_bash(command);
    if sandbox == HookOutcome::Allow {
        return Ok(sandbox);
    }

    #[cfg(feature = "codex-benchmark")]
    if let Some(tmux) = authorize_tmux_nested_codex_benchmark_bash(command) {
        return Ok(tmux);
    }

    let control = authorize_stateful_control_bash(command);
    if control == HookOutcome::Allow || command_mentions_stateful_control(command) {
        return Ok(control);
    }

    Ok(sandbox)
}

fn deny_stateful_coordination_bash(command: &str) -> Option<HookOutcome> {
    if !command_mentions_stateful_coordination(command) {
        return None;
    }
    Some(bash_policy_deny(stateful_coordination_mcp_guidance()))
}

fn command_mentions_stateful_coordination(command: &str) -> bool {
    let Ok(words) = split_simple_command_words(command) else {
        return false;
    };
    match words.get(1).map(String::as_str) {
        Some("intent") => true,
        Some("mcp")
            if words.get(2).is_some_and(|word| word == "call")
                && words
                    .get(3)
                    .is_some_and(|tool| is_stateful_coordination_mcp_tool(tool)) =>
        {
            true
        }
        _ => false,
    }
}

fn is_stateful_coordination_mcp_tool(tool_name: &str) -> bool {
    if is_stateful_control_plane_tool(tool_name) {
        return true;
    }
    matches!(
        tool_name,
        "state_intent_declare"
            | "state.intent.declare"
            | "state_intent_request"
            | "state.intent.request"
            | "state_intent_claim"
            | "state.intent.claim"
            | "state_intent_cancel"
            | "state.intent.cancel"
            | "state_lease_acquire"
            | "state.lease.acquire"
            | "state_lease_release"
            | "state.lease.release"
            | "state_notifications_poll"
            | "state.notifications.poll"
            | "state_resume_next"
            | "state.resume.next"
    )
}

fn stateful_coordination_mcp_guidance() -> &'static str {
    "Use canonical Stateful MCP tool names such as `state_intent_declare` and `state_lease_acquire`. If the active tool list exposes only runtime-specific tool names, call the exact shown equivalent such as Codex `mcp__stateful__state_intent_declare` or OMP `mcp__stateful_state_intent_declare`. Do not run `stateful intent declare` or `stateful mcp call` through Bash."
}

fn authorize_sandbox_run_bash(command: &str) -> HookOutcome {
    if split_simple_command_words(command)
        .ok()
        .is_some_and(|words| {
            words.len() >= 3 && words[1] == "sandbox" && words[2] == "run-nested-codex-benchmark"
        })
    {
        #[cfg(not(feature = "codex-benchmark"))]
        {
            return bash_policy_deny(
                "stateful sandbox run-nested-codex-benchmark hook authorization requires the codex-benchmark feature",
            );
        }
        #[cfg(feature = "codex-benchmark")]
        return authorize_nested_codex_benchmark_sandbox_bash(command);
    }

    if split_simple_command_words(command)
        .ok()
        .is_some_and(|words| {
            words.len() >= 4 && words[1] == "sandbox" && words[2] == "process" && words[3] == "find"
        })
    {
        return authorize_sandbox_process_find_bash(command);
    }

    let invocation = match parse_sandbox_run_bash_invocation(command) {
        Ok(invocation) => invocation,
        Err(reason) => return bash_policy_deny(reason),
    };

    if !is_trusted_stateful_executable(&invocation.executable) {
        return bash_policy_deny(
            "stateful sandbox run requires the trusted absolute stateful binary",
        );
    }
    if invocation.request.fs == SandboxFsProfile::External
        && !sandbox_external_profile_has_prompt_prefix(command)
    {
        return bash_policy_deny(
            "stateful sandbox external profile requires canonical prompt-matched prefix: `stateful sandbox run --fs external`",
        );
    }
    if let Err(error) = validate_sandbox_run_request_shape(&invocation.request) {
        return bash_policy_deny(error.to_string());
    }

    HookOutcome::Allow
}

fn sandbox_external_profile_has_prompt_prefix(command: &str) -> bool {
    split_simple_command_words(command)
        .ok()
        .is_some_and(|words| {
            words.len() >= 5
                && words[1] == "sandbox"
                && words[2] == "run"
                && words[3] == "--fs"
                && words[4] == "external"
        })
}

fn authorize_sandbox_process_find_bash(command: &str) -> HookOutcome {
    let invocation = match parse_sandbox_process_find_bash_invocation(command) {
        Ok(invocation) => invocation,
        Err(reason) => return bash_policy_deny(reason),
    };

    if !is_trusted_stateful_executable(&invocation.executable) {
        return bash_policy_deny(
            "stateful sandbox process find requires the trusted absolute stateful binary",
        );
    }
    if let Err(error) = validate_process_find_request(&invocation.request) {
        return bash_policy_deny(error.to_string());
    }

    HookOutcome::Allow
}

#[cfg(feature = "codex-benchmark")]
fn authorize_nested_codex_benchmark_sandbox_bash(command: &str) -> HookOutcome {
    let invocation = match parse_nested_codex_benchmark_sandbox_invocation(command) {
        Ok(invocation) => invocation,
        Err(reason) => return bash_policy_deny(reason),
    };

    if !is_trusted_stateful_executable(&invocation.executable) {
        return bash_policy_deny(
            "stateful sandbox run-nested-codex-benchmark requires the trusted absolute stateful binary",
        );
    }
    if invocation.purpose.trim().is_empty() {
        return bash_policy_deny("stateful sandbox run-nested-codex-benchmark requires --purpose");
    }
    if invocation.write_dir != "target" {
        return bash_policy_deny(
            "stateful sandbox run-nested-codex-benchmark requires --write-dir target",
        );
    }
    if !hook_path_is_under_target(&invocation.codex_home_root) {
        return bash_policy_deny(
            "stateful sandbox run-nested-codex-benchmark requires --codex-home-root under target",
        );
    }
    if let Some(docker_socket) = &invocation.docker_socket {
        if !Path::new(docker_socket).is_absolute() {
            return bash_policy_deny(
                "stateful sandbox run-nested-codex-benchmark requires --docker-socket to be an absolute path",
            );
        }
    }
    if invocation.command.trim().is_empty() {
        return bash_policy_deny(
            "stateful sandbox run-nested-codex-benchmark requires a non-empty --command",
        );
    }

    HookOutcome::Allow
}

#[cfg(feature = "codex-benchmark")]
fn authorize_tmux_nested_codex_benchmark_bash(command: &str) -> Option<HookOutcome> {
    if !command_starts_with_trusted_tmux(command) {
        return None;
    }
    match parse_tmux_nested_codex_benchmark_command(command) {
        Ok(Some(nested_command)) => Some(authorize_nested_codex_benchmark_sandbox_bash(
            &nested_command,
        )),
        Ok(None) => None,
        Err(reason) => Some(bash_policy_deny(reason)),
    }
}

fn authorize_stateful_control_bash(command: &str) -> HookOutcome {
    let invocation = match parse_stateful_control_invocation(command) {
        Ok(invocation) => invocation,
        Err(reason) => return bash_policy_deny(reason),
    };

    if !is_trusted_stateful_executable(&invocation.executable) {
        return bash_policy_deny(
            "stateful control commands require the trusted absolute stateful binary",
        );
    }

    HookOutcome::Allow
}

fn bash_policy_deny(reason: impl Into<String>) -> HookOutcome {
    HookOutcome::Deny {
        reason: format!("{} {}", reason.into(), bash_policy_guidance()),
    }
}

fn bash_policy_guidance() -> String {
    let binary = stateful_binary_for_guidance();
    format!(
        "Inspect current state first with `state_current_read` or `state_context_render`, then use the `stateful-command-policy` skill before Bash or eval tools. Raw Bash is denied for Codex. OMP raw Bash and Python/JavaScript/JS/Ruby/Julia eval tools are denied; use `sandbox_bash` for stateful sandbox run profiles except `--fs external`, `ext_ro_bash` for read-only external operations, and `ext_rw_bash` for external writes. Use canonical Stateful MCP tool names (`state_intent_declare`, `state_lease_acquire`) for coordination; if the active tool list exposes only runtime-specific tool names, call the exact shown equivalent such as Codex `mcp__stateful__state_intent_declare` or OMP `mcp__stateful_state_intent_declare`. Do not run `stateful intent declare` or `stateful mcp call` through Bash. For file search and inspection, use native read/search tools first and `{binary} sandbox run --fs read-only --network disabled --command <cmd>` only as fallback. For structured process lookup, use `{binary} sandbox process find <selector>` instead of raw `ps` or `pgrep`. For command-shaped writes, declare exact intent, acquire the same-session lease, then use `{binary} sandbox run --fs write-targets --write-target <file> --command <cmd>`. For builds/tests, use `{binary} sandbox run --fs build --network enabled --write-dir <scratch-purpose> --command <cmd>`. For local git, use `{binary} sandbox run --fs git --network disabled --command 'git <args>'`; enable network only for remote git operations. For GitHub PRs, use `{binary} sandbox run --fs github-pr --network enabled --command 'gh pr <list|view|status|create> ...'`.",
    )
}

fn stateful_binary_for_guidance() -> String {
    std::env::current_exe()
        .ok()
        .and_then(|path| path.canonicalize().ok().or(Some(path)))
        .map(|path| path.to_string_lossy().into_owned())
        .unwrap_or_else(|| "<absolute-stateful-binary>".to_string())
}

#[cfg(feature = "codex-benchmark")]
fn parse_nested_codex_benchmark_sandbox_invocation(
    command: &str,
) -> Result<NestedCodexBenchmarkSandboxInvocation, String> {
    reject_outer_shell_syntax(
        command,
        "Bash wrapper must be a single stateful sandbox run-nested-codex-benchmark command",
    )?;
    let words = split_simple_command_words(command)?;
    if words.is_empty() {
        return Err("Bash commands must use stateful sandbox run".to_string());
    }
    if first_word_is_env_assignment(&words[0]) {
        return Err("Bash wrapper must not use outer environment assignments".to_string());
    }
    if words.len() < 3 || words[1] != "sandbox" || words[2] != "run-nested-codex-benchmark" {
        return Err("Bash commands must use stateful sandbox run".to_string());
    }

    let mut purpose = None;
    let mut write_dir = None;
    let mut codex_home_root = None;
    let mut docker_socket = None;
    let mut inner_command = None;
    let mut index = 3;
    while index < words.len() {
        let arg = &words[index];
        match arg.as_str() {
            "--" => {
                return Err(
                    "stateful sandbox run-nested-codex-benchmark does not support argv mode"
                        .to_string(),
                );
            }
            "--purpose" => {
                index += 1;
                purpose = Some(parse_sandbox_run_arg_value(&words, index, "--purpose")?);
            }
            "--write-dir" => {
                index += 1;
                write_dir = Some(parse_sandbox_run_arg_value(&words, index, "--write-dir")?);
            }
            "--codex-home-root" => {
                index += 1;
                codex_home_root = Some(parse_sandbox_run_arg_value(
                    &words,
                    index,
                    "--codex-home-root",
                )?);
            }
            "--docker-socket" => {
                index += 1;
                docker_socket = Some(parse_sandbox_run_arg_value(
                    &words,
                    index,
                    "--docker-socket",
                )?);
            }
            "--command" => {
                if inner_command.is_some() {
                    return Err(
                        "stateful sandbox run-nested-codex-benchmark requires exactly one --command"
                            .to_string(),
                    );
                }
                index += 1;
                inner_command = Some(parse_sandbox_run_arg_value(&words, index, "--command")?);
            }
            "--timeout-seconds" => {
                index += 1;
                let timeout = parse_sandbox_run_arg_value(&words, index, "--timeout-seconds")?;
                if timeout.parse::<u64>().is_err() {
                    return Err(
                        "stateful sandbox run-nested-codex-benchmark --timeout-seconds requires an integer value"
                            .to_string(),
                    );
                }
            }
            _ => {
                return Err(format!(
                    "unsupported stateful sandbox run-nested-codex-benchmark argument `{arg}`"
                ));
            }
        }
        index += 1;
    }

    let Some(purpose) = purpose else {
        return Err("stateful sandbox run-nested-codex-benchmark requires --purpose".to_string());
    };
    let Some(write_dir) = write_dir else {
        return Err(
            "stateful sandbox run-nested-codex-benchmark requires --write-dir target".to_string(),
        );
    };
    let Some(codex_home_root) = codex_home_root else {
        return Err(
            "stateful sandbox run-nested-codex-benchmark requires --codex-home-root".to_string(),
        );
    };
    let Some(command) = inner_command else {
        return Err(
            "stateful sandbox run-nested-codex-benchmark requires exactly one --command"
                .to_string(),
        );
    };

    Ok(NestedCodexBenchmarkSandboxInvocation {
        executable: words[0].clone(),
        purpose,
        write_dir,
        codex_home_root,
        docker_socket,
        command,
    })
}

#[cfg(feature = "codex-benchmark")]
fn parse_tmux_nested_codex_benchmark_command(command: &str) -> Result<Option<String>, String> {
    reject_outer_shell_syntax(
        command,
        "Bash wrapper must be a single tmux new-session launcher",
    )?;
    let words = split_simple_command_words(command)?;
    if words.is_empty() {
        return Ok(None);
    }
    if !is_trusted_tmux_executable(&words[0]) {
        return Ok(None);
    }
    if words.len() < 2 || words[1] != "new-session" {
        return Err("tmux benchmark launcher supports only new-session".to_string());
    }

    let mut detached = false;
    let mut session_name = None;
    let mut nested_command = None;
    let mut index = 2;
    while index < words.len() {
        let arg = &words[index];
        match arg.as_str() {
            "-d" => detached = true,
            "-s" => {
                index += 1;
                session_name = Some(parse_tmux_arg_value(&words, index, "-s")?);
            }
            "-n" | "-c" => {
                index += 1;
                parse_tmux_arg_value(&words, index, arg)?;
            }
            _ if arg.starts_with('-') => {
                return Err(format!(
                    "unsupported tmux benchmark launcher argument `{arg}`"
                ));
            }
            _ => {
                if nested_command.is_some() || index + 1 != words.len() {
                    return Err(
                        "tmux benchmark launcher requires exactly one shell command".to_string()
                    );
                }
                nested_command = Some(arg.clone());
            }
        }
        index += 1;
    }

    if !detached {
        return Err("tmux benchmark launcher requires detached -d mode".to_string());
    }
    let Some(session_name) = session_name else {
        return Err("tmux benchmark launcher requires -s session name".to_string());
    };
    if !is_denovo_tmux_session_name(&session_name) {
        return Err("tmux benchmark launcher session name must be a DeNovo run id".to_string());
    }
    let Some(nested_command) = nested_command else {
        return Err("tmux benchmark launcher requires a nested benchmark command".to_string());
    };

    Ok(Some(nested_command))
}

#[cfg(feature = "codex-benchmark")]
fn parse_tmux_arg_value(words: &[String], index: usize, arg: &str) -> Result<String, String> {
    words
        .get(index)
        .cloned()
        .ok_or_else(|| format!("tmux benchmark launcher argument `{arg}` requires a value"))
}

#[cfg(feature = "codex-benchmark")]
fn is_trusted_tmux_executable(executable: &str) -> bool {
    matches!(executable, "/opt/homebrew/bin/tmux" | "/usr/bin/tmux")
}

#[cfg(feature = "codex-benchmark")]
fn command_starts_with_trusted_tmux(command: &str) -> bool {
    let trimmed = command.trim_start();
    ["/opt/homebrew/bin/tmux", "/usr/bin/tmux"]
        .iter()
        .any(|prefix| {
            trimmed == *prefix
                || trimmed
                    .strip_prefix(prefix)
                    .and_then(|rest| rest.chars().next())
                    .is_some_and(char::is_whitespace)
        })
}

#[cfg(feature = "codex-benchmark")]
fn is_denovo_tmux_session_name(session_name: &str) -> bool {
    !session_name.is_empty()
        && session_name.contains("-denovo-full-3-codex-")
        && session_name
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.'))
}

fn parse_stateful_control_invocation(command: &str) -> Result<StatefulControlInvocation, String> {
    reject_outer_shell_syntax(command, "Bash wrapper must be a single stateful command")?;
    let words = split_simple_command_words(command)?;
    if words.is_empty() {
        return Err("Bash commands must use a supported stateful command".to_string());
    }
    if first_word_is_env_assignment(&words[0]) {
        return Err("Bash wrapper must not use outer environment assignments".to_string());
    }
    if words.len() < 2 || !is_stateful_control_command(&words[1]) {
        return Err(
            "Bash commands must use stateful sandbox run or a trusted stateful server command"
                .to_string(),
        );
    }

    Ok(StatefulControlInvocation {
        executable: words[0].clone(),
    })
}

fn command_mentions_stateful_control(command: &str) -> bool {
    split_simple_command_words(command)
        .ok()
        .and_then(|words| words.get(1).cloned())
        .is_some_and(|command| is_stateful_control_command(&command))
}

fn is_stateful_control_command(command: &str) -> bool {
    matches!(command, "server")
}

#[cfg(feature = "codex-benchmark")]
fn parse_sandbox_run_arg_value(
    words: &[String],
    index: usize,
    arg: &str,
) -> Result<String, String> {
    words
        .get(index)
        .cloned()
        .ok_or_else(|| format!("stateful sandbox run argument `{arg}` requires a value"))
}

#[cfg(feature = "codex-benchmark")]
fn hook_path_is_under_target(path: &str) -> bool {
    let trimmed = path.trim().replace('\\', "/");
    if trimmed.is_empty() || trimmed.starts_with('/') {
        return false;
    }
    let mut segments = Vec::new();
    for segment in trimmed.split('/') {
        if segment.is_empty() || segment == "." {
            continue;
        }
        if segment == ".." || segment.eq_ignore_ascii_case(".git") {
            return false;
        }
        if segment.chars().any(char::is_control) {
            return false;
        }
        segments.push(segment);
    }
    segments.len() >= 2 && segments[0] == "target"
}

fn is_trusted_stateful_executable(executable: &str) -> bool {
    let path = Path::new(executable);
    if !path.is_absolute() {
        return false;
    }
    let Ok(candidate) = path.canonicalize() else {
        return false;
    };
    let Ok(current) = std::env::current_exe() else {
        return false;
    };
    let current = current.canonicalize().unwrap_or(current);
    candidate == current
}

fn authorize_apply_patch(
    input: &PreToolUseInput,
    runtime: Option<&ServerRuntime>,
    repo_root: Option<&Path>,
    cwd: Option<&Path>,
    identity: Option<&RepoIdentity>,
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
    let outcome = authorize_targets(input, runtime, repo_root, targets, identity)?;
    Ok(with_file_tool_live_context(
        outcome, input, runtime, identity,
    ))
}

fn with_file_tool_live_context(
    outcome: HookOutcome,
    input: &PreToolUseInput,
    runtime: Option<&ServerRuntime>,
    identity: Option<&RepoIdentity>,
) -> HookOutcome {
    let Some(runtime) = runtime else {
        return outcome;
    };
    let Ok(context_response) =
        render_context_response(runtime, input.stateful_session_id(), identity)
    else {
        return outcome;
    };
    if context_response.prompt_text.trim().is_empty() {
        return outcome;
    }

    match outcome {
        HookOutcome::Allow if context_response_has_actionable_items(&context_response) => {
            HookOutcome::AllowWithContext {
                message: context_response.prompt_text,
            }
        }
        HookOutcome::Allow => HookOutcome::Allow,
        HookOutcome::AllowWithContext { .. } => outcome,
        HookOutcome::Deny { reason } => HookOutcome::Deny {
            reason: format!("{reason}\n\n{}", context_response.prompt_text),
        },
    }
}

fn context_response_has_actionable_items(response: &ContextRenderResponse) -> bool {
    response.items.iter().any(|item| {
        matches!(
            item.severity,
            ContextRenderSeverity::Block | ContextRenderSeverity::Warn
        )
    })
}

fn authorize_file_change_tool(
    input: &PreToolUseInput,
    runtime: Option<&ServerRuntime>,
    repo_root: Option<&Path>,
    cwd: Option<&Path>,
    identity: Option<&RepoIdentity>,
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
    authorize_targets(input, runtime, repo_root, targets, identity)
}

fn authorize_file_write_tool(
    input: &PreToolUseInput,
    runtime: Option<&ServerRuntime>,
    repo_root: Option<&Path>,
    cwd: Option<&Path>,
    identity: Option<&RepoIdentity>,
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

    let outcome = authorize_targets(
        input,
        runtime,
        repo_root,
        vec![PatchTarget::write(&target)],
        identity,
    )?;
    Ok(with_file_tool_live_context(
        outcome, input, runtime, identity,
    ))
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
        let new_path = if let Some(new_path) = &target.new_path {
            let Some(new_path) = normalize_file_tool_target(new_path, repo_root, cwd)? else {
                return Ok(None);
            };
            Some(new_path)
        } else {
            None
        };
        normalized.push(target.with_paths(path, new_path));
    }

    Ok(Some(normalized))
}

fn hook_authorize_purpose(
    input: &PreToolUseInput,
    runtime: &ServerRuntime,
    target: &PatchTarget,
    workspace_id: &str,
) -> Option<String> {
    let response = get_json(runtime, "/v1/current").ok()?;
    if !(200..300).contains(&response.status_code) {
        return None;
    }
    let body: serde_json::Value = serde_json::from_str(&response.body).ok()?;
    let items = body.get("items")?.as_array()?;
    let mut fallback = None;
    for item in items {
        let matches_intent = item.get("kind").and_then(serde_json::Value::as_str) == Some("intent")
            && item.get("freshness").and_then(serde_json::Value::as_str) == Some("live")
            && item.get("session_id").and_then(serde_json::Value::as_str)
                == Some(input.stateful_session_id())
            && item.get("workspace_id").and_then(serde_json::Value::as_str) == Some(workspace_id);
        if !matches_intent {
            continue;
        }
        let Some(purpose) = item
            .get("purpose")
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|purpose| !purpose.is_empty())
            .map(str::to_string)
        else {
            continue;
        };
        if item.get("resource").and_then(serde_json::Value::as_str) == Some(target.path.as_str()) {
            return Some(purpose);
        }
        if fallback.is_none() {
            fallback = Some(purpose);
        }
    }

    fallback
}

fn authorize_targets(
    input: &PreToolUseInput,
    runtime: Option<&ServerRuntime>,
    repo_root: Option<&Path>,
    targets: Vec<PatchTarget>,
    identity: Option<&RepoIdentity>,
) -> anyhow::Result<HookOutcome> {
    if let Some(repo_root) = repo_root
        && let Err(error) = shadow_guard::check_paths_for_dependency_shadowing(
            repo_root,
            shadow_write_paths(&targets),
        )
    {
        return Ok(HookOutcome::Deny {
            reason: error.to_string(),
        });
    }

    let Some(runtime) = runtime else {
        return Ok(HookOutcome::Deny {
            reason: format!(
                "{} writes require a reachable stateful server, exact file intent, and a same-session file lease",
                input.tool_name
            ),
        });
    };

    if targets.is_empty() {
        return Ok(HookOutcome::Allow);
    }

    let workspace_id = effective_workspace_id(runtime, identity);
    let session_id = input.stateful_session_id().to_string();
    let mut allowed_paths = BTreeSet::new();
    for target in targets {
        let purpose = hook_authorize_purpose(input, runtime, &target, &workspace_id);
        let mut payload = json!({
            "action": target.action,
            "path": target.path,
        });
        if let Some(observation) = base_observation_for_target(repo_root, &target.path) {
            payload["base_observations"] = json!([observation]);
        }
        if let Some(purpose) = purpose {
            payload["queue_on_conflict"] = json!(true);
            payload["purpose"] = json!(purpose);
        }
        if let Some(new_path) = &target.new_path {
            payload["old_path"] = json!(target.path);
            payload["new_path"] = json!(new_path);
        }
        let body = protocol_envelope(ProtocolEnvelopeArgs {
            runtime,
            request_id: uuid::Uuid::new_v4().to_string(),
            session_id: session_id.clone(),
            workspace_id: workspace_id.clone(),
            identity: identity.cloned(),
            source_kind: "hook",
            event: "pre_tool_use",
            source_ref: "hook:pre_tool_use",
            source_tool_name: Some(input.tool_name.as_str()),
            payload,
        });
        let response = match post_json(runtime, "/v1/authorize", &body) {
            Ok(response) => response,
            Err(error) => {
                release_pre_tool_authorized_leases(
                    runtime,
                    &session_id,
                    &workspace_id,
                    &allowed_paths,
                );
                return Ok(HookOutcome::Deny {
                    reason: authorization_unavailable_reason(&error),
                });
            }
        };

        if !(200..300).contains(&response.status_code) {
            release_pre_tool_authorized_leases(runtime, &session_id, &workspace_id, &allowed_paths);
            return Ok(HookOutcome::Deny {
                reason: format!(
                    "stateful authorization failed with HTTP {}: {}",
                    response.status_code, response.body
                ),
            });
        }

        let decision: AuthorizeDecision = match serde_json::from_str(&response.body) {
            Ok(decision) => decision,
            Err(error) => {
                release_pre_tool_authorized_leases(
                    runtime,
                    &session_id,
                    &workspace_id,
                    &allowed_paths,
                );
                return Ok(HookOutcome::Deny {
                    reason: authorization_unavailable_reason(&error),
                });
            }
        };
        if decision.decision != "allow" {
            release_pre_tool_authorized_leases(runtime, &session_id, &workspace_id, &allowed_paths);
            return Ok(HookOutcome::Deny {
                reason: authorization_denial_reason(decision),
            });
        }
        allowed_paths.insert(target.path.clone());
        if let Some(new_path) = &target.new_path {
            allowed_paths.insert(new_path.clone());
        }
    }

    Ok(HookOutcome::Allow)
}

fn release_pre_tool_authorized_leases(
    runtime: &ServerRuntime,
    session_id: &str,
    workspace_id: &str,
    paths: &BTreeSet<String>,
) {
    for path in paths {
        let body = json!({
            "session_id": session_id,
            "workspace_id": workspace_id,
            "path": path,
        });
        let Ok(response) = post_json(runtime, "/v1/lease/release", &body) else {
            continue;
        };
        if !(200..300).contains(&response.status_code) {
            continue;
        }
    }
}

fn shadow_write_paths(targets: &[PatchTarget]) -> impl Iterator<Item = &str> {
    targets.iter().filter_map(|target| match target.action {
        "write_file" => Some(target.path.as_str()),
        "move_file" => target.new_path.as_deref(),
        _ => None,
    })
}

fn authorization_unavailable_reason(error: &dyn std::fmt::Display) -> String {
    format!(
        "server_unavailable: stateful authorization is unavailable while contacting /v1/authorize: {error}. Writes fail closed. Run `stateful server status`, restart or rejoin the stateful server, then retry after declaring exact intent and acquiring a same-session file lease."
    )
}

fn authorization_denial_reason(decision: AuthorizeDecision) -> String {
    let mut reason = decision.required_next_action.unwrap_or(decision.message);
    let Some(wait) = decision.wait else {
        return reason;
    };

    if !reason.contains(&wait.wait_id) {
        let mut wait_details = vec![format!("wait_id {}", wait.wait_id)];
        if let Some(queue_position) = wait.queue_position {
            wait_details.push(format!("queue position {queue_position}"));
        }
        if let Some(blocking_session_id) = wait.blocking_session_id {
            wait_details.push(format!("blocked by session {blocking_session_id}"));
        }
        reason.push_str(&format!(" Track {}.", wait_details.join(", ")));
    }
    let has_resume_guidance = [
        "state_notifications_poll",
        "state_resume_next",
        "reread",
        "state_intent_claim",
    ]
    .iter()
    .all(|term| reason.contains(term));
    if !has_resume_guidance {
        let target = wait.path.as_deref().unwrap_or("the target");
        reason.push_str(&format!(
            " Resume by polling state_notifications_poll or state_resume_next for wait_id {}; when reserved, reread {}, then call state_intent_claim with wait_id {} before retrying the write.",
            wait.wait_id, target, wait.wait_id
        ));
    }
    reason
}

fn base_observation_for_target(
    repo_root: Option<&Path>,
    relative_path: &str,
) -> Option<serde_json::Value> {
    let repo_root = repo_root?;
    let target_path = repo_root.join(relative_path);
    match fs::read(&target_path) {
        Ok(bytes) => Some(json!({
            "path": relative_path,
            "exists": true,
            "content_hash": hook_content_hash(&bytes),
        })),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Some(json!({
            "path": relative_path,
            "exists": false,
            "content_hash": null,
        })),
        Err(_) => None,
    }
}

fn hook_content_hash(bytes: &[u8]) -> String {
    let mut hash = 0xcbf29ce484222325u64;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("fnv1a64:{hash:016x}")
}

fn normalize_file_tool_target(
    path: &str,
    repo_root: Option<&Path>,
    cwd: Option<&Path>,
) -> anyhow::Result<Option<String>> {
    let path = path.trim();
    let candidate = Path::new(path);
    let Some(repo_root) = repo_root else {
        return Ok(Some(normalize_relative_path(path)));
    };
    let repo_root = repo_root
        .canonicalize()
        .unwrap_or_else(|_| repo_root.to_path_buf());

    if candidate.is_absolute() {
        let raw_candidate = normalize_path(candidate.to_path_buf());
        if let Ok(relative) = raw_candidate.strip_prefix(&repo_root) {
            reject_symlinked_target_components(&repo_root, relative)?;
        }
    }

    if candidate.is_absolute() {
        let canonical = candidate
            .canonicalize()
            .unwrap_or_else(|_| candidate.to_path_buf());
        let candidate = normalize_path(canonical);
        let Ok(relative) = candidate.strip_prefix(&repo_root) else {
            return Ok(None);
        };
        reject_symlinked_target_components(&repo_root, relative)?;
        return Ok(Some(normalize_relative_path(relative.to_string_lossy())));
    }

    let base = cwd.unwrap_or(repo_root.as_path());
    let base = base.canonicalize().unwrap_or_else(|_| base.to_path_buf());
    let candidate = normalize_path(base.join(candidate));
    let Ok(relative) = candidate.strip_prefix(&repo_root) else {
        return Ok(None);
    };

    reject_symlinked_target_components(&repo_root, relative)?;
    Ok(Some(normalize_relative_path(relative.to_string_lossy())))
}

fn reject_symlinked_target_components(
    repo_root: &Path,
    relative_path: &Path,
) -> anyhow::Result<()> {
    let mut current = repo_root.to_path_buf();
    for component in relative_path.components() {
        let Component::Normal(part) = component else {
            continue;
        };
        current.push(part);
        match fs::symlink_metadata(&current) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                anyhow::bail!(
                    "stateful native file target `{}` includes symlinked component `{}`",
                    relative_path.display(),
                    current.display()
                );
            }
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => break,
            Err(error) => return Err(error.into()),
        }
    }
    Ok(())
}

fn extract_apply_patch_write_targets(patch: &str) -> Vec<PatchTarget> {
    let mut targets = Vec::new();
    let mut pending_update: Option<String> = None;

    for line in patch.lines() {
        if let Some(path) = line.strip_prefix("*** Update File: ") {
            if let Some(path) = pending_update.replace(path.trim().to_string())
                && !path.is_empty()
            {
                targets.push(PatchTarget::write(&path));
            }
        } else if let Some(path) = line.strip_prefix("*** Move to: ") {
            if let Some(old_path) = pending_update.take() {
                let new_path = path.trim();
                if !old_path.is_empty() && !new_path.is_empty() {
                    targets.push(PatchTarget::move_file(&old_path, new_path));
                }
            }
        } else if let Some(path) = line.strip_prefix("*** Add File: ") {
            if let Some(path) = pending_update.take()
                && !path.is_empty()
            {
                targets.push(PatchTarget::write(&path));
            }
            targets.push(PatchTarget::write(path));
        } else if let Some(path) = line.strip_prefix("*** Delete File: ") {
            if let Some(path) = pending_update.take()
                && !path.is_empty()
            {
                targets.push(PatchTarget::write(&path));
            }
            targets.push(PatchTarget::delete(path));
        }
    }

    if let Some(path) = pending_update
        && !path.is_empty()
    {
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

#[derive(Debug, Clone, PartialEq, Eq)]
struct PatchTarget {
    action: &'static str,
    path: String,
    new_path: Option<String>,
}

impl PatchTarget {
    fn write(path: &str) -> Self {
        Self {
            action: "write_file",
            path: path.trim().to_string(),
            new_path: None,
        }
    }

    fn write_directory(path: &str) -> Self {
        Self {
            action: "write_directory",
            path: path.trim().to_string(),
            new_path: None,
        }
    }

    fn delete(path: &str) -> Self {
        Self {
            action: "delete_file",
            path: path.trim().to_string(),
            new_path: None,
        }
    }

    fn move_file(old_path: &str, new_path: &str) -> Self {
        Self {
            action: "move_file",
            path: old_path.trim().to_string(),
            new_path: Some(new_path.trim().to_string()),
        }
    }

    fn with_paths(mut self, path: String, new_path: Option<String>) -> Self {
        self.path = path;
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
pub struct OmpPreToolUseInput {
    session_id: String,
    #[serde(default)]
    parent_session_id: Option<String>,
    #[serde(default)]
    omp_agent_id: Option<String>,
    #[serde(default)]
    workspace_id: Option<String>,
    #[serde(default)]
    cwd: Option<PathBuf>,
    #[serde(default)]
    yolo: bool,
    #[serde(default)]
    commit_id: Option<String>,
    tool_name: String,
    #[serde(default)]
    tool_input: serde_json::Value,
}

impl OmpPreToolUseInput {
    fn audit_metadata(&self) -> serde_json::Value {
        json!({
            "runtime": "omp",
            "parent_session_id": self.parent_session_id,
            "omp_agent_id": self.omp_agent_id,
            "yolo": self.yolo,
            "commit_id": self.commit_id,
            "cwd": self.cwd,
        })
    }
}

#[derive(Debug, Deserialize)]
pub struct OmpSessionEventInput {
    session_id: String,
    #[serde(default)]
    parent_session_id: Option<String>,
    #[serde(default)]
    omp_agent_id: Option<String>,
    #[serde(default)]
    workspace_id: Option<String>,
    #[serde(default)]
    cwd: Option<PathBuf>,
    #[serde(default)]
    commit_id: Option<String>,
    #[serde(default)]
    tool_name: Option<String>,
    #[serde(default)]
    tool_input: serde_json::Value,
}

impl OmpSessionEventInput {
    fn audit_metadata(&self) -> serde_json::Value {
        json!({
            "runtime": "omp",
            "parent_session_id": self.parent_session_id,
            "omp_agent_id": self.omp_agent_id,
            "commit_id": self.commit_id,
            "cwd": self.cwd,
            "raw_runtime_payload": self.tool_input,
        })
    }

    fn command(&self) -> Option<&str> {
        self.tool_input.get("command")?.as_str()
    }
}

#[derive(Debug, Deserialize)]
struct PreToolUseInput {
    #[serde(flatten)]
    runtime: RuntimeHookInput,
    tool_name: String,
    #[serde(default)]
    tool_input: serde_json::Value,
}

#[derive(Debug, Deserialize)]
struct RuntimeHookInput {
    session_id: String,
    #[serde(default)]
    thread_id: Option<String>,
}

impl RuntimeHookInput {
    fn stateful_session_id(&self) -> &str {
        self.thread_id
            .as_deref()
            .map(str::trim)
            .filter(|thread_id| !thread_id.is_empty())
            .unwrap_or(&self.session_id)
    }
}

#[derive(Debug, Deserialize)]
struct HookCwdInput {
    #[serde(default)]
    cwd: Option<PathBuf>,
}

impl PreToolUseInput {
    fn stateful_session_id(&self) -> &str {
        self.runtime.stateful_session_id()
    }

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
    #[serde(default)]
    wait: Option<AuthorizeWait>,
}

#[derive(Debug, Deserialize)]
struct AuthorizeWait {
    wait_id: String,
    #[serde(default)]
    path: Option<String>,
    #[serde(default)]
    queue_position: Option<u64>,
    #[serde(default)]
    blocking_session_id: Option<String>,
}

#[derive(Debug, Deserialize)]
struct SessionStartInput {
    #[serde(flatten)]
    runtime: RuntimeHookInput,
}

#[derive(Debug, Deserialize)]
struct SessionEventInput {
    #[serde(flatten)]
    runtime: RuntimeHookInput,
    #[serde(default)]
    cwd: Option<PathBuf>,
    #[serde(default)]
    tool_name: Option<String>,
    #[serde(default)]
    tool_input: serde_json::Value,
}

#[derive(Debug, Deserialize)]
struct UserPromptSubmitInput {
    #[serde(flatten)]
    runtime: RuntimeHookInput,
}

#[derive(Debug, Deserialize)]
struct ContextRenderResponse {
    #[serde(default)]
    items: Vec<ContextRenderItem>,
    prompt_text: String,
}

#[derive(Debug, Deserialize)]
struct ContextRenderItem {
    severity: ContextRenderSeverity,
}

#[derive(Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum ContextRenderSeverity {
    Block,
    Warn,
    Info,
}

impl SessionStartInput {
    fn stateful_session_id(&self) -> &str {
        self.runtime.stateful_session_id()
    }
}

impl SessionEventInput {
    fn stateful_session_id(&self) -> &str {
        self.runtime.stateful_session_id()
    }

    fn patch_text(&self) -> Option<&str> {
        string_payload_field(
            &self.tool_input,
            &["command", "patch", "input", "cmd", "diff"],
        )
    }
}

impl UserPromptSubmitInput {
    fn stateful_session_id(&self) -> &str {
        self.runtime.stateful_session_id()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runtime_hook_input_prefers_thread_id_when_present() {
        let input = RuntimeHookInput {
            session_id: "codex-session-1".to_string(),
            thread_id: Some("codex-thread-1".to_string()),
        };

        assert_eq!(input.stateful_session_id(), "codex-thread-1");
    }

    #[test]
    fn runtime_hook_input_falls_back_to_session_id_without_thread_id() {
        let input = RuntimeHookInput {
            session_id: "codex-session-1".to_string(),
            thread_id: None,
        };

        assert_eq!(input.stateful_session_id(), "codex-session-1");
    }

    #[test]
    fn hook_session_extraction_does_not_keep_fake_runtime_adapter() {
        let source = include_str!("hook.rs");

        assert!(!source.contains(concat!("Claude", "CodeRuntimeAdapter")));
        assert!(!source.contains(concat!("trait ", "RuntimeAdapter")));
    }

    #[test]
    fn stateful_command_policy_reminder_mentions_process_find_local_git_github_pr_and_binary_path()
    {
        let reminder = stateful_command_policy_reminder();
        let current_exe = std::env::current_exe()
            .expect("current executable should resolve")
            .canonicalize()
            .expect("current executable should canonicalize")
            .to_string_lossy()
            .to_string();

        assert!(
            reminder.contains("sandbox process find"),
            "reminder should mention structured process lookup: {reminder}"
        );
        assert!(
            reminder.contains("sandbox run --fs git --network disabled"),
            "reminder should default local git to network disabled: {reminder}"
        );
        assert!(
            reminder.contains("sandbox run --fs github-pr --network enabled"),
            "reminder should mention GitHub PR profile: {reminder}"
        );
        assert!(
            reminder.contains(&current_exe),
            "reminder should show the trusted executable path: {reminder}"
        );

        let guidance = bash_policy_guidance();
        assert!(
            guidance.contains("sandbox run --fs git --network disabled"),
            "denial guidance should default local git to network disabled: {guidance}"
        );
        assert!(
            guidance.contains("sandbox run --fs github-pr --network enabled"),
            "denial guidance should mention GitHub PR profile: {guidance}"
        );
        assert!(
            guidance.contains("network enabled"),
            "denial guidance should mention networked git exception: {guidance}"
        );
        assert!(
            guidance.contains(&current_exe),
            "denial guidance should show the trusted executable path: {guidance}"
        );
    }

    #[test]
    fn hook_file_target_uses_core_relative_path_normalization() {
        let temp =
            std::env::temp_dir().join(format!("stateful-hook-normalize-{}", std::process::id()));
        let repo = temp.join("repo");
        let cwd = repo.join("src");
        std::fs::create_dir_all(&cwd).expect("repo dirs should create");

        let normalized = normalize_file_tool_target(r".\auth\..\auth.ts", Some(&repo), Some(&cwd))
            .expect("target should normalize")
            .expect("target should stay in repo");

        assert_eq!(
            normalized,
            stateful_core::normalize_relative_path("src/auth.ts")
        );

        let _ = std::fs::remove_dir_all(&temp);
    }
    #[test]
    fn omp_post_tool_targets_include_write_path() {
        let temp =
            std::env::temp_dir().join(format!("stateful-omp-post-write-{}", std::process::id()));
        let repo = temp.join("repo");
        let cwd = repo.join("src");
        std::fs::create_dir_all(&cwd).expect("repo dirs should create");
        let input = OmpSessionEventInput {
            session_id: "omp-session-1".to_string(),
            parent_session_id: None,
            omp_agent_id: None,
            workspace_id: None,
            cwd: Some(cwd),
            commit_id: None,
            tool_name: Some("write".to_string()),
            tool_input: serde_json::json!({ "path": "../README.md" }),
        };

        let targets =
            omp_post_tool_refresh_targets(&input, &repo).expect("targets should normalize");

        assert_eq!(targets, vec![PatchTarget::write("README.md")]);
        let _ = std::fs::remove_dir_all(&temp);
    }

    #[test]
    fn omp_post_tool_targets_include_edit_hashlines() {
        let temp =
            std::env::temp_dir().join(format!("stateful-omp-post-edit-{}", std::process::id()));
        let repo = temp.join("repo");
        std::fs::create_dir_all(&repo).expect("repo dir should create");
        let input = OmpSessionEventInput {
            session_id: "omp-session-1".to_string(),
            parent_session_id: None,
            omp_agent_id: None,
            workspace_id: None,
            cwd: Some(repo.clone()),
            commit_id: None,
            tool_name: Some("edit".to_string()),
            tool_input: serde_json::json!({
                "input": "[src/lib.rs#ABCD]\nSWAP 1.=1:\n+pub fn value() -> u8 { 1 }\n"
            }),
        };

        let targets =
            omp_post_tool_refresh_targets(&input, &repo).expect("targets should normalize");

        assert_eq!(targets, vec![PatchTarget::write("src/lib.rs")]);
        let _ = std::fs::remove_dir_all(&temp);
    }
}
