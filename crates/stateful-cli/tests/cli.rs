use clap::Parser;
use stateful_cli::{
    Cli, Command, HookCommand, HookRuntime, InstallAgent, LeaseCommand, ReposCommand,
    SandboxCommand, SandboxFsProfile, SandboxNetworkPolicy, SandboxProcessCommand, ServerCommand,
};

#[test]
fn parses_typed_sandbox_operation() {
    let cli = Cli::try_parse_from([
        "stateful",
        "sandbox",
        "run",
        "--operation",
        r#"{"kind":"update","path":"README.md"}"#,
        "--command",
        "printf ok",
    ])
    .expect("sandbox run should parse");

    match cli.command {
        Command::Sandbox(SandboxCommand::Run {
            fs,
            network,
            operations,
            command,
            ..
        }) => {
            assert_eq!(fs, SandboxFsProfile::ReadOnly);
            assert_eq!(network, SandboxNetworkPolicy::Disabled);
            assert_eq!(
                operations,
                vec![r#"{"kind":"update","path":"README.md"}"#.to_string()]
            );
            assert_eq!(command.as_deref(), Some("printf ok"));
        }
        other => panic!("expected sandbox run command, got {other:?}"),
    }
}

#[test]
fn parses_codex_passthrough_arguments() {
    let cli = Cli::try_parse_from(["stateful", "codex", "exec", "--json"])
        .expect("codex command should parse");

    match cli.command {
        Command::Codex { codex_bin, args } => {
            assert_eq!(codex_bin, "codex");
            assert_eq!(args, vec!["exec".to_string(), "--json".to_string()]);
        }
        other => panic!("expected codex command, got {other:?}"),
    }
}

#[test]
fn parses_retained_lifecycle_commands() {
    let start = Cli::try_parse_from(["stateful", "server", "start", "--foreground"])
        .expect("server start should parse");
    assert!(matches!(
        start.command,
        Command::Server {
            command: Some(ServerCommand::Start {
                foreground: true,
                ..
            }),
            ..
        }
    ));

    let restart = Cli::try_parse_from(["stateful", "server", "restart"])
        .expect("server restart should parse");
    assert!(matches!(
        restart.command,
        Command::Server {
            command: Some(ServerCommand::Restart),
            ..
        }
    ));
}

#[test]
fn server_start_rejects_public_tokens_and_accepts_internal_token_stdin() {
    assert!(Cli::try_parse_from(["stateful", "server", "--token", "secret"]).is_err());
    assert!(Cli::try_parse_from(["stateful", "server", "start", "--token", "secret"]).is_err());

    let internal = Cli::try_parse_from([
        "stateful",
        "server",
        "start",
        "--foreground",
        "--token-stdin",
    ])
    .expect("detached child command should parse");
    assert!(matches!(
        internal.command,
        Command::Server {
            command: Some(ServerCommand::Start {
                foreground: true,
                token_stdin: true,
                ..
            }),
            ..
        }
    ));
}

#[test]
fn parses_retained_adapter_commands() {
    let process = Cli::try_parse_from([
        "stateful",
        "sandbox",
        "process",
        "find",
        "--contains",
        "stateful",
        "--json",
    ])
    .expect("process query should parse");
    assert!(matches!(
        process.command,
        Command::Sandbox(SandboxCommand::Process {
            command: SandboxProcessCommand::Find { .. }
        })
    ));

    let hook = Cli::try_parse_from(["stateful", "hook", "omp", "pre-tool-use"])
        .expect("OMP hook should parse");
    assert!(matches!(
        hook.command,
        Command::Hook(HookRuntime::Omp {
            command: HookCommand::PreToolUse
        })
    ));
}

#[test]
fn parses_retained_install_and_registry_commands() {
    let install = Cli::try_parse_from(["stateful", "install", "--agent", "codex"])
        .expect("install should parse");
    assert!(matches!(
        install.command,
        Command::Install { agents, .. } if agents == vec![InstallAgent::Codex]
    ));

    let repos =
        Cli::try_parse_from(["stateful", "repos", "list"]).expect("repo listing should parse");
    assert!(matches!(repos.command, Command::Repos(ReposCommand::List)));
}

#[test]
fn parses_v2_status_commit_and_lease_release_commands() {
    assert!(matches!(
        Cli::try_parse_from(["stateful", "status"])
            .expect("status should parse")
            .command,
        Command::Status
    ));

    let commit = Cli::try_parse_from([
        "stateful",
        "commit",
        "--task-id",
        "task-1",
        "--agent-id",
        "agent-1",
        "-m",
        "update docs",
        "README.md",
    ])
    .expect("commit should parse");
    assert!(matches!(
        commit.command,
        Command::Commit {
            message,
            task_id,
            agent_id,
            paths,
        } if message == "update docs"
            && task_id == "task-1"
            && agent_id == "agent-1"
            && paths == vec!["README.md"]
    ));

    let release = Cli::try_parse_from([
        "stateful",
        "lease",
        "release",
        "batch-1",
        "--task-id",
        "task-1",
        "--agent-id",
        "agent-1",
    ])
    .expect("lease release should parse");
    assert!(matches!(
        release.command,
        Command::Lease(LeaseCommand::Release {
            batch_id,
            task_id,
            agent_id,
        }) if batch_id == "batch-1" && task_id == "task-1" && agent_id == "agent-1"
    ));
}

#[test]
fn rejects_removed_current_and_events_commands() {
    assert!(Cli::try_parse_from(["stateful", "current"]).is_err());
    assert!(Cli::try_parse_from(["stateful", "events"]).is_err());
}
