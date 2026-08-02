use clap::ValueEnum;
use serde_json::Value;
use stateful_core::{
    ActorType, AgentIdentity, MutationOperation, ResourceObservation, ResourceResolver, SourceKind,
    SourceRef, WorkspaceIdentity, validate_operation_transition,
};
use stateful_store::{
    LeaseActivateInput, LeaseRequestState, LeaseRequestStatus, ReadCompleteInput, ReadStartInput,
    WriteCompleteInput, WritePrepareInput, WritePrepareResult, WriteTerminal,
};
use std::{
    collections::BTreeSet,
    ffi::OsString,
    fs, io,
    io::{Read, Write},
    path::{Path, PathBuf},
    process::{Command, ExitStatus, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

#[cfg(unix)]
use signal_hook::{
    consts::signal::{SIGHUP, SIGINT, SIGTERM},
    iterator::{Handle as SignalsHandle, Signals},
};
#[cfg(unix)]
use std::os::unix::{fs::MetadataExt, process::CommandExt};

use crate::{
    CommandIdentity, GlobalPaths, RepoGate, ServerRuntime, discover_runtime_with_global,
    get_payload,
    hook::settings,
    now_rfc3339, post_command, repo_gate, repo_identity_for_enabled_repo,
    shell_command::{first_word_is_env_assignment, split_simple_command_words},
    validate_agent_id,
};
mod parse;
mod process_find;

// Temporary re-export for CLI sequence plumbing; keep the lint narrow.
pub(crate) use parse::resolve_sandbox_run_command;
use process_find::process_comm_basename;
pub use process_find::run_sandbox_process_find;

pub(crate) const STATEFUL_SANDBOX_RUN_ACTIVE_ENV: &str = "STATEFUL_SANDBOX_RUN_ACTIVE";
pub(crate) const STATEFUL_ALLOW_NESTED_SANDBOX_RUN_ENV: &str = "STATEFUL_ALLOW_NESTED_SANDBOX_RUN";
const DEFAULT_SANDBOX_RUN_TIMEOUT_SECONDS: u64 = 3600;
#[cfg(unix)]
const SIGKILL: i32 = 9;

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize, serde::Serialize, ValueEnum)]
pub enum SandboxFsProfile {
    ReadOnly,
    Mutation,
    Git,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize, serde::Serialize, ValueEnum)]
