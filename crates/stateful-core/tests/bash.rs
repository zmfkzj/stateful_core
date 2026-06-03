use stateful_core::{BashKind, classify_bash};

#[test]
fn read_only_rg_is_allowed() {
    assert_eq!(classify_bash("rg auth src").kind, BashKind::ReadOnly);
}

#[test]
fn find_is_read_only_only_without_mutating_actions() {
    assert_eq!(
        classify_bash("find docs -name '*.md'").kind,
        BashKind::ReadOnly
    );

    let rejected = [
        "find docs -delete",
        "find docs \\-delete",
        "find docs -'delete'",
        "find docs -$'delete'",
        "find docs -{delete,print}",
        "find docs \"x' y\" -{delete,print}",
        "find docs -exec rm {} +",
        "find docs -execdir rm {} +",
        "find docs -ok rm {} +",
        "find docs -okdir rm {} +",
        "find docs -fprint out.txt",
        "find docs -fprint0 out.txt",
        "find docs -fprintf out.txt '%p\\n'",
        "find docs -fls out.txt",
    ];

    for command in rejected {
        assert_eq!(
            classify_bash(command).kind,
            BashKind::Mutating,
            "command `{command}` should not be allowed as read-only"
        );
    }
}

#[test]
fn git_branch_and_diff_read_only_allowlists_reject_mutating_options() {
    let allowed = [
        "git branch",
        "git branch --show-current",
        "git branch --list",
        "git branch --list 'feature/*'",
        "git branch --all",
        "git branch -a",
        "git branch --remotes",
        "git branch -r",
        "git diff",
        "git diff -- docs/a.md",
    ];

    for command in allowed {
        assert_eq!(
            classify_bash(command).kind,
            BashKind::ReadOnly,
            "command `{command}` should remain read-only"
        );
    }

    let rejected = [
        "git branch -D name",
        "git branch \\-D name",
        "git branch -'D' name",
        "git branch -d name",
        "git branch -m old new",
        "git branch -c old new",
        "git branch -C old new",
        "git branch -f topic HEAD",
        "git branch --delete name",
        "git branch --de'lete' name",
        "git branch --move old new",
        "git branch --copy old new",
        "git branch --track topic origin/main",
        "git branch --set-upstream-to origin/main",
        "git branch --edit-description topic",
        "git branch new-name HEAD",
        "git diff --output file",
        "git diff --output=file",
        "git diff \\--output=file",
        "git diff --out'put'=file",
        "git diff --out$'put'=file",
        "git diff --out$SUFFIX=file",
        "git diff --out{put,put}=file",
        "git diff \"x' y\" --out{put,put}=file",
    ];

    for command in rejected {
        assert_eq!(
            classify_bash(command).kind,
            BashKind::Mutating,
            "command `{command}` should not be allowed as read-only"
        );
    }
}

#[test]
fn redirect_write_is_denied() {
    assert_eq!(
        classify_bash("echo hi > src/auth.ts").kind,
        BashKind::Mutating
    );
}

#[test]
fn shell_control_syntax_is_denied_before_read_only_allowlists() {
    let rejected = [
        "stateful status\nrm docs/a.md",
        "stateful status & rm docs/a.md",
        "stateful status>docs/a.md",
        "rg auth\nrm src/a.rs",
    ];

    for command in rejected {
        assert_eq!(
            classify_bash(command).kind,
            BashKind::Mutating,
            "command `{command}` should be denied before any read-only allowlist"
        );
    }
}

#[test]
fn shell_expansion_syntax_is_denied_before_read_only_allowlists() {
    let rejected = [
        "find docs $(printf name)",
        "find docs ${PREDICATE}",
        "find docs $((1))",
        "find docs $\"PREDICATE\"",
        "find docs -$PREDICATE",
        "rg $PATTERN docs",
        "rg $1 docs",
        "rg $? docs",
        "rg *.rs src",
        "rg \"x' y\" *.rs src",
    ];

    for command in rejected {
        assert_eq!(
            classify_bash(command).kind,
            BashKind::Mutating,
            "command `{command}` should be denied before any read-only allowlist"
        );
    }
}

