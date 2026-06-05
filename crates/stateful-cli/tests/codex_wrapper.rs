use stateful_cli::{
    CodexSandboxMode, CodexWrapperOptions, STATEFUL_CODEX_RUN_ID_ENV, STATEFUL_TRUSTED_SANDBOX_ENV,
    build_codex_invocation,
};

#[test]
fn codex_wrapper_builds_read_only_tmp_profile_and_attestation() {
    let invocation = build_codex_invocation(CodexWrapperOptions {
        codex_bin: "/opt/codex/bin/codex".to_string(),
        sandbox: CodexSandboxMode::ReadOnlyTmp,
        no_stateful: false,
        args: vec!["exec".to_string(), "--json".to_string(), "-".to_string()],
    })
    .expect("read-only tmp invocation should build");

    assert_eq!(invocation.program, "/opt/codex/bin/codex");
    assert!(invocation.args.contains(&"-c".to_string()));
    assert!(
        invocation
            .args
            .contains(&"default_permissions=\"stateful-read-only-tmp\"".to_string())
    );
    assert!(
        invocation
            .args
            .contains(&"permissions.stateful-read-only-tmp.extends=\":read-only\"".to_string())
    );
    assert!(
        invocation
            .args
            .contains(&"permissions.stateful-read-only-tmp.network.enabled=false".to_string())
    );
    assert!(
        invocation
            .args
            .windows(2)
            .any(|window| window == ["-c", "web_search=\"live\""]),
        "stateful codex should leave Bash networking disabled while enabling Codex web search"
    );
    assert!(
        invocation.args.windows(2).any(|window| {
            window
                == [
                    "-c",
                    "mcp_servers.stateful.default_tools_approval_mode=\"approve\"",
                ]
        }),
        "stateful codex should approve stateful MCP tools by default"
    );
    assert!(invocation.args.windows(2).any(|window| {
        window
            == [
                "-c",
                "permissions.stateful-read-only-tmp.filesystem./tmp=\"write\"",
            ]
    }));
    assert!(
        !invocation
            .args
            .iter()
            .any(|arg| arg.contains("filesystem.\"")),
        "filesystem override keys must not quote path segments because Codex treats those quotes as part of the path"
    );
    assert!(!invocation.args.contains(&"--sandbox".to_string()));
    assert!(invocation.args.ends_with(&[
        "exec".to_string(),
        "--json".to_string(),
        "-".to_string()
    ]));

    let sandbox_json = invocation
        .env
        .iter()
        .find_map(|(key, value)| (key == STATEFUL_TRUSTED_SANDBOX_ENV).then_some(value))
        .expect("trusted sandbox env should be set");
    let sandbox: serde_json::Value =
        serde_json::from_str(sandbox_json).expect("sandbox env should be json");
    assert_eq!(sandbox["mode"], "read-only");
    assert_eq!(sandbox["network_access"], false);
    assert!(
        sandbox["writable_roots"]
            .as_array()
            .expect("writable roots should be an array")
            .iter()
            .any(|root| root == "/tmp")
    );
    let codex_run_id = invocation
        .env
        .iter()
        .find_map(|(key, value)| (key == STATEFUL_CODEX_RUN_ID_ENV).then_some(value))
        .expect("codex run id env should be set");
    uuid::Uuid::parse_str(codex_run_id).expect("codex run id should be a uuid");
}

#[test]
fn codex_wrapper_no_stateful_disables_codex_hooks() {
    let invocation = build_codex_invocation(CodexWrapperOptions {
        codex_bin: "codex".to_string(),
        sandbox: CodexSandboxMode::ReadOnlyTmp,
        no_stateful: true,
        args: vec!["exec".to_string(), "-".to_string()],
    })
    .expect("no-stateful invocation should build");

    assert!(
        invocation
            .args
            .windows(2)
            .any(|window| window == ["-c", "features.hooks=false"]),
        "stateful codex --no-stateful should disable Codex lifecycle hooks"
    );
}

#[test]
fn codex_wrapper_rejects_user_sandbox_overrides() {
    let error = build_codex_invocation(CodexWrapperOptions {
        codex_bin: "codex".to_string(),
        sandbox: CodexSandboxMode::ReadOnlyTmp,
        no_stateful: false,
        args: vec![
            "exec".to_string(),
            "-c".to_string(),
            "default_permissions=\":danger-full-access\"".to_string(),
            "-".to_string(),
        ],
    })
    .expect_err("user permission override should be rejected");

    assert!(error.to_string().contains("default_permissions"));
}
