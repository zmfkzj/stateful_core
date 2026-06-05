use stateful_core::{BashKind, classify_bash};

#[test]
fn all_bash_commands_require_structured_sandbox_metadata() {
    for command in [
        "rg auth src",
        "git diff",
        "cargo test",
        "stateful validate cargo-test",
        "stateful intent declare src/auth.ts",
        "stateful commit -m 'docs: add plan' -- docs/plan.md",
        "stateful push origin main",
        "target/debug/stateful-bench run --pairs .stateful_bench/pairs/all.jsonl --mode no-state",
        "echo hi > src/auth.ts",
        "rm src/auth.ts",
        "unknown-command --flag",
    ] {
        let classification = classify_bash(command);

        assert_eq!(
            classification.kind,
            BashKind::Mutating,
            "command `{command}` should not be authorized from command text alone"
        );
        assert!(
            classification
                .reason
                .contains("structured read-only sandbox metadata"),
            "reason `{}` should direct callers to structured sandbox metadata",
            classification.reason
        );
    }
}

#[test]
fn bash_classifier_reason_is_stable_for_policy_denials() {
    let classification = classify_bash("rg auth src");

    assert_eq!(
        classification.reason,
        "Bash commands require structured read-only sandbox metadata with network disabled"
    );
}
