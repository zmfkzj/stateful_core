use clap::{Parser, Subcommand};
use std::{
    fs,
    net::SocketAddr,
    path::{Path, PathBuf},
};

mod codex_wrapper;
mod commit;
mod global_paths;
mod hook;
mod install;
mod mcp;
mod outbox;
mod push;
mod repo_registry;
mod runtime;
mod sandbox;
mod server_lifecycle;
mod validation;

pub use codex_wrapper::{
    CodexInvocation, CodexSandboxMode, CodexWrapperOptions, build_codex_invocation, run_codex,
};
pub use commit::{CommitRequest, CommitResult, run_structured_commit};
pub use global_paths::GlobalPaths;
pub use hook::{
    HookOutcome, handle_post_tool_use_in_repo, handle_pre_tool_use, handle_pre_tool_use_in_repo,
    handle_session_start_in_repo, handle_stop_in_repo, handle_user_prompt_submit_in_repo,
};
pub use install::{
    InstallOptions, InstallPlan, apply_global_install, current_stateful_binary_path,
    default_codex_config_path, plan_global_install,
};
pub use mcp::{call_mcp_tool_in_repo, handle_mcp_jsonrpc_in_repo, serve_mcp_stdio_in_repo};
pub use outbox::{sync_outbox_in_repo, sync_outbox_in_repo_with_runtime};
pub use push::{PushRequest, PushResult, run_structured_push};
pub use repo_registry::{
    CodexMode, RepoEntry, RepoGate, RepoIdentity, RepoRegistry, detect_git_root, disable_repo,
    enable_repo, repo_gate, repo_identity_for_enabled_repo,
};
pub use runtime::{
    CurrentSession, HttpResponse, IntentDeclareArgs, ProtocolEnvelopeArgs,
    STATEFUL_CODEX_RUN_ID_ENV, ServerRuntime, declare_intent_via_http, discover_runtime,
    discover_runtime_with_global, discover_runtime_with_optional_global, get_json,
    global_state_db_path, intent_declare_protocol_body, post_json, protocol_envelope,
    read_current_session_file, read_current_session_file_for_codex_run, write_current_session_file,
    write_current_session_file_for_codex_run, write_global_runtime_file, write_runtime_file,
};
pub use sandbox::{SandboxFsProfile, SandboxNetworkPolicy};
pub use server_lifecycle::{
    ServerStartOptions, detached_server_args, ensure_server, ensure_server_with,
    ensure_server_with_options, runtime_is_healthy, stop_server,
};
pub use validation::{
    ResultParser, ValidationConfig, ValidationProfile, ValidationResult, ValidationStatus,
    run_validation_profile,
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
    Init {
        #[arg(long, default_value = "target/debug/stateful")]
        binary: String,
    },
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
    Status,
    Current,
    Events,
    Doctor,
    Validate {
        profile: String,
    },
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
    Sandbox(SandboxCommand),
    Enable {
        #[arg(long)]
        repo: Option<PathBuf>,
        #[arg(long)]
        repo_local_codex: bool,
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
        files_planned: Vec<String>,
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
        Command::Init { binary } => {
            install_repo_local_avoiding_global_hook_duplicates(std::env::current_dir()?, binary)?;
            println!("installed stateful repo-local configuration");
        }
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
        Command::Validate { profile } => {
            let result = run_validation_profile(current_repo_root_or_current_dir()?, &profile)?;
            println!("{}", serde_json::to_string(&result)?);
            if !matches!(result.status, ValidationStatus::Passed) {
                std::process::exit(1);
            }
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
        Command::Sandbox(SandboxCommand::Run {
            fs,
            network,
            write_targets,
            create_targets,
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
        Command::Enable {
            repo,
            repo_local_codex,
        } => {
            let paths = GlobalPaths::from_env()?;
            let repo = repo.unwrap_or(std::env::current_dir()?);
            let global_codex_config = if repo_local_codex {
                default_codex_config_path().ok()
            } else {
                None
            };
            let entry = repo_registry::enable_repo_with_global_codex_config(
                &paths,
                repo,
                repo_local_codex,
                global_codex_config.as_deref(),
            )?;
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
                    files_planned,
                    identity: GlobalPaths::from_env()
                        .ok()
                        .and_then(|paths| repo_identity_for_enabled_repo(&paths, &repo_root).ok()),
                },
            )?;
            println!("declared stateful intent");
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
    pub validation_yml: bool,
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
    let validation_yml = repo_root.join(".stateful").join("validation.yml").is_file();
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
        installed: (hooks_json || codex_config_toml || paths.config_yml.is_file())
            && config_yml
            && validation_yml,
        hooks_json,
        config_yml,
        validation_yml,
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

pub fn install_repo_local(
    repo_root: impl AsRef<Path>,
    binary_path: impl AsRef<str>,
) -> anyhow::Result<()> {
    install_repo_local_with_hooks(repo_root, binary_path, true)
}

pub fn install_repo_local_avoiding_global_hook_duplicates(
    repo_root: impl AsRef<Path>,
    binary_path: impl AsRef<str>,
) -> anyhow::Result<()> {
    let global_codex_config = default_codex_config_path().ok();
    install_repo_local_with_global_codex_config(
        repo_root,
        binary_path,
        global_codex_config.as_deref(),
    )
}

pub fn install_repo_local_with_global_codex_config(
    repo_root: impl AsRef<Path>,
    binary_path: impl AsRef<str>,
    global_codex_config: Option<&Path>,
) -> anyhow::Result<()> {
    let include_hooks = match global_codex_config {
        Some(path) => !global_codex_config_has_stateful_hooks(path)?,
        None => true,
    };
    install_repo_local_with_hooks(repo_root, binary_path, include_hooks)
}

fn install_repo_local_with_hooks(
    repo_root: impl AsRef<Path>,
    binary_path: impl AsRef<str>,
    include_hooks: bool,
) -> anyhow::Result<()> {
    let repo_root = repo_root.as_ref();
    let binary_path = binary_path.as_ref();

    ensure_repo_local_install_can_write(repo_root)?;

    fs::create_dir_all(repo_root.join(".codex"))?;
    fs::create_dir_all(repo_root.join(".codex/skills/stateful-command-policy"))?;
    fs::create_dir_all(repo_root.join(".stateful"))?;

    let hooks_json = repo_root.join(".codex/hooks.json");
    if hooks_json.exists() {
        fs::remove_file(&hooks_json)?;
    }
    fs::write(
        repo_root.join(".codex/config.toml"),
        repo_local_codex_config_toml(binary_path, include_hooks),
    )?;
    fs::write(
        repo_root.join(".codex/skills/stateful-command-policy/SKILL.md"),
        stateful_command_policy_skill(),
    )?;
    fs::write(repo_root.join(".stateful/config.yml"), default_config_yml())?;
    fs::write(
        repo_root.join(".stateful/validation.yml"),
        default_validation_yml(),
    )?;

    Ok(())
}

const STATEFUL_GLOBAL_CODEX_BLOCK_START: &str = "# stateful-core-global-install";
const STATEFUL_GLOBAL_CODEX_BLOCK_END: &str = "# /stateful-core-global-install";

fn global_codex_config_has_stateful_hooks(path: &Path) -> anyhow::Result<bool> {
    if !path.exists() {
        return Ok(false);
    }
    let contents = fs::read_to_string(path)?;
    let Some(start) = contents.find(STATEFUL_GLOBAL_CODEX_BLOCK_START) else {
        return Ok(false);
    };
    let block = &contents[start..];
    let block = match block.find(STATEFUL_GLOBAL_CODEX_BLOCK_END) {
        Some(end) => &block[..end],
        None => block,
    };

    Ok(block.contains("[[hooks.SessionStart]]")
        || block.contains("[[hooks.UserPromptSubmit]]")
        || block.contains("[[hooks.PreToolUse]]")
        || block.contains("[[hooks.PostToolUse]]")
        || block.contains("[[hooks.Stop]]"))
}

const STATEFUL_CODEX_JSON_MARKER: &str = "stateful_core_owned";
const STATEFUL_CODEX_TOML_MARKER: &str = "# stateful-core-owned";

pub(crate) fn ensure_repo_local_install_can_write(repo_root: &Path) -> anyhow::Result<()> {
    let hooks_json = repo_root.join(".codex/hooks.json");
    let config_toml = repo_root.join(".codex/config.toml");
    let has_legacy_stateful_hooks =
        hooks_json.exists() && is_legacy_stateful_hooks_file(&hooks_json)?;

    if hooks_json.exists()
        && !is_stateful_owned_codex_file(&hooks_json)?
        && !has_legacy_stateful_hooks
    {
        anyhow::bail!(
            "repo-local Codex install would overwrite existing Codex config {}",
            hooks_json.display()
        );
    }

    if config_toml.exists()
        && !is_stateful_owned_codex_file(&config_toml)?
        && !(has_legacy_stateful_hooks && is_legacy_stateful_codex_toml_file(&config_toml)?)
    {
        anyhow::bail!(
            "repo-local Codex install would overwrite existing Codex config {}",
            config_toml.display()
        );
    }

    Ok(())
}

fn is_stateful_owned_codex_file(path: &Path) -> anyhow::Result<bool> {
    let contents = fs::read_to_string(path)?;

    match path.file_name().and_then(|name| name.to_str()) {
        Some("hooks.json") => {
            let Ok(value) = serde_json::from_str::<serde_json::Value>(&contents) else {
                return Ok(false);
            };
            Ok(value
                .get(STATEFUL_CODEX_JSON_MARKER)
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false))
        }
        Some("config.toml") => Ok(contents
            .lines()
            .any(|line| line.trim() == STATEFUL_CODEX_TOML_MARKER)),
        _ => Ok(false),
    }
}

fn is_legacy_stateful_hooks_file(path: &Path) -> anyhow::Result<bool> {
    let contents = fs::read_to_string(path)?;
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&contents) else {
        return Ok(false);
    };

    Ok(is_legacy_stateful_hooks_json(&value))
}

fn is_legacy_stateful_codex_toml_file(path: &Path) -> anyhow::Result<bool> {
    let contents = fs::read_to_string(path)?;
    Ok(is_legacy_stateful_codex_toml(&contents))
}

const LEGACY_STATEFUL_HOOKS: &[(&str, &str)] = &[
    ("SessionStart", "session-start"),
    ("UserPromptSubmit", "user-prompt-submit"),
    ("PreToolUse", "pre-tool-use"),
    ("PostToolUse", "post-tool-use"),
    ("Stop", "stop"),
];

fn is_legacy_stateful_hooks_json(value: &serde_json::Value) -> bool {
    let Some(root) = value.as_object() else {
        return false;
    };
    if root.keys().any(|key| key != "hooks") {
        return false;
    }

    let Some(hooks) = root.get("hooks").and_then(serde_json::Value::as_object) else {
        return false;
    };
    if hooks.len() != LEGACY_STATEFUL_HOOKS.len() {
        return false;
    }

    LEGACY_STATEFUL_HOOKS.iter().all(|(event, action)| {
        let Some(entries) = hooks.get(*event).and_then(serde_json::Value::as_array) else {
            return false;
        };
        if entries.is_empty() {
            return false;
        }

        entries.iter().all(|entry| {
            let Some(commands) = entry.get("hooks").and_then(serde_json::Value::as_array) else {
                return false;
            };
            if commands.is_empty() {
                return false;
            }

            commands.iter().all(|command_hook| {
                let hook_type = command_hook
                    .get("type")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or_default();
                let command = command_hook
                    .get("command")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or_default();

                hook_type == "command" && command_invokes_stateful_hook(command, action)
            })
        })
    })
}

fn command_invokes_stateful_hook(command: &str, action: &str) -> bool {
    let suffix = format!(" hook {action}");
    let Some(binary) = command.trim().strip_suffix(&suffix) else {
        return false;
    };

    is_stateful_binary_reference(binary.trim())
}

fn is_stateful_binary_reference(binary: &str) -> bool {
    let binary = strip_matching_double_quotes(binary.trim());
    binary == "stateful" || binary.ends_with("/stateful")
}

fn strip_matching_double_quotes(value: &str) -> &str {
    value
        .strip_prefix('"')
        .and_then(|value| value.strip_suffix('"'))
        .unwrap_or(value)
}

fn is_legacy_stateful_codex_toml(contents: &str) -> bool {
    let mut saw_section = false;
    let mut current_section_is_stateful_mcp = false;
    let mut saw_stateful_mcp_server = false;
    let mut saw_stateful_command = false;

    for line in contents.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }

        if trimmed.starts_with('[') {
            let Some(section) = simple_toml_table_name(trimmed) else {
                return false;
            };
            if !is_legacy_stateful_codex_toml_section(section) {
                return false;
            }

            saw_stateful_mcp_server |= section == "mcp_servers.stateful";
            current_section_is_stateful_mcp = section == "mcp_servers.stateful";
            saw_section = true;
            continue;
        }

        if !saw_section {
            return false;
        }

        if current_section_is_stateful_mcp {
            if let Some(command) = simple_toml_string_assignment(trimmed, "command") {
                saw_stateful_command = is_stateful_binary_reference(&command);
            }
        }
    }

    saw_stateful_mcp_server && saw_stateful_command
}

