# OMP Command-Policy Skill Install Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make `stateful install --agent omp --yes` install the canonical `stateful-command-policy` skill under the OMP agent home.

**Architecture:** Reuse the existing `stateful-command-policy` asset and install it into `default_omp_agent_dir(paths)/skills/stateful-command-policy/SKILL.md`. Codex install remains unchanged; OMP install gains one planned file path and one write call.

**Tech Stack:** Rust CLI, `std::fs`, existing `stateful_cli::install` helpers, Cargo integration tests in `crates/stateful-cli/tests/install_global.rs`.

---

## File Structure

- Modify `crates/stateful-cli/tests/install_global.rs`
  - Extend OMP dry-run/apply coverage.
  - Compare installed OMP skill bytes to `crates/stateful-cli/assets/stateful-command-policy/SKILL.md`.
- Modify `crates/stateful-cli/src/install.rs`
  - Add `omp_command_policy_skill_path(&GlobalPaths) -> PathBuf`.
  - Add `write_omp_command_policy_skill(&GlobalPaths) -> anyhow::Result<()>`.
  - Add the OMP skill path to `plan_omp_install`.
  - Call the writer from `apply_omp_install`.

---

### Task 1: Install command-policy skill for OMP

**Files:**
- Modify: `crates/stateful-cli/tests/install_global.rs:151-193`
- Modify: `crates/stateful-cli/tests/install_global.rs:195-228`
- Modify: `crates/stateful-cli/src/install.rs:167-230`
- Modify: `crates/stateful-cli/src/install.rs:327-346`

- [ ] **Step 1: Write the failing dry-run/apply assertions**

In `crates/stateful-cli/tests/install_global.rs`, update `install_omp_yes_creates_extension_and_mcp_config` by adding the `omp_skill` path near the existing OMP paths and asserting both the plan and file content:

```rust
    let omp_skill = omp_agent_dir
        .join("skills")
        .join("stateful-command-policy")
        .join("SKILL.md");

    assert!(omp_config.is_file());
    assert!(omp_mcp.is_file());
    assert!(omp_extension.is_file());
    assert!(omp_skill.is_file());
```

Then add this content assertion before the final `plan.files` assertion:

```rust
    let command_policy_skill = fs::read_to_string(&omp_skill).expect("omp skill should read");
    let source_command_policy_skill = fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/stateful-command-policy/SKILL.md"),
    )
    .expect("source stateful command policy skill should exist");
    assert_eq!(command_policy_skill, source_command_policy_skill);
    assert!(command_policy_skill.contains("name: stateful-command-policy"));
    assert!(plan.files.contains(&omp_skill));
```

Also add a new test after `install_codex_dry_run_plans_codex_config_without_writing`:

```rust
#[test]
fn install_omp_dry_run_plans_command_policy_skill_without_writing() {
    let fixture = TestFixture::new("omp-dry-run-skill");
    let options = OmpInstallOptions {
        yes: false,
        paths: fixture.paths.clone(),
        binary_path: "/opt/stateful/bin/stateful".to_string(),
        project_config_path: None,
    };

    let plan = stateful_cli::plan_omp_install(&options).expect("omp install should plan");
    let applied = apply_omp_install(options).expect("dry-run omp install should succeed");
    let skill_path = fixture
        .paths
        .home
        .join(".omp")
        .join("agent")
        .join("skills")
        .join("stateful-command-policy")
        .join("SKILL.md");

    assert!(plan.summary.contains("dry-run"));
    assert!(applied.summary.contains("dry-run"));
    assert!(plan.files.contains(&skill_path));
    assert!(!fixture.paths.home.exists());
    assert!(!skill_path.exists());
}
```

- [ ] **Step 2: Run tests to verify RED**

Run:

```bash
/Users/arthur/.cargo/bin/stateful sandbox run --fs build --network enabled --write-dir red-omp-skill-install --command 'cargo test -p stateful-cli install_omp'
```

Expected: failure because `plan_omp_install` does not include `skills/stateful-command-policy/SKILL.md` and `apply_omp_install` does not write that file.

- [ ] **Step 3: Add minimal OMP skill install implementation**

In `crates/stateful-cli/src/install.rs`, add the skill path to `plan_omp_install` after `mcp_path` is pushed:

```rust
    let skill_path = omp_command_policy_skill_path(&options.paths);
    plan.files.push(config_path);
    plan.files.push(extension_path);
    plan.files.push(mcp_path);
    plan.files.push(skill_path);
```

In `apply_omp_install`, call the writer after `write_omp_mcp_config`:

```rust
    write_omp_config(&config_path, &extension_path)?;
    write_omp_extension(&extension_path, &options.binary_path)?;
    write_omp_mcp_config(&mcp_path, &options.binary_path)?;
    write_omp_command_policy_skill(&options.paths)?;
```

Below `write_global_command_policy_skill`, add these helpers:

```rust
fn write_omp_command_policy_skill(paths: &GlobalPaths) -> anyhow::Result<()> {
    let path = omp_command_policy_skill_path(paths);
    let parent = containing_dir(&path);
    fs::create_dir_all(parent).with_context(|| {
        format!(
            "failed to create OMP skills directory {}",
            parent.display()
        )
    })?;
    fs::write(&path, stateful_command_policy_skill())
        .with_context(|| format!("failed to write {}", path.display()))
}

fn omp_command_policy_skill_path(paths: &GlobalPaths) -> PathBuf {
    default_omp_agent_dir(paths)
        .join("skills")
        .join("stateful-command-policy")
        .join("SKILL.md")
}
```

- [ ] **Step 4: Run focused tests to verify GREEN**

Run:

```bash
/Users/arthur/.cargo/bin/stateful sandbox run --fs build --network enabled --write-dir green-omp-skill-install --command 'cargo test -p stateful-cli install_omp'
/Users/arthur/.cargo/bin/stateful sandbox run --fs build --network enabled --write-dir green-codex-skill-install --command 'cargo test -p stateful-cli install_codex_yes_creates_global_command_policy_skill'
```

Expected: all selected tests pass.

- [ ] **Step 5: Run formatting check**

Run:

```bash
/Users/arthur/.cargo/bin/stateful sandbox run --fs build --network enabled --write-dir fmt-omp-skill-install --command 'cargo fmt --check'
```

Expected: exit code 0.

- [ ] **Step 6: Review focused diff**

Run:

```bash
/Users/arthur/.cargo/bin/stateful sandbox run --fs git --network disabled --command 'git --no-pager diff --no-ext-diff -- crates/stateful-cli/src/install.rs crates/stateful-cli/tests/install_global.rs'
```

Expected: diff only adds OMP skill plan/write behavior and tests.

- [ ] **Step 7: Commit and push**

Run:

```bash
/Users/arthur/.cargo/bin/stateful sandbox run --fs git --network disabled --command 'git add crates/stateful-cli/src/install.rs crates/stateful-cli/tests/install_global.rs'
/Users/arthur/.cargo/bin/stateful sandbox run --fs git --network disabled --command 'git commit -m "Install command policy skill for OMP"'
/Users/arthur/.cargo/bin/stateful sandbox run --fs git --network enabled --command 'git push origin dev'
```

Expected: commit on `dev` and push succeeds.
