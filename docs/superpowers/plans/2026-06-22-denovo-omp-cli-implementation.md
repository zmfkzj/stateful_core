# DeNovo OMP CLI Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add `stateful-bench denovo run --agent omp-cli` so DeNovoSWE can run through isolated OMP with `deepseek-v4-flash`.

**Architecture:** Reuse the existing DeNovo adapter and condition matrix. Add an OMP runtime branch beside Codex command construction/execution, with isolated OMP home state and stateful install/config present only for `stateful:on`.

**Tech Stack:** Rust/clap/serde for `stateful-bench`; Python stdlib subprocess/pathlib for the adapter; existing Rust integration tests that import Python helper functions.

---

## File Map

- Modify: `crates/stateful-bench/src/denovo.rs`
  - Add `DeNovoAgentKind::OmpCli`.
  - Add `--omp-bin` and OMP default model selection.
  - Pass `--cli-runtime codex|omp` and `--omp-bin` to the existing adapter.
  - Write OMP outputs under `conditions/<condition>/omp-cli`.
- Modify: `crates/stateful-bench/scripts/denovo_codex_agent.py`
  - Add `--cli-runtime codex|omp` and `--omp-bin`.
  - Build/run OMP commands without Codex flags.
  - Create isolated OMP home/profile state; run stateful OMP install only for `stateful:on`.
- Modify: `crates/stateful-bench/tests/cli.rs`
  - Add CLI parse tests and Python helper tests for OMP command/isolation.
- Modify: `crates/stateful-bench/tests/denovo.rs`
  - Add command-builder test for `DeNovoAgentKind::OmpCli`.
- Modify: `docs/denovo-benchmark-commands.md`
  - Add OMP `deepseek-v4-flash` benchmark command notes.

---

### Task 1: Rust CLI accepts OMP agent and options

**Files:**
- Modify: `crates/stateful-bench/src/denovo.rs`
- Modify: `crates/stateful-bench/tests/cli.rs`

- [ ] **Step 1: Write the failing CLI parse test**

Add this test after `denovo_run_command_parses_codex_cli_agent_options` in `crates/stateful-bench/tests/cli.rs`:

```rust
#[test]
fn denovo_run_command_parses_omp_cli_agent_options() {
    let cli = Cli::try_parse_from([
        "stateful-bench",
        "denovo",
        "run",
        "--agent",
        "omp-cli",
        "--aweagent-root",
        "../AweAgent",
        "--data-file",
        ".stateful_bench/denovo/extracts/dev/results.jsonl",
        "--output-dir",
        "target/stateful-bench/denovo/runs",
        "--run-id",
        "dev-denovo-omp",
        "--condition",
        "stateful:off,subagent:on",
        "--condition",
        "stateful:on,subagent:on",
        "--omp-bin",
        "/opt/homebrew/bin/omp",
        "--stateful-binary",
        "/Users/arthur/.cargo/bin/stateful",
        "--benchmark-model",
        "deepseek-v4-flash",
    ])
    .expect("denovo omp run command should parse");

    assert!(matches!(
        cli.command,
        Command::Denovo {
            command: DeNovoCommand::Run {
                agent: stateful_bench::DeNovoAgentKind::OmpCli,
                ref run_id,
                ref condition,
                ref omp_bin,
                ref benchmark_model,
                ..
            }
        } if run_id == "dev-denovo-omp"
            && condition == &vec![
                "stateful:off,subagent:on".to_string(),
                "stateful:on,subagent:on".to_string(),
            ]
            && omp_bin == "/opt/homebrew/bin/omp"
            && benchmark_model.as_deref() == Some("deepseek-v4-flash")
    ));
}
```

- [ ] **Step 2: Run the focused test and verify RED**

Run:

```bash
/Users/arthur/.cargo/bin/stateful sandbox run --fs build --network enabled --write-dir omp-cli-red-1 --command '/usr/bin/env CARGO_HOME=$TMPDIR/cargo-home cargo test -p stateful-bench denovo_run_command_parses_omp_cli_agent_options'
```

Expected: FAIL because `omp-cli`, `--omp-bin`, and `benchmark_model: Option<String>` are not implemented.

- [ ] **Step 3: Add Rust CLI fields and defaults**

In `crates/stateful-bench/src/denovo.rs`, change the default constants near the top to include OMP:

```rust
const DEFAULT_CODEX_BIN: &str = "codex";
const DEFAULT_OMP_BIN: &str = "omp";
const DEFAULT_CODEX_MODEL: &str = "gpt-5.4-mini";
const DEFAULT_OMP_MODEL: &str = "deepseek-v4-flash";
```

Extend `DeNovoAgentKind`:

```rust
pub enum DeNovoAgentKind {
    Official,
    CodexCli,
    OmpCli,
}
```

In `DeNovoCommand::Run`, add `omp_bin` after `codex_bin`, and make `benchmark_model` optional so the default can depend on `agent`:

