# OMP LSP MCP Resources Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add `lsp` to the default OMP repo tool allowlist and expose read-only Stateful MCP resources for `stateful/current`, `stateful/events`, and `stateful/context`.

**Architecture:** Keep the allowlist change in `repo_registry.rs`. Keep MCP resources in the existing JSON-RPC handler in `crates/stateful-cli/src/mcp.rs`, reusing `call_mcp_tool_in_repo` so resource reads share existing session, repo gate, runtime discovery, and HTTP forwarding behavior.

**Tech Stack:** Rust, serde_json, existing stateful-cli MCP JSON-RPC helpers, cargo test.

## Global Constraints

- Use the stateful MCP tools to declare reservations and acquire same-session claims before editing files.
- Use `sandbox_bash` with `fs: build` for cargo tests; do not use raw Bash.
- Write failing tests before implementation code.
- No new dependencies.
- No write-capable MCP resources.
- No MCP resource templates.
- No new OMP config settings.

---

## File Structure

- Modify `crates/stateful-cli/src/repo_registry.rs`: add `lsp` to `DEFAULT_OMP_ALLOWED_TOOLS`.
- Modify `crates/stateful-cli/tests/cli.rs`: update the allowed tools assertion so the test fails before the allowlist change.
- Modify `crates/stateful-cli/src/mcp.rs`: advertise `resources`, list fixed read-only resources, and read resources by forwarding to existing Stateful MCP tool paths.
- Modify `crates/stateful-cli/tests/mcp.rs`: add JSON-RPC tests for initialize capabilities, resources/list, resources/read, and unknown resource errors.

---

### Task 1: Add `lsp` to OMP default allowlist

**Files:**
- Modify: `crates/stateful-cli/tests/cli.rs:723-736`
- Modify: `crates/stateful-cli/src/repo_registry.rs:12-22`

**Interfaces:**
- Consumes: existing `enable_repo`, `tools list`, and default allowlist merge behavior.
- Produces: `DEFAULT_OMP_ALLOWED_TOOLS` includes `"lsp"`; new repos list `lsp` by default.

- [ ] **Step 1: Reserve and claim files**

```text
state_reservation_declare(
  purpose = "Add lsp to OMP default tools allowlist.",
  files_planned = ["crates/stateful-cli/tests/cli.rs", "crates/stateful-cli/src/repo_registry.rs"]
)
state_claim_acquire(paths = ["crates/stateful-cli/tests/cli.rs", "crates/stateful-cli/src/repo_registry.rs"])
```

- [ ] **Step 2: Write the failing test**

In `crates/stateful-cli/tests/cli.rs`, update `tools_list_prints_allowed_and_unclassified_tools`:

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
            "KnownTool"
        ])
    );
```

- [ ] **Step 3: Run test to verify it fails**

```bash
cargo test -p stateful-cli --test cli tools_list_prints_allowed_and_unclassified_tools -- --exact
```

Expected: FAIL because `json["allowed_tools"]` is missing `"lsp"`.

- [ ] **Step 4: Write minimal implementation**

In `crates/stateful-cli/src/repo_registry.rs`:

```rust
const DEFAULT_OMP_ALLOWED_TOOLS: &[&str] = &["task", "yield", "lsp"];
```

- [ ] **Step 5: Run test to verify it passes**

```bash
cargo test -p stateful-cli --test cli tools_list_prints_allowed_and_unclassified_tools -- --exact
```

Expected: PASS.

- [ ] **Step 6: Commit task**

```bash
git add crates/stateful-cli/tests/cli.rs crates/stateful-cli/src/repo_registry.rs
git commit -m "Allow OMP lsp tool by default"
```

---

### Task 2: Expose read-only Stateful MCP resources

**Files:**
- Modify: `crates/stateful-cli/tests/mcp.rs:1428-1496`
- Modify: `crates/stateful-cli/src/mcp.rs:546-611`

**Interfaces:**
- Consumes: `call_mcp_tool_in_repo(repo_root, name, arguments) -> anyhow::Result<HttpResponse>`.
- Produces: MCP JSON-RPC methods `resources/list` and `resources/read`; initialize capability `resources`; resource URIs `stateful/current`, `stateful/events`, `stateful/context`.

- [ ] **Step 1: Reserve and claim files**

```text
state_reservation_declare(
  purpose = "Expose read-only Stateful MCP resources.",
  files_planned = ["crates/stateful-cli/tests/mcp.rs", "crates/stateful-cli/src/mcp.rs"]
)
state_claim_acquire(paths = ["crates/stateful-cli/tests/mcp.rs", "crates/stateful-cli/src/mcp.rs"])
```

- [ ] **Step 2: Write failing tests**

Add these tests near `mcp_tools_list_returns_stateful_tool_descriptors` in `crates/stateful-cli/tests/mcp.rs`:

```rust
#[test]
fn mcp_initialize_advertises_resources_capability() {
    let temp_root = temp_root("stateful-mcp-initialize-resources");
    fs::create_dir_all(&temp_root).expect("temp root should be creatable");

    let response = handle_mcp_jsonrpc_in_repo(
        &temp_root,
        r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}"#,
    )
    .expect("initialize should handle")
    .expect("initialize should produce response");
    let json: serde_json::Value = serde_json::from_str(&response).expect("response should be json");

    assert_eq!(json["result"]["capabilities"]["resources"], serde_json::json!({}));

    fs::remove_dir_all(&temp_root).expect("temp root should be removable");
}

