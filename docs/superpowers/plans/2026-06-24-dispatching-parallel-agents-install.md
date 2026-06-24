# Dispatching Parallel Agents Install Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Install `dispatching-parallel-agents` with every Stateful Codex and OMP install so DeNovo OMP stateful runs receive it through the normal OMP install path.

**Architecture:** Reuse the existing installer asset-copy path for skills. Add one new embedded CLI asset and write it beside `skills/stateful-command-policy/SKILL.md` for both Codex and OMP installs; DeNovo remains unchanged because it already calls OMP install.

**Tech Stack:** Rust installer code in `crates/stateful-cli`, static assets via `include_str!`, Rust integration tests in `crates/stateful-cli/tests/install_global.rs`.

## Global Constraints

- Do not add benchmark-specific duplicate copying.
- Do not change DeNovo benchmark prompts.
- Keep `stateful-command-policy` unchanged.
- Use the smallest installer change that writes the new skill for both Codex and OMP.
- In this repo, run shell commands only through the Stateful sandbox tools required by `stateful-command-policy`.

---

## File Structure

- Create `crates/stateful-cli/assets/dispatching-parallel-agents/SKILL.md`: embedded source for the installed skill. Copy the current `skill://dispatching-parallel-agents` content verbatim.
- Modify `crates/stateful-cli/src/install.rs`: plan and apply the new Codex/OMP skill file.
- Modify `crates/stateful-cli/tests/install_global.rs`: assert dry-run plans and applied installs include the new skill and match the asset content.

---

### Task 1: Add failing installer tests

**Files:**
- Modify: `crates/stateful-cli/tests/install_global.rs:32-71`
- Modify: `crates/stateful-cli/tests/install_global.rs:171-322`
- Modify: `crates/stateful-cli/tests/install_global.rs:542-587`

**Interfaces:**
- Consumes: existing test helpers `TestFixture::codex_config_parent()`, `TestFixture::omp_agent_dir()`, `apply_codex_install`, `apply_omp_install`, `plan_codex_install`, `plan_omp_install`.
- Produces: failing coverage for `skills/dispatching-parallel-agents/SKILL.md` plan and install paths.

- [ ] **Step 1: Update Codex dry-run test to expect the new skill path**

In `install_codex_dry_run_plans_codex_config_without_writing`, replace the single `skill_path` binding with two paths and add both assertions:

```rust
    let command_policy_skill_path = fixture
        .codex_config_parent()
        .join("skills/stateful-command-policy/SKILL.md");
    let dispatching_skill_path = fixture
        .codex_config_parent()
        .join("skills/dispatching-parallel-agents/SKILL.md");

    assert!(plan.summary.contains("dry-run"));
    assert!(applied.summary.contains("dry-run"));
    assert!(plan.files.contains(&fixture.paths.home));
    assert!(plan.files.contains(&fixture.paths.state_db));
    assert!(plan.files.contains(&fixture.codex_config));
    assert!(plan.files.contains(&command_policy_skill_path));
    assert!(plan.files.contains(&dispatching_skill_path));
    assert!(!fixture.paths.home.exists());
    assert!(!fixture.codex_config.exists());
    assert!(!dispatching_skill_path.exists());
```

- [ ] **Step 2: Update OMP dry-run test to expect the new skill path**

In `install_omp_dry_run_plans_command_policy_skill_without_writing`, keep the existing command-policy path and add a dispatching path:

```rust
    let command_policy_skill_path = fixture
        .omp_agent_dir()
        .join("skills")
        .join("stateful-command-policy")
        .join("SKILL.md");
    let dispatching_skill_path = fixture
        .omp_agent_dir()
        .join("skills")
        .join("dispatching-parallel-agents")
        .join("SKILL.md");

    assert!(plan.summary.contains("dry-run"));
    assert!(applied.summary.contains("dry-run"));
    assert!(plan.files.contains(&command_policy_skill_path));
    assert!(plan.files.contains(&dispatching_skill_path));
    assert!(!fixture.paths.home.exists());
    assert!(!command_policy_skill_path.exists());
    assert!(!dispatching_skill_path.exists());
```

- [ ] **Step 3: Update OMP apply test to assert installed content**