```rust
#[arg(long, default_value = DEFAULT_CODEX_BIN)]
codex_bin: String,
#[arg(long, default_value = DEFAULT_OMP_BIN)]
omp_bin: String,
#[arg(long)]
stateful_binary: String,
#[arg(long)]
benchmark_model: Option<String>,
```

In the `run_denovo_cli` match arm for `DeNovoCommand::Run`, compute the model before building `DeNovoMatrixRunOptions`:

```rust
let benchmark_model = benchmark_model.unwrap_or_else(|| match agent {
    DeNovoAgentKind::OmpCli => DEFAULT_OMP_MODEL.to_string(),
    _ => DEFAULT_CODEX_MODEL.to_string(),
});
```

Pass `omp_bin` and the computed `benchmark_model` into `DeNovoMatrixRunOptions`.

- [ ] **Step 4: Thread `omp_bin` through Rust options**

Add this field to `DeNovoConditionRunOptions`, `DeNovoMatrixRunOptions`, and `DeNovoCodexRunOptions`:

```rust
pub omp_bin: String,
```

Pass it through `run_denovo_matrix`, `run_denovo_condition`, and `build_denovo_codex_adapter_command` calls.

- [ ] **Step 5: Run the focused test and verify GREEN**

Run:

```bash
/Users/arthur/.cargo/bin/stateful sandbox run --fs build --network enabled --write-dir omp-cli-green-1 --command '/usr/bin/env CARGO_HOME=$TMPDIR/cargo-home cargo test -p stateful-bench denovo_run_command_parses_omp_cli_agent_options'
```

Expected: PASS.

- [ ] **Step 6: Commit Task 1**

Run separate git commands through the git profile:

```bash
/Users/arthur/.cargo/bin/stateful sandbox run --fs git --network disabled --command 'git add crates/stateful-bench/src/denovo.rs crates/stateful-bench/tests/cli.rs'
/Users/arthur/.cargo/bin/stateful sandbox run --fs git --network disabled --command 'git commit -m "Add DeNovo OMP CLI options"'
```

---

### Task 2: Rust command builder routes OMP through the existing adapter

**Files:**
- Modify: `crates/stateful-bench/src/denovo.rs`
- Modify: `crates/stateful-bench/tests/denovo.rs`

- [ ] **Step 1: Write the failing command-builder test**

Add this test after `denovo_codex_adapter_command_uses_stateful_adapter_and_condition_axes` in `crates/stateful-bench/tests/denovo.rs`:

```rust
#[test]
fn denovo_omp_adapter_command_uses_existing_adapter_with_omp_runtime() {
    let command = build_denovo_codex_adapter_command(DeNovoCodexRunOptions {
        aweagent_root: "../AweAgent".into(),
        python: "python3".to_string(),
        data_file: "denovoswe_with_patches.jsonl".into(),
        output: "target/stateful-bench/denovo/runs/dev/omp-cli".into(),
        base_config: "configs/tasks/denovoswe.yaml".into(),
        condition: DeNovoCondition::new(true, true),
        mode: DeNovoRunMode::Batch,
        instance_ids: vec!["PyCQA_pep8_pr970".to_string()],
        max_steps: Some(500),
        max_concurrent: Some(1),
        skip_eval: false,
        validate_run: true,
        eval_iters: 1,
        del_done_images: false,
        dump_clean_snapshot: None,
        prompt_version: "v2".to_string(),
        verbose: true,
        codex_bin: "/opt/homebrew/bin/codex".to_string(),
        omp_bin: "/opt/homebrew/bin/omp".to_string(),
        stateful_binary: "/Users/arthur/.cargo/bin/stateful".to_string(),
        benchmark_model: "deepseek-v4-flash".to_string(),
        benchmark_reasoning_effort: "low".to_string(),
        benchmark_model_context_window: 256000,
        benchmark_temperature: "1".to_string(),
        benchmark_max_turns: 500,
        subagent_min_count: 4,
        max_resumes: 2,
        codex_timeout_seconds: 7200,
        adapter_script: Some("crates/stateful-bench/scripts/denovo_codex_agent.py".into()),
        cli_runtime: DeNovoCliRuntime::Omp,
    })
    .expect("omp adapter command should build");

    assert_eq!(command.program, "python3");
    assert!(command.args.windows(2).any(|pair| pair == ["--cli-runtime", "omp"]));
    assert!(command.args.windows(2).any(|pair| pair == ["--omp-bin", "/opt/homebrew/bin/omp"]));
    assert!(command.args.windows(2).any(|pair| pair == ["--benchmark-model", "deepseek-v4-flash"]));
    assert!(command.args.windows(2).any(|pair| pair == ["--agent-mode", "stateful"]));
    assert!(command.args.windows(2).any(|pair| pair == ["--subagent", "on"]));
}
```

