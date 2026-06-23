use clap::Parser;
use stateful_cli::{Cli, Command, SandboxCommand, SandboxFsProfile, SandboxNetworkPolicy};

#[test]
fn external_run_subcommand_is_removed() {
    let error = Cli::try_parse_from([
        "stateful",
        "external-run",
        "request",
        "--purpose",
        "install rebuilt binaries",
        "--write-dir",
        "/Users/me/.cargo/bin",
        "--command",
        "true",
    ])
    .expect_err("external-run should no longer parse");

    assert!(
        error.to_string().contains("unrecognized subcommand"),
        "unexpected parse error: {error}"
    );
}

#[test]
fn sandbox_external_profile_parses_external_scopes() {
    let cli = Cli::try_parse_from([
        "stateful",
        "sandbox",
        "run",
        "--fs",
        "external",
        "--purpose",
        "install rebuilt binaries",
        "--create-target",
        "/Users/me/.cargo/bin/stateful",
        "--write-dir",
        "/Users/me/.cargo/bin",
        "--connect-socket",
        "/private/tmp/tmux-501/default",
        "--allow-signal",
        "--network",
        "enabled",
        "--command",
        "install -m 755 target/release/stateful /Users/me/.cargo/bin/stateful",
    ])
    .expect("sandbox external profile should parse");

    match cli.command {
        Command::Sandbox(SandboxCommand::Run {
            fs,
            network,
            purpose,
            create_targets,
            write_dirs,
            connect_sockets,
            allow_signal,
            ..
        }) => {
            assert_eq!(fs, SandboxFsProfile::External);
            assert_eq!(network, SandboxNetworkPolicy::Enabled);
            assert_eq!(purpose, Some("install rebuilt binaries".to_string()));
            assert_eq!(create_targets, vec!["/Users/me/.cargo/bin/stateful"]);
            assert_eq!(write_dirs, vec!["/Users/me/.cargo/bin"]);
            assert_eq!(connect_sockets, vec!["/private/tmp/tmux-501/default"]);
            assert!(allow_signal);
        }
        other => panic!("expected sandbox external run, got {other:?}"),
    }
}
