use clap::{Parser, Subcommand};
use std::{fs, net::SocketAddr, path::Path};

mod commit;
mod hook;
mod mcp;
mod outbox;
mod runtime;
mod validation;

pub use commit::{CommitRequest, CommitResult, run_structured_commit};
pub use hook::{
    HookOutcome, handle_post_tool_use_in_repo, handle_pre_tool_use, handle_pre_tool_use_in_repo,
    handle_session_start_in_repo, handle_stop_in_repo, handle_user_prompt_submit_in_repo,
};
pub use mcp::{call_mcp_tool_in_repo, handle_mcp_jsonrpc_in_repo, serve_mcp_stdio_in_repo};
pub use outbox::sync_outbox_in_repo;
pub use runtime::{
    CurrentSession, HttpResponse, IntentDeclareArgs, ServerRuntime, declare_intent_via_http,
    discover_runtime, get_json, post_json, read_current_session_file, write_current_session_file,
    write_runtime_file,
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
    Server {
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
        #[arg(required = true, trailing_var_arg = true)]
        paths: Vec<String>,
    },
    #[command(subcommand)]
    Intent(IntentCommand),
    #[command(subcommand)]
    Mcp(McpCommand),
    SyncOutbox,
    #[command(subcommand)]
    Hook(HookCommand),
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
            install_repo_local(std::env::current_dir()?, binary)?;
            println!("installed stateful repo-local configuration");
        }
        Command::Server {
            host,
            port,
            token,
            workspace_id,
        } => run_server(host, port, token, workspace_id)?,
        Command::Status => {
            let report = doctor_report(std::env::current_dir()?);
            println!("{}", serde_json::to_string_pretty(&report)?);
        }
        Command::Current => {
            let runtime = discover_runtime(std::env::current_dir()?)?;
            let response = get_json(&runtime, "/v1/current")?;
            print_http_response(response)?;
        }
        Command::Events => {
            let runtime = discover_runtime(std::env::current_dir()?)?;
            let response = get_json(&runtime, "/v1/events")?;
            print_http_response(response)?;
        }
        Command::SyncOutbox => {
            let synced = sync_outbox_in_repo(std::env::current_dir()?)?;
            println!("{}", serde_json::json!({ "synced": synced }));
        }
        Command::Doctor => {
            let report = doctor_report(std::env::current_dir()?);
            println!("{}", serde_json::to_string_pretty(&report)?);
        }
        Command::Validate { profile } => {
            let result = run_validation_profile(std::env::current_dir()?, &profile)?;
            println!("{}", serde_json::to_string(&result)?);
            if !matches!(result.status, ValidationStatus::Passed) {
                std::process::exit(1);
            }
        }
        Command::Commit { message, paths } => {
            let repo_root = std::env::current_dir()?;
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
        Command::Intent(IntentCommand::Declare {
            session_id,
            workspace_id,
            files_planned,
        }) => {
            let repo_root = std::env::current_dir()?;
            let runtime = discover_runtime(&repo_root)?;
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
    let repo_root = std::env::current_dir()?;
    let runtime = ServerRuntime::new(&base_url, &token, workspace_id, std::process::id());
    write_runtime_file(&repo_root, &runtime)?;
    let store = stateful_store::Store::open(state_db_path(&repo_root))?;

    let addr: SocketAddr = format!("{host}:{port}").parse()?;
    let tokio_runtime = tokio::runtime::Runtime::new()?;
    tokio_runtime.block_on(stateful_server::serve_addr(
        addr,
        stateful_server::ServerConfig::with_store(token, store),
    ))
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
}

pub fn doctor_report(repo_root: impl AsRef<Path>) -> DoctorReport {
    let repo_root = repo_root.as_ref();
    let hooks_json = repo_root.join(".codex").join("hooks.json").is_file();
    let config_yml = repo_root.join(".stateful").join("config.yml").is_file();
    let validation_yml = repo_root.join(".stateful").join("validation.yml").is_file();
    let runtime_server_json = repo_root
        .join(".stateful_core")
        .join("runtime")
        .join("server.json")
        .is_file();
    let state_db = state_db_path(repo_root).is_file();

    DoctorReport {
        installed: hooks_json && config_yml && validation_yml,
        hooks_json,
        config_yml,
        validation_yml,
        runtime_server_json,
        state_db,
    }
}

pub fn install_repo_local(
    repo_root: impl AsRef<Path>,
    binary_path: impl AsRef<str>,
) -> anyhow::Result<()> {
    let repo_root = repo_root.as_ref();
    let binary_path = binary_path.as_ref();

    fs::create_dir_all(repo_root.join(".codex"))?;
    fs::create_dir_all(repo_root.join(".codex/skills/stateful-command-policy"))?;
    fs::create_dir_all(repo_root.join(".stateful"))?;

    fs::write(repo_root.join(".codex/hooks.json"), hooks_json(binary_path))?;
    fs::write(
        repo_root.join(".codex/config.toml"),
        mcp_config_toml(binary_path),
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

fn hooks_json(binary_path: &str) -> String {
    let command = if Path::new(binary_path).is_absolute() {
        binary_path.to_string()
    } else {
        format!("$(git rev-parse --show-toplevel)/{binary_path}")
    };
    let command_prefix = format!(
        "\"{}\" hook",
        command.replace('\\', "\\\\").replace('"', "\\\"")
    );
    let value = serde_json::json!({
        "hooks": {
            "SessionStart": [{
                "matcher": "startup|resume|clear|compact",
                "hooks": [{
                    "type": "command",
                    "command": format!("{command_prefix} session-start"),
                    "statusMessage": "Loading stateful current state"
                }]
            }],
            "UserPromptSubmit": [{
                "hooks": [{
                    "type": "command",
                    "command": format!("{command_prefix} user-prompt-submit"),
                    "statusMessage": "Checking stateful intent context"
                }]
            }],
            "PreToolUse": [{
                "matcher": "Bash|apply_patch|Edit|Write|mcp__filesystem__.*",
                "hooks": [{
                    "type": "command",
                    "command": format!("{command_prefix} pre-tool-use"),
                    "statusMessage": "Authorizing stateful tool use"
                }]
            }],
            "PostToolUse": [{
                "matcher": "Bash|apply_patch|Edit|Write|mcp__filesystem__.*",
                "hooks": [{
                    "type": "command",
                    "command": format!("{command_prefix} post-tool-use"),
                    "statusMessage": "Recording stateful activity"
                }]
            }],
            "Stop": [{
                "hooks": [{
                    "type": "command",
                    "command": format!("{command_prefix} stop"),
                    "statusMessage": "Finalizing stateful activity"
                }]
            }]
        }
    });

    format!(
        "{}\n",
        serde_json::to_string_pretty(&value).expect("static hook config should serialize")
    )
}

fn mcp_config_toml(binary_path: &str) -> String {
    let mcp_command = if Path::new(binary_path).is_absolute() {
        binary_path.to_string()
    } else if binary_path.contains('/') {
        format!("./{binary_path}")
    } else {
        binary_path.to_string()
    };
    let hook_binary = if Path::new(binary_path).is_absolute() {
        binary_path.to_string()
    } else {
        format!("$(git rev-parse --show-toplevel)/{binary_path}")
    };
    let hook_prefix = format!("\"{hook_binary}\" hook");

    format!(
        r#"[features]
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
matcher = "Bash|apply_patch|Edit|Write|mcp__filesystem__.*"

[[hooks.PreToolUse.hooks]]
type = "command"
command = "{} pre-tool-use"
statusMessage = "Authorizing stateful tool use"

[[hooks.PostToolUse]]
matcher = "Bash|apply_patch|Edit|Write|mcp__filesystem__.*"

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
    r#"---
name: stateful-command-policy
description: Use when running shell commands, editing files, or responding to stateful hook denials in a repo with stateful Codex hooks
---

# Stateful Command Policy

Stateful hooks are authoritative. Use this skill to choose policy-aligned commands before invoking tools.

## Before Writes

- Declare intent for planned files first: `stateful intent declare <paths...>`.
- Keep declared paths narrow; prefer exact files for edits, deletes, and renames.
- If a hook denies an action, read the denial and choose the documented alternative instead of retrying variants.

## Prefer

- Search and inspect: `rg`, `rg --files`, `sed -n`, `cat`, `ls`, read-only `find`, `wc`.
- Git inspection: `git status`, `git diff`, `git show`, `git log`, `git branch`, `git rev-parse`.
- Validation: `cargo test`, `npm test`, `pnpm test`, `yarn test`, `pytest`, `go test`.
- Stateful diagnostics: `stateful doctor`, `stateful status`, `stateful current`, `stateful events`, `stateful validate <profile>`.

## Avoid In Bash

- Shell write syntax: `>`, `>>`, heredocs, and `| tee`.
- Direct file mutation: `rm`, `mv`, `cp`, `mkdir`, `touch`, `chmod`, `chown`.
- Raw mutation git commands: `git checkout`, `git switch`, `git restore`, `git reset`, `git clean`, `git apply`, `git merge`, `git rebase`.
- Package installs, long-running processes, broad filesystem edits, and ad hoc scripts that mutate code.
- Most `stateful` control commands through Bash; use MCP tools when available. `stateful intent declare` is the Bash-safe coordination exception.

## If Blocked

- Do not retry the same command with small variations.
- Declare or narrow intent if the denial asks for scope.
- Use structured edit tools for file changes and validation profiles for controlled checks.
- If no policy-compliant path is available, report the exact command and denial reason.
"#
}