Also export `DeNovoCliRuntime` from `crates/stateful-bench/src/lib.rs` by adding it immediately after `DeNovoAgentKind` in the existing `pub use denovo` list.

- [ ] **Step 2: Run the focused test and verify RED**

Run:

```bash
/Users/arthur/.cargo/bin/stateful sandbox run --fs build --network enabled --write-dir omp-cli-red-2 --command '/usr/bin/env CARGO_HOME=$TMPDIR/cargo-home cargo test -p stateful-bench denovo_omp_adapter_command_uses_existing_adapter_with_omp_runtime'
```

Expected: FAIL because `DeNovoCliRuntime` and `--cli-runtime` are absent.

- [ ] **Step 3: Add runtime enum and adapter args**

In `crates/stateful-bench/src/denovo.rs`, add:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeNovoCliRuntime {
    Codex,
    Omp,
}

impl DeNovoCliRuntime {
    fn as_str(self) -> &'static str {
        match self {
            Self::Codex => "codex",
            Self::Omp => "omp",
        }
    }
}
```

Add `pub cli_runtime: DeNovoCliRuntime` to `DeNovoCodexRunOptions`.

In `run_denovo_condition`, map the agent directory and command runtime:

```rust
let agent_dir_name = match options.agent {
    DeNovoAgentKind::Official => "official",
    DeNovoAgentKind::CodexCli => "codex-cli",
    DeNovoAgentKind::OmpCli => "omp-cli",
};
```

```rust
DeNovoAgentKind::CodexCli | DeNovoAgentKind::OmpCli => {
    let cli_runtime = match options.agent {
        DeNovoAgentKind::OmpCli => DeNovoCliRuntime::Omp,
        _ => DeNovoCliRuntime::Codex,
    };
    build_denovo_codex_adapter_command(DeNovoCodexRunOptions {
        aweagent_root: options.aweagent_root.clone(),
        python: options.python,
        data_file: command_data_file,
        output: command_output_dir,
        base_config: options.base_config,
        condition: options.condition.clone(),
        mode: options.mode,
        instance_ids: options.instance_ids,
        max_steps: options.max_steps,
        max_concurrent: options.max_concurrent,
        skip_eval: options.skip_eval,
        validate_run: options.validate_run,
        eval_iters: options.eval_iters,
        del_done_images: options.del_done_images,
        dump_clean_snapshot: options.dump_clean_snapshot,
        prompt_version: options.prompt_version,
        verbose: options.verbose,
        codex_bin: options.codex_bin,
        omp_bin: options.omp_bin,
        stateful_binary: options.stateful_binary,
        benchmark_model: options.benchmark_model,
        benchmark_reasoning_effort: options.benchmark_reasoning_effort,
        benchmark_model_context_window: options.benchmark_model_context_window,
        benchmark_temperature: options.benchmark_temperature,
        benchmark_max_turns: options.benchmark_max_turns,
        subagent_min_count: options.subagent_min_count,
        max_resumes: options.max_resumes,
        codex_timeout_seconds: options.codex_timeout_seconds,
        adapter_script: command_adapter_script,
        cli_runtime,
    })
}
```

In `build_denovo_codex_adapter_command`, add these args before `--codex-bin`:

```rust
"--cli-runtime".to_string(),
options.cli_runtime.as_str().to_string(),
```

Add `--omp-bin` next to `--codex-bin`:

```rust
"--omp-bin".to_string(),
options.omp_bin,
```

- [ ] **Step 4: Run the focused test and verify GREEN**

Run:

```bash
/Users/arthur/.cargo/bin/stateful sandbox run --fs build --network enabled --write-dir omp-cli-green-2 --command '/usr/bin/env CARGO_HOME=$TMPDIR/cargo-home cargo test -p stateful-bench denovo_omp_adapter_command_uses_existing_adapter_with_omp_runtime'
```

Expected: PASS.

- [ ] **Step 5: Commit Task 2**

```bash
/Users/arthur/.cargo/bin/stateful sandbox run --fs git --network disabled --command 'git add crates/stateful-bench/src/denovo.rs crates/stateful-bench/src/lib.rs crates/stateful-bench/tests/denovo.rs'
/Users/arthur/.cargo/bin/stateful sandbox run --fs git --network disabled --command 'git commit -m "Route DeNovo OMP through adapter"'
```

---

### Task 3: Python adapter builds isolated OMP commands

**Files:**
- Modify: `crates/stateful-bench/scripts/denovo_codex_agent.py`
- Modify: `crates/stateful-bench/tests/cli.rs`

- [ ] **Step 1: Write the failing Python helper test**

Add this test after `denovo_codex_agent_builds_no_state_and_stateful_commands` in `crates/stateful-bench/tests/cli.rs`:

```rust
#[test]
fn denovo_codex_agent_builds_omp_command_without_codex_flags() {
    let script = format!(
        r#"
import importlib.util
import json
import sys
from pathlib import Path

spec = importlib.util.spec_from_file_location("denovo_omp_agent_command_test", {agent_path})
module = importlib.util.module_from_spec(spec)
sys.modules[spec.name] = module
spec.loader.exec_module(module)

command = module.omp_command_for_profile(
    workspace=Path("/tmp/workspace"),
    prompt_path=Path("/tmp/instance/prompt.txt"),
    omp_bin="/opt/homebrew/bin/omp",
    benchmark_model="deepseek-v4-flash",
)
print(json.dumps({{"command": command}}))
"#,
        agent_path = denovo_codex_agent_path_json(),
    );
    let output = run_python_json(&script);
    let command = output["command"].as_array().expect("command should be an array");

    assert_eq!(command[0], "/opt/homebrew/bin/omp");
    assert!(command_contains(command, "-p"));
    assert!(command_contains(command, "--mode"));
    assert!(command_contains(command, "json"));
    assert!(command_contains(command, "--model"));
    assert!(command_contains(command, "deepseek-v4-flash"));
    assert!(command_contains(command, "--cwd"));
    assert!(command_contains(command, "/tmp/workspace"));
    assert!(command_contains(command, "--approval-mode"));
    assert!(command_contains(command, "yolo"));
    assert!(command_contains(command, "@/tmp/instance/prompt.txt"));
    assert!(!command_contains(command, "exec"));
    assert!(!command_contains(command, "--json"));
    assert!(!command_contains(command, "--ignore-rules"));
    assert!(!command_contains(command, "--ignore-user-config"));
    assert!(!command_contains(command, "--dangerously-bypass-hook-trust"));
}
```

- [ ] **Step 2: Run the focused test and verify RED**

Run:

```bash
/Users/arthur/.cargo/bin/stateful sandbox run --fs build --network enabled --write-dir omp-cli-red-3 --command '/usr/bin/env CARGO_HOME=$TMPDIR/cargo-home cargo test -p stateful-bench denovo_codex_agent_builds_omp_command_without_codex_flags'
```

Expected: FAIL because `omp_command_for_profile` is absent.

- [ ] **Step 3: Add OMP command helper**

In `crates/stateful-bench/scripts/denovo_codex_agent.py`, add near `codex_command_for_profile`:

```python
def omp_command_for_profile(
    workspace: Path,
    prompt_path: Path,
    omp_bin: str,
    benchmark_model: str,
) -> list[str]:
    return [
        omp_bin,
        "-p",
        "--mode",
        "json",
        "--model",
        benchmark_model,
        "--cwd",
        str(workspace),
        "--approval-mode",
        "yolo",
        f"@{prompt_path}",
    ]