fn simple_toml_table_name(line: &str) -> Option<&str> {
    let body = line.strip_prefix('[')?.strip_suffix(']')?;
    if body.starts_with('[') || body.ends_with(']') || body.is_empty() {
        return None;
    }

    if body.split('.').all(|segment| {
        !segment.is_empty()
            && segment.chars().all(|character| {
                character.is_ascii_alphanumeric() || character == '_' || character == '-'
            })
    }) {
        Some(body)
    } else {
        None
    }
}

fn is_legacy_stateful_codex_toml_section(section: &str) -> bool {
    section == "mcp_servers.stateful" || section.starts_with("mcp_servers.stateful.tools.")
}

fn simple_toml_string_assignment(line: &str, key: &str) -> Option<String> {
    let (left, right) = line.split_once('=')?;
    if left.trim() != key {
        return None;
    }

    serde_json::from_str(right.trim()).ok()
}

fn repo_local_codex_config_toml(binary_path: &str, include_hooks: bool) -> String {
    let mcp_command = if Path::new(binary_path).is_absolute() {
        binary_path.to_string()
    } else if binary_path.contains('/') {
        format!("./{binary_path}")
    } else {
        binary_path.to_string()
    };
    let mcp_config = format!(
        r#"{STATEFUL_CODEX_TOML_MARKER}
[mcp_servers.stateful]
command = "{}"
args = ["mcp", "serve"]
startup_timeout_sec = 20
"#,
        escape_toml_string(&mcp_command),
    );
    if !include_hooks {
        return mcp_config;
    }

    let hook_binary = if Path::new(binary_path).is_absolute() {
        binary_path.to_string()
    } else {
        format!("$(git rev-parse --show-toplevel)/{binary_path}")
    };
    let hook_prefix = format!("\"{hook_binary}\" hook");

    format!(
        r#"{STATEFUL_CODEX_TOML_MARKER}
[features]
hooks = true

[mcp_servers.stateful]
command = "{}"
args = ["mcp", "serve"]
startup_timeout_sec = 20

[[hooks.SessionStart]]
matcher = "startup|resume|clear|compact"

[[hooks.SessionStart.hooks]]
type = "command"
command = "{} session-start"
statusMessage = "Loading stateful current state"

[[hooks.UserPromptSubmit]]

[[hooks.UserPromptSubmit.hooks]]
type = "command"
command = "{} user-prompt-submit"
statusMessage = "Checking stateful intent context"

[[hooks.PreToolUse]]
matcher = "Bash|apply_patch|Edit|Write|file_change|mcp__filesystem__.*"

[[hooks.PreToolUse.hooks]]
type = "command"
command = "{} pre-tool-use"
statusMessage = "Authorizing stateful tool use"

[[hooks.PostToolUse]]
matcher = "Bash|apply_patch|Edit|Write|file_change|mcp__filesystem__.*"

[[hooks.PostToolUse.hooks]]
type = "command"
command = "{} post-tool-use"
statusMessage = "Recording stateful activity"

[[hooks.Stop]]

[[hooks.Stop.hooks]]
type = "command"
command = "{} stop"
statusMessage = "Finalizing stateful activity"
"#,
        escape_toml_string(&mcp_command),
        escape_toml_string(&hook_prefix),
        escape_toml_string(&hook_prefix),
        escape_toml_string(&hook_prefix),
        escape_toml_string(&hook_prefix),
        escape_toml_string(&hook_prefix)
    )
}

fn escape_toml_string(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
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

fn default_validation_yml() -> &'static str {
    r#"profiles:
  - profile_id: cargo-test
    description: Run the Rust workspace test suite
    command: cargo test --workspace
    cwd: .
    timeout_seconds: 300
    allowed_writes:
      - target/**
    denied_writes:
      - crates/**
      - docs/**
      - Cargo.toml
      - Cargo.lock
    exclusive: true
    result_parser: exit_code
"#
}

fn stateful_command_policy_skill() -> &'static str {
    include_str!("../assets/stateful-command-policy/SKILL.md")
}
