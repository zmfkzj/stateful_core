use clap::{Parser, Subcommand};
use std::{
    net::SocketAddr,
    path::{Path, PathBuf},
};

mod codex_wrapper;
mod commit;
mod external_run;
mod global_paths;
mod hook;
mod install;
mod lan;
mod mcp;
mod outbox;
mod push;
mod repo_registry;
mod runtime;
mod sandbox;
mod server_lifecycle;

pub use codex_wrapper::{
    CodexInvocation, CodexSandboxMode, CodexWrapperOptions, build_codex_invocation, run_codex,
};
pub use commit::{CommitRequest, CommitResult, run_structured_commit};
pub use external_run::{
    ExternalRunApproval, ExternalRunRequest, approve_external_run, request_external_run,
    run_approved_external_run,
};
pub use global_paths::GlobalPaths;
pub use hook::{
    HookOutcome, handle_post_tool_use_in_repo, handle_pre_tool_use, handle_pre_tool_use_in_repo,
    handle_session_start_in_repo, handle_stop_in_repo, handle_user_prompt_submit_in_repo,
};
pub use install::{
    InstallOptions, InstallPlan, apply_global_install, current_stateful_binary_path,
    default_codex_config_path, plan_global_install,
};
pub use lan::{
    LanCommand, LanJoinOptions, LanJoinResult, LanServeOptions, LanServeResult, join_lan_runtime,
    lan_join_commands, serve_lan_runtime,
};
pub use mcp::{call_mcp_tool_in_repo, handle_mcp_jsonrpc_in_repo, serve_mcp_stdio_in_repo};
pub use outbox::{sync_outbox_in_repo, sync_outbox_in_repo_with_runtime};
pub use push::{PushRequest, PushResult, run_structured_push};
pub use repo_registry::{
    RepoEntry, RepoGate, RepoIdentity, RepoRegistry, detect_git_root, disable_repo, enable_repo,
    repo_gate, repo_identity_for_enabled_repo,
};
pub use runtime::{
    CODEX_THREAD_ID_ENV, CurrentSession, HttpResponse, IntentCancelArgs, IntentClaimArgs,
    IntentDeclareArgs, IntentRequestArgs, ProtocolEnvelopeArgs, STATEFUL_CODEX_RUN_ID_ENV,
    ServerRuntime, cancel_intent_via_http, claim_intent_via_http, declare_intent_via_http,
    discover_runtime, discover_runtime_with_global, discover_runtime_with_optional_global,
    get_json, global_state_db_path, intent_cancel_protocol_body, intent_claim_protocol_body,
    intent_declare_protocol_body, intent_request_protocol_body, post_json, protocol_envelope,
    read_current_session_file, read_current_session_file_for_codex_run, request_intent_via_http,
    runtime_env_override_is_configured, runtime_from_remote, runtime_has_required_identity,
    runtime_identity_matches_pid, write_current_session_file,
    write_current_session_file_for_codex_run, write_current_session_file_for_codex_session,
    write_global_runtime_file, write_runtime_file,
};
pub use sandbox::{SandboxFsProfile, SandboxNetworkPolicy};
pub use server_lifecycle::{
    ServerStartOptions, detached_server_args, ensure_server, ensure_server_with,
    ensure_server_with_options, runtime_is_healthy, stop_server,
};