```

- [ ] **Step 4: Add parser args**

In `parse_args`, add:

```python
parser.add_argument("--cli-runtime", choices=["codex", "omp"], default="codex")
parser.add_argument("--omp-bin", default="omp")
```

Keep `--codex-bin` required for now because Rust always passes it. This avoids extra parser branching.

- [ ] **Step 5: Run the focused test and verify GREEN**

Run:

```bash
/Users/arthur/.cargo/bin/stateful sandbox run --fs build --network enabled --write-dir omp-cli-green-3 --command '/usr/bin/env CARGO_HOME=$TMPDIR/cargo-home cargo test -p stateful-bench denovo_codex_agent_builds_omp_command_without_codex_flags'
```

Expected: PASS.

- [ ] **Step 6: Commit Task 3**

```bash
/Users/arthur/.cargo/bin/stateful sandbox run --fs git --network disabled --command 'git add crates/stateful-bench/scripts/denovo_codex_agent.py crates/stateful-bench/tests/cli.rs'
/Users/arthur/.cargo/bin/stateful sandbox run --fs git --network disabled --command 'git commit -m "Build isolated OMP benchmark command"'
```

---

### Task 4: Python adapter isolates OMP stateful/on and off profiles

**Files:**
- Modify: `crates/stateful-bench/scripts/denovo_codex_agent.py`
- Modify: `crates/stateful-bench/tests/cli.rs`

- [ ] **Step 1: Write the failing isolation test**

Add this test after `denovo_codex_agent_prepares_local_isolated_profiles_without_nested_root` in `crates/stateful-bench/tests/cli.rs`:

```rust
#[test]
fn denovo_codex_agent_prepares_isolated_omp_profiles_with_stateful_only_on() {
    let temp_dir = target_temp_dir("stateful-bench-denovo-omp-profiles");
    let script = format!(
        r#"
import importlib.util
import json
import sys
from pathlib import Path

spec = importlib.util.spec_from_file_location("denovo_omp_profiles_test", {agent_path})
module = importlib.util.module_from_spec(spec)
sys.modules[spec.name] = module
spec.loader.exec_module(module)

root = Path({temp_dir})
source_home = root / "source-home"
(source_home / ".codex").mkdir(parents=True, exist_ok=True)
(source_home / ".codex" / "config.toml").write_text("[mcp_servers.stateful]\ncommand = 'leak'\n")
source_env = {{
    "HOME": str(source_home),
    "PATH": "/bin",
    "CODEX_HOME": str(source_home / ".codex"),
    "CODEX_THREAD_ID": "outer-thread",
    "STATEFUL_CODEX_RUN_ID": "outer-run",
    "STATEFUL_SESSION_ID": "outer-session",
    "STATEFUL_SERVER_URL": "http://127.0.0.1:43873",
    "STATEFUL_SERVER_TOKEN": "token-123",
}}
output = root / "adapter-output"
workspace = root / "workspace"
workspace.mkdir(parents=True, exist_ok=True)
task_path = root / "extracts" / "results.jsonl"

commands = []
def fake_runner(command, text, check, env, stdout, stderr):
    commands.append({{"command": command, "home": env.get("HOME"), "stateful_home": env.get("STATEFUL_HOME")}})
    class Result:
        returncode = 0
        stdout = ""
        stderr = ""
    return Result()

no_state_env = module.denovo_omp_environment(output, "issue/no-state", task_path, workspace, source_env)
module.prepare_omp_environment(no_state_env, enable_stateful=False, stateful_binary="/tmp/stateful", runner=fake_runner)
no_state_agent = Path(no_state_env["PI_CODING_AGENT_DIR"])

stateful_env = module.denovo_omp_environment(output, "issue/stateful", task_path, workspace, source_env)
module.prepare_omp_environment(stateful_env, enable_stateful=True, stateful_binary="/tmp/stateful", runner=fake_runner)
stateful_agent = Path(stateful_env["PI_CODING_AGENT_DIR"])

print(json.dumps({{
    "no_state_home": no_state_env["HOME"],
    "no_state_agent": no_state_env["PI_CODING_AGENT_DIR"],
    "no_state_has_codex_home": "CODEX_HOME" in no_state_env,
    "no_state_has_codex_thread": "CODEX_THREAD_ID" in no_state_env,
    "no_state_has_session": "STATEFUL_SESSION_ID" in no_state_env,
    "no_state_config_exists": (no_state_agent / "config.yml").exists(),
    "stateful_home": stateful_env["HOME"],
    "stateful_agent": stateful_env["PI_CODING_AGENT_DIR"],
    "stateful_has_codex_home": "CODEX_HOME" in stateful_env,
    "stateful_has_codex_thread": "CODEX_THREAD_ID" in stateful_env,
    "stateful_has_session": "STATEFUL_SESSION_ID" in stateful_env,
    "stateful_config_exists": (stateful_agent / "config.yml").exists(),
    "install_command": commands[0]["command"] if commands else [],
}}, sort_keys=True))
"#,
        agent_path = denovo_codex_agent_path_json(),
        temp_dir = serde_json::to_string(&temp_dir.to_string_lossy()).expect("temp dir should encode as json"),
    );
    let output = run_python_json(&script);

    assert!(output["no_state_home"].as_str().unwrap().ends_with("adapter-output/omp-homes/issue-no-state/home"));
    assert!(output["stateful_home"].as_str().unwrap().ends_with("adapter-output/omp-homes/issue-stateful/home"));
    assert_eq!(output["no_state_has_codex_home"], false);
    assert_eq!(output["stateful_has_codex_home"], false);
    assert_eq!(output["no_state_has_codex_thread"], false);
    assert_eq!(output["stateful_has_codex_thread"], false);
    assert_eq!(output["no_state_has_session"], false);
    assert_eq!(output["stateful_has_session"], false);
    assert_eq!(output["no_state_config_exists"], false);
    assert_eq!(output["stateful_config_exists"], true);
    let install_command = output["install_command"].as_array().expect("install command should be captured");
    assert!(command_contains(install_command, "install"));
    assert!(command_contains(install_command, "--agent"));
    assert!(command_contains(install_command, "omp"));
    assert!(command_contains(install_command, "--yes"));

    fs::remove_dir_all(temp_dir).expect("temp dir should clean up");
}
```

- [ ] **Step 2: Run the focused test and verify RED**

Run:

```bash
/Users/arthur/.cargo/bin/stateful sandbox run --fs build --network enabled --write-dir omp-cli-red-4 --command '/usr/bin/env CARGO_HOME=$TMPDIR/cargo-home cargo test -p stateful-bench denovo_codex_agent_prepares_isolated_omp_profiles_with_stateful_only_on'
```

Expected: FAIL because `denovo_omp_environment` and `prepare_omp_environment` are absent.

- [ ] **Step 3: Add OMP environment helpers**

In `crates/stateful-bench/scripts/denovo_codex_agent.py`, add near `denovo_codex_environment`:

```python
def denovo_omp_environment(
    output: Path,
    instance_id: str,
    task_path: Path,
    workspace: Path,
    base_env: dict[str, str] | None = None,
) -> dict[str, str]:
    source_env = os.environ if base_env is None else base_env
    env = dict(source_env)
    for key in [
        "CODEX_HOME",
        "CODEX_THREAD_ID",
        "STATEFUL_CODEX_RUN_ID",
        "STATEFUL_SESSION_ID",
    ]:
        env.pop(key, None)
    home = output / "omp-homes" / path_scope_digest(instance_id, task_path, workspace) / "home"
    agent_dir = home / ".omp" / "agent"
    env["HOME"] = str(home)
    env["STATEFUL_HOME"] = str(home)
    env["PI_CODING_AGENT_DIR"] = str(agent_dir)
    env.setdefault("XDG_CONFIG_HOME", str(home / ".config"))
    env.setdefault("XDG_CACHE_HOME", str(home / ".cache"))
    return env


