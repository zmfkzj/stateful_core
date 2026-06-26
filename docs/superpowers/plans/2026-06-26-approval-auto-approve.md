# Stateful Approval Auto-approve Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add opt-in auto-approval for Stateful-owned OMP approval prompts, starting with `ext_rw_bash`.

**Architecture:** Keep the change inside the generated OMP extension emitted by `crates/stateful-cli/src/install.rs`. Add one config/tool-param helper and reuse the existing external grant cache so auto-approved calls follow the same grant key, max-use, TTL, and sandbox execution path as manually approved calls.

**Tech Stack:** Rust generator code, generated JavaScript OMP extension, Rust integration tests, Markdown docs.

## Global Constraints

- No new dependencies.
- Preserve Stateful sandbox, hook, reservation, claim, raw Bash/eval denial, and external scope validation behavior.
- `stateful.autoApprove: true` enables profile-wide auto-approval for Stateful-owned prompts.
- `ext_rw_bash(auto_approve: true)` enables per-call auto-approval.
- Auto-approval skips only OMP UI confirmation; it must still record grant metadata and respect grant limits.
- Follow TDD: write a failing test first, run it red, implement minimal code, run it green.
- Use `sandbox_bash` with `fs=build` for tests; use `sandbox_bash` with `fs=git` for git operations.

---

## File Structure

- Modify `crates/stateful-cli/tests/install_global.rs`: generated extension assertions for schema, config lookup, auto-approval branch, and shared grant recording.
- Modify `crates/stateful-cli/src/install.rs`: generated JavaScript helper, grant-recording helper, `ext_rw_bash` schema, and execute flow.
- Modify `README.md`: one sentence in the generated OMP tool summary.
- Modify `docs/usage-reference.md`: user-facing option names and safety boundary.
- Modify `docs/architecture.md`: generated extension behavior and approval boundary.
- Modify `docs/current-state-coordination.md`: policy/state-model wording for auto-approval.
- Modify `docs/implementation-contract.md`: contract wording for OMP approval behavior.
- Modify `crates/stateful-cli/assets/omp-stateful-required-rule.md`: agent-facing rule update.
- Modify `crates/stateful-cli/assets/stateful-command-policy/SKILL.md`: agent-facing manual update.

---

### Task 1: Generated extension test

**Files:**
- Modify: `crates/stateful-cli/tests/install_global.rs:318-342`
- Test: `crates/stateful-cli/tests/install_global.rs`

**Interfaces:**
- Consumes: generated extension text loaded into `extension` in `install_omp_yes_writes_config_mcp_extension_skills_and_custom_tools`.
- Produces: failing assertions requiring `auto_approve`, `stateful.autoApprove`, `shouldAutoApproveStatefulPrompt`, `recordExternalBashGrant`, and auto-approval prompt skip behavior.

- [ ] **Step 1: Write the failing assertions**

Insert these assertions after the existing `assert!(extension.contains("ctx?.ui?.confirm"));` line:

```rust
    assert!(extension.contains("auto_approve: { type: \"boolean\""));
    assert!(extension.contains("function shouldAutoApproveStatefulPrompt(ctx, params)"));
    assert!(extension.contains("ctx?.config?.stateful?.autoApprove"));
    assert!(extension.contains("params?.auto_approve === true"));
    assert!(extension.contains("function recordExternalBashGrant(params, now)"));
    assert!(extension.contains("if (!shouldAutoApproveStatefulPrompt(ctx, params))"));
```

- [ ] **Step 2: Run the targeted test to verify RED**

Run with OMP `sandbox_bash`:

```text
fs: build
network: enabled
write_dirs: ["approval-auto-approve-test"]
command: cargo test -p stateful-cli --test install_global install_omp_yes_writes_config_mcp_extension_skills_and_custom_tools -- --exact
async: false
```

Expected: FAIL in `install_omp_yes_writes_config_mcp_extension_skills_and_custom_tools` because the generated extension lacks the new strings.

- [ ] **Step 3: Commit is not allowed yet**

Do not commit red-only tests. Continue to Task 2 in the same working tree.

---

### Task 2: Generated extension implementation

**Files:**
- Modify: `crates/stateful-cli/src/install.rs:2046-2118`
- Modify: `crates/stateful-cli/src/install.rs:2683-2737`
- Test: `crates/stateful-cli/tests/install_global.rs`

**Interfaces:**
- Consumes: test assertions from Task 1.
- Produces: generated extension support for `stateful.autoApprove` and `auto_approve` without changing sandbox args.

- [ ] **Step 1: Add prompt auto-approval helpers**

In the generated JavaScript section, add these helpers before `externalBashApprovalMessage(params)`:

```javascript
function statefulPromptAutoApproveConfig(ctx) {
  return ctx?.config?.stateful?.autoApprove === true;
}

function shouldAutoApproveStatefulPrompt(ctx, params) {
  return statefulPromptAutoApproveConfig(ctx) || params?.auto_approve === true;
}

function recordExternalBashGrant(params, now) {
  const key = externalGrantKey(params);
  const settings = externalGrantSettings(params);
  const approvedAt = now ?? Date.now();
  externalBashGrants.set(key, {
    expiresAt: approvedAt + settings.ttlMs,
    maxUses: settings.maxUses,
    uses: 1,
  });
}
```

- [ ] **Step 2: Reuse the helper in `ensureExternalBashGrant`**

Replace the manual grant-recording block in `ensureExternalBashGrant` with:

```javascript
  recordExternalBashGrant(params, Date.now());
  return true;
```

