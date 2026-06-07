# Sandbox Run Implementation Plan

> Public documentation note: this is a historical implementation record kept for traceability, not current user-facing guidance. See `README.md` and the top-level `docs/` contract documents for current behavior.

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the local file-writing MCP Bash tool with a generic `stateful sandbox run` CLI wrapper that enforces read-only and declared-target filesystem profiles.

**Architecture:** MCP keeps only centralized state operations. Bash execution moves to a local CLI runner that validates profile flags, binds to the current session for writes, authorizes declared targets through `/v1/authorize`, and runs the inner command in the existing macOS/Linux sandbox backends. Codex hooks deny raw Bash and allow only a strictly parsed, trusted `stateful sandbox run` outer command.

**Tech Stack:** Rust, Clap, serde/serde_json, current stateful HTTP runtime helpers, macOS Seatbelt, Linux bubblewrap, existing CLI/MCP/hook tests.

---

## File Structure

- Create `crates/stateful-cli/src/sandbox.rs`
  - Owns `SandboxFsProfile`, `SandboxNetworkPolicy`, `SandboxRunRequest`, `SandboxRunOutput`, argument validation, target normalization, authorization, writable-file preparation, sandbox command construction, timeout handling, and execution.
  - Extracts the current sandbox code from `crates/stateful-cli/src/mcp.rs`.
- Modify `crates/stateful-cli/src/lib.rs`
  - Adds `stateful sandbox run` CLI parsing.
  - Dispatches to `sandbox::run_sandbox_in_repo`.
  - Re-exports sandbox enums for parser tests.
  - Updates generated `stateful-command-policy` skill text.
- Modify `crates/stateful-cli/src/mcp.rs`
  - Removes `call_bash_write_tool`, `BashWriteArguments`, local sandbox code, and the `state.bash.write` special case.
  - Keeps normal MCP-to-HTTP mapping.
- Modify `crates/stateful-cli/src/hook.rs`
  - Replaces Bash classifier-based allow behavior with strict `stateful sandbox run` outer-command validation.
  - Keeps native write-tool authorization for `apply_patch`, `Edit`, `Write`, and `file_change`.
- Modify `crates/stateful-mcp/src/lib.rs`
  - Removes `state_bash_write` / `state.bash.write` descriptors and protocol mapping.
- Modify tests:
  - `crates/stateful-cli/tests/cli.rs`
  - `crates/stateful-cli/tests/hook.rs`
  - `crates/stateful-cli/tests/mcp.rs`
  - `crates/stateful-cli/tests/init.rs`
  - `crates/stateful-mcp/tests/tools.rs`
  - module tests in `crates/stateful-cli/src/sandbox.rs`
- Modify docs:
  - `README.md`
  - `docs/architecture.md`
  - `docs/core-concept.md`
  - `docs/current-state-coordination.md`
  - `docs/implementation-contract.md`
  - `docs/state-model.md`
  - `docs/v1-hardening-scope-decisions.md`

---

### Task 1: Add CLI Shape For `stateful sandbox run`

**Files:**
- Modify: `crates/stateful-cli/src/lib.rs`
- Modify: `crates/stateful-cli/tests/cli.rs`

- [ ] **Step 1: Write CLI parser tests**

Append these tests to `crates/stateful-cli/tests/cli.rs`:

```rust
use stateful_cli::{SandboxFsProfile, SandboxNetworkPolicy};

#[test]
fn parses_sandbox_run_read_only_defaults() {
    let cli = Cli::try_parse_from([
        "stateful",
        "sandbox",
        "run",
        "--command",
        "rg auth src",
    ])
    .expect("sandbox run should parse");

    match cli.command {
        Command::Sandbox(SandboxCommand::Run {
            fs,
            network,
            write_targets,
            create_targets,
            command,
            timeout_seconds,
        }) => {
            assert_eq!(fs, SandboxFsProfile::ReadOnly);
            assert_eq!(network, SandboxNetworkPolicy::Disabled);
            assert!(write_targets.is_empty());
            assert!(create_targets.is_empty());
            assert_eq!(command, "rg auth src");
            assert_eq!(timeout_seconds, None);
        }
        other => panic!("expected sandbox run command, got {other:?}"),
    }
}

#[test]
fn parses_sandbox_run_write_targets_network_enabled() {
    let cli = Cli::try_parse_from([
        "stateful",
        "sandbox",
        "run",
        "--fs",
        "write-targets",
        "--network",
        "enabled",
        "--write-target",
        "README.md",
        "--create-target",
        "docs/new.md",
        "--timeout-seconds",
        "12",
        "--command",
        "printf x > README.md",
    ])
    .expect("sandbox run should parse");

    match cli.command {
        Command::Sandbox(SandboxCommand::Run {
            fs,
            network,
            write_targets,
            create_targets,
            command,
            timeout_seconds,
        }) => {
            assert_eq!(fs, SandboxFsProfile::WriteTargets);
            assert_eq!(network, SandboxNetworkPolicy::Enabled);
            assert_eq!(write_targets, vec!["README.md"]);
            assert_eq!(create_targets, vec!["docs/new.md"]);
            assert_eq!(command, "printf x > README.md");
            assert_eq!(timeout_seconds, Some(12));
        }
        other => panic!("expected sandbox run command, got {other:?}"),
    }
}

#[test]
fn sandbox_run_rejects_missing_command() {
    let error = Cli::try_parse_from(["stateful", "sandbox", "run"])
        .expect_err("sandbox run requires --command");

    assert!(error.to_string().contains("--command"));
}
```

Update the import at the top of the file to include `SandboxCommand`:

```rust
use stateful_cli::{
    Cli, CodexSandboxMode, Command, HookCommand, McpCommand, NotificationsCommand, ReposCommand,
    ResumeCommand, SandboxCommand, ServerCommand,
};
```

- [ ] **Step 2: Run parser tests and verify failure**

Run:

```bash
cargo test -p stateful-cli --test cli sandbox_run -- --nocapture
```

Expected: compile failure because `SandboxCommand`, `SandboxFsProfile`, and `SandboxNetworkPolicy` do not exist.

- [ ] **Step 3: Add CLI enum types**

In `crates/stateful-cli/src/lib.rs`, add the module and exports near the existing module/export list:

```rust
mod sandbox;

pub use sandbox::{SandboxFsProfile, SandboxNetworkPolicy};
```

Add the new command variant to `Command`:

```rust
#[command(subcommand)]
Sandbox(SandboxCommand),
```

Add the command enums near the other subcommand enums:

```rust
#[derive(Debug, Subcommand)]
pub enum SandboxCommand {
    Run {
        #[arg(long, value_enum, default_value = "read-only")]
        fs: SandboxFsProfile,
        #[arg(long, value_enum, default_value = "disabled")]
        network: SandboxNetworkPolicy,
        #[arg(long = "write-target")]
        write_targets: Vec<String>,
        #[arg(long = "create-target")]
        create_targets: Vec<String>,
        #[arg(long)]
        command: String,
        #[arg(long)]
        timeout_seconds: Option<u64>,
    },
}
```

Add an empty `crates/stateful-cli/src/sandbox.rs` shell with the value enums:

```rust
use clap::ValueEnum;

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum SandboxFsProfile {
    ReadOnly,
    WriteTargets,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum SandboxNetworkPolicy {
    Disabled,
    Enabled,
}
```

- [ ] **Step 4: Wire a temporary dispatch error**

In the `run()` match in `crates/stateful-cli/src/lib.rs`, add:

```rust
Command::Sandbox(SandboxCommand::Run { .. }) => {
    anyhow::bail!("stateful sandbox run is not implemented yet");
}
```

- [ ] **Step 5: Run parser tests and verify pass**

Run:

```bash
cargo test -p stateful-cli --test cli sandbox_run -- --nocapture
```

Expected: the new parser tests pass.

- [ ] **Step 6: Commit**

```bash
git add crates/stateful-cli/src/lib.rs crates/stateful-cli/src/sandbox.rs crates/stateful-cli/tests/cli.rs
git commit -m "feat: add sandbox run cli shape"
```

---

### Task 2: Move Sandbox Backend Code Into `sandbox.rs`

**Files:**
- Modify: `crates/stateful-cli/src/sandbox.rs`
- Modify: `crates/stateful-cli/src/mcp.rs`

- [ ] **Step 1: Add backend tests in the new module**

Add this test module to `crates/stateful-cli/src/sandbox.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::path::{Path, PathBuf};

    #[test]
    fn bubblewrap_read_only_uses_unshare_net_and_dev_null_device() {
        let args = bubblewrap_args(
            "rg auth src",
            Path::new("/repo"),
            &[],
            SandboxNetworkPolicy::Disabled,
        );
        let args = args
            .into_iter()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect::<Vec<_>>();

        assert!(args.contains(&"--unshare-net".to_string()));
        assert!(args.windows(3).any(|window| {
            window == ["--dev-bind", "/dev/null", "/dev/null"]
        }));
        assert!(args.ends_with(&[
            "--".to_string(),
            "/bin/sh".to_string(),
            "-c".to_string(),
            "rg auth src".to_string(),
        ]));
    }

    #[test]
    fn bubblewrap_network_enabled_omits_unshare_net() {
        let args = bubblewrap_args(
            "git ls-remote origin",
            Path::new("/repo"),
            &[],
            SandboxNetworkPolicy::Enabled,
        );
        let args = args
            .into_iter()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect::<Vec<_>>();

        assert!(!args.contains(&"--unshare-net".to_string()));
    }

    #[test]
    fn bubblewrap_write_targets_bind_authorized_files_and_dev_null() {
        let writable_files = vec![
            PathBuf::from("/repo/src/allowed.ts"),
            PathBuf::from("/repo/src/new.ts"),
        ];
        let args = bubblewrap_args(
            "printf ok > src/allowed.ts",
            Path::new("/repo"),
            &writable_files,
            SandboxNetworkPolicy::Disabled,
        );
        let args = args
            .into_iter()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect::<Vec<_>>();

        assert!(args.windows(3).any(|window| {
            window == ["--bind", "/repo/src/allowed.ts", "/repo/src/allowed.ts"]
        }));
        assert!(args.windows(3).any(|window| {
            window == ["--bind", "/repo/src/new.ts", "/repo/src/new.ts"]
        }));
        assert!(args.windows(3).any(|window| {
            window == ["--dev-bind", "/dev/null", "/dev/null"]
        }));
    }

    #[test]
    fn seatbelt_profile_allows_dev_null_and_exact_targets() {
        let profile = seatbelt_profile(&[
            PathBuf::from("/repo/src/allowed.ts"),
            PathBuf::from("/repo/src/quoted\"path.ts"),
        ]);

        assert!(profile.contains("(deny default)"));
        assert!(profile.contains("(allow file-read*)"));
        assert!(profile.contains("(literal \"/dev/null\")"));
        assert!(profile.contains("(literal \"/repo/src/allowed.ts\")"));
        assert!(profile.contains("(literal \"/repo/src/quoted\\\"path.ts\")"));
        assert!(!profile.contains("subpath \"/repo/src\""));
        assert!(!profile.contains("subpath \"/dev\""));
    }
}
```

- [ ] **Step 2: Run new module tests and verify failure**

Run:

```bash
cargo test -p stateful-cli sandbox::tests -- --nocapture
```

Expected: compile failure because `bubblewrap_args` and `seatbelt_profile` have not been moved.

- [ ] **Step 3: Move backend code**

Move these items from `crates/stateful-cli/src/mcp.rs` to `crates/stateful-cli/src/sandbox.rs`:

```rust
fn run_sandboxed_bash(...)
fn seatbelt_command(...)
fn seatbelt_profile(...)
fn seatbelt_escape(...)
fn bubblewrap_command(...)
fn bubblewrap_args(...)
fn run_command_with_timeout(...)
```

Move the result type and adapt its name:

```rust
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct SandboxCommandResult {
    pub status: &'static str,
    pub exit_code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
}
```

Update the backend entry point signature in `sandbox.rs`:

```rust
pub fn run_sandboxed_command(
    command: &str,
    cwd: &Path,
    writable_files: &[PathBuf],
    network: SandboxNetworkPolicy,
    timeout: Duration,
) -> anyhow::Result<SandboxCommandResult> {
    #[cfg(target_os = "macos")]
    {
        run_command_with_timeout(seatbelt_command(command, cwd, writable_files, network), timeout)
    }

    #[cfg(target_os = "linux")]
    {
        run_command_with_timeout(bubblewrap_command(command, cwd, writable_files, network), timeout)
    }

    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        let _ = (command, cwd, writable_files, network, timeout);
        anyhow::bail!("stateful sandbox run is only supported on macOS and Linux");
    }
}
```

For macOS, add the `network` parameter even if the first implementation does not alter the Seatbelt profile:

```rust
#[cfg(target_os = "macos")]
fn seatbelt_command(
    command: &str,
    cwd: &Path,
    writable_files: &[PathBuf],
    _network: SandboxNetworkPolicy,
) -> Command {
    let profile = seatbelt_profile(writable_files);
    let mut sandbox = Command::new("/usr/bin/sandbox-exec");
    sandbox
        .arg("-p")
        .arg(profile)
        .arg("/bin/sh")
        .arg("-c")
        .arg(command)
        .current_dir(cwd);
    sandbox
}
```