def prepare_omp_environment(
    env: dict[str, str],
    enable_stateful: bool,
    stateful_binary: str,
    runner: Any = subprocess.run,
) -> None:
    agent_dir = Path(env["PI_CODING_AGENT_DIR"])
    agent_dir.mkdir(parents=True, exist_ok=True)
    if not enable_stateful:
        return
    completed = runner(
        [stateful_binary, "install", "--agent", "omp", "--yes"],
        text=True,
        check=False,
        env=env,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
    )
    if completed.returncode != 0:
        message = (completed.stderr or completed.stdout).strip()
        raise StatefulRepoEnableError(message or f"stateful omp install exited {completed.returncode}")
```

- [ ] **Step 4: Run the focused test and verify GREEN**

Run:

```bash
/Users/arthur/.cargo/bin/stateful sandbox run --fs build --network enabled --write-dir omp-cli-green-4 --command '/usr/bin/env CARGO_HOME=$TMPDIR/cargo-home cargo test -p stateful-bench denovo_codex_agent_prepares_isolated_omp_profiles_with_stateful_only_on'
```

Expected: PASS.

- [ ] **Step 5: Commit Task 4**

```bash
/Users/arthur/.cargo/bin/stateful sandbox run --fs git --network disabled --command 'git add crates/stateful-bench/scripts/denovo_codex_agent.py crates/stateful-bench/tests/cli.rs'
/Users/arthur/.cargo/bin/stateful sandbox run --fs git --network disabled --command 'git commit -m "Isolate DeNovo OMP profiles"'
```

---

### Task 5: Adapter executes OMP runtime and records metadata

**Files:**
- Modify: `crates/stateful-bench/scripts/denovo_codex_agent.py`
- Modify: `crates/stateful-bench/tests/cli.rs`

- [ ] **Step 1: Write the failing timeout/run test for OMP**

Add this test near `denovo_codex_agent_timeout_wrapper_bounds_run` in `crates/stateful-bench/tests/cli.rs`:

```rust
#[test]
fn denovo_codex_agent_omp_timeout_wrapper_runs_command_without_stdin() {
    let script = format!(
        r#"
import importlib.util
import json
import sys
from pathlib import Path

spec = importlib.util.spec_from_file_location("denovo_omp_timeout_test", {agent_path})
module = importlib.util.module_from_spec(spec)
sys.modules[spec.name] = module
spec.loader.exec_module(module)

calls = []
def runner(command, cwd, text, check, env, stdout, stderr, timeout):
    calls.append({{"command": command, "cwd": str(cwd), "timeout": timeout}})
    class Result:
        returncode = 0
        stdout = '{"type":"done"}\n'
        stderr = ""
    return Result()

summary = module.run_omp_with_timeout(
    ["omp", "-p", "@/tmp/prompt.txt"],
    Path("/tmp/workspace"),
    {"HOME": "/tmp/home"},
    timeout_seconds=5,
    runner=runner,
)
print(json.dumps({{"returncode": summary.returncode, "token_usage": summary.token_usage, "calls": calls}}))
"#,
        agent_path = denovo_codex_agent_path_json(),
    );
    let output = run_python_json(&script);
    assert_eq!(output["returncode"], 0);
    assert_eq!(output["token_usage"]["turns"], 0);
    assert_eq!(output["calls"][0]["command"][0], "omp");
    assert_eq!(output["calls"][0]["cwd"], "/tmp/workspace");
}
```

- [ ] **Step 2: Run the focused test and verify RED**

Run:

```bash
/Users/arthur/.cargo/bin/stateful sandbox run --fs build --network enabled --write-dir omp-cli-red-5 --command '/usr/bin/env CARGO_HOME=$TMPDIR/cargo-home cargo test -p stateful-bench denovo_codex_agent_omp_timeout_wrapper_runs_command_without_stdin'
```

Expected: FAIL because `run_omp_with_timeout` is absent.

- [ ] **Step 3: Add OMP timeout runner**

In `crates/stateful-bench/scripts/denovo_codex_agent.py`, add near `run_codex_with_timeout`:

```python
def run_omp_with_timeout(
    command: list[str],
    workspace: Path,
    env: dict[str, str] | None,
    timeout_seconds: float,
    runner: Any = subprocess.run,
) -> CodexExecutionSummary:
    if timeout_seconds <= 0:
        raise CodexTimeoutError(f"omp timed out after {timeout_seconds:g}s")
    try:
        completed = runner(
            command,
            cwd=workspace,
            text=True,
            check=False,
            env=env,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            timeout=timeout_seconds,
        )
    except subprocess.TimeoutExpired as error:
        raise CodexTimeoutError(f"omp timed out after {timeout_seconds:g}s") from error
    return CodexExecutionSummary(returncode=completed.returncode, token_usage=empty_codex_token_usage())