#[derive(Debug, Parser)]
#[command(name = "stateful")]
#[command(about = "Current-state coordination for coding agents")]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    Install {
        #[arg(long)]
        yes: bool,
        #[arg(long)]
        codex_config: Option<PathBuf>,
        #[arg(long)]
        binary: Option<String>,
    },
    Server {
        #[command(subcommand)]
        command: Option<ServerCommand>,
        #[arg(long, default_value = "127.0.0.1")]
        host: String,
        #[arg(long, default_value_t = 43873)]
        port: u16,
        #[arg(long)]
        token: Option<String>,
        #[arg(long, default_value = "local")]
        workspace_id: String,
    },
    #[command(subcommand)]
    Lan(LanCommand),
    Status,
    Current,
    Events,
    Doctor,
    Commit {
        #[arg(short = 'm', long)]
        message: String,
        #[arg(required = true, num_args = 1.., last = true)]
        paths: Vec<String>,
    },
    Push {
        #[arg(requires = "branch")]
        remote: Option<String>,
        branch: Option<String>,
    },
    Codex {
        #[arg(long, default_value = "codex")]
        codex_bin: String,
        #[arg(long, value_enum, default_value = "passthrough")]
        sandbox: CodexSandboxMode,
        #[arg(long)]
        no_stateful: bool,
        #[arg(num_args = 0.., allow_hyphen_values = true, trailing_var_arg = true)]
        args: Vec<String>,
    },
    #[command(subcommand)]
    ExternalRun(ExternalRunCommand),
    #[command(subcommand)]
    Sandbox(SandboxCommand),
    Enable {
        #[arg(long)]
        repo: Option<PathBuf>,
    },
    Disable {
        #[arg(long)]
        repo: Option<PathBuf>,
    },
    #[command(subcommand)]
    Repos(ReposCommand),
    #[command(subcommand)]
    Notifications(NotificationsCommand),
    #[command(subcommand)]
    Resume(ResumeCommand),
    #[command(subcommand)]
    Intent(IntentCommand),
    #[command(subcommand)]
    Mcp(McpCommand),
    SyncOutbox,
    #[command(subcommand)]
    Hook(HookCommand),
}

#[derive(Debug, Subcommand)]
pub enum ExternalRunCommand {
    Request {
        #[arg(long)]
        purpose: String,
        #[arg(long = "write-target")]
        write_targets: Vec<String>,
        #[arg(long = "create-target")]
        create_targets: Vec<String>,
        #[arg(long = "write-dir")]
        write_dirs: Vec<String>,
        #[arg(long, value_enum, default_value = "disabled")]
        network: SandboxNetworkPolicy,
        #[arg(long)]
        timeout_seconds: Option<u64>,
        #[arg(long)]
        command: String,
    },
    Approve {
        request_id: String,
        #[arg(long)]
        run: bool,
    },
    Run {
        request_id: String,
    },
}

#[derive(Debug, Subcommand)]
pub enum ServerCommand {
    Start {
        #[arg(long)]
        foreground: bool,
        #[arg(long, default_value = "127.0.0.1")]
        host: String,
        #[arg(long, default_value_t = 43873)]
        port: u16,
        #[arg(long)]
        token: Option<String>,
        #[arg(long, default_value = "local")]
        workspace_id: String,
    },
    Stop,
    Status,
}

#[derive(Debug, Subcommand)]
pub enum SandboxCommand {
    Run {
        #[arg(long, value_enum, default_value = "read-only")]
        fs: SandboxFsProfile,
        #[arg(long, value_enum, default_value = "disabled")]
        network: SandboxNetworkPolicy,
        #[arg(long = "write-target")]
        write_targets: Vec<String>,
        #[arg(long = "create-target")]
        create_targets: Vec<String>,
        #[arg(long = "write-dir")]
        write_dirs: Vec<String>,
        #[arg(long)]
        command: String,
        #[arg(long)]
        timeout_seconds: Option<u64>,
    },
    RunNestedCodexBenchmark {
        #[arg(long)]
        purpose: String,
        #[arg(long = "write-dir")]
        write_dir: String,
        #[arg(long = "codex-home-root")]
        codex_home_root: String,
        #[arg(long)]
        command: String,
        #[arg(long)]
        timeout_seconds: Option<u64>,
    },
}