For Linux, include `/dev/null` and make `--unshare-net` conditional:

```rust
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn bubblewrap_args(
    command: &str,
    cwd: &Path,
    writable_files: &[PathBuf],
    network: SandboxNetworkPolicy,
) -> Vec<OsString> {
    let mut args = vec![
        OsString::from("--unshare-all"),
        OsString::from("--die-with-parent"),
    ];

    if matches!(network, SandboxNetworkPolicy::Disabled) {
        args.push(OsString::from("--unshare-net"));
    }

    args.extend([
        OsString::from("--ro-bind"),
        OsString::from("/"),
        OsString::from("/"),
        OsString::from("--proc"),
        OsString::from("/proc"),
        OsString::from("--dev-bind"),
        OsString::from("/dev/null"),
        OsString::from("/dev/null"),
    ]);

    for file in writable_files {
        args.push(OsString::from("--bind"));
        args.push(file.as_os_str().to_owned());
        args.push(file.as_os_str().to_owned());
    }

    args.push(OsString::from("--chdir"));
    args.push(cwd.as_os_str().to_owned());
    args.push(OsString::from("--"));
    args.push(OsString::from("/bin/sh"));
    args.push(OsString::from("-c"));
    args.push(OsString::from(command));
    args
}
```

Keep these imports in `sandbox.rs`:

```rust
use std::{
    ffi::OsString,
    io::Read,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};
```

- [ ] **Step 4: Remove duplicate mcp module backend tests**

Delete the old `#[cfg(test)] mod tests` at the bottom of `crates/stateful-cli/src/mcp.rs` once the functions are no longer in that file.

- [ ] **Step 5: Run backend tests and verify pass**

Run:

```bash
cargo test -p stateful-cli sandbox::tests -- --nocapture
```

Expected: all sandbox module backend tests pass.

- [ ] **Step 6: Commit**

```bash
git add crates/stateful-cli/src/sandbox.rs crates/stateful-cli/src/mcp.rs
git commit -m "refactor: move sandbox backend into cli module"
```

---

### Task 3: Implement `sandbox run` Validation And Write Authorization

**Files:**
- Modify: `crates/stateful-cli/src/sandbox.rs`
- Modify: `crates/stateful-cli/src/lib.rs`
- Modify: `crates/stateful-cli/tests/mcp.rs`

- [ ] **Step 1: Write CLI execution tests**

Add these tests to `crates/stateful-cli/tests/mcp.rs` near the existing bash write tests. They reuse the existing helpers in that file.

```rust
#[test]
fn sandbox_run_write_targets_reports_allowed_and_denied_without_running_command() {
    let temp_root = temp_root("stateful-sandbox-run-deny");
    let paths = GlobalPaths::new(temp_root.join("home"));
    let repo_root = temp_root.join("repo");
    fs::create_dir_all(repo_root.join("src")).expect("repo src should be creatable");
    enable_test_repo(&paths, &repo_root);
    write_current_session_file(&repo_root, &CurrentSession::new("s-current", "w1"))
        .expect("current session should write");
    fs::write(repo_root.join("src/allowed.ts"), "old\n").expect("allowed file should seed");
    let (runtime, rx) = spawn_fake_stateful_server_sequence(vec![
        r#"{"decision":"allow","reason_code":"authorized","message":"ok","required_next_action":null}"#,
        r#"{"decision":"deny","reason_code":"scope_mismatch","message":"Target is outside active intent scope.","required_next_action":"Declare matching intent."}"#,
    ]);
    write_global_runtime_file(&paths, &runtime).expect("global runtime file should write");

    let output = run_stateful_in_repo(
        &repo_root,
        &paths,
        &[
            "sandbox",
            "run",
            "--fs",
            "write-targets",
            "--network",
            "enabled",
            "--write-target",
            "src/allowed.ts",
            "--write-target",
            "src/denied.ts",
            "--command",
            "printf changed > src/allowed.ts",
        ],
    );

    assert!(!output.status.success(), "denied target should fail");
    let first = rx
        .recv_timeout(Duration::from_secs(1))
        .expect("first authorize request should arrive");
    let second = rx
        .recv_timeout(Duration::from_secs(1))
        .expect("second authorize request should arrive");
    assert_eq!(request_json_body(&first)["payload"]["path"], "src/allowed.ts");
    assert_eq!(request_json_body(&second)["payload"]["path"], "src/denied.ts");
    assert_eq!(
        fs::read_to_string(repo_root.join("src/allowed.ts")).expect("allowed file should read"),
        "old\n",
        "command should not run when any target is denied"
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("\"allowed_write_targets\":[\"src/allowed.ts\"]"));
    assert!(stdout.contains("\"path\":\"src/denied.ts\""));
    assert!(stdout.contains("\"decision\":\"deny\""));

    fs::remove_dir_all(&temp_root).expect("temp root should be removable");
}

#[test]
fn sandbox_run_read_only_rejects_write_targets() {
    let temp_root = temp_root("stateful-sandbox-run-readonly-target");
    let paths = GlobalPaths::new(temp_root.join("home"));
    let repo_root = temp_root.join("repo");
    fs::create_dir_all(&repo_root).expect("repo root should be creatable");
    enable_test_repo(&paths, &repo_root);
    let (runtime, _rx) = spawn_fake_stateful_server(r#"{"status":"ok"}"#);
    write_global_runtime_file(&paths, &runtime).expect("global runtime file should write");

    let output = run_stateful_in_repo(
        &repo_root,
        &paths,
        &[
            "sandbox",
            "run",
            "--fs",
            "read-only",
            "--write-target",
            "README.md",
            "--command",
            "rg README",
        ],
    );

    assert!(!output.status.success(), "read-only must reject write targets");
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("read-only profile rejects write targets")
    );

    fs::remove_dir_all(&temp_root).expect("temp root should be removable");
}
```

- [ ] **Step 2: Run tests and verify failure**

Run:

```bash
cargo test -p stateful-cli --test mcp sandbox_run_ -- --nocapture
```

Expected: failures because `sandbox run` dispatch still returns “not implemented yet”.

- [ ] **Step 3: Add request and output types**

