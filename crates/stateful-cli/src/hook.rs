use std::{
    collections::BTreeMap,
    fs,
    io::{self, Read, Write},
    path::{Path, PathBuf},
    process::{Command as ProcessCommand, Stdio},
    thread,
    time::Duration,
};

#[cfg(unix)]
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};

use clap::Parser;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use stateful_core::{
    ActorType, AgentIdentity, CoordinationSettings, EntryState, MutationOperation,
    ResourceObservation, ResourceResolver, SourceKind, SourceRef, WorkspaceIdentity, digest_bytes,
};
use stateful_store::{
    LeaseActivateInput, LeaseRequestState, LeaseRequestStatus, ReadCommandResult,
    ReadCompleteInput, ReadStartInput, RuntimeProcessInput, TaskCommandResult, TaskEndInput,
    TaskHeartbeatInput, TaskStartInput, WriteCompleteInput, WriteCompleteResult, WritePrepareInput,
    WritePrepareResult, WriteTerminal,
};
use time::{Duration as TimeDuration, OffsetDateTime, format_description::well_known::Rfc3339};

use crate::{
    Cli, Command as CliCommand, CommandError, CommandIdentity, GlobalPaths, HookCommand,
    HookRuntime, LeaseCommand, ReposCommand, SandboxCommand, SandboxFsProfile, ServerRuntime,
    discover_runtime_with_optional_global, get_payload, now_rfc3339, post_command,
    process_parent_pid, process_start_identity_for_pid, repo_gate, runtime::sync_parent_dir,
};

const OFFER_POLL_MILLIS: u64 = 250;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HookOutcome {
    Allow,
    AllowWithContext { message: String },
    Deny { reason: String },
}