pub enum SandboxNetworkPolicy {
    Disabled,
    Enabled,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SandboxRunRequest {
    pub fs: SandboxFsProfile,
    pub network: SandboxNetworkPolicy,
    pub purpose: Option<String>,
    pub task_id: Option<String>,
    pub agent_id: Option<String>,
    pub workspace_id: Option<String>,
    /// JSON-serialized `MutationOperation` values. Only this typed surface may mutate a workspace.
    pub operations: Vec<String>,
    pub command: String,
    pub timeout_seconds: Option<u64>,
    pub stream_events: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SandboxProcessFindRequest {
    pub names: Vec<String>,
    pub contains: Vec<String>,
    pub pids: Vec<u32>,
    pub parent_pids: Vec<u32>,
    pub process_groups: Vec<u32>,
    pub fields: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ValidatedSandboxDirectCommand {
    Git(Vec<String>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ValidatedSandboxRunShape {
    pub(crate) operations: Vec<MutationOperation>,
    pub(crate) direct_command: Option<ValidatedSandboxDirectCommand>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct SandboxRunOutput {
    pub status: &'static str,
    pub exit_code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct SandboxProcessFindOutput {
    pub status: &'static str,
    pub processes: Vec<Value>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct SandboxProcessInfo {
    pub pid: u32,
    pub ppid: u32,
    pub pgid: u32,
    pub user: String,
    pub uid: i32,
    pub stat: String,
    pub start: String,
    pub etime: String,
    pub time: String,
    pub pcpu: String,
    pub pmem: String,
    pub rss: u64,
    pub vsz: u64,
    pub nice: i32,
    pub pri: i32,
    pub tty: String,
    pub comm: String,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct SandboxCommandResult {
    pub status: &'static str,
    pub exit_code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SandboxWritablePath {
    pub(crate) path: PathBuf,
    pub(crate) kind: SandboxWritablePathKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct GitProfileIdentity {
    name: String,
    email: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
struct GitProfileConfig {
    identity: Option<GitProfileIdentity>,
    credential_helpers: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SandboxWritablePathKind {
    File,
    Directory,
}

impl SandboxWritablePath {
    pub(crate) fn file(path: PathBuf) -> Self {
        Self {
            path,
            kind: SandboxWritablePathKind::File,
        }
    }
    pub(crate) fn directory(path: PathBuf) -> Self {
        Self {
            path,
            kind: SandboxWritablePathKind::Directory,
        }
    }
}

pub(crate) fn sandbox_run_cli_exit_code(output: &SandboxRunOutput) -> Option<i32> {
    match (output.status, output.exit_code) {
        ("exited", Some(0)) => None,
        ("exited", Some(code)) => Some(code),
        ("exited", None) => Some(1),
        _ => Some(1),
    }
}

fn sandbox_run_timeout_duration(timeout_seconds: Option<u64>) -> Duration {
    Duration::from_secs(
        timeout_seconds
            .unwrap_or(DEFAULT_SANDBOX_RUN_TIMEOUT_SECONDS)
            .max(1),
    )
}

pub fn run_sandbox_in_repo(
    repo_root: &Path,
    paths: &GlobalPaths,
    request: SandboxRunRequest,
) -> anyhow::Result<SandboxRunOutput> {
    let repo_root = match repo_gate(paths, repo_root)? {
        RepoGate::Enabled { repo_root } => repo_root,
        RepoGate::Disabled => anyhow::bail!("stateful sandbox run requires an enabled repo"),
        RepoGate::OutsideGitRepo => anyhow::bail!("stateful sandbox run requires a Git repo"),
    };
    let shape = validate_sandbox_run_request_shape(&request)?;
    let timeout = sandbox_run_timeout_duration(request.timeout_seconds);
    let result = match request.fs {
        SandboxFsProfile::ReadOnly => run_sandboxed_command(
            &request.command,
            &resolve_sandbox_cwd(&repo_root)?,
            &[],
            &[],
            false,
            false,
            request.network,
            timeout,
            request.stream_events,
            true,
        )?,
        SandboxFsProfile::Git => {
            let Some(ValidatedSandboxDirectCommand::Git(words)) = shape.direct_command else {
                unreachable!("git request shape always contains a git command")
            };
            let temporary = PrivateStage::new(&repo_root)?;
            run_sandboxed_git_command(
                &words,
                &resolve_sandbox_cwd(&repo_root)?,
                &[SandboxWritablePath::directory(temporary.root.clone())],
                &temporary.root,
                &temporary.root.join("hooks-disabled"),
                request.network,
                timeout,
                request.stream_events,
            )?
        }
        SandboxFsProfile::Mutation => {
            return run_staged_mutations(&repo_root, paths, &request, shape.operations, timeout);
        }
    };
    Ok(SandboxRunOutput {
        status: result.status,
        exit_code: result.exit_code,
        stdout: result.stdout,
        stderr: result.stderr,
    })
}

pub(crate) fn validate_sandbox_run_request_shape(
    request: &SandboxRunRequest,
) -> anyhow::Result<ValidatedSandboxRunShape> {
    if request.command.trim().is_empty() {
        anyhow::bail!("stateful sandbox run requires a non-empty --command");
    }
    validate_sandbox_run_process_inspection(&request.command)?;
    let operations = parse_mutation_operations(&request.operations)?;
    let direct_command = match request.fs {
        SandboxFsProfile::ReadOnly => {
            if !operations.is_empty() {
                anyhow::bail!("read-only sandbox does not accept --operation");
            }
            if request.network != SandboxNetworkPolicy::Disabled {
                anyhow::bail!("read-only sandbox run requires --network disabled");
            }
            None
        }
        SandboxFsProfile::Git => {
            if !operations.is_empty() || request.network == SandboxNetworkPolicy::Enabled {
                anyhow::bail!(
                    "git sandbox permits only read-only git commands with disabled network"
                );
            }
            Some(ValidatedSandboxDirectCommand::Git(
                validate_git_profile_command(&request.command)?,
            ))
        }
        SandboxFsProfile::Mutation => {
            if operations.is_empty() {
                anyhow::bail!("mutation sandbox requires at least one --operation");
            }
            if operations.len() != 1 {
                anyhow::bail!("mutation sandbox accepts exactly one --operation per command");
            }
            if request.task_id.as_deref().is_none_or(str::is_empty) {
                anyhow::bail!("mutation sandbox requires --task-id");
            }
            if request.agent_id.as_deref().is_none_or(str::is_empty) {
                anyhow::bail!("mutation sandbox requires --agent-id");
            }
            if request.network == SandboxNetworkPolicy::Enabled
                && request.purpose.as_deref().is_none_or(str::is_empty)
            {
                anyhow::bail!("networked mutation sandbox requires --purpose");
            }
            None
        }
    };
    Ok(ValidatedSandboxRunShape {
        operations,
        direct_command,
    })
}

fn parse_mutation_operations(values: &[String]) -> anyhow::Result<Vec<MutationOperation>> {
    let mut operations = Vec::with_capacity(values.len());
    let mut targets = BTreeSet::new();
    for value in values {
        let mut operation: MutationOperation = serde_json::from_str(value).map_err(|error| {
            anyhow::anyhow!("--operation must be a typed MutationOperation JSON object: {error}")
        })?;
        normalize_mutation_operation(&mut operation)?;
        for path in operation_paths(&operation) {
            if !targets.insert(path.to_string()) {
                anyhow::bail!("sandbox operations must not declare target `{path}` more than once");
            }
        }
        operations.push(operation);
    }
    Ok(operations)
}

fn normalize_mutation_operation(operation: &mut MutationOperation) -> anyhow::Result<()> {
    let normalize = |path: &mut String| -> anyhow::Result<()> {
        *path = normalize_sandbox_target_path("operation path", path)?;
        Ok(())
    };
    match operation {
        MutationOperation::Create { path }
        | MutationOperation::Update { path }
        | MutationOperation::Mkdir { path }
        | MutationOperation::Rmdir { path }
        | MutationOperation::WriteDirectory { path } => normalize(path),
        MutationOperation::Delete { path, entry_only } => {
            if *entry_only {
                anyhow::bail!("sandbox does not execute symlink-entry deletes");
            }
            normalize(path)
        }
        MutationOperation::Rename {
            old_path,
            new_path,
            entry_only,
        }
        | MutationOperation::Move {
            old_path,
            new_path,
            entry_only,
        } => {
            if *entry_only {
                anyhow::bail!("sandbox does not execute symlink-entry moves");
            }
            normalize(old_path)?;
            normalize(new_path)?;
            if old_path == new_path {
                anyhow::bail!("sandbox move source and destination must differ");
            }
            Ok(())
        }
        MutationOperation::Hardlink { old_path, new_path } => {
            normalize(old_path)?;
            normalize(new_path)?;
            if old_path == new_path {
                anyhow::bail!("sandbox hardlink source and destination must differ");
            }
            Ok(())
        }
        _ => anyhow::bail!(
            "sandbox does not execute operation `{}`",
            operation.kind_name()
        ),
    }
}

fn operation_paths(operation: &MutationOperation) -> Vec<&str> {
    match operation {
        MutationOperation::Create { path }
        | MutationOperation::Update { path }
        | MutationOperation::Mkdir { path }
        | MutationOperation::Rmdir { path }
        | MutationOperation::WriteDirectory { path }
        | MutationOperation::Delete { path, .. } => vec![path],
        MutationOperation::Rename {
            old_path, new_path, ..
        }
        | MutationOperation::Move {
            old_path, new_path, ..
        }
        | MutationOperation::Hardlink { old_path, new_path } => vec![old_path, new_path],
        _ => Vec::new(),
    }
}

fn run_staged_mutations(
    repo_root: &Path,
    paths: &GlobalPaths,
    request: &SandboxRunRequest,
    operations: Vec<MutationOperation>,
    timeout: Duration,
) -> anyhow::Result<SandboxRunOutput> {
    let owner = crate::hook::resolve_owned_wrapper_task(repo_root)?;
    let task_id = request.task_id.as_deref().expect("validated task id");
    let agent_id = request.agent_id.as_deref().expect("validated agent id");
    if owner.task_id != task_id || owner.agent_id != agent_id {
        anyhow::bail!("mutation sandbox task and agent must match its live hook-owned ancestor");
    }
    validate_agent_id(agent_id, "agent_id")?;
    let runtime = discover_runtime_with_global(repo_root, paths)?;
    let workspace =
        sandbox_workspace_identity(repo_root, paths, &runtime, request.workspace_id.as_deref())?;
    let mut output = SandboxRunOutput {
        status: "exited",
        exit_code: Some(0),
        stdout: String::new(),
        stderr: String::new(),
    };
    for operation in operations {
        let result = run_staged_mutation(
            repo_root, &runtime, &workspace, task_id, agent_id, &operation, request, timeout,
        )?;
        output.stdout.push_str(&result.stdout);
        output.stderr.push_str(&result.stderr);
        if result.status != "exited" || result.exit_code != Some(0) {
            output.status = result.status;
            output.exit_code = result.exit_code;
            break;
        }
    }
    Ok(output)
}

#[expect(
    clippy::too_many_arguments,
    reason = "the write permit owns explicit staging boundaries"
)]
fn run_staged_mutation(
    repo_root: &Path,
    runtime: &ServerRuntime,
    workspace: &WorkspaceIdentity,
    task_id: &str,
    agent_id: &str,
    operation: &MutationOperation,
    request: &SandboxRunRequest,
    timeout: Duration,
) -> anyhow::Result<SandboxCommandResult> {
    let resolver = ResourceResolver::new(&workspace.workspace_id, repo_root)?;
    let pre_resources = resolver.observe_operation(operation)?;
    let invocation_id = uuid::Uuid::new_v4().to_string();
    let WritePrepareResult::Ready {
        attempt_id,
        permit_id,
        ..
    } = prepare_until_ready(
        runtime,
        workspace,
        task_id,
        agent_id,
        operation,
        &invocation_id,
        &pre_resources,
        timeout,
    )?
    else {
        anyhow::bail!("write prepare did not return an executable permit");
    };

    let execution = (|| -> anyhow::Result<SandboxCommandResult> {
        let stage = PrivateStage::new(repo_root)?;
        stage.populate(operation)?;
        let result = run_sandboxed_command(
            &request.command,
            &stage.root,
            &[SandboxWritablePath::directory(stage.root.clone())],
            &[],
            false,
            false,
            request.network,
            timeout,
            request.stream_events,
            true,
        )?;
        if result.status != "exited" || result.exit_code != Some(0) {
            return Ok(result);
        }
        stage.validate(operation)?;
        if resolver.observe_operation(operation)? != pre_resources {
            anyhow::bail!("workspace changed after write permit was issued");
        }
        stage.apply(operation)?;
        let post = observe_expected_post(&resolver, operation)?;
        validate_operation_transition(operation, &pre_resources, &post, &post)?;
        Ok(result)
    })();

    let (result, terminal, post_resources, expected_post_resources, error) = match execution {
        Ok(result) if result.status == "exited" && result.exit_code == Some(0) => {
            match observe_expected_post(&resolver, operation) {
                Ok(post) => (result, WriteTerminal::Success, post.clone(), post, None),
                Err(error) => (
                    SandboxCommandResult {
                        status: "failed",
                        exit_code: Some(1),
                        stdout: result.stdout,
                        stderr: format!(
                            "{}; post-write state is uncertain: {error}",
                            result.stderr
                        ),
                    },
                    WriteTerminal::Uncertain,
                    pre_resources.clone(),
                    Vec::new(),
                    Some(error.to_string()),
                ),
            }
        }
        Ok(result) => (
            result,
            WriteTerminal::FailedKnown,
            resolver
                .observe_operation(operation)
                .unwrap_or_else(|_| pre_resources.clone()),
            Vec::new(),
            Some("sandbox child failed; private staging was discarded".to_string()),
        ),
        Err(error) => {
            let post = resolver
                .observe_operation(operation)
                .unwrap_or_else(|_| pre_resources.clone());
            let terminal = if post == pre_resources {
                WriteTerminal::FailedKnown
            } else {
                WriteTerminal::Uncertain
            };
            (
                SandboxCommandResult {
                    status: "failed",
                    exit_code: Some(1),
                    stdout: String::new(),
                    stderr: error.to_string(),
                },
                terminal,
                post,
                Vec::new(),
                Some(error.to_string()),
            )
        }
    };
    let complete = WriteCompleteInput {
        invocation_id,
        attempt_id,
        permit_id,
        terminal,
        post_resources,
        expected_post_resources,
        error,
    };
    let _: stateful_store::WriteCompleteResult = post_command(
        runtime,
        "/v2/writes/complete",
        &sandbox_identity(
            task_id,
            agent_id,
            workspace.clone(),
            "sandbox.write.complete",
        ),
        &complete,
    )?;
    Ok(result)
}

#[expect(
    clippy::too_many_arguments,
    reason = "each prepare retry needs a complete command identity"
)]
fn prepare_until_ready(
    runtime: &ServerRuntime,
    workspace: &WorkspaceIdentity,
    task_id: &str,
    agent_id: &str,
    operation: &MutationOperation,
    invocation_id: &str,
    current: &[ResourceObservation],
    timeout: Duration,
) -> anyhow::Result<WritePrepareResult> {
    let started = Instant::now();
    let settings = settings(Path::new(&workspace.root))?;
    let offer_timeout = Duration::from_secs(settings.offer_ttl_seconds);
    let mut current = current.to_vec();
    record_exact_read(runtime, workspace, task_id, agent_id, &current)?;
    loop {
        let command = sandbox_identity(
            task_id,
            agent_id,
            workspace.clone(),
            "sandbox.write.prepare",
        );
        let prepared: WritePrepareResult = post_command(
            runtime,
            "/v2/writes/prepare",
            &command,
            &WritePrepareInput {
                invocation_id: invocation_id.to_string(),
                operation: operation.clone(),
                current: current.clone(),
                request_expires_at: timestamp_after(
                    &command.observed_at,
                    settings.offer_ttl_seconds,
                )?,
                lease_expires_at: timestamp_after(
                    &command.observed_at,
                    settings.lease_expiry_seconds,
                )?,
                attempt_deadline: timestamp_after(&command.observed_at, timeout.as_secs())?,
            },
        )?;
        match prepared {
            ready @ WritePrepareResult::Ready { .. }
            | ready @ WritePrepareResult::Denied { .. } => return Ok(ready),
            WritePrepareResult::RereadRequired { .. } => {
                current = ResourceResolver::new(&workspace.workspace_id, &workspace.root)?
                    .observe_operation(operation)?;
                record_exact_read(runtime, workspace, task_id, agent_id, &current)?;
            }
            WritePrepareResult::Queued { batch_id } => {
                let (batch_id, status) = wait_for_offer(
                    runtime,
                    workspace,
                    task_id,
                    &batch_id,
                    started,
                    offer_timeout,
                )?;
                current = ResourceResolver::new(&workspace.workspace_id, &workspace.root)?
                    .observe_operation(operation)?;
                record_exact_read(runtime, workspace, task_id, agent_id, &current)?;
                if status.state == LeaseRequestState::Offered {
                    let offer_id = status
                        .offer_id
                        .ok_or_else(|| anyhow::anyhow!("offered lease request has no offer id"))?;
                    let command = sandbox_identity(
                        task_id,
                        agent_id,
                        workspace.clone(),
                        "sandbox.lease.activate",
                    );
                    let activated: stateful_store::LeaseActivateResult = post_command(
                        runtime,
                        "/v2/leases/activate",
                        &command,
                        &LeaseActivateInput {
                            batch_id,
                            offer_id,
                            version: status.version,
                            lease_expires_at: timestamp_after(
                                &command.observed_at,
                                settings.lease_expiry_seconds,
                            )?,
                        },
                    )?;
                    anyhow::ensure!(activated.active, "sandbox lease activation was rejected");
                }
            }
        }
    }
}
fn record_exact_read(
    runtime: &ServerRuntime,
    workspace: &WorkspaceIdentity,
    task_id: &str,
    agent_id: &str,
    resources: &[ResourceObservation],
) -> anyhow::Result<()> {
    let read_id = uuid::Uuid::new_v4().to_string();
    let invocation_id = uuid::Uuid::new_v4().to_string();
    let _: stateful_store::ReadCommandResult = post_command(
        runtime,
        "/v2/reads/start",
        &sandbox_identity(task_id, agent_id, workspace.clone(), "sandbox.read.start"),
        &ReadStartInput {
            read_id: read_id.clone(),
            invocation_id: invocation_id.clone(),
            resources: resources.to_vec(),
        },
    )?;
    let _: stateful_store::ReadCommandResult = post_command(
        runtime,
        "/v2/reads/complete",
        &sandbox_identity(
            task_id,
            agent_id,
            workspace.clone(),
            "sandbox.read.complete",
        ),
        &ReadCompleteInput {
            read_id,
            invocation_id,
            resources: resources.to_vec(),
            terminal_success: true,
            complete: true,
            stable: true,
            exact: true,
        },
    )?;
    Ok(())
}

fn wait_for_offer(
    runtime: &ServerRuntime,
    workspace: &WorkspaceIdentity,
    task_id: &str,
    initial_batch_id: &str,
    started: Instant,
    timeout: Duration,
) -> anyhow::Result<(String, LeaseRequestStatus)> {
    let mut batch_id = initial_batch_id.to_string();
    while started.elapsed() < timeout {
        let status: LeaseRequestStatus = get_payload(
            runtime,
            &format!(
                "/v2/lease-requests/{batch_id}?workspace_id={}&task_id={task_id}&now={}",
                workspace.workspace_id,
                now_rfc3339()
            ),
        )?;
        match status.state {
            LeaseRequestState::Offered => return Ok((batch_id, status)),
            LeaseRequestState::Superseded => {
                batch_id = status.superseded_by.ok_or_else(|| {
                    anyhow::anyhow!("superseded lease request has no replacement")
                })?;
            }
            LeaseRequestState::Queued => thread::sleep(Duration::from_millis(50)),
            LeaseRequestState::Activated => return Ok((batch_id, status)),
            LeaseRequestState::Cancelled | LeaseRequestState::Expired => {
                anyhow::bail!("sandbox write lease request is no longer active");
            }
        }
    }
    anyhow::bail!("sandbox write lease offer timed out")
}

fn observe_expected_post(
    resolver: &ResourceResolver,
    operation: &MutationOperation,
) -> anyhow::Result<Vec<ResourceObservation>> {
    match operation {
        MutationOperation::Create { path } | MutationOperation::Update { path } => {
            Ok(resolver.observe_operation(&MutationOperation::Update { path: path.clone() })?)
        }
        MutationOperation::Delete { path, .. } | MutationOperation::Rmdir { path } => {
            Ok(resolver.observe_operation(&MutationOperation::Create { path: path.clone() })?)
        }
        MutationOperation::Mkdir { path } => {
            Ok(resolver.observe_operation(&MutationOperation::Rmdir { path: path.clone() })?)
        }
        MutationOperation::Rename {
            old_path, new_path, ..
        }
        | MutationOperation::Move {
            old_path, new_path, ..
        } => {
            let mut post = resolver.observe_operation(&MutationOperation::Update {
                path: new_path.clone(),
            })?;
            post.extend(resolver.observe_operation(&MutationOperation::Create {
                path: old_path.clone(),
            })?);
            Ok(post)
        }
        MutationOperation::Hardlink { old_path, new_path } => {
            let mut post = resolver.observe_operation(&MutationOperation::Update {
                path: old_path.clone(),
            })?;
            post.extend(resolver.observe_operation(&MutationOperation::Update {
                path: new_path.clone(),
            })?);
            Ok(post)
        }
        MutationOperation::WriteDirectory { path } => Ok(resolver
            .observe_operation(&MutationOperation::WriteDirectory { path: path.clone() })?),
        _ => anyhow::bail!(
            "sandbox does not execute operation `{}`",
            operation.kind_name()
        ),
    }
}

fn sandbox_workspace_identity(
    repo_root: &Path,
    paths: &GlobalPaths,
    runtime: &ServerRuntime,
    workspace_id: Option<&str>,
) -> anyhow::Result<WorkspaceIdentity> {
    let repo = repo_identity_for_enabled_repo(paths, repo_root)?;
    Ok(WorkspaceIdentity {
        root: repo.root,
        workspace_id: workspace_id.unwrap_or(&runtime.workspace_id).to_string(),
        repo_id: repo.repo_id,
        worktree_id: repo.worktree_id,
        branch: repo.branch,
    })
}

fn sandbox_identity(
    task_id: &str,
    agent_id: &str,
    workspace: WorkspaceIdentity,
    event: &str,
) -> CommandIdentity {
    CommandIdentity::new(
        task_id.to_string(),
        uuid::Uuid::new_v4().to_string(),
        now_rfc3339(),
        AgentIdentity {
            agent_id: agent_id.to_string(),
            turn_id: None,
            actor_id: agent_id.to_string(),
            actor_type: ActorType::Agent,
            owner_id: None,
            parent_agent_id: None,
            parent_actor_id: None,
        },
        workspace,
        SourceRef {
            kind: SourceKind::Cli,
            event: event.to_string(),
            tool_name: Some("sandbox".to_string()),
            source_ref: event.to_string(),
        },
    )
}

fn timestamp_after(observed_at: &str, seconds: u64) -> anyhow::Result<String> {
    use time::{Duration as TimeDuration, OffsetDateTime, format_description::well_known::Rfc3339};
    let observed_at = OffsetDateTime::parse(observed_at, &Rfc3339)?;
    let seconds = i64::try_from(seconds)
        .map_err(|_| anyhow::anyhow!("sandbox command duration is too large"))?;
    Ok((observed_at + TimeDuration::seconds(seconds)).format(&Rfc3339)?)
}

struct PrivateStage {
    repo_root: PathBuf,
    root: PathBuf,
}

impl PrivateStage {
    fn new(repo_root: &Path) -> anyhow::Result<Self> {
        let private_root = repo_root.join(".stateful").join("sandbox-staging");
        ensure_private_directory(repo_root, &private_root)?;
        let root = private_root.join(uuid::Uuid::new_v4().to_string());
        fs::create_dir(&root)?;
        Ok(Self {
            repo_root: repo_root.to_path_buf(),
            root,
        })
    }

    fn populate(&self, operation: &MutationOperation) -> anyhow::Result<()> {
        self.validate_workspace_paths(operation)?;
        match operation {
            MutationOperation::Update { path }
            | MutationOperation::Delete { path, .. }
            | MutationOperation::Rmdir { path }
            | MutationOperation::WriteDirectory { path } => self.copy_existing(path),
            MutationOperation::Create { path } | MutationOperation::Mkdir { path } => {
                self.make_parent(path)
            }
            MutationOperation::Rename {
                old_path, new_path, ..
            }
            | MutationOperation::Move {
                old_path, new_path, ..
            }
            | MutationOperation::Hardlink { old_path, new_path } => {
                self.copy_existing(old_path)?;
                self.make_parent(new_path)
            }
            _ => anyhow::bail!("unsupported staged operation `{}`", operation.kind_name()),
        }
    }

    fn copy_existing(&self, path: &str) -> anyhow::Result<()> {
        let source = self.workspace(path);
        let metadata = fs::symlink_metadata(&source)?;
        if metadata.file_type().is_symlink() {
            anyhow::bail!("sandbox staging refuses symlink target `{path}`");
        }
        self.make_parent(path)?;
        let destination = self.staged(path);
        if metadata.is_file() {
            fs::copy(source, destination)?;
        } else if metadata.is_dir() {
            copy_directory_tree(&source, &destination)?;
        } else {
            anyhow::bail!("sandbox staging supports only regular files and directories");
        }
        Ok(())
    }

    fn make_parent(&self, path: &str) -> anyhow::Result<()> {
        let parent = self
            .staged(path)
            .parent()
            .expect("normalized relative target has a parent")
            .to_path_buf();
        fs::create_dir_all(parent)?;
        Ok(())
    }

    fn validate(&self, operation: &MutationOperation) -> anyhow::Result<()> {
        match operation {
            MutationOperation::Create { path } | MutationOperation::Update { path } => {
                require_regular_file(&self.staged(path), path)
            }
            MutationOperation::Delete { path, .. } | MutationOperation::Rmdir { path } => {
                require_absent(&self.staged(path), path)
            }
            MutationOperation::Mkdir { path } => require_empty_directory(&self.staged(path), path),
            MutationOperation::Rename {
                old_path, new_path, ..
            }
            | MutationOperation::Move {
                old_path, new_path, ..
            } => {
                require_absent(&self.staged(old_path), old_path)?;
                require_regular_file(&self.staged(new_path), new_path)
            }
            MutationOperation::Hardlink { old_path, new_path } => require_same_regular_file(
                &self.staged(old_path),
                &self.staged(new_path),
                old_path,
                new_path,
            ),
            MutationOperation::WriteDirectory { path } => {
                validate_directory_tree(&self.staged(path))
            }
            _ => anyhow::bail!("unsupported staged operation `{}`", operation.kind_name()),
        }
    }

    fn apply(&self, operation: &MutationOperation) -> anyhow::Result<()> {
        self.validate_workspace_paths(operation)?;
        match operation {
            MutationOperation::Create { path } | MutationOperation::Update { path } => {
                fs::rename(self.staged(path), self.workspace(path))?;
            }
            MutationOperation::Delete { path, .. } => fs::remove_file(self.workspace(path))?,
            MutationOperation::Rename {
                old_path, new_path, ..
            }
            | MutationOperation::Move {
                old_path, new_path, ..
            } => {
                fs::rename(self.workspace(old_path), self.workspace(new_path))?;
            }
            MutationOperation::Hardlink { old_path, new_path } => {
                fs::hard_link(self.workspace(old_path), self.workspace(new_path))?;
            }
            MutationOperation::Mkdir { path } => fs::create_dir(self.workspace(path))?,
            MutationOperation::Rmdir { path } => fs::remove_dir(self.workspace(path))?,
            MutationOperation::WriteDirectory { path } => self.replace_directory(path)?,
            _ => anyhow::bail!("unsupported staged operation `{}`", operation.kind_name()),
        }
        Ok(())
    }

    fn replace_directory(&self, path: &str) -> anyhow::Result<()> {
        let destination = self.workspace(path);
        let backup = self.root.join("previous-directory");
        fs::rename(&destination, &backup)?;
        if let Err(error) = fs::rename(self.staged(path), &destination) {
            let _ = fs::rename(&backup, &destination);
            return Err(error.into());
        }
        fs::remove_dir_all(backup)?;
        Ok(())
    }
    fn validate_workspace_paths(&self, operation: &MutationOperation) -> anyhow::Result<()> {
        for path in operation_paths(operation) {
            ensure_workspace_components(&self.repo_root, path)?;
        }
        Ok(())
    }

    fn workspace(&self, relative: &str) -> PathBuf {
        self.repo_root.join(relative)
    }

    fn staged(&self, relative: &str) -> PathBuf {
        self.root.join(relative)
    }
}

impl Drop for PrivateStage {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn ensure_private_directory(repo_root: &Path, directory: &Path) -> anyhow::Result<()> {
    let relative = directory.strip_prefix(repo_root)?;
    let mut cursor = repo_root.to_path_buf();
    for component in relative.components() {
        cursor.push(component);
        match fs::symlink_metadata(&cursor) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                anyhow::bail!("sandbox private staging path is symlinked")
            }
            Ok(metadata) if !metadata.is_dir() => {
                anyhow::bail!("sandbox private staging path is not a directory")
            }
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => fs::create_dir(&cursor)?,
            Err(error) => return Err(error.into()),
        }
    }
    Ok(())
}

fn ensure_workspace_components(repo_root: &Path, relative: &str) -> anyhow::Result<()> {
    let mut cursor = repo_root.canonicalize()?;
    for component in Path::new(relative).components() {
        cursor.push(component);
        match fs::symlink_metadata(&cursor) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                anyhow::bail!("sandbox workspace target has a symlinked component")
            }
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => break,
            Err(error) => return Err(error.into()),
        }
    }
    Ok(())
}

fn copy_directory_tree(source: &Path, destination: &Path) -> anyhow::Result<()> {
    validate_directory_tree(source)?;
    fs::create_dir(destination)?;
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        let source = entry.path();
        let destination = destination.join(entry.file_name());
        let metadata = fs::symlink_metadata(&source)?;
        if metadata.is_dir() {
            copy_directory_tree(&source, &destination)?;
        } else if metadata.is_file() {
            fs::copy(source, destination)?;
        } else {
            anyhow::bail!("sandbox directory staging supports only regular files");
        }
    }
    Ok(())
}

fn validate_directory_tree(path: &Path) -> anyhow::Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        anyhow::bail!("sandbox write-directory target must be a real directory");
    }
    for entry in fs::read_dir(path)? {
        let entry = entry?;
        let path = entry.path();
        let metadata = fs::symlink_metadata(&path)?;
        if metadata.file_type().is_symlink() {
            anyhow::bail!(
                "sandbox write-directory refuses symlink `{}`",
                path.display()
            );
        }
        if metadata.is_dir() {
            validate_directory_tree(&path)?;
        } else if metadata.is_file() {
            #[cfg(unix)]
            if metadata.nlink() > 1 {
                anyhow::bail!(
                    "sandbox write-directory refuses hardlinked file `{}`",
                    path.display()
                );
            }
        } else {
            anyhow::bail!(
                "sandbox write-directory refuses unsupported entry `{}`",
                path.display()
            );
        }
    }
    Ok(())
}

fn require_regular_file(path: &Path, display: &str) -> anyhow::Result<()> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|_| anyhow::anyhow!("sandbox staged target `{display}` must be a regular file"))?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        anyhow::bail!("sandbox staged target `{display}` must be a regular file");
    }
    Ok(())
}

