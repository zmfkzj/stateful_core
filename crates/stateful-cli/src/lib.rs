use clap::{Parser, Subcommand, ValueEnum};
use std::{
    io::{Read, Write},
    net::SocketAddr,
    path::{Path, PathBuf},
};

mod codex_wrapper;
mod commit;
mod global_paths;
mod hook;
mod install;
mod push;
mod repo_registry;
mod runtime;
pub mod runtime_contract;
mod sandbox;
mod server_lifecycle;
mod shell_command;

pub use codex_wrapper::{CodexInvocation, CodexWrapperOptions, build_codex_invocation, run_codex};
pub use commit::{CommitRequest, CommitResult, run_structured_commit};
pub use global_paths::GlobalPaths;
pub use hook::{
    HookOutcome, OmpHookOutcome, handle_omp_post_tool_use_with_runtime,
    handle_omp_pre_tool_use_with_runtime, handle_omp_session_start_with_runtime,
    handle_pre_tool_use, handle_pre_tool_use_in_repo, handle_stop_in_repo,
    handle_user_prompt_submit_in_repo,
};
pub use install::{
    CodexInstallOptions, InstallOptions, InstallPlan, OmpInstallOptions, apply_codex_install,
    apply_global_install, apply_omp_install, current_stateful_binary_path,
    default_codex_config_path, plan_codex_install, plan_global_install, plan_omp_install,
};
pub use push::{PushRequest, PushResult, run_structured_push};
pub use repo_registry::{
    RepoEntry, RepoGate, RepoIdentity, RepoRegistry, detect_git_root, disable_repo, enable_repo,
    repo_gate, repo_identity_for_enabled_repo, workspace_id_for_enabled_repo,
};
pub use runtime::{
    CommandError, CommandIdentity, ServerRuntime, discover_runtime, discover_runtime_with_global,
    discover_runtime_with_optional_global, get_payload, global_state_db_path, now_rfc3339,
    post_command, process_parent_pid, process_start_identity_for_pid, validate_agent_id,
    validate_runtime_process_identity, write_global_runtime_file, write_runtime_file,
};
pub use sandbox::{SandboxFsProfile, SandboxNetworkPolicy};
pub use server_lifecycle::{
    ServerStartOptions, detached_server_args, ensure_server, ensure_server_with,
    ensure_server_with_options, restart_server, runtime_is_healthy,
    server_start_options_from_runtime, stop_server,
};

#[derive(Debug, Parser)]
#[command(name = "stateful")]
#[command(about = "Collision prevention for coding agents")]
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
        #[arg(long, default_value = "local")]
        workspace_id: String,
    },
    Status,
    Commit {
        #[arg(short = 'm', long)]
        message: String,
        #[arg(long = "task-id")]
        task_id: String,
        #[arg(long = "agent-id")]
        agent_id: String,
        #[arg(required = true)]
        paths: Vec<String>,
    },
    #[command(subcommand)]
    Lease(LeaseCommand),
    Doctor,
    Codex {
        #[arg(long, default_value = "codex")]
        codex_bin: String,
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
    Hook(HookRuntime),
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
        #[arg(long, hide = true)]
        token_stdin: bool,
        #[arg(long, default_value = "local")]
        workspace_id: String,
    },
    Restart,
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
        #[arg(long)]
        purpose: Option<String>,
        #[arg(long = "task-id")]
        task_id: Option<String>,
        #[arg(long = "agent-id")]
        agent_id: Option<String>,
        #[arg(long = "workspace-id")]
        workspace_id: Option<String>,
        /// One JSON-serialized stateful_core::MutationOperation.
        #[arg(long = "operation")]
        operations: Vec<String>,
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
pub enum ReposCommand {
    List,
}

#[derive(Debug, Subcommand)]
pub enum LeaseCommand {
    Release {
        batch_id: String,
        #[arg(long = "task-id")]
        task_id: String,
        #[arg(long = "agent-id")]
        agent_id: String,
    },
}