#[derive(Debug, Subcommand)]
pub enum IntentCommand {
    Declare {
        #[arg(long)]
        session_id: Option<String>,
        #[arg(long)]
        workspace_id: Option<String>,
        #[arg(long)]
        purpose: String,
        #[arg(required = true, num_args = 1..)]
        files_planned: Vec<String>,
    },
    Request {
        #[arg(long)]
        session_id: Option<String>,
        #[arg(long)]
        workspace_id: Option<String>,
        #[arg(long)]
        request_id: String,
        #[arg(long)]
        action: String,
        #[arg(long)]
        path: String,
        #[arg(long)]
        purpose: String,
    },
    Claim {
        #[arg(long)]
        session_id: Option<String>,
        #[arg(long)]
        workspace_id: Option<String>,
        #[arg(long)]
        wait_id: String,
    },
    Cancel {
        #[arg(long)]
        session_id: Option<String>,
        #[arg(long)]
        workspace_id: Option<String>,
        #[arg(long)]
        request_id: String,
    },
}

#[derive(Debug, Subcommand)]
pub enum ReposCommand {
    List,
}

#[derive(Debug, Subcommand)]
pub enum NotificationsCommand {
    Poll {
        #[arg(long)]
        session_id: Option<String>,
        #[arg(long)]
        workspace_id: Option<String>,
    },
}

#[derive(Debug, Subcommand)]
pub enum ResumeCommand {
    Next {
        #[arg(long)]
        session_id: Option<String>,
        #[arg(long)]
        workspace_id: Option<String>,
    },
}

#[derive(Debug, Subcommand)]
pub enum McpCommand {
    Call {
        tool_name: String,
        arguments_json: Option<String>,
    },
    Serve,
}

#[derive(Debug, Subcommand)]
pub enum HookCommand {
    SessionStart,
    UserPromptSubmit,
    PreToolUse,
    PostToolUse,
    Stop,
}