fn require_absent(path: &Path, display: &str) -> anyhow::Result<()> {
    if fs::symlink_metadata(path).is_ok() {
        anyhow::bail!("sandbox staged target `{display}` must be absent");
    }
    Ok(())
}

fn require_empty_directory(path: &Path, display: &str) -> anyhow::Result<()> {
    let metadata = fs::symlink_metadata(path).map_err(|_| {
        anyhow::anyhow!("sandbox staged target `{display}` must be an empty directory")
    })?;
    if metadata.file_type().is_symlink()
        || !metadata.is_dir()
        || fs::read_dir(path)?.next().is_some()
    {
        anyhow::bail!("sandbox staged target `{display}` must be an empty directory");
    }
    Ok(())
}

fn require_same_regular_file(
    left: &Path,
    right: &Path,
    old: &str,
    new: &str,
) -> anyhow::Result<()> {
    require_regular_file(left, old)?;
    require_regular_file(right, new)?;
    #[cfg(unix)]
    {
        let left = fs::metadata(left)?;
        let right = fs::metadata(right)?;
        if (left.dev(), left.ino()) != (right.dev(), right.ino()) {
            anyhow::bail!("sandbox staged hardlink targets must be aliases");
        }
    }
    Ok(())
}

#[expect(
    clippy::too_many_arguments,
    reason = "sandbox launch wires policy knobs explicitly"
)]
fn run_sandboxed_command(
    command: &str,
    cwd: &Path,
    writable_paths: &[SandboxWritablePath],
    connect_sockets: &[PathBuf],
    allow_signal: bool,
    allow_macos_identity_and_trust_services: bool,
    network: SandboxNetworkPolicy,
    timeout: Duration,
    stream_events: bool,
    allow_direct_nested: bool,
) -> anyhow::Result<SandboxCommandResult> {
    let temp_dir = sandbox_temp_dir(writable_paths);
    #[cfg(not(target_os = "macos"))]
    let _ = allow_macos_identity_and_trust_services;
    if allow_direct_nested && allow_direct_nested_sandbox_run() {
        let mut command = direct_shell_command(command, cwd);
        apply_sandbox_temp_env(&mut command, temp_dir.as_deref());
        return run_command_with_timeout(command, timeout, stream_events);
    }
    #[cfg(target_os = "macos")]
    {
        run_command_with_timeout(
            seatbelt_command(
                command,
                cwd,
                writable_paths,
                connect_sockets,
                allow_signal,
                allow_macos_identity_and_trust_services,
                temp_dir.as_deref(),
                network,
            ),
            timeout,
            stream_events,
        )
    }

    #[cfg(target_os = "linux")]
    {
        run_command_with_timeout(
            bubblewrap_command(
                command,
                cwd,
                writable_paths,
                connect_sockets,
                allow_signal,
                temp_dir.as_deref(),
                network,
            ),
            timeout,
            stream_events,
        )
    }

    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        let _ = (
            command,
            cwd,
            writable_paths,
            connect_sockets,
            allow_signal,
            allow_macos_identity_and_trust_services,
            temp_dir,
            network,
            timeout,
        );
        anyhow::bail!("stateful sandbox run is only supported on macOS and Linux");
    }
}

