use clap::Parser;
use stateful_bench::{Cli, Command, ReportFormat, RunMode};
use std::{fs, path::PathBuf, process::Command as ProcessCommand};

#[test]
fn fetch_command_parses_defaults_and_output_override() {
    let cli = Cli::try_parse_from([
        "stateful-bench",
        "fetch",
        "--output",
        ".stateful_bench/datasets/swe-bench-verified.jsonl",
    ])
    .expect("fetch command should parse");

    assert!(matches!(
        cli.command,
        Command::Fetch {
            ref dataset,
            ref config,
            ref split,
            ref output,
            page_size,
        } if dataset == "SWE-bench/SWE-bench_Verified"
            && config == "default"
            && split == "test"
            && output.to_string_lossy() == ".stateful_bench/datasets/swe-bench-verified.jsonl"
            && page_size == 100
    ));
}

#[test]
fn run_command_parses_mode_and_agent_template() {
    let cli = Cli::try_parse_from([
        "stateful-bench",
        "run",
        "--pairs",
        ".stateful_bench/pairs/dev-50.jsonl",
        "--mode",
        "no-state",
        "--run-id",
        "dev-no-state",
        "--agent-cmd-template",
        "agent --workspace {workspace} --task {task_json}",
        "--auth-check-cmd-template",
        "codex login status",
        "--budget-check-cmd-template",
        "test -n \"$STATEFUL_BENCH_BUDGET_OK\"",
        "--pair-id",
        "pair-1/pair-2",
        "--pair-id",
        "pair-3/pair-4",
        "--jobs",
        "4",
    ])
    .expect("run command should parse");

    assert!(matches!(
        cli.command,
        Command::Run {
            mode: RunMode::NoState,
            ref run_id,
            ref agent_cmd_template,
            ref auth_check_cmd_template,
            ref budget_check_cmd_template,
            ref pair_id,
            jobs,
            ..
        } if run_id == "dev-no-state"
            && agent_cmd_template == "agent --workspace {workspace} --task {task_json}"
            && auth_check_cmd_template.as_deref() == Some("codex login status")
            && budget_check_cmd_template.as_deref() == Some("test -n \"$STATEFUL_BENCH_BUDGET_OK\"")
            && pair_id == &vec!["pair-1/pair-2".to_string(), "pair-3/pair-4".to_string()]
            && jobs == 4
    ));
}

#[test]
fn generate_fallback_preflight_command_parses_assume_clean_apply() {
    let cli = Cli::try_parse_from([
        "stateful-bench",
        "generate-fallback-preflight",
        "--dataset",
        ".stateful_bench/datasets/swe-bench-verified.jsonl",
        "--output",
        ".stateful_bench/pairs/same-version-preflight.jsonl",
        "--assume-clean-apply",
    ])
    .expect("generate fallback preflight command should parse");

    assert!(matches!(
        cli.command,
        Command::GenerateFallbackPreflight {
            ref dataset,
            ref output,
            assume_clean_apply,
        } if dataset.to_string_lossy() == ".stateful_bench/datasets/swe-bench-verified.jsonl"
            && output.to_string_lossy() == ".stateful_bench/pairs/same-version-preflight.jsonl"
            && assume_clean_apply
    ));
}

#[test]
fn compare_command_parses_run_dirs_manifest_and_format() {
    let cli = Cli::try_parse_from([
        "stateful-bench",
        "compare",
        "--stateful-run-dir",
        ".stateful_bench/runs/dev-stateful",
        "--stateful-run-dir",
        ".stateful_bench/runs/dev-stateful-remainder",
        "--no-state-run-dir",
        ".stateful_bench/runs/dev-no-state",
        "--manifest",
        ".stateful_bench/pairs/dev-30.jsonl",
        "--max-pairs",
        "30",
        "--format",
        "markdown",
        "--output",
        ".stateful_bench/runs/dev-compare.md",
    ])
    .expect("compare command should parse");

    assert!(matches!(
        cli.command,
        Command::Compare {
            ref stateful_run_dir,
            ref no_state_run_dir,
            ref manifest,
            max_pairs: Some(30),
            format: ReportFormat::Markdown,
            ref output,
        } if stateful_run_dir.len() == 2
            && stateful_run_dir[0].to_string_lossy() == ".stateful_bench/runs/dev-stateful"
            && stateful_run_dir[1].to_string_lossy() == ".stateful_bench/runs/dev-stateful-remainder"
            && no_state_run_dir.len() == 1
            && no_state_run_dir[0].to_string_lossy() == ".stateful_bench/runs/dev-no-state"
            && manifest.to_string_lossy() == ".stateful_bench/pairs/dev-30.jsonl"
            && output.as_ref().is_some_and(|path| path.to_string_lossy() == ".stateful_bench/runs/dev-compare.md")
    ));
}