#[test]
fn raw_test_commands_are_allowed_as_non_code_edits() {
    assert_eq!(classify_bash("cargo test").kind, BashKind::ReadOnly);
    assert_eq!(
        classify_bash("python tests/runtests.py schema.tests.SchemaTests").kind,
        BashKind::ReadOnly
    );
    assert_eq!(
        classify_bash("npm test -- --runInBand").kind,
        BashKind::ReadOnly
    );
}

#[test]
fn codex_auth_status_is_allowed_as_preflight_but_exec_is_not() {
    assert_eq!(classify_bash("codex --help").kind, BashKind::ReadOnly);
    assert_eq!(classify_bash("codex doctor").kind, BashKind::ReadOnly);
    assert_eq!(
        classify_bash("codex doctor --help").kind,
        BashKind::ReadOnly
    );
    assert_eq!(classify_bash("codex exec --help").kind, BashKind::ReadOnly);
    assert_eq!(
        classify_bash("codex doctor --json").kind,
        BashKind::ReadOnly
    );
    assert_eq!(classify_bash("codex login --help").kind, BashKind::ReadOnly);
    assert_eq!(classify_bash("codex login status").kind, BashKind::ReadOnly);
    assert_eq!(classify_bash("codex auth status").kind, BashKind::ReadOnly);
    assert_eq!(
        classify_bash("codex exec --cd . 'edit src/lib.rs'").kind,
        BashKind::Mutating
    );
}

#[test]
fn environment_diagnostics_are_read_only() {
    assert_eq!(classify_bash("which docker").kind, BashKind::ReadOnly);
    assert_eq!(classify_bash("docker info").kind, BashKind::ReadOnly);
    assert_eq!(classify_bash("docker version").kind, BashKind::ReadOnly);
    assert_eq!(classify_bash("colima status").kind, BashKind::ReadOnly);
    assert_eq!(classify_bash("colima start").kind, BashKind::ReadOnly);
}

#[test]
fn git_diff_is_read_only_but_git_checkout_is_mutating() {
    assert_eq!(classify_bash("git diff -- src").kind, BashKind::ReadOnly);
    assert_eq!(
        classify_bash("git checkout -- src/auth.ts").kind,
        BashKind::Mutating
    );
}

#[test]
fn stateful_diagnostic_commands_are_read_only_escape_hatches() {
    assert_eq!(classify_bash("stateful doctor").kind, BashKind::ReadOnly);
    assert_eq!(
        classify_bash("./target/debug/stateful current").kind,
        BashKind::ReadOnly
    );
    assert_eq!(
        classify_bash("/repo/target/debug/stateful events").kind,
        BashKind::ReadOnly
    );
    assert_eq!(
        classify_bash("\"./target/debug/stateful\" status").kind,
        BashKind::ReadOnly
    );
}

#[test]
fn stateful_validate_is_the_bash_controlled_validation_escape_hatch() {
    assert_eq!(
        classify_bash("./target/debug/stateful validate cargo-test").kind,
        BashKind::ReadOnly
    );
}

#[test]
fn stateful_bench_operational_commands_are_allowed_outside_code_paths() {
    let classification = classify_bash(
        "target/debug/stateful-bench run --pairs .stateful_bench/pairs/all.jsonl --mode no-state --agent-cmd-template codex",
    );

    assert_eq!(classification.kind, BashKind::ReadOnly);

    let classification = classify_bash(
        "target/debug/stateful-bench generate-fallback-preflight --dataset .stateful_bench/datasets/swe-bench-verified.jsonl --output .stateful_bench/pairs/same-version-preflight.jsonl --assume-clean-apply",
    );

    assert_eq!(classification.kind, BashKind::ReadOnly);

    let classification = classify_bash(
        "target/debug/stateful-bench compare --stateful-run-dir .stateful_bench/runs/stateful --no-state-run-dir .stateful_bench/runs/no-state --manifest .stateful_bench/pairs/dev-30.jsonl --format markdown",
    );

    assert_eq!(classification.kind, BashKind::ReadOnly);
}

