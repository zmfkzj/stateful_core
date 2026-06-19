use clap::ValueEnum;
use serde_json::Value;
use stateful_core::normalize_relative_path;
use std::{
    collections::BTreeSet,
    error::Error as StdError,
    ffi::OsString,
    fmt, fs,
    io::Read,
    path::{Path, PathBuf},
    process::{Command, ExitStatus, Stdio},
    thread,
    time::{Duration, Instant},
};

#[cfg(unix)]
use std::os::unix::fs::FileTypeExt;
#[cfg(unix)]
use std::os::unix::process::CommandExt;

use crate::{
    CurrentSession, GlobalPaths, HttpResponse, ProtocolEnvelopeArgs, RepoGate, ServerRuntime,
    discover_runtime_with_global, ensure_server, post_json, protocol_envelope,
    read_current_session_file, repo_gate, repo_identity_for_enabled_repo,
    runtime_env_override_is_configured, shadow_guard,
    shell_command::{
        first_word_is_env_assignment, reject_outer_shell_syntax, split_simple_command_words,
    },
};

pub(crate) const STATEFUL_SANDBOX_RUN_ACTIVE_ENV: &str = "STATEFUL_SANDBOX_RUN_ACTIVE";
pub(crate) const STATEFUL_ALLOW_NESTED_SANDBOX_RUN_ENV: &str = "STATEFUL_ALLOW_NESTED_SANDBOX_RUN";
const BUILD_PROFILE_WRITE_DIR: &str = "tmp";
#[cfg(unix)]
const SIGTERM: i32 = 15;
#[cfg(unix)]
const SIGKILL: i32 = 9;

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize, serde::Serialize, ValueEnum)]
pub enum SandboxFsProfile {
    ReadOnly,
    WriteTargets,
    Build,
    Git,
    GithubPr,
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
    pub write_targets: Vec<String>,
    pub create_targets: Vec<String>,
    pub write_dirs: Vec<String>,
    pub command: String,
    pub timeout_seconds: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SandboxProcessFindRequest {
    pub names: Vec<String>,
    pub contains: Vec<String>,
    pub pids: Vec<u32>,
    pub parent_pids: Vec<u32>,
    pub process_groups: Vec<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SandboxRunBashInvocation {
    pub(crate) executable: String,
    pub(crate) request: SandboxRunRequest,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SandboxProcessFindBashInvocation {
    pub(crate) executable: String,
    pub(crate) request: SandboxProcessFindRequest,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ValidatedSandboxDirectCommand {
    Git(Vec<String>),
    GithubPr(Vec<String>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ValidatedSandboxRunShape {
    pub(crate) write_targets: Vec<String>,
    pub(crate) create_targets: Vec<String>,
    pub(crate) write_dirs: Vec<String>,
    pub(crate) direct_command: Option<ValidatedSandboxDirectCommand>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct SandboxRunOutput {
    pub status: &'static str,
    pub exit_code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
    pub allowed_write_targets: Vec<String>,
    pub denied_write_targets: Vec<serde_json::Value>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct SandboxProcessFindOutput {
    pub status: &'static str,
    pub processes: Vec<SandboxProcessInfo>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct SandboxProcessInfo {
    pub pid: u32,
    pub ppid: u32,
    pub pgid: u32,
    pub stat: String,
    pub etime: String,
    pub comm: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SandboxProcessRow {
    info: SandboxProcessInfo,
    command: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SandboxAuthorizationDenied {
    body: String,
}

impl SandboxAuthorizationDenied {
    pub(crate) fn new(body: String) -> Self {
        Self { body }
    }

    pub fn body(&self) -> &str {
        &self.body
    }
}

impl fmt::Display for SandboxAuthorizationDenied {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.body)
    }
}

impl StdError for SandboxAuthorizationDenied {}

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
    fn file(path: PathBuf) -> Self {
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
    let write_targets = shape.write_targets;
    let create_targets = shape.create_targets;
    let write_dirs = shape.write_dirs;
    let direct_command = shape.direct_command;
    if request.fs == SandboxFsProfile::Build {
        shadow_guard::audit_dependency_shadowing(&repo_root)?;
    } else if request.fs == SandboxFsProfile::WriteTargets {
        shadow_guard::check_paths_for_dependency_shadowing(
            &repo_root,
            create_targets.iter().map(String::as_str),
        )?;
    }
    let runtime = if sandbox_profile_requires_runtime(request.fs) {
        if !runtime_env_override_is_configured() {
            ensure_server(paths)?;
        }
        Some(discover_runtime_with_global(&repo_root, paths)?)
    } else {
        None
    };

    let mut allowed_write_targets = Vec::new();
    let mut denied_write_targets = Vec::new();
    let writable_paths = match request.fs {
        SandboxFsProfile::ReadOnly => Vec::new(),
        SandboxFsProfile::WriteTargets => {
            let current_session: CurrentSession =
                read_current_session_file(&repo_root).map_err(|_| {
                    anyhow::anyhow!("sandbox write-targets requires a current stateful session")
                })?;
            let runtime = runtime
                .as_ref()
                .expect("write-targets sandbox profile requires runtime");
            let authorize_context = SandboxAuthorizeContext {
                runtime,
                repo_root: &repo_root,
                paths,
                session_id: &current_session.session_id,
                workspace_id: &current_session.workspace_id,
                network: request.network,
                fs_profile: sandbox_fs_profile_name(request.fs),
            };

            for path in write_targets.iter().chain(create_targets.iter()) {
                let response = authorize_sandbox_write(&authorize_context, "write_file", path)?;
                match classify_sandbox_authorize_response(path, response)? {
                    SandboxAuthorizeDecision::Allow => allowed_write_targets.push(path.clone()),
                    SandboxAuthorizeDecision::Deny(body) => {
                        denied_write_targets.push(serde_json::json!({
                            "path": path,
                            "authorization": body,
                        }));
                    }
                }
            }
            for path in &write_dirs {
                let authorization_path = sandbox_write_dir_display_path(path);
                let response = authorize_sandbox_write(
                    &authorize_context,
                    "write_directory",
                    &authorization_path,
                )?;
                match classify_sandbox_authorize_response(path, response)? {
                    SandboxAuthorizeDecision::Allow => {
                        allowed_write_targets.push(sandbox_write_dir_display_path(path));
                    }
                    SandboxAuthorizeDecision::Deny(body) => {
                        let body = enrich_sandbox_write_dir_denial(body);
                        denied_write_targets.push(serde_json::json!({
                            "path": sandbox_write_dir_display_path(path),
                            "authorization": body,
                        }));
                    }
                }
            }

            if !denied_write_targets.is_empty() {
                let body = serde_json::json!({
                    "status": "error",
                    "message": "stateful sandbox run target authorization denied",
                    "allowed_write_targets": allowed_write_targets,
                    "denied_write_targets": denied_write_targets,
                })
                .to_string();
                return Err(SandboxAuthorizationDenied::new(body).into());
            }

            prepare_sandbox_writable_paths(
                &repo_root,
                &write_targets,
                &create_targets,
                &write_dirs,
            )?
        }
        SandboxFsProfile::Build => {
            let current_session: CurrentSession =
                read_current_session_file(&repo_root).map_err(|_| {
                    anyhow::anyhow!("sandbox build profile requires a current stateful session")
                })?;
            let runtime = runtime
                .as_ref()
                .expect("build sandbox profile requires runtime");
            let authorize_context = SandboxAuthorizeContext {
                runtime,
                repo_root: &repo_root,
                paths,
                session_id: &current_session.session_id,
                workspace_id: &current_session.workspace_id,
                network: request.network,
                fs_profile: sandbox_fs_profile_name(request.fs),
            };
            let build_write_dirs = write_dirs.clone();
            let build_write_dir = build_write_dirs
                .first()
                .expect("build profile validation requires one write dir");
            let authorization_path = sandbox_write_dir_display_path(build_write_dir);
            let response = authorize_sandbox_write(
                &authorize_context,
                "write_directory",
                &authorization_path,
            )?;
            match classify_sandbox_authorize_response(build_write_dir, response)? {
                SandboxAuthorizeDecision::Allow => {
                    allowed_write_targets.push(authorization_path);
                }
                SandboxAuthorizeDecision::Deny(body) => {
                    let body = enrich_sandbox_write_dir_denial(body);
                    denied_write_targets.push(serde_json::json!({
                        "path": sandbox_write_dir_display_path(build_write_dir),
                        "authorization": body,
                    }));
                }
            }

            if !denied_write_targets.is_empty() {
                let body = serde_json::json!({
                    "status": "error",
                    "message": "stateful sandbox run target authorization denied",
                    "allowed_write_targets": allowed_write_targets,
                    "denied_write_targets": denied_write_targets,
                })
                .to_string();
                return Err(SandboxAuthorizationDenied::new(body).into());
            }

            prepare_sandbox_writable_paths(&repo_root, &[], &[], &build_write_dirs)?
        }
        SandboxFsProfile::Git => {
            allowed_write_targets
                .push(sandbox_write_dir_display_path(&repo_root.to_string_lossy()));
            prepare_git_profile_writable_paths(&repo_root)?
        }
        SandboxFsProfile::GithubPr => {
            allowed_write_targets.push(sandbox_write_dir_display_path(
                &github_pr_profile_private_dir(&repo_root).to_string_lossy(),
            ));
            prepare_github_pr_profile_writable_paths(&repo_root)?
        }
    };

    let cwd = resolve_sandbox_cwd(&repo_root)?;
    let timeout = Duration::from_secs(request.timeout_seconds.unwrap_or(300).max(1));
    let result = match direct_command.as_ref() {
        Some(ValidatedSandboxDirectCommand::Git(words)) => {
            let temp_dir = sandbox_temp_dir(&writable_paths)
                .ok_or_else(|| anyhow::anyhow!("git profile requires a temp directory"))?;
            run_sandboxed_git_command(
                words,
                &cwd,
                &writable_paths,
                &temp_dir,
                &git_profile_hooks_dir(&repo_root),
                request.network,
                timeout,
            )?
        }
        Some(ValidatedSandboxDirectCommand::GithubPr(words)) => {
            let temp_dir = sandbox_temp_dir(&writable_paths)
                .ok_or_else(|| anyhow::anyhow!("github-pr profile requires a temp directory"))?;
            run_sandboxed_github_pr_command(
                words,
                &cwd,
                &writable_paths,
                &temp_dir,
                request.network,
                timeout,
            )?
        }
        None => run_sandboxed_command(
            &request.command,
            &cwd,
            &writable_paths,
            &[],
            false,
            request.network,
            timeout,
        )?,
    };

    Ok(SandboxRunOutput {
        status: result.status,
        exit_code: result.exit_code,
        stdout: result.stdout,
        stderr: result.stderr,
        allowed_write_targets,
        denied_write_targets: Vec::new(),
    })
}

pub(crate) fn parse_sandbox_run_bash_invocation(
    command: &str,
) -> Result<SandboxRunBashInvocation, String> {
    reject_outer_shell_syntax(
        command,
        "Bash wrapper must be a single stateful sandbox run command",
    )?;
    let words = split_simple_command_words(command)?;
    if words.is_empty() {
        return Err("Bash commands must use stateful sandbox run".to_string());
    }
    if first_word_is_env_assignment(&words[0]) {
        return Err("Bash wrapper must not use outer environment assignments".to_string());
    }
    if words.len() < 3 || words[1] != "sandbox" || words[2] != "run" {
        return Err("Bash commands must use stateful sandbox run".to_string());
    }

    let mut fs = SandboxFsProfile::ReadOnly;
    let mut network = SandboxNetworkPolicy::Disabled;
    let mut write_targets = Vec::new();
    let mut create_targets = Vec::new();
    let mut write_dirs = Vec::new();
    let mut inner_command = None;
    let mut timeout_seconds = None;
    let mut index = 3;
    while index < words.len() {
        let arg = &words[index];
        match arg.as_str() {
            "--" => {
                return Err("stateful sandbox run does not support argv mode".to_string());
            }
            "--fs" => {
                index += 1;
                let value = parse_sandbox_run_arg_value(&words, index, "--fs")?;
                fs = parse_sandbox_fs_profile(&value)?;
            }
            "--network" => {
                index += 1;
                let value = parse_sandbox_run_arg_value(&words, index, "--network")?;
                network = parse_sandbox_network_policy(&value)?;
            }
            "--write-target" => {
                index += 1;
                write_targets.push(parse_sandbox_run_arg_value(
                    &words,
                    index,
                    "--write-target",
                )?);
            }
            "--create-target" => {
                index += 1;
                create_targets.push(parse_sandbox_run_arg_value(
                    &words,
                    index,
                    "--create-target",
                )?);
            }
            "--write-dir" => {
                index += 1;
                write_dirs.push(parse_sandbox_run_arg_value(&words, index, "--write-dir")?);
            }
            "--command" => {
                if inner_command.is_some() {
                    return Err("stateful sandbox run requires exactly one --command".to_string());
                }
                index += 1;
                inner_command = Some(parse_sandbox_run_arg_value(&words, index, "--command")?);
            }
            "--timeout-seconds" => {
                index += 1;
                let timeout = parse_sandbox_run_arg_value(&words, index, "--timeout-seconds")?;
                timeout_seconds = Some(timeout.parse::<u64>().map_err(|_| {
                    "stateful sandbox run --timeout-seconds requires an integer value".to_string()
                })?);
            }
            _ => {
                return Err(format!("unsupported stateful sandbox run argument `{arg}`"));
            }
        }
        index += 1;
    }

    let Some(command) = inner_command else {
        return Err("stateful sandbox run requires exactly one --command".to_string());
    };

    Ok(SandboxRunBashInvocation {
        executable: words[0].clone(),
        request: SandboxRunRequest {
            fs,
            network,
            write_targets,
            create_targets,
            write_dirs,
            command,
            timeout_seconds,
        },
    })
}

pub(crate) fn parse_sandbox_process_find_bash_invocation(
    command: &str,
) -> Result<SandboxProcessFindBashInvocation, String> {
    reject_outer_shell_syntax(
        command,
        "Bash wrapper must be a single stateful sandbox process find command",
    )?;
    let words = split_simple_command_words(command)?;
    if words.is_empty() {
        return Err("Bash commands must use stateful sandbox process find".to_string());
    }
    if first_word_is_env_assignment(&words[0]) {
        return Err("Bash wrapper must not use outer environment assignments".to_string());
    }
    if words.len() < 4 || words[1] != "sandbox" || words[2] != "process" || words[3] != "find" {
        return Err("Bash commands must use stateful sandbox process find".to_string());
    }

    let mut request = SandboxProcessFindRequest {
        names: Vec::new(),
        contains: Vec::new(),
        pids: Vec::new(),
        parent_pids: Vec::new(),
        process_groups: Vec::new(),
    };
    let mut index = 4;
    while index < words.len() {
        let arg = &words[index];
        match arg.as_str() {
            "--" => {
                return Err("stateful sandbox process find does not support argv mode".to_string());
            }
            "--name" => {
                index += 1;
                request
                    .names
                    .push(parse_sandbox_run_arg_value(&words, index, "--name")?);
            }
            "--contains" => {
                index += 1;
                request
                    .contains
                    .push(parse_sandbox_run_arg_value(&words, index, "--contains")?);
            }
            "--pid" => {
                index += 1;
                request
                    .pids
                    .push(parse_process_selector_arg(&words, index, "--pid")?);
            }
            "--parent-pid" | "--ppid" => {
                index += 1;
                request
                    .parent_pids
                    .push(parse_process_selector_arg(&words, index, arg)?);
            }
            "--process-group" | "--pgid" => {
                index += 1;
                request
                    .process_groups
                    .push(parse_process_selector_arg(&words, index, arg)?);
            }
            _ => {
                return Err(format!(
                    "unsupported stateful sandbox process find argument `{arg}`"
                ));
            }
        }
        index += 1;
    }

    Ok(SandboxProcessFindBashInvocation {
        executable: words[0].clone(),
        request,
    })
}

pub(crate) fn validate_sandbox_run_request_shape(
    request: &SandboxRunRequest,
) -> anyhow::Result<ValidatedSandboxRunShape> {
    if request.command.trim().is_empty() {
        anyhow::bail!("stateful sandbox run requires a non-empty --command");
    }

    let write_targets = normalize_sandbox_target_paths("write_targets", &request.write_targets)?;
    let create_targets = normalize_sandbox_target_paths("create_targets", &request.create_targets)?;
    let write_dirs = normalize_sandbox_target_paths("write_dirs", &request.write_dirs)?;
    validate_profile_network_policy(request.fs, request.network)?;
    validate_profile_targets(request.fs, &write_targets, &create_targets, &write_dirs)?;
    validate_sandbox_run_process_inspection(&request.command)?;
    let direct_command = match request.fs {
        SandboxFsProfile::Git => Some(ValidatedSandboxDirectCommand::Git(
            validate_git_profile_command(&request.command)?,
        )),
        SandboxFsProfile::GithubPr => Some(ValidatedSandboxDirectCommand::GithubPr(
            validate_github_pr_profile_command(&request.command)?,
        )),
        SandboxFsProfile::ReadOnly | SandboxFsProfile::WriteTargets | SandboxFsProfile::Build => {
            None
        }
    };

    Ok(ValidatedSandboxRunShape {
        write_targets,
        create_targets,
        write_dirs,
        direct_command,
    })
}

pub(crate) fn validate_process_find_request(
    request: &SandboxProcessFindRequest,
) -> anyhow::Result<()> {
    if request.names.is_empty()
        && request.contains.is_empty()
        && request.pids.is_empty()
        && request.parent_pids.is_empty()
        && request.process_groups.is_empty()
    {
        anyhow::bail!("stateful sandbox process find requires at least one selector");
    }
    for name in &request.names {
        validate_process_name_selector(name)?;
    }
    for contains in &request.contains {
        validate_process_contains_selector(contains)?;
    }
    for (label, ids) in [
        ("--pid", request.pids.as_slice()),
        ("--parent-pid", request.parent_pids.as_slice()),
        ("--process-group", request.process_groups.as_slice()),
    ] {
        if ids.contains(&0) {
            anyhow::bail!("stateful sandbox process find {label} selectors must be positive");
        }
    }

    Ok(())
}

pub fn run_sandbox_process_find(
    request: SandboxProcessFindRequest,
) -> anyhow::Result<SandboxProcessFindOutput> {
    validate_process_find_request(&request)?;
    let rows = read_process_find_rows()?;
    let processes = filter_process_find_rows(&request, rows);
    Ok(SandboxProcessFindOutput {
        status: "ok",
        processes,
    })
}

fn parse_process_selector_arg(words: &[String], index: usize, arg: &str) -> Result<u32, String> {
    let value = parse_sandbox_run_arg_value(words, index, arg)?;
    value
        .parse::<u32>()
        .map_err(|_| format!("stateful sandbox process find argument `{arg}` requires an integer"))
}

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

fn parse_sandbox_fs_profile(value: &str) -> Result<SandboxFsProfile, String> {
    match value {
        "read-only" => Ok(SandboxFsProfile::ReadOnly),
        "write-targets" => Ok(SandboxFsProfile::WriteTargets),
        "build" => Ok(SandboxFsProfile::Build),
        "git" => Ok(SandboxFsProfile::Git),
        "github-pr" => Ok(SandboxFsProfile::GithubPr),
        _ => Err(
            "stateful sandbox run supports only read-only, write-targets, build, git, and github-pr profiles"
                .to_string(),
        ),
    }
}

fn parse_sandbox_network_policy(value: &str) -> Result<SandboxNetworkPolicy, String> {
    match value {
        "disabled" => Ok(SandboxNetworkPolicy::Disabled),
        "enabled" => Ok(SandboxNetworkPolicy::Enabled),
        _ => Err("stateful sandbox run network must be disabled or enabled".to_string()),
    }
}

pub fn run_external_sandboxed_command(
    command: &str,
    cwd: &Path,
    write_targets: &[PathBuf],
    create_targets: &[PathBuf],
    write_dirs: &[PathBuf],
    connect_sockets: &[PathBuf],
    allow_signal: bool,
    network: SandboxNetworkPolicy,
    timeout: Duration,
) -> anyhow::Result<SandboxCommandResult> {
    if command.trim().is_empty() {
        anyhow::bail!("external-run command is required");
    }

    let writable_paths =
        prepare_external_writable_paths(write_targets, create_targets, write_dirs)?;
    let connect_sockets = prepare_external_connect_sockets(connect_sockets)?;
    let cwd = cwd
        .canonicalize()
        .map_err(|error| anyhow::anyhow!("external-run cwd must exist: {error}"))?;
    if !cwd.is_dir() {
        anyhow::bail!("external-run cwd must be a directory");
    }

    run_sandboxed_command(
        command,
        &cwd,
        &writable_paths,
        &connect_sockets,
        allow_signal,
        network,
        timeout,
    )
}

fn run_sandboxed_command(
    command: &str,
    cwd: &Path,
    writable_paths: &[SandboxWritablePath],
    connect_sockets: &[PathBuf],
    allow_signal: bool,
    network: SandboxNetworkPolicy,
    timeout: Duration,
) -> anyhow::Result<SandboxCommandResult> {
    let temp_dir = sandbox_temp_dir(writable_paths);
    if allow_direct_nested_sandbox_run() {
        let mut command = direct_shell_command(command, cwd);
        apply_sandbox_temp_env(&mut command, temp_dir.as_deref());
        return run_command_with_timeout(command, timeout);
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
                temp_dir.as_deref(),
                network,
            ),
            timeout,
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
            temp_dir,
            network,
            timeout,
        );
        anyhow::bail!("stateful sandbox run is only supported on macOS and Linux");
    }
}

fn run_sandboxed_git_command(
    words: &[String],
    cwd: &Path,
    writable_paths: &[SandboxWritablePath],
    temp_dir: &Path,
    hooks_dir: &Path,
    network: SandboxNetworkPolicy,
    timeout: Duration,
) -> anyhow::Result<SandboxCommandResult> {
    let config = discover_git_profile_config(cwd);

    if allow_direct_nested_sandbox_run() {
        let mut command = direct_git_command(words, cwd);
        apply_git_profile_env(&mut command, temp_dir, hooks_dir, &config);
        return run_command_with_timeout(command, timeout);
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

fn run_sandboxed_github_pr_command(
    words: &[String],
    cwd: &Path,
    writable_paths: &[SandboxWritablePath],
    temp_dir: &Path,
    network: SandboxNetworkPolicy,
    timeout: Duration,
) -> anyhow::Result<SandboxCommandResult> {
    if allow_direct_nested_sandbox_run() {
        let mut command = direct_github_pr_command(words, cwd);
        apply_github_pr_profile_env(&mut command, temp_dir);
        return run_command_with_timeout(command, timeout);
    }

    #[cfg(target_os = "macos")]
    {
        run_command_with_timeout(
            seatbelt_github_pr_command(words, cwd, writable_paths, temp_dir, network),
            timeout,
        )
    }

    #[cfg(target_os = "linux")]
    {
        run_command_with_timeout(
            bubblewrap_github_pr_command(words, cwd, writable_paths, temp_dir, network),
            timeout,
        )
    }

    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        let _ = (words, cwd, writable_paths, temp_dir, network, timeout);
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

fn direct_github_pr_command(words: &[String], cwd: &Path) -> Command {
    let mut direct = Command::new("gh");
    direct.args(&words[1..]).current_dir(cwd);
    direct
}

fn prepare_external_writable_paths(
    write_targets: &[PathBuf],
    create_targets: &[PathBuf],
    write_dirs: &[PathBuf],
) -> anyhow::Result<Vec<SandboxWritablePath>> {
    let create_set = create_targets.iter().collect::<BTreeSet<_>>();

    for path in create_targets {
        ensure_external_file_target(path, true).map_err(|error| {
            anyhow::anyhow!(
                "stateful external-run create target `{}` is unsafe: {error}",
                path.display()
            )
        })?;
    }

    for path in write_targets {
        ensure_external_file_target(path, false).map_err(|error| {
            anyhow::anyhow!(
                "stateful external-run write target `{}` is unsafe: {error}",
                path.display()
            )
        })?;
        if !path.exists() && !create_set.contains(path) {
            anyhow::bail!(
                "stateful external-run write target `{}` must already exist or be listed in create targets",
                path.display()
            );
        }
    }

    for path in write_dirs {
        ensure_external_dir_target(path).map_err(|error| {
            anyhow::anyhow!(
                "stateful external-run write dir `{}` is unsafe: {error}",
                path.display()
            )
        })?;
    }

    for path in create_targets {
        let Some(parent) = path.parent() else {
            anyhow::bail!(
                "stateful external-run create target `{}` has no parent directory",
                path.display()
            );
        };
        if !parent.is_dir() {
            anyhow::bail!(
                "stateful external-run create target parent `{}` is not a directory",
                parent.display()
            );
        }
        if !path.exists() {
            fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(path)?;
        }
    }

    let mut seen = BTreeSet::new();
    let mut writable_paths = Vec::new();
    for path in write_targets.iter().chain(create_targets.iter()) {
        let canonical = path.canonicalize()?;
        if seen.insert(canonical.clone()) {
            writable_paths.push(SandboxWritablePath::file(canonical));
        }
    }
    for path in write_dirs {
        let canonical = path.canonicalize()?;
        if seen.insert(canonical.clone()) {
            writable_paths.push(SandboxWritablePath::directory(canonical));
        }
    }

    Ok(writable_paths)
}

fn prepare_external_connect_sockets(connect_sockets: &[PathBuf]) -> anyhow::Result<Vec<PathBuf>> {
    let mut seen = BTreeSet::new();
    let mut normalized = Vec::new();
    for socket_path in connect_sockets {
        ensure_external_socket_target(socket_path).map_err(|error| {
            anyhow::anyhow!(
                "stateful external-run connect socket `{}` is unsafe: {error}",
                socket_path.display()
            )
        })?;
        let canonical = socket_path.canonicalize()?;
        if seen.insert(canonical.clone()) {
            normalized.push(canonical);
        }
    }

    Ok(normalized)
}

fn ensure_external_socket_target(path: &Path) -> anyhow::Result<()> {
    if !path.is_absolute() {
        anyhow::bail!("external-run connect socket targets must be normalized absolute paths");
    }
    ensure_no_symlinked_existing_components(path)?;
    let metadata = fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() {
        anyhow::bail!("external-run refuses symlink socket targets");
    }
    #[cfg(unix)]
    {
        if !metadata.file_type().is_socket() {
            anyhow::bail!("external-run connect socket target must be a Unix socket");
        }
    }
    #[cfg(not(unix))]
    {
        let _ = metadata;
        anyhow::bail!("external-run connect socket is only supported on Unix");
    }

    Ok(())
}

fn normalize_sandbox_target_paths(field: &str, paths: &[String]) -> anyhow::Result<Vec<String>> {
    let mut seen = BTreeSet::new();
    let mut normalized = Vec::new();
    for path in paths {
        let path = normalize_sandbox_target_path(field, path)?;
        if seen.insert(path.clone()) {
            normalized.push(path);
        }
    }

    Ok(normalized)
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
        if segment.chars().any(char::is_control) {
            anyhow::bail!("stateful sandbox run paths must not contain control characters");
        }
        segments.push(segment);
    }

    if segments.is_empty() {
        anyhow::bail!("stateful sandbox run {field} entries must not be empty");
    }

    Ok(normalize_relative_path(segments.join("/")))
}

fn ensure_repo_file_target(repo_root: &Path, relative_path: &str) -> anyhow::Result<()> {
    let canonical_repo = repo_root
        .canonicalize()
        .unwrap_or_else(|_| repo_root.to_path_buf());
    let target = repo_root.join(relative_path);
    let Some(parent) = Path::new(relative_path).parent() else {
        anyhow::bail!("stateful sandbox run target has no parent directory");
    };

    let mut cursor = repo_root.to_path_buf();
    for component in parent.components() {
        cursor.push(component);
        if let Ok(metadata) = fs::symlink_metadata(&cursor) {
            if metadata.file_type().is_symlink() {
                anyhow::bail!("stateful sandbox run refuses symlinked parent directories");
            }
            if !metadata.is_dir() {
                anyhow::bail!("stateful sandbox run parent path is not a directory");
            }
        }
    }

    if let Ok(metadata) = fs::symlink_metadata(&target) {
        if metadata.file_type().is_symlink() {
            anyhow::bail!("stateful sandbox run refuses symlink file targets");
        }
        if metadata.is_dir() {
            anyhow::bail!("stateful sandbox run target is a directory");
        }
    }

    if let Some(parent) = target.parent()
        && parent.exists()
    {
        let canonical_parent = parent.canonicalize()?;
        if !canonical_parent.starts_with(canonical_repo) {
            anyhow::bail!("stateful sandbox run parent path escapes the repo");
        }
    }

    Ok(())
}

pub(crate) fn ensure_repo_dir_target(repo_root: &Path, relative_path: &str) -> anyhow::Result<()> {
    let canonical_repo = repo_root
        .canonicalize()
        .unwrap_or_else(|_| repo_root.to_path_buf());
    let target = repo_root.join(relative_path);

    let mut cursor = repo_root.to_path_buf();
    for component in Path::new(relative_path).components() {
        cursor.push(component);
        if let Ok(metadata) = fs::symlink_metadata(&cursor) {
            if metadata.file_type().is_symlink() {
                anyhow::bail!("stateful sandbox run refuses symlinked directory targets");
            }
            if !metadata.is_dir() {
                anyhow::bail!("stateful sandbox run directory target path is not a directory");
            }
        }
    }

    if let Some(parent) = target.parent()
        && parent.exists()
    {
        let canonical_parent = parent.canonicalize()?;
        if !canonical_parent.starts_with(canonical_repo) {
            anyhow::bail!("stateful sandbox run directory target escapes the repo");
        }
    }

    Ok(())
}

fn ensure_external_file_target(path: &Path, allow_missing_file: bool) -> anyhow::Result<()> {
    if !path.is_absolute() {
        anyhow::bail!("external-run targets must be normalized absolute paths");
    }
    ensure_no_symlinked_existing_components(path)?;
    let Some(parent) = path.parent() else {
        anyhow::bail!("external-run target has no parent directory");
    };
    if !parent.is_dir() {
        anyhow::bail!("external-run target parent is not a directory");
    }

    match fs::symlink_metadata(path) {
        Ok(metadata) => {
            if metadata.file_type().is_symlink() {
                anyhow::bail!("external-run refuses symlink file targets");
            }
            if metadata.is_dir() {
                anyhow::bail!("external-run file target is a directory");
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound && allow_missing_file => {}
        Err(error) => return Err(error.into()),
    }

    Ok(())
}

fn ensure_external_dir_target(path: &Path) -> anyhow::Result<()> {
    if !path.is_absolute() {
        anyhow::bail!("external-run targets must be normalized absolute paths");
    }
    ensure_no_symlinked_existing_components(path)?;
    let metadata = fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() {
        anyhow::bail!("external-run refuses symlink directory targets");
    }
    if !metadata.is_dir() {
        anyhow::bail!("external-run directory target is not a directory");
    }

    Ok(())
}

fn ensure_no_symlinked_existing_components(path: &Path) -> anyhow::Result<()> {
    let mut cursor = PathBuf::new();
    for component in path.components() {
        cursor.push(component.as_os_str());
        match fs::symlink_metadata(&cursor) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                anyhow::bail!("external-run refuses symlinked target components");
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => break,
            Err(error) => return Err(error.into()),
        }
    }

    Ok(())
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

fn prepare_sandbox_writable_paths(
    repo_root: &Path,
    write_targets: &[String],
    create_targets: &[String],
    write_dirs: &[String],
) -> anyhow::Result<Vec<SandboxWritablePath>> {
    let create_set = create_targets
        .iter()
        .map(String::as_str)
        .collect::<BTreeSet<_>>();

    for path in create_targets {
        ensure_repo_file_target(repo_root, path).map_err(|error| {
            anyhow::anyhow!("stateful sandbox run create target `{path}` is unsafe: {error}")
        })?;
    }

    for path in write_targets {
        ensure_repo_file_target(repo_root, path).map_err(|error| {
            anyhow::anyhow!("stateful sandbox run write target `{path}` is unsafe: {error}")
        })?;
        let target = repo_root.join(path);
        if !target.exists() && !create_set.contains(path.as_str()) {
            anyhow::bail!(
                "stateful sandbox run write target `{path}` must already exist or be listed in create_targets"
            );
        }
    }

    for path in write_dirs {
        ensure_repo_dir_target(repo_root, path).map_err(|error| {
            anyhow::anyhow!("stateful sandbox run write dir `{path}` is unsafe: {error}")
        })?;
    }

    for path in write_dirs {
        let target = repo_root.join(path);
        fs::create_dir_all(&target)?;
        fs::create_dir_all(target.join(".stateful-tmp"))?;
    }

    for path in create_targets {
        let target = repo_root.join(path);
        let Some(parent) = target.parent() else {
            anyhow::bail!("stateful sandbox run create target `{path}` has no parent directory");
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
    let mut writable_paths = Vec::new();
    for path in write_targets.iter().chain(create_targets.iter()) {
        if !seen.insert(path.clone()) {
            continue;
        }
        let canonical = repo_root.join(path).canonicalize().map_err(|error| {
            anyhow::anyhow!("stateful sandbox run target `{path}` must exist: {error}")
        })?;
        writable_paths.push(SandboxWritablePath::file(canonical));
    }

    for path in write_dirs {
        let key = format!("{path}/");
        if !seen.insert(key) {
            continue;
        }
        let canonical = repo_root.join(path).canonicalize().map_err(|error| {
            anyhow::anyhow!("stateful sandbox run write dir `{path}` must exist: {error}")
        })?;
        writable_paths.push(SandboxWritablePath::directory(canonical));
    }

    Ok(writable_paths)
}

fn validate_profile_targets(
    fs: SandboxFsProfile,
    write_targets: &[String],
    create_targets: &[String],
    write_dirs: &[String],
) -> anyhow::Result<()> {
    match fs {
        SandboxFsProfile::ReadOnly => {
            if !write_targets.is_empty() || !create_targets.is_empty() || !write_dirs.is_empty() {
                anyhow::bail!(
                    "read-only profile rejects write targets, create targets, and write dirs"
                );
            }
        }
        SandboxFsProfile::WriteTargets => {
            if write_targets.is_empty() && create_targets.is_empty() && write_dirs.is_empty() {
                anyhow::bail!(
                    "write-targets profile requires at least one write target, create target, or write dir"
                );
            }
            for path in write_dirs {
                ensure_artifact_write_dir_target(path)?;
            }
        }
        SandboxFsProfile::Build => {
            if !write_targets.is_empty() || !create_targets.is_empty() {
                anyhow::bail!("build profile rejects explicit write targets and create targets");
            }
            if write_dirs.len() != 1 {
                anyhow::bail!(
                    "build profile requires exactly one scoped artifact directory with --write-dir tmp/<purpose>"
                );
            }
            ensure_artifact_write_dir_target(&write_dirs[0])?;
        }
        SandboxFsProfile::Git => {
            if !write_targets.is_empty() || !create_targets.is_empty() || !write_dirs.is_empty() {
                anyhow::bail!(
                    "git profile manages repo writes automatically and rejects explicit write targets, create targets, and write dirs"
                );
            }
        }
        SandboxFsProfile::GithubPr => {
            if !write_targets.is_empty() || !create_targets.is_empty() || !write_dirs.is_empty() {
                anyhow::bail!(
                    "github-pr profile manages transient PR state automatically and rejects explicit write targets, create targets, and write dirs"
                );
            }
        }
    }
    Ok(())
}

fn validate_profile_network_policy(
    fs: SandboxFsProfile,
    network: SandboxNetworkPolicy,
) -> anyhow::Result<()> {
    if fs == SandboxFsProfile::ReadOnly && network == SandboxNetworkPolicy::Enabled {
        anyhow::bail!("read-only sandbox run requires --network disabled");
    }
    if fs == SandboxFsProfile::GithubPr && network != SandboxNetworkPolicy::Enabled {
        anyhow::bail!("github-pr sandbox run requires --network enabled");
    }

    Ok(())
}

fn validate_process_name_selector(name: &str) -> anyhow::Result<()> {
    if name.trim().is_empty() {
        anyhow::bail!("stateful sandbox process find --name must not be empty");
    }
    if !name
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
    {
        anyhow::bail!("stateful sandbox process find --name contains unsupported characters");
    }
    Ok(())
}

fn validate_process_contains_selector(contains: &str) -> anyhow::Result<()> {
    let trimmed = contains.trim();
    if trimmed.len() < 3 {
        anyhow::bail!("stateful sandbox process find --contains must be at least 3 characters");
    }
    if trimmed.chars().any(char::is_control) {
        anyhow::bail!("stateful sandbox process find --contains contains control characters");
    }
    Ok(())
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

fn read_process_find_rows() -> anyhow::Result<Vec<SandboxProcessRow>> {
    let output = Command::new("/bin/ps")
        .args(["-axo", "pid=,ppid=,pgid=,stat=,etime=,comm=,command="])
        .output()
        .or_else(|_| {
            Command::new("ps")
                .args(["-axo", "pid=,ppid=,pgid=,stat=,etime=,comm=,command="])
                .output()
        })?;
    if !output.status.success() {
        anyhow::bail!(
            "stateful sandbox process find failed to inspect processes: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }

    parse_process_find_ps_output(&String::from_utf8_lossy(&output.stdout))
}

fn parse_process_find_ps_output(output: &str) -> anyhow::Result<Vec<SandboxProcessRow>> {
    let mut rows = Vec::new();
    for line in output.lines() {
        let fields = line.split_whitespace().collect::<Vec<_>>();
        if fields.len() < 6 {
            continue;
        }
        let pid = parse_process_find_field(fields[0], "pid")?;
        let ppid = parse_process_find_field(fields[1], "ppid")?;
        let pgid = parse_process_find_field(fields[2], "pgid")?;
        let stat = fields[3].to_string();
        let etime = fields[4].to_string();
        let comm = fields[5].to_string();
        let command = if fields.len() > 6 {
            fields[6..].join(" ")
        } else {
            comm.clone()
        };
        rows.push(SandboxProcessRow {
            info: SandboxProcessInfo {
                pid,
                ppid,
                pgid,
                stat,
                etime,
                comm,
            },
            command,
        });
    }
    Ok(rows)
}

fn parse_process_find_field(value: &str, field: &str) -> anyhow::Result<u32> {
    value.parse::<u32>().map_err(|_| {
        anyhow::anyhow!("stateful sandbox process find invalid {field} field `{value}`")
    })
}

fn filter_process_find_rows(
    request: &SandboxProcessFindRequest,
    rows: Vec<SandboxProcessRow>,
) -> Vec<SandboxProcessInfo> {
    rows.into_iter()
        .filter(|row| row.info.pid != std::process::id())
        .filter(|row| process_find_row_matches(request, row))
        .map(|row| row.info)
        .collect()
}

fn process_find_row_matches(request: &SandboxProcessFindRequest, row: &SandboxProcessRow) -> bool {
    (request.names.is_empty()
        || request
            .names
            .iter()
            .any(|name| row.info.comm == *name || process_comm_basename(&row.info.comm) == name))
        && (request.contains.is_empty()
            || request
                .contains
                .iter()
                .any(|contains| row.command.contains(contains)))
        && (request.pids.is_empty() || request.pids.contains(&row.info.pid))
        && (request.parent_pids.is_empty() || request.parent_pids.contains(&row.info.ppid))
        && (request.process_groups.is_empty() || request.process_groups.contains(&row.info.pgid))
}

fn process_comm_basename(comm: &str) -> &str {
    Path::new(comm)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(comm)
}

fn sandbox_fs_profile_name(fs: SandboxFsProfile) -> &'static str {
    match fs {
        SandboxFsProfile::ReadOnly => "read-only",
        SandboxFsProfile::WriteTargets => "write-targets",
        SandboxFsProfile::Build => "build",
        SandboxFsProfile::Git => "git",
        SandboxFsProfile::GithubPr => "github-pr",
    }
}

fn sandbox_profile_requires_runtime(fs: SandboxFsProfile) -> bool {
    !matches!(fs, SandboxFsProfile::ReadOnly)
}

fn git_profile_writable_paths(repo_root: &Path) -> Vec<SandboxWritablePath> {
    vec![
        SandboxWritablePath::directory(git_profile_private_dir(repo_root)),
        SandboxWritablePath::directory(repo_root.to_path_buf()),
    ]
}

fn prepare_git_profile_writable_paths(
    repo_root: &Path,
) -> anyhow::Result<Vec<SandboxWritablePath>> {
    let writable_paths = git_profile_writable_paths(repo_root);
    let temp_dir = sandbox_temp_dir(&writable_paths)
        .ok_or_else(|| anyhow::anyhow!("git profile requires a temp directory"))?;
    fs::create_dir_all(&temp_dir)?;
    fs::create_dir_all(git_profile_hooks_dir(repo_root))?;
    Ok(writable_paths)
}

fn github_pr_profile_private_dir(repo_root: &Path) -> PathBuf {
    let dot_git = repo_root.join(".git");
    if dot_git.is_dir() || !dot_git.exists() {
        dot_git.join("stateful").join("github-pr")
    } else {
        repo_root.join(".stateful-git").join("github-pr")
    }
}

fn github_pr_profile_writable_paths(repo_root: &Path) -> Vec<SandboxWritablePath> {
    vec![SandboxWritablePath::directory(
        github_pr_profile_private_dir(repo_root),
    )]
}

fn prepare_github_pr_profile_writable_paths(
    repo_root: &Path,
) -> anyhow::Result<Vec<SandboxWritablePath>> {
    let writable_paths = github_pr_profile_writable_paths(repo_root);
    let temp_dir = sandbox_temp_dir(&writable_paths)
        .ok_or_else(|| anyhow::anyhow!("github-pr profile requires a temp directory"))?;
    fs::create_dir_all(&temp_dir)?;
    Ok(writable_paths)
}

fn git_profile_private_dir(repo_root: &Path) -> PathBuf {
    let dot_git = repo_root.join(".git");
    if dot_git.is_dir() || !dot_git.exists() {
        dot_git.join("stateful")
    } else {
        repo_root.join(".stateful-git")
    }
}

fn git_profile_hooks_dir(repo_root: &Path) -> PathBuf {
    git_profile_private_dir(repo_root).join("hooks-disabled")
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

fn validate_github_pr_profile_command(command: &str) -> anyhow::Result<Vec<String>> {
    parse_github_pr_profile_command(command).map_err(|reason| {
        anyhow::anyhow!("github-pr profile requires a single gh pr command: {reason}")
    })
}

pub(crate) fn parse_github_pr_profile_command(command: &str) -> Result<Vec<String>, String> {
    reject_direct_profile_shell_syntax(command)
        .map_err(|reason| format!("github-pr profile requires a single gh pr command: {reason}"))?;
    let words = split_git_profile_command_words(command)
        .map_err(|reason| format!("github-pr profile requires a single gh pr command: {reason}"))?;
    validate_github_pr_profile_words(&words)?;
    Ok(words)
}

pub(crate) fn parse_git_profile_command(command: &str) -> Result<Vec<String>, String> {
    reject_direct_profile_shell_syntax(command)
        .map_err(|reason| format!("git profile requires a single git command: {reason}"))?;
    let words = split_git_profile_command_words(command)
        .map_err(|reason| format!("git profile requires a single git command: {reason}"))?;
    if words.first().is_none_or(|word| word != "git") {
        return Err("git profile requires a single git command".to_string());
    }
    validate_git_profile_words(&words)?;
    Ok(words)
}

fn validate_github_pr_profile_words(words: &[String]) -> Result<(), String> {
    if words.len() < 3 || words[0] != "gh" || words[1] != "pr" {
        return Err("github-pr profile requires a single gh pr command".to_string());
    }

    let subcommand = words[2].as_str();
    if !GITHUB_PR_PROFILE_ALLOWED_SUBCOMMANDS.contains(&subcommand) {
        return Err(format!(
            "github-pr profile does not allow gh pr subcommand `{subcommand}`"
        ));
    }

    for word in &words[3..] {
        if let Some(flag) = github_pr_profile_denied_flag(word) {
            return Err(format!(
                "github-pr profile does not allow interactive/browser flag `{flag}`"
            ));
        }
    }

    Ok(())
}

fn github_pr_profile_denied_flag(word: &str) -> Option<String> {
    let flag = word.split_once('=').map_or(word, |(flag, _)| flag);
    if GITHUB_PR_PROFILE_DENIED_FLAGS.contains(&flag) {
        return Some(flag.to_string());
    }
    if flag.starts_with("--") {
        return None;
    }

    flag.strip_prefix('-')?.chars().find_map(|short| {
        GITHUB_PR_PROFILE_DENIED_SHORT_FLAGS
            .contains(&short)
            .then(|| format!("-{short}"))
    })
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
            return Err("git profile does not allow git path or exec overrides".to_string());
        }
        if word.starts_with('-') {
            index += 1;
            continue;
        }
        return Ok((index, word));
    }
    Err("git profile requires an explicit git subcommand".to_string())
}

fn validate_git_profile_subcommand_args(subcommand: &str, args: &[String]) -> Result<(), String> {
    if subcommand == "remote" {
        return validate_git_profile_remote_args(args);
    }

    for arg in args {
        match subcommand {
            "branch" if is_git_profile_branch_config_persistence_arg(arg) => {
                return Err("git profile does not allow branch config persistence".to_string());
            }
            "checkout" | "switch" if is_git_profile_tracking_arg(arg) => {
                return Err(
                    "git profile does not allow branch tracking config persistence".to_string(),
                );
            }
            "grep"
                if arg == "-O"
                    || arg.starts_with("-O")
                    || arg == "--open-files-in-pager"
                    || arg.starts_with("--open-files-in-pager=") =>
            {
                return Err("git profile does not allow grep pager dispatch".to_string());
            }
            "archive" if arg == "--exec" || arg.starts_with("--exec=") => {
                return Err("git profile does not allow archive --exec".to_string());
            }
            "fetch" | "pull" if arg == "--upload-pack" || arg.starts_with("--upload-pack=") => {
                return Err("git profile does not allow upload-pack overrides".to_string());
            }
            "push"
                if arg == "--receive-pack"
                    || arg.starts_with("--receive-pack=")
                    || arg == "--exec"
                    || arg.starts_with("--exec=") =>
            {
                return Err("git profile does not allow receive-pack overrides".to_string());
            }
            "push" if is_git_profile_push_set_upstream_arg(arg) => {
                return Err("git profile does not allow push upstream persistence".to_string());
            }
            "rebase" if arg == "--exec" || arg == "-x" || arg.starts_with("--exec=") => {
                return Err("git profile does not allow rebase --exec".to_string());
            }
            _ => {}
        }
    }
    Ok(())
}

fn is_git_profile_branch_config_persistence_arg(arg: &str) -> bool {
    arg == "--set-upstream-to"
        || arg.starts_with("--set-upstream-to=")
        || arg == "--set-upstream"
        || arg.starts_with("--set-upstream=")
        || arg == "--unset-upstream"
        || is_git_profile_tracking_arg(arg)
        || is_git_profile_short_option(arg, 'u')
}

fn is_git_profile_tracking_arg(arg: &str) -> bool {
    arg == "--track" || arg.starts_with("--track=") || is_git_profile_short_option(arg, 't')
}

fn is_git_profile_push_set_upstream_arg(arg: &str) -> bool {
    arg == "--set-upstream"
        || arg.starts_with("--set-upstream=")
        || is_git_profile_short_option(arg, 'u')
}

fn is_git_profile_short_option(arg: &str, short: char) -> bool {
    let Some(rest) = arg.strip_prefix('-') else {
        return false;
    };
    !rest.starts_with('-') && rest.starts_with(short)
}

fn validate_git_profile_remote_args(args: &[String]) -> Result<(), String> {
    let mut index = 0;
    while let Some(arg) = args.get(index) {
        match arg.as_str() {
            "-v" | "--verbose" => index += 1,
            arg if arg.starts_with('-') => {
                return Err(format!("git profile does not allow remote option `{arg}`"));
            }
            _ => break,
        }
    }

    let Some(action) = args.get(index).map(String::as_str) else {
        return Ok(());
    };

    match action {
        "get-url" => validate_git_profile_remote_get_url_args(&args[index + 1..]),
        "show" => validate_git_profile_remote_show_args(&args[index + 1..]),
        "add" | "rename" | "remove" | "rm" | "set-branches" | "set-head" | "set-url" | "update"
        | "prune" => Err("git profile does not allow remote metadata mutation".to_string()),
        _ => Err(format!(
            "git profile does not allow remote subcommand `{action}`"
        )),
    }
}

fn validate_git_profile_remote_get_url_args(args: &[String]) -> Result<(), String> {
    for arg in args {
        if arg == "--all" || arg == "--push" || !arg.starts_with('-') {
            continue;
        }
        return Err(format!(
            "git profile does not allow remote get-url option `{arg}`"
        ));
    }
    Ok(())
}

fn validate_git_profile_remote_show_args(args: &[String]) -> Result<(), String> {
    for arg in args {
        if arg == "-n" || !arg.starts_with('-') {
            continue;
        }
        return Err(format!(
            "git profile does not allow remote show option `{arg}`"
        ));
    }
    Ok(())
}

const GIT_PROFILE_ALLOWED_SUBCOMMANDS: &[&str] = &[
    "add",
    "am",
    "apply",
    "archive",
    "blame",
    "branch",
    "cat-file",
    "checkout",
    "cherry-pick",
    "clean",
    "commit",
    "describe",
    "diff",
    "fetch",
    "grep",
    "log",
    "ls-files",
    "merge",
    "mv",
    "pull",
    "push",
    "rebase",
    "remote",
    "reset",
    "restore",
    "rev-list",
    "rev-parse",
    "revert",
    "rm",
    "show",
    "stash",
    "status",
    "switch",
    "tag",
];

const GITHUB_PR_PROFILE_ALLOWED_SUBCOMMANDS: &[&str] = &["create", "list", "status", "view"];
const GITHUB_PR_PROFILE_DENIED_FLAGS: &[&str] = &["--editor", "--recover", "--web"];
const GITHUB_PR_PROFILE_DENIED_SHORT_FLAGS: &[char] = &['w', 'e'];

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

fn split_git_profile_command_words(command: &str) -> Result<Vec<String>, String> {
    let mut words = Vec::new();
    let mut current = String::new();
    let mut state = ShellQuoteState::None;
    let mut in_word = false;

    for ch in command.chars() {
        match state {
            ShellQuoteState::None => match ch {
                '\'' => {
                    state = ShellQuoteState::Single;
                    in_word = true;
                }
                '"' => {
                    state = ShellQuoteState::Double;
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
            ShellQuoteState::Single => {
                if ch == '\'' {
                    state = ShellQuoteState::None;
                } else {
                    current.push(ch);
                }
            }
            ShellQuoteState::Double => {
                if ch == '"' {
                    state = ShellQuoteState::None;
                } else {
                    current.push(ch);
                }
            }
        }
    }

    if state != ShellQuoteState::None {
        return Err("unterminated quotes".to_string());
    }
    if in_word {
        words.push(current);
    }

    Ok(words)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ShellQuoteState {
    None,
    Single,
    Double,
}

fn ensure_artifact_write_dir_target(relative_path: &str) -> anyhow::Result<()> {
    let trimmed = relative_path.trim_end_matches('/');
    let mut components = trimmed.split('/');
    let top_level = components.next().unwrap_or_default();
    if top_level != BUILD_PROFILE_WRITE_DIR {
        anyhow::bail!(
            "stateful sandbox run --write-dir is limited to the tmp/ artifact tree; use native Codex edit tools or exact file write targets for source-tree edits"
        );
    }
    if components.next().is_none() {
        anyhow::bail!(
            "stateful sandbox run --write-dir cannot target tmp directly; use --write-dir tmp/<purpose>"
        );
    }
    Ok(())
}

pub(crate) fn sandbox_write_dir_display_path(path: &str) -> String {
    format!("{}/", path.trim_end_matches('/'))
}

pub(crate) fn enrich_sandbox_write_dir_denial(mut body: Value) -> Value {
    let is_unsupported_action = body
        .get("reason_code")
        .and_then(Value::as_str)
        .is_some_and(|reason_code| reason_code == "unsupported_action");

    if is_unsupported_action {
        body["message"] = Value::String(
            "The running stateful server does not support sandbox write directories.".to_string(),
        );
        body["required_next_action"] = Value::String(
            "Restart the stateful server with the newly built stateful binary, then retry the sandbox run.".to_string(),
        );
    }

    body
}

pub(crate) fn sandbox_temp_dir(writable_paths: &[SandboxWritablePath]) -> Option<PathBuf> {
    writable_paths
        .iter()
        .find(|path| path.kind == SandboxWritablePathKind::Directory)
        .map(|path| path.path.join(".stateful-tmp"))
}

pub(crate) fn apply_sandbox_temp_env(command: &mut Command, temp_dir: Option<&Path>) {
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

pub(crate) struct SandboxAuthorizeContext<'a> {
    pub(crate) runtime: &'a ServerRuntime,
    pub(crate) repo_root: &'a Path,
    pub(crate) paths: &'a GlobalPaths,
    pub(crate) session_id: &'a str,
    pub(crate) workspace_id: &'a str,
    pub(crate) network: SandboxNetworkPolicy,
    pub(crate) fs_profile: &'static str,
}

pub(crate) fn authorize_sandbox_write(
    context: &SandboxAuthorizeContext<'_>,
    action: &str,
    path: &str,
) -> anyhow::Result<HttpResponse> {
    let body = protocol_envelope(ProtocolEnvelopeArgs {
        runtime: context.runtime,
        request_id: uuid::Uuid::new_v4().to_string(),
        session_id: context.session_id.to_string(),
        workspace_id: context.workspace_id.to_string(),
        identity: repo_identity_for_enabled_repo(context.paths, context.repo_root).ok(),
        source_kind: "cli",
        event: "sandbox_run",
        source_ref: "stateful.sandbox.run",
        source_tool_name: None,
        payload: serde_json::json!({
            "action": action,
            "path": path,
            "purpose": sandbox_authorize_purpose(action, path),
            "queue_on_conflict": true,
            "fs_profile": context.fs_profile,
            "network_policy": match context.network {
                SandboxNetworkPolicy::Disabled => "disabled",
                SandboxNetworkPolicy::Enabled => "enabled",
            },
        }),
    });

    post_json(context.runtime, "/v1/authorize", &body)
}

fn sandbox_authorize_purpose(action: &str, path: &str) -> String {
    match action {
        "write_directory" => format!("Run sandbox command for write directory `{path}`."),
        _ => format!("Run sandbox command for write target `{path}`."),
    }
}

pub(crate) enum SandboxAuthorizeDecision {
    Allow,
    Deny(Value),
}

pub(crate) fn classify_sandbox_authorize_response(
    path: &str,
    response: HttpResponse,
) -> anyhow::Result<SandboxAuthorizeDecision> {
    if !(200..300).contains(&response.status_code) {
        anyhow::bail!(
            "stateful sandbox run authorize request for `{path}` failed with HTTP {}: {}",
            response.status_code,
            response.body
        );
    }

    let body = serde_json::from_str::<Value>(&response.body).map_err(|error| {
        anyhow::anyhow!(
            "stateful sandbox run authorize response for `{path}` was not valid JSON: {error}"
        )
    })?;

    match body.get("decision").and_then(Value::as_str) {
        Some("allow") => Ok(SandboxAuthorizeDecision::Allow),
        Some("deny") => Ok(SandboxAuthorizeDecision::Deny(body)),
        Some(decision) => {
            anyhow::bail!(
                "stateful sandbox run authorize response for `{path}` returned unsupported decision `{decision}`"
            );
        }
        None => {
            anyhow::bail!("stateful sandbox run authorize response for `{path}` missing decision");
        }
    }
}

fn is_git_internal_segment(segment: &str) -> bool {
    segment.eq_ignore_ascii_case(".git")
}

#[cfg(target_os = "macos")]
fn seatbelt_command(
    command: &str,
    cwd: &Path,
    writable_paths: &[SandboxWritablePath],
    connect_sockets: &[PathBuf],
    allow_signal: bool,
    temp_dir: Option<&Path>,
    network: SandboxNetworkPolicy,
) -> Command {
    let profile = seatbelt_profile_with_connect_sockets(
        writable_paths,
        connect_sockets,
        allow_signal,
        network,
    );
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

#[cfg(target_os = "macos")]
fn seatbelt_github_pr_command(
    words: &[String],
    cwd: &Path,
    writable_paths: &[SandboxWritablePath],
    temp_dir: &Path,
    network: SandboxNetworkPolicy,
) -> Command {
    let profile = seatbelt_profile(writable_paths, network);
    let mut sandbox = Command::new("/usr/bin/sandbox-exec");
    sandbox
        .arg("-p")
        .arg(profile)
        .arg("gh")
        .args(&words[1..])
        .current_dir(cwd);
    apply_github_pr_profile_env(&mut sandbox, temp_dir);
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
    profile.push_str(
        "(allow mach-lookup\n\
             (global-name \"com.apple.system.opendirectoryd.libinfo\")\n\
             (global-name \"com.apple.system.DirectoryService.libinfo_v1\")\n\
             (global-name \"com.apple.trustd\")\n\
             (global-name \"com.apple.trustd.agent\"))\n",
    );
    profile
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

#[cfg(target_os = "linux")]
fn bubblewrap_github_pr_command(
    words: &[String],
    cwd: &Path,
    writable_paths: &[SandboxWritablePath],
    temp_dir: &Path,
    network: SandboxNetworkPolicy,
) -> Command {
    let mut bwrap = Command::new("bwrap");
    let mut args = bubblewrap_base_args(cwd, writable_paths, network);
    args.push(OsString::from("--"));
    args.push(OsString::from("gh"));
    args.extend(words.iter().skip(1).map(OsString::from));
    bwrap.args(args);
    apply_github_pr_profile_env(&mut bwrap, temp_dir);
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

fn apply_github_pr_profile_env(command: &mut Command, temp_dir: &Path) {
    apply_sandbox_temp_env(command, Some(temp_dir));
    remove_git_profile_env(command);
    for key in [
        "GH_PAGER",
        "GH_EDITOR",
        "VISUAL",
        "EDITOR",
        "GH_BROWSER",
        "BROWSER",
        "GH_FORCE_TTY",
    ] {
        command.env_remove(key);
    }
    command
        .env("GH_PROMPT_DISABLED", "1")
        .env("GH_NO_UPDATE_NOTIFIER", "1")
        .env("GH_NO_EXTENSION_UPDATE_NOTIFIER", "1")
        .env("GH_SPINNER_DISABLED", "1")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_EDITOR", ":")
        .env("GIT_PAGER", "cat")
        .env("PAGER", "cat");
}

pub(crate) fn run_command_with_timeout(
    mut command: Command,
    timeout: Duration,
) -> anyhow::Result<SandboxCommandResult> {
    command.stdout(Stdio::piped()).stderr(Stdio::piped());
    isolate_sandbox_process_group(&mut command);
    let mut child = command.spawn()?;
    let mut stdout = child
        .stdout
        .take()
        .ok_or_else(|| anyhow::anyhow!("failed to capture sandbox stdout"))?;
    let mut stderr = child
        .stderr
        .take()
        .ok_or_else(|| anyhow::anyhow!("failed to capture sandbox stderr"))?;
    let stdout_reader = thread::spawn(move || {
        let mut bytes = Vec::new();
        stdout.read_to_end(&mut bytes).map(|_| bytes)
    });
    let stderr_reader = thread::spawn(move || {
        let mut bytes = Vec::new();
        stderr.read_to_end(&mut bytes).map(|_| bytes)
    });

    let deadline = Instant::now() + timeout;
    let mut timed_out = false;
    let exit_status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if Instant::now() >= deadline {
            timed_out = true;
            break terminate_sandbox_child(&mut child)?.ok_or_else(|| {
                anyhow::anyhow!("timed out and failed to terminate sandbox command")
            })?;
        }
        thread::sleep(Duration::from_millis(25));
    };
    cleanup_sandbox_process_group(&mut child, timed_out)?;

    let stdout = stdout_reader
        .join()
        .map_err(|_| anyhow::anyhow!("sandbox stdout reader panicked"))??;
    let stderr = stderr_reader
        .join()
        .map_err(|_| anyhow::anyhow!("sandbox stderr reader panicked"))??;

    Ok(SandboxCommandResult {
        status: if timed_out { "timed_out" } else { "exited" },
        exit_code: exit_status.code(),
        stdout: String::from_utf8_lossy(&stdout).into_owned(),
        stderr: String::from_utf8_lossy(&stderr).into_owned(),
    })
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
    let _ = Command::new("/bin/kill")
        .args([signal, group.as_str()])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        fs,
        path::{Path, PathBuf},
    };

    #[test]
    fn sandbox_run_cli_exit_code_maps_non_exited_results_to_one() {
        assert_eq!(
            sandbox_run_cli_exit_code(&sandbox_output("exited", Some(0))),
            None
        );
        assert_eq!(
            sandbox_run_cli_exit_code(&sandbox_output("exited", Some(7))),
            Some(7)
        );
        assert_eq!(
            sandbox_run_cli_exit_code(&sandbox_output("exited", None)),
            Some(1)
        );
        assert_eq!(
            sandbox_run_cli_exit_code(&sandbox_output("timed_out", Some(0))),
            Some(1)
        );
        assert_eq!(
            sandbox_run_cli_exit_code(&sandbox_output("timed_out", Some(9))),
            Some(1)
        );
    }

    #[test]
    fn sandbox_target_uses_core_relative_path_normalization() {
        let normalized = normalize_sandbox_target_path("write_targets", r".\src\.\auth.ts")
            .expect("target should normalize");

        assert_eq!(
            normalized,
            stateful_core::normalize_relative_path("src/auth.ts")
        );
    }

    #[test]
    fn process_find_request_requires_a_selector() {
        let request = SandboxProcessFindRequest {
            names: Vec::new(),
            contains: Vec::new(),
            pids: Vec::new(),
            parent_pids: Vec::new(),
            process_groups: Vec::new(),
        };

        let error = validate_process_find_request(&request)
            .expect_err("process find should require a selector");

        assert!(
            error
                .to_string()
                .contains("stateful sandbox process find requires at least one selector"),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn process_find_filters_rows_without_exposing_args() {
        let request = SandboxProcessFindRequest {
            names: Vec::new(),
            contains: vec!["denovo_codex_agent".to_string()],
            pids: Vec::new(),
            parent_pids: Vec::new(),
            process_groups: Vec::new(),
        };
        let rows = parse_process_find_ps_output(
            "101 1 101 S 00:01 codex /opt/bin/codex exec\n\
             202 1 202 S 00:02 python3 python3 crates/stateful-bench/scripts/denovo_codex_agent.py\n",
        )
        .expect("ps output should parse");

        let matches = filter_process_find_rows(&request, rows);

        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].pid, 202);
        assert_eq!(matches[0].comm, "python3");
    }

    #[test]
    fn process_find_excludes_current_finder_process() {
        let request = SandboxProcessFindRequest {
            names: Vec::new(),
            contains: vec!["denovo_codex_agent".to_string()],
            pids: Vec::new(),
            parent_pids: Vec::new(),
            process_groups: Vec::new(),
        };
        let current_pid = std::process::id();
        let rows = parse_process_find_ps_output(&format!(
            "{current_pid} 1 {current_pid} S 00:01 stateful stateful sandbox process find --contains denovo_codex_agent\n\
             202 1 202 S 00:02 python3 python3 crates/stateful-bench/scripts/denovo_codex_agent.py\n",
        ))
        .expect("ps output should parse");

        let matches = filter_process_find_rows(&request, rows);

        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].pid, 202);
    }

    #[test]
    fn sandbox_run_rejects_raw_ps_process_inspection() {
        let request = SandboxRunRequest {
            fs: SandboxFsProfile::ReadOnly,
            network: SandboxNetworkPolicy::Disabled,
            write_targets: Vec::new(),
            create_targets: Vec::new(),
            write_dirs: Vec::new(),
            command: "ps -o pid,comm -p 1".to_string(),
            timeout_seconds: None,
        };

        let error = validate_sandbox_run_request_shape(&request)
            .expect_err("sandbox run should reject raw ps inspection");

        assert!(
            error
                .to_string()
                .contains("process inspection must use stateful sandbox process find"),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn sandbox_run_rejects_raw_pgrep_process_inspection() {
        let request = SandboxRunRequest {
            fs: SandboxFsProfile::ReadOnly,
            network: SandboxNetworkPolicy::Disabled,
            write_targets: Vec::new(),
            create_targets: Vec::new(),
            write_dirs: Vec::new(),
            command: "pgrep -f denovo_codex_agent".to_string(),
            timeout_seconds: None,
        };

        let error = validate_sandbox_run_request_shape(&request)
            .expect_err("sandbox run should reject raw pgrep inspection");

        assert!(
            error
                .to_string()
                .contains("process inspection must use stateful sandbox process find"),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn sandbox_run_rejects_raw_process_inspection_shell_forms() {
        for command in [
            "env -i pgrep -f denovo_codex_agent",
            "command -p ps -o pid",
            "exec ps -o pid",
            "if true; then ps -ef; fi",
            "(pgrep -f denovo_codex_agent)",
            "sh -c 'pgrep -f denovo_codex_agent'",
            "echo $(ps -axo command)",
            "echo `pgrep -af denovo_codex_agent`",
            "env -S 'pgrep -f denovo_codex_agent'",
            "env --split-string='pgrep -f denovo_codex_agent'",
            "echo $(echo $(echo $(echo $(echo $(pgrep -f denovo_codex_agent)))))",
            r"p\s -ef",
        ] {
            let request = SandboxRunRequest {
                fs: SandboxFsProfile::ReadOnly,
                network: SandboxNetworkPolicy::Disabled,
                write_targets: Vec::new(),
                create_targets: Vec::new(),
                write_dirs: Vec::new(),
                command: command.to_string(),
                timeout_seconds: None,
            };

            let Err(error) = validate_sandbox_run_request_shape(&request) else {
                panic!("sandbox run should reject raw process inspection in `{command}`");
            };

            assert!(
                error
                    .to_string()
                    .contains("process inspection must use stateful sandbox process find"),
                "unexpected result for `{command}`: {error}"
            );
        }
    }

    #[test]
    fn sandbox_run_allows_non_process_command_with_process_text_argument() {
        let request = SandboxRunRequest {
            fs: SandboxFsProfile::ReadOnly,
            network: SandboxNetworkPolicy::Disabled,
            write_targets: Vec::new(),
            create_targets: Vec::new(),
            write_dirs: Vec::new(),
            command: "rg ps crates".to_string(),
            timeout_seconds: None,
        };

        validate_sandbox_run_request_shape(&request)
            .expect("literal ps argument should not be treated as process inspection");
    }

    fn sandbox_output(status: &'static str, exit_code: Option<i32>) -> SandboxRunOutput {
        SandboxRunOutput {
            status,
            exit_code,
            stdout: String::new(),
            stderr: String::new(),
            allowed_write_targets: Vec::new(),
            denied_write_targets: Vec::new(),
        }
    }

    #[cfg(unix)]
    #[test]
    fn timeout_kills_process_group_descendants_before_joining_readers() {
        if std::env::var_os(STATEFUL_SANDBOX_RUN_ACTIVE_ENV).is_some() {
            return;
        }

        let mut command = Command::new("/bin/sh");
        command
            .arg("-c")
            .arg("(trap '' TERM HUP INT; sleep 5) & wait");

        let started = Instant::now();
        let output = run_command_with_timeout(command, Duration::from_millis(100))
            .expect("sandbox command should time out and terminate");

        assert_eq!(output.status, "timed_out");
        assert!(
            started.elapsed() < Duration::from_secs(3),
            "timeout cleanup should not wait for ignored-TERM descendants"
        );
    }

    #[cfg(unix)]
    #[test]
    fn exited_wrapper_cleans_background_descendants_before_joining_readers() {
        if std::env::var_os(STATEFUL_SANDBOX_RUN_ACTIVE_ENV).is_some() {
            return;
        }

        let mut command = Command::new("/bin/sh");
        command
            .arg("-c")
            .arg("(trap '' TERM HUP INT; sleep 5) & printf done");

        let started = Instant::now();
        let output = run_command_with_timeout(command, Duration::from_secs(10))
            .expect("sandbox command should exit and clean descendants");

        assert_eq!(output.status, "exited");
        assert_eq!(output.stdout, "done");
        assert!(
            started.elapsed() < Duration::from_secs(3),
            "normal exit cleanup should not wait for background descendants"
        );
    }

    #[test]
    fn bubblewrap_read_only_uses_unshare_net_and_device_policy() {
        let args = bubblewrap_args(
            "rg auth src",
            Path::new("/repo"),
            &[],
            &[],
            false,
            SandboxNetworkPolicy::Disabled,
        );
        let args = args
            .into_iter()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect::<Vec<_>>();

        assert!(args.contains(&"--unshare-net".to_string()));
        assert!(!args.contains(&"--share-net".to_string()));
        assert!(
            args.windows(3)
                .any(|window| { window == ["--dev-bind", "/dev/null", "/dev/null"] })
        );
        assert!(
            args.windows(3)
                .any(|window| { window == ["--dev-bind", "/dev/zero", "/dev/zero"] })
        );
        assert!(!args.windows(5).any(|window| {
            window
                == [
                    "--dev-bind",
                    "/dev/zero",
                    "/dev/zero",
                    "--remount-ro",
                    "/dev/zero",
                ]
        }));
        assert!(
            args.windows(3)
                .any(|window| { window == ["--dev-bind", "/dev/urandom", "/dev/urandom"] })
        );
        assert!(!args.windows(5).any(|window| {
            window
                == [
                    "--dev-bind",
                    "/dev/urandom",
                    "/dev/urandom",
                    "--remount-ro",
                    "/dev/urandom",
                ]
        }));
        assert!(
            !args
                .windows(3)
                .any(|window| { window == ["--ro-bind", "/dev/zero", "/dev/zero"] })
        );
        assert!(
            !args
                .windows(3)
                .any(|window| { window == ["--ro-bind", "/dev/urandom", "/dev/urandom"] })
        );
        assert!(args.ends_with(&[
            "--".to_string(),
            "/bin/sh".to_string(),
            "-c".to_string(),
            "rg auth src".to_string(),
        ]));
    }

    #[test]
    fn bubblewrap_network_enabled_uses_share_net_and_omits_unshare_net() {
        let args = bubblewrap_args(
            "git ls-remote origin",
            Path::new("/repo"),
            &[],
            &[],
            false,
            SandboxNetworkPolicy::Enabled,
        );
        let args = args
            .into_iter()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect::<Vec<_>>();

        assert!(args.contains(&"--share-net".to_string()));
        assert!(!args.contains(&"--unshare-net".to_string()));
    }

    #[test]
    fn bubblewrap_write_targets_bind_authorized_files_and_devices() {
        let writable_paths = vec![
            SandboxWritablePath::file(PathBuf::from("/repo/src/allowed.ts")),
            SandboxWritablePath::file(PathBuf::from("/repo/src/new.ts")),
            SandboxWritablePath::directory(PathBuf::from("/repo/tmp")),
        ];
        let args = bubblewrap_args(
            "printf ok > src/allowed.ts",
            Path::new("/repo"),
            &writable_paths,
            &[],
            false,
            SandboxNetworkPolicy::Disabled,
        );
        let args = args
            .into_iter()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect::<Vec<_>>();

        assert!(args.windows(3).any(|window| {
            window == ["--bind", "/repo/src/allowed.ts", "/repo/src/allowed.ts"]
        }));
        assert!(
            args.windows(3)
                .any(|window| { window == ["--bind", "/repo/src/new.ts", "/repo/src/new.ts"] })
        );
        assert!(
            args.windows(3)
                .any(|window| { window == ["--bind", "/repo/tmp", "/repo/tmp"] })
        );
        assert!(
            args.windows(3)
                .any(|window| { window == ["--dev-bind", "/dev/null", "/dev/null"] })
        );
        assert!(
            args.windows(3)
                .any(|window| { window == ["--dev-bind", "/dev/zero", "/dev/zero"] })
        );
        assert!(!args.windows(5).any(|window| {
            window
                == [
                    "--dev-bind",
                    "/dev/zero",
                    "/dev/zero",
                    "--remount-ro",
                    "/dev/zero",
                ]
        }));
        assert!(
            args.windows(3)
                .any(|window| { window == ["--dev-bind", "/dev/urandom", "/dev/urandom"] })
        );
        assert!(!args.windows(5).any(|window| {
            window
                == [
                    "--dev-bind",
                    "/dev/urandom",
                    "/dev/urandom",
                    "--remount-ro",
                    "/dev/urandom",
                ]
        }));
        assert!(
            !args
                .windows(3)
                .any(|window| { window == ["--ro-bind", "/dev/zero", "/dev/zero"] })
        );
        assert!(
            !args
                .windows(3)
                .any(|window| { window == ["--ro-bind", "/dev/urandom", "/dev/urandom"] })
        );
        assert!(args.contains(&"--unshare-net".to_string()));
        assert!(!args.contains(&"--share-net".to_string()));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn bubblewrap_read_only_profile_can_read_required_devices() {
        if std::env::var_os(STATEFUL_SANDBOX_RUN_ACTIVE_ENV).is_some() {
            return;
        }
        if Command::new("bwrap").arg("--version").status().is_err() {
            return;
        }

        let output = run_command_with_timeout(
            bubblewrap_command(
                "dd if=/dev/zero of=/dev/null bs=1 count=1 >/dev/null 2>&1 && dd if=/dev/urandom of=/dev/null bs=1 count=1 >/dev/null 2>&1",
                Path::new("/"),
                &[],
                &[],
                false,
                None,
                SandboxNetworkPolicy::Disabled,
            ),
            Duration::from_secs(10),
        )
        .expect("bubblewrap command should run");

        assert_eq!(output.status, "exited");
        assert_eq!(
            output.exit_code,
            Some(0),
            "device reads should succeed: stdout={} stderr={}",
            output.stdout,
            output.stderr
        );
    }

    #[test]
    fn bubblewrap_git_profile_binds_repo_root_writable() {
        let writable_paths = git_profile_writable_paths(Path::new("/repo"));
        let words = vec![
            "git".to_string(),
            "checkout".to_string(),
            "main".to_string(),
        ];
        let args = bubblewrap_git_args(
            &words,
            Path::new("/repo"),
            &writable_paths,
            SandboxNetworkPolicy::Disabled,
        );
        let args = args
            .into_iter()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect::<Vec<_>>();

        assert!(
            args.windows(3)
                .any(|window| { window == ["--bind", "/repo", "/repo"] })
        );
        assert!(args.ends_with(&[
            "--".to_string(),
            "git".to_string(),
            "checkout".to_string(),
            "main".to_string(),
        ]));
        assert!(
            !args
                .windows(2)
                .any(|window| { window == ["/bin/sh", "-c"] })
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn bubblewrap_github_pr_profile_invokes_gh_by_argv() {
        let writable_paths = github_pr_profile_writable_paths(Path::new("/repo"));
        let words = vec![
            "gh".to_string(),
            "pr".to_string(),
            "view".to_string(),
            "123".to_string(),
        ];
        let command = bubblewrap_github_pr_command(
            &words,
            Path::new("/repo"),
            &writable_paths,
            Path::new("/repo/.git/stateful/github-pr/.stateful-tmp"),
            SandboxNetworkPolicy::Enabled,
        );
        let args = command
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect::<Vec<_>>();

        assert_eq!(command.get_program(), "bwrap");
        assert!(args.ends_with(&[
            "--".to_string(),
            "gh".to_string(),
            "pr".to_string(),
            "view".to_string(),
            "123".to_string(),
        ]));
        assert!(
            !args
                .windows(2)
                .any(|window| { window == ["/bin/sh", "-c"] })
        );
    }

    #[test]
    fn bubblewrap_git_profile_rebinds_persistent_git_metadata_read_only() {
        let repo_root = std::env::temp_dir().join(format!(
            "stateful-sandbox-git-readonly-overrides-{}",
            std::process::id()
        ));
        if repo_root.exists() {
            fs::remove_dir_all(&repo_root).expect("old temp root should be removable");
        }
        fs::create_dir_all(repo_root.join(".git/hooks"))
            .expect("git hooks dir should be creatable");
        fs::write(
            repo_root.join(".git/config"),
            "[core]\n\trepositoryformatversion = 0\n",
        )
        .expect("git config should be writable");
        let config = repo_root.join(".git/config").to_string_lossy().into_owned();
        let hooks = repo_root.join(".git/hooks").to_string_lossy().into_owned();

        let writable_paths = git_profile_writable_paths(&repo_root);
        let words = vec!["git".to_string(), "status".to_string()];
        let args = bubblewrap_git_args(
            &words,
            &repo_root,
            &writable_paths,
            SandboxNetworkPolicy::Disabled,
        );
        let args = args
            .into_iter()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect::<Vec<_>>();

        let command_separator_index = args
            .iter()
            .position(|arg| arg == "--")
            .expect("bubblewrap args should include a command separator");
        let config_rebind_index = args
            .windows(3)
            .position(|window| {
                window[0] == "--ro-bind" && window[1] == config && window[2] == config
            })
            .expect("git config should be rebound read-only");
        let hooks_rebind_index = args
            .windows(3)
            .position(|window| window[0] == "--ro-bind" && window[1] == hooks && window[2] == hooks)
            .expect("git hooks should be rebound read-only");

        assert!(args.windows(3).any(|window| {
            window[0] == "--bind"
                && window[1] == repo_root.to_string_lossy()
                && window[2] == repo_root.to_string_lossy()
        }));
        assert!(config_rebind_index < command_separator_index);
        assert!(hooks_rebind_index < command_separator_index);

        fs::remove_dir_all(&repo_root).expect("temp root should be removable");
    }

    #[test]
    fn prepare_writable_files_validates_all_targets_before_creating_files() {
        let repo_root = std::env::temp_dir().join(format!(
            "stateful-sandbox-create-target-order-{}",
            std::process::id()
        ));
        if repo_root.exists() {
            fs::remove_dir_all(&repo_root).expect("old temp root should be removable");
        }
        fs::create_dir_all(repo_root.join("src")).expect("repo src dir should be creatable");

        let error = prepare_sandbox_writable_paths(
            &repo_root,
            &["src/missing.txt".to_string()],
            &["src/new.txt".to_string()],
            &["tmp".to_string()],
        )
        .expect_err("missing write target should fail before create target is materialized");

        assert!(
            error
                .to_string()
                .contains("must already exist or be listed in create_targets"),
            "unexpected error: {error}"
        );
        assert!(
            !repo_root.join("src/new.txt").exists(),
            "failed sandbox run should not leave create target behind"
        );
        assert!(
            !repo_root.join("tmp").exists(),
            "failed sandbox run should not leave write dir behind"
        );

        fs::remove_dir_all(&repo_root).expect("temp root should be removable");
    }

    #[test]
    fn prepare_writable_paths_creates_write_dirs() {
        let repo_root =
            std::env::temp_dir().join(format!("stateful-sandbox-write-dir-{}", std::process::id()));
        if repo_root.exists() {
            fs::remove_dir_all(&repo_root).expect("old temp root should be removable");
        }
        fs::create_dir_all(&repo_root).expect("repo root should be creatable");

        let writable_paths =
            prepare_sandbox_writable_paths(&repo_root, &[], &[], &["tmp".to_string()])
                .expect("write dir should prepare");

        assert!(repo_root.join("tmp").is_dir());
        assert!(repo_root.join("tmp/.stateful-tmp").is_dir());
        assert_eq!(
            writable_paths,
            vec![SandboxWritablePath::directory(
                repo_root
                    .join("tmp")
                    .canonicalize()
                    .expect("tmp write dir should be canonicalizable")
            )]
        );

        fs::remove_dir_all(&repo_root).expect("temp root should be removable");
    }

    #[test]
    fn sandbox_temp_dir_uses_first_writable_directory() {
        let writable_paths = vec![
            SandboxWritablePath::file(PathBuf::from("/repo/README.md")),
            SandboxWritablePath::directory(PathBuf::from("/repo/tmp")),
        ];

        assert_eq!(
            sandbox_temp_dir(&writable_paths),
            Some(PathBuf::from("/repo/tmp/.stateful-tmp"))
        );
    }

    #[test]
    fn write_dir_unsupported_action_denial_points_to_server_restart() {
        let body = enrich_sandbox_write_dir_denial(serde_json::json!({
            "decision": "deny",
            "message": "Action is not supported by the v1 authorization API.",
            "reason_code": "unsupported_action",
            "required_next_action": "Use a supported action such as write_file."
        }));

        assert_eq!(
            body.get("message").and_then(Value::as_str),
            Some("The running stateful server does not support sandbox write directories.")
        );
        assert_eq!(
            body.get("required_next_action").and_then(Value::as_str),
            Some(
                "Restart the stateful server with the newly built stateful binary, then retry the sandbox run."
            )
        );
    }

    #[test]
    fn apply_sandbox_temp_env_sets_standard_temp_vars() {
        let mut command = Command::new("true");

        apply_sandbox_temp_env(&mut command, Some(Path::new("/repo/tmp/.stateful-tmp")));

        let envs = command
            .get_envs()
            .map(|(key, value)| {
                (
                    key.to_string_lossy().into_owned(),
                    value.map(|value| value.to_string_lossy().into_owned()),
                )
            })
            .collect::<std::collections::BTreeMap<_, _>>();
        assert_eq!(
            envs.get(STATEFUL_SANDBOX_RUN_ACTIVE_ENV),
            Some(&Some("1".to_string()))
        );
        assert_eq!(
            envs.get("TMPDIR"),
            Some(&Some("/repo/tmp/.stateful-tmp".to_string()))
        );
        assert_eq!(
            envs.get("TEMP"),
            Some(&Some("/repo/tmp/.stateful-tmp".to_string()))
        );
        assert_eq!(
            envs.get("TMP"),
            Some(&Some("/repo/tmp/.stateful-tmp".to_string()))
        );
        assert_eq!(
            envs.get("CARGO_TARGET_DIR"),
            Some(&Some("/repo/tmp/target".to_string()))
        );
    }

    #[test]
    fn seatbelt_profile_allows_device_reads_dev_null_writes_and_targets() {
        let profile = seatbelt_profile(
            &[
                SandboxWritablePath::file(PathBuf::from("/repo/src/allowed.ts")),
                SandboxWritablePath::file(PathBuf::from("/repo/src/quoted\"path.ts")),
                SandboxWritablePath::directory(PathBuf::from("/repo/tmp")),
            ],
            SandboxNetworkPolicy::Disabled,
        );

        assert!(profile.contains("(deny default)"));
        assert!(profile.contains("(allow file-read*)"));
        assert!(profile.contains("(allow sysctl-read)"));
        assert!(profile.contains(
            "(allow file-read* (literal \"/dev/null\") (literal \"/dev/zero\") (literal \"/dev/urandom\"))"
        ));
        let write_rules = profile
            .lines()
            .filter(|line| line.starts_with("(allow file-write*"))
            .collect::<Vec<_>>();
        assert!(
            write_rules
                .iter()
                .any(|line| line.contains("(literal \"/dev/null\")"))
        );
        assert!(!write_rules.iter().any(|line| line.contains("/dev/zero")));
        assert!(!write_rules.iter().any(|line| line.contains("/dev/urandom")));
        assert!(profile.contains("(literal \"/repo/src/allowed.ts\")"));
        assert!(profile.contains("(literal \"/repo/src/quoted\\\"path.ts\")"));
        assert!(profile.contains("(subpath \"/repo/tmp\")"));
        assert!(!profile.contains("subpath \"/repo/src\""));
        assert!(!profile.contains("subpath \"/dev\""));
        assert!(!profile.contains("(allow network*)"));
    }

    #[test]
    fn seatbelt_profile_allows_exact_connect_socket_without_full_network() {
        let profile = seatbelt_profile_with_connect_sockets(
            &[],
            &[PathBuf::from("/private/tmp/tmux-501/default")],
            false,
            SandboxNetworkPolicy::Disabled,
        );

        assert!(
            profile
                .contains("(allow network-outbound (literal \"/private/tmp/tmux-501/default\"))")
        );
        assert!(!profile.contains("(allow network*)"));
        assert!(!profile.contains("(subpath \"/private/tmp/tmux-501\")"));
    }

    #[test]
    fn seatbelt_profile_allows_signal_only_when_requested() {
        let default_profile = seatbelt_profile(&[], SandboxNetworkPolicy::Disabled);
        let signal_profile =
            seatbelt_profile_with_connect_sockets(&[], &[], true, SandboxNetworkPolicy::Disabled);

        assert!(!default_profile.contains("(allow signal)"));
        assert!(signal_profile.contains("(allow signal)"));
    }

    #[test]
    fn seatbelt_git_profile_allows_repo_root_subpath() {
        let writable_paths = git_profile_writable_paths(Path::new("/repo"));
        let profile = seatbelt_git_profile(
            &writable_paths,
            Path::new("/repo"),
            SandboxNetworkPolicy::Enabled,
        );

        assert!(profile.contains("(subpath \"/repo\")"));
        assert!(profile.contains("(allow network*)"));
    }

    #[test]
    fn seatbelt_git_profile_denies_persistent_git_metadata_writes() {
        let writable_paths = git_profile_writable_paths(Path::new("/repo"));
        let profile = seatbelt_git_profile(
            &writable_paths,
            Path::new("/repo"),
            SandboxNetworkPolicy::Enabled,
        );

        assert!(profile.contains("(deny file-write*"));
        assert!(profile.contains("(literal \"/repo/.git/config\")"));
        assert!(profile.contains("(literal \"/repo/.git/config.worktree\")"));
        assert!(profile.contains("(literal \"/repo/.git/hooks\")"));
        assert!(profile.contains("(subpath \"/repo/.git/hooks\")"));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn seatbelt_git_command_profile_allows_macos_identity_and_trust_services() {
        let writable_paths = git_profile_writable_paths(Path::new("/repo"));
        let words = vec![
            "git".to_string(),
            "push".to_string(),
            "origin".to_string(),
            "dev".to_string(),
        ];
        let command = seatbelt_git_command(
            &words,
            Path::new("/repo"),
            &writable_paths,
            Path::new("/repo/.git/stateful/.stateful-tmp"),
            Path::new("/repo/.git/stateful/hooks-disabled"),
            &GitProfileConfig::default(),
            SandboxNetworkPolicy::Enabled,
        );
        let args = command
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        let profile = args
            .windows(2)
            .find_map(|window| {
                if window[0] == "-p" {
                    Some(window[1].clone())
                } else {
                    None
                }
            })
            .expect("seatbelt git command should pass a profile after -p");

        assert!(profile.contains("(allow mach-lookup"));
        for global_name in [
            "com.apple.system.opendirectoryd.libinfo",
            "com.apple.system.DirectoryService.libinfo_v1",
            "com.apple.trustd",
            "com.apple.trustd.agent",
        ] {
            assert!(
                profile.contains(&format!("(global-name \"{global_name}\")")),
                "git profile should allow {global_name}: {profile}"
            );
        }
    }

    #[test]
    fn seatbelt_profile_does_not_allow_macos_identity_or_trust_services_by_default() {
        let profile = seatbelt_profile(&[], SandboxNetworkPolicy::Enabled);

        for global_name in [
            "com.apple.system.opendirectoryd.libinfo",
            "com.apple.system.DirectoryService.libinfo_v1",
            "com.apple.trustd",
            "com.apple.trustd.agent",
        ] {
            assert!(!profile.contains(global_name));
        }
    }

    #[test]
    fn read_only_profile_rejects_network_enabled() {
        let error = validate_profile_network_policy(
            SandboxFsProfile::ReadOnly,
            SandboxNetworkPolicy::Enabled,
        )
        .expect_err("read-only profile should reject network enabled");

        assert!(
            error
                .to_string()
                .contains("read-only sandbox run requires --network disabled")
        );
    }

    #[test]
    fn build_profile_rejects_explicit_write_targets() {
        validate_profile_targets(
            SandboxFsProfile::Build,
            &[],
            &[],
            &["tmp/build".to_string()],
        )
        .expect("build profile should accept one scoped tmp write dir");

        let missing_write_dir = validate_profile_targets(SandboxFsProfile::Build, &[], &[], &[])
            .expect_err("build profile should require a scoped tmp write dir");
        assert!(
            missing_write_dir
                .to_string()
                .contains("--write-dir tmp/<purpose>")
        );

        let error = validate_profile_targets(
            SandboxFsProfile::Build,
            &["README.md".to_string()],
            &[],
            &[],
        )
        .expect_err("build profile should reject explicit write targets");

        assert!(
            error
                .to_string()
                .contains("build profile rejects explicit write targets")
        );
    }

    #[test]
    fn git_profile_rejects_explicit_write_targets() {
        validate_profile_targets(SandboxFsProfile::Git, &[], &[], &[])
            .expect("git profile should manage write scope automatically");

        let error =
            validate_profile_targets(SandboxFsProfile::Git, &["README.md".to_string()], &[], &[])
                .expect_err("git profile should reject explicit write targets");

        assert!(
            error
                .to_string()
                .contains("git profile manages repo writes automatically")
        );
    }

    #[test]
    fn github_pr_profile_requires_network_enabled() {
        let error = validate_profile_network_policy(
            SandboxFsProfile::GithubPr,
            SandboxNetworkPolicy::Disabled,
        )
        .expect_err("github-pr profile should require network enabled");

        assert!(
            error
                .to_string()
                .contains("github-pr sandbox run requires --network enabled"),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn github_pr_profile_rejects_explicit_write_targets() {
        let error = validate_profile_targets(
            SandboxFsProfile::GithubPr,
            &["README.md".to_string()],
            &[],
            &[],
        )
        .expect_err("github-pr profile should reject explicit targets");

        assert!(
            error
                .to_string()
                .contains("github-pr profile manages transient PR state automatically"),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn github_pr_profile_writable_paths_are_private() {
        assert_eq!(
            github_pr_profile_writable_paths(Path::new("/repo")),
            vec![SandboxWritablePath::directory(PathBuf::from(
                "/repo/.git/stateful/github-pr"
            ))]
        );
    }

    #[test]
    fn github_pr_profile_writable_paths_use_stateful_git_for_linked_worktrees() {
        let repo_root = std::env::temp_dir().join(format!(
            "stateful-github-pr-linked-worktree-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&repo_root);
        fs::create_dir_all(&repo_root).expect("temp repo root should be created");
        fs::write(
            repo_root.join(".git"),
            "gitdir: ../main/.git/worktrees/linked\n",
        )
        .expect(".git file should be writable");

        assert_eq!(
            github_pr_profile_writable_paths(&repo_root),
            vec![SandboxWritablePath::directory(
                repo_root.join(".stateful-git/github-pr")
            )]
        );

        let _ = fs::remove_dir_all(&repo_root);
    }

    #[test]
    fn github_pr_profile_env_is_non_interactive_and_clears_launcher_overrides() {
        let mut command = Command::new("gh");
        command
            .env("GH_PAGER", "sh -c nope")
            .env("GH_EDITOR", "sh -c nope")
            .env("VISUAL", "sh -c nope")
            .env("EDITOR", "sh -c nope")
            .env("GH_BROWSER", "sh -c nope")
            .env("BROWSER", "sh -c nope")
            .env("GH_FORCE_TTY", "1");
        apply_github_pr_profile_env(
            &mut command,
            Path::new("/repo/.git/stateful/github-pr/.stateful-tmp"),
        );
        let envs = command
            .get_envs()
            .filter_map(|(key, value)| {
                value.map(|value| {
                    (
                        key.to_string_lossy().into_owned(),
                        value.to_string_lossy().into_owned(),
                    )
                })
            })
            .collect::<std::collections::BTreeMap<_, _>>();

        assert_eq!(envs.get("GH_PROMPT_DISABLED"), Some(&"1".to_string()));
        assert_eq!(envs.get("GH_NO_UPDATE_NOTIFIER"), Some(&"1".to_string()));
        assert_eq!(
            envs.get("GH_NO_EXTENSION_UPDATE_NOTIFIER"),
            Some(&"1".to_string())
        );
        assert_eq!(envs.get("GH_SPINNER_DISABLED"), Some(&"1".to_string()));
        assert_eq!(envs.get("GIT_TERMINAL_PROMPT"), Some(&"0".to_string()));
        assert_eq!(envs.get("GIT_EDITOR"), Some(&":".to_string()));
        assert_eq!(envs.get("GIT_PAGER"), Some(&"cat".to_string()));
        assert_eq!(envs.get("PAGER"), Some(&"cat".to_string()));
        for key in [
            "GH_PAGER",
            "GH_EDITOR",
            "VISUAL",
            "EDITOR",
            "GH_BROWSER",
            "BROWSER",
            "GH_FORCE_TTY",
        ] {
            assert!(!envs.contains_key(key), "{key} should not be inherited");
        }
    }

    #[test]
    fn direct_github_pr_command_invokes_gh_by_argv() {
        let words = vec![
            "gh".to_string(),
            "pr".to_string(),
            "view".to_string(),
            "123".to_string(),
            "--json".to_string(),
            "title,url".to_string(),
        ];
        let command = direct_github_pr_command(&words, Path::new("/repo"));
        let args = command
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect::<Vec<_>>();

        assert_eq!(command.get_program(), "gh");
        assert_eq!(
            args,
            vec![
                "pr".to_string(),
                "view".to_string(),
                "123".to_string(),
                "--json".to_string(),
                "title,url".to_string(),
            ]
        );
        assert!(
            !args
                .windows(2)
                .any(|window| { window == ["/bin/sh", "-c"] })
        );
    }

    #[test]
    fn git_profile_env_disables_implicit_branch_tracking_config() {
        let mut command = Command::new("git");
        apply_git_profile_env(
            &mut command,
            Path::new("/repo/.git/stateful/.stateful-tmp"),
            Path::new("/repo/.git/stateful/hooks-disabled"),
            &GitProfileConfig::default(),
        );
        let envs = command
            .get_envs()
            .filter_map(|(key, value)| {
                value.map(|value| {
                    (
                        key.to_string_lossy().into_owned(),
                        value.to_string_lossy().into_owned(),
                    )
                })
            })
            .collect::<std::collections::BTreeMap<_, _>>();

        assert_eq!(envs.get("GIT_CONFIG_COUNT"), Some(&"7".to_string()));
        assert_eq!(
            envs.get("GIT_CONFIG_KEY_5"),
            Some(&"branch.autoSetupMerge".to_string())
        );
        assert_eq!(envs.get("GIT_CONFIG_VALUE_5"), Some(&"false".to_string()));
        assert_eq!(
            envs.get("GIT_CONFIG_KEY_6"),
            Some(&"branch.autoSetupRebase".to_string())
        );
        assert_eq!(envs.get("GIT_CONFIG_VALUE_6"), Some(&"never".to_string()));
        assert!(!envs.contains_key("GIT_EXTERNAL_DIFF"));
    }

    #[test]
    fn git_profile_env_injects_discovered_commit_identity() {
        let mut command = Command::new("git");
        let config = GitProfileConfig {
            identity: Some(GitProfileIdentity {
                name: "Stateful User".into(),
                email: "stateful@example.invalid".into(),
            }),
            credential_helpers: vec![],
        };
        apply_git_profile_env(
            &mut command,
            Path::new("/repo/.git/stateful/.stateful-tmp"),
            Path::new("/repo/.git/stateful/hooks-disabled"),
            &config,
        );
        let envs = command
            .get_envs()
            .filter_map(|(key, value)| {
                value.map(|value| {
                    (
                        key.to_string_lossy().into_owned(),
                        value.to_string_lossy().into_owned(),
                    )
                })
            })
            .collect::<std::collections::BTreeMap<_, _>>();

        assert_eq!(envs.get("GIT_CONFIG_COUNT"), Some(&"9".to_string()));
        assert_eq!(envs.get("GIT_CONFIG_KEY_7"), Some(&"user.name".to_string()));
        assert_eq!(
            envs.get("GIT_CONFIG_VALUE_7"),
            Some(&"Stateful User".to_string())
        );
        assert_eq!(
            envs.get("GIT_CONFIG_KEY_8"),
            Some(&"user.email".to_string())
        );
        assert_eq!(
            envs.get("GIT_CONFIG_VALUE_8"),
            Some(&"stateful@example.invalid".to_string())
        );
    }

    #[test]
    fn git_profile_env_injects_allowed_credential_helpers() {
        let mut command = Command::new("git");
        let config = GitProfileConfig {
            identity: None,
            credential_helpers: vec!["store".into(), "!gh auth git-credential".into()],
        };
        apply_git_profile_env(
            &mut command,
            Path::new("/repo/.git/stateful/.stateful-tmp"),
            Path::new("/repo/.git/stateful/hooks-disabled"),
            &config,
        );
        let envs = command
            .get_envs()
            .filter_map(|(key, value)| {
                value.map(|value| {
                    (
                        key.to_string_lossy().into_owned(),
                        value.to_string_lossy().into_owned(),
                    )
                })
            })
            .collect::<std::collections::BTreeMap<_, _>>();

        assert_eq!(envs.get("GIT_CONFIG_COUNT"), Some(&"9".to_string()));
        assert_eq!(
            envs.get("GIT_CONFIG_KEY_7"),
            Some(&"credential.helper".to_string())
        );
        assert_eq!(envs.get("GIT_CONFIG_VALUE_7"), Some(&"store".to_string()));
        assert_eq!(
            envs.get("GIT_CONFIG_KEY_8"),
            Some(&"credential.helper".to_string())
        );
        assert_eq!(
            envs.get("GIT_CONFIG_VALUE_8"),
            Some(&"!gh auth git-credential".to_string())
        );
    }

    #[test]
    fn git_profile_credential_helper_filter_allows_only_safe_helpers() {
        assert_eq!(
            allowed_git_profile_credential_helper("store"),
            Some("store".to_string())
        );
        assert_eq!(
            allowed_git_profile_credential_helper("osxkeychain"),
            Some("osxkeychain".to_string())
        );
        assert_eq!(
            allowed_git_profile_credential_helper("!gh auth git-credential"),
            Some("!gh auth git-credential".to_string())
        );
        assert_eq!(
            allowed_git_profile_credential_helper("! gh auth git-credential"),
            Some("!gh auth git-credential".to_string())
        );
        assert_eq!(
            allowed_git_profile_credential_helper("!curl example.test"),
            None
        );
        assert_eq!(allowed_git_profile_credential_helper("/tmp/helper"), None);
        assert_eq!(
            allowed_git_profile_credential_helper("store --file=/tmp/x"),
            None
        );
    }

    #[test]
    fn git_profile_identity_discovers_normal_git_config() {
        let root = std::env::temp_dir().join(format!(
            "stateful-git-profile-identity-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).expect("temp repo should be created");
        let run_git = |args: &[&str]| {
            let status = Command::new("git")
                .args(args)
                .current_dir(&root)
                .status()
                .expect("git should run");
            assert!(status.success(), "git {args:?} should succeed");
        };
        run_git(&["init"]);
        run_git(&["config", "user.name", "Config User"]);
        run_git(&["config", "user.email", "config@example.invalid"]);

        let identity =
            discover_git_profile_identity(&root).expect("identity should be read from git config");

        assert_eq!(
            identity,
            GitProfileIdentity {
                name: "Config User".to_string(),
                email: "config@example.invalid".to_string(),
            }
        );
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn git_profile_config_discovers_allowed_credential_helpers() {
        let root = std::env::temp_dir().join(format!(
            "stateful-git-profile-credentials-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).expect("temp repo should be created");
        let run_git = |args: &[&str]| {
            let status = Command::new("git")
                .args(args)
                .current_dir(&root)
                .status()
                .expect("git should run");
            assert!(status.success(), "git {args:?} should succeed");
        };
        run_git(&["init"]);
        run_git(&["config", "user.name", "Config User"]);
        run_git(&["config", "user.email", "config@example.invalid"]);
        run_git(&["config", "--add", "credential.helper", "store"]);
        run_git(&[
            "config",
            "--add",
            "credential.helper",
            "!gh auth git-credential",
        ]);
        run_git(&["config", "--add", "credential.helper", "!curl example.test"]);

        let config = discover_git_profile_config(&root);

        assert!(config.credential_helpers.contains(&"store".to_string()));
        assert!(
            config
                .credential_helpers
                .contains(&"!gh auth git-credential".to_string())
        );
        assert!(
            !config
                .credential_helpers
                .contains(&"!curl example.test".to_string())
        );
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn git_profile_rejects_git_alias_config_dispatch() {
        let cases = [
            "git -c alias.x='!curl https://example.test' x",
            "git -calias.x='!curl https://example.test' x",
            "git --config-env=alias.x=STATEFUL_ALIAS x",
            "git x",
        ];

        for command in cases {
            let error = validate_git_profile_command(command)
                .expect_err("git profile should reject alias dispatch surfaces");

            assert!(
                error.to_string().contains("git profile"),
                "unexpected error for `{command}`: {error}"
            );
        }
    }

    #[test]
    fn git_profile_rejects_shell_dispatching_git_subcommands() {
        let cases = [
            "git submodule foreach 'printf nope'",
            "git filter-branch --tree-filter 'printf nope'",
            "git difftool --tool vimdiff",
            "git mergetool --tool vimdiff",
            "git rebase --exec 'printf nope'",
            "git bisect run printf nope",
            "git grep --open-files-in-pager='sh -c id' TODO",
            "git grep --open-files-in-pager TODO",
            "git grep -Ocat TODO",
            "git grep -O TODO",
            "git archive --exec=sh HEAD",
            "git archive --remote=origin --exec sh HEAD",
            "git fetch --upload-pack=sh origin",
            "git fetch --upload-pack sh origin",
            "git pull --upload-pack=sh origin main",
            "git push --receive-pack=sh origin main",
            "git push --exec=sh origin main",
        ];

        for command in cases {
            let error = validate_git_profile_command(command)
                .expect_err("git profile should reject shell-dispatching git commands");

            assert!(
                error.to_string().contains("git profile"),
                "unexpected error for `{command}`: {error}"
            );
        }
    }

    #[test]
    fn git_profile_rejects_persistent_metadata_mutation() {
        let cases = [
            "git remote add origin https://example.test/repo.git",
            "git remote set-url origin https://example.test/repo.git",
            "git remote rename origin backup",
            "git remote remove origin",
            "git remote rm origin",
            "git remote set-head origin main",
            "git remote set-branches origin main",
            "git remote update origin",
            "git remote prune origin",
        ];

        for command in cases {
            let error = validate_git_profile_command(command)
                .expect_err("git profile should reject persistent metadata mutation");

            assert!(
                error.to_string().contains("git profile"),
                "unexpected error for `{command}`: {error}"
            );
        }
    }

    #[test]
    fn git_profile_rejects_local_config_persistence_options() {
        let cases = [
            "git init",
            "git init --template /tmp/template",
            "git branch --set-upstream-to=origin/main",
            "git branch --set-upstream-to origin/main",
            "git branch -u origin/main",
            "git branch --track new origin/main",
            "git branch -t new origin/main",
            "git branch --unset-upstream",
            "git push --set-upstream origin main",
            "git push -u origin main",
            "git push --follow-tags --set-upstream origin main",
            "git checkout --track origin/main",
            "git checkout -t origin/main",
            "git switch --track origin/main",
            "git switch -t origin/main",
        ];

        for command in cases {
            let error = validate_git_profile_command(command)
                .expect_err("git profile should reject local config persistence options");

            assert!(
                error.to_string().contains("git profile"),
                "unexpected error for `{command}`: {error}"
            );
        }
    }

    #[test]
    fn git_profile_allows_read_only_remote_queries() {
        let cases = [
            "git remote",
            "git remote -v",
            "git remote get-url origin",
            "git remote get-url --push origin",
            "git remote get-url --all origin",
            "git remote show -n origin",
        ];

        for command in cases {
            validate_git_profile_command(command)
                .unwrap_or_else(|error| panic!("expected `{command}` to be allowed: {error}"));
        }
    }

    #[test]
    fn github_pr_profile_command_validation_accepts_allowed_pr_surface() {
        for command in [
            "gh pr list",
            "gh pr view 123 --json title,url",
            "gh pr status",
            "gh pr create --title 'Update policy' --body 'Adds github-pr profile' --base dev --head feature --draft",
        ] {
            parse_github_pr_profile_command(command)
                .unwrap_or_else(|error| panic!("expected `{command}` to be accepted: {error}"));
        }
    }

    #[test]
    fn github_pr_profile_command_validation_rejects_non_pr_and_interactive_surfaces() {
        for (command, expected) in [
            ("gh api repos/owner/repo", "requires a single gh pr command"),
            ("gh auth status", "requires a single gh pr command"),
            ("gh pr merge 123", "does not allow gh pr subcommand `merge`"),
            ("gh pr close 123", "does not allow gh pr subcommand `close`"),
            ("gh pr edit 123", "does not allow gh pr subcommand `edit`"),
            (
                "gh pr review 123",
                "does not allow gh pr subcommand `review`",
            ),
            (
                "gh pr checks 123",
                "does not allow gh pr subcommand `checks`",
            ),
            ("gh pr ready 123", "does not allow gh pr subcommand `ready`"),
            (
                "gh pr create --web",
                "does not allow interactive/browser flag `--web`",
            ),
            (
                "gh pr view --web=true",
                "does not allow interactive/browser flag `--web`",
            ),
            (
                "gh pr create --editor",
                "does not allow interactive/browser flag `--editor`",
            ),
            (
                "gh pr create --editor=true",
                "does not allow interactive/browser flag `--editor`",
            ),
            (
                "gh pr create --recover abc",
                "does not allow interactive/browser flag `--recover`",
            ),
            (
                "gh pr create --recover=abc",
                "does not allow interactive/browser flag `--recover`",
            ),
            (
                "gh pr view -w",
                "does not allow interactive/browser flag `-w`",
            ),
            (
                "gh pr create -e",
                "does not allow interactive/browser flag `-e`",
            ),
            (
                "gh pr create -we",
                "does not allow interactive/browser flag `-w`",
            ),
            (
                "gh pr create -ew",
                "does not allow interactive/browser flag `-e`",
            ),
            ("gh pr list | cat", "shell control syntax is not supported"),
            (
                "gh pr view $(echo 1)",
                "command substitution is not supported",
            ),
        ] {
            let error = parse_github_pr_profile_command(command)
                .expect_err("github-pr profile should reject unsafe command");
            assert!(
                error.contains(expected),
                "for `{command}`, expected `{expected}`, got `{error}`"
            );
        }
    }

    #[test]
    fn git_profile_temp_dir_uses_private_git_dir() {
        let writable_paths = git_profile_writable_paths(Path::new("/repo"));

        assert_eq!(
            sandbox_temp_dir(&writable_paths),
            Some(PathBuf::from("/repo/.git/stateful/.stateful-tmp"))
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn seatbelt_github_pr_profile_invokes_gh_by_argv() {
        let writable_paths = github_pr_profile_writable_paths(Path::new("/repo"));
        let words = vec![
            "gh".to_string(),
            "pr".to_string(),
            "view".to_string(),
            "123".to_string(),
        ];
        let command = seatbelt_github_pr_command(
            &words,
            Path::new("/repo"),
            &writable_paths,
            Path::new("/repo/.git/stateful/github-pr/.stateful-tmp"),
            SandboxNetworkPolicy::Enabled,
        );
        let args = command
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect::<Vec<_>>();

        assert_eq!(command.get_program(), "/usr/bin/sandbox-exec");
        assert!(args.ends_with(&[
            "gh".to_string(),
            "pr".to_string(),
            "view".to_string(),
            "123".to_string(),
        ]));
        assert!(
            !args
                .windows(2)
                .any(|window| { window == ["/bin/sh", "-c"] })
        );
    }

    #[test]
    fn seatbelt_profile_allows_network_only_when_enabled() {
        let disabled = seatbelt_profile(&[], SandboxNetworkPolicy::Disabled);
        let enabled = seatbelt_profile(&[], SandboxNetworkPolicy::Enabled);

        assert!(!disabled.contains("(allow network*)"));
        assert!(enabled.contains("(allow network*)"));
    }
}