#[test]
fn synthetic_command_parses_run_id_output_dir_and_report_output() {
    let cli = Cli::try_parse_from([
        "stateful-bench",
        "synthetic",
        "--output-dir",
        ".stateful_bench/synthetic",
        "--run-id",
        "dev-synthetic",
        "--format",
        "markdown",
        "--output",
        ".stateful_bench/synthetic/comparison.md",
    ])
    .expect("synthetic command should parse");

    assert!(matches!(
        cli.command,
        Command::Synthetic {
            ref output_dir,
            ref run_id,
            format: ReportFormat::Markdown,
            ref output,
        } if output_dir.to_string_lossy() == ".stateful_bench/synthetic"
            && run_id == "dev-synthetic"
            && output.as_ref().is_some_and(|path| path.to_string_lossy() == ".stateful_bench/synthetic/comparison.md")
    ));
}

#[test]
fn codex_pair_agent_accepts_explicit_stateful_session_arguments() {
    let output = ProcessCommand::new("python3")
        .args([
            concat!(env!("CARGO_MANIFEST_DIR"), "/scripts/codex_pair_agent.py"),
            "--help",
        ])
        .output()
        .expect("codex pair agent help should run");

    assert!(
        output.status.success(),
        "help command failed with status {:?}: {}",
        output.status.code(),
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).expect("help output should be utf-8");
    assert!(stdout.contains("--session-id"));
    assert!(stdout.contains("--workspace-id"));
    assert!(stdout.contains("--benchmark-model"));
    assert!(stdout.contains("--benchmark-reasoning-effort"));
}

#[test]
fn codex_pair_agent_switches_inner_sandbox_for_nested_benchmark() {
    let script = format!(
        r#"
import importlib.util
import json
from pathlib import Path

spec = importlib.util.spec_from_file_location("codex_pair_agent_for_test", {agent_path})
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)

default_command = module.codex_command(
    workspace=Path("/tmp/workspace"),
    mode="no-state",
    stateful_binary="/tmp/stateful",
    base_env={{}},
)
nested_command = module.codex_command(
    workspace=Path("/tmp/workspace"),
    mode="no-state",
    stateful_binary="/tmp/stateful",
    base_env={{"STATEFUL_NESTED_CODEX_HOME_ROOT": "/repo/target/nested-codex-homes"}},
)
print(json.dumps({{"default": default_command, "nested": nested_command}}))
"#,
        agent_path = codex_pair_agent_path_json(),
    );
    let output = run_python_json(&script);
    let default_command = output["default"].as_array().expect("default command");
    let nested_command = output["nested"].as_array().expect("nested command");

    assert_eq!(
        command_arg_after(default_command, "--sandbox"),
        Some("workspace-write")
    );
    assert!(command_contains(
        default_command,
        "sandbox_workspace_write.network_access=true"
    ));
    assert_eq!(
        command_arg_after(nested_command, "--sandbox"),
        Some("danger-full-access")
    );
    assert!(!command_contains(
        nested_command,
        "sandbox_workspace_write.network_access=true"
    ));
}