pub(crate) fn run_private_sandbox_command(
    command: &str,
    cwd: &Path,
    writable_paths: &[SandboxWritablePath],
    timeout: Duration,
) -> anyhow::Result<SandboxCommandResult> {
    run_sandboxed_command(
        command,
        cwd,
        writable_paths,
        &[],
        false,
        false,
        SandboxNetworkPolicy::Disabled,
        timeout,
        false,
        false,
    )
}

#[expect(
    clippy::too_many_arguments,
    reason = "git sandbox launch reuses parsed policy knobs"
)]
fn run_sandboxed_git_command(
    words: &[String],
    cwd: &Path,
    writable_paths: &[SandboxWritablePath],
    temp_dir: &Path,
    hooks_dir: &Path,
    network: SandboxNetworkPolicy,
    timeout: Duration,
    stream_events: bool,
) -> anyhow::Result<SandboxCommandResult> {
    let config = discover_git_profile_config(cwd);

    if allow_direct_nested_sandbox_run() {
        let mut command = direct_git_command(words, cwd);
        apply_git_profile_env(&mut command, temp_dir, hooks_dir, &config);
        return run_command_with_timeout(command, timeout, stream_events);
    }

    #[cfg(target_os = "macos")]
    {
        run_command_with_timeout(
            seatbelt_git_command(
                words,
                cwd,
                writable_paths,
                temp_dir,
                hooks_dir,
                &config,
                network,
            ),
            timeout,
            stream_events,
        )
    }

    #[cfg(target_os = "linux")]
    {
        run_command_with_timeout(
            bubblewrap_git_command(
                words,
                cwd,
                writable_paths,
                temp_dir,
                hooks_dir,
                &config,
                network,
            ),
            timeout,
            stream_events,
        )
    }

    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        let _ = (
            words,
            cwd,
            writable_paths,
            temp_dir,
            hooks_dir,
            network,
            timeout,
        );
        anyhow::bail!("stateful sandbox run is only supported on macOS and Linux");
    }
}

fn allow_direct_nested_sandbox_run() -> bool {
    std::env::var_os(STATEFUL_SANDBOX_RUN_ACTIVE_ENV).is_some()
        && matches!(
            std::env::var_os(STATEFUL_ALLOW_NESTED_SANDBOX_RUN_ENV)
                .as_deref()
                .and_then(|value| value.to_str()),
            Some("1")
        )
}

fn direct_shell_command(command: &str, cwd: &Path) -> Command {
    let mut direct = Command::new("/bin/sh");
    direct.arg("-c").arg(command).current_dir(cwd);
    direct
}

fn direct_git_command(words: &[String], cwd: &Path) -> Command {
    let mut direct = Command::new("git");
    direct.args(&words[1..]).current_dir(cwd);
    direct
}

pub(crate) fn normalize_sandbox_target_path(field: &str, path: &str) -> anyhow::Result<String> {
    let trimmed = path.trim();
    if trimmed.is_empty() {
        anyhow::bail!("stateful sandbox run {field} entries must not be empty");
    }
    if Path::new(trimmed).is_absolute() {
        anyhow::bail!("stateful sandbox run {field} entries must be repo-relative");
    }

    let normalized = trimmed.replace('\\', "/");
    if normalized.starts_with('/') {
        anyhow::bail!("stateful sandbox run {field} entries must be repo-relative");
    }

    let mut segments = Vec::new();
    for segment in normalized.split('/') {
        if segment.is_empty() || segment == "." {
            continue;
        }
        if segment == ".." {
            anyhow::bail!("stateful sandbox run {field} entries must stay inside the repo");
        }
        if is_git_internal_segment(segment) {
            anyhow::bail!("stateful sandbox run refuses Git internals");
        }
        if segment == ".stateful" {
            anyhow::bail!("stateful sandbox run refuses its private staging directory");
        }
        if segment.chars().any(char::is_control) {
            anyhow::bail!("stateful sandbox run paths must not contain control characters");
        }
        segments.push(segment);
    }

    if segments.is_empty() {
        anyhow::bail!("stateful sandbox run {field} entries must not be empty");
    }

    Ok(segments.join("/"))
}

pub(crate) fn resolve_sandbox_cwd(repo_root: &Path) -> anyhow::Result<PathBuf> {
    let cwd = repo_root
        .canonicalize()
        .map_err(|error| anyhow::anyhow!("stateful sandbox run cwd must exist: {error}"))?;
    if !cwd.is_dir() {
        anyhow::bail!("stateful sandbox run cwd must be a directory");
    }
    Ok(cwd)
}

fn validate_sandbox_run_process_inspection(command: &str) -> anyhow::Result<()> {
    if sandbox_run_command_invokes_raw_process_inspection(command, 0) {
        anyhow::bail!("process inspection must use stateful sandbox process find");
    }
    Ok(())
}

fn sandbox_run_command_invokes_raw_process_inspection(command: &str, depth: usize) -> bool {
    if depth > 4 {
        return true;
    }

    if shell_substitutions_invoke_raw_process_inspection(command, depth) {
        return true;
    }

    simple_shell_command_segments(command)
        .iter()
        .any(|segment| {
            split_process_inspection_command_words(segment)
                .ok()
                .is_some_and(|words| {
                    simple_command_words_invoke_raw_process_inspection(&words, depth)
                })
        })
}

