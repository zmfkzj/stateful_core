use stateful_cli::{CodexWrapperOptions, build_codex_invocation};

#[test]
fn wrapper_forwards_only_the_requested_codex_arguments() {
    let invocation = build_codex_invocation(CodexWrapperOptions {
        codex_bin: "/opt/codex/bin/codex".to_string(),
        args: vec!["exec".to_string(), "--json".to_string()],
    })
    .expect("invocation should build");

    assert_eq!(invocation.program, "/opt/codex/bin/codex");
    assert_eq!(invocation.args, ["exec", "--json"]);
    assert!(invocation.env.is_empty());
    assert!(
        !invocation
            .args
            .iter()
            .any(|arg| arg.contains("hooks=false"))
    );
}