#[derive(Debug, Subcommand)]
pub enum HookCommand {
    SessionStart,
    UserPromptSubmit,
    PreToolUse,
    PostToolUse,
    Stop,
    #[command(hide = true)]
    Heartbeat,
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
            workspace_id: legacy_workspace_id,
        } => match command.unwrap_or(ServerCommand::Start {
            foreground: true,
            host: legacy_host,
            port: legacy_port,
            workspace_id: legacy_workspace_id,
            token_stdin: false,
        }) {
            ServerCommand::Start {
                foreground,
                host,
                port,
                workspace_id,
                token_stdin,
            } => {
                if foreground {
                    run_server(host, port, workspace_id, token_stdin)?;
                } else {
                    if token_stdin {
                        anyhow::bail!("--token-stdin is reserved for detached server startup");
                    }
                    let paths = GlobalPaths::from_env()?;
                    let runtime = ensure_server_with_options(
                        &paths,
                        ServerStartOptions {
                            host,
                            port,
                            token: None,
                            workspace_id,
                        },
                    )?;
                    print_server_runtime(&runtime)?;
                }
            }
            ServerCommand::Restart => {
                let paths = GlobalPaths::from_env()?;
                let runtime = restart_server(&paths)?;
                print_server_runtime(&runtime)?;
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
            let (_repo_root, runtime) = discover_runtime_for_current_dir()?;
            let payload: serde_json::Value = get_payload(&runtime, "/v2/status")?;
            println!("{}", serde_json::to_string_pretty(&payload)?);
        }
        Command::Commit {
            message,
            task_id,
            agent_id,
            paths,
        } => {
            let (repo_root, runtime) = discover_runtime_for_current_dir()?;
            let result = run_structured_commit(CommitRequest {
                identity: cli_identity(&repo_root, &runtime, &task_id, &agent_id, "commit")?,
                repo_root,
                message,
                paths,
            })?;
            println!("{}", serde_json::to_string_pretty(&result)?);
        }
        Command::Lease(LeaseCommand::Release {
            batch_id,
            task_id,
            agent_id,
        }) => {
            let (repo_root, runtime) = discover_runtime_for_current_dir()?;
            let identity =
                cli_identity(&repo_root, &runtime, &task_id, &agent_id, "lease.release")?;
            let result: stateful_store::LeaseReleaseResult = post_command(
                &runtime,
                "/v2/leases/release",
                &identity,
                &stateful_store::LeaseReleaseInput { batch_id },
            )?;
            println!("{}", serde_json::to_string_pretty(&result)?);
        }
        Command::Doctor => {
            let report = doctor_report(current_repo_root_or_current_dir()?);
            println!("{}", serde_json::to_string_pretty(&report)?);
        }
        Command::Codex { codex_bin, args } => {
            let code = run_codex(CodexWrapperOptions { codex_bin, args })?;
            std::process::exit(code);
        }
        Command::Sandbox(SandboxCommand::Run {
            fs,
            network,
            purpose,
            task_id,
            agent_id,
            workspace_id,
            operations,
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
            let output = sandbox::run_sandbox_in_repo(
                &repo_root,
                &paths,
                sandbox::SandboxRunRequest {
                    fs,
                    network,
                    purpose,
                    task_id,
                    agent_id,
                    workspace_id,
                    operations,
                    command,
                    timeout_seconds,
                    stream_events,
                },
            )?;
            let rendered = render_sandbox_run_cli_output(&output, json)?;
            std::io::stdout().write_all(&rendered.stdout)?;
            std::io::stderr().write_all(&rendered.stderr)?;
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

fn print_server_runtime(runtime: &ServerRuntime) -> anyhow::Result<()> {
    println!(
        "{}",
        serde_json::to_string_pretty(&serde_json::json!({
            "status": "ok",
            "base_url": runtime.base_url,
            "workspace_id": runtime.workspace_id,
            "pid": runtime.pid,

        }))?
    );
    Ok(())
}
fn cli_identity(
    repo_root: &Path,
    runtime: &ServerRuntime,
    task_id: &str,
    agent_id: &str,
    event: &str,
) -> anyhow::Result<CommandIdentity> {
    if task_id.trim().is_empty() {
        anyhow::bail!("task id must not be empty");
    }
    validate_agent_id(agent_id, "agent id")?;
    let paths = GlobalPaths::from_env()?;
    let repo = repo_identity_for_enabled_repo(&paths, repo_root)?;
    Ok(CommandIdentity::new_now(
        task_id,
        uuid::Uuid::new_v4().to_string(),
        stateful_core::AgentIdentity {
            agent_id: agent_id.to_string(),
            turn_id: None,
            actor_id: agent_id.to_string(),
            actor_type: stateful_core::ActorType::Agent,
            owner_id: None,
            parent_agent_id: None,
            parent_actor_id: None,
        },
        stateful_core::WorkspaceIdentity {
            root: repo.root,
            workspace_id: runtime.workspace_id.clone(),
            repo_id: repo.repo_id,
            worktree_id: repo.worktree_id,
            branch: repo.branch,
        },
        stateful_core::SourceRef {
            kind: stateful_core::SourceKind::Cli,
            event: event.to_string(),
            tool_name: None,
            source_ref: "stateful-cli".to_string(),
        },
    ))
}

fn run_server(
    host: String,
    port: u16,
    workspace_id: String,
    token_stdin: bool,
) -> anyhow::Result<()> {
    let token = if token_stdin {
        read_server_token_from_stdin()?
    } else {
        uuid::Uuid::new_v4().to_string()
    };
    let base_url = format!("http://{host}:{port}");
    let paths = GlobalPaths::from_env()?;
    let options = ServerStartOptions {
        host: host.clone(),
        port,
        token: Some(token.clone()),
        workspace_id: workspace_id.clone(),
    };
    let pid = std::process::id();
    let runtime = ServerRuntime::new(
        &base_url,
        &token,
        workspace_id,
        pid,
        process_start_identity_for_pid(pid)?,
    );
    let store = stateful_store::Store::open(global_state_db_path(&paths))?;

    let addr: SocketAddr = format!("{host}:{port}").parse()?;
    let tokio_runtime = tokio::runtime::Runtime::new()?;
    tokio_runtime.block_on(async move {
        let listener = tokio::net::TcpListener::bind(addr).await?;
        server_lifecycle::register_foreground_runtime(&paths, &runtime, &options)?;
        print_server_runtime(&runtime)?;
        stateful_server::serve_listener(
            listener,
            stateful_server::ServerConfig::with_store(token, store),
        )
        .await
    })
}

fn read_server_token_from_stdin() -> anyhow::Result<String> {
    let mut token = String::new();
    std::io::stdin().lock().read_to_string(&mut token)?;
    if token.is_empty() {
        anyhow::bail!("stateful server token stdin was empty");
    }
    Ok(token)
}

pub fn state_db_path(repo_root: impl AsRef<Path>) -> std::path::PathBuf {
    repo_root.as_ref().join(".stateful_core").join("state.db")
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
    r#"protocol_version: stateful.v2
heartbeat_interval_seconds: 1
inactivity_timeout_seconds: 5
lease_expiry_seconds: 60
offer_ttl_seconds: 120
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
}