In `crates/stateful-cli/src/sandbox.rs`, add:

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SandboxRunRequest {
    pub fs: SandboxFsProfile,
    pub network: SandboxNetworkPolicy,
    pub write_targets: Vec<String>,
    pub create_targets: Vec<String>,
    pub command: String,
    pub timeout_seconds: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct SandboxRunOutput {
    pub status: &'static str,
    pub exit_code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
    pub allowed_write_targets: Vec<String>,
    pub denied_write_targets: Vec<serde_json::Value>,
}
```

- [ ] **Step 4: Add validation and target helpers**

Move and rename the current target helpers from `mcp.rs` into `sandbox.rs`:

```rust
fn normalize_sandbox_target_paths(field: &str, paths: &[String]) -> anyhow::Result<Vec<String>>
fn normalize_sandbox_target_path(field: &str, path: &str) -> anyhow::Result<String>
fn ensure_repo_file_target(repo_root: &Path, relative_path: &str) -> anyhow::Result<()>
fn resolve_sandbox_cwd(repo_root: &Path) -> anyhow::Result<PathBuf>
fn prepare_sandbox_writable_files(
    repo_root: &Path,
    write_targets: &[String],
    create_targets: &[String],
) -> anyhow::Result<Vec<PathBuf>>
```

Add profile validation:

```rust
fn validate_profile_targets(
    fs: SandboxFsProfile,
    write_targets: &[String],
    create_targets: &[String],
) -> anyhow::Result<()> {
    match fs {
        SandboxFsProfile::ReadOnly => {
            if !write_targets.is_empty() || !create_targets.is_empty() {
                anyhow::bail!("read-only profile rejects write targets and create targets");
            }
        }
        SandboxFsProfile::WriteTargets => {
            if write_targets.is_empty() && create_targets.is_empty() {
                anyhow::bail!("write-targets profile requires at least one write or create target");
            }
        }
    }

    Ok(())
}
```

- [ ] **Step 5: Add authorization**

Add:

```rust
fn authorize_sandbox_write(
    runtime: &ServerRuntime,
    repo_root: &Path,
    paths: &GlobalPaths,
    session_id: &str,
    workspace_id: &str,
    network: SandboxNetworkPolicy,
    path: &str,
) -> anyhow::Result<HttpResponse> {
    let body = protocol_envelope(ProtocolEnvelopeArgs {
        runtime,
        request_id: uuid::Uuid::new_v4().to_string(),
        session_id: session_id.to_string(),
        workspace_id: workspace_id.to_string(),
        identity: repo_identity_for_enabled_repo(paths, repo_root).ok(),
        source_kind: "cli",
        event: "sandbox_run",
        source_ref: "stateful.sandbox.run",
        payload: serde_json::json!({
            "action": "write_file",
            "path": path,
            "queue_on_conflict": true,
            "fs_profile": "write-targets",
            "network_policy": match network {
                SandboxNetworkPolicy::Disabled => "disabled",
                SandboxNetworkPolicy::Enabled => "enabled",
            },
        }),
    });

    post_json(runtime, "/v1/authorize", &body)
}
```

Import these crate items in `sandbox.rs`:

```rust
use crate::{
    CurrentSession, GlobalPaths, HttpResponse, ProtocolEnvelopeArgs, RepoGate, ServerRuntime,
    discover_runtime_with_global, ensure_server, post_json, protocol_envelope,
    read_current_session_file, repo_gate, repo_identity_for_enabled_repo,
};
```

- [ ] **Step 6: Implement `run_sandbox_in_repo`**

Add:

```rust
pub fn run_sandbox_in_repo(
    start: impl AsRef<Path>,
    paths: &GlobalPaths,
    request: SandboxRunRequest,
) -> anyhow::Result<SandboxRunOutput> {
    if request.command.trim().is_empty() {
        anyhow::bail!("sandbox run command is required");
    }

    let repo_root = match repo_gate(paths, start.as_ref())? {
        RepoGate::Enabled { repo_root } => {
            ensure_server(paths)?;
            repo_root
        }
        RepoGate::Disabled | RepoGate::OutsideGitRepo => {
            anyhow::bail!("repo not enabled");
        }
    };
    let runtime = discover_runtime_with_global(&repo_root, paths)?;
    let write_targets =
        normalize_sandbox_target_paths("write_targets", &request.write_targets)?;
    let create_targets =
        normalize_sandbox_target_paths("create_targets", &request.create_targets)?;
    validate_profile_targets(request.fs, &write_targets, &create_targets)?;

    let mut allowed = Vec::new();
    let mut denied = Vec::new();
    if matches!(request.fs, SandboxFsProfile::WriteTargets) {
        let CurrentSession {
            session_id,
            workspace_id,
        } = read_current_session_file(&repo_root)
            .map_err(|_| anyhow::anyhow!("sandbox write-targets requires a current stateful session"))?;

        for path in write_targets.iter().chain(create_targets.iter()) {
            let response = authorize_sandbox_write(
                &runtime,
                &repo_root,
                paths,
                &session_id,
                &workspace_id,
                request.network,
                path,
            )?;
            let body = serde_json::from_str::<serde_json::Value>(&response.body)
                .unwrap_or_else(|_| serde_json::json!({ "message": response.body }));
            if (200..300).contains(&response.status_code)
                && body.get("decision").and_then(serde_json::Value::as_str) == Some("allow")
            {
                allowed.push(path.clone());
            } else {
                denied.push(serde_json::json!({
                    "path": path,
                    "authorization": body,
                }));
            }
        }
    }

    if !denied.is_empty() {
        anyhow::bail!(
            "{}",
            serde_json::json!({
                "status": "error",
                "message": "stateful sandbox run target authorization denied",
                "allowed_write_targets": allowed,
                "denied_write_targets": denied,
            })
        );
    }

    let cwd = resolve_sandbox_cwd(&repo_root)?;
    let writable_files = if matches!(request.fs, SandboxFsProfile::WriteTargets) {
        prepare_sandbox_writable_files(&repo_root, &write_targets, &create_targets)?
    } else {
        Vec::new()
    };
    let timeout = Duration::from_secs(request.timeout_seconds.unwrap_or(300).max(1));
    let run = run_sandboxed_command(
        &request.command,
        &cwd,
        &writable_files,
        request.network,
        timeout,
    )?;

    Ok(SandboxRunOutput {
        status: run.status,
        exit_code: run.exit_code,
        stdout: run.stdout,
        stderr: run.stderr,
        allowed_write_targets: allowed,
        denied_write_targets: Vec::new(),
    })
}
```

If `CurrentSession` fields are private in the active tree, replace destructuring with:

```rust
let current_session = read_current_session_file(&repo_root)
    .map_err(|_| anyhow::anyhow!("sandbox write-targets requires a current stateful session"))?;
let session_id = current_session.session_id;
let workspace_id = current_session.workspace_id;
```

- [ ] **Step 7: Dispatch from `run()`**

In `crates/stateful-cli/src/lib.rs`, replace the temporary sandbox dispatch with:

```rust
Command::Sandbox(SandboxCommand::Run {
    fs,
    network,
    write_targets,
    create_targets,
    command,
    timeout_seconds,
}) => {
    let paths = GlobalPaths::from_env()?;
    let repo_root = current_repo_root_or_current_dir()?;
    let output = sandbox::run_sandbox_in_repo(
        &repo_root,
        &paths,
        sandbox::SandboxRunRequest {
            fs,
            network,
            write_targets,
            create_targets,
            command,
            timeout_seconds,
        },
    )?;
    println!("{}", serde_json::to_string(&output)?);
    if output.status != "exited" || output.exit_code != Some(0) {
        std::process::exit(output.exit_code.unwrap_or(1));
    }
}
```

- [ ] **Step 8: Run targeted tests**

Run:

```bash
cargo test -p stateful-cli --test cli sandbox_run -- --nocapture
cargo test -p stateful-cli --test mcp sandbox_run_ -- --nocapture
```

Expected: parser tests pass; sandbox run authorization tests pass.

- [ ] **Step 9: Commit**

```bash
git add crates/stateful-cli/src/lib.rs crates/stateful-cli/src/sandbox.rs crates/stateful-cli/tests/mcp.rs
git commit -m "feat: implement sandbox run write authorization"
```

---

### Task 4: Replace Bash Hook Authorization With Strict Wrapper Validation

**Files:**
- Modify: `crates/stateful-cli/src/hook.rs`
- Modify: `crates/stateful-cli/tests/hook.rs`

- [ ] **Step 1: Add hook tests for strict sandbox wrapper calls**

Add these tests to `crates/stateful-cli/tests/hook.rs`:

```rust
#[test]
fn pre_tool_use_denies_raw_read_only_bash_after_sandbox_runner_migration() {
    let input = r#"{
      "session_id": "s1",
      "cwd": "/repo",
      "hook_event_name": "PreToolUse",
      "tool_name": "Bash",
      "tool_input": {
        "command": "rg auth src"
      }
    }"#;

    let outcome = handle_pre_tool_use(input).expect("hook input should parse");

    assert_bash_denial_mentions(outcome, "stateful sandbox run");
}

