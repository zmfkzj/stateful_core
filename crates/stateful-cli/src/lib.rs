use clap::{Parser, Subcommand, ValueEnum};
use stateful_core::{ActorType, SourceKind};
use std::{
    io::Write,
    net::SocketAddr,
    path::{Path, PathBuf},
    str::FromStr,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

mod codex_benchmark;
mod codex_wrapper;
mod commit;
mod global_paths;
mod hook;
mod install;
mod lan;
mod outbox;
mod push;
mod repo_registry;
mod runtime;
mod sandbox;
mod server_lifecycle;
mod shadow_guard;
mod shell_command;
mod watch;

pub use codex_wrapper::{
    CodexInvocation, CodexSandboxMode, CodexWrapperOptions, build_codex_invocation, run_codex,
};
pub use commit::{CommitRequest, CommitResult, run_structured_commit};
pub use global_paths::GlobalPaths;
pub use hook::{
    HookOutcome, OmpHookOutcome, handle_omp_post_tool_use_with_runtime,
    handle_omp_pre_tool_use_with_runtime, handle_omp_session_start_with_runtime,
    handle_post_tool_use_in_repo, handle_pre_tool_use, handle_pre_tool_use_in_repo,
    handle_session_start_in_repo, handle_stop_in_repo, handle_user_prompt_submit_in_repo,
};
pub use install::{
    CodexInstallOptions, InstallOptions, InstallPlan, OmpInstallOptions, apply_codex_install,
    apply_global_install, apply_omp_install, current_stateful_binary_path,
    default_codex_config_path, plan_codex_install, plan_global_install, plan_omp_install,
};
pub use lan::{
    ServerJoinOptions, ServerJoinResult, ServerStartRuntimeOptions, ServerStartRuntimeResult,
    join_server_runtime, print_server_start_result, server_join_commands,
    server_start_runtime_result, start_server_runtime,
};
pub use outbox::{sync_outbox_in_repo, sync_outbox_in_repo_with_runtime, sync_outbox_with_runtime};
pub use push::{PushRequest, PushResult, run_structured_push};
pub use repo_registry::{
    RepoEntry, RepoGate, RepoIdentity, RepoRegistry, RepoToolList, allow_tool_for_repo,
    allowed_tools_for_repo, deny_tool_for_repo, detect_git_root, disable_repo,
    effective_workspace_id_for_repo, enable_repo, record_unclassified_tool_for_repo, repo_gate,
    repo_identity_for_enabled_repo, tool_allowed_for_enabled_repo, tool_list_for_repo,
    workspace_id_for_enabled_repo, workspace_id_for_repo_identity,
};
pub use runtime::{
    AgentContext, HttpResponse, ProtocolEnvelopeArgs, ReservationCancelArgs, ReservationClaimArgs,
    ReservationDeclareArgs, ReservationRequestArgs, ServerRuntime, cancel_reservation_via_http,
    claim_reservation_via_http, declare_reservation_via_http, discover_runtime,
    discover_runtime_with_global, discover_runtime_with_optional_global, get_json, get_v2,
    global_state_db_path, post_json, post_v2, post_v2_raw, protocol_envelope, replay_v2_request,
    request_reservation_via_http, reservation_cancel_protocol_body,
    reservation_claim_protocol_body, reservation_declare_protocol_body,
    reservation_request_protocol_body, resolve_runtime_origin, runtime_env_override_is_configured,
    runtime_from_remote, runtime_has_required_identity, runtime_identity_matches_pid,
    runtime_origin_for_authorization, runtime_status, v2_query_envelope, v2_query_for_runtime,
    v2_request_envelope, validate_agent_id, write_global_runtime_file, write_runtime_file,
    RuntimeOrigin,
};
pub use sandbox::{SandboxFsProfile, SandboxNetworkPolicy};
pub use server_lifecycle::{
    ServerStartOptions, detached_server_args, ensure_server, ensure_server_with,
    ensure_server_with_options, restart_server, runtime_is_healthy,
    server_start_options_from_runtime, stop_server,
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
        #[arg(long = "agent", value_enum)]
        agents: Vec<InstallAgent>,
        #[arg(long)]
        codex_config: Option<PathBuf>,
        #[arg(long)]
        binary: Option<String>,
        #[arg(long)]
        update: bool,
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
        #[arg(long, default_value = "awareness", value_parser = ["enforcement", "awareness"])]
        coordination_mode: String,
    },
    Status,
    Current,
    Events,
    Doctor,
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
    Tools(ToolsCommand),
    #[command(subcommand)]
    Notifications(NotificationsCommand),
    #[command(subcommand)]
    Resume(ResumeCommand),
    #[command(subcommand)]
    Reservation(ReservationCommand),
    #[command(subcommand)]
    Human(HumanCommand),
    #[command(subcommand)]
    Reconcile(ReconcileCommand),
    #[command(subcommand)]
    Watch(WatchCommand),
    SyncOutbox,
    #[command(subcommand)]
    Hook(HookRuntime),
}