pub fn run() -> anyhow::Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Install {
            yes,
            codex_config,
            binary,
        } => {
            let paths = GlobalPaths::from_env()?;
            let codex_config_path = match codex_config {
                Some(path) => path,
                None => default_codex_config_path()?,
            };
            let binary_path = match binary {
                Some(path) => path,
                None => current_stateful_binary_path()?,
            };
            let plan = apply_global_install(InstallOptions {
                yes,
                paths,
                codex_config_path,
                binary_path,
            })?;
            println!(
                "{}",
                serde_json::to_string_pretty(&serde_json::json!({
                    "summary": plan.summary,
                    "files": plan.files,
                }))?
            );
        }
        Command::Server {
            command,
            host: legacy_host,
            port: legacy_port,
            token: legacy_token,
            workspace_id: legacy_workspace_id,
        } => match command.unwrap_or(ServerCommand::Start {
            foreground: true,
            host: legacy_host,
            port: legacy_port,
            token: legacy_token,
            workspace_id: legacy_workspace_id,
        }) {
            ServerCommand::Start {
                foreground,
                host,
                port,
                token,
                workspace_id,
            } => {
                if foreground {
                    run_server(host, port, token, workspace_id)?;
                } else {
                    let paths = GlobalPaths::from_env()?;
                    let runtime = ensure_server_with_options(
                        &paths,
                        ServerStartOptions {
                            host,
                            port,
                            token,
                            workspace_id,
                        },
                    )?;
                    println!("{}", serde_json::to_string_pretty(&runtime)?);
                }
            }
            ServerCommand::Status => {
                let paths = GlobalPaths::from_env()?;
                let runtime =
                    discover_runtime_with_global(current_repo_root_or_current_dir()?, &paths).ok();
                println!("{}", serde_json::to_string_pretty(&runtime)?);
            }
            ServerCommand::Stop => {
                let paths = GlobalPaths::from_env()?;
                stop_server(&paths)?;
                println!("{}", serde_json::json!({ "status": "ok" }));
            }
        },
        Command::Lan(command) => match command {
            LanCommand::Serve {
                host,
                port,
                token,
                workspace_id,
            } => {
                lan::run_lan_command(
                    LanCommand::Serve {
                        host,
                        port,
                        token,
                        workspace_id,
                    },
                    GlobalPaths::from_env()?,
                )?;
            }
            LanCommand::Join {
                base_url,
                token,
                workspace_id,
                enable_repo,
                binary,
                codex_config,
            } => {
                let paths = GlobalPaths::from_env()?;
                let binary_path = match binary {
                    Some(binary) => binary,
                    None => current_stateful_binary_path()?,
                };
                let codex_config_path = match codex_config {
                    Some(path) => path,
                    None => default_codex_config_path()?,
                };
                let enable_repo_root = if enable_repo {
                    Some(current_repo_root_or_current_dir()?)
                } else {
                    None
                };
                let result = join_lan_runtime(LanJoinOptions {
                    paths,
                    codex_config_path,
                    binary_path,
                    base_url,
                    token,
                    workspace_id,
                    enable_repo_root,
                })?;
                println!(
                    "{}",
                    serde_json::to_string_pretty(&serde_json::json!({
                        "status": result.status,
                        "base_url": result.runtime.base_url,
                        "workspace_id": result.runtime.workspace_id,
                        "repo_enabled": result.repo_enabled,
                    }))?
                );
            }
        },
        Command::Status => {
            let report = doctor_report(current_repo_root_or_current_dir()?);
            println!("{}", serde_json::to_string_pretty(&report)?);
        }
        Command::Current => {
            let (_repo_root, runtime) = discover_runtime_for_current_dir()?;
            let response = get_json(&runtime, "/v1/current")?;
            print_http_response(response)?;
        }
        Command::Events => {
            let (_repo_root, runtime) = discover_runtime_for_current_dir()?;
            let response = get_json(&runtime, "/v1/events")?;
            print_http_response(response)?;
        }
        Command::SyncOutbox => {
            let synced = sync_outbox_in_repo(current_repo_root_or_current_dir()?)?;
            println!("{}", serde_json::json!({ "synced": synced }));
        }
        Command::Doctor => {
            let report = doctor_report(current_repo_root_or_current_dir()?);
            println!("{}", serde_json::to_string_pretty(&report)?);
        }
        Command::Commit { message, paths } => {
            let cwd = std::env::current_dir()?;
            let repo_root = detect_git_root(&cwd)?;
            let paths = root_relative_paths(&repo_root, &cwd, paths)?;
            let current_session = read_current_session_file(&repo_root).ok();
            let result = run_structured_commit(CommitRequest {
                repo_root,
                message,
                paths,
                session_id: current_session
                    .as_ref()
                    .map(|session| session.session_id.clone()),
                workspace_id: current_session.map(|session| session.workspace_id),
                authorize: None,
            })?;
            println!(
                "{}",
                serde_json::to_string(&serde_json::json!({
                    "status": "ok",
                    "commit": result.commit_sha,
                    "paths": result.committed_paths
                }))?
            );
        }
        Command::Push { remote, branch } => {
            let cwd = std::env::current_dir()?;
            let repo_root = detect_git_root(&cwd)?;
            let result = run_structured_push(PushRequest {
                repo_root,
                remote,
                branch,
            })?;
            println!(
                "{}",
                serde_json::to_string(&serde_json::json!({
                    "status": "ok",
                    "remote": result.remote,
                    "branch": result.branch
                }))?
            );
        }
        Command::Codex {
            codex_bin,
            sandbox,
            no_stateful,
            args,
        } => {
            let code = run_codex(CodexWrapperOptions {
                codex_bin,
                sandbox,
                no_stateful,
                args,
            })?;
            std::process::exit(code);
        }
        Command::ExternalRun(ExternalRunCommand::Request {
            purpose,
            write_targets,
            create_targets,
            write_dirs,
            network,
            timeout_seconds,
            command,
        }) => {
            let approval = request_external_run(ExternalRunRequest {
                repo_root: current_repo_root_or_current_dir()?,
                paths: GlobalPaths::from_env()?,
                purpose,
                command,
                write_targets,
                create_targets,
                write_dirs,
                network,
                timeout_seconds,
            })?;
            print!("{}", approval.guidance);
        }
        Command::ExternalRun(ExternalRunCommand::Approve { request_id, run }) => {
            let paths = GlobalPaths::from_env()?;
            let approval = approve_external_run(&paths, &request_id, run)?;
            println!("{}", approval.guidance);
            if run {
                let output = run_approved_external_run(&paths, &request_id)?;
                println!("{}", serde_json::to_string(&output)?);
                if let Some(exit_code) =
                    sandbox::sandbox_run_cli_exit_code(&sandbox::SandboxRunOutput {
                        status: output.status,
                        exit_code: output.exit_code,
                        stdout: output.stdout.clone(),
                        stderr: output.stderr.clone(),
                        allowed_write_targets: Vec::new(),
                        denied_write_targets: Vec::new(),
                    })
                {
                    std::process::exit(exit_code);
                }
            }
        }
        Command::ExternalRun(ExternalRunCommand::Run { request_id }) => {
            let output = run_approved_external_run(&GlobalPaths::from_env()?, &request_id)?;
            println!("{}", serde_json::to_string(&output)?);
            if output.status != "exited" || output.exit_code != Some(0) {
                std::process::exit(output.exit_code.unwrap_or(1));
            }
        }
        Command::Sandbox(SandboxCommand::Run {
            fs,
            network,
            write_targets,
            create_targets,
            write_dirs,
            command,
            timeout_seconds,
        }) => {
            let paths = GlobalPaths::from_env()?;
            let repo_root = current_repo_root_or_current_dir()?;
            let output = match sandbox::run_sandbox_in_repo(
                &repo_root,
                &paths,
                sandbox::SandboxRunRequest {
                    fs,
                    network,
                    write_targets,
                    create_targets,
                    write_dirs,
                    command,
                    timeout_seconds,
                },
            ) {
                Ok(output) => output,
                Err(error) => {
                    if let Some(denied) =
                        error.downcast_ref::<sandbox::SandboxAuthorizationDenied>()
                    {
                        println!("{}", denied.body());
                        std::process::exit(1);
                    }
                    return Err(error);
                }
            };
            println!("{}", serde_json::to_string(&output)?);
            if let Some(exit_code) = sandbox::sandbox_run_cli_exit_code(&output) {
                std::process::exit(exit_code);
            }
        }
        Command::Sandbox(SandboxCommand::RunNestedCodexBenchmark {
            purpose,
            write_dir,
            codex_home_root,
            command,
            timeout_seconds,
        }) => {
            let paths = GlobalPaths::from_env()?;
            let repo_root = current_repo_root_or_current_dir()?;
            let output = match sandbox::run_nested_codex_benchmark_sandbox_in_repo(
                &repo_root,
                &paths,
                sandbox::NestedCodexBenchmarkSandboxRequest {
                    purpose,
                    write_dir,
                    codex_home_root,
                    command,
                    timeout_seconds,
                },
            ) {
                Ok(output) => output,
                Err(error) => {
                    if let Some(denied) =
                        error.downcast_ref::<sandbox::SandboxAuthorizationDenied>()
                    {
                        println!("{}", denied.body());
                        std::process::exit(1);
                    }
                    return Err(error);
                }
            };
            println!("{}", serde_json::to_string(&output)?);
            if let Some(exit_code) = sandbox::sandbox_run_cli_exit_code(&output) {
                std::process::exit(exit_code);
            }
        }
        Command::Enable { repo } => {
            let paths = GlobalPaths::from_env()?;
            let repo = repo.unwrap_or(std::env::current_dir()?);
            let entry = enable_repo(&paths, repo)?;
            println!("{}", serde_json::to_string(&entry)?);
        }
        Command::Disable { repo } => {
            let paths = GlobalPaths::from_env()?;
            let repo = repo.unwrap_or(std::env::current_dir()?);
            let entry = disable_repo(&paths, repo)?;
            println!("{}", serde_json::to_string(&entry)?);
        }
        Command::Repos(ReposCommand::List) => {
            let paths = GlobalPaths::from_env()?;
            let registry = RepoRegistry::load(&paths)?;
            println!("{}", serde_json::to_string_pretty(&registry)?);
        }
        Command::Notifications(NotificationsCommand::Poll {
            session_id,
            workspace_id,
        }) => {
            let (repo_root, runtime) = discover_runtime_for_current_dir()?;
            let (session_id, workspace_id) =
                resolve_session_workspace(repo_root.as_path(), &runtime, session_id, workspace_id)?;
            let response = post_json(
                &runtime,
                "/v1/notifications/poll",
                &serde_json::json!({
                    "session_id": session_id,
                    "workspace_id": workspace_id,
                }),
            )?;
            print_http_response(response)?;
        }
        Command::Resume(ResumeCommand::Next {
            session_id,
            workspace_id,
        }) => {
            let (repo_root, runtime) = discover_runtime_for_current_dir()?;
            let (session_id, workspace_id) =
                resolve_session_workspace(repo_root.as_path(), &runtime, session_id, workspace_id)?;
            let response = post_json(
                &runtime,
                "/v1/resume/next",
                &serde_json::json!({
                    "session_id": session_id,
                    "workspace_id": workspace_id,
                }),
            )?;
            print_http_response(response)?;
        }
        Command::Intent(IntentCommand::Declare {
            session_id,
            workspace_id,
            purpose,
            files_planned,
        }) => {
            let (repo_root, runtime) = discover_runtime_for_current_dir()?;
            let current_session = if session_id.is_none() || workspace_id.is_none() {
                read_current_session_file(&repo_root).ok()
            } else {
                None
            };
            let session_id = session_id
                .or_else(|| {
                    current_session
                        .as_ref()
                        .map(|session| session.session_id.clone())
                })
                .ok_or_else(|| {
                    anyhow::anyhow!(
                        "session id not provided and no current stateful session file was found"
                    )
                })?;
            let workspace_id = workspace_id
                .or_else(|| current_session.map(|session| session.workspace_id))
                .unwrap_or_else(|| runtime.workspace_id.clone());
            declare_intent_via_http(
                &runtime,
                IntentDeclareArgs {
                    session_id,
                    workspace_id,
                    purpose,
                    files_planned,
                    identity: GlobalPaths::from_env()
                        .ok()
                        .and_then(|paths| repo_identity_for_enabled_repo(&paths, &repo_root).ok()),
                },
            )?;
            println!("declared stateful intent");
        }
        Command::Intent(IntentCommand::Request {
            session_id,
            workspace_id,
            request_id,
            action,
            path,
            purpose,
        }) => {
            let (repo_root, runtime) = discover_runtime_for_current_dir()?;
            let (session_id, workspace_id) =
                resolve_session_workspace(repo_root.as_path(), &runtime, session_id, workspace_id)?;
            let response = request_intent_via_http(
                &runtime,
                IntentRequestArgs {
                    session_id,
                    workspace_id,
                    request_id,
                    action,
                    path,
                    purpose,
                    identity: GlobalPaths::from_env()
                        .ok()
                        .and_then(|paths| repo_identity_for_enabled_repo(&paths, &repo_root).ok()),
                },
            )?;
            print_http_response(response)?;
        }
        Command::Intent(IntentCommand::Claim {
            session_id,
            workspace_id,
            wait_id,
        }) => {
            let (repo_root, runtime) = discover_runtime_for_current_dir()?;
            let (session_id, workspace_id) =
                resolve_session_workspace(repo_root.as_path(), &runtime, session_id, workspace_id)?;
            claim_intent_via_http(
                &runtime,
                IntentClaimArgs {
                    session_id,
                    workspace_id,
                    wait_id,
                    identity: GlobalPaths::from_env()
                        .ok()
                        .and_then(|paths| repo_identity_for_enabled_repo(&paths, &repo_root).ok()),
                },
            )?;
            println!("claimed stateful intent");
        }
        Command::Intent(IntentCommand::Cancel {
            session_id,
            workspace_id,
            request_id,
        }) => {
            let (repo_root, runtime) = discover_runtime_for_current_dir()?;
            let (session_id, workspace_id) =
                resolve_session_workspace(repo_root.as_path(), &runtime, session_id, workspace_id)?;
            cancel_intent_via_http(
                &runtime,
                IntentCancelArgs {
                    session_id,
                    workspace_id,
                    request_id,
                    identity: GlobalPaths::from_env()
                        .ok()
                        .and_then(|paths| repo_identity_for_enabled_repo(&paths, &repo_root).ok()),
                },
            )?;
            println!("canceled stateful intent");
        }
        Command::Mcp(McpCommand::Call {
            tool_name,
            arguments_json,
        }) => {
            let arguments = parse_json_arguments(arguments_json)?;
            let response = call_mcp_tool_in_repo(std::env::current_dir()?, tool_name, arguments)?;
            print_http_response(response)?;
        }
        Command::Mcp(McpCommand::Serve) => {
            serve_mcp_stdio_in_repo(
                std::env::current_dir()?,
                std::io::stdin().lock(),
                std::io::stdout().lock(),
            )?;
        }
        Command::Hook(hook) => hook::run_hook(hook)?,
    }
    Ok(())
}