#[test]
fn stateful_bench_commands_targeting_code_paths_are_denied() {
    let classification = classify_bash(
        "target/debug/stateful-bench run --pairs .stateful_bench/pairs/all.jsonl --mode no-state --output-dir crates/generated --agent-cmd-template codex",
    );

    assert_eq!(classification.kind, BashKind::Mutating);

    let classification = classify_bash(
        "target/debug/stateful-bench compare --stateful-run-dir crates/generated --no-state-run-dir .stateful_bench/runs/no-state --manifest .stateful_bench/pairs/dev-30.jsonl",
    );

    assert_eq!(classification.kind, BashKind::Mutating);
}

#[test]
fn codex_bash_is_denied_as_possible_code_mutation() {
    let classification = classify_bash("codex exec --cd . 'edit src/lib.rs'");

    assert_eq!(classification.kind, BashKind::Mutating);
}

#[test]
fn stateful_intent_declare_is_bash_allowed_as_coordination_gate() {
    let classification =
        classify_bash("stateful intent declare --session-id s1 --workspace-id w1 src/auth.ts");

    assert_eq!(classification.kind, BashKind::ReadOnly);
    assert!(classification.reason.contains("coordination gate"));
}

#[test]
fn stateful_intent_declare_with_shell_control_syntax_is_denied() {
    let classification = classify_bash("stateful intent declare src/auth.ts; rm src/auth.ts");

    assert_eq!(classification.kind, BashKind::Mutating);
}

#[test]
fn stateful_commit_is_allowed_as_structured_git_escape_hatch() {
    let classification =
        classify_bash("stateful commit -m 'docs: add plan' -- docs/superpowers/plans/plan.md");

    assert_eq!(classification.kind, BashKind::ReadOnly);
}

#[test]
fn stateful_commit_with_shell_control_syntax_is_denied() {
    let classification =
        classify_bash("stateful commit -m 'docs: add plan' -- docs/plan.md; git add .");

    assert_eq!(classification.kind, BashKind::Mutating);
}

#[test]
fn stateful_commit_with_shell_control_or_redirection_syntax_is_denied() {
    let rejected = [
        "stateful commit -m x -- docs/a.md\nrm docs/b.md",
        "stateful commit -m x -- docs/a.md & rm docs/b.md",
        "stateful commit -m x -- docs/a.md>file",
        "stateful commit -m x -- docs/a.md $(git status)",
        "stateful commit -m x -- docs/a.md `git status`",
        "stateful commit -m x -- docs/a.md <(git status)",
        "stateful commit -m x -- docs/a.md >(cat)",
    ];

    for command in rejected {
        let classification = classify_bash(command);

        assert_ne!(
            classification.kind,
            BashKind::ReadOnly,
            "command `{command}` should not be allowed"
        );
    }
}

#[test]
fn stateful_commit_with_broad_pathspecs_is_not_allowed() {
    let rejected = [
        "",
        ".",
        "*",
        ":/",
        "-n",
        "docs/../plan.md",
        "docs/*.md",
        ":(glob)docs/*.md",
        "./docs/plan.md",
        "docs//plan.md",
        "/tmp/stateful/plan.md",
        "docs/",
    ];

    for pathspec in rejected {
        let classification = classify_bash(&format!(
            "stateful commit -m 'docs: add plan' -- {pathspec}"
        ));

        assert_ne!(
            classification.kind,
            BashKind::ReadOnly,
            "pathspec `{pathspec}` should not be allowed"
        );
    }
}

#[test]
fn other_stateful_control_commands_are_not_bash_allowed() {
    let classification = classify_bash("stateful sync-outbox");

    assert_eq!(classification.kind, BashKind::Mutating);
    assert!(classification.reason.contains("MCP"));
}