#[test]
fn codex_pair_agent_builds_nested_codex_environment() {
    let script = format!(
        r#"
import importlib.util
import json
from pathlib import Path

spec = importlib.util.spec_from_file_location("codex_pair_agent_env_test", {agent_path})
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)

env = module.codex_environment(
    task_path=Path("/repo/runs/pair one/workspace/.stateful_bench/task-a.json"),
    workspace=Path("/repo/runs/pair one/workspace"),
    base_env={{
        "PATH": "/bin",
        "STATEFUL_NESTED_CODEX_HOME_ROOT": "/repo/target/nested-codex-homes",
    }},
)
print(json.dumps(env, sort_keys=True))
"#,
        agent_path = codex_pair_agent_path_json(),
    );
    let output = run_python_json(&script);

    assert_eq!(
        output["HOME"],
        "/repo/target/nested-codex-homes/pair-one/task-a/home"
    );
    assert_eq!(
        output["CODEX_HOME"],
        "/repo/target/nested-codex-homes/pair-one/task-a/home/.codex"
    );
    assert_eq!(
        output["XDG_CONFIG_HOME"],
        "/repo/target/nested-codex-homes/pair-one/task-a/home/.config"
    );
    assert_eq!(
        output["XDG_CACHE_HOME"],
        "/repo/target/nested-codex-homes/pair-one/task-a/home/.cache"
    );
    assert_eq!(output["PATH"], "/bin");
}

#[test]
fn codex_pair_agent_seeds_and_cleans_nested_auth() {
    let temp_dir = target_temp_dir("stateful-bench-codex-pair-agent-auth");
    let script = format!(
        r#"
import importlib.util
import json
from pathlib import Path

spec = importlib.util.spec_from_file_location("codex_pair_agent_auth_test", {agent_path})
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)

root = Path({temp_dir})
source_home = root / "source-home"
source_auth = source_home / ".codex" / "auth.json"
source_auth.parent.mkdir(parents=True, exist_ok=True)
source_auth.write_text("{{\"token\":\"source\"}}")

workspace = root / "runs" / "pair-one" / "workspace"
task_path = workspace / ".stateful_bench" / "task-a.json"
source_env = {{
    "PATH": "/bin",
    "HOME": str(source_home),
    "STATEFUL_NESTED_CODEX_HOME_ROOT": str(root / "nested-codex-homes"),
}}
env = module.codex_environment(task_path=task_path, workspace=workspace, base_env=source_env)
seeded = module.prepare_codex_environment(env, source_env=source_env)
target_auth = Path(env["CODEX_HOME"]) / "auth.json"
copied = target_auth.read_text()
module.cleanup_seeded_auth(seeded)
print(json.dumps({{
    "copied": copied,
    "seeded": str(seeded.path if seeded else None),
    "target_exists_after_cleanup": target_auth.exists(),
    "source_exists_after_cleanup": source_auth.exists(),
}}))
"#,
        agent_path = codex_pair_agent_path_json(),
        temp_dir = serde_json::to_string(&temp_dir.to_string_lossy())
            .expect("temp dir should encode as json"),
    );
    let output = run_python_json(&script);

    assert_eq!(output["copied"], "{\"token\":\"source\"}");
    assert_eq!(output["target_exists_after_cleanup"], false);
    assert_eq!(output["source_exists_after_cleanup"], true);

    fs::remove_dir_all(temp_dir).expect("temp dir should clean up");
}