impl HookOutcome {
    pub fn to_stdout_json(&self) -> serde_json::Result<Value> {
        Ok(match self {
            Self::Allow => json!({}),
            Self::AllowWithContext { message } => {
                json!({ "hookSpecificOutput": { "hookEventName": "PreToolUse", "additionalContext": message } })
            }
            Self::Deny { reason } => {
                json!({ "hookSpecificOutput": { "hookEventName": "PreToolUse", "permissionDecision": "deny", "permissionDecisionReason": reason } })
            }
        })
    }
    fn emits_stdout(&self) -> bool {
        !matches!(self, Self::Allow)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OmpHookOutcome {
    Allow,
    AllowWithWriteAttempt {
        attempt_id: String,
        permit_id: String,
    },
    Block {
        reason: String,
    },
}

impl OmpHookOutcome {
    fn json(&self) -> Value {
        match self {
            Self::Allow => json!({ "decision": "allow" }),
            Self::AllowWithWriteAttempt {
                attempt_id,
                permit_id,
            } => {
                json!({ "decision": "allow", "stateful": { "write_attempt": { "attempt_id": attempt_id, "permit_id": permit_id } } })
            }
            Self::Block { reason } => json!({ "decision": "block", "reason": reason }),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct OmpSessionStartOutput {
    pub decision: &'static str,
    pub task_id: String,
    pub agent_id: String,
    pub workspace_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct OwnedTaskOwner {
    pub(crate) task_id: String,
    pub(crate) agent_id: String,
    pub(crate) workspace_id: String,
    pub(crate) session_id: String,
    pub(crate) turn_id: String,
    pub(crate) pid: u32,
    pub(crate) process_start_identity: String,
    #[serde(default = "heartbeat_enabled")]
    pub(crate) heartbeat_enabled: bool,
    pub(crate) task_start_observed_at: String,
    pub(crate) task_start_input: TaskStartInput,
}

fn heartbeat_enabled() -> bool {
    true
}

#[derive(Serialize, Deserialize)]
struct CodexHeartbeatInput {
    repo_root: PathBuf,
    owner: OwnedTaskOwner,
}

#[derive(Clone)]
struct TaskContext {
    task_id: String,
    agent_id: String,
    turn_id: Option<String>,
    actor_id: String,
    owner_id: String,
    workspace: WorkspaceIdentity,
}

#[derive(Deserialize)]
struct CodexInput {
    session_id: String,
    turn_id: String,
    #[serde(default)]
    tool_name: Option<String>,
    #[serde(default)]
    tool_input: Value,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct OmpInput {
    #[serde(rename = "type", alias = "event_type")]
    event_type: String,
    runtime: String,
    version: String,
    #[serde(default)]
    cwd: Option<PathBuf>,
    session_id: String,
    leaf_agent_id: String,
    #[serde(default, alias = "task_id")]
    task_id: Option<String>,
    #[serde(default, alias = "tool_name")]
    tool_name: Option<String>,
    #[serde(default, alias = "tool_call_id")]
    tool_call_id: Option<String>,
    #[serde(default)]
    input: Value,
    #[serde(default, alias = "is_error")]
    is_error: bool,
    #[serde(default)]
    content: Value,
    #[serde(default)]
    details: Value,
    #[serde(default, alias = "attempt_id")]
    attempt_id: Option<String>,
    #[serde(default, alias = "permit_id")]
    permit_id: Option<String>,
    #[serde(default, alias = "will_continue")]
    will_continue: Option<bool>,
}

#[derive(Default, Deserialize)]
struct SettingsFile {
    heartbeat_interval_seconds: Option<u64>,
    inactivity_timeout_seconds: Option<u64>,
    lease_expiry_seconds: Option<u64>,
    offer_ttl_seconds: Option<u64>,
}

pub fn run_hook(runtime: HookRuntime) -> anyhow::Result<()> {
    match runtime {
        HookRuntime::Codex { command } => run_codex(command),
        HookRuntime::Omp { command } => run_omp(command),
    }
}

fn run_codex(command: HookCommand) -> anyhow::Result<()> {
    let input = stdin()?;
    if let HookCommand::Heartbeat = &command {
        return run_codex_heartbeat(&input);
    }
    let Some(repo) = enabled_repo(&hook_cwd(&input)?)? else {
        return Ok(());
    };
    match command {
        HookCommand::PreToolUse => {
            let outcome = handle_pre_tool_use_in_repo(&input, &repo)?;
            if outcome.emits_stdout() {
                println!("{}", serde_json::to_string(&outcome.to_stdout_json()?)?);
            }
        }
        HookCommand::UserPromptSubmit => match handle_user_prompt_submit_in_repo(&input, &repo) {
            Ok(message) => println!(
                "{}",
                json!({
                    "hookSpecificOutput": {
                        "hookEventName": "UserPromptSubmit",
                        "additionalContext": message,
                    }
                })
            ),
            Err(error) => eprintln!("stateful Codex task start denied: {error}"),
        },
        HookCommand::Stop => {
            if let Err(error) = handle_stop_in_repo(&input, &repo) {
                eprintln!("stateful Codex task finalize denied: {error}");
            }
        }
        HookCommand::SessionStart | HookCommand::PostToolUse => {}
        HookCommand::Heartbeat => unreachable!("heartbeat is handled before Codex input parsing"),
    }
    Ok(())
}

fn run_omp(command: HookCommand) -> anyhow::Result<()> {
    let input = stdin()?;
    let Some(repo) = enabled_repo(&hook_cwd(&input)?)? else {
        if matches!(command, HookCommand::PreToolUse) {
            println!("{}", json!({ "decision": "allow" }));
        }
        return Ok(());
    };
    let runtime = discover_runtime_with_optional_global(&repo)?;
    match command {
        HookCommand::SessionStart => println!(
            "{}",
            serde_json::to_string(&omp_start(&input, &runtime, &repo)?)?
        ),
        HookCommand::PreToolUse => println!("{}", omp_pre(&input, &runtime, &repo)?.json()),
        HookCommand::PostToolUse => omp_post(&input, &runtime, &repo)?,
        HookCommand::Stop => omp_stop(&input, &runtime, &repo)?,
        HookCommand::UserPromptSubmit => anyhow::bail!("OMP does not use UserPromptSubmit"),
        HookCommand::Heartbeat => anyhow::bail!("OMP does not use the Codex heartbeat helper"),
    }
    Ok(())
}

pub fn handle_pre_tool_use(input: &str) -> anyhow::Result<HookOutcome> {
    let input: CodexInput = serde_json::from_str(input)?;
    let reason = match input.tool_name.as_deref().unwrap_or("unknown") {
        "read" | "Read" => {
            "Codex native reads are wrapper_required: complete typed terminal payload is unavailable"
        }
        "apply_patch" | "write" | "edit" | "Write" | "Edit" => {
            "Codex native writes are wrapper_required: use the owned Stateful wrapper"
        }
        _ => "unknown_writer: only owned Stateful wrappers may mutate an enabled workspace",
    };
    Ok(HookOutcome::Deny {
        reason: reason.to_string(),
    })
}

pub fn handle_pre_tool_use_in_repo(
    input: &str,
    _repo_root: impl AsRef<Path>,
) -> anyhow::Result<HookOutcome> {
    let input: CodexInput = serde_json::from_str(input)?;
    if input
        .tool_name
        .as_deref()
        .is_some_and(|name| name.eq_ignore_ascii_case("bash"))
    {
        return Ok(match validate_codex_wrapper_command(&input) {
            Ok(()) => HookOutcome::Allow,
            Err(error) => HookOutcome::Deny {
                reason: format!("Codex Bash is wrapper_required: {error}"),
            },
        });
    }
    handle_pre_tool_use(&serde_json::to_string(&json!({
        "session_id": input.session_id,
        "turn_id": input.turn_id,
        "tool_name": input.tool_name,
    }))?)
}

fn validate_codex_wrapper_command(input: &CodexInput) -> anyhow::Result<()> {
    let command = input
        .tool_input
        .get("command")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow::anyhow!("Bash payload lacks a command"))?;
    let words =
        crate::shell_command::split_simple_command_words(command).map_err(anyhow::Error::msg)?;
    anyhow::ensure!(!words.is_empty(), "Bash command is empty");
    let requested = Path::new(&words[0]).canonicalize()?;
    let installed = std::env::current_exe()?.canonicalize()?;
    anyhow::ensure!(
        requested == installed,
        "use the exact installed Stateful binary `{}`",
        installed.display()
    );
    let cli = Cli::try_parse_from(words)
        .map_err(|error| anyhow::anyhow!("invalid Stateful wrapper command: {error}"))?;
    let task_id = id("codex-task", &[&input.session_id, &input.turn_id]);
    let agent_id = format!("codex:{}", input.session_id);
    match cli.command {
        CliCommand::Sandbox(SandboxCommand::Run {
            fs: SandboxFsProfile::Mutation,
            task_id: Some(command_task),
            agent_id: Some(command_agent),
            ..
        }) if command_task == task_id && command_agent == agent_id => Ok(()),
        CliCommand::Sandbox(SandboxCommand::Run {
            fs: SandboxFsProfile::ReadOnly | SandboxFsProfile::Git,
            ..
        })
        | CliCommand::Sandbox(SandboxCommand::Process { .. })
        | CliCommand::Status
        | CliCommand::Doctor
        | CliCommand::Repos(ReposCommand::List) => Ok(()),
        CliCommand::Commit {
            task_id: command_task,
            agent_id: command_agent,
            ..
        }
        | CliCommand::Lease(LeaseCommand::Release {
            task_id: command_task,
            agent_id: command_agent,
            ..
        }) if command_task == task_id && command_agent == agent_id => Ok(()),
        _ => anyhow::bail!(
            "only read-only sandbox commands and mutation, commit, or lease-release wrappers bound to this task are allowed"
        ),
    }
}

pub fn handle_omp_pre_tool_use_with_runtime(
    input: &str,
    runtime: Option<&ServerRuntime>,
    repo_root: Option<&Path>,
    _cwd: Option<&Path>,
) -> anyhow::Result<OmpHookOutcome> {
    omp_pre(
        input,
        runtime.ok_or_else(|| anyhow::anyhow!("local Stateful runtime is unavailable"))?,
        repo_root.ok_or_else(|| anyhow::anyhow!("enabled workspace is unavailable"))?,
    )
}

fn omp_pre(input: &str, runtime: &ServerRuntime, repo: &Path) -> anyhow::Result<OmpHookOutcome> {
    let input: OmpInput = serde_json::from_str(input)?;
    let task = omp_task(&input, runtime, repo)?;
    if input.event_type != "tool_call" {
        return Ok(OmpHookOutcome::Block {
            reason: "OMP pre-tool payload must be tool_call".to_string(),
        });
    }
    match input.tool_name.as_deref().unwrap_or_default() {
        "read" => {
            let invocation = invocation(&input)?;
            let resolver = ResourceResolver::new(&task.workspace.workspace_id, repo)?;
            post_command::<_, ReadCommandResult>(
                runtime,
                "/v2/reads/start",
                &identity(&task, "read.start", &invocation, Some("read")),
                &ReadStartInput {
                    read_id: id("read", &[&task.task_id, &invocation]),
                    invocation_id: invocation,
                    resources: read_observation(&resolver, &raw_path(&input.input)?)?,
                },
            )
            .map_err(command_error)?;
            Ok(OmpHookOutcome::Allow)
        }
        "write" => prepare_write(&input, runtime, repo, &task),
        tool => Ok(OmpHookOutcome::Block {
            reason: format!("unknown_writer: unsupported OMP tool `{tool}`"),
        }),
    }
}

fn prepare_write(
    input: &OmpInput,
    runtime: &ServerRuntime,
    repo: &Path,
    task: &TaskContext,
) -> anyhow::Result<OmpHookOutcome> {
    let invocation = invocation(input)?;
    let operation = write_operation(&input.input, repo)?;
    let settings = settings(repo)?;
    let resolver = ResourceResolver::new(&task.workspace.workspace_id, repo)?;
    let command = identity(task, "write.prepare", &invocation, Some("write"));
    let payload = WritePrepareInput {
        invocation_id: invocation,
        operation: operation.clone(),
        current: resolver.observe_operation(&operation)?,
        request_expires_at: after(&command.observed_at, settings.offer_ttl_seconds)?,
        lease_expires_at: after(&command.observed_at, settings.lease_expiry_seconds)?,
        attempt_deadline: after(&command.observed_at, settings.lease_expiry_seconds)?,
    };
    match post_command::<_, WritePrepareResult>(runtime, "/v2/writes/prepare", &command, &payload)
        .map_err(command_error)?
    {
        WritePrepareResult::Ready {
            attempt_id,
            permit_id,
            ..
        } => Ok(OmpHookOutcome::AllowWithWriteAttempt {
            attempt_id,
            permit_id,
        }),
        WritePrepareResult::Queued { batch_id } => wait_offer(runtime, task, &settings, &batch_id),
        WritePrepareResult::RereadRequired { .. } => Ok(OmpHookOutcome::Block {
            reason: "reread_required: full native reread is required before mutation".to_string(),
        }),
        WritePrepareResult::Denied { reason_code } => Ok(OmpHookOutcome::Block {
            reason: format!("write denied: {reason_code}"),
        }),
    }
}

fn wait_offer(
    runtime: &ServerRuntime,
    task: &TaskContext,
    settings: &CoordinationSettings,
    initial_batch: &str,
) -> anyhow::Result<OmpHookOutcome> {
    let deadline =
        OffsetDateTime::now_utc() + TimeDuration::seconds(settings.offer_ttl_seconds as i64);
    let mut batch = initial_batch.to_string();
    while OffsetDateTime::now_utc() < deadline {
        let status: LeaseRequestStatus = get_payload(
            runtime,
            &format!(
                "/v2/lease-requests/{batch}?workspace_id={}&task_id={}&now={}",
                url(&task.workspace.workspace_id),
                url(&task.task_id),
                url(&now_rfc3339())
            ),
        )
        .map_err(command_error)?;
        match status.state {
            LeaseRequestState::Queued => thread::sleep(Duration::from_millis(OFFER_POLL_MILLIS)),
            LeaseRequestState::Offered => {
                let offer_id = status
                    .offer_id
                    .ok_or_else(|| anyhow::anyhow!("offered lease lacks offer_id"))?;
                let command = identity(
                    task,
                    "lease.activate",
                    &format!(
                        "{batch}:{}:{offer_id}:{}",
                        status.version,
                        uuid::Uuid::new_v4()
                    ),
                    Some("write"),
                );
                let activated: stateful_store::LeaseActivateResult = post_command(
                    runtime,
                    "/v2/leases/activate",
                    &command,
                    &LeaseActivateInput {
                        batch_id: batch.clone(),
                        offer_id,
                        version: status.version,
                        lease_expires_at: after(
                            &command.observed_at,
                            settings.lease_expiry_seconds,
                        )?,
                    },
                )
                .map_err(command_error)?;
                if !activated.active {
                    return Ok(OmpHookOutcome::Block {
                        reason: "reread_required: offered lease remains inactive; full native reread is required before a new prepare retries activation".to_string(),
                    });
                }
                return Ok(OmpHookOutcome::Block {
                    reason: "reread_required: lease activated; full native reread and a new prepare are required before mutation".to_string(),
                });
            }
            LeaseRequestState::Superseded => {
                batch = status
                    .superseded_by
                    .ok_or_else(|| anyhow::anyhow!("superseded request lacks replacement"))?
            }
            LeaseRequestState::Activated => {
                return Ok(OmpHookOutcome::Block {
                    reason: "reread_required: activated leases cannot execute before a full reread"
                        .to_string(),
                });
            }
            LeaseRequestState::Expired | LeaseRequestState::Cancelled => {
                return Ok(OmpHookOutcome::Block {
                    reason: "write queue request is no longer active".to_string(),
                });
            }
        }
    }
    Ok(OmpHookOutcome::Block {
        reason: "offer_timeout: queued write offer did not arrive in time".to_string(),
    })
}

pub fn handle_omp_session_start_with_runtime(
    input: &str,
    runtime: &ServerRuntime,
) -> anyhow::Result<OmpSessionStartOutput> {
    let parsed: OmpInput = serde_json::from_str(input)?;
    omp_start_parsed(
        &parsed,
        runtime,
        parsed
            .cwd
            .as_deref()
            .ok_or_else(|| anyhow::anyhow!("OMP agent_start lacks cwd"))?,
    )
}

fn omp_start(
    input: &str,
    runtime: &ServerRuntime,
    repo: &Path,
) -> anyhow::Result<OmpSessionStartOutput> {
    omp_start_parsed(&serde_json::from_str(input)?, runtime, repo)
}

fn omp_start_parsed(
    input: &OmpInput,
    runtime: &ServerRuntime,
    repo: &Path,
) -> anyhow::Result<OmpSessionStartOutput> {
    anyhow::ensure!(
        input.event_type == "agent_start",
        "OMP task start requires agent_start"
    );
    let task = omp_task(input, runtime, repo)?;
    let settings = settings(repo)?;
    let command = identity(&task, "task.start", "agent_start", None);
    let result: TaskCommandResult = post_command(
        runtime,
        "/v2/tasks/start",
        &command,
        &TaskStartInput {
            next_action: "OMP leaf task active".to_string(),
            settings,
            expires_at: after(&command.observed_at, settings.inactivity_timeout_seconds)?,
            runtime_process: Some(runtime_process()?),
        },
    )
    .map_err(command_error)?;
    anyhow::ensure!(
        result.task_id == task.task_id,
        "task start returned a different task owner"
    );
    Ok(OmpSessionStartOutput {
        decision: "allow",
        task_id: task.task_id,
        agent_id: task.agent_id,
        workspace_id: task.workspace.workspace_id,
    })
}

pub fn handle_omp_post_tool_use_with_runtime(
    input: &str,
    runtime: &ServerRuntime,
) -> anyhow::Result<()> {
    let parsed: OmpInput = serde_json::from_str(input)?;
    omp_post_parsed(
        &parsed,
        runtime,
        parsed
            .cwd
            .as_deref()
            .ok_or_else(|| anyhow::anyhow!("OMP post payload lacks cwd"))?,
    )
}

fn omp_post(input: &str, runtime: &ServerRuntime, repo: &Path) -> anyhow::Result<()> {
    omp_post_parsed(&serde_json::from_str(input)?, runtime, repo)
}

fn omp_post_parsed(input: &OmpInput, runtime: &ServerRuntime, repo: &Path) -> anyhow::Result<()> {
    let task = omp_task(input, runtime, repo)?;
    match input.event_type.as_str() {
        "heartbeat" => heartbeat_task(&task, runtime, settings(repo)?, "OMP leaf task active")?,
        "tool_result" => match input.tool_name.as_deref().unwrap_or_default() {
            "read" => complete_read(input, runtime, repo, &task)?,
            "write" => complete_write(input, runtime, repo, &task)?,
            _ => {}
        },
        _ => anyhow::bail!(
            "OMP post payload has unsupported type `{}`",
            input.event_type
        ),
    }
    Ok(())
}

fn complete_read(
    input: &OmpInput,
    runtime: &ServerRuntime,
    repo: &Path,
    task: &TaskContext,
) -> anyhow::Result<()> {
    let invocation = invocation(input)?;
    let path = raw_path(&input.input)?;
    let resolver = ResourceResolver::new(&task.workspace.workspace_id, repo)?;
    let exact = exact_read(input, repo, &path);
    post_command::<_, ReadCommandResult>(
        runtime,
        "/v2/reads/complete",
        &identity(task, "read.complete", &invocation, Some("read")),
        &ReadCompleteInput {
            read_id: id("read", &[&task.task_id, &invocation]),
            invocation_id: invocation,
            resources: read_observation(&resolver, &path).unwrap_or_default(),
            terminal_success: exact,
            complete: exact,
            stable: exact,
            exact,
        },
    )
    .map_err(command_error)?;
    Ok(())
}

fn complete_write(
    input: &OmpInput,
    runtime: &ServerRuntime,
    repo: &Path,
    task: &TaskContext,
) -> anyhow::Result<()> {
    let invocation = invocation(input)?;
    let operation = write_operation(&input.input, repo)?;
    let resolver = ResourceResolver::new(&task.workspace.workspace_id, repo)?;
    let success = !input.is_error && write_target_matches(input, repo, &operation);
    let post = if success {
        observe_written(&resolver, &operation).unwrap_or_default()
    } else {
        resolver.observe_operation(&operation).unwrap_or_default()
    };
    let terminal = if success && !post.is_empty() {
        WriteTerminal::Success
    } else if input.is_error {
        WriteTerminal::FailedKnown
    } else {
        WriteTerminal::Uncertain
    };
    post_command::<_, WriteCompleteResult>(
        runtime,
        "/v2/writes/complete",
        &identity(task, "write.complete", &invocation, Some("write")),
        &WriteCompleteInput {
            attempt_id: required(
                input.attempt_id.as_deref(),
                "OMP write result lacks prepared attempt_id",
            )?
            .to_string(),
            permit_id: required(
                input.permit_id.as_deref(),
                "OMP write result lacks prepared permit_id",
            )?
            .to_string(),
            invocation_id: invocation,
            terminal,
            post_resources: post.clone(),
            expected_post_resources: if success { post } else { Vec::new() },
            error: input
                .is_error
                .then(|| "OMP reported tool failure".to_string()),
        },
    )
    .map_err(command_error)?;
    Ok(())
}

fn omp_stop(input: &str, runtime: &ServerRuntime, repo: &Path) -> anyhow::Result<()> {
    let input: OmpInput = serde_json::from_str(input)?;
    let task = omp_task(&input, runtime, repo)?;
    match input.event_type.as_str() {
        "agent_end" if input.will_continue == Some(true) => Ok(()),
        "agent_end" => end_task(runtime, &task, "/v2/tasks/finalize", "task.finalize"),
        "session_shutdown" => end_task(runtime, &task, "/v2/tasks/cancel", "task.cancel"),
        _ => anyhow::bail!(
            "OMP stop payload has unsupported type `{}`",
            input.event_type
        ),
    }
}

pub fn handle_user_prompt_submit_in_repo(
    input: &str,
    repo: impl AsRef<Path>,
) -> anyhow::Result<String> {
    let input: CodexInput = serde_json::from_str(input)?;
    let repo = repo.as_ref();
    let runtime = discover_runtime_with_optional_global(repo)?;
    cleanup_missed_codex_owners(repo, &runtime, &input.session_id, &input.turn_id)?;
    let task = codex_task(&input, &runtime, repo)?;
    let settings = settings(repo)?;
    let observed_at = now_rfc3339();
    let task_start_input = TaskStartInput {
        next_action: "Codex root turn active".to_string(),
        settings,
        expires_at: after(&observed_at, settings.inactivity_timeout_seconds)?,
        runtime_process: Some(runtime_process()?),
    };
    let owner = record_owner(repo, &task, &input, observed_at, task_start_input)?;
    let command = identity_at(
        &task,
        "task.start",
        "user_prompt_submit",
        None,
        &owner.task_start_observed_at,
    );
    let result: TaskCommandResult = match post_command(
        &runtime,
        "/v2/tasks/start",
        &command,
        &owner.task_start_input,
    ) {
        Ok(result) => result,
        Err(error) => {
            if error.reason_code == "invalid_input" {
                let _ = remove_owner(repo, &task.task_id);
            }
            return Err(command_error(error));
        }
    };
    anyhow::ensure!(
        result.task_id == task.task_id,
        "task start returned a different task owner"
    );
    if owner.heartbeat_enabled
        && let Err(error) = spawn_codex_heartbeat(repo, &owner)
    {
        let _ = deactivate_owner(repo, &task.task_id);
        if let Ok(result) =
            end_codex_task_result(&runtime, &task, "/v2/tasks/cancel", "task.cancel")
        {
            let _ = finish_codex_owner_result(repo, &task.task_id, result);
        }
        return Err(error);
    }
    let binary = std::env::current_exe()?.canonicalize()?;
    Ok(format!(
        "Stateful V2 task_id={} agent_id={}. Native Codex tools are fail-closed because their terminal payload contract is unverified. Use adapter-owned wrapper commands through Bash, beginning with the exact binary '{}'. Mutation sandbox, commit, and lease release commands must pass this task and agent identity.",
        task.task_id,
        task.agent_id,
        binary.display().to_string().replace('\'', "'\\''"),
    ))
}

pub fn handle_stop_in_repo(input: &str, repo: impl AsRef<Path>) -> anyhow::Result<()> {
    let input: CodexInput = serde_json::from_str(input)?;
    nonempty("Codex session_id", &input.session_id)?;
    nonempty("Codex turn_id", &input.turn_id)?;
    let repo = repo.as_ref();
    let task_id = id("codex-task", &[&input.session_id, &input.turn_id]);
    deactivate_owner(repo, &task_id)?;
    let runtime = discover_runtime_with_optional_global(repo)?;
    let task = codex_task(&input, &runtime, repo)?;
    finish_codex_owner(repo, &runtime, &task, &task_id).map(|_| ())
}

fn end_task(
    runtime: &ServerRuntime,
    task: &TaskContext,
    path: &str,
    event: &str,
) -> anyhow::Result<()> {
    post_command::<_, TaskCommandResult>(
        runtime,
        path,
        &identity(task, event, event, None),
        &TaskEndInput { handoff: None },
    )
    .map_err(command_error)?;
    Ok(())
}

fn end_codex_task_result(
    runtime: &ServerRuntime,
    task: &TaskContext,
    path: &str,
    event: &str,
) -> Result<TaskCommandResult, CommandError> {
    let observed_at = now_rfc3339();
    post_command(
        runtime,
        path,
        &identity_at(task, event, &observed_at, None, &observed_at),
        &TaskEndInput { handoff: None },
    )
}

fn finish_codex_owner(
    repo: &Path,
    runtime: &ServerRuntime,
    task: &TaskContext,
    task_id: &str,
) -> anyhow::Result<bool> {
    match end_codex_task_result(runtime, task, "/v2/tasks/finalize", "task.finalize") {
        Ok(result) => finish_codex_owner_result(repo, task_id, result),
        Err(error) if error.reason_code == "not_found" => {
            remove_owner(repo, task_id)?;
            Ok(true)
        }
        Err(finalize_error) => {
            match end_codex_task_result(runtime, task, "/v2/tasks/cancel", "task.cancel") {
                Ok(result) => finish_codex_owner_result(repo, task_id, result),
                Err(error) if error.reason_code == "not_found" => {
                    remove_owner(repo, task_id)?;
                    Ok(true)
                }
                Err(cancel_error) => Err(anyhow::anyhow!(
                    "could not terminally stop Codex task {task_id}: finalize failed ({finalize_error}); cancel failed ({cancel_error})"
                )),
            }
        }
    }
}

fn finish_codex_owner_result(
    repo: &Path,
    task_id: &str,
    result: TaskCommandResult,
) -> anyhow::Result<bool> {
    if result.status.is_terminal() {
        remove_owner(repo, task_id)?;
        return Ok(true);
    }
    if result.draining {
        return Ok(false);
    }
    anyhow::bail!(
        "Codex task {task_id} stop returned unexpected non-terminal status {:?}",
        result.status
    )
}

fn cleanup_missed_codex_owners(
    repo: &Path,
    runtime: &ServerRuntime,
    session_id: &str,
    current_turn_id: &str,
) -> anyhow::Result<()> {
    let entries = match fs::read_dir(owner_directory(repo)) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.into()),
    };
    for entry in entries {
        let path = entry?.path();
        if path.extension().and_then(|value| value.to_str()) != Some("json") {
            continue;
        }
        let owner: OwnedTaskOwner = serde_json::from_slice(&fs::read(&path)?)?;
        if owner.session_id != session_id || owner.turn_id == current_turn_id {
            continue;
        }
        let Some(owner) = deactivate_owner(repo, &owner.task_id)? else {
            continue;
        };
        let task = task_for_owner(&owner, runtime, repo)?;
        if !finish_codex_owner(repo, runtime, &task, &owner.task_id)? {
            anyhow::bail!(
                "previous Codex task {} is draining; refusing a new turn in session {session_id}",
                owner.task_id
            );
        }
    }
    Ok(())
}

fn task_for_owner(
    owner: &OwnedTaskOwner,
    runtime: &ServerRuntime,
    repo: &Path,
) -> anyhow::Result<TaskContext> {
    let workspace = workspace(runtime, repo)?;
    anyhow::ensure!(
        workspace.workspace_id == owner.workspace_id,
        "Codex owner workspace changed"
    );
    Ok(TaskContext {
        task_id: owner.task_id.clone(),
        agent_id: owner.agent_id.clone(),
        turn_id: Some(owner.turn_id.clone()),
        actor_id: format!("{}:{}", owner.agent_id, owner.turn_id),
        owner_id: owner.agent_id.clone(),
        workspace,
    })
}

pub(crate) fn resolve_owned_wrapper_task(repo: &Path) -> anyhow::Result<OwnedTaskOwner> {
    let ancestors = ancestors()?;
    let entries = match fs::read_dir(owner_directory(repo)) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            anyhow::bail!("owned wrapper has no active Codex task owner")
        }
        Err(error) => return Err(error.into()),
    };
    let mut found = Vec::new();
    for entry in entries {
        let path = entry?.path();
        if path.extension().and_then(|value| value.to_str()) != Some("json") {
            continue;
        }
        let owner: OwnedTaskOwner = serde_json::from_slice(&fs::read(path)?)?;
        if !owner.heartbeat_enabled {
            continue;
        }
        let Some(identity) = ancestors.get(&owner.pid) else {
            continue;
        };
        if identity != &owner.process_start_identity {
            anyhow::bail!("owned wrapper found a stale Codex task owner record")
        }
        found.push(owner);
    }
    match found.len() {
        1 => Ok(found.pop().expect("one task owner exists")),
        0 => anyhow::bail!("owned wrapper has no live Codex task owner"),
        _ => anyhow::bail!("owned wrapper found multiple live Codex task owners"),
    }
}

fn ancestors() -> anyhow::Result<BTreeMap<u32, String>> {
    let mut result = BTreeMap::new();
    let mut pid = std::process::id();
    for _ in 0..128 {
        if result
            .insert(pid, process_start_identity_for_pid(pid)?)
            .is_some()
        {
            anyhow::bail!("process ancestor cycle detected")
        }
        let Some(parent) = process_parent_pid(pid)? else {
            break;
        };
        if parent == pid {
            break;
        }
        pid = parent;
    }
    Ok(result)
}

fn record_owner(
    repo: &Path,
    task: &TaskContext,
    input: &CodexInput,
    task_start_observed_at: String,
    task_start_input: TaskStartInput,
) -> anyhow::Result<OwnedTaskOwner> {
    let target = owner_path(repo, &task.task_id);
    match fs::read(&target) {
        Ok(contents) => return matching_owner(&contents, task, input),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    let pid = process_parent_pid(std::process::id())?
        .ok_or_else(|| anyhow::anyhow!("Codex hook has no live parent process"))?;
    let owner = OwnedTaskOwner {
        task_id: task.task_id.clone(),
        agent_id: task.agent_id.clone(),
        workspace_id: task.workspace.workspace_id.clone(),
        session_id: input.session_id.clone(),
        turn_id: input.turn_id.clone(),
        pid,
        process_start_identity: process_start_identity_for_pid(pid)?,
        heartbeat_enabled: true,
        task_start_observed_at,
        task_start_input,
    };
    if create_owner(repo, &owner)? {
        return Ok(owner);
    }
    matching_owner(&fs::read(target)?, task, input)
}

fn matching_owner(
    contents: &[u8],
    task: &TaskContext,
    input: &CodexInput,
) -> anyhow::Result<OwnedTaskOwner> {
    let owner: OwnedTaskOwner = serde_json::from_slice(contents)?;
    let pid = process_parent_pid(std::process::id())?
        .ok_or_else(|| anyhow::anyhow!("Codex hook has no live parent process"))?;
    anyhow::ensure!(
        owner.task_id == task.task_id
            && owner.agent_id == task.agent_id
            && owner.workspace_id == task.workspace.workspace_id
            && owner.session_id == input.session_id
            && owner.turn_id == input.turn_id
            && owner.pid == pid
            && process_start_identity_for_pid(pid)
                .is_ok_and(|identity| identity == owner.process_start_identity),
        "existing Codex owner does not match the replayed task"
    );
    Ok(owner)
}

fn create_owner(repo: &Path, owner: &OwnedTaskOwner) -> anyhow::Result<bool> {
    let directory = owner_directory(repo);
    fs::create_dir_all(&directory)?;
    let target = owner_path(repo, &owner.task_id);
    let temporary = directory.join(format!(".{}.{}.tmp", owner.task_id, std::process::id()));
    #[cfg(unix)]
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&temporary)?;
    #[cfg(not(unix))]
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)?;
    let written = file
        .write_all(&serde_json::to_vec(owner)?)
        .and_then(|_| file.sync_all());
    drop(file);
    if let Err(error) = written {
        let _ = fs::remove_file(&temporary);
        return Err(error.into());
    }
    #[cfg(unix)]
    let created: anyhow::Result<bool> = match fs::hard_link(&temporary, &target) {
        Ok(()) => Ok(true),
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => Ok(false),
        Err(error) => Err(error.into()),
    };
    #[cfg(not(unix))]
    let created: anyhow::Result<bool> = match fs::rename(&temporary, &target) {
        Ok(()) => Ok(true),
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => Ok(false),
        Err(error) => Err(error.into()),
    };
    let _ = fs::remove_file(&temporary);
    let created = created?;
    #[cfg(unix)]
    if created {
        fs::set_permissions(target, fs::Permissions::from_mode(0o600))?;
    }
    if created {
        sync_parent_dir(&owner_path(repo, &owner.task_id))?;
    }
    Ok(created)
}

fn deactivate_owner(repo: &Path, task_id: &str) -> anyhow::Result<Option<OwnedTaskOwner>> {
    let target = owner_path(repo, task_id);
    let contents = match fs::read(&target) {
        Ok(contents) => contents,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    let mut owner: OwnedTaskOwner = serde_json::from_slice(&contents)?;
    owner.heartbeat_enabled = false;
    persist_owner(repo, &owner)?;
    Ok(Some(owner))
}

fn persist_owner(repo: &Path, owner: &OwnedTaskOwner) -> anyhow::Result<()> {
    let directory = owner_directory(repo);
    fs::create_dir_all(&directory)?;
    let target = owner_path(repo, &owner.task_id);
    let temporary = directory.join(format!(".{}.{}.tmp", owner.task_id, std::process::id()));
    #[cfg(unix)]
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&temporary)?;
    #[cfg(not(unix))]
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)?;
    let written = file
        .write_all(&serde_json::to_vec(owner)?)
        .and_then(|_| file.sync_all());
    drop(file);
    if let Err(error) = written {
        let _ = fs::remove_file(&temporary);
        return Err(error.into());
    }
    if let Err(error) = fs::rename(&temporary, &target) {
        let _ = fs::remove_file(&temporary);
        return Err(error.into());
    }
    #[cfg(unix)]
    fs::set_permissions(target, fs::Permissions::from_mode(0o600))?;
    sync_parent_dir(&owner_path(repo, &owner.task_id))?;
    Ok(())
}