fn discover_runtime_for_current_dir() -> anyhow::Result<(PathBuf, ServerRuntime)> {
    let repo_root = current_repo_root_or_current_dir()?;
    let runtime = discover_runtime_with_optional_global(&repo_root)?;

    Ok((repo_root, runtime))
}

fn current_repo_root_or_current_dir() -> anyhow::Result<PathBuf> {
    let cwd = std::env::current_dir()?;
    Ok(detect_git_root(&cwd).unwrap_or(cwd))
}

fn root_relative_paths(
    repo_root: &Path,
    cwd: &Path,
    paths: Vec<String>,
) -> anyhow::Result<Vec<String>> {
    let canonical_repo = repo_root
        .canonicalize()
        .unwrap_or_else(|_| repo_root.to_path_buf());
    let canonical_cwd = cwd.canonicalize().unwrap_or_else(|_| cwd.to_path_buf());
    let cwd_relative = canonical_cwd.strip_prefix(&canonical_repo).ok();

    Ok(paths
        .into_iter()
        .map(|path| {
            if Path::new(&path).is_absolute() {
                path.to_string()
            } else if let Some(prefix) =
                cwd_relative.filter(|prefix| !prefix.as_os_str().is_empty())
            {
                normalize_root_relative_path(&prefix.join(&path))
            } else {
                path.to_string()
            }
        })
        .collect())
}

