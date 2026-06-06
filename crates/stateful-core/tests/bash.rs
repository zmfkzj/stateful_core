use stateful_core::{BashKind, classify_bash};

#[test]
fn classifies_common_inspection_commands_as_read_only() {
    for command in [
        "rg --no-config auth src",
        "LC_ALL=C rg --no-config auth src",
        "rg --no-config -n \"future work|Future work\" docs crates README.md",
        "rg --no-config '\\\"id\\\"' .",
        "rg --no-config \\\"id\\\" .",
        "grep -R auth src 2>/dev/null",
        "sed -n '1,20p' README.md",
        "tail -c 3 README.md | od -An -tx1",
        "find crates -type f | sed -n '1,20p'",
        "xxd -l 16 README.md",
        "uniq -f 1 README.md",
        "git diff --no-ext-diff --no-textconv -- README.md",
        "git --no-pager diff --no-ext-diff --no-textconv -- README.md",
        "git --no-pager remote -v",
        "git -C crates/stateful-core remote get-url origin",
        "git show --no-ext-diff --no-textconv HEAD:README.md",
        "git grep -n -e -O -- crates/stateful-core/src/bash.rs",
        "git grep -e --textc -- crates/stateful-core/src/bash.rs",
        "pwd",
        "date",
        "ls -la",
    ] {
        let classification = classify_bash(command);

        assert_eq!(
            classification.kind,
            BashKind::ReadOnly,
            "command `{command}` should be authorized as read-only inspection"
        );
    }
}

#[test]
fn classifies_write_capable_commands_as_mutating() {
    for command in [
        "printf '\\n' >> README.md",
        "echo hi > src/auth.ts",
        "cat <<EOF",
        "rg auth src | tee out.txt",
        "cargo test && rm README.md",
        "git diff --output=out.patch",
        "git diff --output\\=out.patch",
        "git diff --ext-diff",
        "git remote add origin git@example.invalid:repo.git",
        "sort --output=/tmp/sorted file",
        "sort --out=/tmp/sorted README.md",
        "sort --o=/tmp/sorted README.md",
        "sort -ro /tmp/sorted README.md",
        "sort --compress-program=./scripts/filter -S 1K big.txt",
        "sort --compress-prog=./scripts/filter -S 1K big.txt",
        "sort --com=./scripts/filter -S 1K big.txt",
        "find . -fprint out.txt",
        "find . -fls /tmp/files",
        "find . -ok rm {} \\;",
        "find -- . -maxdepth 0 -exec printf FOUND ';'",
        "fd -x rm {}",
        "fd -xsh -c 'printf pwned >/tmp/fd-pwned' .",
        "fd -Xsh -c 'printf pwned >/tmp/fd-pwned' .",
        "fd --exec=rm .",
        "yq -i '.version = 1' package.yml",
        "yq --inplace '.version = 1' package.yml",
        "yq eval --split-exp '.name' '.items[]' input.yml",
        "yq eval -s '.name' '.items[]' input.yml",
        "sed -n -f p README.md",
        "sed -n -Ei.bak '1p' README.md",
        "sed -n --in-place '1p' README.md",
        "xxd -r input.hex output.bin",
        "xxd README.md /tmp/readme.hex",
        "xxd - /tmp/out.hex",
        "uniq README.md /tmp/readme.uniq",
        "uniq - /tmp/out.txt",
        "file -C -m /tmp/custom.magic",
        "file --co -m /tmp/custom.magic README.md",
        "rm src/auth.ts",
        "mv old new",
        "cp old new",
        "mkdir src/new",
        "touch src/auth.ts",
        "chmod +x scripts/run.sh",
        "python scripts/generate.py",
        "npm install",
        "cargo fmt",
        "CARGO_TARGET_DIR=/tmp/stateful cargo fmt",
        "rustfmt crates/stateful-core/src/bash.rs",
        "prettier --write README.md",
        "git checkout -- README.md",
        "stateful intent declare --session-id s1 --workspace-id w1 src/auth.ts",
    ] {
        let classification = classify_bash(command);

        assert_eq!(
            classification.kind,
            BashKind::Mutating,
            "command `{command}` should require stateful write authorization"
        );
        assert!(
            classification.reason.contains("stateful"),
            "reason `{}` should direct callers to stateful tools",
            classification.reason
        );
    }
}

