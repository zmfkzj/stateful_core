use stateful_cli::{CodexSandboxMode, CodexWrapperOptions, build_codex_invocation};

const LEGACY_TRUSTED_SANDBOX_ENV: &str = "STATEFUL_HOOK_TRUSTED_SANDBOX";

#[test]
fn codex_wrapper_defaults_to_passthrough_configuration() {
    let invocation = build_codex_invocation(CodexWrapperOptions {
        codex_bin: "/opt/codex/bin/codex".to_string(),
        sandbox: CodexSandboxMode::Passthrough,
        no_stateful: false,
        args: vec!["exec".to_string(), "--json".to_string(), "-".to_string()],
    })
    .expect("passthrough invocation should build");

    assert_eq!(invocation.program, "/opt/codex/bin/codex");
    assert_eq!(
        invocation.args,
        vec!["exec".to_string(), "--json".to_string(), "-".to_string()]
    );
    assert!(
        invocation
            .env
            .iter()
            .all(|(key, _)| key != LEGACY_TRUSTED_SANDBOX_ENV),
        "wrapper env must not authorize raw Bash with legacy trusted sandbox metadata"
    );
}

#[test]
fn codex_wrapper_no_stateful_disables_codex_hooks_in_passthrough_mode() {
    let invocation = build_codex_invocation(CodexWrapperOptions {
        codex_bin: "codex".to_string(),
        sandbox: CodexSandboxMode::Passthrough,
        no_stateful: true,
        args: vec!["exec".to_string(), "-".to_string()],
    })
    .expect("no-stateful invocation should build");

    assert_eq!(
        invocation.args,
        vec![
            "-c".to_string(),
            "features.hooks=false".to_string(),
            "exec".to_string(),
            "-".to_string()
        ]
    );
}

#[test]
fn codex_wrapper_passthrough_allows_user_sandbox_overrides() {
    let invocation = build_codex_invocation(CodexWrapperOptions {
        codex_bin: "codex".to_string(),
        sandbox: CodexSandboxMode::Passthrough,
        no_stateful: false,
        args: vec![
            "exec".to_string(),
            "--dangerously-bypass-approvals-and-sandbox".to_string(),
            "--sandbox".to_string(),
            "danger-full-access".to_string(),
            "-c".to_string(),
            "default_permissions=\":danger-full-access\"".to_string(),
            "-".to_string(),
        ],
    })
    .expect("passthrough mode should not reject user Codex sandbox configuration");

    assert_eq!(
        invocation.args,
        vec![
            "exec".to_string(),
            "--dangerously-bypass-approvals-and-sandbox".to_string(),
            "--sandbox".to_string(),
            "danger-full-access".to_string(),
            "-c".to_string(),
            "default_permissions=\":danger-full-access\"".to_string(),
            "-".to_string(),
        ]
    );
}