fn remove_owner(repo: &Path, task_id: &str) -> anyhow::Result<()> {
    let target = owner_path(repo, task_id);
    match fs::remove_file(&target) {
        Ok(()) => sync_parent_dir(&target),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

fn owner_directory(repo: &Path) -> PathBuf {
    repo.join(".stateful_core/runtime/tasks")
}
fn owner_path(repo: &Path, task_id: &str) -> PathBuf {
    owner_directory(repo).join(format!("{task_id}.json"))
}

fn spawn_codex_heartbeat(repo: &Path, owner: &OwnedTaskOwner) -> anyhow::Result<()> {
    let input = serde_json::to_vec(&CodexHeartbeatInput {
        repo_root: repo.canonicalize()?,
        owner: owner.clone(),
    })?;
    let mut command = ProcessCommand::new(std::env::current_exe()?);
    command
        .args(["hook", "codex", "heartbeat"])
        .current_dir(repo)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    let mut child = command.spawn()?;
    child
        .stdin
        .take()
        .ok_or_else(|| anyhow::anyhow!("Codex heartbeat helper stdin is unavailable"))?
        .write_all(&input)?;
    Ok(())
}

fn run_codex_heartbeat(input: &str) -> anyhow::Result<()> {
    let input: CodexHeartbeatInput = serde_json::from_str(input)?;
    let repo = input.repo_root.canonicalize()?;
    let expected_task_id = id(
        "codex-task",
        &[&input.owner.session_id, &input.owner.turn_id],
    );
    anyhow::ensure!(
        input.owner.task_id == expected_task_id
            && input.owner.agent_id == format!("codex:{}", input.owner.session_id),
        "Codex heartbeat owner identity is invalid"
    );
    input
        .owner
        .task_start_input
        .settings
        .validate()
        .map_err(anyhow::Error::msg)?;
    let settings = input.owner.task_start_input.settings;
    let interval = Duration::from_secs(settings.heartbeat_interval_seconds);
    loop {
        if !owned_task_is_live(&repo, &input.owner)? {
            return Ok(());
        }
        let _ = heartbeat_codex_once(&repo, &input.owner, settings);
        thread::sleep(interval);
    }
}

fn owned_task_is_live(repo: &Path, expected: &OwnedTaskOwner) -> anyhow::Result<bool> {
    let contents = match fs::read_to_string(owner_path(repo, &expected.task_id)) {
        Ok(contents) => contents,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error.into()),
    };
    let current: OwnedTaskOwner = serde_json::from_str(&contents)?;
    if current != *expected {
        return Ok(false);
    }
    Ok(process_start_identity_for_pid(expected.pid)
        .is_ok_and(|identity| identity == expected.process_start_identity))
}

fn heartbeat_codex_once(
    repo: &Path,
    owner: &OwnedTaskOwner,
    settings: CoordinationSettings,
) -> anyhow::Result<()> {
    let runtime = discover_runtime_with_optional_global(repo)?;
    let input = CodexInput {
        session_id: owner.session_id.clone(),
        turn_id: owner.turn_id.clone(),
        tool_name: None,
        tool_input: Value::Null,
    };
    let task = codex_task(&input, &runtime, repo)?;
    anyhow::ensure!(
        task.task_id == owner.task_id
            && task.agent_id == owner.agent_id
            && task.workspace.workspace_id == owner.workspace_id,
        "Codex heartbeat task ownership changed"
    );
    heartbeat_task(&task, &runtime, settings, "Codex root turn active")
}

fn heartbeat_task(
    task: &TaskContext,
    runtime: &ServerRuntime,
    settings: CoordinationSettings,
    next_action: &str,
) -> anyhow::Result<()> {
    let discriminator = now_rfc3339();
    let command = identity(task, "task.heartbeat", &discriminator, None);
    post_command::<_, TaskCommandResult>(
        runtime,
        "/v2/tasks/heartbeat",
        &command,
        &TaskHeartbeatInput {
            next_action: next_action.to_string(),
            expires_at: after(&command.observed_at, settings.inactivity_timeout_seconds)?,
        },
    )
    .map_err(command_error)?;
    Ok(())
}

fn codex_task(
    input: &CodexInput,
    runtime: &ServerRuntime,
    repo: &Path,
) -> anyhow::Result<TaskContext> {
    nonempty("Codex session_id", &input.session_id)?;
    nonempty("Codex turn_id", &input.turn_id)?;
    let task_id = id("codex-task", &[&input.session_id, &input.turn_id]);
    let agent_id = format!("codex:{}", input.session_id);
    Ok(TaskContext {
        task_id,
        actor_id: format!("{agent_id}:{}", input.turn_id),
        owner_id: agent_id.clone(),
        agent_id,
        turn_id: Some(input.turn_id.clone()),
        workspace: workspace(runtime, repo)?,
    })
}

fn omp_task(input: &OmpInput, runtime: &ServerRuntime, repo: &Path) -> anyhow::Result<TaskContext> {
    anyhow::ensure!(
        input.runtime == "omp" && input.version == "17.2.3",
        "unsupported OMP runtime contract; requires 17.2.3"
    );
    nonempty("OMP sessionId", &input.session_id)?;
    nonempty("OMP leafAgentId", &input.leaf_agent_id)?;
    let task_id = id("omp-task", &[&input.session_id, &input.leaf_agent_id]);
    if let Some(task) = &input.task_id {
        anyhow::ensure!(
            task == &task_id,
            "OMP task_id does not match its session and leaf owner"
        );
    }
    let agent_id = format!("omp:{}:{}", input.session_id, input.leaf_agent_id);
    Ok(TaskContext {
        task_id,
        actor_id: agent_id.clone(),
        owner_id: input.leaf_agent_id.clone(),
        agent_id,
        turn_id: None,
        workspace: workspace(runtime, repo)?,
    })
}

fn workspace(runtime: &ServerRuntime, repo: &Path) -> anyhow::Result<WorkspaceIdentity> {
    let root = fs::canonicalize(repo)?.to_string_lossy().into_owned();
    let repo_id = id("repo", &[&root]);
    Ok(WorkspaceIdentity {
        root,
        workspace_id: runtime.workspace_id.clone(),
        repo_id: repo_id.clone(),
        worktree_id: repo_id,
        branch: "canonical".to_string(),
    })
}

fn identity(
    task: &TaskContext,
    event: &str,
    discriminator: &str,
    tool: Option<&str>,
) -> CommandIdentity {
    identity_at(task, event, discriminator, tool, &now_rfc3339())
}

fn identity_at(
    task: &TaskContext,
    event: &str,
    discriminator: &str,
    tool: Option<&str>,
    observed_at: &str,
) -> CommandIdentity {
    CommandIdentity::new(
        &task.task_id,
        id("request", &[&task.task_id, event, discriminator]),
        observed_at,
        AgentIdentity {
            agent_id: task.agent_id.clone(),
            turn_id: task.turn_id.clone(),
            actor_id: task.actor_id.clone(),
            actor_type: ActorType::Agent,
            owner_id: Some(task.owner_id.clone()),
            parent_agent_id: None,
            parent_actor_id: None,
        },
        task.workspace.clone(),
        SourceRef {
            kind: SourceKind::Hook,
            event: event.to_string(),
            tool_name: tool.map(str::to_string),
            source_ref: format!("{}:{discriminator}", task.agent_id),
        },
    )
}

fn runtime_process() -> anyhow::Result<RuntimeProcessInput> {
    let pid = process_parent_pid(std::process::id())?
        .ok_or_else(|| anyhow::anyhow!("hook has no live adapter parent process"))?;
    Ok(RuntimeProcessInput {
        pid,
        process_start_identity: process_start_identity_for_pid(pid)?,
    })
}

pub(crate) fn settings(repo: &Path) -> anyhow::Result<CoordinationSettings> {
    let configured: SettingsFile = match fs::read_to_string(repo.join(".stateful/config.yml")) {
        Ok(contents) => serde_yaml::from_str(&contents)?,
        Err(error) if error.kind() == io::ErrorKind::NotFound => SettingsFile::default(),
        Err(error) => return Err(error.into()),
    };
    let defaults = CoordinationSettings::default();
    let settings = CoordinationSettings {
        heartbeat_interval_seconds: configured
            .heartbeat_interval_seconds
            .unwrap_or(defaults.heartbeat_interval_seconds),
        inactivity_timeout_seconds: configured
            .inactivity_timeout_seconds
            .unwrap_or(defaults.inactivity_timeout_seconds),
        lease_expiry_seconds: configured
            .lease_expiry_seconds
            .unwrap_or(defaults.lease_expiry_seconds),
        offer_ttl_seconds: configured
            .offer_ttl_seconds
            .unwrap_or(defaults.offer_ttl_seconds),
    };
    settings.validate().map_err(anyhow::Error::msg)?;
    Ok(settings)
}

fn read_observation(
    resolver: &ResourceResolver,
    path: &str,
) -> anyhow::Result<Vec<ResourceObservation>> {
    match resolver.observe_existing_file(path) {
        Ok(resources) => Ok(resources),
        Err(stateful_core::ResourceError::Io { source, .. })
            if source.kind() == io::ErrorKind::NotFound =>
        {
            Ok(resolver
                .absent_entry(path)?
                .into_resources()
                .into_iter()
                .map(|resource| ResourceObservation::Entry {
                    resource,
                    observed: EntryState::Absent,
                    generation: 0,
                })
                .collect())
        }
        Err(error) => Err(error.into()),
    }
}

fn write_operation(input: &Value, repo: &Path) -> anyhow::Result<MutationOperation> {
    let path = relative(required(
        input.get("path").and_then(Value::as_str),
        "OMP write lacks typed path",
    )?)?;
    match fs::symlink_metadata(repo.join(&path)) {
        Ok(_) => Ok(MutationOperation::Update { path }),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            Ok(MutationOperation::Create { path })
        }
        Err(error) => Err(error.into()),
    }
}