fn normalize_root_relative_path(path: &Path) -> String {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            std::path::Component::CurDir => {}
            std::path::Component::Normal(part) => normalized.push(part),
            std::path::Component::ParentDir => {
                if !normalized.pop() {
                    normalized.push("..");
                }
            }
            other => normalized.push(other.as_os_str()),
        }
    }
    normalized.to_string_lossy().replace('\\', "/")
}

fn resolve_session_workspace(
    repo_root: &Path,
    runtime: &ServerRuntime,
    session_id: Option<String>,
    workspace_id: Option<String>,
) -> anyhow::Result<(String, String)> {
    let current_session = if session_id.is_none() || workspace_id.is_none() {
        read_current_session_file(repo_root).ok()
    } else {
        None
    };
    let session_id = session_id
        .or_else(|| {
            current_session
                .as_ref()
                .map(|session| session.session_id.clone())
        })
        .ok_or_else(|| {
            anyhow::anyhow!(
                "session id not provided and no current stateful session file was found"
            )
        })?;
    let workspace_id = workspace_id
        .or_else(|| current_session.map(|session| session.workspace_id))
        .unwrap_or_else(|| runtime.workspace_id.clone());

    Ok((session_id, workspace_id))
}

fn parse_json_arguments(arguments_json: Option<String>) -> anyhow::Result<serde_json::Value> {
    match arguments_json {
        Some(arguments_json) => Ok(serde_json::from_str(&arguments_json)?),
        None => Ok(serde_json::json!({})),
    }
}

