use clap::Parser;
use stateful_cli::{
    Cli, CodexSandboxMode, Command, GlobalPaths, HookCommand, HookRuntime, InstallAgent,
    McpCommand, NotificationsCommand, ReposCommand, ResumeCommand, SandboxCommand,
    SandboxFsProfile, SandboxNetworkPolicy, ServerCommand, ToolsCommand, allow_tool_for_repo,
    doctor_report_with_global, enable_repo, record_unclassified_tool_for_repo,
};
use std::{fs, path::PathBuf, process::Command as ProcessCommand};

#[test]
fn parses_sandbox_run_read_only_defaults() {
    let cli = Cli::try_parse_from(["stateful", "sandbox", "run", "--command", "rg auth src"])
        .expect("sandbox run should parse");

    match cli.command {
        Command::Sandbox(SandboxCommand::Run {
            fs,
            network,
            purpose,
            write_targets,
            create_targets,
            write_dirs,
            connect_sockets,
            allow_signal,
            command,
            timeout_seconds,
        }) => {
            assert_eq!(fs, SandboxFsProfile::ReadOnly);
            assert_eq!(network, SandboxNetworkPolicy::Disabled);
            assert_eq!(purpose, None);
            assert!(write_targets.is_empty());
            assert!(create_targets.is_empty());
            assert!(write_dirs.is_empty());
            assert!(connect_sockets.is_empty());
            assert!(!allow_signal);
            assert_eq!(command, "rg auth src");
            assert_eq!(timeout_seconds, None);
        }
        other => panic!("expected sandbox run command, got {other:?}"),
    }
}

#[test]
fn doctor_labels_legacy_hooks_json_without_counting_it_as_installed() {
    let temp = std::env::temp_dir().join(format!(
        "stateful-doctor-legacy-hooks-{}",
        std::process::id()
    ));
    if temp.exists() {
        fs::remove_dir_all(&temp).expect("old temp root should remove");
    }
    let repo = temp.join("repo");
    let hooks_dir = repo.join(".codex");
    fs::create_dir_all(&hooks_dir).expect("hooks dir should create");
    fs::create_dir_all(repo.join(".git")).expect("fixture git dir should create");
    fs::create_dir_all(repo.join(".stateful")).expect("stateful dir should create");
    fs::write(hooks_dir.join("hooks.json"), "{}").expect("legacy hooks should write");
    fs::create_dir_all(repo.join(".stateful_core")).expect("legacy state dir should create");
    fs::write(repo.join(".stateful_core/state.db"), "legacy")
        .expect("legacy repo state db should write");
    fs::write(
        repo.join(".stateful/config.yml"),
        "protocol_version: stateful.v1\n",
    )
    .expect("repo config should write");

    let paths = GlobalPaths::new(temp.join("home"));
    let report = doctor_report_with_global(&repo, &paths);

    assert!(report.legacy_hooks_json);
    assert!(report.legacy_repo_state_db);
    assert!(!report.installed);

    fs::remove_dir_all(&temp).expect("temp root should remove");
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
        "tmp",
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
            purpose,
            write_targets,
            create_targets,
            write_dirs,
            connect_sockets,
            allow_signal,
            command,
            timeout_seconds,
        }) => {
            assert_eq!(fs, SandboxFsProfile::WriteTargets);
            assert_eq!(network, SandboxNetworkPolicy::Enabled);
            assert_eq!(purpose, None);
            assert_eq!(write_targets, vec!["README.md"]);
            assert_eq!(create_targets, vec!["docs/new.md"]);
            assert_eq!(write_dirs, vec!["tmp"]);
            assert!(connect_sockets.is_empty());
            assert!(!allow_signal);
            assert_eq!(command, "printf x > README.md");
            assert_eq!(timeout_seconds, Some(12));
        }
        other => panic!("expected sandbox run command, got {other:?}"),
    }
}

#[test]
fn parses_sandbox_run_git_profile() {
    let cli = Cli::try_parse_from([
        "stateful",
        "sandbox",
        "run",
        "--fs",
        "git",
        "--network",
        "enabled",
        "--timeout-seconds",
        "30",
        "--command",
        "git fetch --all",
    ])
    .expect("sandbox git profile should parse");

    match cli.command {
        Command::Sandbox(SandboxCommand::Run {
            fs,
            network,
            purpose,
            write_targets,
            create_targets,
            write_dirs,
            connect_sockets,
            allow_signal,
            command,
            timeout_seconds,
        }) => {
            assert_eq!(fs, SandboxFsProfile::Git);
            assert_eq!(network, SandboxNetworkPolicy::Enabled);
            assert_eq!(purpose, None);
            assert!(write_targets.is_empty());
            assert!(create_targets.is_empty());
            assert!(write_dirs.is_empty());
            assert!(connect_sockets.is_empty());
            assert!(!allow_signal);
            assert_eq!(command, "git fetch --all");
            assert_eq!(timeout_seconds, Some(30));
        }
        other => panic!("expected sandbox run command, got {other:?}"),
    }
}

