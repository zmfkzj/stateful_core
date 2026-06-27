# process_find Fields Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Return all safe `ps` metadata from `process_find` by default and allow callers to select a smaller safe field set.

**Architecture:** Keep process discovery in `crates/stateful-cli/src/sandbox.rs`. `ps` still includes `command=` only for internal `contains` filtering; output serialization uses a safe field allowlist. CLI and OMP pass a repeated `field`/`fields` selector into `SandboxProcessFindRequest`.

**Tech Stack:** Rust, clap, serde/serde_json, generated OMP TypeScript extension embedded in Rust strings.

## Global Constraints

- Do not expose `command`, full argv, or environment variables in result JSON.
- Keep `contains` matching against the internal command line.
- Default output returns every safe field.
- Field selectors reject unknown and forbidden fields.
- No new dependencies.

---

## File Structure

- Modify `crates/stateful-cli/src/sandbox.rs`: request field selector, safe process fields, parser, validation, selected serialization, unit tests.
- Modify `crates/stateful-cli/src/lib.rs`: add CLI `--field`, pass fields to sandbox request.
- Modify `crates/stateful-cli/src/install.rs`: add OMP `fields` schema and argument mapping.
- Modify `docs/usage-reference.md`: document `fields` and default safe output.
- Modify `crates/stateful-cli/assets/stateful-command-policy/SKILL.md` and `README.md` only if their existing `process_find` summary conflicts after the implementation.

---

### Task 1: Rust process output model and CLI selector

**Files:**
- Modify: `crates/stateful-cli/src/sandbox.rs:79-145`, `crates/stateful-cli/src/sandbox.rs:740-792`, `crates/stateful-cli/src/sandbox.rs:856-895`, `crates/stateful-cli/src/sandbox.rs:2458-2526`, `crates/stateful-cli/src/sandbox.rs:4217-4280`
- Modify: `crates/stateful-cli/src/lib.rs:249-263`, `crates/stateful-cli/src/lib.rs:666-683`

**Interfaces:**
- Consumes: existing `SandboxProcessFindRequest`, `SandboxProcessFindOutput`, `parse_sandbox_process_find_bash_invocation`, `run_sandbox_process_find`.
- Produces:
  - `SandboxProcessFindRequest { ..., fields: Vec<String> }`
  - `SandboxProcessInfo` with safe fields: `pid`, `ppid`, `pgid`, `user`, `uid`, `stat`, `start`, `etime`, `time`, `pcpu`, `pmem`, `rss`, `vsz`, `nice`, `pri`, `tty`, `comm`
  - selected output JSON where `processes` entries contain either all safe fields or requested safe fields only.

- [ ] **Step 1: Write failing tests**

Add tests near the existing `process_find_*` tests in `crates/stateful-cli/src/sandbox.rs`:

```rust
#[test]
fn process_find_default_output_includes_safe_ps_metadata() {
    let request = SandboxProcessFindRequest {
        names: Vec::new(),
        contains: vec!["denovo_codex_agent".to_string()],
        pids: Vec::new(),
        parent_pids: Vec::new(),
        process_groups: Vec::new(),
        fields: Vec::new(),
    };
    let rows = parse_process_find_ps_output(
        "202 1 202 arthur 501 S 10:30 00:02 00:00:01 37.5 1.2 123456 789012 0 31 ttys001 python3 python3 crates/stateful-bench/scripts/denovo_codex_agent.py\n",
    )
    .expect("ps output should parse");

    let output = process_find_output_for_rows(&request, rows).expect("output should serialize");
    let process = output.processes[0]
        .as_object()
        .expect("process should be an object");

    assert_eq!(process.get("pid").and_then(|v| v.as_u64()), Some(202));
    assert_eq!(process.get("user").and_then(|v| v.as_str()), Some("arthur"));
    assert_eq!(process.get("uid").and_then(|v| v.as_u64()), Some(501));
    assert_eq!(process.get("tty").and_then(|v| v.as_str()), Some("ttys001"));
    assert_eq!(process.get("pmem").and_then(|v| v.as_str()), Some("1.2"));
    assert_eq!(process.get("rss").and_then(|v| v.as_u64()), Some(123456));
    assert!(!process.contains_key("command"));
    assert!(!process.contains_key("argv"));
    assert!(!process.contains_key("env"));
}

#[test]
fn process_find_selected_fields_omit_unselected_safe_fields() {
    let request = SandboxProcessFindRequest {
        names: Vec::new(),
        contains: vec!["denovo_codex_agent".to_string()],
        pids: Vec::new(),
        parent_pids: Vec::new(),
        process_groups: Vec::new(),
        fields: vec!["pid".to_string(), "user".to_string()],
    };
    let rows = parse_process_find_ps_output(
        "202 1 202 arthur 501 S 10:30 00:02 00:00:01 37.5 1.2 123456 789012 0 31 ttys001 python3 python3 crates/stateful-bench/scripts/denovo_codex_agent.py\n",
    )
    .expect("ps output should parse");

    let output = process_find_output_for_rows(&request, rows).expect("output should serialize");
    let process = output.processes[0]
        .as_object()
        .expect("process should be an object");

    assert_eq!(process.len(), 2);
    assert_eq!(process.get("pid").and_then(|v| v.as_u64()), Some(202));
    assert_eq!(process.get("user").and_then(|v| v.as_str()), Some("arthur"));
}

#[test]
fn process_find_rejects_forbidden_output_fields() {
    let request = SandboxProcessFindRequest {
        names: Vec::new(),
        contains: vec!["denovo_codex_agent".to_string()],
        pids: Vec::new(),
        parent_pids: Vec::new(),
        process_groups: Vec::new(),
        fields: vec!["command".to_string()],
    };

    let error = validate_process_find_request(&request)
        .expect_err("forbidden process field should be rejected");

    assert!(
        error
            .to_string()
            .contains("stateful sandbox process find cannot expose field `command`"),
        "unexpected error: {error}"
    );
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run:

```bash
cargo test -p stateful-cli sandbox::tests::process_find_default_output_includes_safe_ps_metadata sandbox::tests::process_find_selected_fields_omit_unselected_safe_fields sandbox::tests::process_find_rejects_forbidden_output_fields
```

Expected: FAIL because `fields` and `process_find_output_for_rows` do not exist yet.

- [ ] **Step 3: Implement minimal Rust model and serializer**

In `crates/stateful-cli/src/sandbox.rs`, change the process output types to use JSON objects for selectable fields:

```rust
pub struct SandboxProcessFindRequest {
    pub names: Vec<String>,
    pub contains: Vec<String>,
    pub pids: Vec<u32>,
    pub parent_pids: Vec<u32>,
    pub process_groups: Vec<u32>,
    pub fields: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct SandboxProcessFindOutput {
    pub status: &'static str,
    pub processes: Vec<serde_json::Value>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SandboxProcessInfo {
    pub pid: u32,
    pub ppid: u32,
    pub pgid: u32,
    pub user: String,
    pub uid: u32,
    pub stat: String,
    pub start: String,
    pub etime: String,
    pub time: String,
    pub pcpu: String,
    pub pmem: String,
    pub rss: u64,
    pub vsz: u64,
    pub nice: String,
    pub pri: String,
    pub tty: String,
    pub comm: String,
}
```

Add helpers:

```rust
const PROCESS_FIND_SAFE_FIELDS: &[&str] = &[
    "pid", "ppid", "pgid", "user", "uid", "stat", "start", "etime", "time", "pcpu",
    "pmem", "rss", "vsz", "nice", "pri", "tty", "comm",
];

fn validate_process_find_field(field: &str) -> anyhow::Result<()> {
    match field {
        "command" | "args" | "argv" | "env" => {
            anyhow::bail!("stateful sandbox process find cannot expose field `{field}`")
        }
        safe if PROCESS_FIND_SAFE_FIELDS.contains(&safe) => Ok(()),
        _ => anyhow::bail!("stateful sandbox process find unknown field `{field}`"),
    }
}

fn process_find_output_for_rows(
    request: &SandboxProcessFindRequest,
    rows: Vec<SandboxProcessRow>,
) -> anyhow::Result<SandboxProcessFindOutput> {
    validate_process_find_request(request)?;
    let processes = filter_process_find_rows(request, rows)
        .into_iter()
        .map(|info| process_find_info_json(&info, &request.fields))
        .collect::<anyhow::Result<Vec<_>>>()?;
    Ok(SandboxProcessFindOutput {
        status: "ok",
        processes,
    })
}

fn process_find_info_json(
    info: &SandboxProcessInfo,
    fields: &[String],
) -> anyhow::Result<serde_json::Value> {
    let selected = if fields.is_empty() {
        PROCESS_FIND_SAFE_FIELDS.to_vec()
    } else {
        fields.iter().map(String::as_str).collect::<Vec<_>>()
    };
    let mut object = serde_json::Map::new();
    for field in selected {
        validate_process_find_field(field)?;
        let value = match field {
            "pid" => serde_json::json!(info.pid),
            "ppid" => serde_json::json!(info.ppid),
            "pgid" => serde_json::json!(info.pgid),
            "user" => serde_json::json!(info.user),
            "uid" => serde_json::json!(info.uid),
            "stat" => serde_json::json!(info.stat),
            "start" => serde_json::json!(info.start),
            "etime" => serde_json::json!(info.etime),
            "time" => serde_json::json!(info.time),
            "pcpu" => serde_json::json!(info.pcpu),
            "pmem" => serde_json::json!(info.pmem),
            "rss" => serde_json::json!(info.rss),
            "vsz" => serde_json::json!(info.vsz),
            "nice" => serde_json::json!(info.nice),
            "pri" => serde_json::json!(info.pri),
            "tty" => serde_json::json!(info.tty),
            "comm" => serde_json::json!(info.comm),
            _ => unreachable!("field was validated"),
        };
        object.insert(field.to_string(), value);
    }
    Ok(serde_json::Value::Object(object))
}
```

Update `validate_process_find_request` to validate `request.fields`, update `run_sandbox_process_find` to call `process_find_output_for_rows`, update parser setup and CLI request construction to include `fields`, and update `parse_process_find_ps_output` to parse this fixed order:

```text
pid ppid pgid user uid stat start etime time pcpu pmem rss vsz nice pri tty comm command...
```

Use `parse_process_find_field` for `uid`; add a `parse_process_find_u64_field` for `rss` and `vsz`.

- [ ] **Step 4: Run tests to verify pass**

Run:

```bash
cargo test -p stateful-cli sandbox::tests::process_find_default_output_includes_safe_ps_metadata sandbox::tests::process_find_selected_fields_omit_unselected_safe_fields sandbox::tests::process_find_rejects_forbidden_output_fields
```

Expected: PASS.

- [ ] **Step 5: Commit task**

Stage only changed source/test files for this task:

```bash
git add crates/stateful-cli/src/sandbox.rs crates/stateful-cli/src/lib.rs
git commit -m "Add safe process_find output fields"
```

---

### Task 2: OMP schema and docs

**Files:**
- Modify: `crates/stateful-cli/src/install.rs:1970-1982`, `crates/stateful-cli/src/install.rs:2659-2683`, `crates/stateful-cli/tests/install_global.rs:249-299`
- Modify: `docs/usage-reference.md:331-340`
- Modify if conflicting: `README.md:218-220`, `crates/stateful-cli/assets/stateful-command-policy/SKILL.md:36-38`

**Interfaces:**
- Consumes: `SandboxProcessFindRequest.fields` from Task 1.
- Produces: OMP `process_find` parameter `fields: string[]`; docs explaining safe default fields and field selection.

- [ ] **Step 1: Write failing generated-extension test**

Add assertions to `crates/stateful-cli/tests/install_global.rs` near `omp_stateful_extension_contains_sandbox_tools`:

```rust
assert!(extension.contains("fields: { type: \"array\", items: { type: \"string\" }"));
assert!(extension.contains("for (const field of params.fields || [])"));
assert!(extension.contains("args.push(\"--field\", field)"));
```

- [ ] **Step 2: Run test to verify it fails**

Run:

```bash
cargo test -p stateful-cli --test install_global omp_stateful_extension_contains_sandbox_tools
```

Expected: FAIL because the generated extension does not mention `fields`.

- [ ] **Step 3: Implement OMP argument mapping**

In `processFindArgs(params)` in `crates/stateful-cli/src/install.rs`, add:

```javascript
  for (const field of params.fields || []) args.push("--field", field);
```

In the `process_find` tool schema, add:

```javascript
        fields: { type: "array", items: { type: "string" }, description: "Safe output fields to include. Omit to include every safe process metadata field; command, argv, and env are never exposed." },
```

- [ ] **Step 4: Update docs**

Change `docs/usage-reference.md` process section to say:

```markdown
In OMP, call the generated `process_find` tool directly instead of routing
process inspection through `sandbox_bash` or raw Bash. By default, process
output includes safe `ps` metadata: `pid`, `ppid`, `pgid`, `user`, `uid`,
`stat`, `start`, `etime`, `time`, `pcpu`, `pmem`, `rss`, `vsz`, `nice`, `pri`,
`tty`, and `comm`. Pass `fields` in OMP or repeated `--field` flags on the CLI
to return a smaller safe set. `command`, argv, and env are never exposed in
result JSON.
```

If README or the installed stateful-command-policy skill still claims only `pcpu` was added, update that sentence to the same safe-field summary.

- [ ] **Step 5: Run targeted tests**

Run:

```bash
cargo test -p stateful-cli --test install_global omp_stateful_extension_contains_sandbox_tools
cargo test -p stateful-cli sandbox::tests::process_find_default_output_includes_safe_ps_metadata sandbox::tests::process_find_selected_fields_omit_unselected_safe_fields sandbox::tests::process_find_rejects_forbidden_output_fields
```

Expected: PASS.

- [ ] **Step 6: Commit task**

Stage only changed files for this task:

```bash
git add crates/stateful-cli/src/install.rs crates/stateful-cli/tests/install_global.rs docs/usage-reference.md README.md crates/stateful-cli/assets/stateful-command-policy/SKILL.md
git commit -m "Document process_find field selection"
```

---

## Self-Review

- Spec coverage: Task 1 covers safe defaults, selected fields, forbidden field rejection, and internal command filtering. Task 2 covers OMP schema and user docs.
- Placeholder scan: no TBD/TODO/fill-in steps remain.
- Type consistency: the plan uses `fields: Vec<String>` in Rust, CLI `--field`, and OMP `fields: string[]` throughout.