```

- [ ] **Step 4: Branch run_one_instance_async for OMP**

In `run_one_instance_async`:

1. Write both prompt files:

```python
write_json(instance_dir / "prompt.json", {"prompt": prompt})
prompt_text_path = instance_dir / "prompt.txt"
prompt_text_path.write_text(prompt, encoding="utf-8")
```

2. Replace the unconditional Codex command/env setup with runtime branching:

```python
if args.cli_runtime == "omp":
    command = omp_command_for_profile(
        workspace=workspace,
        prompt_path=prompt_text_path,
        omp_bin=args.omp_bin,
        benchmark_model=args.benchmark_model,
    )
    env = denovo_omp_environment(
        output=output,
        instance_id=inst.id,
        task_path=Path(args.data_file),
        workspace=workspace,
        base_env=source_env,
    )
    prepare_omp_environment(
        env,
        enable_stateful=args.agent_mode == "stateful",
        stateful_binary=args.stateful_binary,
    )
    codex_home = Path(env["PI_CODING_AGENT_DIR"])
else:
    command = codex_command_for_profile(
        workspace=workspace,
        agent_mode=args.agent_mode,
        subagent=args.subagent,
        codex_bin=args.codex_bin,
        stateful_binary=args.stateful_binary,
        benchmark_model=args.benchmark_model,
        benchmark_reasoning_effort=args.benchmark_reasoning_effort,
        benchmark_model_context_window=args.benchmark_model_context_window,
        benchmark_temperature=args.benchmark_temperature,
        base_env=source_env,
    )
    env = denovo_codex_environment(
        output=output,
        instance_id=inst.id,
        task_path=Path(args.data_file),
        workspace=workspace,
        base_env=source_env,
        stateful_session_id=(
            denovo_stateful_session_id(
                output=output,
                instance_id=inst.id,
                task_path=Path(args.data_file),
                workspace=workspace,
            )
            if args.agent_mode == "stateful"
            else None
        ),
    )
    codex_home = Path(env["CODEX_HOME"])
    seeded_auth = prepare_codex_environment(
        env,
        source_env=source_env,
        enable_stateful=args.agent_mode == "stateful",
        stateful_binary=args.stateful_binary,
        stateful_integration=(
            STATEFUL_INTEGRATION_FULL
            if args.agent_mode == "stateful"
            else STATEFUL_INTEGRATION_NONE
        ),
    )
