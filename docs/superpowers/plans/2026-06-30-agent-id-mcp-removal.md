# Agent ID and MCP Removal Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace Stateful's agent-facing coordination identity with extension-injected `agent_id` and delete the agent-facing MCP path.

**Architecture:** OMP becomes the supported agent-facing integration: its extension/native tools inject the active `agent_id` into every Stateful hook and state operation. Runtime current-session files, session env fallback, and MCP current-session repair are removed instead of shimmed. Server/store ownership fields are renamed from `session_id` to `agent_id` in one clean cutover.

**Tech Stack:** Rust workspace (`stateful-cli`, `stateful-store`, `stateful-core`, `stateful-bench`), generated OMP extension JavaScript in `install.rs`, Python benchmark adapters, SQLite-backed local store.

## Global Constraints

- Strict identity only: if the adapter/extension cannot provide the active agent's explicit `agent_id`, Stateful coordination must deny instead of falling back.
- No fallback from `ctx.sessionId`, `event.sessionId`, session file paths, leaf/path hashes, `STATEFUL_SESSION_ID`, `CODEX_THREAD_ID`, or `STATEFUL_CODEX_RUN_ID` to `agent_id`.
- Main agent, each native subagent, and each fork/handoff flow must have distinct `agent_id` values; repeated calls inside the same active agent flow must keep the same `agent_id`.
- Agents must not hand-write their own coordination identity in MCP args; extension/native tools are the only agent-facing path.
- Delete MCP from agent-facing Stateful: no installed `[mcp_servers.stateful]`, no OMP `mcp.json`, no `stateful mcp serve` in agent configs, no stateful MCP guidance in skills.
- Delete runtime current-session files: `.stateful_core/runtime/session.json` and `.stateful_core/runtime/sessions/<id>.json` are no longer read or written.
- Keep `.stateful_core/runtime/server.json` for server discovery.
- Prefer deletion over compatibility aliases. Legacy `session_id` inputs should fail tests unless a task explicitly scopes an internal migration helper.
- Do not add dependencies.
- Use targeted tests per task; do not run project-wide suites until all tasks land.

---

## File Structure

### Delete or stop exposing

- `crates/stateful-mcp/`: remove if no non-agent internal compile dependency remains after Task 5.
- `crates/stateful-cli/src/mcp.rs`: delete with CLI command removal.
- Installed Codex/OMP MCP config generation in `crates/stateful-cli/src/install.rs`.
- MCP-specific benchmark config in `crates/stateful-bench/scripts/codex_pair_agent.py` and `crates/stateful-bench/scripts/denovo_codex_agent.py`.

### Modify

- `crates/stateful-cli/src/runtime.rs`: `CurrentSession` -> `AgentContext`; remove current-session file helpers and env identity lookup.
- `crates/stateful-cli/src/hook.rs`: hook inputs/output bodies use `agent_id`; missing `agent_id` denies.
- `crates/stateful-cli/src/install.rs`: generated OMP extension detects/injects `agent_id`; installed config removes MCP.
- `crates/stateful-cli/src/lib.rs`: remove `McpCommand`; rename CLI state args to `agent_id` where CLI/admin commands remain.
- `crates/stateful-cli/src/sandbox.rs`: build scratch paths and release bodies use `agent_id`; no session recovery.
- `crates/stateful-cli/src/outbox.rs`: queued records use `agent_id` as file stem.
- `crates/stateful-cli/src/commit.rs`: request field becomes `agent_id`.
- `crates/stateful-store/src/lib.rs`: schema/API/current-state fields use `agent_id`, `blocking_agent_id`, `target_agent_id`.
- `crates/stateful-bench/**`: benchmark conditions no longer advertise MCP; trace/report fields rename to `agent_id` where they refer to Stateful owner identity.
- `docs/**`, `README.md`, skill templates embedded in `install.rs`: delete MCP guidance and session-file/env guidance.

### Test targets

- `cargo test -p stateful-cli --test cli <test-name> -- --exact`
- `cargo test -p stateful-cli <unit-test-name> -- --exact`
- `cargo test -p stateful-store <unit-test-name> -- --exact`
- `cargo test -p stateful-bench --test cli <test-name> -- --exact`
- `cargo test -p stateful-bench <unit-test-name> -- --exact`

---

### Task 1: Delete installed MCP config paths

