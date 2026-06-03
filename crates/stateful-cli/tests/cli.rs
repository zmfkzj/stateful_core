use clap::Parser;
use stateful_cli::{
    Cli, Command, HookCommand, McpCommand, NotificationsCommand, ReposCommand, ResumeCommand,
    ServerCommand,
};
use std::path::PathBuf;

#[test]
fn parses_structured_commit_command() {
    let cli = Cli::try_parse_from([
        "stateful",
        "commit",
        "-m",
        "docs: add plan",
        "--",
        "docs/plan.md",
    ])
    .expect("commit command should parse");

    match cli.command {
        Command::Commit { message, paths } => {
            assert_eq!(message, "docs: add plan");
            assert_eq!(paths, vec!["docs/plan.md"]);
        }
        other => panic!("expected commit command, got {other:?}"),
    }
}

#[test]
fn structured_commit_command_requires_path_separator() {
    let error = Cli::try_parse_from(["stateful", "commit", "-m", "docs: add plan", "docs/plan.md"])
        .expect_err("commit paths should require -- separator");

    assert!(error.to_string().contains("--"));
}

#[test]
fn parses_enable_command() {
    let cli = Cli::try_parse_from([
        "stateful",
        "enable",
        "--repo",
        "/work/repo",
        "--repo-local-codex",
    ])
    .expect("enable command should parse");

    assert!(matches!(
        cli.command,
        Command::Enable {
            ref repo,
            repo_local_codex: true
        } if repo == &Some(PathBuf::from("/work/repo"))
    ));
}

#[test]
fn parses_disable_command() {
    let cli = Cli::try_parse_from(["stateful", "disable", "--repo", "/work/repo"])
        .expect("disable command should parse");

    assert!(matches!(
        cli.command,
        Command::Disable { ref repo } if repo == &Some(PathBuf::from("/work/repo"))
    ));
}

#[test]
fn parses_install_yes_command() {
    let cli = Cli::try_parse_from([
        "stateful",
        "install",
        "--yes",
        "--codex-config",
        "/home/me/.codex/config.toml",
        "--binary",
        "/opt/stateful/bin/stateful",
    ])
    .expect("install command should parse");

    assert!(matches!(
        cli.command,
        Command::Install {
            yes: true,
            ref codex_config,
            ref binary
        } if codex_config == &Some(PathBuf::from("/home/me/.codex/config.toml"))
            && binary.as_deref() == Some("/opt/stateful/bin/stateful")
    ));
}

#[test]
fn parses_repos_list_command() {
    let cli = Cli::try_parse_from(["stateful", "repos", "list"])
        .expect("repos list command should parse");

    assert!(matches!(cli.command, Command::Repos(ReposCommand::List)));
}

#[test]
fn parses_notifications_poll_command() {
    let cli = Cli::try_parse_from([
        "stateful",
        "notifications",
        "poll",
        "--session-id",
        "s1",
        "--workspace-id",
        "w1",
    ])
    .expect("notifications poll command should parse");

    assert!(matches!(
        cli.command,
        Command::Notifications(NotificationsCommand::Poll {
            ref session_id,
            ref workspace_id,
        }) if session_id.as_deref() == Some("s1")
            && workspace_id.as_deref() == Some("w1")
    ));
}

#[test]
fn parses_resume_next_command() {
    let cli = Cli::try_parse_from([
        "stateful",
        "resume",
        "next",
        "--session-id",
        "s1",
        "--workspace-id",
        "w1",
    ])
    .expect("resume next command should parse");

    assert!(matches!(
        cli.command,
        Command::Resume(ResumeCommand::Next {
            ref session_id,
            ref workspace_id,
        }) if session_id.as_deref() == Some("s1")
            && workspace_id.as_deref() == Some("w1")
    ));
}

#[test]
fn hook_pre_tool_use_command_parses() {
    let cli = Cli::try_parse_from(["stateful", "hook", "pre-tool-use"])
        .expect("hook pre-tool-use command should parse");

    assert!(matches!(
        cli.command,
        Command::Hook(HookCommand::PreToolUse)
    ));
}

#[test]
fn validate_profile_command_parses() {
    let cli = Cli::try_parse_from(["stateful", "validate", "unit"])
        .expect("validate command should parse");

    assert!(matches!(cli.command, Command::Validate { profile } if profile == "unit"));
}

#[test]
fn intent_declare_command_parses_file_scopes() {
    let cli = Cli::try_parse_from([
        "stateful",
        "intent",
        "declare",
        "--session-id",
        "s1",
        "--workspace-id",
        "w1",
        "src/auth.ts",
        "src/session/",
    ])
    .expect("intent declare command should parse");

    assert!(matches!(
        cli.command,
        Command::Intent(stateful_cli::IntentCommand::Declare {
            ref session_id,
            ref workspace_id,
            ref files_planned,
        }) if session_id.as_deref() == Some("s1")
            && workspace_id.as_deref() == Some("w1")
            && files_planned == &vec!["src/auth.ts".to_string(), "src/session/".to_string()]
    ));
}

#[test]
fn intent_declare_command_can_default_session_and_workspace() {
    let cli = Cli::try_parse_from(["stateful", "intent", "declare", "src/auth.ts"])
        .expect("intent declare command should parse without explicit session flags");

    assert!(matches!(
        cli.command,
        Command::Intent(stateful_cli::IntentCommand::Declare {
            session_id: None,
            workspace_id: None,
            ref files_planned,
        }) if files_planned == &vec!["src/auth.ts".to_string()]
    ));
}

#[test]
fn server_command_parses_runtime_options() {
    let cli = Cli::try_parse_from([
        "stateful",
        "server",
        "start",
        "--host",
        "127.0.0.1",
        "--port",
        "43873",
        "--token",
        "secret-token",
        "--workspace-id",
        "w1",
    ])
    .expect("server command should parse");

    assert!(matches!(
        cli.command,
        Command::Server {
            command: Some(ServerCommand::Start {
                ref host,
                port,
                ref token,
                ref workspace_id,
                ..
            })
        } if host == "127.0.0.1"
            && port == 43873
            && token.as_deref() == Some("secret-token")
            && workspace_id == "w1"
    ));
}

#[test]
fn parses_server_start_subcommand() {
    let cli = Cli::try_parse_from(["stateful", "server", "start", "--foreground"])
        .expect("server start should parse");

    match cli.command {
        Command::Server {
            command: Some(ServerCommand::Start { foreground, .. }),
        } => assert!(foreground),
        other => panic!("expected server start command, got {other:?}"),
    }
}

#[test]
fn mcp_call_command_parses_tool_and_arguments() {
    let cli = Cli::try_parse_from([
        "stateful",
        "mcp",
        "call",
        "state.current.read",
        r#"{"mode":"brief"}"#,
    ])
    .expect("mcp call command should parse");

    assert!(matches!(
        cli.command,
        Command::Mcp(McpCommand::Call {
            ref tool_name,
            ref arguments_json
        }) if tool_name == "state.current.read"
            && arguments_json.as_deref() == Some(r#"{"mode":"brief"}"#)
    ));
}