#[test]
fn codex_pair_agent_auth_seed_copy_failure_is_best_effort() {
    let temp_dir = target_temp_dir("stateful-bench-codex-pair-agent-auth-failure");
    let script = format!(
        r#"
import importlib.util
import json
import shutil
from pathlib import Path

spec = importlib.util.spec_from_file_location("codex_pair_agent_auth_failure_test", {agent_path})
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)

root = Path({temp_dir})
source_home = root / "source-home"
source_auth = source_home / ".codex" / "auth.json"
source_auth.parent.mkdir(parents=True, exist_ok=True)
source_auth.write_text("{{\"token\":\"source\"}}")

workspace = root / "runs" / "pair-one" / "workspace"
task_path = workspace / ".stateful_bench" / "task-a.json"
source_env = {{
    "PATH": "/bin",
    "HOME": str(source_home),
    "STATEFUL_NESTED_CODEX_HOME_ROOT": str(root / "nested-codex-homes"),
}}
env = module.codex_environment(task_path=task_path, workspace=workspace, base_env=source_env)

def fail_copy(*_args, **_kwargs):
    raise OSError("simulated copy failure")

original_copy2 = module.shutil.copy2
module.shutil.copy2 = fail_copy
try:
    seeded = module.prepare_codex_environment(env, source_env=source_env)
finally:
    module.shutil.copy2 = original_copy2

target_auth = Path(env["CODEX_HOME"]) / "auth.json"
print(json.dumps({{
    "seeded": seeded is None,
    "target_exists": target_auth.exists(),
    "source_exists": source_auth.exists(),
}}))
"#,
        agent_path = codex_pair_agent_path_json(),
        temp_dir = serde_json::to_string(&temp_dir.to_string_lossy())
            .expect("temp dir should encode as json"),
    );
    let output = run_python_json(&script);

    assert_eq!(output["seeded"], true);
    assert_eq!(output["target_exists"], false);
    assert_eq!(output["source_exists"], true);

    fs::remove_dir_all(temp_dir).expect("temp dir should clean up");
}

#[test]
fn codex_pair_agent_replaces_stale_nested_auth_and_removes_only_unchanged_seed() {
    let temp_dir = target_temp_dir("stateful-bench-codex-pair-agent-stale-auth");
    let script = format!(
        r#"
import importlib.util
import json
from pathlib import Path

spec = importlib.util.spec_from_file_location("codex_pair_agent_stale_auth_test", {agent_path})
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)

root = Path({temp_dir})
source_home = root / "source-home"
source_auth = source_home / ".codex" / "auth.json"
source_auth.parent.mkdir(parents=True, exist_ok=True)
source_auth.write_text("{{\"token\":\"current\"}}")

workspace = root / "runs" / "pair-one" / "workspace"
task_path = workspace / ".stateful_bench" / "task-a.json"
source_env = {{
    "PATH": "/bin",
    "HOME": str(source_home),
    "STATEFUL_NESTED_CODEX_HOME_ROOT": str(root / "nested-codex-homes"),
}}
env = module.codex_environment(task_path=task_path, workspace=workspace, base_env=source_env)
target_auth = Path(env["CODEX_HOME"]) / "auth.json"
target_auth.parent.mkdir(parents=True, exist_ok=True)
target_auth.write_text("{{\"token\":\"stale\"}}")

seeded = module.prepare_codex_environment(env, source_env=source_env)
copied = target_auth.read_text()
module.cleanup_seeded_auth(seeded)
print(json.dumps({{
    "copied": copied,
    "target_exists_after_cleanup": target_auth.exists(),
    "source_exists_after_cleanup": source_auth.exists(),
}}))
"#,
        agent_path = codex_pair_agent_path_json(),
        temp_dir = serde_json::to_string(&temp_dir.to_string_lossy())
            .expect("temp dir should encode as json"),
    );
    let output = run_python_json(&script);

    assert_eq!(output["copied"], "{\"token\":\"current\"}");
    assert_eq!(output["target_exists_after_cleanup"], false);
    assert_eq!(output["source_exists_after_cleanup"], true);

    fs::remove_dir_all(temp_dir).expect("temp dir should clean up");
}