**Files:**
- Modify: `crates/stateful-cli/src/install.rs`
- Modify: `crates/stateful-cli/tests/cli.rs`
- Modify: `crates/stateful-bench/scripts/codex_pair_agent.py`
- Modify: `crates/stateful-bench/tests/cli.rs`
- Modify: `crates/stateful-bench/tests/denovo_source_guard.rs`

**Interfaces:**
- Consumes: existing install config generation.
- Produces: agent installs with hooks/extension/skills only; no MCP config or MCP server block.

- [ ] **Step 1: Write failing install tests**

Update CLI/install assertions that currently expect `[mcp_servers.stateful]` and OMP `mcp.json`.

Expected assertions:

```rust
assert!(!config.contains("[mcp_servers.stateful]"));
assert!(!config.contains("args = [\"mcp\", \"serve\"]"));
assert!(!config.contains("STATEFUL_SESSION_ID"));
assert!(!config.contains("CODEX_THREAD_ID"));
assert!(!config.contains("STATEFUL_CODEX_RUN_ID"));
```

For OMP install plan tests, assert generated file list does not include `mcp.json`.

- [ ] **Step 2: Run targeted failing tests**

Run:

```bash
cargo test -p stateful-cli --test cli install -- --nocapture
cargo test -p stateful-bench --test cli denovo_adapter_installs -- --nocapture
```

Expected: FAIL on old MCP config still present.

- [ ] **Step 3: Remove installed MCP generation**

In `install.rs`:

- Remove `mcp_path` from plan file lists.
- Remove parent directory creation for `mcp_path`.
- Remove `write_omp_mcp_config` call and function.
- Remove global Codex `[mcp_servers.stateful]` block from `global_codex_config_block`.
- Remove MCP env vars from installed config.
- Keep hook install and extension install.

In Python benchmark config builders:

```python
STATEFUL_INTEGRATION_FULL = "hooks-skill"
```

or delete the `FULL` vs `HOOKS_ONLY` distinction if it only controlled MCP. Keep the shortest mapping that preserves benchmark axes.

- [ ] **Step 4: Run targeted tests**

Run the same commands as Step 2. Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/stateful-cli/src/install.rs crates/stateful-cli/tests/cli.rs crates/stateful-bench/scripts/codex_pair_agent.py crates/stateful-bench/tests/cli.rs crates/stateful-bench/tests/denovo_source_guard.rs
git commit -m "remove installed stateful mcp config"
```

---

### Task 2: Add strict OMP agent_id injection

**Files:**
- Modify: `crates/stateful-cli/src/install.rs`
- Modify: `crates/stateful-cli/src/hook.rs`
- Modify: `crates/stateful-cli/tests/cli.rs`

**Interfaces:**
- Consumes: Task 1's MCP-free extension install.
- Produces: generated OMP extension sends `agent_id` in every Stateful hook payload; missing id denies.

- [ ] **Step 1: Write failing generated-extension tests**

Add tests around generated extension text or existing fixture helper:

```rust
assert!(extension.contains("function agentIdFromString"));
assert!(extension.contains("function detectAgentId"));
assert!(extension.contains("agent_id"));
assert!(!extension.contains("process.env.STATEFUL_SESSION_ID"));
assert!(!extension.contains("sessionIdFromSessionManager"));
```

Add a hook parsing test that rejects legacy input:

```rust
let input = serde_json::json!({
    "session_id": "old-session",
    "workspace_id": "workspace",
    "cwd": repo.display().to_string(),
});
let error = handle_omp_session_start_with_runtime(&input.to_string(), &runtime)
    .expect_err("legacy session_id should not be accepted");
assert!(error.to_string().contains("agent_id"));
```

- [ ] **Step 2: Run targeted failing tests**

Run:

```bash
cargo test -p stateful-cli --test cli omp_extension -- --nocapture
cargo test -p stateful-cli hook:: -- --nocapture
```

Expected: FAIL until extension/hook structs use `agent_id`.

- [ ] **Step 3: Implement strict JS identity detection**

In generated extension JS, replace session detection with:

```js
function agentIdFromString(value) {
  if (typeof value !== "string") return undefined;
  const id = value.trim();
  if (!id) return undefined;
  if (!/^[A-Za-z0-9_-]+$/.test(id)) return undefined;
  return id;
}