```

3. Replace the execution call with:

```python
if args.cli_runtime == "omp":
    codex_summary = run_omp_with_timeout(
        command,
        workspace,
        env,
        timeout_seconds=args.codex_timeout_seconds,
    )
else:
    codex_summary = run_codex_with_timeout(
        command,
        prompt,
        workspace,
        env,
        max_resumes=args.max_resumes,
        timeout_seconds=args.codex_timeout_seconds,
    )
```

4. Use runtime-specific finish reasons:

```python
error_reason = "omp-error" if args.cli_runtime == "omp" else "codex-error"
error_message = f"{args.cli_runtime} exited {returncode}"
```

- [ ] **Step 5: Keep subagent enforcement Codex-only**

Change the native subagent requirement block to:

```python
if args.cli_runtime == "codex" and args.subagent == "on" and not subagent_usage["subagent_requirement_met"]:
    patch_path.write_text("", encoding="utf-8")
    orchestration_trace = capture_trace()
    finish_command_record(orchestration_trace)
    cleanup_stateful_repo_enable(workspace, stateful_repo_cleanup)
    stateful_repo_cleanup = None
    spawn_count = subagent_usage["native_subagent"]["subagent_spawn_count"]
    return InstanceResult(
        inst.id,
        False,
        None,
        "subagent-requirement-failed",
        (
            f"subagent:on requires at least {args.subagent_min_count} native Codex "
            f"subagent spawns; observed {spawn_count}"
        ),
        None,
        subagent_used=subagent_usage["subagent_used"],
        subagent_usage=subagent_usage,
        token_usage=token_usage,
        orchestration_trace=orchestration_trace,
    )