#[test]
fn classifies_unsafe_shell_constructs_as_unknown() {
    for command in [
        "pwd & rm README.md",
        "echo $(rm README.md)",
        "git diff ${OUT:---output=/tmp/out.patch}",
        "git diff --out{put=/tmp/out.patch,put=/tmp/out.patch}",
        "GIT_EXTERNAL_DIFF=rm git diff",
        "GIT_CONFIG_COUNT=1 GIT_CONFIG_KEY_0=diff.external GIT_CONFIG_VALUE_0=rm git diff",
        "GIT_PAGER=rm git log",
        "RIPGREP_CONFIG_PATH=./ripgreprc rg auth src",
        "PATH=/tmp/bin:/usr/bin rg auth src",
        "LD_PRELOAD=/tmp/libhack.so git status",
        "DYLD_INSERT_LIBRARIES=/tmp/libhack.dylib git status",
        "BASH_ENV=./envfile rg auth src",
        "rg auth README.md",
        "rg --no-config needle *",
        "rg --no-config needle ?",
        "rg --no-config needle [a-z]",
        "ls *.rs",
        "./rg auth src",
        "../bin/git status",
        "/tmp/stateful doctor",
        "./tools/stateful doctor",
        "./target/debug/stateful doctor",
        "target/debug/stateful doctor",
        "target/debug/stateful-bench run --pairs x --mode no-state",
        "git status --short",
        "git -C crates/stateful-core status --short",
        "git remote show origin",
        "git diff -- README.md",
        "git diff -- README.md --no-ext-diff --no-textconv",
        "rg auth README.md -- --no-config",
        "git diff --no-ext-diff --no-textconv --textconv -- file.bin",
        "git diff --no-ext-diff --no-textconv --textc -- file.bin",
        "git show HEAD:README.md",
        "git grep --textconv pattern",
        "git grep --textc pattern",
        "git grep --textcon pattern",
        "git grep --open-files-in-pager='sh -c \"printf pwned > /tmp/out\"' pattern",
        "git grep --open-files-in='sh -c \"printf pwned > /tmp/out\"' pattern",
        "git grep --open='sh -c \"printf pwned > /tmp/out\"' pattern",
        "git grep --open-files='sh -c \"printf pwned > /tmp/out\"' pattern",
        "git grep -Osh pattern",
        "git grep -nO'sh -c \"printf pwned > /tmp/out\"' pattern",
        "git -c diff.external=rm diff --no-ext-diff --no-textconv -- README.md",
        "git remote -v update",
        "git remote --verbose prune origin",
        "echo \\\"safe\\\"; rm README.md",
        "echo \\\\\\\"; touch /tmp/stateful-pwned; echo \\\\\\\"",
        "rg --pre 'sh -c \"rm README.md; cat\"' auth .",
        "rg --pre=./scripts/filter auth .",
        "echo `rm README.md`",
        "cat <(rm README.md)",
        "pwd\nrm README.md",
        "awk 'BEGIN{system(\"rm README.md\")}'",
        "sed '1w out.txt' README.md",
    ] {
        let classification = classify_bash(command);

        assert_eq!(
            classification.kind,
            BashKind::Unknown,
            "command `{command}` should be denied instead of trusted as read-only"
        );
    }
}

#[test]
fn classifies_validation_commands_as_validation_bypass() {
    for command in [
        "cargo test",
        "CARGO_TARGET_DIR=/tmp/stateful cargo test",
        "cargo build",
        "cargo fmt --all --check",
        "npm test",
        "make test",
        "stateful validate cargo-test",
    ] {
        let classification = classify_bash(command);

        assert_eq!(
            classification.kind,
            BashKind::ValidationBypass,
            "command `{command}` should use controlled validation"
        );
        assert!(
            classification.reason.contains("validation"),
            "reason `{}` should direct callers to validation profiles",
            classification.reason
        );
    }
}

#[test]
fn classifies_mixed_validation_and_unknown_commands_as_unknown() {
    let classification = classify_bash("cargo test && unknown-command --flag");

    assert_eq!(classification.kind, BashKind::Unknown);
}

#[test]
fn classifies_stateful_diagnostics_as_read_only() {
    for command in ["stateful doctor"] {
        let classification = classify_bash(command);

        assert_eq!(classification.kind, BashKind::ReadOnly);
    }
}

#[test]
fn classifies_unknown_commands_as_unknown() {
    let classification = classify_bash("unknown-command --flag");

    assert_eq!(classification.kind, BashKind::Unknown);
    assert!(
        classification.reason.contains("Unrecognized"),
        "reason `{}` should explain why the command is not trusted",
        classification.reason
    );
}