fn simple_command_words_invoke_raw_process_inspection(words: &[String], depth: usize) -> bool {
    let mut index = 0;
    while index < words.len() {
        let word = &words[index];
        if first_word_is_env_assignment(word)
            || matches!(
                word.as_str(),
                "!" | "if" | "then" | "do" | "else" | "elif" | "while" | "until"
            )
        {
            index += 1;
            continue;
        }

        let command = process_comm_basename(word);
        if matches!(command, "ps" | "pgrep") {
            return true;
        }
        if command == "time" {
            index = skip_process_wrapper_options(words, index + 1);
            continue;
        }
        if command == "command" {
            index = skip_process_wrapper_options(words, index + 1);
            continue;
        }
        if command == "exec" {
            index = skip_exec_process_wrapper_options(words, index + 1);
            continue;
        }
        if command == "env" {
            if env_process_wrapper_invokes_raw_process_inspection(words, index + 1, depth) {
                return true;
            }
            index = skip_env_process_wrapper(words, index + 1);
            continue;
        }
        if matches!(command, "sh" | "bash" | "zsh" | "dash") {
            return shell_c_argument_invokes_raw_process_inspection(words, index, depth);
        }
        return false;
    }

    false
}

fn skip_process_wrapper_options(words: &[String], mut index: usize) -> usize {
    while index < words.len() {
        let word = &words[index];
        if word == "--" {
            return index + 1;
        }
        if word.starts_with('-') && word.len() > 1 {
            index += 1;
            continue;
        }
        break;
    }
    index
}

fn skip_exec_process_wrapper_options(words: &[String], mut index: usize) -> usize {
    while index < words.len() {
        let word = &words[index];
        if word == "--" {
            return index + 1;
        }
        if word == "-a" {
            index = (index + 2).min(words.len());
            continue;
        }
        if word.starts_with('-') && word.len() > 1 {
            index += 1;
            continue;
        }
        break;
    }
    index
}

fn env_process_wrapper_invokes_raw_process_inspection(
    words: &[String],
    mut index: usize,
    depth: usize,
) -> bool {
    while index < words.len() {
        let word = &words[index];
        if word == "--" {
            return false;
        }
        if first_word_is_env_assignment(word) {
            index += 1;
            continue;
        }
        if let Some(command) = word.strip_prefix("--split-string=") {
            return sandbox_run_command_invokes_raw_process_inspection(command, depth + 1);
        }
        if word == "-S" || word == "--split-string" {
            return words.get(index + 1).is_some_and(|command| {
                sandbox_run_command_invokes_raw_process_inspection(command, depth + 1)
            });
        }
        if let Some(command) = word.strip_prefix("-S") {
            if !command.is_empty() {
                return sandbox_run_command_invokes_raw_process_inspection(command, depth + 1);
            }
        }
        if word.starts_with('-') && word.len() > 1 {
            let consumes_arg = matches!(
                word.as_str(),
                "-u" | "--unset" | "-C" | "--chdir" | "-P" | "--path"
            );
            index += 1;
            if consumes_arg {
                index = (index + 1).min(words.len());
            }
            continue;
        }
        break;
    }

    false
}

fn skip_env_process_wrapper(words: &[String], mut index: usize) -> usize {
    while index < words.len() {
        let word = &words[index];
        if word == "--" {
            return index + 1;
        }
        if first_word_is_env_assignment(word) {
            index += 1;
            continue;
        }
        if word.starts_with('-') && word.len() > 1 {
            let consumes_arg = matches!(
                word.as_str(),
                "-u" | "--unset" | "-C" | "--chdir" | "-P" | "--path" | "-S" | "--split-string"
            );
            index += 1;
            if consumes_arg {
                index = (index + 1).min(words.len());
            }
            continue;
        }
        break;
    }
    index
}

fn shell_c_argument_invokes_raw_process_inspection(
    words: &[String],
    shell_index: usize,
    depth: usize,
) -> bool {
    let mut index = shell_index + 1;
    while index < words.len() {
        let word = &words[index];
        if word == "-c" || (word.starts_with('-') && word.contains('c')) {
            return words.get(index + 1).is_some_and(|command| {
                sandbox_run_command_invokes_raw_process_inspection(command, depth + 1)
            });
        }
        index += 1;
    }

    false
}

fn shell_substitutions_invoke_raw_process_inspection(command: &str, depth: usize) -> bool {
    let chars = command.chars().collect::<Vec<_>>();
    let mut index = 0;
    let mut state = ShellSegmentQuoteState::None;

    while index < chars.len() {
        let ch = chars[index];
        match state {
            ShellSegmentQuoteState::None => match ch {
                '\'' => state = ShellSegmentQuoteState::Single,
                '"' => state = ShellSegmentQuoteState::Double,
                '\\' => index += 1,
                '$' if chars.get(index + 1) == Some(&'(') => {
                    if let Some((nested, end_index)) =
                        collect_parenthesized_shell_command(&chars, index + 2)
                    {
                        if sandbox_run_command_invokes_raw_process_inspection(&nested, depth + 1) {
                            return true;
                        }
                        index = end_index;
                    }
                }
                '`' => {
                    if let Some((nested, end_index)) = collect_backtick_shell_command(&chars, index)
                    {
                        if sandbox_run_command_invokes_raw_process_inspection(&nested, depth + 1) {
                            return true;
                        }
                        index = end_index;
                    }
                }
                _ => {}
            },
            ShellSegmentQuoteState::Single => {
                if ch == '\'' {
                    state = ShellSegmentQuoteState::None;
                }
            }
            ShellSegmentQuoteState::Double => match ch {
                '"' => state = ShellSegmentQuoteState::None,
                '\\' => index += 1,
                '$' if chars.get(index + 1) == Some(&'(') => {
                    if let Some((nested, end_index)) =
                        collect_parenthesized_shell_command(&chars, index + 2)
                    {
                        if sandbox_run_command_invokes_raw_process_inspection(&nested, depth + 1) {
                            return true;
                        }
                        index = end_index;
                    }
                }
                '`' => {
                    if let Some((nested, end_index)) = collect_backtick_shell_command(&chars, index)
                    {
                        if sandbox_run_command_invokes_raw_process_inspection(&nested, depth + 1) {
                            return true;
                        }
                        index = end_index;
                    }
                }
                _ => {}
            },
        }
        index += 1;
    }

    false
}

fn collect_parenthesized_shell_command(
    chars: &[char],
    mut index: usize,
) -> Option<(String, usize)> {
    let mut nested = String::new();
    let mut depth = 1;
    let mut state = ShellSegmentQuoteState::None;

    while index < chars.len() {
        let ch = chars[index];
        match state {
            ShellSegmentQuoteState::None => match ch {
                '\'' => {
                    state = ShellSegmentQuoteState::Single;
                    nested.push(ch);
                }
                '"' => {
                    state = ShellSegmentQuoteState::Double;
                    nested.push(ch);
                }
                '\\' => {
                    nested.push(ch);
                    if let Some(next) = chars.get(index + 1) {
                        nested.push(*next);
                        index += 1;
                    }
                }
                '(' => {
                    depth += 1;
                    nested.push(ch);
                }
                ')' => {
                    depth -= 1;
                    if depth == 0 {
                        return Some((nested, index));
                    }
                    nested.push(ch);
                }
                _ => nested.push(ch),
            },
            ShellSegmentQuoteState::Single => {
                nested.push(ch);
                if ch == '\'' {
                    state = ShellSegmentQuoteState::None;
                }
            }
            ShellSegmentQuoteState::Double => {
                nested.push(ch);
                if ch == '"' {
                    state = ShellSegmentQuoteState::None;
                }
            }
        }
        index += 1;
    }

    None
}

fn collect_backtick_shell_command(chars: &[char], mut index: usize) -> Option<(String, usize)> {
    let mut nested = String::new();
    index += 1;
    while index < chars.len() {
        let ch = chars[index];
        if ch == '`' {
            return Some((nested, index));
        }
        if ch == '\\' {
            if let Some(next) = chars.get(index + 1) {
                nested.push(*next);
                index += 2;
                continue;
            }
        }
        nested.push(ch);
        index += 1;
    }

    None
}

fn split_process_inspection_command_words(command: &str) -> Result<Vec<String>, String> {
    let mut words = Vec::new();
    let mut current = String::new();
    let mut state = ShellSegmentQuoteState::None;
    let mut in_word = false;
    let mut chars = command.chars().peekable();

    while let Some(ch) = chars.next() {
        match state {
            ShellSegmentQuoteState::None => match ch {
                '\'' => {
                    state = ShellSegmentQuoteState::Single;
                    in_word = true;
                }
                '"' => {
                    state = ShellSegmentQuoteState::Double;
                    in_word = true;
                }
                '\\' => {
                    if let Some(next) = chars.next() {
                        current.push(next);
                    } else {
                        current.push(ch);
                    }
                    in_word = true;
                }
                ch if ch.is_whitespace() => {
                    if in_word {
                        words.push(std::mem::take(&mut current));
                        in_word = false;
                    }
                }
                _ => {
                    current.push(ch);
                    in_word = true;
                }
            },
            ShellSegmentQuoteState::Single => {
                if ch == '\'' {
                    state = ShellSegmentQuoteState::None;
                } else {
                    current.push(ch);
                }
            }
            ShellSegmentQuoteState::Double => match ch {
                '"' => state = ShellSegmentQuoteState::None,
                '\\' => {
                    if let Some(next) = chars.next() {
                        current.push(next);
                    } else {
                        current.push(ch);
                    }
                }
                _ => current.push(ch),
            },
        }
    }

    if state != ShellSegmentQuoteState::None {
        return Err("sandbox run command has unterminated quotes".to_string());
    }
    if in_word {
        words.push(current);
    }

    Ok(words)
}