#[test]
fn parses_sandbox_run_github_pr_profile() {
    let cli = Cli::try_parse_from([
        "stateful",
        "sandbox",
        "run",
        "--fs",
        "github-pr",
        "--network",
        "enabled",
        "--timeout-seconds",
        "30",
        "--command",
        "gh pr status",
    ])
    .expect("sandbox github-pr profile should parse");

    match cli.command {
        Command::Sandbox(SandboxCommand::Run {
            fs,
            network,
            purpose,
            write_targets,
            create_targets,
            write_dirs,
            connect_sockets,
            allow_signal,
            command,
            timeout_seconds,
        }) => {
            assert_eq!(fs, SandboxFsProfile::GithubPr);
            assert_eq!(network, SandboxNetworkPolicy::Enabled);
            assert_eq!(purpose, None);
            assert!(write_targets.is_empty());
            assert!(create_targets.is_empty());
            assert!(write_dirs.is_empty());
            assert!(connect_sockets.is_empty());
            assert!(!allow_signal);
            assert_eq!(command, "gh pr status");
            assert_eq!(timeout_seconds, Some(30));
        }
        other => panic!("expected sandbox run command, got {other:?}"),
    }
}

#[test]
fn parses_sandbox_run_build_profile() {
    let cli = Cli::try_parse_from([
        "stateful",
        "sandbox",
        "run",
        "--fs",
        "build",
        "--network",
        "enabled",
        "--timeout-seconds",
        "60",
        "--command",
        "npm test",
    ])
    .expect("sandbox build profile should parse");

    match cli.command {
        Command::Sandbox(SandboxCommand::Run {
            fs,
            network,
            purpose,
            write_targets,
            create_targets,
            write_dirs,
            connect_sockets,
            allow_signal,
            command,
            timeout_seconds,
        }) => {
            assert_eq!(fs, SandboxFsProfile::Build);
            assert_eq!(network, SandboxNetworkPolicy::Enabled);
            assert_eq!(purpose, None);
            assert!(write_targets.is_empty());
            assert!(create_targets.is_empty());
            assert!(write_dirs.is_empty());
            assert!(connect_sockets.is_empty());
            assert!(!allow_signal);
            assert_eq!(command, "npm test");
            assert_eq!(timeout_seconds, Some(60));
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
            docker_socket,
            command,
            timeout_seconds,
        }) => {
            assert_eq!(purpose, "run nested Codex chaos benchmark");
            assert_eq!(write_dir, "target");
            assert_eq!(codex_home_root, "target/nested-codex-homes/run-1");
            assert_eq!(docker_socket, None);
            assert_eq!(command, "cargo run -p stateful-bench -- run");
            assert_eq!(timeout_seconds, Some(120));
        }
        other => panic!("expected nested Codex benchmark sandbox command, got {other:?}"),
    }
}

#[test]
fn parses_nested_codex_benchmark_sandbox_command_with_docker_socket() {
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
        "--docker-socket",
        "/Users/arthur/.colima/default/docker.sock",
        "--command",
        "cargo run -p stateful-bench -- run",
    ]);

    assert!(
        cli.is_ok(),
        "nested Codex benchmark sandbox should accept an explicit Docker socket"
    );
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
fn parses_sandbox_run_external_profile() {
    let cli = Cli::try_parse_from([
        "stateful",
        "sandbox",
        "run",
        "--fs",
        "external",
        "--purpose",
        "install rebuilt binaries",
        "--write-target",
        "/Users/me/.cargo/bin/stateful",
        "--create-target",
        "/Users/me/.cargo/bin/stateful-bench",
        "--write-dir",
        "/Users/me/.cargo/bin",
        "--connect-socket",
        "/private/tmp/tmux-501/default",
        "--allow-signal",
        "--network",
        "enabled",
        "--timeout-seconds",
        "10",
        "--command",
        "install -m 755 target/release/stateful /Users/me/.cargo/bin/stateful",
    ])
    .expect("sandbox external run should parse");

    match cli.command {
        Command::Sandbox(SandboxCommand::Run {
            fs,
            network,
            purpose,
            write_targets,
            create_targets,
            write_dirs,
            connect_sockets,
            allow_signal,
            timeout_seconds,
            command,
        }) => {
            assert_eq!(fs, SandboxFsProfile::External);
            assert_eq!(purpose, Some("install rebuilt binaries".to_string()));
            assert_eq!(write_targets, vec!["/Users/me/.cargo/bin/stateful"]);
            assert_eq!(create_targets, vec!["/Users/me/.cargo/bin/stateful-bench"]);
            assert_eq!(write_dirs, vec!["/Users/me/.cargo/bin"]);
            assert_eq!(connect_sockets, vec!["/private/tmp/tmux-501/default"]);
            assert!(allow_signal);
            assert_eq!(network, SandboxNetworkPolicy::Enabled);
            assert_eq!(timeout_seconds, Some(10));
            assert_eq!(
                command,
                "install -m 755 target/release/stateful /Users/me/.cargo/bin/stateful"
            );
        }
        other => panic!("expected sandbox external run command, got {other:?}"),
    }
}