fn observe_written(
    resolver: &ResourceResolver,
    operation: &MutationOperation,
) -> anyhow::Result<Vec<ResourceObservation>> {
    match operation {
        MutationOperation::Create { path } | MutationOperation::Update { path } => {
            Ok(resolver.observe_existing_file(path)?)
        }
        _ => anyhow::bail!("OMP native write does not support this operation"),
    }
}

fn write_target_matches(input: &OmpInput, repo: &Path, operation: &MutationOperation) -> bool {
    let (MutationOperation::Create { path } | MutationOperation::Update { path }) = operation
    else {
        return false;
    };
    let Some(reported) = input.details.get("resolvedPath").and_then(Value::as_str) else {
        return false;
    };
    let Ok(expected) = fs::canonicalize(repo.join(path)) else {
        return false;
    };
    fs::canonicalize(reported).ok().as_ref() == Some(&expected)
}

fn exact_read(input: &OmpInput, repo: &Path, path: &str) -> bool {
    if input.is_error
        || non_null(input.details.get("truncation"))
        || non_null(input.details.pointer("/meta/truncation"))
        || non_null(input.details.pointer("/meta/limits"))
    {
        return false;
    }
    let Some(source) = input
        .details
        .pointer("/meta/source")
        .filter(|value| value.get("type").and_then(Value::as_str) == Some("path"))
        .and_then(|value| value.get("value").and_then(Value::as_str))
    else {
        return false;
    };
    let Some([content]) = input.content.as_array().map(Vec::as_slice) else {
        return false;
    };
    let Some(text) = content
        .get("text")
        .filter(|_| content.get("type").and_then(Value::as_str) == Some("text"))
        .and_then(Value::as_str)
    else {
        return false;
    };
    if input.details.get("fileSize").and_then(Value::as_u64) != Some(text.len() as u64)
        || input
            .details
            .pointer("/displayContent/text")
            .and_then(Value::as_str)
            != Some(text)
    {
        return false;
    }
    let Ok(expected) = fs::canonicalize(repo.join(path)) else {
        return false;
    };
    if fs::canonicalize(source).ok().as_ref() != Some(&expected) {
        return false;
    }
    fs::read(expected)
        .ok()
        .and_then(|bytes| String::from_utf8(bytes).ok())
        .as_deref()
        == Some(text)
}