fn simple_shell_command_segments(command: &str) -> Vec<String> {
    let mut segments = Vec::new();
    let mut current = String::new();
    let mut state = ShellSegmentQuoteState::None;

    for ch in command.chars() {
        match state {
            ShellSegmentQuoteState::None => match ch {
                '\'' => {
                    state = ShellSegmentQuoteState::Single;
                    current.push(ch);
                }
                '"' => {
                    state = ShellSegmentQuoteState::Double;
                    current.push(ch);
                }
                ';' | '|' | '&' | '(' | ')' | '\n' | '\r' => {
                    if !current.trim().is_empty() {
                        segments.push(current.trim().to_string());
                    }
                    current.clear();
                }
                _ => current.push(ch),
            },
            ShellSegmentQuoteState::Single => {
                current.push(ch);
                if ch == '\'' {
                    state = ShellSegmentQuoteState::None;
                }
            }
            ShellSegmentQuoteState::Double => {
                current.push(ch);
                if ch == '"' {
                    state = ShellSegmentQuoteState::None;
                }
            }
        }
    }

    if !current.trim().is_empty() {
        segments.push(current.trim().to_string());
    }

    segments
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ShellSegmentQuoteState {
    None,
    Single,
    Double,
}

fn git_profile_persistent_metadata_paths(repo_root: &Path) -> Vec<SandboxWritablePath> {
    let dot_git = repo_root.join(".git");
    if dot_git.exists() && !dot_git.is_dir() {
        return vec![SandboxWritablePath::file(dot_git)];
    }

    vec![
        SandboxWritablePath::file(dot_git.join("config")),
        SandboxWritablePath::file(dot_git.join("config.worktree")),
        SandboxWritablePath::directory(dot_git.join("hooks")),
    ]
}

fn existing_git_profile_persistent_metadata_paths(repo_root: &Path) -> Vec<SandboxWritablePath> {
    git_profile_persistent_metadata_paths(repo_root)
        .into_iter()
        .filter(|path| path.path.exists())
        .collect()
}

fn validate_git_profile_command(command: &str) -> anyhow::Result<Vec<String>> {
    parse_git_profile_command(command)
        .map_err(|reason| anyhow::anyhow!("git profile requires a single git command: {reason}"))
}

pub(crate) fn parse_git_profile_command(command: &str) -> Result<Vec<String>, String> {
    reject_direct_profile_shell_syntax(command)
        .map_err(|reason| format!("git profile requires a single git command: {reason}"))?;
    let words = split_simple_command_words(command)
        .map_err(|reason| format!("git profile requires a single git command: {reason}"))?;
    if words.first().is_none_or(|word| word != "git") {
        return Err("git profile requires a single git command".to_string());
    }
    validate_git_profile_words(&words)?;
    Ok(words)
}

fn validate_git_profile_words(words: &[String]) -> Result<(), String> {
    let (subcommand_index, subcommand) = git_profile_subcommand(words)?;
    if !GIT_PROFILE_ALLOWED_SUBCOMMANDS.contains(&subcommand) {
        return Err(format!(
            "git profile does not allow git subcommand `{subcommand}`"
        ));
    }
    validate_git_profile_subcommand_args(subcommand, &words[subcommand_index + 1..])
}

fn git_profile_subcommand(words: &[String]) -> Result<(usize, &str), String> {
    let mut index = 1;
    while index < words.len() {
        let word = words[index].as_str();
        if word == "--" {
            return Err("git profile requires an explicit git subcommand".to_string());
        }
        if word == "-c" || word.starts_with("-c") {
            return Err("git profile does not allow inline git config".to_string());
        }
        if word == "--config-env" || word.starts_with("--config-env=") {
            return Err("git profile does not allow config-env".to_string());
        }
        if word == "-C"
            || word.starts_with("-C")
            || matches!(
                word,
                "--git-dir" | "--work-tree" | "--namespace" | "--exec-path"
            )
            || word.starts_with("--git-dir=")
            || word.starts_with("--work-tree=")
            || word.starts_with("--namespace=")
            || word.starts_with("--exec-path=")
        {
            return Err("git profile does not allow git path or exec flags".to_string());
        }
        if word.starts_with('-') {
            index += 1;
            continue;
        }
        return Ok((index, word));
    }
    Err("git profile requires an explicit git subcommand".to_string())
}

fn validate_git_profile_subcommand_args(_subcommand: &str, _args: &[String]) -> Result<(), String> {
    Ok(())
}

const GIT_PROFILE_ALLOWED_SUBCOMMANDS: &[&str] = &[
    "blame",
    "cat-file",
    "describe",
    "diff",
    "log",
    "ls-files",
    "rev-list",
    "rev-parse",
    "show",
    "status",
];

fn reject_direct_profile_shell_syntax(command: &str) -> Result<(), String> {
    let mut state = ShellQuoteState::None;
    let mut chars = command.chars().peekable();
    while let Some(ch) = chars.next() {
        match state {
            ShellQuoteState::None => match ch {
                '\'' => state = ShellQuoteState::Single,
                '"' => state = ShellQuoteState::Double,
                '$' if chars.peek().is_some_and(|next| *next == '(') => {
                    return Err("command substitution is not supported".to_string());
                }
                '\\' => return Err("shell escapes are not supported".to_string()),
                ';' | '|' | '&' | '<' | '>' | '\n' | '\r' | '`' => {
                    return Err("shell control syntax is not supported".to_string());
                }
                _ => {}
            },
            ShellQuoteState::Single => {
                if ch == '\'' {
                    state = ShellQuoteState::None;
                }
            }
            ShellQuoteState::Double => match ch {
                '"' => state = ShellQuoteState::None,
                '$' if chars.peek().is_some_and(|next| *next == '(') => {
                    return Err("command substitution is not supported".to_string());
                }
                '`' => return Err("command substitution is not supported".to_string()),
                '\\' => return Err("shell escapes are not supported".to_string()),
                _ => {}
            },
        }
    }

    if state != ShellQuoteState::None {
        return Err("unterminated quotes".to_string());
    }

    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ShellQuoteState {
    None,
    Single,
    Double,
}

pub(crate) fn sandbox_temp_dir(writable_paths: &[SandboxWritablePath]) -> Option<PathBuf> {
    writable_paths
        .iter()
        .find(|path| path.kind == SandboxWritablePathKind::Directory)
        .map(|path| path.path.join(".stateful-tmp"))
}

pub(crate) fn apply_sandbox_temp_env(command: &mut Command, temp_dir: Option<&Path>) {
    command.env_clear();
    for key in [
        "PATH",
        "HOME",
        "USER",
        "LOGNAME",
        "LANG",
        "LC_ALL",
        "LC_CTYPE",
        "SSL_CERT_FILE",
        "SSL_CERT_DIR",
        "NIX_SSL_CERT_FILE",
    ] {
        if let Some(value) = std::env::var_os(key) {
            command.env(key, value);
        }
    }
    command.env(STATEFUL_SANDBOX_RUN_ACTIVE_ENV, "1");
    let Some(temp_dir) = temp_dir else {
        return;
    };
    command
        .env("TMPDIR", temp_dir)
        .env("TEMP", temp_dir)
        .env("TMP", temp_dir);
    if let Some(tmp_root) = temp_dir.parent() {
        command.env("CARGO_TARGET_DIR", tmp_root.join("target"));
    }
}

fn is_git_internal_segment(segment: &str) -> bool {
    segment.eq_ignore_ascii_case(".git")
}

#[cfg(target_os = "macos")]
#[expect(
    clippy::too_many_arguments,
    reason = "seatbelt profile generation keeps sandbox knobs explicit"
)]
fn seatbelt_command(
    command: &str,
    cwd: &Path,
    writable_paths: &[SandboxWritablePath],
    connect_sockets: &[PathBuf],
    allow_signal: bool,
    allow_macos_identity_and_trust_services: bool,
    temp_dir: Option<&Path>,
    network: SandboxNetworkPolicy,
) -> Command {
    let mut profile = seatbelt_profile_with_connect_sockets(
        writable_paths,
        connect_sockets,
        allow_signal,
        network,
    );
    if allow_macos_identity_and_trust_services {
        push_seatbelt_macos_identity_and_trust_services(&mut profile);
    }
    let mut sandbox = Command::new("/usr/bin/sandbox-exec");
    sandbox
        .arg("-p")
        .arg(profile)
        .arg("/bin/sh")
        .arg("-c")
        .arg(command)
        .current_dir(cwd);
    apply_sandbox_temp_env(&mut sandbox, temp_dir);
    sandbox
}

#[cfg(target_os = "macos")]
fn seatbelt_git_command(
    words: &[String],
    cwd: &Path,
    writable_paths: &[SandboxWritablePath],
    temp_dir: &Path,
    hooks_dir: &Path,
    config: &GitProfileConfig,
    network: SandboxNetworkPolicy,
) -> Command {
    let profile = seatbelt_git_profile(writable_paths, cwd, network);
    let mut sandbox = Command::new("/usr/bin/sandbox-exec");
    sandbox
        .arg("-p")
        .arg(profile)
        .arg("git")
        .args(&words[1..])
        .current_dir(cwd);
    apply_git_profile_env(&mut sandbox, temp_dir, hooks_dir, config);
    sandbox
}

#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn seatbelt_git_profile(
    writable_paths: &[SandboxWritablePath],
    repo_root: &Path,
    network: SandboxNetworkPolicy,
) -> String {
    let mut profile = seatbelt_profile(writable_paths, network);
    push_seatbelt_file_write_denies(
        &mut profile,
        &git_profile_persistent_metadata_paths(repo_root),
    );
    push_seatbelt_macos_identity_and_trust_services(&mut profile);
    profile
}

#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn push_seatbelt_macos_identity_and_trust_services(profile: &mut String) {
    profile.push_str(
        "(allow mach-lookup\n\
             (global-name \"com.apple.system.opendirectoryd.libinfo\")\n\
             (global-name \"com.apple.system.DirectoryService.libinfo_v1\")\n\
             (global-name \"com.apple.trustd\")\n\
             (global-name \"com.apple.trustd.agent\"))\n",
    );
}

#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn push_seatbelt_file_write_denies(profile: &mut String, protected_paths: &[SandboxWritablePath]) {
    if protected_paths.is_empty() {
        return;
    }

    profile.push_str("(deny file-write*");
    for protected_path in protected_paths {
        profile.push_str(" (literal \"");
        profile.push_str(&seatbelt_escape(&protected_path.path.to_string_lossy()));
        profile.push_str("\")");
        if protected_path.kind == SandboxWritablePathKind::Directory {
            profile.push_str(" (subpath \"");
            profile.push_str(&seatbelt_escape(&protected_path.path.to_string_lossy()));
            profile.push_str("\")");
        }
    }
    profile.push_str(")\n");
}

#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn seatbelt_profile(
    writable_paths: &[SandboxWritablePath],
    network: SandboxNetworkPolicy,
) -> String {
    seatbelt_profile_with_connect_sockets(writable_paths, &[], false, network)
}

#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn seatbelt_profile_with_connect_sockets(
    writable_paths: &[SandboxWritablePath],
    connect_sockets: &[PathBuf],
    allow_signal: bool,
    network: SandboxNetworkPolicy,
) -> String {
    let mut profile =
        String::from("(version 1)\n(deny default)\n(allow process*)\n(allow file-read*)\n");
    push_seatbelt_device_read_allows(&mut profile);
    profile.push_str("(allow sysctl-read)\n(allow file-write* (literal \"/dev/null\")");
    for writable_path in writable_paths {
        profile.push_str(match writable_path.kind {
            SandboxWritablePathKind::File => " (literal \"",
            SandboxWritablePathKind::Directory => " (subpath \"",
        });
        profile.push_str(&seatbelt_escape(&writable_path.path.to_string_lossy()));
        profile.push_str("\")");
    }
    profile.push_str(")\n");
    if network == SandboxNetworkPolicy::Enabled {
        profile.push_str("(allow network*)\n");
    } else {
        for socket_path in connect_sockets {
            profile.push_str("(allow network-outbound (literal \"");
            profile.push_str(&seatbelt_escape(&socket_path.to_string_lossy()));
            profile.push_str("\"))\n");
        }
    }
    if allow_signal {
        profile.push_str("(allow signal)\n");
    }
    profile
}

#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub(crate) fn push_seatbelt_device_read_allows(profile: &mut String) {
    profile.push_str(
        "(allow file-read* (literal \"/dev/null\") (literal \"/dev/zero\") (literal \"/dev/urandom\"))\n",
    );
}