#[test]
fn rejects_external_run_command() {
    for args in [
        vec![
            "stateful",
            "external-run",
            "request",
            "--purpose",
            "install rebuilt binaries",
            "--write-dir",
            "/Users/me/.cargo/bin",
            "--command",
            "true",
        ],
        vec![
            "stateful",
            "external-run",
            "approve",
            "request-123",
            "--run",
        ],
        vec!["stateful", "external-run", "run", "request-123"],
    ] {
        let error = Cli::try_parse_from(args).expect_err("external-run command should be removed");

        assert!(
            error.to_string().contains("unrecognized subcommand"),
            "unexpected parse error: {error}"
        );
    }
}

#[test]
fn git_related_stateful_subcommands_are_removed() {
    for command in ["commit", "pull", "push"] {
        let error = Cli::try_parse_from(["stateful", command])
            .expect_err("git-related stateful subcommands should be removed");

        assert!(
            error.to_string().contains("unrecognized subcommand"),
            "unexpected parse error for {command}: {error}"
        );
    }
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
fn rejects_codex_wrapper_command_with_read_only_tmp_sandbox() {
    let error = Cli::try_parse_from([
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
    .expect_err("read-only-tmp sandbox mode should be removed");

    assert!(
        error.to_string().contains("read-only-tmp"),
        "error should name the rejected sandbox mode: {error}"
    );
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
    let cli = Cli::try_parse_from(["stateful", "install", "--yes"])
        .expect("install command should parse");

    assert!(matches!(
        cli.command,
        Command::Install {
            yes: true,
            ref agents,
            codex_config: None,
            binary: None,
        }
        if agents.is_empty()
    ));
}

#[test]
fn parses_install_agent_codex_command() {
    let cli = Cli::try_parse_from([
        "stateful",
        "install",
        "--agent",
        "codex",
        "--yes",
        "--codex-config",
        "/home/me/.codex/config.toml",
        "--binary",
        "/opt/stateful/bin/stateful",
    ])
    .expect("install --agent codex command should parse");

    assert!(matches!(
        cli.command,
        Command::Install {
            yes: true,
            ref agents,
            ref codex_config,
            ref binary,
        } if codex_config == &Some(PathBuf::from("/home/me/.codex/config.toml"))
            && binary.as_deref() == Some("/opt/stateful/bin/stateful")
            && agents == &vec![InstallAgent::Codex]
    ));
}

#[test]
fn parses_install_agent_omp_command() {
    let cli = Cli::try_parse_from(["stateful", "install", "--agent", "omp", "--yes"])
        .expect("install --agent omp command should parse");

    assert!(matches!(
        cli.command,
        Command::Install {
            yes: true,
            ref agents,
            ..
        } if agents == &vec![InstallAgent::Omp]
    ));
}

#[test]
fn rejects_install_codex_subcommand() {
    assert!(Cli::try_parse_from(["stateful", "install", "codex", "--yes"]).is_err());
}

#[test]
fn parses_repos_list_command() {
    let cli = Cli::try_parse_from(["stateful", "repos", "list"])
        .expect("repos list command should parse");

    assert!(matches!(cli.command, Command::Repos(ReposCommand::List)));
}

#[test]
fn parses_tools_allow_list_and_deny_commands() {
    let cli = Cli::try_parse_from([
        "stateful",
        "tools",
        "allow",
        "mcp__codex_apps__github__merge_pull_request",
        "--repo",
        "/workspace/repo",
    ])
    .expect("tools allow command should parse");
    assert!(matches!(
        cli.command,
        Command::Tools(ToolsCommand::Allow {
            ref tool_name,
            ref repo,
        }) if tool_name == "mcp__codex_apps__github__merge_pull_request"
            && repo.as_deref() == Some(std::path::Path::new("/workspace/repo"))
    ));

    let cli = Cli::try_parse_from(["stateful", "tools", "list"])
        .expect("tools list command should parse");
    assert!(matches!(
        cli.command,
        Command::Tools(ToolsCommand::List { repo: None })
    ));

    let cli = Cli::try_parse_from(["stateful", "tools", "deny", "spawn_agent"])
        .expect("tools deny command should parse");
    assert!(matches!(
        cli.command,
        Command::Tools(ToolsCommand::Deny {
            ref tool_name,
            repo: None,
        }) if tool_name == "spawn_agent"
    ));
}

#[test]
fn tools_list_prints_allowed_and_unclassified_tools() {
    let root = std::env::temp_dir().join(format!("stateful-tools-list-{}", std::process::id()));
    if root.exists() {
        fs::remove_dir_all(&root).expect("old fixture root should be removable");
    }
    let paths = GlobalPaths::new(root.join("home"));
    let repo = root.join("repo");
    fs::create_dir_all(repo.join(".git")).expect("git directory should be creatable");
    enable_repo(&paths, &repo).expect("repo should enable");
    allow_tool_for_repo(&paths, &repo, "KnownTool").expect("tool should be allowed");
    record_unclassified_tool_for_repo(&paths, &repo, "FutureWriteTool")
        .expect("unclassified tool should record");

    let output = ProcessCommand::new(env!("CARGO_BIN_EXE_stateful"))
        .args(["tools", "list", "--repo"])
        .arg(&repo)
        .env_clear()
        .env("STATEFUL_HOME", &paths.home)
        .output()
        .expect("tools list should run");

    assert!(
        output.status.success(),
        "tools list failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let json: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("tools list should print json");
    assert_eq!(
        json["allowed_tools"],
        serde_json::json!([
            "multi_agent_v1spawn_agent",
            "multi_agent_v1wait_agent",
            "multi_agent_v1close_agent",
            "multi_agent_v1resume_agent",
            "mcp__openaiDeveloperDocs__fetch_openai_doc",
            "mcp__openaiDeveloperDocs__search_openai_docs",
            "multi_agent_v1send_input",
            "KnownTool"
        ])
    );
    assert_eq!(
        json["unclassified_tools"],
        serde_json::json!(["FutureWriteTool"])
    );

    fs::remove_dir_all(&root).expect("fixture root should be removable");
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
fn hook_codex_pre_tool_use_command_parses() {
    let cli = Cli::try_parse_from(["stateful", "hook", "codex", "pre-tool-use"])
        .expect("hook codex pre-tool-use command should parse");

    assert!(matches!(
        cli.command,
        Command::Hook(HookRuntime::Codex {
            command: HookCommand::PreToolUse,
        })
    ));
}

#[test]
fn hook_omp_pre_tool_use_command_parses() {
    let cli = Cli::try_parse_from(["stateful", "hook", "omp", "pre-tool-use"])
        .expect("hook omp pre-tool-use command should parse");

    assert!(matches!(
        cli.command,
        Command::Hook(HookRuntime::Omp {
            command: HookCommand::PreToolUse,
        })
    ));
}

#[test]
fn hook_legacy_pre_tool_use_command_is_rejected() {
    assert!(Cli::try_parse_from(["stateful", "hook", "pre-tool-use"]).is_err());
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
                    allow_plain_http,
                    enable_repo,
                    binary,
                    codex_config,
                }),
            ..
        } => {
            assert_eq!(base_url, "http://192.168.0.23:43873");
            assert_eq!(token, "secret-token");
            assert_eq!(workspace_id, "shared");
            assert!(!allow_plain_http);
            assert!(!enable_repo);
            assert_eq!(binary, None);
            assert_eq!(codex_config, None);
        }
        other => panic!("expected server join command, got {other:?}"),
    }
}

#[test]
fn parses_server_join_allow_plain_http() {
    let cli = Cli::try_parse_from([
        "stateful",
        "server",
        "join",
        "http://192.168.0.23:43873",
        "--token",
        "secret-token",
        "--allow-plain-http",
    ])
    .expect("server join should parse allow-plain-http");

    match cli.command {
        Command::Server {
            command: Some(ServerCommand::Join {
                allow_plain_http, ..
            }),
            ..
        } => assert!(allow_plain_http),
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
                    allow_plain_http,
                    enable_repo,
                    binary,
                    codex_config,
                }),
            ..
        } => {
            assert_eq!(base_url, "http://192.168.0.23:43873");
            assert_eq!(token, "secret-token");
            assert_eq!(workspace_id, "w1");
            assert!(!allow_plain_http);
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
