# Glob Default Allowlist Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add `glob` to the default OMP allowed tool list.

**Architecture:** The repo registry already builds default repo allowlists from two fixed slices: Codex tools and OMP tools. This change appends `glob` to the OMP slice and updates tests that assert the exact default allowlist.

**Tech Stack:** Rust 2024 workspace, `stateful-cli` crate, standard Rust integration tests.

## Global Constraints

- Do not change Codex-specific default allowed tools.
- Do not add new dependencies.
- Do not add new abstractions or duplicate allowlist logic.
- Keep user-managed allowlist behavior unchanged.
- Use TDD: update expected behavior first, verify failure, then update production code.
- Stage only files changed for this task; leave existing unrelated dirty files untouched.

---

### Task 1: Add glob to default allowlist

**Files:**
- Modify: `crates/stateful-cli/tests/repo_registry.rs:9-25`
- Modify: `crates/stateful-cli/tests/cli.rs:723-737`
- Modify: `crates/stateful-cli/src/repo_registry.rs:12-22`

**Interfaces:**
- Consumes: existing `default_allowed_tools()` test helper and `DEFAULT_OMP_ALLOWED_TOOLS` constant.
- Produces: default repo allowlist containing `glob` after `lsp`.

- [ ] **Step 1: Write the failing tests**

In `crates/stateful-cli/tests/repo_registry.rs`, update the helper to include `glob` after `lsp`:

```rust
fn default_allowed_tools() -> Vec<String> {
    [
        "multi_agent_v1spawn_agent",
        "multi_agent_v1wait_agent",
        "multi_agent_v1close_agent",
        "multi_agent_v1resume_agent",
        "mcp__openaiDeveloperDocs__fetch_openai_doc",
        "mcp__openaiDeveloperDocs__search_openai_docs",
        "multi_agent_v1send_input",
        "task",
        "yield",
        "lsp",
        "glob",
    ]
    .into_iter()
    .map(str::to_string)
    .collect()
}
```

In `crates/stateful-cli/tests/cli.rs`, update the `tools_list_prints_allowed_and_unclassified_tools` expected JSON to include `glob` after `lsp` and before `KnownTool`:

```rust
    assert_eq!(
        json["allowed_tools"],
        serde_json::json!([
            "multi_agent_v1spawn_agent",
            "multi_agent_v1wait_agent",
            "multi_agent_v1close_agent",
            "multi_agent_v1resume_agent",
            "mcp__openaiDeveloperDocs__fetch_openai_doc",
            "mcp__openaiDeveloperDocs__search_openai_docs",
            "multi_agent_v1send_input",
            "task",
            "yield",
            "lsp",
            "glob",
            "KnownTool"
        ])
    );
```

- [ ] **Step 2: Run tests to verify they fail**

Run:

```bash
cargo test -p stateful-cli --test repo_registry enable_repo_registers_git_root_and_writes_repo_configs -- --exact
```

Expected: FAIL because `entry.allowed_tools` lacks `glob`.

Run:

```bash
cargo test -p stateful-cli --test cli tools_list_prints_allowed_and_unclassified_tools -- --exact
```

Expected: FAIL because `json["allowed_tools"]` lacks `glob`.

- [ ] **Step 3: Write minimal implementation**

In `crates/stateful-cli/src/repo_registry.rs`, change only the OMP defaults:

```rust
const DEFAULT_OMP_ALLOWED_TOOLS: &[&str] = &["task", "yield", "lsp", "glob"];
```

- [ ] **Step 4: Run tests to verify they pass**

Run:

```bash
cargo test -p stateful-cli --test repo_registry enable_repo_registers_git_root_and_writes_repo_configs -- --exact
```

Expected: PASS.

Run:

```bash
cargo test -p stateful-cli --test cli tools_list_prints_allowed_and_unclassified_tools -- --exact
```

Expected: PASS.

- [ ] **Step 5: Run focused regression tests**

Run:

```bash
cargo test -p stateful-cli --test repo_registry tool_allowlist_is_repo_scoped_deduplicated_and_preserved -- --exact
```

Expected: PASS, proving user-managed allowlist behavior still works with the new default.

- [ ] **Step 6: Commit implementation**

Run:

```bash
git add crates/stateful-cli/src/repo_registry.rs crates/stateful-cli/tests/repo_registry.rs crates/stateful-cli/tests/cli.rs
git commit -m "fix: allow glob by default"
```