#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub(crate) fn seatbelt_escape(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}

#[cfg(target_os = "linux")]
fn bubblewrap_command(
    command: &str,
    cwd: &Path,
    writable_paths: &[SandboxWritablePath],
    connect_sockets: &[PathBuf],
    allow_signal: bool,
    temp_dir: Option<&Path>,
    network: SandboxNetworkPolicy,
) -> Command {
    let mut bwrap = Command::new("bwrap");
    bwrap.args(bubblewrap_args(
        command,
        cwd,
        writable_paths,
        connect_sockets,
        allow_signal,
        network,
    ));
    apply_sandbox_temp_env(&mut bwrap, temp_dir);
    bwrap
}

#[cfg(target_os = "linux")]
fn bubblewrap_git_command(
    words: &[String],
    cwd: &Path,
    writable_paths: &[SandboxWritablePath],
    temp_dir: &Path,
    hooks_dir: &Path,
    config: &GitProfileConfig,
    network: SandboxNetworkPolicy,
) -> Command {
    let mut bwrap = Command::new("bwrap");
    bwrap.args(bubblewrap_git_args(words, cwd, writable_paths, network));
    apply_git_profile_env(&mut bwrap, temp_dir, hooks_dir, config);
    bwrap
}

#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn bubblewrap_args(
    command: &str,
    cwd: &Path,
    writable_paths: &[SandboxWritablePath],
    connect_sockets: &[PathBuf],
    allow_signal: bool,
    network: SandboxNetworkPolicy,
) -> Vec<OsString> {
    let _ = allow_signal;
    let mut args =
        bubblewrap_base_args_with_connect_sockets(cwd, writable_paths, connect_sockets, network);
    args.push(OsString::from("--"));
    args.push(OsString::from("/bin/sh"));
    args.push(OsString::from("-c"));
    args.push(OsString::from(command));
    args
}

#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn bubblewrap_git_args(
    words: &[String],
    cwd: &Path,
    writable_paths: &[SandboxWritablePath],
    network: SandboxNetworkPolicy,
) -> Vec<OsString> {
    let mut args = bubblewrap_base_args(cwd, writable_paths, network);
    for protected_path in existing_git_profile_persistent_metadata_paths(cwd) {
        args.push(OsString::from("--ro-bind"));
        args.push(protected_path.path.as_os_str().to_owned());
        args.push(protected_path.path.as_os_str().to_owned());
    }
    args.push(OsString::from("--"));
    args.push(OsString::from("git"));
    args.extend(words.iter().skip(1).map(OsString::from));
    args
}

#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn bubblewrap_base_args(
    cwd: &Path,
    writable_paths: &[SandboxWritablePath],
    network: SandboxNetworkPolicy,
) -> Vec<OsString> {
    bubblewrap_base_args_with_connect_sockets(cwd, writable_paths, &[], network)
}

#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn bubblewrap_base_args_with_connect_sockets(
    cwd: &Path,
    writable_paths: &[SandboxWritablePath],
    connect_sockets: &[PathBuf],
    network: SandboxNetworkPolicy,
) -> Vec<OsString> {
    let mut args = vec![
        OsString::from("--unshare-all"),
        OsString::from("--die-with-parent"),
    ];

    match network {
        SandboxNetworkPolicy::Disabled => args.push(OsString::from("--unshare-net")),
        SandboxNetworkPolicy::Enabled => args.push(OsString::from("--share-net")),
    }

    args.extend([
        OsString::from("--ro-bind"),
        OsString::from("/"),
        OsString::from("/"),
        OsString::from("--proc"),
        OsString::from("/proc"),
        OsString::from("--dev-bind"),
        OsString::from("/dev/null"),
        OsString::from("/dev/null"),
        OsString::from("--dev-bind"),
        OsString::from("/dev/zero"),
        OsString::from("/dev/zero"),
        OsString::from("--dev-bind"),
        OsString::from("/dev/urandom"),
        OsString::from("/dev/urandom"),
    ]);

    for writable_path in writable_paths {
        args.push(OsString::from("--bind"));
        args.push(writable_path.path.as_os_str().to_owned());
        args.push(writable_path.path.as_os_str().to_owned());
    }
    for socket_path in connect_sockets {
        args.push(OsString::from("--ro-bind"));
        args.push(socket_path.as_os_str().to_owned());
        args.push(socket_path.as_os_str().to_owned());
    }

    args.push(OsString::from("--chdir"));
    args.push(cwd.as_os_str().to_owned());
    args
}

fn discover_git_profile_config(cwd: &Path) -> GitProfileConfig {
    GitProfileConfig {
        identity: discover_git_profile_identity(cwd),
        credential_helpers: discover_git_profile_credential_helpers(cwd),
    }
}

fn discover_git_profile_identity(cwd: &Path) -> Option<GitProfileIdentity> {
    let name = git_profile_config_value(cwd, "user.name")?;
    let email = git_profile_config_value(cwd, "user.email")?;
    Some(GitProfileIdentity { name, email })
}

fn git_profile_config_value(cwd: &Path, key: &str) -> Option<String> {
    git_profile_config_values(cwd, key).into_iter().last()
}

fn discover_git_profile_credential_helpers(cwd: &Path) -> Vec<String> {
    let mut seen = BTreeSet::new();
    git_profile_config_values(cwd, "credential.helper")
        .into_iter()
        .filter_map(|helper| allowed_git_profile_credential_helper(&helper))
        .filter(|helper| seen.insert(helper.clone()))
        .collect()
}

fn git_profile_config_values(cwd: &Path, key: &str) -> Vec<String> {
    let mut command = Command::new("git");
    remove_git_profile_env(&mut command);
    command
        .arg("-C")
        .arg(cwd)
        .arg("config")
        .arg("--get-all")
        .arg(key)
        .env("GIT_TERMINAL_PROMPT", "0");
    let Some(output) = command.output().ok() else {
        return Vec::new();
    };
    if !output.status.success() {
        return Vec::new();
    }
    let Some(output) = String::from_utf8(output.stdout).ok() else {
        return Vec::new();
    };
    output
        .lines()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .collect()
}

fn allowed_git_profile_credential_helper(helper: &str) -> Option<String> {
    match helper.trim() {
        "store" => Some("store".to_string()),
        "cache" => Some("cache".to_string()),
        "osxkeychain" => Some("osxkeychain".to_string()),
        "!gh auth git-credential" | "! gh auth git-credential" => {
            Some("!gh auth git-credential".to_string())
        }
        _ => None,
    }
}

fn remove_git_profile_env(command: &mut Command) {
    for (key, _) in std::env::vars_os() {
        let key_string = key.to_string_lossy();
        if key_string == "GIT_CONFIG_COUNT"
            || key_string.starts_with("GIT_CONFIG_KEY_")
            || key_string.starts_with("GIT_CONFIG_VALUE_")
            || key_string.starts_with("GIT_")
        {
            command.env_remove(key);
        }
    }
}

fn apply_git_profile_env(
    command: &mut Command,
    temp_dir: &Path,
    hooks_dir: &Path,
    config: &GitProfileConfig,
) {
    apply_sandbox_temp_env(command, Some(temp_dir));
    remove_git_profile_env(command);
    let mut config_entries = vec![
        ("core.hooksPath", hooks_dir.as_os_str().to_owned()),
        ("core.fsmonitor", OsString::from("false")),
        ("diff.external", OsString::from("")),
        ("interactive.diffFilter", OsString::from("")),
        ("protocol.ext.allow", OsString::from("never")),
        ("branch.autoSetupMerge", OsString::from("false")),
        ("branch.autoSetupRebase", OsString::from("never")),
    ];
    if let Some(identity) = &config.identity {
        config_entries.push(("user.name", OsString::from(&identity.name)));
        config_entries.push(("user.email", OsString::from(&identity.email)));
    }
    config_entries.extend(
        config
            .credential_helpers
            .iter()
            .map(|helper| ("credential.helper", OsString::from(helper))),
    );
    command
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_COUNT", config_entries.len().to_string())
        .env("GIT_LFS_SKIP_SMUDGE", "1")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_EDITOR", ":")
        .env("GIT_SEQUENCE_EDITOR", ":")
        .env("GIT_PAGER", "cat")
        .env("PAGER", "cat");
    for (index, (key, value)) in config_entries.into_iter().enumerate() {
        command
            .env(format!("GIT_CONFIG_KEY_{index}"), key)
            .env(format!("GIT_CONFIG_VALUE_{index}"), value);
    }
}

fn emit_sandbox_stream_event(stream: &str, bytes: &[u8]) {
    if bytes.is_empty() {
        return;
    }
    let chunk = String::from_utf8_lossy(bytes);
    let event = serde_json::json!({
        "event": "sandbox_output",
        "stream": stream,
        "chunk": chunk,
    });
    let mut stdout = io::stdout().lock();
    let _ = writeln!(stdout, "{event}");
    let _ = stdout.flush();
}

fn read_sandbox_pipe_to_end<R>(
    mut pipe: R,
    stream: &'static str,
    stream_events: bool,
) -> io::Result<Vec<u8>>
where
    R: Read,
{
    let mut bytes = Vec::new();
    let mut buffer = [0_u8; 8192];
    loop {
        let count = pipe.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        let chunk = &buffer[..count];
        if stream_events {
            emit_sandbox_stream_event(stream, chunk);
        }
        bytes.extend_from_slice(chunk);
    }
    Ok(bytes)
}

pub(crate) fn run_command_with_timeout(
    command: Command,
    timeout: Duration,
    stream_events: bool,
) -> anyhow::Result<SandboxCommandResult> {
    let signal_monitor = SandboxSignalMonitor::install()?;
    run_command_with_timeout_and_cancel(command, timeout, stream_events, || {
        signal_monitor.is_cancelled()
    })
}

fn run_command_with_timeout_and_cancel(
    mut command: Command,
    timeout: Duration,
    stream_events: bool,
    mut is_cancelled: impl FnMut() -> bool,
) -> anyhow::Result<SandboxCommandResult> {
    command.stdout(Stdio::piped()).stderr(Stdio::piped());
    isolate_sandbox_process_group(&mut command);
    let mut child = command.spawn()?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| anyhow::anyhow!("failed to capture sandbox stdout"))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| anyhow::anyhow!("failed to capture sandbox stderr"))?;
    let stdout_reader =
        thread::spawn(move || read_sandbox_pipe_to_end(stdout, "stdout", stream_events));
    let stderr_reader =
        thread::spawn(move || read_sandbox_pipe_to_end(stderr, "stderr", stream_events));

    let deadline = Instant::now() + timeout;
    let mut timed_out = false;
    let mut cancelled = false;
    let exit_status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if is_cancelled() {
            cancelled = true;
            break terminate_sandbox_child(&mut child)?.ok_or_else(|| {
                anyhow::anyhow!("cancelled and failed to terminate sandbox command")
            })?;
        }
        if Instant::now() >= deadline {
            timed_out = true;
            break terminate_sandbox_child(&mut child)?.ok_or_else(|| {
                anyhow::anyhow!("timed out and failed to terminate sandbox command")
            })?;
        }
        thread::sleep(Duration::from_millis(25));
    };
    cleanup_sandbox_process_group(&mut child, timed_out || cancelled)?;

    let stdout = stdout_reader
        .join()
        .map_err(|_| anyhow::anyhow!("sandbox stdout reader panicked"))??;
    let stderr = stderr_reader
        .join()
        .map_err(|_| anyhow::anyhow!("sandbox stderr reader panicked"))??;

    Ok(SandboxCommandResult {
        status: if cancelled {
            "cancelled"
        } else if timed_out {
            "timed_out"
        } else {
            "exited"
        },
        exit_code: exit_status.code(),
        stdout: String::from_utf8_lossy(&stdout).into_owned(),
        stderr: String::from_utf8_lossy(&stderr).into_owned(),
    })
}