#[test]
fn mcp_resources_list_returns_stateful_read_resources() {
    let temp_root = temp_root("stateful-mcp-resources-list");
    fs::create_dir_all(&temp_root).expect("temp root should be creatable");

    let response = handle_mcp_jsonrpc_in_repo(
        &temp_root,
        r#"{"jsonrpc":"2.0","id":2,"method":"resources/list","params":{}}"#,
    )
    .expect("resources/list should handle")
    .expect("resources/list should produce response");
    let json: serde_json::Value = serde_json::from_str(&response).expect("response should be json");
    let resources = json["result"]["resources"].as_array().expect("resources should be array");

    assert!(resources.iter().any(|resource| resource["uri"] == "stateful/current" && resource["name"] == "current" && resource["mimeType"] == "application/json"));
    assert!(resources.iter().any(|resource| resource["uri"] == "stateful/events" && resource["name"] == "events" && resource["mimeType"] == "application/json"));
    assert!(resources.iter().any(|resource| resource["uri"] == "stateful/context" && resource["name"] == "context" && resource["mimeType"] == "text/plain"));

    fs::remove_dir_all(&temp_root).expect("temp root should be removable");
}

#[test]
fn mcp_resources_read_current_forwards_to_current_endpoint() {
    let temp_root = temp_root("stateful-mcp-resources-read-current");
    let paths = GlobalPaths::new(temp_root.join("home"));
    let repo_root = temp_root.join("repo");
    fs::create_dir_all(&repo_root).expect("repo root should be creatable");
    enable_test_repo(&paths, &repo_root);
    let (runtime, rx) = spawn_fake_stateful_server(r#"{"status":"ok","current":{"active_reservation_count":0}}"#);
    write_global_runtime_file(&paths, &runtime).expect("global runtime file should write");

    let response = run_mcp_jsonrpc_in_repo(
        &repo_root,
        &paths,
        r#"{"jsonrpc":"2.0","id":3,"method":"resources/read","params":{"uri":"stateful/current"}}"#,
    );

    let request = rx.recv_timeout(Duration::from_secs(1)).expect("current request should arrive");
    assert!(request.contains("GET /v1/current HTTP/1.1"));
    let json: serde_json::Value = serde_json::from_str(&response).expect("response should be json");
    assert_eq!(json["result"]["contents"][0]["uri"], "stateful/current");
    assert_eq!(json["result"]["contents"][0]["mimeType"], "application/json");
    assert!(json["result"]["contents"][0]["text"].as_str().unwrap_or_default().contains("active_reservation_count"));

    fs::remove_dir_all(&temp_root).expect("temp root should be removable");
}

#[test]
fn mcp_resources_read_rejects_unknown_resource_uri() {
    let temp_root = temp_root("stateful-mcp-resources-read-unknown");
    fs::create_dir_all(&temp_root).expect("temp root should be creatable");

    let response = handle_mcp_jsonrpc_in_repo(
        &temp_root,
        r#"{"jsonrpc":"2.0","id":4,"method":"resources/read","params":{"uri":"stateful/missing"}}"#,
    )
    .expect("resources/read should handle")
    .expect("resources/read should produce response");
    let json: serde_json::Value = serde_json::from_str(&response).expect("response should be json");

    assert_eq!(json["error"]["code"], -32602);
    assert!(json["error"]["message"].as_str().unwrap_or_default().contains("unknown Stateful MCP resource URI"));

    fs::remove_dir_all(&temp_root).expect("temp root should be removable");
}
```

- [ ] **Step 3: Run tests to verify they fail**

```bash
cargo test -p stateful-cli --test mcp mcp_initialize_advertises_resources_capability -- --exact
cargo test -p stateful-cli --test mcp mcp_resources_list_returns_stateful_read_resources -- --exact
cargo test -p stateful-cli --test mcp mcp_resources_read_current_forwards_to_current_endpoint -- --exact
cargo test -p stateful-cli --test mcp mcp_resources_read_rejects_unknown_resource_uri -- --exact
```

Expected: FAIL because `resources` capability and resource methods do not exist.

- [ ] **Step 4: Add resource descriptors and reader helper**

In `crates/stateful-cli/src/mcp.rs`, add above `handle_mcp_jsonrpc_in_repo`:

```rust
fn stateful_resource_descriptors() -> Vec<Value> {
    vec![
        serde_json::json!({"uri":"stateful/current","name":"current","description":"Materialized Stateful current-state summary.","mimeType":"application/json"}),
        serde_json::json!({"uri":"stateful/events","name":"events","description":"Recent Stateful audit events.","mimeType":"application/json"}),
        serde_json::json!({"uri":"stateful/context","name":"context","description":"Brief Stateful current-state context for an agent prompt.","mimeType":"text/plain"}),
    ]
}

fn read_stateful_resource(repo_root: &Path, uri: &str) -> anyhow::Result<Result<Value, String>> {
    let (tool_name, arguments, mime_type) = match uri {
        "stateful/current" => ("state_current_read", serde_json::json!({}), "application/json"),
        "stateful/events" => ("state_events_read", serde_json::json!({}), "application/json"),
        "stateful/context" => ("state_context_render", serde_json::json!({ "mode": "brief" }), "text/plain"),
        unknown => return Ok(Err(format!("unknown Stateful MCP resource URI: {unknown}"))),
    };
    let response = call_mcp_tool_in_repo(repo_root, tool_name, arguments)?;
    Ok(Ok(serde_json::json!({"contents":[{"uri":uri,"mimeType":mime_type,"text":response.body}]})))
}
```

- [ ] **Step 5: Advertise resource capability and methods**

In `handle_mcp_jsonrpc_in_repo`, change capabilities to:

```rust
"capabilities": {
    "tools": {},
    "resources": {}
},
```

Add match arms before `"tools/call"`:

```rust
        "resources/list" => jsonrpc_result(
            id,
            serde_json::json!({"resources": stateful_resource_descriptors()}),
        ),
        "resources/read" => {
            let uri = request
                .get("params")
                .and_then(|params| params.get("uri"))
                .and_then(Value::as_str)
                .ok_or_else(|| anyhow::anyhow!("MCP resources/read missing params.uri"))?;
            match read_stateful_resource(repo_root, uri)? {
                Ok(result) => jsonrpc_result(id, result),
                Err(message) => jsonrpc_error(id, -32602, message),
            }
        }
```

- [ ] **Step 6: Run resource tests to verify they pass**

```bash
cargo test -p stateful-cli --test mcp mcp_initialize_advertises_resources_capability -- --exact
cargo test -p stateful-cli --test mcp mcp_resources_list_returns_stateful_read_resources -- --exact
cargo test -p stateful-cli --test mcp mcp_resources_read_current_forwards_to_current_endpoint -- --exact
cargo test -p stateful-cli --test mcp mcp_resources_read_rejects_unknown_resource_uri -- --exact
```

Expected: PASS.

- [ ] **Step 7: Commit task**

```bash
git add crates/stateful-cli/tests/mcp.rs crates/stateful-cli/src/mcp.rs
git commit -m "Expose Stateful MCP read resources"
```

---

### Task 3: Final targeted verification

**Files:**
- Test only; no source edits expected.

**Interfaces:**
- Consumes: completed Task 1 and Task 2 behavior.
- Produces: verification evidence that the changed test targets pass together.

- [ ] **Step 1: Run changed tests together**

```bash
cargo test -p stateful-cli --test cli tools_list_prints_allowed_and_unclassified_tools -- --exact
cargo test -p stateful-cli --test mcp mcp_initialize_advertises_resources_capability -- --exact
cargo test -p stateful-cli --test mcp mcp_resources_list_returns_stateful_read_resources -- --exact
cargo test -p stateful-cli --test mcp mcp_resources_read_current_forwards_to_current_endpoint -- --exact
cargo test -p stateful-cli --test mcp mcp_resources_read_rejects_unknown_resource_uri -- --exact
```

Expected: all PASS.

- [ ] **Step 2: Check docs and skills impact**

No README, skill, or usage-reference text changes are required unless implementation behavior differs from this plan. If behavior differs, update the exact affected doc file in the same TDD/verification flow.

- [ ] **Step 3: Push commits**

```bash
git push origin HEAD
```

Expected: push succeeds to the current branch.

---

## Self-Review

- Spec coverage: Task 1 covers `lsp`; Task 2 covers `initialize`, `resources/list`, `resources/read`, the three resource URIs, and unknown URI errors; Task 3 covers targeted verification.
- Placeholder scan: no TBD/TODO/fill-later steps remain.
- Type consistency: helper names and JSON field names match their later use: `stateful_resource_descriptors`, `read_stateful_resource`, `contents`, `uri`, `mimeType`, `text`.