In `install_omp_yes_creates_extension_and_mcp_config`, add the path after `omp_skill`:

```rust
    let omp_dispatching_skill = omp_agent_dir
        .join("skills")
        .join("dispatching-parallel-agents")
        .join("SKILL.md");
```

Add assertions near the existing command-policy skill assertions:

```rust
    let dispatching_skill = fs::read_to_string(&omp_dispatching_skill)
        .expect("omp dispatching skill should read");
    let source_dispatching_skill = fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("assets/dispatching-parallel-agents/SKILL.md"),
    )
    .expect("source dispatching skill should exist");
    assert_eq!(dispatching_skill, source_dispatching_skill);
    assert!(dispatching_skill.contains("name: dispatching-parallel-agents"));
    assert!(plan.files.contains(&omp_dispatching_skill));
```

- [ ] **Step 4: Update Codex apply test to assert installed content**

In `install_codex_yes_creates_global_command_policy_skill`, add a second skill path and assertions:

```rust
    let dispatching_skill_path = fixture
        .codex_config_parent()
        .join("skills/dispatching-parallel-agents/SKILL.md");
    let dispatching_skill = fs::read_to_string(&dispatching_skill_path)
        .expect("global dispatching skill should exist");
    let source_dispatching_skill = fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("assets/dispatching-parallel-agents/SKILL.md"),
    )
    .expect("source dispatching skill should exist");
    assert_eq!(dispatching_skill, source_dispatching_skill);
    assert!(dispatching_skill.contains("name: dispatching-parallel-agents"));
    assert!(dispatching_skill.contains("Dispatch one agent per independent problem domain"));
```

- [ ] **Step 5: Run the focused tests to verify they fail**

Run:

```bash
cargo test -p stateful-cli --test install_global install_
```

Expected: FAIL because `assets/dispatching-parallel-agents/SKILL.md` does not exist and installer plan/apply paths do not include it.

- [ ] **Step 6: Commit failing tests only**

Run:

```bash
git add crates/stateful-cli/tests/install_global.rs
git commit -m "test: cover installed dispatching skill"
```

Expected: commit succeeds with only test changes.

---

### Task 2: Install the dispatching skill asset

**Files:**
- Create: `crates/stateful-cli/assets/dispatching-parallel-agents/SKILL.md`
- Modify: `crates/stateful-cli/src/install.rs:127-167`
- Modify: `crates/stateful-cli/src/install.rs:170-241`
- Modify: `crates/stateful-cli/src/install.rs:338-373`
- Modify: `crates/stateful-cli/src/install.rs:2267-2274`

**Interfaces:**
- Consumes: tests from Task 1.
- Produces: `dispatching-parallel-agents` skill installed for Codex and OMP.

- [ ] **Step 1: Create the skill asset**

Create `crates/stateful-cli/assets/dispatching-parallel-agents/SKILL.md` by copying the current `skill://dispatching-parallel-agents` content verbatim. The file must start with:

```markdown
---
name: dispatching-parallel-agents
description: Use when facing 2+ independent tasks that can be worked on without shared state or sequential dependencies
---
```

- [ ] **Step 2: Add Codex plan/apply calls**

In `plan_codex_install`, add the dispatching skill path immediately after the command-policy skill path:

```rust
    plan.files
        .push(global_command_policy_skill_path(&options.codex_config_path));
    plan.files.push(global_dispatching_parallel_agents_skill_path(
        &options.codex_config_path,
    ));
```

In `apply_codex_install`, write the skill immediately after `write_global_command_policy_skill`:

```rust
    write_global_command_policy_skill(&options.codex_config_path)?;
    write_global_dispatching_parallel_agents_skill(&options.codex_config_path)?;
```

- [ ] **Step 3: Add OMP plan/apply calls**

In `plan_omp_install`, add a second skill path variable:

```rust
    let command_policy_skill_path = omp_command_policy_skill_path(&agent_dir);
    let dispatching_skill_path = omp_dispatching_parallel_agents_skill_path(&agent_dir);
```

Replace the existing `plan.files.push(skill_path);` with:

```rust
    plan.files.push(command_policy_skill_path);
    plan.files.push(dispatching_skill_path);
```

In `apply_omp_install`, write the skill immediately after command-policy skill:

```rust
    write_omp_command_policy_skill(&agent_dir)?;
    write_omp_dispatching_parallel_agents_skill(&agent_dir)?;
```

- [ ] **Step 4: Add path/write helpers**

Add these helpers after `omp_command_policy_skill_path`:

```rust
fn write_global_dispatching_parallel_agents_skill(codex_config_path: &Path) -> anyhow::Result<()> {
    let path = global_dispatching_parallel_agents_skill_path(codex_config_path);
    let parent = containing_dir(&path);
    fs::create_dir_all(parent).with_context(|| {
        format!(
            "failed to create Codex dispatching skill directory {}",
            parent.display()
        )
    })?;
    fs::write(&path, dispatching_parallel_agents_skill())
        .with_context(|| format!("failed to write {}", path.display()))
}

fn global_dispatching_parallel_agents_skill_path(codex_config_path: &Path) -> PathBuf {
    containing_dir(codex_config_path)
        .join("skills")
        .join("dispatching-parallel-agents")
        .join("SKILL.md")
}

fn write_omp_dispatching_parallel_agents_skill(agent_dir: &Path) -> anyhow::Result<()> {
    let path = omp_dispatching_parallel_agents_skill_path(agent_dir);
    let parent = containing_dir(&path);
    fs::create_dir_all(parent).with_context(|| {
        format!(
            "failed to create OMP dispatching skill directory {}",
            parent.display()
        )
    })?;
    fs::write(&path, dispatching_parallel_agents_skill())
        .with_context(|| format!("failed to write {}", path.display()))
}

fn omp_dispatching_parallel_agents_skill_path(agent_dir: &Path) -> PathBuf {
    agent_dir
        .join("skills")
        .join("dispatching-parallel-agents")
        .join("SKILL.md")
}
```

- [ ] **Step 5: Embed the new asset**

Add this beside `stateful_command_policy_skill()`:

```rust
fn dispatching_parallel_agents_skill() -> &'static str {
    include_str!("../assets/dispatching-parallel-agents/SKILL.md")
}
```

- [ ] **Step 6: Run focused installer tests**

Run:

```bash
cargo test -p stateful-cli --test install_global install_
```

Expected: PASS for all installer tests selected by the `install_` filter.

- [ ] **Step 7: Commit implementation**

Run:

```bash
git add crates/stateful-cli/assets/dispatching-parallel-agents/SKILL.md crates/stateful-cli/src/install.rs crates/stateful-cli/tests/install_global.rs
git commit -m "feat: install dispatching skill"
```

Expected: commit succeeds with the asset, installer, and test changes.

---

### Task 3: Verify the shared DeNovo OMP path remains covered

**Files:**
- Test only: `crates/stateful-bench/tests/cli.rs`
- Test only: `crates/stateful-cli/tests/install_global.rs`

**Interfaces:**
- Consumes: Task 2 installer behavior.
- Produces: verification evidence that DeNovo OMP still uses OMP install and installer tests cover the skill asset.

- [ ] **Step 1: Run OMP CLI parser coverage**

Run:

```bash
cargo test -p stateful-bench --test cli denovo_run_command_parses_omp_cli_agent_options
```

Expected: PASS. This confirms the DeNovo OMP CLI path still accepts OMP options; no benchmark adapter change was needed.

- [ ] **Step 2: Run all installer tests**

Run:

```bash
cargo test -p stateful-cli --test install_global
```

Expected: PASS. This confirms Codex and OMP installer behavior as a group.

- [ ] **Step 3: Commit if verification required incidental fixes**

If Step 1 or Step 2 required code changes, commit only the exact files changed in this task. For example, if only installer code changed:

```bash
git add crates/stateful-cli/src/install.rs crates/stateful-cli/tests/install_global.rs
git commit -m "fix: keep installer verification passing"
```

Expected: no commit is needed if Task 2 was correct.

---

## Plan Self-Review

- Spec coverage: Codex install, OMP install, normal DeNovo inheritance, no prompt changes, no benchmark duplicate copy, and installer tests are each covered by Tasks 1-3.
- Red-flag scan: no unresolved markers or unspecified implementation steps remain.
- Type consistency: helper names and asset paths match across tests and installer code.