#[test]
fn codex_pair_agent_keeps_child_replaced_auth_during_cleanup() {
    let temp_dir = target_temp_dir("stateful-bench-codex-pair-agent-child-auth");
    let script = format!(
        r#"
import importlib.util
import json
from pathlib import Path

spec = importlib.util.spec_from_file_location("codex_pair_agent_child_auth_test", {agent_path})
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)

root = Path({temp_dir})
source_home = root / "source-home"
source_auth = source_home / ".codex" / "auth.json"
source_auth.parent.mkdir(parents=True, exist_ok=True)
source_auth.write_text("{{\"token\":\"source\"}}")

workspace = root / "runs" / "pair-one" / "workspace"
task_path = workspace / ".stateful_bench" / "task-a.json"
source_env = {{
    "PATH": "/bin",
    "HOME": str(source_home),
    "STATEFUL_NESTED_CODEX_HOME_ROOT": str(root / "nested-codex-homes"),
}}
env = module.codex_environment(task_path=task_path, workspace=workspace, base_env=source_env)
seeded = module.prepare_codex_environment(env, source_env=source_env)
target_auth = Path(env["CODEX_HOME"]) / "auth.json"
target_auth.write_text("{{\"token\":\"child\"}}")
module.cleanup_seeded_auth(seeded)
print(json.dumps({{
    "target_exists_after_cleanup": target_auth.exists(),
    "target_contents": target_auth.read_text(),
    "source_exists_after_cleanup": source_auth.exists(),
}}))
"#,
        agent_path = codex_pair_agent_path_json(),
        temp_dir = serde_json::to_string(&temp_dir.to_string_lossy())
            .expect("temp dir should encode as json"),
    );
    let output = run_python_json(&script);

    assert_eq!(output["target_exists_after_cleanup"], true);
    assert_eq!(output["target_contents"], "{\"token\":\"child\"}");
    assert_eq!(output["source_exists_after_cleanup"], true);

    fs::remove_dir_all(temp_dir).expect("temp dir should clean up");
}

#[test]
fn codex_pair_agent_rejects_symlinked_nested_home_parent() {
    let temp_dir = target_temp_dir("stateful-bench-codex-pair-agent-symlink-auth");
    let script = format!(
        r#"
import importlib.util
import json
from pathlib import Path

spec = importlib.util.spec_from_file_location("codex_pair_agent_symlink_auth_test", {agent_path})
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)

root = Path({temp_dir})
workspace = root / "runs" / "pair-one" / "workspace"
task_path = workspace / ".stateful_bench" / "task-a.json"
source_env = {{
    "PATH": "/bin",
    "STATEFUL_NESTED_CODEX_HOME_ROOT": str(root / "nested-codex-homes"),
}}
home_parent = root / "nested-codex-homes" / "pair-one"
home_parent.parent.mkdir(parents=True, exist_ok=True)
(root / "outside-home").mkdir()
home_parent.symlink_to(root / "outside-home", target_is_directory=True)

env = module.codex_environment(task_path=task_path, workspace=workspace, base_env=source_env)
try:
    module.prepare_codex_environment(env, source_env=source_env)
    rejected = False
except module.UnsafeNestedCodexHome:
    rejected = True

print(json.dumps({{
    "rejected": rejected,
    "outside_codex_exists": (root / "outside-home" / "task-a" / "home" / ".codex").exists(),
}}))
"#,
        agent_path = codex_pair_agent_path_json(),
        temp_dir = serde_json::to_string(&temp_dir.to_string_lossy())
            .expect("temp dir should encode as json"),
    );
    let output = run_python_json(&script);

    assert_eq!(output["rejected"], true);
    assert_eq!(output["outside_codex_exists"], false);

    fs::remove_dir_all(temp_dir).expect("temp dir should clean up");
}

fn codex_pair_agent_path_json() -> String {
    serde_json::to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/scripts/codex_pair_agent.py"
    ))
    .expect("agent path should encode as json")
}

fn run_python_json(script: &str) -> serde_json::Value {
    let output = ProcessCommand::new("python3")
        .args(["-c", script])
        .output()
        .expect("python script should run");

    assert!(
        output.status.success(),
        "python script failed with status {:?}: stdout={} stderr={}",
        output.status.code(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).expect("python script should print json")
}

fn command_arg_after<'a>(command: &'a [serde_json::Value], flag: &str) -> Option<&'a str> {
    command
        .iter()
        .position(|value| value.as_str() == Some(flag))
        .and_then(|index| command.get(index + 1))
        .and_then(serde_json::Value::as_str)
}

fn command_contains(command: &[serde_json::Value], expected: &str) -> bool {
    command.iter().any(|value| value.as_str() == Some(expected))
}

fn target_temp_dir(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../target")
        .join(format!("{name}-{}", std::process::id()));
    if dir.exists() {
        fs::remove_dir_all(&dir).expect("old temp dir should clean up");
    }
    fs::create_dir_all(&dir).expect("temp dir should be created");
    dir
}