#[test]
fn pre_tool_use_allows_canonical_sandbox_run_read_only() {
    let stateful = std::env::current_exe()
        .expect("test executable path should resolve")
        .to_string_lossy()
        .into_owned();
    let input = serde_json::json!({
        "session_id": "s1",
        "cwd": "/repo",
        "hook_event_name": "PreToolUse",
        "tool_name": "Bash",
        "tool_input": {
            "command": format!("{stateful} sandbox run --fs read-only --network disabled --command 'rg auth src'")
        }
    })
    .to_string();

    let outcome = handle_pre_tool_use(&input).expect("hook input should parse");

    assert_eq!(outcome, HookOutcome::Allow);
}

#[test]
fn pre_tool_use_allows_canonical_sandbox_run_write_targets() {
    let stateful = std::env::current_exe()
        .expect("test executable path should resolve")
        .to_string_lossy()
        .into_owned();
    let input = serde_json::json!({
        "session_id": "s1",
        "cwd": "/repo",
        "hook_event_name": "PreToolUse",
        "tool_name": "Bash",
        "tool_input": {
            "command": format!("{stateful} sandbox run --fs write-targets --network enabled --write-target README.md --command 'printf x > README.md'")
        }
    })
    .to_string();

    let outcome = handle_pre_tool_use(&input).expect("hook input should parse");

    assert_eq!(outcome, HookOutcome::Allow);
}

#[test]
fn pre_tool_use_denies_sandbox_run_with_outer_command_separator() {
    let stateful = std::env::current_exe()
        .expect("test executable path should resolve")
        .to_string_lossy()
        .into_owned();
    let input = serde_json::json!({
        "session_id": "s1",
        "cwd": "/repo",
        "hook_event_name": "PreToolUse",
        "tool_name": "Bash",
        "tool_input": {
            "command": format!("{stateful} sandbox run --fs read-only --command 'rg auth src'; rm README.md")
        }
    })
    .to_string();

    let outcome = handle_pre_tool_use(&input).expect("hook input should parse");

    assert_bash_denial_mentions(outcome, "single stateful sandbox run command");
}

#[test]
fn pre_tool_use_denies_sandbox_run_write_targets_without_target() {
    let stateful = std::env::current_exe()
        .expect("test executable path should resolve")
        .to_string_lossy()
        .into_owned();
    let input = serde_json::json!({
        "session_id": "s1",
        "cwd": "/repo",
        "hook_event_name": "PreToolUse",
        "tool_name": "Bash",
        "tool_input": {
            "command": format!("{stateful} sandbox run --fs write-targets --network enabled --command 'printf x > README.md'")
        }
    })
    .to_string();

    let outcome = handle_pre_tool_use(&input).expect("hook input should parse");

    assert_bash_denial_mentions(outcome, "requires at least one write target");
}
```

- [ ] **Step 2: Run hook tests and verify failure**

Run:

```bash
cargo test -p stateful-cli --test hook sandbox_run raw_read_only -- --nocapture
```

Expected: new tests fail because Bash still uses the old classifier/sandbox metadata path.

- [ ] **Step 3: Add a strict outer command parser**

In `crates/stateful-cli/src/hook.rs`, add a small parser that accepts only one simple command. The parser must preserve quoted `--command` as one argument and reject outer shell syntax before `--command` is interpreted.

Add:

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
struct SandboxRunInvocation {
    executable: String,
    fs: String,
    network: String,
    write_targets: Vec<String>,
    create_targets: Vec<String>,
    command: String,
}

fn parse_sandbox_run_invocation(command: &str) -> Result<SandboxRunInvocation, String> {
    reject_outer_shell_syntax(command)?;
    let words = split_simple_command_words(command)?;
    if words.len() < 5 {
        return Err("Bash commands must use stateful sandbox run".to_string());
    }
    if words.get(1).map(String::as_str) != Some("sandbox")
        || words.get(2).map(String::as_str) != Some("run")
    {
        return Err("Bash commands must use stateful sandbox run".to_string());
    }

    let executable = words[0].clone();
    let mut fs = "read-only".to_string();
    let mut network = "disabled".to_string();
    let mut write_targets = Vec::new();
    let mut create_targets = Vec::new();
    let mut inner_command = None;
    let mut index = 3;
    while index < words.len() {
        match words[index].as_str() {
            "--fs" => {
                index += 1;
                fs = words
                    .get(index)
                    .cloned()
                    .ok_or_else(|| "--fs requires a value".to_string())?;
            }
            "--network" => {
                index += 1;
                network = words
                    .get(index)
                    .cloned()
                    .ok_or_else(|| "--network requires a value".to_string())?;
            }
            "--write-target" => {
                index += 1;
                write_targets.push(
                    words
                        .get(index)
                        .cloned()
                        .ok_or_else(|| "--write-target requires a value".to_string())?,
                );
            }
            "--create-target" => {
                index += 1;
                create_targets.push(
                    words
                        .get(index)
                        .cloned()
                        .ok_or_else(|| "--create-target requires a value".to_string())?,
                );
            }
            "--command" => {
                if inner_command.is_some() {
                    return Err("stateful sandbox run requires exactly one --command".to_string());
                }
                index += 1;
                inner_command = Some(
                    words
                        .get(index)
                        .cloned()
                        .ok_or_else(|| "--command requires a value".to_string())?,
                );
            }
            "--" => return Err("stateful sandbox run does not support argv mode".to_string()),
            other => return Err(format!("unsupported stateful sandbox run argument `{other}`")),
        }
        index += 1;
    }

    Ok(SandboxRunInvocation {
        executable,
        fs,
        network,
        write_targets,
        create_targets,
        command: inner_command
            .ok_or_else(|| "stateful sandbox run requires exactly one --command".to_string())?,
    })
}
```