```

For OMP, keep `subagent_usage = empty_subagent_usage_metadata` or the existing `native_subagent_usage` result with `subagent_requirement_met` ignored. Do not count Codex DB files for OMP.

- [ ] **Step 6: Run focused tests and verify GREEN**

Run:

```bash
/Users/arthur/.cargo/bin/stateful sandbox run --fs build --network enabled --write-dir omp-cli-green-5 --command '/usr/bin/env CARGO_HOME=$TMPDIR/cargo-home cargo test -p stateful-bench denovo_codex_agent_'
```

Expected: PASS for the adapter helper tests selected by the `denovo_codex_agent_` filter.

- [ ] **Step 7: Commit Task 5**

```bash
/Users/arthur/.cargo/bin/stateful sandbox run --fs git --network disabled --command 'git add crates/stateful-bench/scripts/denovo_codex_agent.py crates/stateful-bench/tests/cli.rs'
/Users/arthur/.cargo/bin/stateful sandbox run --fs git --network disabled --command 'git commit -m "Run DeNovo through OMP CLI"'
```

---

### Task 6: Docs and final verification

**Files:**
- Modify: `docs/denovo-benchmark-commands.md`

- [ ] **Step 1: Update benchmark docs**

In `docs/denovo-benchmark-commands.md`, add this section after the command variables block:

```markdown
## OMP CLI Variant

Use `--agent omp-cli` to run the same DeNovoSWE condition matrix through OMP.
For OMP runs, use `deepseek-v4-flash` unless deliberately testing another model:

```bash
"$REPO_ROOT/target/debug/stateful-bench" denovo run \
  --agent omp-cli \
  --aweagent-root "$AWEAGENT_ROOT" \
  --python "$PYTHON" \
  --data-file "$REPO_ROOT/datasets/denovo/shards/denovoswe_public_shard_a.jsonl" \
  --output-dir "$REPO_ROOT/.stateful_bench/denovo/runs" \
  --run-id "$RUN_ID-shard-a-omp" \
  --mode batch \
  --condition stateful:off,subagent:off \
  --condition stateful:on,subagent:off \
  --condition stateful:off,subagent:on \
  --condition stateful:on,subagent:on \
  --omp-bin "${OMP_BIN:-omp}" \
  --stateful-binary "$STATEFUL_BIN" \
  --benchmark-model deepseek-v4-flash \
  --benchmark-reasoning-effort low \
  --benchmark-model-context-window 256000 \
  --benchmark-temperature 1 \
  --benchmark-max-turns 500 \
  --prompt-version v2 \
  --eval-iters 1
```

OMP `stateful:on` and `stateful:off` both use isolated OMP home/profile state.
They must not inherit host Codex config, active Codex session state, Codex rules,
or Codex skills. The only difference is whether the isolated OMP profile receives
stateful OMP install/config.
```

- [ ] **Step 2: Run targeted Rust tests**

Run:

```bash
/Users/arthur/.cargo/bin/stateful sandbox run --fs build --network enabled --write-dir omp-cli-final-tests --command '/usr/bin/env CARGO_HOME=$TMPDIR/cargo-home cargo test -p stateful-bench denovo_'
```

Expected: PASS.

- [ ] **Step 3: Run package tests for touched crate**

Run:

```bash
/Users/arthur/.cargo/bin/stateful sandbox run --fs build --network enabled --write-dir omp-cli-stateful-bench --command '/usr/bin/env CARGO_HOME=$TMPDIR/cargo-home cargo test -p stateful-bench'
```

Expected: PASS.

- [ ] **Step 4: Run formatting check**

Run:

```bash
/Users/arthur/.cargo/bin/stateful sandbox run --fs build --network enabled --write-dir omp-cli-fmt --command '/usr/bin/env CARGO_HOME=$TMPDIR/cargo-home cargo fmt --check'
```

Expected: PASS. If it fails, run `cargo fmt` with a write-authorized command or make exact native edits, then re-run this check.

- [ ] **Step 5: Commit docs and formatting fixes**

```bash
/Users/arthur/.cargo/bin/stateful sandbox run --fs git --network disabled --command 'git add docs/denovo-benchmark-commands.md crates/stateful-bench/src/denovo.rs crates/stateful-bench/src/lib.rs crates/stateful-bench/scripts/denovo_codex_agent.py crates/stateful-bench/tests/cli.rs crates/stateful-bench/tests/denovo.rs'
/Users/arthur/.cargo/bin/stateful sandbox run --fs git --network disabled --command 'git commit -m "Document DeNovo OMP CLI benchmark"'
```

- [ ] **Step 6: Final status**

Run:

```bash
/Users/arthur/.cargo/bin/stateful sandbox run --fs git --network disabled --command 'git status --short'
```

Expected: no output.