The function should still prune grants, reuse existing grants, call `confirmExternalBashGrant`, and return false when not approved.

- [ ] **Step 3: Add the tool schema flag**

In the `ext_rw_bash` properties block, add:

```javascript
        auto_approve: { type: "boolean", description: "Skip the OMP UI approval prompt for this Stateful-owned external write grant. Sandbox scope validation and Stateful hook authorization still apply." },
```

- [ ] **Step 4: Skip `ctx.ui.confirm` only when auto-approved**

Replace the current unconditional `ctx.ui.confirm` availability check with this branch:

```javascript
      if (!shouldAutoApproveStatefulPrompt(ctx, params) && typeof ctx?.ui?.confirm !== "function") {
        return {
          isError: true,
          content: [{ type: "text", text: "ext_rw_bash requires OMP UI confirmation, but ctx.ui.confirm is unavailable." }],
          details: { error: "confirmation_unavailable" },
        };
      }
```

Then replace the grant call with:

```javascript
        if (shouldAutoApproveStatefulPrompt(ctx, params)) {
          pruneExternalBashGrants(Date.now());
          recordExternalBashGrant(params, Date.now());
          approved = true;
        } else {
          approved = await ensureExternalBashGrant(ctx, params, signal);
        }
```

- [ ] **Step 5: Run the targeted test to verify GREEN**

Run with OMP `sandbox_bash`:

```text
fs: build
network: enabled
write_dirs: ["approval-auto-approve-test"]
command: cargo test -p stateful-cli --test install_global install_omp_yes_writes_config_mcp_extension_skills_and_custom_tools -- --exact
async: false
```

Expected: PASS.

- [ ] **Step 6: Commit code and test together**

Run with OMP `sandbox_bash`:

```text
fs: git
network: disabled
command: git add crates/stateful-cli/src/install.rs crates/stateful-cli/tests/install_global.rs
async: false
```

Then:

```text
fs: git
network: disabled
command: git commit -m "Add OMP auto approval option"
async: false
```

---

### Task 3: Documentation update

**Files:**
- Modify: `README.md`
- Modify: `docs/usage-reference.md`
- Modify: `docs/architecture.md`
- Modify: `docs/current-state-coordination.md`
- Modify: `docs/implementation-contract.md`
- Modify: `crates/stateful-cli/assets/omp-stateful-required-rule.md`
- Modify: `crates/stateful-cli/assets/stateful-command-policy/SKILL.md`
- Test: `crates/stateful-cli/tests/install_global.rs`

**Interfaces:**
- Consumes: option names from Task 2: `stateful.autoApprove` and `auto_approve`.
- Produces: docs that state auto-approval skips only Stateful-owned OMP UI prompts and leaves authorization intact.

- [ ] **Step 1: Update each `ext_rw_bash` approval sentence**

Use this exact wording where the docs describe `ext_rw_bash` prompting for a scoped grant:

```markdown
`ext_rw_bash` asks for a scoped OMP UI grant by default; `stateful.autoApprove: true` or the per-call `auto_approve: true` flag skips only that Stateful-owned prompt while sandbox scope validation, hooks, reservation/claim checks, and grant limits still apply.
```

When a paragraph already explains raw command text hiding, keep that sentence and add:

```markdown
When auto-approval is enabled, no prompt is shown.
```

- [ ] **Step 2: Update generated skill/rule wording**

In `crates/stateful-cli/assets/omp-stateful-required-rule.md` and `crates/stateful-cli/assets/stateful-command-policy/SKILL.md`, use this compact wording near the existing `ext_rw_bash` guidance:

```markdown
`ext_rw_bash` prompts for a scoped OMP UI grant unless `stateful.autoApprove: true` or per-call `auto_approve: true` is set; auto-approval skips only the Stateful-owned UI prompt and does not bypass Stateful sandbox scope validation, hooks, reservation/claim checks, or grant limits.
```

- [ ] **Step 3: Run the install test that compares generated assets**

Run with OMP `sandbox_bash`:

```text
fs: build
network: enabled
write_dirs: ["approval-auto-approve-docs-test"]
command: cargo test -p stateful-cli --test install_global install_omp_yes_writes_config_mcp_extension_skills_and_custom_tools -- --exact
async: false
```

Expected: PASS. This catches drift between source assets and installed files.

- [ ] **Step 4: Commit documentation**

Run with OMP `sandbox_bash`:

```text
fs: git
network: disabled
command: git add README.md docs/usage-reference.md docs/architecture.md docs/current-state-coordination.md docs/implementation-contract.md crates/stateful-cli/assets/omp-stateful-required-rule.md crates/stateful-cli/assets/stateful-command-policy/SKILL.md
async: false
```

Then:

```text
fs: git
network: disabled
command: git commit -m "Document OMP auto approval option"
async: false
```

---

### Task 4: Final targeted verification

**Files:**
- Test: `crates/stateful-cli/tests/install_global.rs`

**Interfaces:**
- Consumes: implementation and docs from Tasks 1-3.
- Produces: final evidence that generated install output includes the option and asset comparisons still pass.

- [ ] **Step 1: Run the full install-global test file**

Run with OMP `sandbox_bash`:

```text
fs: build
network: enabled
write_dirs: ["approval-auto-approve-final"]
command: cargo test -p stateful-cli --test install_global
async: false
```

Expected: PASS for every test in `install_global.rs`.

- [ ] **Step 2: Check the current branch status**

Run with OMP `sandbox_bash`:

```text
fs: git
network: disabled
command: git status --short
async: false
```

Expected: no uncommitted files from this task.