#[derive(Debug, Subcommand)]
pub enum WatchCommand {
    Run {
        #[arg(long)]
        repo: Option<PathBuf>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum InstallAgent {
    Codex,
    Omp,
}

#[derive(Debug, Subcommand)]
pub enum HookRuntime {
    Codex {
        #[command(subcommand)]
        command: HookCommand,
    },
    Omp {
        #[command(subcommand)]
        command: HookCommand,
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
        #[arg(long, default_value = "awareness", value_parser = ["enforcement", "awareness"])]
        coordination_mode: String,
    },
    Restart,
    Join {
        base_url: String,
        #[arg(long)]
        token: String,
        #[arg(long, default_value = "shared")]
        workspace_id: String,
        #[arg(long)]
        allow_plain_http: bool,
        #[arg(long)]
        enable_repo: bool,
        #[arg(long)]
        binary: Option<String>,
        #[arg(long)]
        codex_config: Option<PathBuf>,
    },
    Stop,
    Status,
}

#[derive(Debug, Subcommand)]
pub enum HumanCommand {
    Observe {
        path: String,
        #[arg(long, default_value = "save")]
        kind: String,
        #[arg(long, default_value = "high")]
        confidence: String,
        #[arg(long, default_value = "watcher:save")]
        source: String,
        #[arg(long, default_value = "Human changed the file.")]
        summary: String,
        #[arg(long, default_value = "human-watcher")]
        agent_id: String,
        #[arg(long)]
        workspace_id: Option<String>,
    },
    SaveCheck {
        paths: Vec<String>,
        #[arg(long)]
        workspace_id: Option<String>,
    },
}

#[derive(Debug, Subcommand)]
pub enum ReconcileCommand {
    Ack {
        #[arg(long = "resource", required = true)]
        resources: Vec<String>,
        #[arg(long = "files-reread", required = true)]
        files_reread: Vec<String>,
        #[arg(long)]
        summary: String,
        #[arg(long, value_parser = ["adopt", "reapply", "ask_user", "abandon"])]
        decision: String,
        #[arg(long = "reservation-id")]
        reservation_id: Option<String>,
        #[arg(long = "agent-id", default_value = "cli-reconcile")]
        agent_id: String,
        #[arg(long)]
        workspace_id: Option<String>,
        #[arg(long)]
        conflict_with_plan: bool,
    },
}

#[derive(Debug, Subcommand)]
pub enum SandboxCommand {
    Run {
        #[arg(long, value_enum, default_value = "read-only")]
        fs: SandboxFsProfile,
        #[arg(long, value_enum, default_value = "disabled")]
        network: SandboxNetworkPolicy,
        #[arg(long)]
        purpose: Option<String>,
        #[arg(long)]
        reservation_id: Option<String>,
        #[arg(long = "agent-id")]
        agent_id: Option<String>,
        #[arg(long = "workspace-id")]
        workspace_id: Option<String>,
        #[arg(long = "write-target")]
        write_targets: Vec<String>,
        #[arg(long = "create-target")]
        create_targets: Vec<String>,
        #[arg(long = "write-dir")]
        write_dirs: Vec<String>,
        #[arg(long = "connect-socket")]
        connect_sockets: Vec<String>,
        #[arg(long)]
        allow_signal: bool,
        #[arg(long)]
        json: bool,
        #[arg(long)]
        command: Option<String>,
        #[arg(long = "sequence")]
        sequences: Vec<String>,
        #[arg(long = "sequence-shell")]
        sequence_shell: Option<String>,
        #[arg(long)]
        timeout_seconds: Option<u64>,
        #[arg(long, hide = true)]
        stream_events: bool,
    },
    Process {
        #[command(subcommand)]
        command: SandboxProcessCommand,
    },
    RunNestedCodexBenchmark {
        #[arg(long)]
        purpose: String,
        #[arg(long = "agent-id")]
        agent_id: String,
        #[arg(long = "workspace-id")]
        workspace_id: Option<String>,
        #[arg(long = "write-dir")]
        write_dir: String,
        #[arg(long = "codex-home-root")]
        codex_home_root: String,
        #[arg(long = "docker-socket")]
        docker_socket: Option<PathBuf>,
        #[arg(long)]
        command: String,
        #[arg(long)]
        timeout_seconds: Option<u64>,
    },
}

#[derive(Debug, Subcommand)]
pub enum SandboxProcessCommand {
    Find {
        #[arg(long = "name")]
        names: Vec<String>,
        #[arg(long = "contains")]
        contains: Vec<String>,
        #[arg(long = "pid")]
        pids: Vec<u32>,
        #[arg(long = "parent-pid", alias = "ppid")]
        parent_pids: Vec<u32>,
        #[arg(long = "process-group", alias = "pgid")]
        process_groups: Vec<u32>,
        #[arg(long = "field")]
        fields: Vec<String>,
        #[arg(long)]
        json: bool,
    },
}

#[derive(Debug, Subcommand)]
pub enum ReservationCommand {
    Declare {
        #[arg(long = "agent-id")]
        agent_id: Option<String>,
        #[arg(long = "workspace-id")]
        workspace_id: Option<String>,
        #[arg(long)]
        purpose: String,
        #[arg(required = true, num_args = 1..)]
        files_planned: Vec<String>,
    },
    Request {
        #[arg(long = "agent-id")]
        agent_id: Option<String>,
        #[arg(long = "workspace-id")]
        workspace_id: Option<String>,
        #[arg(long)]
        request_id: String,
        #[arg(long)]
        reservation_id: Option<String>,
        #[arg(long)]
        action: String,
        #[arg(long)]
        path: String,
        #[arg(long)]
        purpose: String,
    },
    Claim {
        #[arg(long = "agent-id")]
        agent_id: Option<String>,
        #[arg(long = "workspace-id")]
        workspace_id: Option<String>,
        #[arg(long)]
        reservation_id: Option<String>,
        #[arg(long)]
        wait_id: String,
        #[arg(long)]
        path: String,
    },
    Cancel {
        #[arg(long = "agent-id")]
        agent_id: Option<String>,
        #[arg(long = "workspace-id")]
        workspace_id: Option<String>,
        #[arg(long)]
        wait_id: String,
    },
}

#[derive(Debug, Subcommand)]
pub enum ReposCommand {
    List,
}

#[derive(Debug, Subcommand)]
pub enum ToolsCommand {
    Allow {
        tool_name: String,
        #[arg(long)]
        repo: Option<PathBuf>,
    },
    Deny {
        tool_name: String,
        #[arg(long)]
        repo: Option<PathBuf>,
    },
    List {
        #[arg(long)]
        repo: Option<PathBuf>,
    },
}

#[derive(Debug, Subcommand)]
pub enum NotificationsCommand {
    Poll {
        #[arg(long = "agent-id")]
        agent_id: Option<String>,
        #[arg(long)]
        workspace_id: Option<String>,
    },
}

#[derive(Debug, Subcommand)]
pub enum ResumeCommand {
    Next {
        #[arg(long = "agent-id")]
        agent_id: Option<String>,
        #[arg(long)]
        workspace_id: Option<String>,
    },
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
            agents,
            codex_config,
            binary,
            update,
        } => {
            let paths = GlobalPaths::from_env()?;
            let binary_path = match binary {
                Some(path) => Some(path),
                None if agents.is_empty() => None,
                None => Some(current_stateful_binary_path()?),
            };
            let plan = if agents.is_empty() {
                if codex_config.is_some() {
                    anyhow::bail!("--codex-config requires --agent codex");
                }
                if binary_path.is_some() {
                    anyhow::bail!("--binary requires --agent codex or --agent omp");
                }
                if update {
                    anyhow::bail!("--update requires --agent omp");
                }
                apply_global_install(InstallOptions { yes, paths })?
            } else if agents.contains(&InstallAgent::Codex) {
                if update {
                    anyhow::bail!("--update requires --agent omp");
                }
                let codex_config_path = match codex_config {
                    Some(path) => path,
                    None => default_codex_config_path()?,
                };
                apply_codex_install(CodexInstallOptions {
                    yes,
                    paths,
                    codex_config_path,
                    binary_path: binary_path.expect("agent install should resolve binary"),
                })?
            } else if agents.contains(&InstallAgent::Omp) {
                if codex_config.is_some() {
                    anyhow::bail!("--codex-config requires --agent codex");
                }
                apply_omp_install(OmpInstallOptions {
                    yes,
                    paths,
                    binary_path: binary_path.expect("agent install should resolve binary"),
                    project_config_path: None,
                    omp_agent_dir: None,
                    update,
                })?
            } else {
                anyhow::bail!("no supported install agents selected");
            };
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
            coordination_mode: legacy_coordination_mode,
        } => match command.unwrap_or(ServerCommand::Start {
            foreground: true,
            host: legacy_host,
            port: legacy_port,
            token: legacy_token,
            workspace_id: legacy_workspace_id,
            coordination_mode: legacy_coordination_mode,
        }) {
            ServerCommand::Start {
                foreground,
                host,
                port,
                token,
                workspace_id,
                coordination_mode,
            } => {
                if foreground {
                    run_server(host, port, token, workspace_id, coordination_mode)?;
                } else {
                    let paths = GlobalPaths::from_env()?;
                    let result = start_server_runtime(ServerStartRuntimeOptions {
                        paths,
                        host,
                        port,
                        token,
                        workspace_id,
                        coordination_mode,
                    })?;
                    print_server_start_result(&result)?;
                }
            }
            ServerCommand::Restart => {
                let paths = GlobalPaths::from_env()?;
                let runtime = restart_server(&paths)?;
                let options = server_start_options_from_runtime(&runtime)?;
                let result = start_server_runtime(ServerStartRuntimeOptions {
                    paths,
                    host: options.host,
                    port: options.port,
                    token: Some(runtime.token),
                    coordination_mode: options.coordination_mode,
                    workspace_id: options.workspace_id,
                })?;
                print_server_start_result(&result)?;
            }
            ServerCommand::Join {
                base_url,
                token,
                workspace_id,
                allow_plain_http,
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
                let result = join_server_runtime(ServerJoinOptions {
                    paths,
                    codex_config_path,
                    binary_path,
                    base_url,
                    token,
                    workspace_id,
                    allow_plain_http,
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
            ServerCommand::Status => {
                let paths = GlobalPaths::from_env()?;
                let runtime =
                    discover_runtime_with_global(current_repo_root_or_current_dir()?, &paths).ok();
                let runtime = runtime.map(|runtime| {
                    serde_json::json!({
                        "base_url": runtime.base_url,
                        "workspace_id": runtime.workspace_id,
                        "pid": runtime.pid,
                        "token": "<redacted>",
                    })
                });
                println!("{}", serde_json::to_string_pretty(&runtime)?);
            }
            ServerCommand::Stop => {
                let paths = GlobalPaths::from_env()?;
                stop_server(&paths)?;
                println!("{}", serde_json::json!({ "status": "ok" }));
            }
        },
        Command::Status => {
            let report = doctor_report(current_repo_root_or_current_dir()?);
            println!("{}", serde_json::to_string_pretty(&report)?);
        }
        Command::Current => {
            let (repo_root, runtime) = discover_runtime_for_current_dir()?;
            let identity = GlobalPaths::from_env()
                .ok()
                .and_then(|paths| repo_identity_for_enabled_repo(&paths, &repo_root).ok());
            let workspace_id = query_workspace_id(&runtime, identity.as_ref());
            let request = v2_query_for_runtime(
                uuid::Uuid::new_v4(),
                "stateful-cli".to_string(),
                workspace_id,
                identity,
                SourceKind::Cli,
                "current",
                "stateful-cli",
                None,
                serde_json::json!({}),
            )?;
            print_http_response(get_v2(&runtime, "/v2/current", &request)?)?;
        }
        Command::Events => {
            let (repo_root, runtime) = discover_runtime_for_current_dir()?;
            let identity = GlobalPaths::from_env()
                .ok()
                .and_then(|paths| repo_identity_for_enabled_repo(&paths, &repo_root).ok());
            let workspace_id = query_workspace_id(&runtime, identity.as_ref());
            let request = v2_query_for_runtime(
                uuid::Uuid::new_v4(),
                "stateful-cli".to_string(),
                workspace_id,
                identity,
                SourceKind::Cli,
                "events",
                "stateful-cli",
                None,
                serde_json::json!({ "limit": 100 }),
            )?;
            print_http_response(get_v2(&runtime, "/v2/events", &request)?)?;
        }
        Command::SyncOutbox => {
            let synced = sync_outbox_in_repo(current_repo_root_or_current_dir()?)?;
            println!("{}", serde_json::json!({ "synced": synced }));
        }
        Command::Doctor => {
            let report = doctor_report(current_repo_root_or_current_dir()?);
            println!("{}", serde_json::to_string_pretty(&report)?);
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
        Command::Sandbox(SandboxCommand::Run {
            fs,
            network,
            purpose,
            reservation_id,
            agent_id,
            workspace_id,
            write_targets,
            create_targets,
            write_dirs,
            connect_sockets,
            allow_signal,
            json,
            command,
            sequences,
            sequence_shell,
            timeout_seconds,
            stream_events,
        }) => {
            let command = sandbox::resolve_sandbox_run_command(command, sequences, sequence_shell)
                .map_err(anyhow::Error::msg)?;
            let paths = GlobalPaths::from_env()?;
            let repo_root = current_repo_root_or_current_dir()?;
            let output = match sandbox::run_sandbox_in_repo(
                &repo_root,
                &paths,
                sandbox::SandboxRunRequest {
                    fs,
                    network,
                    purpose,
                    reservation_id,
                    agent_id,
                    workspace_id,
                    write_targets,
                    create_targets,
                    write_dirs,
                    connect_sockets,
                    allow_signal,
                    command,
                    timeout_seconds,
                    stream_events,
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
            let rendered = render_sandbox_run_cli_output(&output, json)?;
            std::io::stdout().write_all(&rendered.stdout)?;
            std::io::stderr().write_all(&rendered.stderr)?;
            if let Some(exit_code) = sandbox::sandbox_run_cli_exit_code(&output) {
                std::process::exit(exit_code);
            }
        }
        Command::Sandbox(SandboxCommand::RunNestedCodexBenchmark {
            purpose,
            agent_id,
            workspace_id,
            write_dir,
            codex_home_root,
            docker_socket,
            command,
            timeout_seconds,
        }) => {
            let paths = GlobalPaths::from_env()?;
            let repo_root = current_repo_root_or_current_dir()?;
            let output = match codex_benchmark::run_nested_codex_benchmark_sandbox_in_repo(
                &repo_root,
                &paths,
                codex_benchmark::NestedCodexBenchmarkSandboxRequest {
                    purpose,
                    agent_id,
                    workspace_id,
                    write_dir,
                    codex_home_root,
                    docker_socket,
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
        Command::Sandbox(SandboxCommand::Process {
            command:
                SandboxProcessCommand::Find {
                    names,
                    contains,
                    pids,
                    parent_pids,
                    process_groups,
                    fields,
                    json,
                },
        }) => {
            let output = sandbox::run_sandbox_process_find(sandbox::SandboxProcessFindRequest {
                names,
                contains,
                pids,
                parent_pids,
                process_groups,
                fields,
            })?;
            let rendered = render_sandbox_process_find_cli_output(&output, json)?;
            std::io::stdout().write_all(&rendered.stdout)?;
            std::io::stderr().write_all(&rendered.stderr)?;
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
        Command::Tools(ToolsCommand::Allow { tool_name, repo }) => {
            let paths = GlobalPaths::from_env()?;
            let repo = repo.unwrap_or(std::env::current_dir()?);
            let entry = allow_tool_for_repo(&paths, repo, &tool_name)?;
            println!(
                "{}",
                serde_json::to_string_pretty(&serde_json::json!({
                    "repo": entry.root,
                    "allowed_tools": entry.allowed_tools,
                }))?
            );
        }
        Command::Tools(ToolsCommand::Deny { tool_name, repo }) => {
            let paths = GlobalPaths::from_env()?;
            let repo = repo.unwrap_or(std::env::current_dir()?);
            let entry = deny_tool_for_repo(&paths, repo, &tool_name)?;
            println!(
                "{}",
                serde_json::to_string_pretty(&serde_json::json!({
                    "repo": entry.root,
                    "allowed_tools": entry.allowed_tools,
                }))?
            );
        }
        Command::Tools(ToolsCommand::List { repo }) => {
            let paths = GlobalPaths::from_env()?;
            let repo = repo.unwrap_or(std::env::current_dir()?);
            let root = detect_git_root(&repo)?;
            let tools = tool_list_for_repo(&paths, &root)?;
            println!(
                "{}",
                serde_json::to_string_pretty(&serde_json::json!({
                    "repo": root,
                    "allowed_tools": tools.allowed_tools,
                    "unclassified_tools": tools.unclassified_tools,
                }))?
            );
        }
        Command::Notifications(NotificationsCommand::Poll {
            agent_id,
            workspace_id,
        }) => {
            let (repo_root, runtime) = discover_runtime_for_current_dir()?;
            let (agent_id, workspace_id) =
                resolve_agent_workspace(repo_root.as_path(), &runtime, agent_id, workspace_id)?;
            let identity = GlobalPaths::from_env()
                .ok()
                .and_then(|paths| repo_identity_for_enabled_repo(&paths, &repo_root).ok());
            let request = v2_request_envelope(
                uuid::Uuid::new_v4(),
                agent_id,
                workspace_id,
                identity,
                ActorType::Agent,
                SourceKind::Cli,
                "notifications_poll",
                "stateful-cli",
                None,
                serde_json::json!({}),
            )?;
            print_http_response(post_v2(&runtime, "/v2/notifications/poll", &request)?)?;
        }
        Command::Resume(ResumeCommand::Next {
            agent_id,
            workspace_id,
        }) => {
            let (repo_root, runtime) = discover_runtime_for_current_dir()?;
            let (agent_id, workspace_id) =
                resolve_agent_workspace(repo_root.as_path(), &runtime, agent_id, workspace_id)?;
            let identity = GlobalPaths::from_env()
                .ok()
                .and_then(|paths| repo_identity_for_enabled_repo(&paths, &repo_root).ok());
            let request = v2_request_envelope(
                uuid::Uuid::new_v4(),
                agent_id,
                workspace_id,
                identity,
                ActorType::Agent,
                SourceKind::Cli,
                "resume_next",
                "stateful-cli",
                None,
                serde_json::json!({}),
            )?;
            print_http_response(post_v2(&runtime, "/v2/resume/next", &request)?)?;
        }
        Command::Reservation(ReservationCommand::Declare {
            agent_id,
            workspace_id,
            purpose,
            files_planned,
        }) => {
            let (repo_root, runtime) = discover_runtime_for_current_dir()?;
            let (agent_id, workspace_id) =
                resolve_agent_workspace(repo_root.as_path(), &runtime, agent_id, workspace_id)?;
            let response = declare_reservation_via_http(
                &runtime,
                ReservationDeclareArgs {
                    request_id: uuid::Uuid::new_v4(),
                    agent_id,
                    workspace_id,
                    purpose,
                    files_planned,
                    identity: GlobalPaths::from_env()
                        .ok()
                        .and_then(|paths| repo_identity_for_enabled_repo(&paths, &repo_root).ok()),
                },
            )?;
            print_http_response(response)?;
        }
        Command::Reservation(ReservationCommand::Request {
            agent_id,
            workspace_id,
            request_id,
            reservation_id,
            action,
            path,
            purpose,
        }) => {
            let (repo_root, runtime) = discover_runtime_for_current_dir()?;
            let (agent_id, workspace_id) =
                resolve_agent_workspace(repo_root.as_path(), &runtime, agent_id, workspace_id)?;
            let response = request_reservation_via_http(
                &runtime,
                ReservationRequestArgs {
                    request_id: uuid::Uuid::parse_str(&request_id)?,
                    agent_id,
                    workspace_id,
                    reservation_id,
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
        Command::Reservation(ReservationCommand::Claim {
            agent_id,
            workspace_id,
            wait_id,
            path,
            reservation_id,
        }) => {
            let (repo_root, runtime) = discover_runtime_for_current_dir()?;
            let (agent_id, workspace_id) =
                resolve_agent_workspace(repo_root.as_path(), &runtime, agent_id, workspace_id)?;
            claim_reservation_via_http(
                &runtime,
                ReservationClaimArgs {
                    request_id: uuid::Uuid::new_v4(),
                    agent_id,
                    workspace_id,
                    wait_id,
                    relative_path: path,
                    reservation_id,
                    identity: GlobalPaths::from_env()
                        .ok()
                        .and_then(|paths| repo_identity_for_enabled_repo(&paths, &repo_root).ok()),
                },
            )?;
            println!("claimed stateful reservation");
        }
        Command::Reservation(ReservationCommand::Cancel {
            agent_id,
            workspace_id,
            wait_id,
        }) => {
            let (repo_root, runtime) = discover_runtime_for_current_dir()?;
            let (agent_id, workspace_id) =
                resolve_agent_workspace(repo_root.as_path(), &runtime, agent_id, workspace_id)?;
            cancel_reservation_via_http(
                &runtime,
                ReservationCancelArgs {
                    request_id: uuid::Uuid::new_v4(),
                    agent_id,
                    workspace_id,
                    wait_id,
                    identity: GlobalPaths::from_env()
                        .ok()
                        .and_then(|paths| repo_identity_for_enabled_repo(&paths, &repo_root).ok()),
                },
            )?;
            println!("canceled stateful reservation");
        }
        Command::Human(HumanCommand::Observe {
            path,
            kind,
            confidence,
            source,
            summary,
            agent_id,
            workspace_id,
        }) => {
            let (repo_root, runtime) = discover_runtime_for_current_dir()?;
            let (_, workspace_id) = resolve_agent_workspace(
                repo_root.as_path(),
                &runtime,
                Some(agent_id.clone()),
                workspace_id,
            )?;
            let identity = GlobalPaths::from_env()
                .ok()
                .and_then(|paths| repo_identity_for_enabled_repo(&paths, &repo_root).ok());
            let request = v2_request_envelope(
                uuid::Uuid::new_v4(),
                agent_id,
                workspace_id,
                identity,
                ActorType::Human,
                SourceKind::Watcher,
                "human_observe",
                "stateful-human-observe",
                None,
                serde_json::json!({
                    "relative_path": path,
                    "kind": kind,
                    "confidence": confidence,
                    "source": source,
                    "summary": summary
                }),
            )?;
            print_http_response(post_v2(&runtime, "/v2/human/observe", &request)?)?;
        }
        Command::Human(HumanCommand::SaveCheck {
            paths,
            workspace_id,
        }) => {
            let (repo_root, runtime) = discover_runtime_for_current_dir()?;
            let workspace_id = workspace_id.unwrap_or_else(|| {
                GlobalPaths::from_env()
                    .ok()
                    .and_then(|paths| repo_identity_for_enabled_repo(&paths, &repo_root).ok())
                    .map(|identity| {
                        effective_workspace_id_for_repo(&runtime.workspace_id, Some(&identity))
                    })
                    .unwrap_or_else(|| runtime.workspace_id.clone())
            });
            let identity = GlobalPaths::from_env()
                .ok()
                .and_then(|paths| repo_identity_for_enabled_repo(&paths, &repo_root).ok());
            let request = v2_request_envelope(
                uuid::Uuid::new_v4(),
                "human-watcher".to_string(),
                workspace_id,
                identity,
                ActorType::Human,
                SourceKind::Cli,
                "human_save_check",
                "stateful-human-save-check",
                None,
                serde_json::json!({ "paths": paths }),
            )?;
            print_http_response(post_v2(&runtime, "/v2/human/save-check", &request)?)?;
        }
        Command::Reconcile(ReconcileCommand::Ack {
            resources,
            files_reread,
            summary,
            decision,
            reservation_id,
            agent_id,
            workspace_id,
            conflict_with_plan,
        }) => {
            let (repo_root, runtime) = discover_runtime_for_current_dir()?;
            let (_, workspace_id) = resolve_agent_workspace(
                repo_root.as_path(),
                &runtime,
                Some(agent_id.clone()),
                workspace_id,
            )?;
            let identity = GlobalPaths::from_env()
                .ok()
                .and_then(|paths| repo_identity_for_enabled_repo(&paths, &repo_root).ok());
            let request = v2_request_envelope(
                uuid::Uuid::new_v4(),
                agent_id,
                workspace_id,
                identity,
                ActorType::Agent,
                SourceKind::Cli,
                "reconcile_ack",
                "stateful-reconcile-ack",
                None,
                serde_json::json!({
                    "decision": decision,
                    "files_reread": files_reread,
                    "human_change_summary": summary,
                    "resources": resources,
                    "reservation_id": reservation_id,
                    "conflict_with_plan": conflict_with_plan
                }),
            )?;
            print_http_response(post_v2(&runtime, "/v2/reconcile/ack", &request)?)?;
        }
        Command::Watch(WatchCommand::Run { repo }) => {
            watch::run_watch(repo)?;
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

fn resolve_agent_workspace(
    repo_root: &Path,
    runtime: &ServerRuntime,
    agent_id: Option<String>,
    workspace_id: Option<String>,
) -> anyhow::Result<(String, String)> {
    let agent_id =
        agent_id.ok_or_else(|| anyhow::anyhow!("agent id not provided; pass --agent-id"))?;
    validate_agent_id(&agent_id, "agent_id")?;
    let workspace_id = workspace_id.unwrap_or_else(|| {
        GlobalPaths::from_env()
            .ok()
            .and_then(|paths| repo_identity_for_enabled_repo(&paths, repo_root).ok())
            .map(|identity| effective_workspace_id_for_repo(&runtime.workspace_id, Some(&identity)))
            .unwrap_or_else(|| runtime.workspace_id.clone())
    });

    Ok((agent_id, workspace_id))
}

fn query_workspace_id(runtime: &ServerRuntime, identity: Option<&RepoIdentity>) -> String {
    effective_workspace_id_for_repo(&runtime.workspace_id, identity)
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
    coordination_mode: String,
) -> anyhow::Result<()> {
    let token = token.unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
    let base_url = format!("http://{host}:{port}");
    let paths = GlobalPaths::from_env()?;
    let options = ServerStartOptions {
        host: host.clone(),
        port,
        token: Some(token.clone()),
        workspace_id: workspace_id.clone(),
        coordination_mode: coordination_mode.clone(),
    };
    let mut runtime = ServerRuntime::new(&base_url, &token, workspace_id, std::process::id());
    runtime.coordination_mode = coordination_mode.clone();
    let store = stateful_store::Store::open(global_state_db_path(&paths))?;

    let addr: SocketAddr = format!("{host}:{port}").parse()?;
    let tokio_runtime = tokio::runtime::Runtime::new()?;
    tokio_runtime.block_on(async move {
        let listener = tokio::net::TcpListener::bind(addr).await?;
        server_lifecycle::register_foreground_runtime(&paths, &runtime, &options)?;
        let result = server_start_runtime_result(runtime.clone(), &host, port);
        print_server_start_result(&result)?;
        let coordination_mode = stateful_server::CoordinationMode::from_str(&coordination_mode)
            .map_err(|message| anyhow::anyhow!(message))?;
        stateful_server::serve_listener(
            listener,
            stateful_server::ServerConfig::with_store(token, store)
                .with_coordination_mode(coordination_mode),
        )
        .await
    })
}

pub fn state_db_path(repo_root: impl AsRef<Path>) -> std::path::PathBuf {
    repo_root.as_ref().join(".stateful_core").join("state.db")
}

const DEFAULT_DOCTOR_JOURNAL_THRESHOLD_BYTES: u64 = 512 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct DoctorJournalTimeRange {
    pub earliest: Option<String>,
    pub latest: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct DoctorJournalDiagnostics {
    pub available: bool,
    pub size_bytes: u64,
    pub rows: u64,
    pub event_types: Vec<String>,
    pub time_range: DoctorJournalTimeRange,
    pub growth_bytes: u64,
    pub growth_status: String,
    pub threshold_bytes: u64,
    pub warning: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct DoctorRuntimeDiagnostics {
    pub configured: bool,
    pub ready: bool,
    pub protocol_version: Option<String>,
    pub journal_schema_version: Option<u64>,
    pub replay_status: String,
    pub capabilities: Vec<String>,
    pub coordination_mode: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct DoctorReport {
    pub installed: bool,
    pub legacy_hooks_json: bool,
    pub config_yml: bool,
    pub runtime_server_json: bool,
    pub legacy_repo_state_db: bool,
    pub global_config_yml: bool,
    pub global_runtime_server_json: bool,
    pub global_state_db: bool,
    pub repo_enabled: bool,
    pub global_paths_error: Option<String>,
    pub journal: DoctorJournalDiagnostics,
    pub runtime: DoctorRuntimeDiagnostics,
    pub capabilities: Vec<String>,
    pub coordination_mode: Option<String>,
    pub global_registry_error: Option<String>,
}

fn doctor_journal_threshold_bytes() -> u64 {
    std::env::var("STATEFUL_DOCTOR_JOURNAL_THRESHOLD_BYTES")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(DEFAULT_DOCTOR_JOURNAL_THRESHOLD_BYTES)
}

const DOCTOR_JOURNAL_BASELINE_MAX_AGE: Duration = Duration::from_secs(24 * 60 * 60);

#[derive(Debug, serde::Serialize, serde::Deserialize)]
struct DoctorJournalBaseline {
    observed_at_unix_seconds: u64,
    footprint_bytes: u64,
}

fn doctor_journal_sidecar_path(path: &Path, suffix: &str) -> PathBuf {
    let mut file_name = path.file_name().unwrap_or_default().to_os_string();
    file_name.push(suffix);
    path.with_file_name(file_name)
}

fn doctor_journal_footprint_bytes(path: &Path) -> u64 {
    std::fs::metadata(path)
        .map(|metadata| metadata.len())
        .unwrap_or(0)
        .saturating_add(
            std::fs::metadata(doctor_journal_sidecar_path(path, "-wal"))
                .map(|metadata| metadata.len())
                .unwrap_or(0),
        )
}

fn doctor_journal_baseline_path(path: &Path) -> PathBuf {
    doctor_journal_sidecar_path(path, ".doctor-baseline.json")
}

fn doctor_journal_baseline_is_recent(baseline: &DoctorJournalBaseline, now: SystemTime) -> bool {
    UNIX_EPOCH
        .checked_add(Duration::from_secs(baseline.observed_at_unix_seconds))
        .and_then(|observed_at| now.duration_since(observed_at).ok())
        .is_some_and(|age| age <= DOCTOR_JOURNAL_BASELINE_MAX_AGE)
}

fn write_doctor_journal_baseline(path: &Path, baseline: &DoctorJournalBaseline) -> bool {
    let Ok(contents) = serde_json::to_vec(baseline) else {
        return false;
    };
    let temp_path = doctor_journal_sidecar_path(path, &format!(".{}.tmp", uuid::Uuid::new_v4()));
    let result = (|| -> std::io::Result<()> {
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp_path)?;
        file.write_all(&contents)?;
        file.sync_all()?;
        std::fs::rename(&temp_path, path)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(temp_path);
        return false;
    }
    true
}

fn doctor_journal_diagnostics(path: &Path) -> DoctorJournalDiagnostics {
    let threshold_bytes = doctor_journal_threshold_bytes();
    let size_bytes = doctor_journal_footprint_bytes(path);
    let baseline_path = doctor_journal_baseline_path(path);
    let now = SystemTime::now();
    let baseline = std::fs::read(&baseline_path)
        .ok()
        .and_then(|contents| serde_json::from_slice::<DoctorJournalBaseline>(&contents).ok())
        .filter(|baseline| doctor_journal_baseline_is_recent(baseline, now));
    let has_recent_baseline = baseline.is_some();
    let (growth_bytes, growth_status) = match baseline {
        Some(baseline) => (
            size_bytes.saturating_sub(baseline.footprint_bytes),
            "measured".to_string(),
        ),
        None => (0, "baseline_captured".to_string()),
    };
    let default = DoctorJournalDiagnostics {
        available: false,
        size_bytes,
        rows: 0,
        event_types: Vec::new(),
        time_range: DoctorJournalTimeRange {
            earliest: None,
            latest: None,
        },
        growth_bytes,
        growth_status,
        threshold_bytes,
        warning: size_bytes >= threshold_bytes,
    };
    let mut report = match stateful_store::inspect_journal(path) {
        Ok(journal) => DoctorJournalDiagnostics {
            available: true,
            rows: journal.rows,
            event_types: journal.event_types,
            time_range: DoctorJournalTimeRange {
                earliest: journal.earliest,
                latest: journal.latest,
            },
            ..default
        },
        Err(_) => default,
    };
    let observed_at_unix_seconds = now.duration_since(UNIX_EPOCH).unwrap_or_default().as_secs();
    let baseline_written = write_doctor_journal_baseline(
        &baseline_path,
        &DoctorJournalBaseline {
            observed_at_unix_seconds,
            footprint_bytes: size_bytes,
        },
    );
    if !has_recent_baseline && !baseline_written {
        report.growth_status = "baseline_write_failed".to_string();
    }
    report
}

fn doctor_runtime_diagnostics(paths: &GlobalPaths) -> DoctorRuntimeDiagnostics {
    let configured_runtime = std::fs::read_to_string(&paths.server_json)
        .ok()
        .and_then(|contents| serde_json::from_str::<ServerRuntime>(&contents).ok());
    let Some(runtime) = configured_runtime else {
        return DoctorRuntimeDiagnostics {
            configured: false,
            ready: false,
            protocol_version: None,
            journal_schema_version: None,
            replay_status: "unavailable".to_string(),
            capabilities: Vec::new(),
            coordination_mode: None,
        };
    };
    let Ok(status) = runtime_status(&runtime) else {
        return DoctorRuntimeDiagnostics {
            configured: true,
            ready: false,
            protocol_version: Some(runtime.protocol_version),
            journal_schema_version: None,
            replay_status: "unavailable".to_string(),
            capabilities: Vec::new(),
            coordination_mode: None,
        };
    };
    let ready = status.protocol_version == "stateful.v2" && status.journal_schema_version == 2;
    DoctorRuntimeDiagnostics {
        configured: true,
        ready,
        protocol_version: Some(status.protocol_version),
        journal_schema_version: Some(status.journal_schema_version),
        replay_status: if ready { "ready" } else { "unavailable" }.to_string(),
        capabilities: status.capabilities,
        coordination_mode: Some(status.coordination_mode),
    }
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
    let legacy_hooks_json = repo_root.join(".codex").join("hooks.json").is_file();
    let codex_config_toml = repo_root.join(".codex").join("config.toml").is_file();
    let config_yml = repo_root.join(".stateful").join("config.yml").is_file();
    let runtime_server_json = repo_root
        .join(".stateful_core")
        .join("runtime")
        .join("server.json")
        .is_file();
    let legacy_repo_state_db = state_db_path(repo_root).is_file();
    let (repo_enabled, global_registry_error) = match RepoRegistry::load(paths) {
        Ok(registry) => (registry.is_enabled(repo_root), None),
        Err(error) => (false, Some(error.to_string())),
    };
    let journal = doctor_journal_diagnostics(&paths.state_db);
    let runtime = doctor_runtime_diagnostics(paths);
    let capabilities = runtime.capabilities.clone();
    let coordination_mode = runtime.coordination_mode.clone();

    DoctorReport {
        installed: (codex_config_toml || paths.config_yml.is_file()) && config_yml,
        legacy_hooks_json,
        config_yml,
        runtime_server_json,
        legacy_repo_state_db,
        global_config_yml: paths.config_yml.is_file(),
        global_runtime_server_json: paths.server_json.is_file(),
        global_state_db: paths.state_db.is_file(),
        repo_enabled,
        global_paths_error: None,
        journal,
        runtime,
        capabilities,
        coordination_mode,
        global_registry_error,
    }
}

struct SandboxRunCliOutput {
    stdout: Vec<u8>,
    stderr: Vec<u8>,
}

fn render_sandbox_run_cli_output(
    output: &sandbox::SandboxRunOutput,
    json: bool,
) -> anyhow::Result<SandboxRunCliOutput> {
    if json {
        let mut stdout = serde_json::to_vec(output)?;
        stdout.push(b'\n');
        return Ok(SandboxRunCliOutput {
            stdout,
            stderr: Vec::new(),
        });
    }

    let mut stderr = output.stderr.as_bytes().to_vec();
    if output.status != "exited" {
        if !stderr.is_empty() && !stderr.ends_with(b"\n") {
            stderr.push(b'\n');
        }
        stderr.extend_from_slice(format!("stateful sandbox run {}\n", output.status).as_bytes());
    }

    Ok(SandboxRunCliOutput {
        stdout: output.stdout.as_bytes().to_vec(),
        stderr,
    })
}

fn render_sandbox_process_find_cli_output(
    output: &sandbox::SandboxProcessFindOutput,
    json: bool,
) -> anyhow::Result<SandboxRunCliOutput> {
    let mut stdout = if json {
        serde_json::to_vec(output)?
    } else {
        serde_json::to_vec(&output.processes)?
    };
    stdout.push(b'\n');
    Ok(SandboxRunCliOutput {
        stdout,
        stderr: Vec::new(),
    })
}

fn default_config_yml() -> &'static str {
    r#"# stateful-core repo policy config
# These are informational target defaults for this repository.
# Runtime loading of these keys is not yet shipped.
protocol_version: stateful.v2
intent_ttl_seconds: 900
intent_max_seconds: 3600
claim_ttl_seconds: 300
reservation_ttl_seconds: 120
directory_scope_depth: 2
delete_requires_exact_file_scope: true
rename_requires_exact_file_scope: true
default_write_policy: deny
event_retention_days: 14
    "#
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sandbox_output(status: &'static str, exit_code: Option<i32>) -> sandbox::SandboxRunOutput {
        sandbox::SandboxRunOutput {
            status,
            exit_code,
            stdout: "child-out".to_string(),
            stderr: "child-err".to_string(),
            allowed_write_targets: Vec::new(),
            denied_write_targets: Vec::new(),
            warnings: Vec::new(),
        }
    }

    fn process_find_output() -> sandbox::SandboxProcessFindOutput {
        sandbox::SandboxProcessFindOutput {
            status: "ok",
            processes: vec![serde_json::json!({ "pid": 202 })],
        }
    }

    #[test]
    fn sandbox_run_cli_default_renders_child_streams_only() {
        let rendered = render_sandbox_run_cli_output(&sandbox_output("exited", Some(7)), false)
            .expect("passthrough rendering should succeed");

        assert_eq!(rendered.stdout, b"child-out");
        assert_eq!(rendered.stderr, b"child-err");
    }

    #[test]
    fn sandbox_run_cli_json_renders_result_envelope() {
        let rendered = render_sandbox_run_cli_output(&sandbox_output("exited", Some(5)), true)
            .expect("json rendering should succeed");
        let body: serde_json::Value =
            serde_json::from_slice(&rendered.stdout).expect("stdout should be json");

        assert_eq!(rendered.stderr, b"");
        assert_eq!(body["status"], "exited");
        assert_eq!(body["exit_code"], 5);
        assert_eq!(body["stdout"], "child-out");
        assert_eq!(body["stderr"], "child-err");
    }

    #[test]
    fn sandbox_process_find_cli_default_renders_processes_only() {
        let rendered = render_sandbox_process_find_cli_output(&process_find_output(), false)
            .expect("passthrough rendering should succeed");
        let body: serde_json::Value =
            serde_json::from_slice(&rendered.stdout).expect("stdout should be json");

        assert_eq!(rendered.stderr, b"");
        assert_eq!(body, serde_json::json!([{ "pid": 202 }]));
    }

    #[test]
    fn sandbox_process_find_cli_json_renders_status_envelope() {
        let rendered = render_sandbox_process_find_cli_output(&process_find_output(), true)
            .expect("json rendering should succeed");
        let body: serde_json::Value =
            serde_json::from_slice(&rendered.stdout).expect("stdout should be json");

        assert_eq!(rendered.stderr, b"");
        assert_eq!(body["status"], "ok");
        assert_eq!(body["processes"], serde_json::json!([{ "pid": 202 }]));
    }
    #[test]
    fn current_and_events_query_the_enabled_repo_workspace_identity() {
        let runtime = ServerRuntime::new("http://127.0.0.1:43873", "token", "local", 0);
        let identity = RepoIdentity {
            repo_id: "repo-abc123".to_string(),
            worktree_id: "worktree-def456".to_string(),
            root: "/repo".to_string(),
            branch: "main".to_string(),
        };

        assert_eq!(
            query_workspace_id(&runtime, Some(&identity)),
            effective_workspace_id_for_repo("local", Some(&identity)),
            "enabled repo queries must use the hook workspace derivation"
        );
        assert_eq!(
            query_workspace_id(&runtime, None),
            "local",
            "outside a repository queries retain the global workspace"
        );
        let request = v2_query_for_runtime(
            uuid::Uuid::new_v4(),
            "stateful-cli".to_string(),
            query_workspace_id(&runtime, Some(&identity)),
            Some(identity),
            SourceKind::Cli,
            "current",
            "stateful-cli",
            None,
            serde_json::json!({}),
        )
        .expect("enabled repo query should build");
        let body = serde_json::to_value(request).expect("query should serialize");
        assert_eq!(body["workspace_id"], "workspace-worktree-def456");
        assert_eq!(body["repo_id"], "repo-abc123");
        assert_eq!(body["worktree_id"], "worktree-def456");
    }


}
