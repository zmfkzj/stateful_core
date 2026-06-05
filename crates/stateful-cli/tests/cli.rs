use clap::Parser;
use stateful_cli::{
    Cli, CodexSandboxMode, Command, HookCommand, McpCommand, NotificationsCommand, ReposCommand,
    ResumeCommand, SandboxCommand, SandboxFsProfile, SandboxNetworkPolicy, ServerCommand,
};
use std::path::PathBuf;

#[test]
fn parses_sandbox_run_read_only_defaults() {
    let cli = Cli::try_parse_from(["stateful", "sandbox", "run", "--command", "rg auth src"])
        .expect("sandbox run should parse");

    match cli.command {
        Command::Sandbox(SandboxCommand::Run {
            fs,
            network,
            write_targets,
            create_targets,
            command,
            timeout_seconds,
        }) => {
            assert_eq!(fs, SandboxFsProfile::ReadOnly);
            assert_eq!(network, SandboxNetworkPolicy::Disabled);
            assert!(write_targets.is_empty());
            assert!(create_targets.is_empty());
            assert_eq!(command, "rg auth src");
            assert_eq!(timeout_seconds, None);
        }
        other => panic!("expected sandbox run command, got {other:?}"),
    }
}

#[test]
fn parses_sandbox_run_write_targets_network_enabled() {
    let cli = Cli::try_parse_from([
        "stateful",
        "sandbox",
        "run",
        "--fs",
        "write-targets",
        "--network",
        "enabled",
        "--write-target",
        "README.md",
        "--create-target",
        "docs/new.md",
        "--timeout-seconds",
        "12",
        "--command",
        "printf x > README.md",
    ])
    .expect("sandbox run should parse");

    match cli.command {
        Command::Sandbox(SandboxCommand::Run {
            fs,
            network,
            write_targets,
            create_targets,
            command,
            timeout_seconds,
        }) => {
            assert_eq!(fs, SandboxFsProfile::WriteTargets);
            assert_eq!(network, SandboxNetworkPolicy::Enabled);
            assert_eq!(write_targets, vec!["README.md"]);
            assert_eq!(create_targets, vec!["docs/new.md"]);
            assert_eq!(command, "printf x > README.md");
            assert_eq!(timeout_seconds, Some(12));
        }
        other => panic!("expected sandbox run command, got {other:?}"),
    }
}

#[test]
fn sandbox_run_rejects_missing_command() {
    let error = Cli::try_parse_from(["stateful", "sandbox", "run"])
        .expect_err("sandbox run requires --command");

    assert!(error.to_string().contains("--command"));
}

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
fn parses_structured_push_command_with_explicit_remote_and_branch() {
    let cli = Cli::try_parse_from(["stateful", "push", "origin", "main"])
        .expect("push command should parse");

    match cli.command {
        Command::Push { remote, branch } => {
            assert_eq!(remote.as_deref(), Some("origin"));
            assert_eq!(branch.as_deref(), Some("main"));
        }
        other => panic!("expected push command, got {other:?}"),
    }
}

#[test]
fn parses_structured_push_command_without_explicit_target() {
    let cli = Cli::try_parse_from(["stateful", "push"]).expect("push command should parse");

    match cli.command {
        Command::Push { remote, branch } => {
            assert_eq!(remote, None);
            assert_eq!(branch, None);
        }
        other => panic!("expected push command, got {other:?}"),
    }
}

#[test]
fn structured_push_rejects_partial_explicit_target() {
    let error =
        Cli::try_parse_from(["stateful", "push", "origin"]).expect_err("push target is a pair");

    assert!(error.to_string().contains("<BRANCH>"));
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
fn parses_codex_wrapper_command_with_explicit_read_only_tmp_sandbox() {
    let cli = Cli::try_parse_from([
        "stateful",
        "codex",
        "--codex-bin",
        "/opt/codex/bin/codex",
        "--sandbox",
        "read-only-tmp",
        "exec",
        "--json",
        "-",
    ])
    .expect("codex wrapper command should parse");

    assert!(matches!(
        cli.command,
        Command::Codex {
            ref codex_bin,
            sandbox: CodexSandboxMode::ReadOnlyTmp,
            no_stateful: false,
            ref args,
        } if codex_bin == "/opt/codex/bin/codex"
            && args == &vec!["exec".to_string(), "--json".to_string(), "-".to_string()]
    ));
}

#[test]
fn parses_codex_wrapper_command_with_passthrough_sandbox_by_default() {
    let cli = Cli::try_parse_from([
        "stateful",
        "codex",
        "--codex-bin",
        "/opt/codex/bin/codex",
        "exec",
        "--json",
        "-",
    ])
    .expect("codex wrapper command should parse");

    assert!(matches!(
        cli.command,
        Command::Codex {
            ref codex_bin,
            sandbox: CodexSandboxMode::Passthrough,
            no_stateful: false,
            ref args,
        } if codex_bin == "/opt/codex/bin/codex"
            && args == &vec!["exec".to_string(), "--json".to_string(), "-".to_string()]
    ));
}

#[test]
fn parses_codex_wrapper_no_stateful_command() {
    let cli = Cli::try_parse_from(["stateful", "codex", "--no-stateful", "exec", "-"])
        .expect("codex wrapper no-stateful command should parse");

    assert!(matches!(
        cli.command,
        Command::Codex {
            no_stateful: true,
            ref args,
            ..
        } if args == &vec!["exec".to_string(), "-".to_string()]
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
            }),
            ..
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
            ..
        } => assert!(foreground),
        other => panic!("expected server start command, got {other:?}"),
    }
}

#[test]
fn parses_server_start_subcommand_as_detached_by_default() {
    let cli =
        Cli::try_parse_from(["stateful", "server", "start"]).expect("server start should parse");

    match cli.command {
        Command::Server {
            command: Some(ServerCommand::Start { foreground, .. }),
            ..
        } => assert!(!foreground),
        other => panic!("expected server start command, got {other:?}"),
    }
}

#[test]
fn parses_legacy_server_runtime_options() {
    let cli = Cli::try_parse_from([
        "stateful",
        "server",
        "--host",
        "127.0.0.1",
        "--port",
        "43874",
        "--token",
        "secret-token",
        "--workspace-id",
        "w1",
    ])
    .expect("legacy server command should parse");

    assert!(matches!(
        cli.command,
        Command::Server {
            command: None,
            ref host,
            port,
            ref token,
            ref workspace_id,
        } if host == "127.0.0.1"
            && port == 43874
            && token.as_deref() == Some("secret-token")
            && workspace_id == "w1"
    ));
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