function detectAgentId(event, ctx) {
  return firstString(
    agentIdFromString(event?.agentId),
    agentIdFromString(event?.agent_id),
    agentIdFromString(event?.agent?.id),
    agentIdFromString(event?.agent?.agentId),
    agentIdFromString(event?.agent?.agent_id),
    agentIdFromString(ctx?.agentId),
    agentIdFromString(ctx?.agent_id),
    agentIdFromString(ctx?.agent?.id),
    agentIdFromString(ctx?.agent?.agentId),
    agentIdFromString(ctx?.agent?.agent_id)
  );
}

function agentId(event, ctx) {
  const id = detectAgentId(event, ctx);
  if (!id) throw new Error("Stateful requires adapter-provided agent_id for the active agent");
  return id;
}
```

Do not use session fields or sessionManager as fallback.

- [ ] **Step 4: Inject agent_id into all hook calls**

Every generated `runStatefulHook(...)` payload gets:

```js
agent_id: agentId(event, ctx)
```

and stops sending `session_id`.

- [ ] **Step 5: Rename OMP hook input structs**

In `hook.rs`, replace input field names:

```rust
pub agent_id: String,
```

Add explicit validation:

```rust
validate_agent_id(&input.agent_id, "agent_id")?;
```

Missing or invalid `agent_id` returns a blocking/failed hook response with the strict error message.

- [ ] **Step 6: Run tests**

Run the Step 2 commands. Expected: PASS.

- [ ] **Step 7: Commit**

```bash
git add crates/stateful-cli/src/install.rs crates/stateful-cli/src/hook.rs crates/stateful-cli/tests/cli.rs
git commit -m "inject strict omp agent ids"
```

---

### Task 3: Replace runtime CurrentSession with AgentContext and delete session files

**Files:**
- Modify: `crates/stateful-cli/src/runtime.rs`
- Modify: `crates/stateful-cli/src/sandbox.rs`
- Modify: `crates/stateful-cli/src/codex_benchmark.rs`
- Modify: `crates/stateful-cli/src/outbox.rs`
- Modify: `crates/stateful-cli/src/commit.rs`
- Modify: `crates/stateful-cli/tests/cli.rs`

**Interfaces:**
- Consumes: Task 2 hook payloads with `agent_id`.
- Produces: runtime helper APIs require explicit `AgentContext`; no env or file current-session lookup remains.

- [ ] **Step 1: Write failing runtime tests**

Add or update tests to assert:

```rust
assert!(!runtime_source.contains("STATEFUL_SESSION_ID"));
assert!(!runtime_source.contains("CODEX_THREAD_ID"));
assert!(!runtime_source.contains("STATEFUL_CODEX_RUN_ID"));
assert!(!runtime_source.contains("runtime/session.json"));
assert!(!runtime_source.contains("runtime/sessions"));
```

Prefer direct Rust tests for public behavior where possible:

```rust
assert!(validate_agent_id("agent-1", "agent_id").is_ok());
assert!(validate_agent_id("", "agent_id").is_err());
assert!(validate_agent_id("agent/id", "agent_id").is_err());
```

- [ ] **Step 2: Run failing tests**

Run:

```bash
cargo test -p stateful-cli runtime -- --nocapture
cargo test -p stateful-cli --test cli stateful_session -- --nocapture
```

Expected: FAIL while old session helpers exist.

- [ ] **Step 3: Replace type and args**

In `runtime.rs`:

```rust
pub struct AgentContext {
    pub agent_id: String,
    pub workspace_id: String,
}