fn raw_path(input: &Value) -> anyhow::Result<String> {
    anyhow::ensure!(
        input.get("offset").is_none() && input.get("limit").is_none(),
        "OMP read evidence requires no selector"
    );
    relative(
        required(
            input.get("path").and_then(Value::as_str),
            "OMP read lacks typed path",
        )?
        .strip_suffix(":raw")
        .ok_or_else(|| anyhow::anyhow!("OMP read evidence requires unrestricted :raw"))?,
    )
}

fn relative(path: &str) -> anyhow::Result<String> {
    anyhow::ensure!(
        !path.is_empty() && !Path::new(path).is_absolute(),
        "target path must be nonempty and relative"
    );
    anyhow::ensure!(
        !path.contains('\\')
            && path
                .split('/')
                .all(|segment| !segment.is_empty() && segment != "." && segment != ".."),
        "target path must be a normalized relative path"
    );
    Ok(path.to_string())
}

fn invocation(input: &OmpInput) -> anyhow::Result<String> {
    Ok(required(
        input.tool_call_id.as_deref(),
        "OMP tool payload lacks toolCallId",
    )?
    .to_string())
}

fn id(namespace: &str, fields: &[&str]) -> String {
    let mut bytes = namespace.as_bytes().to_vec();
    for field in fields {
        bytes.push(0);
        bytes.extend_from_slice(field.as_bytes());
    }
    format!("{namespace}-{}", digest_bytes(&bytes).value)
}