struct SandboxSignalMonitor {
    #[cfg(unix)]
    cancelled: Arc<AtomicBool>,
    #[cfg(unix)]
    handle: SignalsHandle,
    #[cfg(unix)]
    thread: Option<thread::JoinHandle<()>>,
}

impl SandboxSignalMonitor {
    fn install() -> anyhow::Result<Self> {
        #[cfg(unix)]
        {
            let mut signals = Signals::new([SIGHUP, SIGINT, SIGTERM])?;
            let handle = signals.handle();
            let cancelled = Arc::new(AtomicBool::new(false));
            let thread_cancelled = Arc::clone(&cancelled);
            let thread = thread::spawn(move || {
                if signals.forever().next().is_some() {
                    thread_cancelled.store(true, Ordering::SeqCst);
                }
            });
            Ok(Self {
                cancelled,
                handle,
                thread: Some(thread),
            })
        }

        #[cfg(not(unix))]
        {
            Ok(Self {})
        }
    }

    fn is_cancelled(&self) -> bool {
        #[cfg(unix)]
        {
            self.cancelled.load(Ordering::SeqCst)
        }

        #[cfg(not(unix))]
        {
            false
        }
    }
}

impl Drop for SandboxSignalMonitor {
    fn drop(&mut self) {
        #[cfg(unix)]
        {
            self.handle.close();
            if let Some(thread) = self.thread.take() {
                let _ = thread.join();
            }
        }
    }
}

fn isolate_sandbox_process_group(command: &mut Command) {
    #[cfg(unix)]
    {
        command.process_group(0);
    }
}

fn cleanup_sandbox_process_group(
    child: &mut std::process::Child,
    already_terminated: bool,
) -> anyhow::Result<()> {
    if already_terminated {
        return Ok(());
    }

    #[cfg(unix)]
    {
        signal_sandbox_process_group(child, SIGTERM);
        thread::sleep(Duration::from_millis(100));
        signal_sandbox_process_group(child, SIGKILL);
    }

    #[cfg(not(unix))]
    {
        let _ = child;
    }

    Ok(())
}

fn terminate_sandbox_child(child: &mut std::process::Child) -> anyhow::Result<Option<ExitStatus>> {
    #[cfg(unix)]
    {
        signal_sandbox_process_group(child, SIGTERM);
    }
    #[cfg(not(unix))]
    {
        kill_direct_sandbox_child(child)?;
    }

    if let Some(status) = wait_for_sandbox_child_exit(child, Duration::from_millis(500))? {
        #[cfg(unix)]
        signal_sandbox_process_group(child, SIGKILL);
        return Ok(Some(status));
    }

    #[cfg(unix)]
    signal_sandbox_process_group(child, SIGKILL);
    kill_direct_sandbox_child(child)?;
    wait_for_sandbox_child_exit(child, Duration::from_millis(500))
}

fn kill_direct_sandbox_child(child: &mut std::process::Child) -> anyhow::Result<()> {
    match child.kill() {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::InvalidInput => Ok(()),
        Err(error) => Err(error.into()),
    }
}

fn wait_for_sandbox_child_exit(
    child: &mut std::process::Child,
    timeout: Duration,
) -> anyhow::Result<Option<ExitStatus>> {
    let started = Instant::now();
    while started.elapsed() < timeout {
        if let Some(status) = child.try_wait()? {
            return Ok(Some(status));
        }
        thread::sleep(Duration::from_millis(25));
    }

    Ok(None)
}

#[cfg(unix)]
fn signal_sandbox_process_group(child: &std::process::Child, signal: i32) {
    let signal = match signal {
        SIGTERM => "-TERM",
        SIGKILL => "-KILL",
        _ => return,
    };
    let group = format!("-{}", child.id());
    let status = Command::new("/bin/kill")
        .args([signal, "--", group.as_str()])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    if !status.as_ref().is_ok_and(std::process::ExitStatus::success) {
        let _ = Command::new("/bin/kill")
            .args([signal, group.as_str()])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn failed_create_leaves_no_workspace_residue() {
        let repo = tempfile::tempdir().expect("temp repo");
        let stage = PrivateStage::new(repo.path()).expect("stage");
        let operation = MutationOperation::Create {
            path: "created.txt".to_string(),
        };
        stage.populate(&operation).expect("prepare stage");
        assert!(stage.validate(&operation).is_err());
        assert!(!repo.path().join("created.txt").exists());
    }

    #[test]
    fn exact_update_only_applies_the_declared_target() {
        let repo = tempfile::tempdir().expect("temp repo");
        fs::write(repo.path().join("allowed.txt"), "old").expect("allowed input");
        fs::write(repo.path().join("sibling.txt"), "keep").expect("sibling input");
        let stage = PrivateStage::new(repo.path()).expect("stage");
        let operation = MutationOperation::Update {
            path: "allowed.txt".to_string(),
        };
        stage.populate(&operation).expect("prepare stage");
        fs::write(stage.staged("allowed.txt"), "new").expect("stage output");
        stage.validate(&operation).expect("exact manifest");
        stage.apply(&operation).expect("apply staged target");
        assert_eq!(
            fs::read_to_string(repo.path().join("allowed.txt"))
                .expect("allowed target should remain readable"),
            "new"
        );
        assert_eq!(
            fs::read_to_string(repo.path().join("sibling.txt"))
                .expect("sibling target should remain readable"),
            "keep"
        );
    }

    #[test]
    fn delete_move_mkdir_and_rmdir_apply_declared_manifests() {
        let repo = tempfile::tempdir().expect("temp repo");
        fs::write(repo.path().join("delete.txt"), "delete").expect("test operation should succeed");
        fs::write(repo.path().join("old.txt"), "move").expect("test operation should succeed");
        fs::create_dir(repo.path().join("empty")).expect("test operation should succeed");
        let delete = MutationOperation::Delete {
            path: "delete.txt".to_string(),
            entry_only: false,
        };
        let stage = PrivateStage::new(repo.path()).expect("test operation should succeed");
        stage
            .populate(&delete)
            .expect("test operation should succeed");
        fs::remove_file(stage.staged("delete.txt")).expect("test operation should succeed");
        stage
            .validate(&delete)
            .expect("test operation should succeed");
        stage.apply(&delete).expect("test operation should succeed");
        let move_operation = MutationOperation::Move {
            old_path: "old.txt".to_string(),
            new_path: "new.txt".to_string(),
            entry_only: false,
        };
        let stage = PrivateStage::new(repo.path()).expect("test operation should succeed");
        stage
            .populate(&move_operation)
            .expect("test operation should succeed");
        fs::rename(stage.staged("old.txt"), stage.staged("new.txt"))
            .expect("test operation should succeed");
        stage
            .validate(&move_operation)
            .expect("test operation should succeed");
        stage
            .apply(&move_operation)
            .expect("test operation should succeed");
        let mkdir = MutationOperation::Mkdir {
            path: "made".to_string(),
        };
        let stage = PrivateStage::new(repo.path()).expect("test operation should succeed");
        stage
            .populate(&mkdir)
            .expect("test operation should succeed");
        fs::create_dir(stage.staged("made")).expect("test operation should succeed");
        stage
            .validate(&mkdir)
            .expect("test operation should succeed");
        stage.apply(&mkdir).expect("test operation should succeed");
        let rmdir = MutationOperation::Rmdir {
            path: "empty".to_string(),
        };
        let stage = PrivateStage::new(repo.path()).expect("test operation should succeed");
        stage
            .populate(&rmdir)
            .expect("test operation should succeed");
        fs::remove_dir(stage.staged("empty")).expect("test operation should succeed");
        stage
            .validate(&rmdir)
            .expect("test operation should succeed");
        stage.apply(&rmdir).expect("test operation should succeed");
        assert!(!repo.path().join("delete.txt").exists());
        assert!(!repo.path().join("old.txt").exists());
        assert_eq!(
            fs::read_to_string(repo.path().join("new.txt"))
                .expect("moved target should be readable"),
            "move"
        );
        assert!(repo.path().join("made").is_dir());
        assert!(!repo.path().join("empty").exists());
    }

    #[test]
    fn undeclared_sibling_is_never_applied() {
        let repo = tempfile::tempdir().expect("temp repo");
        fs::write(repo.path().join("allowed.txt"), "old").expect("test operation should succeed");
        let stage = PrivateStage::new(repo.path()).expect("test operation should succeed");
        let operation = MutationOperation::Update {
            path: "allowed.txt".to_string(),
        };
        stage
            .populate(&operation)
            .expect("test operation should succeed");
        fs::write(stage.staged("sibling.txt"), "escape").expect("test operation should succeed");
        stage
            .validate(&operation)
            .expect("test operation should succeed");
        stage
            .apply(&operation)
            .expect("test operation should succeed");
        assert!(!repo.path().join("sibling.txt").exists());
    }

    #[test]
    fn outside_and_entry_symlink_operations_are_denied() {
        assert!(normalize_sandbox_target_path("operation", "../outside").is_err());
        let mut operation = MutationOperation::Delete {
            path: "link".to_string(),
            entry_only: true,
        };
        assert!(normalize_mutation_operation(&mut operation).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn symlink_and_hardlink_directory_trees_fail_closed() {
        let repo = tempfile::tempdir().expect("temp repo");
        let tree = repo.path().join("tree");
        fs::create_dir(&tree).expect("test operation should succeed");
        std::os::unix::fs::symlink("/tmp", tree.join("link"))
            .expect("test operation should succeed");
        assert!(validate_directory_tree(&tree).is_err());
        fs::remove_file(tree.join("link")).expect("test operation should succeed");
        fs::write(tree.join("source"), "x").expect("test operation should succeed");
        fs::hard_link(tree.join("source"), tree.join("alias"))
            .expect("test operation should succeed");
        assert!(validate_directory_tree(&tree).is_err());
    }
}