impl AgentContext {
    pub fn new(agent_id: impl Into<String>, workspace_id: impl Into<String>) -> Self {
        Self { agent_id: agent_id.into(), workspace_id: workspace_id.into() }
    }
}
```

Rename request arg fields:

```rust
pub struct ReservationDeclareArgs { pub agent_id: String, ... }
pub struct ReservationClaimArgs { pub agent_id: String, ... }
pub struct ReservationRequestArgs { pub agent_id: String, ... }
pub struct ReservationCancelArgs { pub agent_id: String, ... }
```

- [ ] **Step 4: Delete session-file/env helpers**

Remove:

```rust
STATEFUL_SESSION_ID_ENV
CODEX_THREAD_ID_ENV
STATEFUL_CODEX_RUN_ID_ENV
current_session_file_path
current_session_file_path_for_session
current_stateful_session_id
current_env_session_id
ensure_current_session_matches_env_session
read_current_session_file*
write_current_session_file*
```

Keep only `runtime_file_path(...)/server.json` for server discovery.

- [ ] **Step 5: Update callsites**

Replace `current_session.session_id` with `agent_context.agent_id` where the context is still valid. For CLI/sandbox commands that previously recovered session from env/session file, fail with the existing missing-reservation/claim error unless the caller passed explicit admin CLI `--agent-id`.

- [ ] **Step 6: Update protocol envelope**

Change:

```json
"session": { "session_id": "..." }
```

to:

```json
"agent": {
  "agent_id": "...",
  "actor_id": "...",
  "actor_type": "agent"
}
```

Use `actor_id == agent_id` for agent-origin events unless the source is a local admin CLI, where `actor_id` can remain `stateful-cli:<pid>` metadata.

- [ ] **Step 7: Run tests**

Run the Step 2 commands. Expected: PASS.

- [ ] **Step 8: Commit**

```bash
git add crates/stateful-cli/src/runtime.rs crates/stateful-cli/src/sandbox.rs crates/stateful-cli/src/codex_benchmark.rs crates/stateful-cli/src/outbox.rs crates/stateful-cli/src/commit.rs crates/stateful-cli/tests/cli.rs
git commit -m "replace current sessions with agent context"
```

---

### Task 4: Rename store/protocol ownership fields to agent_id

**Files:**
- Modify: `crates/stateful-store/src/lib.rs`
- Modify: `crates/stateful-core/src/*.rs` as needed for protocol structs
- Modify: `crates/stateful-cli/src/hook.rs`
- Modify: `crates/stateful-cli/src/lib.rs`
- Modify: `crates/stateful-cli/src/runtime.rs`

**Interfaces:**
- Consumes: Task 3 `AgentContext` and `agent_id` request args.
- Produces: database/current-state/events/reservations/claims/notifications use `agent_id` names consistently.

- [ ] **Step 1: Write failing store tests**

Update tests to expect `agent_id` fields and reject old names:

```rust
let item = store.current_state(...).items[0].clone();
assert_eq!(item.agent_id.as_deref(), Some("agent-a"));
assert!(!serde_json::to_value(&item).unwrap().to_string().contains("session_id"));
```

For wait queues:

```rust
assert_eq!(event.payload["blocking_agent_id"], "agent-a");
assert!(event.payload.get("blocking_session_id").is_none());
```

- [ ] **Step 2: Run failing store tests**

Run:

```bash
cargo test -p stateful-store current_state -- --nocapture
cargo test -p stateful-store reservation -- --nocapture
```

Expected: FAIL while schema/API still exposes session names.

- [ ] **Step 3: Rename schema fields**

Apply a clean migration in schema initialization:

```sql
agent_id TEXT NOT NULL
blocking_agent_id TEXT
target_agent_id TEXT
```

Rename Rust record fields and SQL column references. If table names include `sessions`, either rename to `agents` or delete the table if all callers can read from events/reservations directly. Prefer deletion if no invariant requires the table.

- [ ] **Step 4: Update event constructors**

Rename constructor parameters and payload keys:

```rust
pub fn agent_registered(agent_id: impl Into<String>, workspace_id: impl Into<String>) -> Self
pub fn agent_heartbeat(agent_id: impl Into<String>, workspace_id: impl Into<String>) -> Self
pub fn reservation_declared(agent_id: impl Into<String>, workspace_id: impl Into<String>, ...)
```

Payloads use `agent_id`, not `session_id`.

- [ ] **Step 5: Update current-state language**

Current-state messages say:

```text
Agent {agent_id} declared reservation for {resource}.
Avoid overlapping edits to {resource} unless coordinating with {agent_id}.
```

No `Session ...` wording remains for ownership.

- [ ] **Step 6: Run tests**

Run the Step 2 commands. Expected: PASS.

- [ ] **Step 7: Commit**

```bash
git add crates/stateful-store/src/lib.rs crates/stateful-core/src crates/stateful-cli/src/hook.rs crates/stateful-cli/src/lib.rs crates/stateful-cli/src/runtime.rs
git commit -m "rename state ownership to agent ids"
```

---

### Task 5: Remove MCP CLI/crate/server surface

**Files:**
- Modify: `crates/stateful-cli/src/lib.rs`
- Remove: `crates/stateful-cli/src/mcp.rs`
- Modify: `crates/stateful-cli/Cargo.toml`
- Remove: `crates/stateful-mcp/`
- Modify: root `Cargo.toml`
- Modify: tests referencing `McpCommand` or `stateful mcp`

**Interfaces:**
- Consumes: Task 1 installed MCP removal and Task 3 no current-session runtime.
- Produces: `stateful mcp ...` is no longer a CLI command and MCP crate is gone if unused.

- [ ] **Step 1: Write failing CLI parse tests**

Replace old positive MCP parse tests with a negative test:

```rust
let err = Cli::try_parse_from(["stateful", "mcp", "serve"])
    .expect_err("mcp command should be removed");
assert!(err.to_string().contains("unrecognized subcommand"));
```

- [ ] **Step 2: Run failing tests**

Run:

```bash
cargo test -p stateful-cli --test cli mcp -- --nocapture
```

Expected: FAIL while command exists.

- [ ] **Step 3: Delete MCP command and exports**

In `lib.rs`:

- remove `mod mcp;`
- remove `pub use mcp::{...};`
- remove `McpCommand` from imports/exports/tests
- remove `Command::Mcp` enum variant and match arms

In manifests:

- remove `stateful-mcp` dependency from `stateful-cli`
- remove `crates/stateful-mcp` from workspace members
- delete `crates/stateful-mcp`

- [ ] **Step 4: Remove code paths made dead by MCP deletion**

Delete helpers that only existed for MCP current-session repair. If a helper has exactly one remaining callsite and that callsite can inline one line, inline it.

- [ ] **Step 5: Run tests**

Run the Step 2 command. Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add Cargo.toml crates/stateful-cli/Cargo.toml crates/stateful-cli/src/lib.rs crates/stateful-cli/tests/cli.rs
git rm crates/stateful-cli/src/mcp.rs crates/stateful-mcp -r
git commit -m "delete stateful mcp command surface"
```

---

### Task 6: Update benchmark adapters and reports

**Files:**
- Modify: `crates/stateful-bench/scripts/denovo_codex_agent.py`
- Modify: `crates/stateful-bench/scripts/codex_pair_agent.py`
- Modify: `crates/stateful-bench/src/lib.rs`
- Modify: `crates/stateful-bench/tests/cli.rs`
- Modify: `crates/stateful-bench/tests/programbench.rs`
- Modify: `crates/stateful-bench/tests/run.rs`

**Interfaces:**
- Consumes: Task 5 no MCP command and Task 4 `agent_id` protocol.
- Produces: benchmark modes do not claim MCP support; trace/report fields distinguish benchmark `agent_id` from Stateful owner `agent_id` where necessary.

- [ ] **Step 1: Write failing benchmark tests**

Update assertions:

```rust
assert!(!config.contains("[mcp_servers.stateful]"));
assert!(!config.contains("STATEFUL_SESSION_ID"));
assert!(!config.contains("STATEFUL_CODEX_RUN_ID"));
```

ProgramBench env isolation should still remove server token/url, but no longer mention removed session env vars.

- [ ] **Step 2: Run failing tests**

Run:

```bash
cargo test -p stateful-bench --test cli denovo -- --nocapture
cargo test -p stateful-bench --test programbench stateful_env -- --nocapture
cargo test -p stateful-bench --test run mcp -- --nocapture
```

Expected: FAIL while old benchmark MCP/session env assumptions remain.

- [ ] **Step 3: Simplify integration modes**

In Python scripts, remove `hooks-mcp-skill`. Use:

```python
STATEFUL_INTEGRATION_FULL = "hooks-skill"
STATEFUL_INTEGRATION_HOOKS_ONLY = "hooks-only"
STATEFUL_INTEGRATION_NONE = "none"
```

If `FULL` and `HOOKS_ONLY` become identical, delete one axis and update callers/tests.

- [ ] **Step 4: Rename Stateful owner fields**

Where a field means Stateful owner identity, use `stateful_agent_id` or `agent_id` consistently. If a benchmark already has an instance-level `agent_id`, do not overload it silently; use `stateful_agent_id` in trace metadata.

- [ ] **Step 5: Delete MCP-denial heuristics**

Remove setup-error patterns specific to `user cancelled mcp tool call` unless they still apply to external non-Stateful MCP failures. The benchmark should not classify missing Stateful MCP as a normal axis anymore.

- [ ] **Step 6: Run tests**

Run the Step 2 commands. Expected: PASS.

- [ ] **Step 7: Commit**

```bash
git add crates/stateful-bench/scripts/denovo_codex_agent.py crates/stateful-bench/scripts/codex_pair_agent.py crates/stateful-bench/src/lib.rs crates/stateful-bench/tests/cli.rs crates/stateful-bench/tests/programbench.rs crates/stateful-bench/tests/run.rs
git commit -m "remove benchmark mcp integration mode"
```

---

### Task 7: Update docs, skills, and generated guidance

**Files:**
- Modify: `README.md`
- Modify: `docs/architecture.md`
- Modify: `docs/state-model.md`
- Modify: `docs/coordination.md`
- Modify: `docs/stateful-command-policy.md` if present
- Modify: embedded skill text in `crates/stateful-cli/src/install.rs`
- Modify: any `SKILL.md` files installed/generated from repo templates

**Interfaces:**
- Consumes: Tasks 1-6 behavior.
- Produces: no user/agent-facing docs instruct MCP use, session env, or runtime session files.

- [ ] **Step 1: Search documentation for removed terms**

Use repository search for:

```text
state.session
mcp_servers.stateful
STATEFUL_SESSION_ID
CODEX_THREAD_ID
STATEFUL_CODEX_RUN_ID
runtime/session.json
runtime/sessions
session_id
```

Every remaining `session_id` must either be historical/changelog text or changed to `agent_id`.

- [ ] **Step 2: Edit docs**

Replace guidance with:

```text
OMP Stateful coordination is extension-native. The extension injects the active agent_id into Stateful hook and state-tool calls. If no adapter-provided agent_id is available, Stateful denies instead of falling back to session files, environment variables, or MCP.
```

Runtime state section should list:

```text
.stateful_core/runtime/server.json
.stateful_core/state.db
.stateful_core/outbox/*.jsonl
```

and explicitly not list current-session files.

- [ ] **Step 3: Run doc conflict search**

Search again for removed terms. Expected: no active guidance remains.

- [ ] **Step 4: Commit**

```bash
git add README.md docs crates/stateful-cli/src/install.rs
git commit -m "document extension native agent identity"
```

---

### Task 8: Final verification and cleanup

**Files:**
- Modify only if verification finds stale references.

**Interfaces:**
- Consumes: all prior tasks.
- Produces: clean branch with no MCP/session-file fallback and passing targeted verification.

- [ ] **Step 1: Workspace search gate**

Search source/docs/tests for:

```text
stateful-mcp
state.session
mcp_servers.stateful
STATEFUL_SESSION_ID
CODEX_THREAD_ID
STATEFUL_CODEX_RUN_ID
runtime/session.json
runtime/sessions
stateful-mcp:<pid>
```

Expected: no active code/guidance. Historical release notes may remain only if clearly historical.

- [ ] **Step 2: Agent id search gate**

Search source/docs/tests for `session_id`. For each remaining hit, decide one of:

- rename to `agent_id`
- keep because it refers to non-Stateful external benchmark data and add a clarifying name
- delete because it was compatibility glue

- [ ] **Step 3: Targeted test suite**

Run:

```bash
cargo test -p stateful-cli --test cli
cargo test -p stateful-cli
cargo test -p stateful-store
cargo test -p stateful-bench --test cli
cargo test -p stateful-bench --test programbench
cargo test -p stateful-bench --test run
```

Expected: PASS.

- [ ] **Step 4: Build check**

Run:

```bash
cargo check --workspace
```

Expected: PASS.

- [ ] **Step 5: Commit cleanup**

If stale-reference cleanup was needed:

```bash
git add <changed files>
git commit -m "clean up stale session and mcp references"
```

---

## Self-Review

- Spec coverage: strict active `agent_id`, no session/env fallback, MCP deletion, runtime file deletion, store ownership rename, docs/tests covered.
- Placeholder scan: no TBD/TODO/fill-later steps.
- Type consistency: plan uses `AgentContext`, `agent_id`, `blocking_agent_id`, and `target_agent_id` consistently.
- Scope risk: this is a breaking clean cutover. That is intentional per maintainer approval: MCP deleted, no compatibility shims.