fn print_http_response(response: HttpResponse) -> anyhow::Result<()> {
    println!("{}", response.body);
    if !(200..300).contains(&response.status_code) {
        anyhow::bail!("state server returned HTTP {}", response.status_code);
    }

    Ok(())
}

fn run_server(
    host: String,
    port: u16,
    token: Option<String>,
    workspace_id: String,
) -> anyhow::Result<()> {
    let token = token.unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
    let base_url = format!("http://{host}:{port}");
    let paths = GlobalPaths::from_env()?;
    let runtime = ServerRuntime::new(&base_url, &token, workspace_id, std::process::id());
    let store = stateful_store::Store::open(global_state_db_path(&paths))?;

    let addr: SocketAddr = format!("{host}:{port}").parse()?;
    let tokio_runtime = tokio::runtime::Runtime::new()?;
    tokio_runtime.block_on(async move {
        let listener = tokio::net::TcpListener::bind(addr).await?;
        write_global_runtime_file(&paths, &runtime)?;
        stateful_server::serve_listener(
            listener,
            stateful_server::ServerConfig::with_store(token, store),
        )
        .await
    })
}

pub fn state_db_path(repo_root: impl AsRef<Path>) -> std::path::PathBuf {
    repo_root.as_ref().join(".stateful_core").join("state.db")
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct DoctorReport {
    pub installed: bool,
    pub hooks_json: bool,
    pub config_yml: bool,
    pub runtime_server_json: bool,
    pub state_db: bool,
    pub global_config_yml: bool,
    pub global_runtime_server_json: bool,
    pub global_state_db: bool,
    pub repo_enabled: bool,
    pub global_paths_error: Option<String>,
    pub global_registry_error: Option<String>,
}

pub fn doctor_report(repo_root: impl AsRef<Path>) -> DoctorReport {
    match GlobalPaths::from_env() {
        Ok(paths) => doctor_report_with_global(repo_root, &paths),
        Err(error) => {
            let paths = GlobalPaths::new(PathBuf::from(".stateful_core"));
            let mut report = doctor_report_with_global(repo_root, &paths);
            report.global_paths_error = Some(error.to_string());
            report
        }
    }
}

pub fn doctor_report_with_global(repo_root: impl AsRef<Path>, paths: &GlobalPaths) -> DoctorReport {
    let detected_root = detect_git_root(repo_root.as_ref()).ok();
    let repo_root = detected_root.as_deref().unwrap_or(repo_root.as_ref());
    let hooks_json = repo_root.join(".codex").join("hooks.json").is_file();
    let codex_config_toml = repo_root.join(".codex").join("config.toml").is_file();
    let config_yml = repo_root.join(".stateful").join("config.yml").is_file();
    let runtime_server_json = repo_root
        .join(".stateful_core")
        .join("runtime")
        .join("server.json")
        .is_file();
    let state_db = state_db_path(repo_root).is_file();
    let (repo_enabled, global_registry_error) = match RepoRegistry::load(paths) {
        Ok(registry) => (registry.is_enabled(repo_root), None),
        Err(error) => (false, Some(error.to_string())),
    };

    DoctorReport {
        installed: (hooks_json || codex_config_toml || paths.config_yml.is_file()) && config_yml,
        hooks_json,
        config_yml,
        runtime_server_json,
        state_db,
        global_config_yml: paths.config_yml.is_file(),
        global_runtime_server_json: paths.server_json.is_file(),
        global_state_db: paths.state_db.is_file(),
        repo_enabled,
        global_paths_error: None,
        global_registry_error,
    }
}

fn default_config_yml() -> &'static str {
    r#"protocol_version: stateful.v1
intent_ttl_seconds: 900
intent_max_seconds: 3600
directory_scope_depth: 2
delete_requires_exact_file_scope: true
rename_requires_exact_file_scope: true
default_write_policy: deny
event_retention_days: 14
    "#
}
