use std::{
    ffi::OsStr,
    fs,
    io::{self, Read},
    path::{Component, Path, PathBuf},
};

use serde::{Deserialize, Serialize};
use serde_json::json;
use stateful_core::normalize_relative_path;
mod delivery;
mod input;
mod observation;
pub(crate) mod write_lifecycle;

const CODEX_WRITE_LIFECYCLE: write_lifecycle::LifecycleSource = write_lifecycle::LifecycleSource {
    source_kind: stateful_core::SourceKind::Hook,
    start_event: "codex_write_start",
    start_ref: "hook:codex_pre_tool_use",
    complete_event: "codex_write_complete",
    complete_ref: "hook:codex_post_tool_use",
};
const OMP_WRITE_LIFECYCLE: write_lifecycle::LifecycleSource = write_lifecycle::LifecycleSource {
    source_kind: stateful_core::SourceKind::Hook,
    start_event: "omp_write_start",
    start_ref: "hook:omp_pre_tool_use",
    complete_event: "omp_write_complete",
    complete_ref: "hook:omp_post_tool_use",
};

use crate::outbox::{
    acknowledge_exact_envelope, exact_envelope_json, queue_exact_envelope,
    queue_session_heartbeat_outbox, sync_outbox_with_runtime,
};
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
    GlobalPaths, HookCommand, HookRuntime, RepoGate, RepoIdentity, ServerRuntime,
    discover_runtime_with_global, effective_workspace_id_for_repo, ensure_server,
    record_unclassified_tool_for_repo, repo_gate, repo_identity_for_enabled_repo,
    runtime_env_override_is_configured, tool_allowed_for_enabled_repo,
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
    Warn { message: String },
    Block { reason: String },
    Prompt { title: String, message: String },
}

