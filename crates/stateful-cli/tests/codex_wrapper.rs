use stateful_cli::{
    CodexSandboxMode, CodexWrapperOptions, STATEFUL_TRUSTED_SANDBOX_ENV, build_codex_invocation,
};

#[test]
fn codex_wrapper_builds_read_only_tmp_profile_and_attestation() {
    let invocation = build_codex_invocation(CodexWrapperOptions {
        codex_bin: "/opt/codex/bin/codex".to_string(),
        sandbox: CodexSandboxMode::ReadOnlyTmp,
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
    assert!(invocation.args.windows(2).any(|window| {
        window
            == [
                "-c",
                "permissions.stateful-read-only-tmp.filesystem.\"/tmp\"=\"write\"",
            ]
    }));
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
}

#[test]
fn codex_wrapper_rejects_user_sandbox_overrides() {
    let error = build_codex_invocation(CodexWrapperOptions {
        codex_bin: "codex".to_string(),
        sandbox: CodexSandboxMode::ReadOnlyTmp,
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