fn after(observed_at: &str, seconds: u64) -> anyhow::Result<String> {
    let observed_at = OffsetDateTime::parse(observed_at, &Rfc3339)?;
    let seconds = i64::try_from(seconds)
        .map_err(|_| anyhow::anyhow!("coordination duration is too large"))?;
    Ok((observed_at + TimeDuration::seconds(seconds)).format(&Rfc3339)?)
}
fn required<'a>(value: Option<&'a str>, message: &str) -> anyhow::Result<&'a str> {
    let value = value.ok_or_else(|| anyhow::anyhow!("{message}"))?;
    nonempty(message, value)?;
    Ok(value)
}
fn nonempty(label: &str, value: &str) -> anyhow::Result<()> {
    anyhow::ensure!(!value.trim().is_empty(), "{label}");
    Ok(())
}
fn non_null(value: Option<&Value>) -> bool {
    value.is_some_and(|value| !value.is_null())
}

fn hook_cwd(input: &str) -> anyhow::Result<PathBuf> {
    let value: Value = serde_json::from_str(input)?;
    Ok(value
        .get("cwd")
        .and_then(Value::as_str)
        .map(PathBuf::from)
        .unwrap_or(std::env::current_dir()?))
}
fn enabled_repo(start: &Path) -> anyhow::Result<Option<PathBuf>> {
    match repo_gate(&GlobalPaths::from_env()?, start)? {
        crate::RepoGate::Enabled { repo_root } => Ok(Some(repo_root)),
        crate::RepoGate::Disabled | crate::RepoGate::OutsideGitRepo => Ok(None),
    }
}
fn stdin() -> anyhow::Result<String> {
    let mut input = String::new();
    io::stdin().read_to_string(&mut input)?;
    Ok(input)
}
fn command_error(error: CommandError) -> anyhow::Error {
    anyhow::anyhow!("{}: {}", error.reason_code, error.message)
}
fn url(value: &str) -> String {
    value
        .bytes()
        .flat_map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                vec![char::from(byte).to_string()]
            }
            _ => vec![format!("%{byte:02X}")],
        })
        .collect()
}