impl OmpHookOutcome {
    fn to_stdout_json(&self) -> serde_json::Value {
        match self {
            Self::Allow => json!({ "decision": "allow" }),
            Self::Warn { message } => json!({
                "decision": "warn",
                "message": message,
            }),
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
    pub agent_id: String,
    pub workspace_id: String,
    pub notifications_stream: OmpNotificationsStream,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct OmpNotificationsStream {
    pub base_url: String,
    pub authorization: String,
    pub agent_id: String,
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
    if !matches!(command, HookCommand::PostToolUse)
        && let Err(error) = write_lifecycle::replay_pending(&paths, &runtime, &repo_root)
    {
        eprintln!("stateful OMP write lifecycle replay warning: {error}");
    }
    if let Err(error) = sync_outbox_with_runtime(&paths, &runtime) {
        eprintln!("stateful OMP lifecycle outbox replay warning: {error}");
    }

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
            match handle_omp_session_start_with_identity(&input, &runtime, identity.as_ref()) {
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
            input.validate()?;
            post_omp_activity_finalize(&runtime, &input, identity.as_ref())?;
        }
        HookCommand::UserPromptSubmit => {
            anyhow::bail!("OMP hook user-prompt-submit is not supported");
        }
    }
    if matches!(command, HookCommand::PostToolUse)
        && let Err(error) = write_lifecycle::replay_pending(&paths, &runtime, &repo_root)
    {
        eprintln!("stateful OMP write lifecycle replay warning: {error}");
    }
    Ok(())
}

fn run_codex_hook(command: HookCommand) -> anyhow::Result<()> {
    match command {
        HookCommand::SessionStart => {
            let mut input = String::new();
            io::stdin().read_to_string(&mut input)?;
            match handle_session_start_context_in_repo(&input, hook_start_dir(&input)?) {
                Ok(prompt_text) if !prompt_text.is_empty() => println!("{prompt_text}"),
                Ok(_) => {}
                Err(error) => eprintln!("stateful session-start warning: {error}"),
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
    if let Some(runtime) = runtime.as_ref() {
        let _ = sync_outbox_with_runtime(&paths, runtime);
    }
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
    input.validate()?;
    if runtime_tool_name_leaf(&input.tool_name).eq_ignore_ascii_case("read") {
        record_omp_read_start(&input, runtime, repo_root, cwd, identity)?;
    }
    let command = runtime_tool_name_leaf(&input.tool_name)
        .eq_ignore_ascii_case("bash")
        .then(|| testing_command(&input.tool_input))
        .flatten();
    let global_paths = GlobalPaths::from_env().ok();
    let outcome = match omp_pre_tool_action(&input, repo_root, cwd, global_paths.as_ref())? {
        OmpPreToolAction::Allow => OmpHookOutcome::Allow,
        OmpPreToolAction::Block { reason } => OmpHookOutcome::Block { reason },
        OmpPreToolAction::Targets(targets) => {
            authorize_omp_targets(&input, runtime, repo_root, cwd, identity, targets)?
        }
    };
    if matches!(outcome, OmpHookOutcome::Allow | OmpHookOutcome::Warn { .. })
        && let Some(command) = command
    {
        post_omp_testing_start(&input, runtime, identity, command)?;
    }
    Ok(outcome)
}

fn record_omp_read_start(
    input: &OmpPreToolUseInput,
    runtime: Option<&ServerRuntime>,
    repo_root: Option<&Path>,
    cwd: Option<&Path>,
    identity: Option<&RepoIdentity>,
) -> anyhow::Result<()> {
    let (Some(runtime), Some(repo_root), Some(operation_id)) =
        (runtime, repo_root, input.metadata.operation_id())
    else {
        return Ok(());
    };
    let Some(path) = read_target(&input.tool_input, repo_root, cwd)? else {
        return Ok(());
    };
    let workspace_id = input
        .workspace_id
        .clone()
        .unwrap_or_else(|| effective_workspace_id(runtime, identity));
    let mut request = crate::v2_request_envelope(
        uuid::Uuid::new_v4(),
        input.agent_id.clone(),
        workspace_id.clone(),
        identity.cloned(),
        stateful_core::ActorType::Agent,
        stateful_core::SourceKind::Hook,
        "omp_read_start",
        "hook:omp_pre_tool_use",
        Some(input.tool_name.clone()),
        json!({
            "operation_id": operation_id,
            "path": path,
            "before": observation::fingerprint(repo_root, &path)?,
        }),
    )?;
    set_omp_lineage(
        &mut request,
        &input.agent_id,
        input.omp_agent_id.as_deref(),
        input.parent_agent_id.as_deref(),
    );
    let mut failed_completion = crate::v2_request_envelope(
        uuid::Uuid::new_v4(),
        input.agent_id.clone(),
        workspace_id,
        identity.cloned(),
        stateful_core::ActorType::Agent,
        stateful_core::SourceKind::Hook,
        "omp_read_complete",
        "hook:omp_pre_tool_use",
        Some(input.tool_name.clone()),
        json!({
            "operation_id": operation_id,
            "path": path,
            "classification": stateful_core::ReadClassification::Failed,
            "semantic_marker": "pre_tool_start_delivery_failed",
        }),
    )?;
    set_omp_lineage(
        &mut failed_completion,
        &input.agent_id,
        input.omp_agent_id.as_deref(),
        input.parent_agent_id.as_deref(),
    );
    post_durable_read_start(runtime, &request, &failed_completion)?;
    Ok(())
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
        tool_name if tool_name.eq_ignore_ascii_case("bash") => {
            if let Some(action) = input.command().and_then(omp_sandbox_run_action) {
                return Ok(action);
            }
            if let Some(action) = input.command().and_then(omp_process_find_action) {
                return Ok(action);
            }
            Ok(OmpPreToolAction::Block {
                reason: format!(
                    "OMP raw {} is denied; only trusted stateful sandbox run or stateful sandbox process find commands are allowed through built-in Bash",
                    input.tool_name
                ),
            })
        }
        tool_name if is_omp_eval_tool(tool_name) => Ok(OmpPreToolAction::Block {
            reason: format!(
                "OMP eval tool {} is denied; use built-in Bash with trusted stateful sandbox run or stateful sandbox process find commands",
                input.tool_name
            ),
        }),
        tool_name if is_omp_safe_without_repo_write_authorization(tool_name) => {
            Ok(OmpPreToolAction::Allow)
        }
        _ if is_stateful_control_plane_tool(&input.tool_name) => Ok(OmpPreToolAction::Allow),
        _ if is_user_allowed_tool(global_paths, repo_root, tool_name) => {
            Ok(OmpPreToolAction::Allow)
        }
        _ => {
            record_unclassified_tool(global_paths, repo_root, tool_name);
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
        "glob",
        "grep",
        "goal",
        "hub",
        "irc",
        "lazy_edit_resume",
        "lazy_write_resume",
        "lazy_bash_resume",
        "job",
        "read",
        "report_tool_issue",
        "search",
        "search_tool_bm25",
        "task",
        "yield",
        "parallel_tool_calls",
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
            reason: "stateful sandbox run requires a trusted stateful binary".to_string(),
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
        return Some(OmpPreToolAction::Allow);
    }
    Some(OmpPreToolAction::Allow)
}

fn omp_process_find_action(command: &str) -> Option<OmpPreToolAction> {
    let invocation = parse_sandbox_process_find_bash_invocation(command).ok()?;
    if !is_trusted_stateful_executable(&invocation.executable) {
        return Some(OmpPreToolAction::Block {
            reason: "stateful sandbox process find requires a trusted stateful binary".to_string(),
        });
    }
    if let Err(error) = validate_process_find_request(&invocation.request) {
        return Some(OmpPreToolAction::Block {
            reason: error.to_string(),
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
    if let Some(outcome) = omp_external_native_write_prompt(input, &targets, repo_root, cwd)? {
        return Ok(outcome);
    }

    let Some(repo_root) = repo_root else {
        return Ok(OmpHookOutcome::Block {
            reason: format!("{} writes require an enabled repository", input.tool_name),
        });
    };
    let Some(targets) = normalize_targets(targets, Some(repo_root), cwd)? else {
        return Ok(OmpHookOutcome::Block {
            reason: format!("{} target is outside the enabled repo", input.tool_name),
        });
    };
    if targets.is_empty() {
        return Ok(OmpHookOutcome::Allow);
    }
    if targets.iter().any(|target| target.action != targets[0].action) {
        return Ok(OmpHookOutcome::Block {
            reason: format!(
                "{} mixes write actions; split the operation into one action per patch",
                input.tool_name
            ),
        });
    }
    let Some(runtime) = runtime else {
        return Ok(OmpHookOutcome::Block {
            reason: format!(
                "{} writes require a reachable stateful.v2 server",
                input.tool_name
            ),
        });
    };
    let Some(operation_id) = input.metadata.operation_id() else {
        return Ok(OmpHookOutcome::Block {
            reason: format!(
                "{} writes require an operation ID for stateful.v2 authorization",
                input.tool_name
            ),
        });
    };

    let mut fingerprints = Vec::with_capacity(targets.len() * 2);
    for target in &targets {
        fingerprints.push((
            target.path.clone(),
            stateful_core::fingerprint_path(&repo_root.join(&target.path))?,
        ));
        if let Some(new_path) = &target.new_path {
            fingerprints.push((
                new_path.clone(),
                stateful_core::fingerprint_path(&repo_root.join(new_path))?,
            ));
        }
    }
    let paths = GlobalPaths::from_env()?;
    let workspace_id = input
        .workspace_id
        .clone()
        .unwrap_or_else(|| effective_workspace_id(runtime, identity));
    let purpose = format!("Authorize OMP {}.", input.tool_name);
    let mut reservation_id = input.reservation_id().map(str::to_string);
    let mut claim_ids = Vec::new();
    let mut retried = false;

    loop {
        match write_lifecycle::authorize_at_root(
            &paths,
            runtime,
            &input.agent_id,
            &workspace_id,
            repo_root,
            identity,
            input.omp_agent_id.as_deref(),
            input.parent_agent_id.as_deref(),
            operation_id,
            targets[0].action,
            fingerprints.clone(),
            reservation_id.as_deref(),
            claim_ids.clone(),
            &OMP_WRITE_LIFECYCLE,
        ) {
            Ok(authorization) => {
                return Ok(match authorization.decision {
                    Some(decision) if decision.decision == stateful_core::DecisionKind::Warn => {
                        OmpHookOutcome::Warn {
                            message: format!("stateful warning: {}", decision.message),
                        }
                    }
                    Some(decision) if decision.decision == stateful_core::DecisionKind::Deny => {
                        write_lifecycle::complete(
                            &paths,
                            runtime,
                            &input.agent_id,
                            &workspace_id,
                            identity,
                            input.omp_agent_id.as_deref(),
                            input.parent_agent_id.as_deref(),
                            repo_root,
                            operation_id,
                            true,
                            &OMP_WRITE_LIFECYCLE,
                        )?;
                        OmpHookOutcome::Block {
                            reason: format!("{}: {}", decision.reason_code, decision.message),
                        }
                    }
                    _ => OmpHookOutcome::Allow,
                });
            }
            Err(error)
                if !retried
                    && reservation_id.is_none()
                    && should_auto_declare_omp_tool_reservation(
                        input,
                        &error.to_string(),
                        &targets,
                    ) =>
            {
                match declare_and_claim_omp_pre_tool_reservation(
                    input,
                    runtime,
                    identity,
                    &workspace_id,
                    &targets,
                    &fingerprints,
                    &purpose,
                ) {
                    Ok((declared_id, acquired_claim_ids)) => {
                        reservation_id = Some(declared_id);
                        claim_ids = acquired_claim_ids;
                        retried = true;
                    }
                    Err(outcome) => return Ok(outcome),
                }
            }
            Err(error) => {
                release_omp_claims(runtime, input, &workspace_id, identity, &claim_ids);
                return Ok(OmpHookOutcome::Block {
                    reason: format!("stateful.v2 write authorization failed: {error}"),
                });
            }
        }
    }
}

fn omp_external_native_write_prompt(
    input: &OmpPreToolUseInput,
    targets: &[PatchTarget],
    repo_root: Option<&Path>,
    cwd: Option<&Path>,
) -> anyhow::Result<Option<OmpHookOutcome>> {
    let tool_name = runtime_tool_name_leaf(&input.tool_name);
    if !(tool_name.eq_ignore_ascii_case("edit") || tool_name.eq_ignore_ascii_case("write")) {
        return Ok(None);
    }
    let Some(repo_root) = repo_root else {
        return Ok(None);
    };
    let mut external_targets = Vec::new();
    let mut internal_targets = 0;
    for target in targets {
        match target.action {
            "write_file" => {
                if normalize_file_tool_target(&target.path, Some(repo_root), cwd)?.is_some() {
                    internal_targets += 1;
                } else {
                    external_targets.push(omp_external_target_display(&target.path, cwd));
                }
            }
            "move_file" => {
                for path in [&target.path, target.new_path.as_deref().unwrap_or("")] {
                    if path.is_empty() {
                        continue;
                    }
                    if normalize_file_tool_target(path, Some(repo_root), cwd)?.is_some() {
                        internal_targets += 1;
                    } else {
                        external_targets.push(omp_external_target_display(path, cwd));
                    }
                }
            }
            _ => return Ok(None),
        }
    }
    if external_targets.is_empty() {
        return Ok(None);
    }
    if internal_targets > 0 {
        return Ok(Some(OmpHookOutcome::Block {
            reason: format!(
                "{} mixes repo-internal and repo-external targets; split the operation before retrying",
                input.tool_name
            ),
        }));
    }

    let action = if tool_name.eq_ignore_ascii_case("edit") {
        "edit"
    } else {
        "write"
    };
    Ok(Some(OmpHookOutcome::Prompt {
        title: format!("Approve external {action}"),
        message: format!(
            "Stateful is requesting approval for a repo-external OMP {action}.\n\nTargets:\n{}\n\nSet stateful.autoApprove to true to skip this prompt.",
            external_targets.join("\n")
        ),
    }))
}

fn omp_external_target_display(path: &str, cwd: Option<&Path>) -> String {
    let path = Path::new(path.trim());
    if path.is_absolute() {
        return normalize_path(path.to_path_buf())
            .to_string_lossy()
            .to_string();
    }
    let base = cwd.unwrap_or_else(|| Path::new("."));
    normalize_path(base.join(path))
        .to_string_lossy()
        .to_string()
}

fn set_omp_lineage<T>(
    request: &mut stateful_core::RequestEnvelope<T>,
    agent_id: &str,
    actor_id: Option<&str>,
    parent_agent_id: Option<&str>,
) {
    request.agent.actor_id = actor_id.unwrap_or(agent_id).to_string();
    request.agent.parent_agent_id = parent_agent_id.map(str::to_string);
}

fn should_auto_declare_omp_tool_reservation(
    input: &OmpPreToolUseInput,
    error: &str,
    targets: &[PatchTarget],
) -> bool {
    matches!(
        runtime_tool_name_leaf(&input.tool_name),
        tool_name if tool_name.eq_ignore_ascii_case("edit")
            || tool_name.eq_ignore_ascii_case("write")
    ) && (error.contains("missing_reservation") || error.contains("scope_mismatch"))
        && targets.iter().all(|target| target.action == "write_file")
}

fn declare_and_claim_omp_pre_tool_reservation(
    input: &OmpPreToolUseInput,
    runtime: &ServerRuntime,
    identity: Option<&RepoIdentity>,
    workspace_id: &str,
    targets: &[PatchTarget],
    fingerprints: &[(String, stateful_core::ContentFingerprint)],
    purpose: &str,
) -> Result<(String, Vec<String>), OmpHookOutcome> {
    let mut request = crate::v2_request_envelope(
        uuid::Uuid::new_v4(),
        input.agent_id.clone(),
        workspace_id.to_string(),
        identity.cloned(),
        stateful_core::ActorType::Agent,
        stateful_core::SourceKind::Hook,
        "reservation_declare",
        "hook:omp_pre_tool_use",
        Some(input.tool_name.clone()),
        json!({
            "scopes": targets
                .iter()
                .map(|target| stateful_core::ReservationScope::file(&target.path))
                .collect::<Vec<_>>(),
            "action": "write",
            "purpose": purpose,
        }),
    )
    .map_err(|error| OmpHookOutcome::Block {
        reason: error.to_string(),
    })?;
    set_omp_lineage(
        &mut request,
        &input.agent_id,
        input.omp_agent_id.as_deref(),
        input.parent_agent_id.as_deref(),
    );
    let response =
        crate::post_v2(runtime, "/v2/reservation/declare", &request).map_err(|error| {
            OmpHookOutcome::Block {
                reason: format!("stateful reservation declaration failed: {error}"),
            }
        })?;
    let body: serde_json::Value =
        serde_json::from_str(&response.body).map_err(|error| OmpHookOutcome::Block {
            reason: format!("invalid stateful reservation response: {error}"),
        })?;
    let reservation_id = body
        .get("reservation_id")
        .and_then(serde_json::Value::as_str)
        .map(str::to_string)
        .filter(|reservation_id| !reservation_id.is_empty())
        .ok_or_else(|| OmpHookOutcome::Block {
            reason: "stateful reservation declaration response did not include reservation_id"
                .to_string(),
        })?;

    let mut request = crate::v2_request_envelope(
        uuid::Uuid::new_v4(),
        input.agent_id.clone(),
        workspace_id.to_string(),
        identity.cloned(),
        stateful_core::ActorType::Agent,
        stateful_core::SourceKind::Hook,
        "claim_acquire",
        "hook:omp_pre_tool_use",
        Some(input.tool_name.clone()),
        json!({
            "reservation_id": reservation_id,
            "paths": fingerprints
                .iter()
                .map(|(path, before)| json!({
                    "relative_path": path,
                    "observation": {
                        "exists": before.exists,
                        "content_hash": before.sha256,
                    },
                }))
                .collect::<Vec<_>>(),
        }),
    )
    .map_err(|error| OmpHookOutcome::Block {
        reason: error.to_string(),
    })?;
    set_omp_lineage(
        &mut request,
        &input.agent_id,
        input.omp_agent_id.as_deref(),
        input.parent_agent_id.as_deref(),
    );
    let response = crate::post_v2(runtime, "/v2/claim/acquire", &request).map_err(|error| {
        OmpHookOutcome::Block {
            reason: format!("stateful claim acquisition failed: {error}"),
        }
    })?;
    let body: serde_json::Value =
        serde_json::from_str(&response.body).map_err(|error| OmpHookOutcome::Block {
            reason: format!("invalid stateful claim response: {error}"),
        })?;
    let claim_ids = body
        .get("claims")
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|claim| claim.get("claim_id").and_then(serde_json::Value::as_str))
        .map(str::to_string)
        .collect::<Vec<_>>();
    if claim_ids.is_empty() {
        return Err(OmpHookOutcome::Block {
            reason: "stateful claim acquisition response did not include claims".to_string(),
        });
    }

    Ok((reservation_id, claim_ids))
}

fn release_omp_claims(
    runtime: &ServerRuntime,
    input: &OmpPreToolUseInput,
    workspace_id: &str,
    identity: Option<&RepoIdentity>,
    claim_ids: &[String],
) {
    for claim_id in claim_ids {
        let Ok(mut request) = crate::v2_request_envelope(
            uuid::Uuid::new_v4(),
            input.agent_id.clone(),
            workspace_id.to_string(),
            identity.cloned(),
            stateful_core::ActorType::Agent,
            stateful_core::SourceKind::Hook,
            "claim_release",
            "hook:omp_pre_tool_use",
            Some(input.tool_name.clone()),
            json!({ "claim_id": claim_id }),
        ) else {
            continue;
        };
        set_omp_lineage(
            &mut request,
            &input.agent_id,
            input.omp_agent_id.as_deref(),
            input.parent_agent_id.as_deref(),
        );
        let _ = crate::post_v2(runtime, "/v2/claim/release", &request);
    }
}

fn extract_omp_edit_targets(input: &serde_json::Value) -> Vec<PatchTarget> {
    let Some(edit_input) = input.get("input").and_then(serde_json::Value::as_str) else {
        return Vec::new();
    };
    let mut targets = Vec::new();
    let mut current_path: Option<String> = None;
    let mut current_target_index: Option<usize> = None;
    for line in edit_input.lines().map(str::trim) {
        if let Some(header) = line
            .strip_prefix('[')
            .and_then(|line| line.strip_suffix(']'))
        {
            let Some((path, _)) = header.split_once('#') else {
                continue;
            };
            let path = path.trim();
            if path.is_empty() {
                current_path = None;
                current_target_index = None;
                continue;
            }
            current_path = Some(path.to_string());
            current_target_index = Some(targets.len());
            targets.push(PatchTarget::write(path));
            continue;
        }

        let Some(destination) = line
            .strip_prefix("MV ")
            .map(str::trim)
            .filter(|destination| !destination.is_empty())
        else {
            continue;
        };
        let (Some(path), Some(index)) = (current_path.as_deref(), current_target_index) else {
            continue;
        };
        targets[index] = PatchTarget::move_file(path, destination);
    }
    targets
}

pub fn handle_omp_session_start_with_runtime(
    input: &str,
    runtime: &ServerRuntime,
) -> anyhow::Result<OmpSessionStartOutput> {
    let input: OmpSessionEventInput = serde_json::from_str(input)?;
    input.validate()?;
    post_omp_presence_registration(runtime, &input, None)?;
    Ok(omp_session_start_output(
        runtime,
        &input.agent_id,
        input
            .workspace_id
            .clone()
            .unwrap_or_else(|| effective_workspace_id(runtime, None)),
    ))
}

fn handle_omp_session_start_with_identity(
    input: &str,
    runtime: &ServerRuntime,
    identity: Option<&RepoIdentity>,
) -> anyhow::Result<OmpSessionStartOutput> {
    let input: OmpSessionEventInput = serde_json::from_str(input)?;
    input.validate()?;
    post_omp_presence_registration(runtime, &input, identity)?;
    Ok(omp_session_start_output(
        runtime,
        &input.agent_id,
        input
            .workspace_id
            .clone()
            .unwrap_or_else(|| effective_workspace_id(runtime, identity)),
    ))
}

fn post_omp_presence_registration(
    runtime: &ServerRuntime,
    input: &OmpSessionEventInput,
    identity: Option<&RepoIdentity>,
) -> anyhow::Result<()> {
    let workspace_id = input
        .workspace_id
        .clone()
        .unwrap_or_else(|| effective_workspace_id(runtime, identity));
    let mut request = crate::v2_request_envelope(
        uuid::Uuid::new_v4(),
        input.agent_id.clone(),
        workspace_id,
        identity.cloned(),
        stateful_core::ActorType::Agent,
        stateful_core::SourceKind::Hook,
        "omp_session_start",
        "hook:omp_session_start",
        None,
        json!({ "first_prompt": input.prompt.as_deref() }),
    )?;
    request.agent.actor_id = input
        .omp_agent_id
        .clone()
        .unwrap_or_else(|| input.agent_id.clone());
    request.agent.parent_agent_id = input.parent_agent_id.clone();
    crate::post_v2(runtime, "/v2/session/register", &request)?;
    Ok(())
}

fn omp_session_start_output(
    runtime: &ServerRuntime,
    agent_id: &str,
    workspace_id: String,
) -> OmpSessionStartOutput {
    OmpSessionStartOutput {
        decision: "allow",
        agent_id: agent_id.to_string(),
        workspace_id: workspace_id.clone(),
        notifications_stream: OmpNotificationsStream {
            base_url: runtime.base_url.clone(),
            authorization: format!("Bearer {}", runtime.token),
            agent_id: agent_id.to_string(),
            workspace_id,
        },
    }
}

pub fn handle_omp_post_tool_use_with_runtime(
    input: &str,
    runtime: &ServerRuntime,
) -> anyhow::Result<()> {
    let input: OmpSessionEventInput = serde_json::from_str(input)?;
    input.validate()?;
    post_omp_post_tool_use_event(runtime, &input, None, None)
}

fn handle_omp_post_tool_use_with_identity(
    input: &str,
    runtime: &ServerRuntime,
    repo_root: Option<&Path>,
    identity: Option<&RepoIdentity>,
) -> anyhow::Result<()> {
    let input: OmpSessionEventInput = serde_json::from_str(input)?;
    input.validate()?;
    post_omp_post_tool_use_event(runtime, &input, repo_root, identity)
}

fn post_omp_post_tool_use_event(
    runtime: &ServerRuntime,
    input: &OmpSessionEventInput,
    repo_root: Option<&Path>,
    identity: Option<&RepoIdentity>,
) -> anyhow::Result<()> {
    let tool_name = input
        .tool_name
        .as_deref()
        .map(runtime_tool_name_leaf)
        .unwrap_or_default();
    if tool_name.eq_ignore_ascii_case("read") {
        return finish_omp_post_tool(
            runtime,
            input,
            identity,
            record_omp_read_complete(input, runtime, repo_root, identity),
        );
    }
    let auxiliary = (|| {
        if let (Some(repo_root), Some(operation_id)) = (repo_root, input.metadata.operation_id())
            && (tool_name.eq_ignore_ascii_case("write")
                || tool_name.eq_ignore_ascii_case("edit")
                || tool_name.eq_ignore_ascii_case("bash")
                || tool_name.eq_ignore_ascii_case("python"))
        {
            let paths = GlobalPaths::from_env()?;
            write_lifecycle::complete(
                &paths,
                runtime,
                &input.agent_id,
                &input
                    .workspace_id
                    .clone()
                    .unwrap_or_else(|| effective_workspace_id(runtime, identity)),
                identity,
                input.omp_agent_id.as_deref(),
                input.parent_agent_id.as_deref(),
                repo_root,
                operation_id,
                input.metadata.failed(),
                &OMP_WRITE_LIFECYCLE,
            )?;
        }
        if tool_name.eq_ignore_ascii_case("bash")
            && let Some(command) = testing_command(&input.tool_input)
        {
            post_omp_testing_result(runtime, input, identity, command)?;
        }
        Ok(())
    })();
    finish_omp_post_tool(runtime, input, identity, auxiliary)
}

fn finish_omp_post_tool(
    runtime: &ServerRuntime,
    input: &OmpSessionEventInput,
    identity: Option<&RepoIdentity>,
    auxiliary: anyhow::Result<()>,
) -> anyhow::Result<()> {
    let heartbeat = post_omp_presence_heartbeat(runtime, input, identity);
    match (auxiliary, heartbeat) {
        (Err(error), _) => Err(error),
        (Ok(()), Err(error)) => Err(error),
        (Ok(()), Ok(())) => Ok(()),
    }
}

fn post_omp_presence_heartbeat(
    runtime: &ServerRuntime,
    input: &OmpSessionEventInput,
    identity: Option<&RepoIdentity>,
) -> anyhow::Result<()> {
    let mut request = crate::v2_request_envelope(
        uuid::Uuid::new_v4(),
        input.agent_id.clone(),
        input
            .workspace_id
            .clone()
            .unwrap_or_else(|| effective_workspace_id(runtime, identity)),
        identity.cloned(),
        stateful_core::ActorType::Agent,
        stateful_core::SourceKind::Hook,
        "omp_post_tool_use",
        "hook:omp_post_tool_use",
        input.tool_name.clone(),
        json!({ "kind": "heartbeat" }),
    )?;
    set_omp_lineage(
        &mut request,
        &input.agent_id,
        input.omp_agent_id.as_deref(),
        input.parent_agent_id.as_deref(),
    );
    crate::post_v2(runtime, "/v2/presence/update", &request)?;
    Ok(())
}

fn record_omp_read_complete(
    input: &OmpSessionEventInput,
    runtime: &ServerRuntime,
    repo_root: Option<&Path>,
    identity: Option<&RepoIdentity>,
) -> anyhow::Result<()> {
    let Some(repo_root) = repo_root else {
        return Ok(());
    };
    let Some(path) = read_target(&input.tool_input, repo_root, input.cwd.as_deref())? else {
        return Ok(());
    };
    let workspace_id = input
        .workspace_id
        .clone()
        .unwrap_or_else(|| effective_workspace_id(runtime, identity));
    let classification = read_classification(&input.tool_input, &input.metadata);
    let update = if classification != stateful_core::ReadClassification::Exact {
        let mut request = crate::v2_request_envelope(
            uuid::Uuid::new_v4(),
            input.agent_id.clone(),
            workspace_id.clone(),
            identity.cloned(),
            stateful_core::ActorType::Agent,
            stateful_core::SourceKind::Hook,
            "omp_read_result",
            "hook:omp_post_tool_use",
            input.tool_name.clone(),
            json!({
                "kind": "tool_result",
                "tool_name": "read",
                "outcome": classification,
                "summary": input.metadata.result_summary(),
            }),
        )?;
        set_omp_lineage(
            &mut request,
            &input.agent_id,
            input.omp_agent_id.as_deref(),
            input.parent_agent_id.as_deref(),
        );
        Some(request)
    } else {
        None
    };
    let Some(operation_id) = input.metadata.operation_id() else {
        if let Some(update) = update {
            crate::post_v2(runtime, "/v2/presence/update", &update)?;
        }
        return Ok(());
    };
    let semantic_marker = input
        .metadata
        .result_summary()
        .map(str::to_string)
        .or_else(|| (!input.result_metadata.is_null()).then(|| input.result_metadata.to_string()));
    let mut payload = json!({
        "operation_id": operation_id,
        "path": path,
        "classification": classification,
        "semantic_marker": semantic_marker,
    });
    if classification == stateful_core::ReadClassification::Exact {
        payload["after"] = serde_json::to_value(observation::fingerprint(repo_root, &path)?)?;
    }
    let mut request = crate::v2_request_envelope(
        uuid::Uuid::new_v4(),
        input.agent_id.clone(),
        workspace_id,
        identity.cloned(),
        stateful_core::ActorType::Agent,
        stateful_core::SourceKind::Hook,
        "omp_read_complete",
        "hook:omp_post_tool_use",
        input.tool_name.clone(),
        payload,
    )?;
    set_omp_lineage(
        &mut request,
        &input.agent_id,
        input.omp_agent_id.as_deref(),
        input.parent_agent_id.as_deref(),
    );
    post_durable_v2(runtime, "/v2/read/complete", &request)?;
    if let Some(update) = update {
        crate::post_v2(runtime, "/v2/presence/update", &update)?;
    }
    Ok(())
}

fn post_omp_activity_finalize(
    runtime: &ServerRuntime,
    input: &OmpSessionEventInput,
    identity: Option<&RepoIdentity>,
) -> anyhow::Result<()> {
    let mut request = crate::v2_request_envelope(
        uuid::Uuid::new_v4(),
        input.agent_id.clone(),
        input
            .workspace_id
            .clone()
            .unwrap_or_else(|| effective_workspace_id(runtime, identity)),
        identity.cloned(),
        stateful_core::ActorType::Agent,
        stateful_core::SourceKind::Hook,
        "omp_stop",
        "hook:omp_stop",
        input.tool_name.clone(),
        json!({}),
    )?;
    set_omp_lineage(
        &mut request,
        &input.agent_id,
        input.omp_agent_id.as_deref(),
        input.parent_agent_id.as_deref(),
    );
    post_durable_v2(runtime, "/v2/activity/finalize", &request)?;
    Ok(())
}

fn prepare_pre_tool_use_runtime(
    repo_root: &Path,
    paths: &GlobalPaths,
    _input: &str,
) -> anyhow::Result<ServerRuntime> {
    if !runtime_env_override_is_configured() {
        ensure_server(paths)?;
    }
    discover_runtime_with_global(repo_root, paths)
}

pub fn handle_session_start_in_repo(
    input: &str,
    repo_root: impl AsRef<Path>,
) -> anyhow::Result<()> {
    let _ = handle_session_start_context_in_repo(input, repo_root)?;
    Ok(())
}

fn handle_session_start_context_in_repo(
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
    let identity = repo_identity(&paths, &repo_root)?;
    if let Err(error) = write_lifecycle::replay_pending(&paths, &runtime, &repo_root) {
        eprintln!("stateful write lifecycle replay warning: {error}");
    }
    if let Err(error) = sync_outbox_with_runtime(&paths, &runtime) {
        eprintln!("stateful lifecycle outbox replay warning: {error}");
    }
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
    let identity = repo_identity(&paths, &repo_root)?;
    let _ = sync_outbox_with_runtime(&paths, &runtime);
    if let Err(error) =
        handle_post_tool_use_with_runtime(input, &runtime, Some(&repo_root), Some(&identity))
    {
        let input: SessionEventInput = serde_json::from_str(input)?;
        let workspace_id = effective_workspace_id(&runtime, Some(&identity));
        queue_session_heartbeat_outbox(
            &paths,
            &workspace_id,
            input.stateful_agent_id(),
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
    let identity = repo_identity(&paths, &repo_root)?;
    let _ = sync_outbox_with_runtime(&paths, &runtime);
    let input: input::CodexUserPrompt = serde_json::from_str(input)?;
    input.validate()?;
    handle_user_prompt_submit_with_runtime(&input, &runtime, Some(&identity))
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
    let identity = repo_identity(&paths, &repo_root)?;
    let _ = sync_outbox_with_runtime(&paths, &runtime);
    handle_stop_with_runtime(input, &runtime, Some(&identity))
}

fn handle_session_start_with_runtime(
    input: &str,
    runtime: &ServerRuntime,
    identity: Option<&RepoIdentity>,
) -> anyhow::Result<String> {
    let input: input::CodexSessionStart = serde_json::from_str(input)?;
    input.validate()?;
    let workspace_id = effective_workspace_id(runtime, identity);
    let registration = crate::v2_request_envelope(
        uuid::Uuid::new_v4(),
        input.agent_id.clone(),
        workspace_id.clone(),
        identity.cloned(),
        stateful_core::ActorType::Agent,
        stateful_core::SourceKind::Hook,
        "codex_session_start",
        "hook:codex_session_start",
        None,
        json!({ "first_prompt": input.first_prompt() }),
    )?;
    crate::post_v2(runtime, "/v2/session/register", &registration)?;
    delivery::render_and_ack_context(
        runtime,
        &input.agent_id,
        &workspace_id,
        identity,
        "codex_session_start",
        "hook:codex_session_start",
    )
}

fn handle_post_tool_use_with_runtime(
    input: &str,
    runtime: &ServerRuntime,
    repo_root: Option<&Path>,
    identity: Option<&RepoIdentity>,
) -> anyhow::Result<()> {
    let input: SessionEventInput = serde_json::from_str(input)?;
    let tool_name = input
        .tool_name
        .as_deref()
        .map(runtime_tool_name_leaf)
        .unwrap_or_default();
    if tool_name.eq_ignore_ascii_case("read") {
        record_read_complete(&input, runtime, repo_root, identity)?;
        return post_codex_presence_heartbeat(runtime, &input, identity);
    }
    if matches!(
        runtime_tool_name_leaf(tool_name)
            .to_ascii_lowercase()
            .as_str(),
        "write" | "edit" | "apply_patch" | "file_change"
    ) {
        if let (Some(repo_root), Some(operation_id)) = (repo_root, input.metadata.operation_id()) {
            let paths = GlobalPaths::from_env()?;
            write_lifecycle::complete(
                &paths,
                runtime,
                input.stateful_agent_id(),
                &effective_workspace_id(runtime, identity),
                identity,
                None,
                None,
                repo_root,
                operation_id,
                input.metadata.failed(),
                &CODEX_WRITE_LIFECYCLE,
            )?;
        }
    }
    if tool_name.eq_ignore_ascii_case("bash")
        && let Some(command) = testing_command(&input.tool_input)
    {
        post_codex_testing_result(runtime, &input, identity, command)?;
    }
    post_codex_presence_heartbeat(runtime, &input, identity)
}

fn handle_user_prompt_submit_with_runtime(
    input: &input::CodexUserPrompt,
    runtime: &ServerRuntime,
    identity: Option<&RepoIdentity>,
) -> anyhow::Result<String> {
    let workspace_id = effective_workspace_id(runtime, identity);
    if let Some(prompt) = input.prompt()
        && !delivery::presence_has_goal(runtime, &input.agent_id, &workspace_id, identity)?
    {
        let update = crate::v2_request_envelope(
            uuid::Uuid::new_v4(),
            input.agent_id.clone(),
            workspace_id.clone(),
            identity.cloned(),
            stateful_core::ActorType::Agent,
            stateful_core::SourceKind::Hook,
            "codex_first_prompt",
            "hook:codex_user_prompt",
            None,
            json!({
                "kind": "update",
                "goal_excerpt": prompt,
            }),
        )?;
        crate::post_v2(runtime, "/v2/presence/update", &update)?;
    }
    let prompt_text = delivery::render_and_ack_context(
        runtime,
        &input.agent_id,
        &workspace_id,
        identity,
        "codex_user_prompt",
        "hook:codex_user_prompt",
    )?;
    Ok(if prompt_text.trim().is_empty() {
        String::new()
    } else {
        with_stateful_command_policy_reminder(prompt_text)
    })
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
        "Stateful command policy reminder:\n- Use `state_context_render` only for planning/manual inspection when active coordination may affect the plan; if you already inspected this turn for the same resource, reuse that result.\n- Before using Bash or eval tools, use the `stateful-command-policy` skill.\n- Use active Stateful native coordination tools only when they appear in the tool list, for example `state_reservation_declare` and `state_claim_acquire`; runtime-specific names must be copied exactly. If those tools are absent, use OMP native edit/write auto-declare, lazy resume helpers, or an existing `reservation_id` with trusted sandbox write-targets. Do not run `stateful reservation declare` through Bash.\n- Raw Bash is denied for Codex. OMP raw Bash and Python/JavaScript/JS/Ruby/Julia eval tools are denied; use built-in Bash with trusted `{binary} sandbox run` or `{binary} sandbox process find` commands.\n- Use `{binary} sandbox run --fs read-only --network disabled ...` for read-only shell fallback, `{binary} sandbox run --fs build --network enabled --write-dir <scratch-purpose> ...` for builds/tests, and `{binary} sandbox run --fs write-targets --write-target <file> ...` for command-shaped edits.\n- Use `{binary} sandbox run --fs git --network disabled ...` for local git, `{binary} sandbox run --fs git --network enabled ...` only for explicit remote git operations, and `{binary} sandbox run --fs github-pr --network enabled ...` for GitHub PR operations."
    )
}

fn handle_stop_with_runtime(
    input: &str,
    runtime: &ServerRuntime,
    identity: Option<&RepoIdentity>,
) -> anyhow::Result<()> {
    let input: SessionEventInput = serde_json::from_str(input)?;
    let handoff = input.explicit_handoff().unwrap_or(serde_json::Value::Null);
    let request = crate::v2_request_envelope(
        uuid::Uuid::new_v4(),
        input.stateful_agent_id().to_string(),
        effective_workspace_id(runtime, identity),
        identity.cloned(),
        stateful_core::ActorType::Agent,
        stateful_core::SourceKind::Hook,
        "codex_stop",
        "hook:codex_stop",
        None,
        handoff,
    )?;
    post_durable_v2(runtime, "/v2/activity/finalize", &request)?;
    Ok(())
}

fn effective_workspace_id(runtime: &ServerRuntime, identity: Option<&RepoIdentity>) -> String {
    effective_workspace_id_for_repo(&runtime.workspace_id, identity)
}

fn read_target(
    tool_input: &serde_json::Value,
    repo_root: &Path,
    cwd: Option<&Path>,
) -> anyhow::Result<Option<String>> {
    let Some(path) = read_input_path(tool_input) else {
        return Ok(None);
    };
    normalize_file_tool_target(read_selector_target(path).target, Some(repo_root), cwd)
}

fn read_input_path(tool_input: &serde_json::Value) -> Option<&str> {
    tool_input
        .get("file_path")
        .or_else(|| tool_input.get("path"))
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|path| !path.is_empty())
}

struct ReadSelector<'a> {
    target: &'a str,
    raw: bool,
    has_line_selector: bool,
}

fn read_selector_target(path: &str) -> ReadSelector<'_> {
    let mut target = path.trim();
    let mut raw = false;
    let mut has_line_selector = false;
    loop {
        if let Some(stripped) = target.strip_suffix(":raw") {
            raw = true;
            target = stripped;
            continue;
        }
        let Some((stripped, selector)) = target.rsplit_once(':') else {
            break;
        };
        if is_read_line_selector(selector) {
            has_line_selector = true;
            target = stripped;
            continue;
        }
        break;
    }
    ReadSelector {
        target,
        raw,
        has_line_selector,
    }
}

fn is_read_line_selector(selector: &str) -> bool {
    selector
        .bytes()
        .next()
        .is_some_and(|byte| byte.is_ascii_digit())
        && selector
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'-' | b'+' | b','))
}

fn read_classification(
    tool_input: &serde_json::Value,
    metadata: &input::ToolMetadata,
) -> stateful_core::ReadClassification {
    let classification = observation::classification(metadata);
    if classification != stateful_core::ReadClassification::Exact {
        return classification;
    }
    let selector = read_input_path(tool_input).map(read_selector_target);
    if selector
        .as_ref()
        .is_some_and(|selector| selector.has_line_selector)
    {
        return stateful_core::ReadClassification::Partial;
    }
    if !selector.as_ref().is_some_and(|selector| selector.raw)
        || ["offset", "limit", "start_line", "end_line", "line_start", "line_end"]
            .iter()
            .any(|key| tool_input.get(*key).is_some_and(|value| !value.is_null()))
    {
        stateful_core::ReadClassification::StructuralSummary
    } else {
        classification
    }
}

fn record_read_start(
    input: &PreToolUseInput,
    runtime: Option<&ServerRuntime>,
    repo_root: Option<&Path>,
    cwd: Option<&Path>,
    identity: Option<&RepoIdentity>,
) -> anyhow::Result<()> {
    let (Some(runtime), Some(repo_root), Some(operation_id)) =
        (runtime, repo_root, input.metadata.operation_id())
    else {
        return Ok(());
    };
    let Some(path) = read_target(&input.tool_input, repo_root, cwd)? else {
        return Ok(());
    };
    let before = observation::fingerprint(repo_root, &path)?;
    let workspace_id = effective_workspace_id(runtime, identity);
    let request = crate::v2_request_envelope(
        uuid::Uuid::new_v4(),
        input.stateful_agent_id().to_string(),
        workspace_id.clone(),
        identity.cloned(),
        stateful_core::ActorType::Agent,
        stateful_core::SourceKind::Hook,
        "codex_read_start",
        "hook:codex_pre_tool_use",
        Some(input.tool_name.clone()),
        json!({
            "operation_id": operation_id,
            "path": path,
            "before": before,
        }),
    )?;
    let failed_completion = crate::v2_request_envelope(
        uuid::Uuid::new_v4(),
        input.stateful_agent_id().to_string(),
        workspace_id,
        identity.cloned(),
        stateful_core::ActorType::Agent,
        stateful_core::SourceKind::Hook,
        "codex_read_complete",
        "hook:codex_pre_tool_use",
        Some(input.tool_name.clone()),
        json!({
            "operation_id": operation_id,
            "path": path,
            "classification": stateful_core::ReadClassification::Failed,
            "semantic_marker": "pre_tool_start_delivery_failed",
        }),
    )?;
    post_durable_read_start(runtime, &request, &failed_completion)?;
    Ok(())
}

fn record_read_complete(
    input: &SessionEventInput,
    runtime: &ServerRuntime,
    repo_root: Option<&Path>,
    identity: Option<&RepoIdentity>,
) -> anyhow::Result<()> {
    let Some(repo_root) = repo_root else {
        return Ok(());
    };
    let Some(path) = read_target(&input.tool_input, repo_root, input.cwd.as_deref())? else {
        return Ok(());
    };
    let classification = read_classification(&input.tool_input, &input.metadata);
    let workspace_id = effective_workspace_id(runtime, identity);
    let update = if classification != stateful_core::ReadClassification::Exact {
        Some(crate::v2_request_envelope(
            uuid::Uuid::new_v4(),
            input.stateful_agent_id().to_string(),
            workspace_id.clone(),
            identity.cloned(),
            stateful_core::ActorType::Agent,
            stateful_core::SourceKind::Hook,
            "codex_read_result",
            "hook:codex_post_tool_use",
            input.tool_name.clone(),
            json!({
                "kind": "tool_result",
                "tool_name": "Read",
                "outcome": classification,
                "summary": input.metadata.result_summary(),
            }),
        )?)
    } else {
        None
    };
    let Some(operation_id) = input.metadata.operation_id() else {
        if let Some(update) = update {
            crate::post_v2(runtime, "/v2/presence/update", &update)?;
        }
        return Ok(());
    };
    let mut payload = json!({
        "operation_id": operation_id,
        "path": path,
        "classification": classification,
        "semantic_marker": input.metadata.result_summary(),
    });
    if classification == stateful_core::ReadClassification::Exact {
        payload["after"] = serde_json::to_value(observation::fingerprint(repo_root, &path)?)?;
    }
    let request = crate::v2_request_envelope(
        uuid::Uuid::new_v4(),
        input.stateful_agent_id().to_string(),
        workspace_id,
        identity.cloned(),
        stateful_core::ActorType::Agent,
        stateful_core::SourceKind::Hook,
        "codex_read_complete",
        "hook:codex_post_tool_use",
        input.tool_name.clone(),
        payload,
    )?;
    post_durable_v2(runtime, "/v2/read/complete", &request)?;
    if let Some(update) = update {
        crate::post_v2(runtime, "/v2/presence/update", &update)?;
    }
    Ok(())
}

fn post_codex_presence_heartbeat(
    runtime: &ServerRuntime,
    input: &SessionEventInput,
    identity: Option<&RepoIdentity>,
) -> anyhow::Result<()> {
    let request = crate::v2_request_envelope(
        uuid::Uuid::new_v4(),
        input.stateful_agent_id().to_string(),
        effective_workspace_id(runtime, identity),
        identity.cloned(),
        stateful_core::ActorType::Agent,
        stateful_core::SourceKind::Hook,
        "codex_post_tool_use",
        "hook:codex_post_tool_use",
        input.tool_name.clone(),
        json!({ "kind": "heartbeat" }),
    )?;
    crate::post_v2(runtime, "/v2/presence/update", &request)?;
    Ok(())
}

fn post_durable_read_start(
    runtime: &ServerRuntime,
    request: &stateful_core::RequestEnvelope<serde_json::Value>,
    failed_completion: &stateful_core::RequestEnvelope<serde_json::Value>,
) -> anyhow::Result<()> {
    let paths = GlobalPaths::from_env()?;
    crate::outbox::post_durable_read_start_pair(&paths, runtime, request, failed_completion)
}

fn post_durable_v2(
    runtime: &ServerRuntime,
    route: &str,
    request: &stateful_core::RequestEnvelope<serde_json::Value>,
) -> anyhow::Result<()> {
    let paths = GlobalPaths::from_env()?;
    let serialized = exact_envelope_json(request)?;
    queue_exact_envelope(&paths, route, request)?;
    crate::replay_v2_request(runtime, route, &serialized)?;
    acknowledge_exact_envelope(&paths, request)
}

fn testing_command(tool_input: &serde_json::Value) -> Option<String> {
    let command = tool_input.get("command")?.as_str()?;
    let command = parse_sandbox_run_bash_invocation(command)
        .map(|invocation| invocation.request.command)
        .unwrap_or_else(|_| command.to_string());
    let words = split_simple_command_words(&command).ok()?;
    let canonical = match words.as_slice() {
        [command, subcommand, ..] if command == "cargo" && subcommand == "test" => "cargo test",
        [command, subcommand, action, ..]
            if command == "cargo" && subcommand == "nextest" && action == "run" =>
        {
            "cargo nextest"
        }
        [command, ..] if command == "pytest" => "pytest",
        [python, module, command, ..]
            if python == "python" && module == "-m" && command == "pytest" =>
        {
            "pytest"
        }
        [command, subcommand, ..] if command == "go" && subcommand == "test" => "go test",
        [command, subcommand, ..] if command == "npm" && subcommand == "test" => "npm test",
        [command, subcommand, ..] if command == "bun" && subcommand == "test" => "bun test",
        [command, subcommand, ..] if command == "yarn" && subcommand == "test" => "yarn test",
        [command, subcommand, ..] if command == "pnpm" && subcommand == "test" => "pnpm test",
        [command, ..] if command == "jest" => "jest",
        _ => return None,
    };
    Some(canonical.to_string())
}

fn post_codex_testing_start(
    input: &PreToolUseInput,
    runtime: &ServerRuntime,
    identity: Option<&RepoIdentity>,
    tool_name: String,
) -> anyhow::Result<()> {
    post_testing_presence(
        runtime,
        input.stateful_agent_id(),
        &effective_workspace_id(runtime, identity),
        identity,
        tool_name,
        None,
    )
}

fn post_codex_testing_result(
    runtime: &ServerRuntime,
    input: &SessionEventInput,
    identity: Option<&RepoIdentity>,
    tool_name: String,
) -> anyhow::Result<()> {
    post_testing_presence(
        runtime,
        input.stateful_agent_id(),
        &effective_workspace_id(runtime, identity),
        identity,
        tool_name,
        Some(!input.metadata.failed()),
    )
}

fn post_omp_testing_start(
    input: &OmpPreToolUseInput,
    runtime: Option<&ServerRuntime>,
    identity: Option<&RepoIdentity>,
    tool_name: String,
) -> anyhow::Result<()> {
    let Some(runtime) = runtime else {
        return Ok(());
    };
    post_testing_presence(
        runtime,
        &input.agent_id,
        &input.workspace_id.clone().unwrap_or_else(|| effective_workspace_id(runtime, identity)),
        identity,
        tool_name,
        None,
    )
}

fn post_omp_testing_result(
    runtime: &ServerRuntime,
    input: &OmpSessionEventInput,
    identity: Option<&RepoIdentity>,
    tool_name: String,
) -> anyhow::Result<()> {
    post_testing_presence(
        runtime,
        &input.agent_id,
        &input.workspace_id.clone().unwrap_or_else(|| effective_workspace_id(runtime, identity)),
        identity,
        tool_name,
        Some(!input.metadata.failed()),
    )
}

fn post_testing_presence(
    runtime: &ServerRuntime,
    agent_id: &str,
    workspace_id: &str,
    identity: Option<&RepoIdentity>,
    tool_name: String,
    succeeded: Option<bool>,
) -> anyhow::Result<()> {
    let request = crate::v2_request_envelope(
        uuid::Uuid::new_v4(),
        agent_id.to_string(),
        workspace_id.to_string(),
        identity.cloned(),
        stateful_core::ActorType::Agent,
        stateful_core::SourceKind::Hook,
        "hook_testing",
        "hook:post_tool_use",
        Some(tool_name.clone()),
        match succeeded {
            None => json!({ "kind": "tool_start", "tool_name": tool_name }),
            Some(succeeded) => json!({
                "kind": "tool_result",
                "tool_name": tool_name,
                "outcome": if succeeded { "succeeded" } else { "failed" },
            }),
        },
    )?;
    crate::post_v2(runtime, "/v2/presence/update", &request)?;
    Ok(())
}

fn handle_pre_tool_use_with_runtime(
    input: &str,
    runtime: Option<&ServerRuntime>,
    repo_root: Option<&Path>,
    cwd: Option<&Path>,
) -> anyhow::Result<HookOutcome> {
    let input: PreToolUseInput = serde_json::from_str(input)?;
    if let Err(error) = input.validate() {
        return Ok(HookOutcome::Deny {
            reason: format!("stateful hook rejected invalid input: {error}"),
        });
    }
    let global_paths = GlobalPaths::from_env().ok();
    let identity = repo_root.and_then(|repo_root| {
        global_paths
            .as_ref()
            .and_then(|paths| repo_identity_for_enabled_repo(paths, repo_root).ok())
    });

    let command = runtime_tool_name_leaf(&input.tool_name)
        .eq_ignore_ascii_case("bash")
        .then(|| testing_command(&input.tool_input))
        .flatten();
    let tool_name = runtime_tool_name_leaf(&input.tool_name);
    let outcome = match tool_name {
        tool_name if tool_name.eq_ignore_ascii_case("read") => {
            match record_read_start(&input, runtime, repo_root, cwd, identity.as_ref()) {
                Ok(()) => Ok(HookOutcome::Allow),
                Err(error) => Ok(HookOutcome::Deny {
                    reason: format!("stateful read lifecycle start failed: {error}"),
                }),
            }
        }
        tool_name if tool_name.eq_ignore_ascii_case("bash") => authorize_bash(&input),
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
        _ if is_stateful_control_plane_tool(&input.tool_name) => Ok(HookOutcome::Allow),
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
    }?;
    if matches!(outcome, HookOutcome::Allow | HookOutcome::AllowWithContext { .. })
        && let (Some(command), Some(runtime)) = (command, runtime)
    {
        post_codex_testing_start(&input, runtime, identity.as_ref(), command)?;
    }
    Ok(outcome)
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
        "yield",
        "parallel_tool_calls",
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
            | "state_reservation_declare"
            | "state.reservation.declare"
            | "state_reservation_request"
            | "state.reservation.request"
            | "state_reservation_claim"
            | "state.reservation.claim"
            | "state_reservation_cancel"
            | "state.reservation.cancel"
            | "state_claim_acquire"
            | "state.claim.acquire"
            | "state_claim_release"
            | "state.claim.release"
            | "state_activity_finalize"
            | "state.activity.finalize"
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
    Some(bash_policy_deny(stateful_coordination_tool_guidance()))
}

fn command_mentions_stateful_coordination(command: &str) -> bool {
    let Ok(words) = split_simple_command_words(command) else {
        return false;
    };
    matches!(words.get(1).map(String::as_str), Some("reservation"))
        || matches!(
            (
                words.get(1).map(String::as_str),
                words.get(2).map(String::as_str)
            ),
            (Some("mcp"), Some("call"))
        )
}

fn stateful_coordination_tool_guidance() -> &'static str {
    "Use active Stateful native coordination tools only when they appear in the tool list, for example `state_reservation_declare` and `state_claim_acquire`; runtime-specific names must be copied exactly. If those tools are absent, use OMP native edit/write auto-declare or lazy resume helpers instead of inventing tools. Do not run `stateful reservation declare` or legacy `stateful mcp call` through Bash."
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
        return bash_policy_deny("stateful sandbox run requires a trusted stateful binary");
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
            "stateful sandbox process find requires a trusted stateful binary",
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
            "stateful sandbox run-nested-codex-benchmark requires a trusted stateful binary",
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
        return bash_policy_deny("stateful control commands require a trusted stateful binary");
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
        "Use the `stateful-command-policy` skill before Bash or eval tools; use `state_context_render` only for planning/manual inspection when active coordination may affect the plan. Raw Bash is denied for Codex. OMP raw Bash and Python/JavaScript/JS/Ruby/Julia eval tools are denied; use built-in Bash with trusted `{binary} sandbox run` or `{binary} sandbox process find` commands. Use `{binary} sandbox run --fs read-only --network disabled --command ...` for read-only shell fallback, `{binary} sandbox run --fs build --network enabled --write-dir <scratch-purpose> --command ...` for builds/tests, and `{binary} sandbox run --fs write-targets --write-target <file> --command ...` for command-shaped edits. Use `{binary} sandbox run --fs git --network disabled ...` for local git, `{binary} sandbox run --fs git --network enabled ...` only for explicit remote git operations, and `{binary} sandbox run --fs github-pr --network enabled ...` for GitHub PR operations. Use active Stateful native coordination tools only when they appear in the tool list, for example `state_reservation_declare` and `state_claim_acquire`; runtime-specific names must be copied exactly. If those tools are absent, use OMP native edit/write auto-declare or lazy resume helpers instead of inventing tools. Do not run `stateful reservation declare` through Bash."
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
    let Ok(current) = std::env::current_exe() else {
        return false;
    };
    let current = current.canonicalize().unwrap_or(current);

    if is_verified_bare_stateful_executable(
        executable,
        &current,
        std::env::var_os("PATH").as_deref(),
    ) {
        return true;
    }

    let path = Path::new(executable);
    if !path.is_absolute() {
        return false;
    }
    let Ok(candidate) = path.canonicalize() else {
        return false;
    };
    candidate == current
}

fn is_verified_bare_stateful_executable(
    executable: &str,
    current: &Path,
    path_env: Option<&OsStr>,
) -> bool {
    if executable != "stateful" {
        return false;
    }
    let Some(path_env) = path_env else {
        return false;
    };

    for directory in std::env::split_paths(path_env) {
        let candidate = directory.join("stateful");
        if !is_executable_file(&candidate) {
            continue;
        }
        if !candidate.is_absolute() {
            return false;
        }
        return files_have_same_bytes(current, &candidate);
    }
    false
}

fn files_have_same_bytes(left: &Path, right: &Path) -> bool {
    let Ok(left) = fs::read(left) else {
        return false;
    };
    let Ok(right) = fs::read(right) else {
        return false;
    };
    left == right
}

fn is_executable_file(path: &Path) -> bool {
    let Ok(metadata) = fs::metadata(path) else {
        return false;
    };
    if !metadata.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        metadata.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        true
    }
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
    authorize_targets(input, runtime, repo_root, targets, identity)
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

    authorize_targets(
        input,
        runtime,
        repo_root,
        vec![PatchTarget::write(&target)],
        identity,
    )
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

fn authorize_targets(
    input: &PreToolUseInput,
    runtime: Option<&ServerRuntime>,
    repo_root: Option<&Path>,
    targets: Vec<PatchTarget>,
    identity: Option<&RepoIdentity>,
) -> anyhow::Result<HookOutcome> {
    if targets
        .first()
        .is_some_and(|first| targets.iter().any(|target| target.action != first.action))
    {
        return Ok(HookOutcome::Deny {
            reason: format!(
                "{} mixes write actions; split the operation into one action per patch",
                input.tool_name
            ),
        });
    }
    let Some(repo_root) = repo_root else {
        return Ok(HookOutcome::Deny {
            reason: format!("{} writes require an enabled repository", input.tool_name),
        });
    };
    if let Err(error) =
        shadow_guard::check_paths_for_dependency_shadowing(repo_root, shadow_write_paths(&targets))
    {
        return Ok(HookOutcome::Deny {
            reason: error.to_string(),
        });
    }
    let Some(runtime) = runtime else {
        return Ok(HookOutcome::Deny {
            reason: format!(
                "{} writes require a reachable stateful.v2 server",
                input.tool_name
            ),
        });
    };
    let Some(operation_id) = input.metadata.operation_id() else {
        return Ok(HookOutcome::Deny {
            reason: format!(
                "{} writes require an operation ID for stateful.v2 authorization",
                input.tool_name
            ),
        });
    };
    if targets.is_empty() {
        return Ok(HookOutcome::Allow);
    }
    let mut fingerprints = Vec::with_capacity(targets.len() * 2);
    for target in &targets {
        fingerprints.push((
            target.path.clone(),
            stateful_core::fingerprint_path(&repo_root.join(&target.path))?,
        ));
        if let Some(new_path) = &target.new_path {
            fingerprints.push((
                new_path.clone(),
                stateful_core::fingerprint_path(&repo_root.join(new_path))?,
            ));
        }
    }
    let paths = GlobalPaths::from_env()?;
    let workspace_id = effective_workspace_id(runtime, identity);
    match write_lifecycle::authorize_at_root(
        &paths,
        runtime,
        input.stateful_agent_id(),
        &workspace_id,
        repo_root,
        identity,
        None,
        None,
        operation_id,
        targets[0].action,
        fingerprints,
        input.reservation_id(),
        Vec::new(),
        &CODEX_WRITE_LIFECYCLE,
    ) {
        Ok(authorization) => Ok(match authorization.decision {
            Some(decision) if decision.decision == stateful_core::DecisionKind::Warn => {
                HookOutcome::AllowWithContext {
                    message: format!("stateful warning: {}", decision.message),
                }
            }
            Some(decision) if decision.decision == stateful_core::DecisionKind::Deny => {
                write_lifecycle::complete(
                    &paths,
                    runtime,
                    input.stateful_agent_id(),
                    &workspace_id,
                    identity,
                    None,
                    None,
                    repo_root,
                    operation_id,
                    true,
                    &CODEX_WRITE_LIFECYCLE,
                )?;
                HookOutcome::Deny {
                    reason: decision.required_next_action.unwrap_or(decision.message),
                }
            }
            _ => HookOutcome::Allow,
        }),
        Err(error) => Ok(HookOutcome::Deny {
            reason: format!("stateful.v2 write authorization failed: {error}"),
        }),
    }
}

fn shadow_write_paths(targets: &[PatchTarget]) -> impl Iterator<Item = &str> {
    targets.iter().filter_map(|target| match target.action {
        "write_file" => Some(target.path.as_str()),
        "move_file" => target.new_path.as_deref(),
        _ => None,
    })
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

fn validate_agent_id(agent_id: &str, label: &str) -> anyhow::Result<()> {
    if agent_id.is_empty() {
        anyhow::bail!("{label} is set but empty");
    }
    if !agent_id
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
    {
        anyhow::bail!("{label} contains unsupported characters");
    }
    Ok(())
}

#[derive(Debug, Deserialize)]
pub struct OmpPreToolUseInput {
    agent_id: String,
    #[serde(default)]
    parent_agent_id: Option<String>,
    #[serde(default)]
    omp_agent_id: Option<String>,
    #[serde(default)]
    workspace_id: Option<String>,
    #[serde(default)]
    reservation_id: Option<String>,
    #[serde(flatten)]
    metadata: input::ToolMetadata,
    tool_name: String,
    #[serde(default)]
    tool_input: serde_json::Value,
}
impl OmpPreToolUseInput {
    fn validate(&self) -> anyhow::Result<()> {
        validate_agent_id(&self.agent_id, "agent_id")
    }

    fn command(&self) -> Option<&str> {
        self.tool_input.get("command")?.as_str()
    }

    fn reservation_id(&self) -> Option<&str> {
        self.reservation_id
            .as_deref()
            .map(str::trim)
            .filter(|reservation_id| !reservation_id.is_empty())
            .or_else(|| {
                self.tool_input
                    .get("reservation_id")
                    .and_then(serde_json::Value::as_str)
                    .map(str::trim)
                    .filter(|reservation_id| !reservation_id.is_empty())
            })
    }
}

#[derive(Debug, Deserialize)]
pub struct OmpSessionEventInput {
    agent_id: String,
    #[serde(default)]
    parent_agent_id: Option<String>,
    #[serde(default)]
    omp_agent_id: Option<String>,
    #[serde(default)]
    workspace_id: Option<String>,
    #[serde(default)]
    cwd: Option<PathBuf>,
    #[serde(default)]
    prompt: Option<String>,
    #[serde(flatten)]
    metadata: input::ToolMetadata,
    #[serde(default)]
    result_metadata: serde_json::Value,
    #[serde(default)]
    tool_name: Option<String>,
    #[serde(default)]
    tool_input: serde_json::Value,
}

impl OmpSessionEventInput {
    fn validate(&self) -> anyhow::Result<()> {
        validate_agent_id(&self.agent_id, "agent_id")
    }
}

#[derive(Debug, Deserialize)]
struct PreToolUseInput {
    #[serde(flatten)]
    runtime: RuntimeHookInput,
    #[serde(flatten)]
    metadata: input::ToolMetadata,
    #[serde(default)]
    reservation_id: Option<String>,
    tool_name: String,
    #[serde(default)]
    tool_input: serde_json::Value,
}

#[derive(Debug, Deserialize)]
struct RuntimeHookInput {
    #[serde(alias = "session_id")]
    agent_id: String,
}

impl RuntimeHookInput {
    fn stateful_agent_id(&self) -> &str {
        &self.agent_id
    }
}

impl RuntimeHookInput {
    fn validate(&self) -> anyhow::Result<()> {
        validate_agent_id(&self.agent_id, "agent_id")
    }
}

#[derive(Debug, Deserialize)]
struct HookCwdInput {
    #[serde(default)]
    cwd: Option<PathBuf>,
}

impl PreToolUseInput {
    fn stateful_agent_id(&self) -> &str {
        self.runtime.stateful_agent_id()
    }

    fn command(&self) -> Option<&str> {
        self.tool_input.get("command")?.as_str()
    }
    fn reservation_id(&self) -> Option<&str> {
        self.reservation_id
            .as_deref()
            .or_else(|| {
                self.tool_input
                    .get("reservation_id")
                    .and_then(serde_json::Value::as_str)
            })
            .map(str::trim)
            .filter(|reservation_id| !reservation_id.is_empty())
    }

    fn patch_text(&self) -> Option<&str> {
        string_payload_field(
            &self.tool_input,
            &["command", "patch", "input", "cmd", "diff"],
        )
    }
}

impl PreToolUseInput {
    fn validate(&self) -> anyhow::Result<()> {
        self.runtime.validate()
    }
}

#[derive(Debug, Deserialize)]
struct SessionEventInput {
    #[serde(flatten)]
    runtime: RuntimeHookInput,
    #[serde(flatten)]
    metadata: input::ToolMetadata,
    #[serde(default)]
    handoff: Option<serde_json::Value>,
    #[serde(default)]
    cwd: Option<PathBuf>,
    #[serde(default)]
    tool_name: Option<String>,
    #[serde(default)]
    tool_input: serde_json::Value,
}

impl SessionEventInput {
    fn stateful_agent_id(&self) -> &str {
        self.runtime.stateful_agent_id()
    }

    fn explicit_handoff(&self) -> Option<serde_json::Value> {
        self.handoff
            .clone()
            .or_else(|| self.tool_input.get("handoff").cloned())
            .filter(serde_json::Value::is_object)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runtime_hook_input_uses_explicit_agent_id() {
        let input = RuntimeHookInput {
            agent_id: "codex-agent-1".to_string(),
        };

        assert_eq!(input.stateful_agent_id(), "codex-agent-1");
    }

    #[test]
    fn runtime_hook_input_uses_codex_session_id_parameter() {
        let input: RuntimeHookInput = serde_json::from_value(serde_json::json!({
            "session_id": "019f1a3f-0e81-7250-8597-24dd8ef18fb4"
        }))
        .expect("Codex session_id parameter should deserialize as active agent id");

        assert_eq!(
            input.stateful_agent_id(),
            "019f1a3f-0e81-7250-8597-24dd8ef18fb4"
        );
    }

    #[test]
    fn hook_session_extraction_does_not_keep_fake_runtime_adapter() {
        let source = include_str!("hook.rs");

        assert!(!source.contains(concat!("Claude", "CodeRuntimeAdapter")));
        assert!(!source.contains(concat!("trait ", "RuntimeAdapter")));
    }

    #[test]
    fn bare_stateful_is_trusted_when_first_path_match_has_current_binary_bytes() {
        let temp =
            std::env::temp_dir().join(format!("stateful-bare-trust-match-{}", std::process::id()));
        let bin_dir = temp.join("bin");
        fs::create_dir_all(&bin_dir).expect("bin dir should be created");
        let current = std::env::current_exe().expect("current executable should resolve");
        let bare = bin_dir.join("stateful");
        fs::copy(&current, &bare).expect("bare stateful should be copied");

        let path_env = std::env::join_paths([bin_dir]).expect("test PATH should join");

        assert!(is_verified_bare_stateful_executable(
            "stateful",
            &current,
            Some(path_env.as_os_str())
        ));

        fs::remove_dir_all(temp).expect("temp dir should be removable");
    }

    #[test]
    fn bare_stateful_is_denied_when_first_path_match_differs() {
        let temp = std::env::temp_dir().join(format!(
            "stateful-bare-trust-mismatch-{}",
            std::process::id()
        ));
        let bin_dir = temp.join("bin");
        fs::create_dir_all(&bin_dir).expect("bin dir should be created");
        let current = std::env::current_exe().expect("current executable should resolve");
        fs::write(
            bin_dir.join("stateful"),
            b"not the installed stateful binary",
        )
        .expect("fake stateful should be written");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut permissions = fs::metadata(bin_dir.join("stateful"))
                .expect("fake stateful metadata should read")
                .permissions();
            permissions.set_mode(0o755);
            fs::set_permissions(bin_dir.join("stateful"), permissions)
                .expect("fake stateful should be executable");
        }

        let path_env = std::env::join_paths([bin_dir]).expect("test PATH should join");

        assert!(!is_verified_bare_stateful_executable(
            "stateful",
            &current,
            Some(path_env.as_os_str())
        ));

        fs::remove_dir_all(temp).expect("temp dir should be removable");
    }
    #[test]
    fn stateful_command_policy_reminder_mentions_process_lookup_git_github_pr_and_binary_path() {
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
            reminder.contains("built-in Bash with trusted")
                && reminder.contains("sandbox run")
                && reminder.contains("sandbox process find"),
            "reminder should mention built-in Bash trusted Stateful commands: {reminder}"
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
            guidance.contains("built-in Bash with trusted")
                && guidance.contains("sandbox run")
                && guidance.contains("sandbox process find"),
            "denial guidance should mention built-in Bash trusted Stateful commands: {guidance}"
        );
        for removed_tool in ["process_find", "sandbox_bash", "ext_ro_bash", "ext_rw_bash"] {
            assert!(
                !guidance.contains(removed_tool),
                "denial guidance should not mention removed OMP generated tool {removed_tool}: {guidance}"
            );
            assert!(
                !reminder.contains(removed_tool),
                "reminder should not mention removed OMP generated tool {removed_tool}: {reminder}"
            );
        }
        assert!(
            guidance.contains(&current_exe),
            "denial guidance should show the trusted executable path: {guidance}"
        );
    }

    #[test]
    fn omp_session_start_rejects_legacy_agent_id_input() {
        let runtime = ServerRuntime::new("http://127.0.0.1:9", "secret", "workspace", 7);
        let temp = std::env::temp_dir().join(format!(
            "stateful-omp-legacy-session-id-{}",
            std::process::id()
        ));
        let repo = temp.join("repo");
        std::fs::create_dir_all(&repo).expect("repo dir should create");
        let input = serde_json::json!({
            "session_id": "old-session",
            "workspace_id": "workspace",
            "cwd": repo.display().to_string(),
        });

        let error = handle_omp_session_start_with_runtime(&input.to_string(), &runtime)
            .expect_err("legacy agent_id should not be accepted");

        assert!(
            error.to_string().contains("agent_id"),
            "legacy agent_id error should mention agent_id: {error}"
        );
        let _ = std::fs::remove_dir_all(&temp);
    }

    #[test]
    fn omp_session_start_rejects_invalid_agent_id_input() {
        let runtime = ServerRuntime::new("http://127.0.0.1:9", "secret", "workspace", 7);
        let input = serde_json::json!({
            "agent_id": "bad agent id",
            "workspace_id": "workspace",
            "cwd": "/repo",
        });

        let error = handle_omp_session_start_with_runtime(&input.to_string(), &runtime)
            .expect_err("invalid agent_id should not be accepted");

        assert!(
            error.to_string().contains("agent_id"),
            "invalid agent_id error should mention agent_id: {error}"
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
    fn testing_command_accepts_only_executed_supported_test_grammar() {
        assert_eq!(
            testing_command(&json!({ "command": "stateful sandbox run --fs build --network enabled --write-dir target --command 'cargo nextest run'" })),
            Some("cargo nextest".to_string())
        );
        assert_eq!(
            testing_command(&json!({ "command": "cargo nextest list" })),
            None
        );
        assert_eq!(
            testing_command(&json!({ "command": "stateful sandbox run --fs build --network enabled --write-dir target --command 'npm test -- --runInBand'" })),
            Some("npm test".to_string())
        );
        assert_eq!(
            testing_command(&json!({ "command": "python -m pytest tests/unit.py" })),
            Some("pytest".to_string())
        );
        assert_eq!(
            testing_command(&json!({ "command": "echo cargo test" })),
            None
        );
    }
}