Implement `reject_outer_shell_syntax` and `split_simple_command_words` without adding a dependency. Use a quote-aware state machine:

```rust
fn reject_outer_shell_syntax(command: &str) -> Result<(), String> {
    let mut quote = QuoteState::None;
    let chars = command.chars().collect::<Vec<_>>();
    let mut index = 0;
    while index < chars.len() {
        let current = chars[index];
        match quote {
            QuoteState::None => match current {
                '\'' => quote = QuoteState::Single,
                '"' => quote = QuoteState::Double,
                ';' | '|' | '&' | '<' | '>' | '\n' | '\r' | '`' => {
                    return Err("Bash wrapper must be a single stateful sandbox run command".to_string());
                }
                '$' if chars.get(index + 1) == Some(&'(') => {
                    return Err("Bash wrapper must not use command substitution".to_string());
                }
                _ => {}
            },
            QuoteState::Single => {
                if current == '\'' {
                    quote = QuoteState::None;
                }
            }
            QuoteState::Double => match current {
                '"' => quote = QuoteState::None,
                '`' => return Err("Bash wrapper must not use command substitution".to_string()),
                '$' if chars.get(index + 1) == Some(&'(') => {
                    return Err("Bash wrapper must not use command substitution".to_string());
                }
                _ => {}
            },
        }
        index += 1;
    }

    if quote != QuoteState::None {
        return Err("Bash wrapper command has unterminated quotes".to_string());
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum QuoteState {
    None,
    Single,
    Double,
}
```

`split_simple_command_words` must split on whitespace outside quotes and remove quotes:

```rust
fn split_simple_command_words(command: &str) -> Result<Vec<String>, String> {
    let mut words = Vec::new();
    let mut current = String::new();
    let mut quote = QuoteState::None;

    for character in command.chars() {
        match quote {
            QuoteState::None => match character {
                '\'' => quote = QuoteState::Single,
                '"' => quote = QuoteState::Double,
                c if c.is_whitespace() => {
                    if !current.is_empty() {
                        words.push(std::mem::take(&mut current));
                    }
                }
                c => current.push(c),
            },
            QuoteState::Single => {
                if character == '\'' {
                    quote = QuoteState::None;
                } else {
                    current.push(character);
                }
            }
            QuoteState::Double => {
                if character == '"' {
                    quote = QuoteState::None;
                } else {
                    current.push(character);
                }
            }
        }
    }

    if quote != QuoteState::None {
        return Err("Bash wrapper command has unterminated quotes".to_string());
    }
    if !current.is_empty() {
        words.push(current);
    }
    Ok(words)
}
```

Reject environment assignments after parsing:

```rust
fn first_word_is_env_assignment(word: &str) -> bool {
    let Some((name, _value)) = word.split_once('=') else {
        return false;
    };
    !name.is_empty()
        && name
            .chars()
            .all(|character| character == '_' || character.is_ascii_alphanumeric())
        && !name
            .chars()
            .next()
            .is_some_and(|character| character.is_ascii_digit())
}
```

Call this before accepting the executable:

```rust
if first_word_is_env_assignment(&words[0]) {
    return Err("Bash wrapper must not use outer environment assignments".to_string());
}
```

- [ ] **Step 4: Validate profile and targets in hook**

Add:

```rust
fn authorize_sandbox_run_bash(command: &str) -> HookOutcome {
    let invocation = match parse_sandbox_run_invocation(command) {
        Ok(invocation) => invocation,
        Err(reason) => return HookOutcome::Deny { reason },
    };

    if !is_trusted_stateful_executable(&invocation.executable) {
        return HookOutcome::Deny {
            reason: "stateful sandbox run requires the trusted stateful binary".to_string(),
        };
    }

    if !matches!(invocation.fs.as_str(), "read-only" | "write-targets") {
        return HookOutcome::Deny {
            reason: "stateful sandbox run supports only read-only and write-targets profiles".to_string(),
        };
    }
    if !matches!(invocation.network.as_str(), "disabled" | "enabled") {
        return HookOutcome::Deny {
            reason: "stateful sandbox run network must be disabled or enabled".to_string(),
        };
    }
    if invocation.command.trim().is_empty() {
        return HookOutcome::Deny {
            reason: "stateful sandbox run requires a non-empty --command".to_string(),
        };
    }
    if invocation.fs == "read-only"
        && (!invocation.write_targets.is_empty() || !invocation.create_targets.is_empty())
    {
        return HookOutcome::Deny {
            reason: "read-only sandbox run rejects write targets".to_string(),
        };
    }
    if invocation.fs == "write-targets"
        && invocation.write_targets.is_empty()
        && invocation.create_targets.is_empty()
    {
        return HookOutcome::Deny {
            reason: "write-targets sandbox run requires at least one write target".to_string(),
        };
    }

    HookOutcome::Allow
}
```

Implement trusted executable validation conservatively:

```rust
fn is_trusted_stateful_executable(executable: &str) -> bool {
    let path = Path::new(executable);
    if !path.is_absolute() {
        return false;
    }
    let Ok(candidate) = path.canonicalize() else {
        return false;
    };
    let Ok(current) = std::env::current_exe() else {
        return false;
    };
    let current = current.canonicalize().unwrap_or(current);
    candidate == current
}
```

This MVP rejects bare `stateful`. A later implementation can resolve bare names against the installed binary if needed.

- [ ] **Step 5: Replace Bash authorization branch**

In `authorize_bash`, replace classifier behavior with:

```rust
fn authorize_bash(
    input: &PreToolUseInput,
    _trusted_sandbox: Option<&serde_json::Value>,
) -> anyhow::Result<HookOutcome> {
    let command = input.command().unwrap_or_default();
    Ok(authorize_sandbox_run_bash(command))
}
```

Do not remove `classify_bash` from `stateful-core` in this task. That cleanup happens after migration tests are updated.

- [ ] **Step 6: Run targeted hook tests**

Run:

```bash
cargo test -p stateful-cli --test hook pre_tool_use_denies_raw_read_only_bash_after_sandbox_runner_migration -- --nocapture
cargo test -p stateful-cli --test hook pre_tool_use_allows_canonical_sandbox_run -- --nocapture
cargo test -p stateful-cli --test hook pre_tool_use_denies_sandbox_run -- --nocapture
```

Expected: all new hook tests pass. Existing hook tests that expect top-level sandbox metadata may now fail; they are migrated in the next task.

- [ ] **Step 7: Commit**

```bash
git add crates/stateful-cli/src/hook.rs crates/stateful-cli/tests/hook.rs
git commit -m "feat: gate bash through sandbox run wrapper"
```

---

### Task 5: Remove `state_bash_write` From MCP

**Files:**
- Modify: `crates/stateful-mcp/src/lib.rs`
- Modify: `crates/stateful-mcp/tests/tools.rs`
- Modify: `crates/stateful-cli/src/mcp.rs`
- Modify: `crates/stateful-cli/tests/mcp.rs`

- [ ] **Step 1: Update MCP tests to expect removal**

In `crates/stateful-mcp/tests/tools.rs`, replace assertions that expect `state_bash_write` with:

```rust
#[test]
fn bash_write_tool_is_removed_from_mcp_surface() {
    assert!(protocol_tool_name("state_bash_write").is_err());
    assert!(protocol_tool_name("state.bash.write").is_err());

    let names = tool_descriptors()
        .into_iter()
        .map(|tool| tool.name)
        .collect::<Vec<_>>();
    assert!(!names.contains(&"state_bash_write"));
}
```

In `crates/stateful-cli/tests/mcp.rs`, change `mcp_tools_list_returns_stateful_tool_descriptors` so it asserts:

```rust
assert!(!tools.iter().any(|tool| tool["name"] == "state_bash_write"));
```

Add a stale-call test:

```rust
#[test]
fn mcp_stale_bash_write_call_returns_removed_guidance() {
    let temp_root = temp_root("stateful-mcp-stale-bash-write");
    let paths = GlobalPaths::new(temp_root.join("home"));
    let repo_root = temp_root.join("repo");
    fs::create_dir_all(&repo_root).expect("repo root should be creatable");
    enable_test_repo(&paths, &repo_root);
    let (runtime, _rx) = spawn_fake_stateful_server(r#"{"status":"ok"}"#);
    write_global_runtime_file(&paths, &runtime).expect("global runtime file should write");

    let response = run_mcp_jsonrpc_in_repo(
        &repo_root,
        &paths,
        r#"{
          "jsonrpc":"2.0",
          "id":4,
          "method":"tools/call",
          "params":{
            "name":"state_bash_write",
            "arguments":{"command":"true","write_targets":["README.md"]}
          }
        }"#,
    );

    let json: serde_json::Value = serde_json::from_str(&response).expect("response should be json");
    assert_eq!(json["result"]["isError"], true);
    assert!(
        json["result"]["content"][0]["text"]
            .as_str()
            .unwrap_or_default()
            .contains("state_bash_write was removed")
    );

    fs::remove_dir_all(&temp_root).expect("temp root should be removable");
}
```

- [ ] **Step 2: Run MCP tests and verify failure**

Run:

```bash
cargo test -p stateful-mcp --test tools bash_write -- --nocapture
cargo test -p stateful-cli --test mcp stale_bash_write tools_list -- --nocapture
```

Expected: failures because descriptors and CLI MCP bridge still expose `state_bash_write`.

- [ ] **Step 3: Remove MCP descriptor**

In `crates/stateful-mcp/src/lib.rs`, remove this tuple from `TOOLS`:

```rust
(
    "state_bash_write",
    "state.bash.write",
    "Run a write-capable Bash command in an OS sandbox after target authorization.",
),
```

Remove the `"state.bash.write"` arm from `input_schema_for`.

Remove the `"state.bash.write"` special-case error from `map_tool_to_http`.

- [ ] **Step 4: Remove local MCP bash bridge**

In `crates/stateful-cli/src/mcp.rs`, remove:

```rust
if protocol_name == "state.bash.write" {
    return call_bash_write_tool(runtime, repo_root, paths, arguments);
}
```

Remove these now-unused functions/types from `mcp.rs`:

```rust
call_bash_write_tool
authorize_file_write
ensure_repo_file_target
normalize_bash_target_paths
normalize_bash_target_path
resolve_bash_cwd
prepare_bash_writable_files
BashWriteArguments
```

Keep stale-call guidance by adding this before `protocol_tool_name` is called in `call_mcp_tool_in_repo`:

```rust
if matches!(tool_name.as_str(), "state_bash_write" | "state.bash.write") {
    return Ok(error_response(
        410,
        "state_bash_write was removed; use `stateful sandbox run ... --command ...`.",
    ));
}
```

Apply the same stale-call check inside `handle_mcp_jsonrpc_in_repo` before it delegates to `call_mcp_tool_in_repo` if direct JSON-RPC calls still return a generic unknown-tool error.

- [ ] **Step 5: Run MCP tests**

Run:

```bash
cargo test -p stateful-mcp --test tools -- --nocapture
cargo test -p stateful-cli --test mcp -- --nocapture
```

Expected: MCP descriptor tests pass; old mcp bash write tests are either migrated to sandbox run tests or removed.

- [ ] **Step 6: Commit**

```bash
git add crates/stateful-mcp/src/lib.rs crates/stateful-mcp/tests/tools.rs crates/stateful-cli/src/mcp.rs crates/stateful-cli/tests/mcp.rs
git commit -m "refactor: remove bash write mcp tool"
```

---

### Task 6: Update Hook Tests And Remove Old Bash Metadata Guidance

**Files:**
- Modify: `crates/stateful-cli/tests/hook.rs`
- Modify: `crates/stateful-core/src/bash.rs`
- Modify: `crates/stateful-core/src/lib.rs`
- Modify: `crates/stateful-core/tests/bash.rs`

- [ ] **Step 1: Replace old hook expectations**

In `crates/stateful-cli/tests/hook.rs`, remove or rewrite tests that expect:

```text
top-level read-only sandbox metadata allows rg/git/stateful doctor
STATEFUL_HOOK_TRUSTED_SANDBOX is considered
state_bash_write appears in denial guidance
```

Use the new denial assertion:

```rust
assert_bash_denial_mentions(outcome, "stateful sandbox run");
```

Keep native write-tool tests for `apply_patch`, `Edit`, `Write`, `file_change`, and `mcp__filesystem__.*`.

- [ ] **Step 2: Run hook tests and verify failures are limited**

Run:

```bash
cargo test -p stateful-cli --test hook -- --nocapture
```

Expected: remaining failures point to old guidance strings or tests that still assume metadata-authorized raw Bash.

- [ ] **Step 3: Decide whether to keep `stateful-core` Bash classifier**

If no production code uses `classify_bash` after Task 4, remove:

```rust
mod bash;
pub use bash::{BashClassification, BashKind, classify_bash};
```

from `crates/stateful-core/src/lib.rs`, delete `crates/stateful-core/src/bash.rs`, and delete `crates/stateful-core/tests/bash.rs`.

If another current branch change still uses it, leave it in place and add a comment in the final summary that cleanup is blocked by active usage.

- [ ] **Step 4: Run hook/core tests**

Run:

```bash
cargo test -p stateful-cli --test hook -- --nocapture
cargo test -p stateful-core -- --nocapture
```

Expected: hook tests pass; core tests pass or no longer include bash classifier tests.

- [ ] **Step 5: Commit**

```bash
git add crates/stateful-cli/tests/hook.rs crates/stateful-core/src/lib.rs crates/stateful-core/src/bash.rs crates/stateful-core/tests/bash.rs
git commit -m "test: migrate bash hook expectations to sandbox run"
```

If `bash.rs` is retained, omit it from `git add`.

---

### Task 7: Update Generated Skill And Docs

**Files:**
- Modify: `crates/stateful-cli/src/lib.rs`
- Modify: `crates/stateful-cli/tests/init.rs`
- Modify: `README.md`
- Modify: `docs/architecture.md`
- Modify: `docs/core-concept.md`
- Modify: `docs/current-state-coordination.md`
- Modify: `docs/implementation-contract.md`
- Modify: `docs/state-model.md`
- Modify: `docs/v1-hardening-scope-decisions.md`

- [ ] **Step 1: Update generated skill test**

In `crates/stateful-cli/tests/init.rs`, replace:

```rust
assert!(command_policy_skill.contains("state_bash_write"));
```

with:

```rust
assert!(command_policy_skill.contains("stateful sandbox run"));
assert!(!command_policy_skill.contains("state_bash_write"));
```

- [ ] **Step 2: Run init test and verify failure**

Run:

```bash
cargo test -p stateful-cli --test init command_policy -- --nocapture
```

Expected: failure because generated skill text still mentions `state_bash_write`.

- [ ] **Step 3: Update generated skill text**

In `crates/stateful-cli/src/lib.rs`, update `stateful_command_policy_skill()` so these bullets replace the old `state_bash_write` guidance:

```markdown
- Use `stateful sandbox run --fs write-targets --write-target <path> --command <cmd>` for repo writes and other write-capable Bash. Declare intent first, keep targets explicit and repo-relative, and list new files with `--create-target`.
- Use `stateful sandbox run --fs read-only --network disabled --command <cmd>` for read-only inspection commands in Codex sessions.
- Treat generators, formatters, package managers, build commands, and tests that create artifacts as write-capable commands. Route them through `stateful sandbox run` with explicit targets, using `--write-dir target` for build artifacts.
```

Update the “If Blocked” section to say:

```markdown
- If a command needs write permission, choose `stateful sandbox run --fs write-targets ...` with `--write-target`, `--create-target`, or `--write-dir` as appropriate.
```

- [ ] **Step 4: Update docs**

Replace `state_bash_write` examples with:

```bash
stateful sandbox run --fs write-targets --network enabled --write-target README.md --command "printf updated > README.md"
```

Replace read-only Bash examples with:

```bash
stateful sandbox run --fs read-only --network disabled --command "rg auth src"
```

In architecture/contract docs, state:

```text
MCP does not perform local file writes. Shell execution uses `stateful sandbox run`.
The MVP ships `read-only` and `write-targets` profiles. `git-metadata` and
`workspace` are deferred and fail closed.
```

Mention `/dev/null`:

```text
`/dev/null` is writable in every sandbox profile so common shell and Git behavior works.
```

- [ ] **Step 5: Run documentation grep**

Run:

```bash
rg -n "state_bash_write|state\\.bash\\.write" README.md docs crates/stateful-cli/src/lib.rs crates/stateful-cli/tests/init.rs
```

Expected: no matches except removal-guidance tests or historical spec/plan files under `docs/superpowers`.

- [ ] **Step 6: Run init test**

Run:

```bash
cargo test -p stateful-cli --test init -- --nocapture
```

Expected: init tests pass.

- [ ] **Step 7: Commit**

```bash
git add README.md docs/architecture.md docs/core-concept.md docs/current-state-coordination.md docs/implementation-contract.md docs/state-model.md docs/v1-hardening-scope-decisions.md crates/stateful-cli/src/lib.rs crates/stateful-cli/tests/init.rs
git commit -m "docs: document sandbox run command flow"
```

---

### Task 8: Final Verification And Integration Review

**Files:**
- No new files expected.
- Review changed files from Tasks 1-7.

- [ ] **Step 1: Run targeted suites**

Run:

```bash
cargo test -p stateful-mcp -- --nocapture
cargo test -p stateful-cli --test cli -- --nocapture
cargo test -p stateful-cli --test hook -- --nocapture
cargo test -p stateful-cli --test mcp -- --nocapture
cargo test -p stateful-cli --test init -- --nocapture
cargo test -p stateful-core -- --nocapture
```

Expected: all targeted suites pass.

- [ ] **Step 2: Run workspace tests**

Run:

```bash
cargo test --workspace
```

Expected: workspace tests pass. If platform sandbox tests are skipped because Linux `bwrap` is unavailable or macOS Seatbelt differs, record the exact skipped test and reason in the final handoff.

- [ ] **Step 3: Check stale references**

Run:

```bash
rg -n "state_bash_write|state\\.bash\\.write" README.md docs crates
```

Expected: only historical files under `docs/superpowers` mention removed names, plus explicit stale-call error tests if they remain.

- [ ] **Step 4: Check git status**

Run:

```bash
git status --short
```

Expected: only intentional changes from the implementation branch are present. Existing unrelated user changes must not be reverted.

- [ ] **Step 5: Request implementation review**

Dispatch a reviewer with:

```text
Review the sandbox run implementation against docs/superpowers/specs/2026-06-05-sandbox-run-design.md and docs/superpowers/plans/2026-06-05-sandbox-run-implementation-plan.md. Focus on hook parser bypasses, wrapper binary identity checks, session binding, authorization payloads, sandbox profile enforcement, MCP removal, and docs consistency.
```

- [ ] **Step 6: Fix review findings**

Address Critical and Important reviewer findings before final handoff. Run the targeted tests affected by each fix.

- [ ] **Step 7: Final commit if needed**

If review fixes produce new changes:

```bash
git add <changed-files>
git commit -m "fix: harden sandbox run migration"
```

If no review fixes are needed, do not create an empty commit.
