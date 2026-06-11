use clap::Parser;
use stateful_cli::{
    Cli, CodexSandboxMode, Command, ExternalRunCommand, HookCommand, McpCommand,
    NotificationsCommand, ReposCommand, ResumeCommand, SandboxCommand, SandboxFsProfile,
    SandboxNetworkPolicy, ServerCommand,
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
            write_dirs,
            command,
            timeout_seconds,
        }) => {
            assert_eq!(fs, SandboxFsProfile::ReadOnly);
            assert_eq!(network, SandboxNetworkPolicy::Disabled);
            assert!(write_targets.is_empty());
            assert!(create_targets.is_empty());
            assert!(write_dirs.is_empty());
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
        "--write-dir",
        "target",
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
            write_dirs,
            command,
            timeout_seconds,
        }) => {
            assert_eq!(fs, SandboxFsProfile::WriteTargets);
            assert_eq!(network, SandboxNetworkPolicy::Enabled);
            assert_eq!(write_targets, vec!["README.md"]);
            assert_eq!(create_targets, vec!["docs/new.md"]);
            assert_eq!(write_dirs, vec!["target"]);
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
fn parses_nested_codex_benchmark_sandbox_command() {
    let cli = Cli::try_parse_from([
        "stateful",
        "sandbox",
        "run-nested-codex-benchmark",
        "--purpose",
        "run nested Codex chaos benchmark",
        "--write-dir",
        "target",
        "--codex-home-root",
        "target/nested-codex-homes/run-1",
        "--timeout-seconds",
        "120",
        "--command",
        "cargo run -p stateful-bench -- run",
    ])
    .expect("nested Codex benchmark sandbox command should parse");

    match cli.command {
        Command::Sandbox(SandboxCommand::RunNestedCodexBenchmark {
            purpose,
            write_dir,
            codex_home_root,
            command,
            timeout_seconds,
        }) => {
            assert_eq!(purpose, "run nested Codex chaos benchmark");
            assert_eq!(write_dir, "target");
            assert_eq!(codex_home_root, "target/nested-codex-homes/run-1");
            assert_eq!(command, "cargo run -p stateful-bench -- run");
            assert_eq!(timeout_seconds, Some(120));
        }
        other => panic!("expected nested Codex benchmark sandbox command, got {other:?}"),
    }
}

#[test]
fn nested_codex_benchmark_sandbox_requires_purpose_home_root_and_command() {
    for args in [
        vec![
            "stateful",
            "sandbox",
            "run-nested-codex-benchmark",
            "--write-dir",
            "target",
            "--codex-home-root",
            "target/nested-codex-homes/run-1",
            "--command",
            "cargo test",
        ],
        vec![
            "stateful",
            "sandbox",
            "run-nested-codex-benchmark",
            "--purpose",
            "run nested Codex chaos benchmark",
            "--write-dir",
            "target",
            "--command",
            "cargo test",
        ],
        vec![
            "stateful",
            "sandbox",
            "run-nested-codex-benchmark",
            "--purpose",
            "run nested Codex chaos benchmark",
            "--write-dir",
            "target",
            "--codex-home-root",
            "target/nested-codex-homes/run-1",
        ],
    ] {
        let error = Cli::try_parse_from(args)
            .expect_err("nested Codex benchmark command should require explicit scope");
        let message = error.to_string();
        assert!(
            message.contains("required") || message.contains("Usage:"),
            "unexpected parse error: {message}"
        );
    }
}

#[test]
fn parses_external_run_request_command() {
    let cli = Cli::try_parse_from([
        "stateful",
        "external-run",
        "request",
        "--purpose",
        "install rebuilt binaries",
        "--write-target",
        "/Users/me/.cargo/bin/stateful",
        "--create-target",
        "/Users/me/.cargo/bin/stateful-bench",
        "--write-dir",
        "/Users/me/.cargo/bin",
        "--network",
        "disabled",
        "--timeout-seconds",
        "10",
        "--command",
        "install -m 755 target/release/stateful /Users/me/.cargo/bin/stateful",
    ])
    .expect("external-run request should parse");

    match cli.command {
        Command::ExternalRun(ExternalRunCommand::Request {
            purpose,
            write_targets,
            create_targets,
            write_dirs,
            network,
            timeout_seconds,
            command,
        }) => {
            assert_eq!(purpose, "install rebuilt binaries");
            assert_eq!(write_targets, vec!["/Users/me/.cargo/bin/stateful"]);
            assert_eq!(create_targets, vec!["/Users/me/.cargo/bin/stateful-bench"]);
            assert_eq!(write_dirs, vec!["/Users/me/.cargo/bin"]);
            assert_eq!(network, SandboxNetworkPolicy::Disabled);
            assert_eq!(timeout_seconds, Some(10));
            assert_eq!(
                command,
                "install -m 755 target/release/stateful /Users/me/.cargo/bin/stateful"
            );
        }
        other => panic!("expected external-run request command, got {other:?}"),
    }
}

#[test]
fn parses_external_run_approve_and_run_command() {
    let cli = Cli::try_parse_from([
        "stateful",
        "external-run",
        "approve",
        "request-123",
        "--run",
    ])
    .expect("external-run approve should parse");

    assert!(matches!(
        cli.command,
        Command::ExternalRun(ExternalRunCommand::Approve {
            ref request_id,
            run: true
        }) if request_id == "request-123"
    ));
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
    let cli = Cli::try_parse_from(["stateful", "enable", "--repo", "/work/repo"])
        .expect("enable command should parse");

    assert!(matches!(
        cli.command,
        Command::Enable { ref repo } if repo == &Some(PathBuf::from("/work/repo"))
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
fn intent_declare_command_parses_file_scopes() {
    let cli = Cli::try_parse_from([
        "stateful",
        "intent",
        "declare",
        "--session-id",
        "s1",
        "--workspace-id",
        "w1",
        "--purpose",
        "Fix auth validation behavior.",
        "src/auth.ts",
        "src/session/",
    ])
    .expect("intent declare command should parse");

    assert!(matches!(
        cli.command,
        Command::Intent(stateful_cli::IntentCommand::Declare {
            ref session_id,
            ref workspace_id,
            ref purpose,
            ref files_planned,
        }) if session_id.as_deref() == Some("s1")
            && workspace_id.as_deref() == Some("w1")
            && purpose == "Fix auth validation behavior."
            && files_planned == &vec!["src/auth.ts".to_string(), "src/session/".to_string()]
    ));
}

#[test]
fn intent_declare_command_can_default_session_and_workspace() {
    let cli = Cli::try_parse_from([
        "stateful",
        "intent",
        "declare",
        "--purpose",
        "Fix auth validation behavior.",
        "src/auth.ts",
    ])
    .expect("intent declare command should parse without explicit session flags");

    assert!(matches!(
        cli.command,
        Command::Intent(stateful_cli::IntentCommand::Declare {
            session_id: None,
            workspace_id: None,
            ref purpose,
            ref files_planned,
        }) if purpose == "Fix auth validation behavior."
            && files_planned == &vec!["src/auth.ts".to_string()]
    ));
}

#[test]
fn intent_declare_command_requires_at_least_one_file() {
    let error = Cli::try_parse_from([
        "stateful",
        "intent",
        "declare",
        "--purpose",
        "Fix auth validation behavior.",
    ])
    .expect_err("intent declare without files should fail");

    assert!(
        error.to_string().contains("files_planned") || error.to_string().contains("FILES_PLANNED"),
        "unexpected error: {error}"
    );
}

#[test]
fn intent_claim_command_parses_wait_id() {
    let cli = Cli::try_parse_from([
        "stateful",
        "intent",
        "claim",
        "--session-id",
        "s1",
        "--workspace-id",
        "w1",
        "--wait-id",
        "wait-1",
    ])
    .expect("intent claim command should parse");

    assert!(matches!(
        cli.command,
        Command::Intent(stateful_cli::IntentCommand::Claim {
            ref session_id,
            ref workspace_id,
            ref wait_id,
        }) if session_id.as_deref() == Some("s1")
            && workspace_id.as_deref() == Some("w1")
            && wait_id == "wait-1"
    ));
}

#[test]
fn intent_request_command_parses_request_id_action_and_path() {
    let cli = Cli::try_parse_from([
        "stateful",
        "intent",
        "request",
        "--session-id",
        "s1",
        "--workspace-id",
        "w1",
        "--request-id",
        "request-1",
        "--action",
        "write_file",
        "--path",
        "src/auth.ts",
        "--purpose",
        "Queue auth file changes.",
    ])
    .expect("intent request command should parse");

    assert!(matches!(
        cli.command,
        Command::Intent(stateful_cli::IntentCommand::Request {
            ref session_id,
            ref workspace_id,
            ref request_id,
            ref action,
            ref path,
            ref purpose,
        }) if session_id.as_deref() == Some("s1")
            && workspace_id.as_deref() == Some("w1")
            && request_id == "request-1"
            && action == "write_file"
            && path == "src/auth.ts"
            && purpose == "Queue auth file changes."
    ));
}

#[test]
fn intent_cancel_command_parses_request_id() {
    let cli = Cli::try_parse_from([
        "stateful",
        "intent",
        "cancel",
        "--session-id",
        "s1",
        "--workspace-id",
        "w1",
        "--request-id",
        "request-1",
    ])
    .expect("intent cancel command should parse");

    assert!(matches!(
        cli.command,
        Command::Intent(stateful_cli::IntentCommand::Cancel {
            ref session_id,
            ref workspace_id,
            ref request_id,
        }) if session_id.as_deref() == Some("s1")
            && workspace_id.as_deref() == Some("w1")
            && request_id == "request-1"
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
fn rejects_removed_lan_subcommand() {
    assert!(Cli::try_parse_from(["stateful", "lan", "serve"]).is_err());
    assert!(
        Cli::try_parse_from([
            "stateful",
            "lan",
            "join",
            "http://192.168.0.23:43873",
            "--token",
            "secret-token",
        ])
        .is_err()
    );
}

#[test]
fn parses_server_join_without_repo_enablement() {
    let cli = Cli::try_parse_from([
        "stateful",
        "server",
        "join",
        "http://192.168.0.23:43873",
        "--token",
        "secret-token",
    ])
    .expect("server join should parse");

    match cli.command {
        Command::Server {
            command:
                Some(ServerCommand::Join {
                    base_url,
                    token,
                    workspace_id,
                    enable_repo,
                    binary,
                    codex_config,
                }),
            ..
        } => {
            assert_eq!(base_url, "http://192.168.0.23:43873");
            assert_eq!(token, "secret-token");
            assert_eq!(workspace_id, "shared");
            assert!(!enable_repo);
            assert_eq!(binary, None);
            assert_eq!(codex_config, None);
        }
        other => panic!("expected server join command, got {other:?}"),
    }
}

#[test]
fn parses_server_join_with_repo_enablement_and_install_overrides() {
    let cli = Cli::try_parse_from([
        "stateful",
        "server",
        "join",
        "http://192.168.0.23:43873",
        "--token",
        "secret-token",
        "--workspace-id",
        "w1",
        "--enable-repo",
        "--binary",
        "/opt/stateful/bin/stateful",
        "--codex-config",
        "/Users/me/.codex/config.toml",
    ])
    .expect("server join should parse");

    match cli.command {
        Command::Server {
            command:
                Some(ServerCommand::Join {
                    base_url,
                    token,
                    workspace_id,
                    enable_repo,
                    binary,
                    codex_config,
                }),
            ..
        } => {
            assert_eq!(base_url, "http://192.168.0.23:43873");
            assert_eq!(token, "secret-token");
            assert_eq!(workspace_id, "w1");
            assert!(enable_repo);
            assert_eq!(binary.as_deref(), Some("/opt/stateful/bin/stateful"));
            assert_eq!(
                codex_config,
                Some(std::path::PathBuf::from("/Users/me/.codex/config.toml"))
            );
        }
        other => panic!("expected server join command, got {other:?}"),
    }
}

#[test]
fn parses_server_restart_subcommand() {
    let cli = Cli::try_parse_from(["stateful", "server", "restart"])
        .expect("server restart should parse");

    match cli.command {
        Command::Server {
            command: Some(ServerCommand::Restart),
            ..
        } => {}
        other => panic!("expected server restart command, got {other:?}"),
    }
}

#[test]
fn mcp_call_command_parses_tool_and_arguments() {
    let cli = Cli::try_parse_from([
        "stateful",
        "mcp",
        "call",
        "state.session.heartbeat",
        r#"{"session_id":"s1","workspace_id":"w1"}"#,
    ])
    .expect("mcp call command should parse");

    assert!(matches!(
        cli.command,
        Command::Mcp(McpCommand::Call {
            ref tool_name,
            ref arguments_json
        }) if tool_name == "state.session.heartbeat"
            && arguments_json.as_deref() == Some(r#"{"session_id":"s1","workspace_id":"w1"}"#)
    ));
}
