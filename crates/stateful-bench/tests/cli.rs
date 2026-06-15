use clap::Parser;
use stateful_bench::{Cli, Command, DeNovoCommand, DeNovoRunMode, ReportFormat, RunMode};
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
fn denovo_extract_command_parses_official_recipe_options() {
    let cli = Cli::try_parse_from([
        "stateful-bench",
        "denovo",
        "extract",
        "--aweagent-root",
        "../AweAgent",
        "--input",
        "ready_denovoswe.jsonl",
        "--output",
        ".stateful_bench/denovo/extracts",
        "--config",
        "configs/tasks/denovoswe.yaml",
        "--max-concurrent",
        "10",
        "--instance-id",
        "PyCQA_pep8_pr970",
        "--dry-run",
        "--del-done-images",
        "--no-extract-package-info",
    ])
    .expect("denovo extract command should parse");

    assert!(matches!(
        cli.command,
        Command::Denovo {
            command: DeNovoCommand::Extract {
                ref aweagent_root,
                ref input,
                ref output,
                ref config,
                max_concurrent: Some(10),
                ref instance_id,
                dry_run: true,
                del_done_images: true,
                no_extract_package_info: true,
                ..
            }
        } if aweagent_root.as_deref() == Some(std::path::Path::new("../AweAgent"))
            && input.to_string_lossy() == "ready_denovoswe.jsonl"
            && output.to_string_lossy() == ".stateful_bench/denovo/extracts"
            && config.to_string_lossy() == "configs/tasks/denovoswe.yaml"
            && instance_id == &vec!["PyCQA_pep8_pr970".to_string()]
    ));
}

#[test]
fn denovo_run_command_parses_condition_matrix_and_official_options() {
    let cli = Cli::try_parse_from([
        "stateful-bench",
        "denovo",
        "run",
        "--aweagent-root",
        "../AweAgent",
        "--data-file",
        ".stateful_bench/denovo/extracts/dev/results.jsonl",
        "--output-dir",
        ".stateful_bench/denovo/runs",
        "--run-id",
        "dev-denovo",
        "--mode",
        "batch",
        "--condition",
        "stateful:on,subagent:off,config:configs/tasks/denovoswe-stateful.yaml",
        "--llm-config",
        "configs/llm/openai.yaml",
        "--model",
        "gpt-5",
        "--max-steps",
        "500",
        "--max-concurrent",
        "4",
        "--instance-id",
        "PyCQA_pep8_pr970",
        "--eval-iters",
        "2",
        "--prompt-version",
        "v2",
        "--no-search",
    ])
    .expect("denovo run command should parse");

    assert!(matches!(
        cli.command,
        Command::Denovo {
            command: DeNovoCommand::Run {
                mode: DeNovoRunMode::Batch,
                ref run_id,
                ref condition,
                ref llm_config,
                ref model,
                max_steps: Some(500),
                max_concurrent: Some(4),
                ref instance_id,
                eval_iters: 2,
                ref prompt_version,
                enable_search: false,
                no_search: true,
                ..
            }
        } if run_id == "dev-denovo"
            && condition == &vec!["stateful:on,subagent:off,config:configs/tasks/denovoswe-stateful.yaml".to_string()]
            && llm_config.as_deref() == Some(std::path::Path::new("configs/llm/openai.yaml"))
            && model.as_deref() == Some("gpt-5")
            && instance_id == &vec!["PyCQA_pep8_pr970".to_string()]
            && prompt_version == "v2"
    ));
}

#[test]
fn denovo_run_command_parses_codex_cli_agent_options() {
    let cli = Cli::try_parse_from([
        "stateful-bench",
        "denovo",
        "run",
        "--agent",
        "codex-cli",
        "--aweagent-root",
        "../AweAgent",
        "--data-file",
        ".stateful_bench/denovo/extracts/dev/results.jsonl",
        "--output-dir",
        "target/stateful-bench/denovo/runs",
        "--run-id",
        "dev-denovo-codex",
        "--condition",
        "stateful:off,subagent:on",
        "--condition",
        "stateful:on,subagent:on",
        "--codex-bin",
        "/opt/homebrew/bin/codex",
        "--stateful-binary",
        "/Users/arthur/.cargo/bin/stateful",
        "--benchmark-model",
        "gpt-5.4-mini",
        "--benchmark-reasoning-effort",
        "low",
        "--benchmark-model-context-window",
        "256000",
        "--benchmark-temperature",
        "1",
        "--benchmark-max-turns",
        "500",
        "--max-resumes",
        "2",
        "--codex-timeout-seconds",
        "7200",
        "--codex-adapter-script",
        "crates/stateful-bench/scripts/denovo_codex_agent.py",
    ])
    .expect("denovo codex run command should parse");

    assert!(matches!(
        cli.command,
        Command::Denovo {
            command: DeNovoCommand::Run {
                agent: stateful_bench::DeNovoAgentKind::CodexCli,
                ref run_id,
                ref condition,
                ref codex_bin,
                ref stateful_binary,
                ref benchmark_model,
                ref benchmark_reasoning_effort,
                benchmark_model_context_window: 256000,
                ref benchmark_temperature,
                benchmark_max_turns: 500,
                max_resumes: 2,
                codex_timeout_seconds: 7200,
                ref codex_adapter_script,
                ..
            }
        } if run_id == "dev-denovo-codex"
            && condition == &vec![
                "stateful:off,subagent:on".to_string(),
                "stateful:on,subagent:on".to_string(),
            ]
            && codex_bin == "/opt/homebrew/bin/codex"
            && stateful_binary == "/Users/arthur/.cargo/bin/stateful"
            && benchmark_model == "gpt-5.4-mini"
            && benchmark_reasoning_effort == "low"
            && benchmark_temperature == "1"
            && codex_adapter_script.as_deref()
                == Some(std::path::Path::new("crates/stateful-bench/scripts/denovo_codex_agent.py"))
    ));
}

#[test]
fn denovo_report_and_compare_commands_parse_outputs() {
    let report_cli = Cli::try_parse_from([
        "stateful-bench",
        "denovo",
        "report",
        "--run-dir",
        ".stateful_bench/denovo/runs/dev-denovo",
        "--output",
        ".stateful_bench/denovo/runs/dev-denovo/report.json",
    ])
    .expect("denovo report command should parse");

    assert!(matches!(
        report_cli.command,
        Command::Denovo {
            command: DeNovoCommand::Report {
                ref run_dir,
                ref output,
                ..
            }
        } if run_dir.to_string_lossy() == ".stateful_bench/denovo/runs/dev-denovo"
            && output.as_ref().is_some_and(|path| path.to_string_lossy() == ".stateful_bench/denovo/runs/dev-denovo/report.json")
    ));

    let compare_cli = Cli::try_parse_from([
        "stateful-bench",
        "denovo",
        "compare",
        "--report",
        ".stateful_bench/denovo/runs/dev-denovo/conditions/stateful-off_subagent-off/denovo-report.json",
        "--report",
        ".stateful_bench/denovo/runs/dev-denovo/conditions/stateful-on_subagent-off/denovo-report.json",
        "--output",
        ".stateful_bench/denovo/runs/dev-denovo/comparison.json",
    ])
    .expect("denovo compare command should parse");

    assert!(matches!(
        compare_cli.command,
        Command::Denovo {
            command: DeNovoCommand::Compare {
                ref report,
                ref output,
                ..
            }
        } if report.len() == 2
            && output.as_ref().is_some_and(|path| path.to_string_lossy() == ".stateful_bench/denovo/runs/dev-denovo/comparison.json")
    ));
}

#[test]
fn run_command_exits_nonzero_when_no_pairs_are_scored() {
    let temp_dir = target_temp_dir("stateful-bench-cli-no-scored-pairs");
    let pairs_path = temp_dir.join("pairs.jsonl");
    let output_dir = temp_dir.join("runs");
    fs::write(
        &pairs_path,
        serde_json::json!({
            "pair_id": "pair-1/pair-2",
            "repo": "example/repo",
            "base_commit": "base",
            "version": "1.0",
            "eligibility": "same_base_commit",
            "class": "same_repo_disjoint",
            "task_a_files": ["agent-a.txt"],
            "task_b_files": ["agent-b.txt"],
            "task_a": {
                "instance_id": "pair-1",
                "repo": "example/repo",
                "base_commit": "base",
                "problem_statement": "Edit a file",
                "version": "1.0",
                "patch": "",
                "test_patch": "",
                "FAIL_TO_PASS": [],
                "PASS_TO_PASS": []
            },
            "task_b": {
                "instance_id": "pair-2",
                "repo": "example/repo",
                "base_commit": "base",
                "problem_statement": "Edit a file",
                "version": "1.0",
                "patch": "",
                "test_patch": "",
                "FAIL_TO_PASS": [],
                "PASS_TO_PASS": []
            }
        })
        .to_string()
            + "\n",
    )
    .expect("pair manifest should write");

    let output = ProcessCommand::new(env!("CARGO_BIN_EXE_stateful-bench"))
        .args([
            "run",
            "--pairs",
            pairs_path.to_str().expect("pairs path should be utf-8"),
            "--mode",
            "no-state",
            "--run-id",
            "all-setup-errors",
            "--agent-cmd-template",
            "true",
            "--output-dir",
            output_dir.to_str().expect("output dir should be utf-8"),
            "--setup-cmd-template",
            "false",
        ])
        .output()
        .expect("stateful-bench run command should execute");

    assert!(
        !output.status.success(),
        "all-setup-error run should fail: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("no scored pairs"),
        "stderr should explain no scored pairs: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        output_dir
            .join("all-setup-errors/pair-1-pair-2/pair-run.json")
            .is_file(),
        "run artifacts should still be recorded"
    );

    fs::remove_dir_all(temp_dir).expect("temp dir should clean up");
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
    assert!(stdout.contains("--benchmark-model-context-window"));
    assert!(stdout.contains("--benchmark-max-turns"));
    assert!(stdout.contains("--enable-native-subagent"));
    assert!(stdout.contains("--disable-bundled-skills"));
    assert!(stdout.contains("--stateful-integration"));
    assert!(stdout.contains("--max-resumes"));
}

#[test]
fn denovo_codex_agent_prompt_records_benchmark_constraints() {
    let script = format!(
        r#"
import importlib.util
import json
import sys

spec = importlib.util.spec_from_file_location("denovo_codex_agent_prompt_test", {agent_path})
mod = importlib.util.module_from_spec(spec)
sys.modules[spec.name] = mod
spec.loader.exec_module(mod)

prompt = mod.build_codex_prompt(
    instance_id="fake-a",
    document="Build a parser package.",
    benchmark_max_turns=500,
    max_steps=500,
    prompt_version="v1",
)
assert "Build a parser package." in prompt
assert "Benchmark max turns: 500" in prompt
assert "Maximum task steps: 500" in prompt
assert "Do not edit benchmark artifacts" in prompt
print(json.dumps({{"prompt": prompt}}))
"#,
        agent_path = denovo_codex_agent_path_json(),
    );
    let output = run_python_json(&script);
    assert!(
        output["prompt"]
            .as_str()
            .expect("prompt should be a string")
            .contains("fake-a")
    );
}

#[test]
fn denovo_codex_agent_git_diff_includes_new_and_modified_files() {
    let dir = target_temp_dir("denovo-codex-git-diff");
    let workspace = dir.join("workspace");
    let script = format!(
        r#"
import importlib.util
import json
import subprocess
import sys
from pathlib import Path

spec = importlib.util.spec_from_file_location("denovo_codex_agent_git_diff_test", {agent_path})
mod = importlib.util.module_from_spec(spec)
sys.modules[spec.name] = mod
spec.loader.exec_module(mod)

workspace = Path({workspace})
workspace.mkdir(parents=True, exist_ok=True)
subprocess.run(["git", "init"], cwd=workspace, check=True, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
subprocess.run(["git", "config", "user.email", "bench@example.test"], cwd=workspace, check=True)
subprocess.run(["git", "config", "user.name", "Bench Test"], cwd=workspace, check=True)
(workspace / "tracked.txt").write_text("old line\n", encoding="utf-8")
subprocess.run(["git", "add", "tracked.txt"], cwd=workspace, check=True)
subprocess.run(["git", "commit", "-m", "initial"], cwd=workspace, check=True, stdout=subprocess.PIPE, stderr=subprocess.PIPE)

(workspace / "tracked.txt").write_text("modified line\n", encoding="utf-8")
(workspace / "new_file.txt").write_text("created from untracked\n", encoding="utf-8")
patch = mod.git_diff(workspace)
assert "diff --git a/new_file.txt b/new_file.txt" in patch
assert "new file mode" in patch
assert "+created from untracked" in patch
assert "-old line" in patch
assert "+modified line" in patch
print(json.dumps({{"patch": patch}}))
"#,
        agent_path = denovo_codex_agent_path_json(),
        workspace = serde_json::to_string(&workspace).expect("workspace path should serialize"),
    );
    let output = run_python_json(&script);
    assert!(
        output["patch"]
            .as_str()
            .expect("patch should be a string")
            .contains("new_file.txt")
    );
    fs::remove_dir_all(&dir).expect("temp git diff workspace should clean up");
}

#[test]
fn denovo_codex_agent_timeout_wrapper_bounds_run() {
    let script = format!(
        r#"
import importlib.util
import json
import subprocess
import sys
from pathlib import Path

spec = importlib.util.spec_from_file_location("denovo_codex_agent_timeout_test", {agent_path})
mod = importlib.util.module_from_spec(spec)
sys.modules[spec.name] = mod
spec.loader.exec_module(mod)

def fast_runner(command, **kwargs):
    return subprocess.CompletedProcess(command, 0, "", "")

def timeout_runner(command, **kwargs):
    raise subprocess.TimeoutExpired(command, kwargs.get("timeout"))

fast = mod.run_codex_with_timeout(
    ["codex", "exec", "-"],
    "prompt",
    Path("/tmp"),
    None,
    max_resumes=0,
    timeout_seconds=1,
    runner=fast_runner,
)
try:
    mod.run_codex_with_timeout(
        ["codex", "exec", "-"],
        "prompt",
        Path("/tmp"),
        None,
        max_resumes=0,
        timeout_seconds=0.25,
        runner=timeout_runner,
    )
except mod.CodexTimeoutError as error:
    timeout_message = str(error)
else:
    raise AssertionError("expected CodexTimeoutError")

assert fast == 0
assert "codex timed out after 0.25s" in timeout_message
print(json.dumps({{"fast": fast, "timeout": timeout_message}}))
"#,
        agent_path = denovo_codex_agent_path_json(),
    );
    let output = run_python_json(&script);
    assert_eq!(output["fast"], 0);
    assert!(
        output["timeout"]
            .as_str()
            .expect("timeout should be a string")
            .contains("0.25s")
    );
}

#[test]
fn denovo_codex_agent_safe_extract_rejects_symlink_members() {
    let dir = target_temp_dir("denovo-codex-safe-extract");
    let script = format!(
        r#"
import importlib.util
import io
import json
import sys
import tarfile
from pathlib import Path

spec = importlib.util.spec_from_file_location("denovo_codex_agent_tar_test", {agent_path})
mod = importlib.util.module_from_spec(spec)
sys.modules[spec.name] = mod
spec.loader.exec_module(mod)

destination = Path({destination})
destination.mkdir(parents=True, exist_ok=True)
buffer = io.BytesIO()
with tarfile.open(fileobj=buffer, mode="w") as tar:
    info = tarfile.TarInfo("link-out")
    info.type = tarfile.SYMTYPE
    info.linkname = "/etc/passwd"
    tar.addfile(info)

buffer.seek(0)
try:
    with tarfile.open(fileobj=buffer, mode="r") as tar:
        mod._safe_extract_tar(tar, destination)
except RuntimeError as error:
    message = str(error)
else:
    raise AssertionError("expected symlink member to be rejected")

assert "unsafe archive link" in message
print(json.dumps({{"message": message}}))
"#,
        agent_path = denovo_codex_agent_path_json(),
        destination = serde_json::to_string(&dir).expect("destination should serialize"),
    );
    let output = run_python_json(&script);
    assert!(
        output["message"]
            .as_str()
            .expect("message should be a string")
            .contains("link-out")
    );
    fs::remove_dir_all(&dir).expect("temp tar workspace should clean up");
}

#[test]
fn denovo_codex_agent_builds_no_state_and_stateful_commands() {
    let script = format!(
        r#"
import importlib.util
import json
import sys
from pathlib import Path

spec = importlib.util.spec_from_file_location("denovo_codex_agent_command_test", {agent_path})
module = importlib.util.module_from_spec(spec)
sys.modules[spec.name] = module
spec.loader.exec_module(module)

kwargs = {{
    "workspace": Path("/tmp/workspace"),
    "subagent": "on",
    "codex_bin": "/opt/homebrew/bin/codex",
    "stateful_binary": "/Users/arthur/.cargo/bin/stateful",
    "benchmark_model": "gpt-5.4-mini",
    "benchmark_reasoning_effort": "low",
    "benchmark_model_context_window": 256000,
    "benchmark_temperature": "1",
}}
no_state = module.codex_command_for_profile(agent_mode="no-state", **kwargs)
stateful = module.codex_command_for_profile(agent_mode="stateful", **kwargs)
print(json.dumps({{"no_state": no_state, "stateful": stateful}}))
"#,
        agent_path = denovo_codex_agent_path_json(),
    );
    let output = run_python_json(&script);
    let no_state = output["no_state"]
        .as_array()
        .expect("no-state command should be an array");
    let stateful = output["stateful"]
        .as_array()
        .expect("stateful command should be an array");

    assert_eq!(no_state[0], "/opt/homebrew/bin/codex");
    assert!(command_contains(no_state, "--ignore-user-config"));
    assert!(command_contains(no_state, "--ignore-rules"));
    assert!(command_contains(no_state, "skills.bundled.enabled=false"));
    assert!(command_contains(no_state, "features.multi_agent=true"));
    assert!(command_contains(no_state, "temperature=1"));

    assert_eq!(stateful[0], "/opt/homebrew/bin/codex");
    assert!(!command_contains(stateful, "--ignore-user-config"));
    assert!(command_contains(stateful, "--ignore-rules"));
    assert!(command_contains(stateful, "skills.bundled.enabled=false"));
    assert!(command_contains(stateful, "features.multi_agent=true"));
}

#[test]
fn denovo_codex_agent_prepares_local_isolated_profiles_without_nested_root() {
    let temp_dir = target_temp_dir("stateful-bench-denovo-codex-local-profiles");
    let script = format!(
        r#"
import importlib.util
import json
import sys
from pathlib import Path

spec = importlib.util.spec_from_file_location("denovo_codex_agent_local_profiles_test", {agent_path})
module = importlib.util.module_from_spec(spec)
sys.modules[spec.name] = module
spec.loader.exec_module(module)

root = Path({temp_dir})
source_home = root / "source-home"
source_auth = source_home / ".codex" / "auth.json"
source_auth.parent.mkdir(parents=True, exist_ok=True)
source_auth.write_text("{{\"token\":\"source\"}}")
source_env = {{
    "HOME": str(source_home),
    "PATH": "/bin",
}}
output = root / "adapter-output"
workspace = root / "workspace"
workspace.mkdir(parents=True, exist_ok=True)
task_path = root / "extracts" / "results.jsonl"

no_state_env = module.denovo_codex_environment(
    output=output,
    instance_id="issue/no-state",
    task_path=task_path,
    workspace=workspace,
    base_env=source_env,
)
module.prepare_codex_environment(
    no_state_env,
    source_env=source_env,
    enable_stateful=False,
    stateful_integration=module.STATEFUL_INTEGRATION_NONE,
)
no_state_home = Path(no_state_env["CODEX_HOME"])

stateful_env = module.denovo_codex_environment(
    output=output,
    instance_id="issue/stateful",
    task_path=task_path,
    workspace=workspace,
    base_env=source_env,
)
module.prepare_codex_environment(
    stateful_env,
    source_env=source_env,
    enable_stateful=True,
    stateful_binary="/tmp/stateful",
    stateful_integration=module.STATEFUL_INTEGRATION_FULL,
)
stateful_home = Path(stateful_env["CODEX_HOME"])
stateful_config = (stateful_home / "config.toml").read_text()

print(json.dumps({{
    "no_state_home": no_state_env["HOME"],
    "no_state_config_exists": (no_state_home / "config.toml").exists(),
    "no_state_skill_exists": (no_state_home / "skills" / "stateful-command-policy" / "SKILL.md").exists(),
    "no_state_auth_exists": (no_state_home / "auth.json").exists(),
    "stateful_home": stateful_env["HOME"],
    "stateful_config": stateful_config,
    "stateful_skill_exists": (stateful_home / "skills" / "stateful-command-policy" / "SKILL.md").exists(),
    "stateful_auth_exists": (stateful_home / "auth.json").exists(),
}}, sort_keys=True))
"#,
        agent_path = denovo_codex_agent_path_json(),
        temp_dir = serde_json::to_string(&temp_dir.to_string_lossy())
            .expect("temp dir should encode as json"),
    );
    let output = run_python_json(&script);

    assert!(
        output["no_state_home"]
            .as_str()
            .expect("no-state home should be text")
            .ends_with("adapter-output/codex-homes/issue-no-state/home")
    );
    assert_eq!(output["no_state_config_exists"], false);
    assert_eq!(output["no_state_skill_exists"], false);
    assert_eq!(output["no_state_auth_exists"], true);

    assert!(
        output["stateful_home"]
            .as_str()
            .expect("stateful home should be text")
            .ends_with("adapter-output/codex-homes/issue-stateful/home")
    );
    let config = output["stateful_config"]
        .as_str()
        .expect("stateful config should be text");
    assert!(config.contains("[mcp_servers.stateful]"));
    assert!(config.contains("command = \"/tmp/stateful\""));
    assert!(config.contains("[[hooks.SessionStart]]"));
    assert_eq!(output["stateful_skill_exists"], true);
    assert_eq!(output["stateful_auth_exists"], true);

    fs::remove_dir_all(temp_dir).expect("temp dir should clean up");
}

#[test]
fn denovo_codex_agent_enables_stateful_repo_before_codex() {
    let temp_dir = target_temp_dir("stateful-bench-denovo-codex-repo-enable");
    let script = format!(
        r#"
import importlib.util
import json
import subprocess
import sys
from pathlib import Path

spec = importlib.util.spec_from_file_location("denovo_codex_agent_repo_enable_test", {agent_path})
module = importlib.util.module_from_spec(spec)
sys.modules[spec.name] = module
spec.loader.exec_module(module)

root = Path({temp_dir})
workspace = root / "workspace"
workspace.mkdir(parents=True, exist_ok=True)
home = root / "home"
env = {{
    "HOME": str(home),
    "CODEX_HOME": str(home / ".codex"),
    "PATH": "/bin",
}}
calls = []

class Completed:
    returncode = 0
    stdout = "enabled\n"
    stderr = ""

def fake_runner(command, cwd, env, text, check, stdout, stderr):
    calls.append({{
        "command": [str(part) for part in command],
        "cwd": str(cwd),
        "home": env.get("HOME"),
        "codex_home": env.get("CODEX_HOME"),
        "text": text,
        "check": check,
        "stdout_pipe": stdout is subprocess.PIPE,
        "stderr_pipe": stderr is subprocess.PIPE,
    }})
    return Completed()

module.enable_stateful_repo(
    env=env,
    workspace=workspace,
    stateful_binary="/tmp/stateful",
    runner=fake_runner,
)
print(json.dumps({{"calls": calls, "workspace": str(workspace), "home": str(home)}}, sort_keys=True))
"#,
        agent_path = denovo_codex_agent_path_json(),
        temp_dir = serde_json::to_string(&temp_dir.to_string_lossy())
            .expect("temp dir should encode as json"),
    );
    let output = run_python_json(&script);
    let calls = output["calls"]
        .as_array()
        .expect("calls should be an array");
    assert_eq!(calls.len(), 1);

    let call = &calls[0];
    assert_eq!(
        call["command"],
        serde_json::json!([
            "/tmp/stateful",
            "enable",
            "--repo",
            output["workspace"]
                .as_str()
                .expect("workspace should be text"),
        ])
    );
    assert_eq!(call["cwd"], output["workspace"]);
    assert_eq!(call["home"], output["home"]);
    assert_eq!(
        call["codex_home"],
        format!(
            "{}/.codex",
            output["home"].as_str().expect("home should be text")
        )
    );
    assert_eq!(call["text"], true);
    assert_eq!(call["check"], false);
    assert_eq!(call["stdout_pipe"], true);
    assert_eq!(call["stderr_pipe"], true);

    fs::remove_dir_all(temp_dir).expect("temp dir should clean up");
}

#[test]
fn denovo_codex_agent_cleans_repo_enable_metadata_created_by_enable() {
    let temp_dir = target_temp_dir("stateful-bench-denovo-codex-repo-enable-cleanup");
    let script = format!(
        r#"
import importlib.util
import json
import sys
from pathlib import Path

spec = importlib.util.spec_from_file_location("denovo_codex_agent_repo_enable_cleanup_test", {agent_path})
module = importlib.util.module_from_spec(spec)
sys.modules[spec.name] = module
spec.loader.exec_module(module)

root = Path({temp_dir})
workspace = root / "workspace"
stateful_dir = workspace / ".stateful"
stateful_dir.mkdir(parents=True, exist_ok=True)
(stateful_dir / "config.yml").write_text("created by enable\n")

cleanup = module.StatefulRepoEnableCleanup(
    created_stateful_dir=True,
    created_policy_config=True,
)
module.cleanup_stateful_repo_enable(workspace, cleanup)
created_removed = not stateful_dir.exists()

stateful_dir.mkdir(parents=True, exist_ok=True)
(stateful_dir / "config.yml").write_text("existing config\n")
preserve_cleanup = module.StatefulRepoEnableCleanup(
    created_stateful_dir=False,
    created_policy_config=False,
)
module.cleanup_stateful_repo_enable(workspace, preserve_cleanup)
preserved = (stateful_dir / "config.yml").read_text()

print(json.dumps({{
    "created_removed": created_removed,
    "preserved": preserved,
}}, sort_keys=True))
"#,
        agent_path = denovo_codex_agent_path_json(),
        temp_dir = serde_json::to_string(&temp_dir.to_string_lossy())
            .expect("temp dir should encode as json"),
    );
    let output = run_python_json(&script);

    assert_eq!(output["created_removed"], true);
    assert_eq!(output["preserved"], "existing config\n");

    fs::remove_dir_all(temp_dir).expect("temp dir should clean up");
}

#[test]
fn denovo_codex_agent_error_rows_are_successful_after_results_are_written() {
    let script = format!(
        r#"
import importlib.util
import json
import sys

spec = importlib.util.spec_from_file_location("denovo_codex_agent_error_exit_test", {agent_path})
module = importlib.util.module_from_spec(spec)
sys.modules[spec.name] = module
spec.loader.exec_module(module)

result = module.InstanceResult(
    instance_id="case-a",
    success=False,
    score=None,
    finish_reason="codex-error",
    error="codex exited 1",
    eval_result=None,
)
print(json.dumps({{
    "exit_code": module.adapter_exit_code_after_results([result]),
    "row": module.instance_result_row(result),
}}, sort_keys=True))
"#,
        agent_path = denovo_codex_agent_path_json(),
    );
    let output = run_python_json(&script);

    assert_eq!(output["exit_code"], 0);
    assert_eq!(output["row"]["error"], "codex exited 1");
    assert_eq!(output["row"]["finish_reason"], "codex-error");
}

#[test]
fn denovo_codex_agent_metadata_marks_official_single_rollout_protocol() {
    let script = format!(
        r#"
import importlib.util
import json
import sys

spec = importlib.util.spec_from_file_location("denovo_codex_agent_protocol_test", {agent_path})
module = importlib.util.module_from_spec(spec)
sys.modules[spec.name] = module
spec.loader.exec_module(module)

print(json.dumps(module.profile_metadata("stateful", "on"), sort_keys=True))
"#,
        agent_path = denovo_codex_agent_path_json(),
    );
    let output = run_python_json(&script);

    assert_eq!(
        output["official_benchmark_protocol"],
        "denovo_swe_single_rollout"
    );
    assert_eq!(output["agent_rollouts_per_instance"], 1);
    assert_eq!(output["eval_feedback_loop"], false);
    assert_eq!(output["eval_feedback_attempts"], 0);
    assert_eq!(output["resume_policy"], "context_or_token_failure_only");
    assert_eq!(output["stateful_mcp"], true);
}

#[test]
fn denovo_codex_agent_validate_run_skips_codex() {
    let script = format!(
        r#"
import importlib.util
import json
import sys
from argparse import Namespace

spec = importlib.util.spec_from_file_location("denovo_codex_agent_validate_skip_test", {agent_path})
module = importlib.util.module_from_spec(spec)
sys.modules[spec.name] = module
spec.loader.exec_module(module)

print(json.dumps({{
    "validate_run": module.should_run_codex(Namespace(validate_run=True)),
    "normal_run": module.should_run_codex(Namespace(validate_run=False)),
}}, sort_keys=True))
"#,
        agent_path = denovo_codex_agent_path_json(),
    );
    let output = run_python_json(&script);

    assert_eq!(output["validate_run"], false);
    assert_eq!(output["normal_run"], true);
}

#[test]
fn denovo_codex_agent_max_concurrent_over_one_yields_reportable_error_row() {
    let script = format!(
        r#"
import importlib.util
import json
import sys
from argparse import Namespace

spec = importlib.util.spec_from_file_location("denovo_codex_agent_max_concurrent_test", {agent_path})
module = importlib.util.module_from_spec(spec)
sys.modules[spec.name] = module
spec.loader.exec_module(module)

result = module.max_concurrent_error_result(Namespace(max_concurrent=2))
print(json.dumps({{
    "exit_code": module.adapter_exit_code_after_results([result]),
    "row": module.instance_result_row(result),
}}, sort_keys=True))
"#,
        agent_path = denovo_codex_agent_path_json(),
    );
    let output = run_python_json(&script);

    assert_eq!(output["exit_code"], 0);
    assert_eq!(output["row"]["finish_reason"], "setup-error");
    assert!(
        output["row"]["error"]
            .as_str()
            .expect("error should be text")
            .contains("Codex DeNovo adapter currently supports --max-concurrent 1 only")
    );
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
fn codex_pair_agent_can_enable_native_subagent_and_context_window() {
    let script = format!(
        r#"
import importlib.util
import json
from pathlib import Path

spec = importlib.util.spec_from_file_location("codex_pair_agent_subagent_test", {agent_path})
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)

command = module.codex_command(
    workspace=Path("/tmp/workspace"),
    mode="stateful",
    stateful_binary="/tmp/stateful",
    benchmark_model="gpt-5.5",
    benchmark_reasoning_effort="xhigh",
    benchmark_model_context_window=256000,
    enable_native_subagent=True,
    disable_bundled_skills=True,
    stateful_integration="hooks-only",
    base_env={{"STATEFUL_NESTED_CODEX_HOME_ROOT": "/repo/target/nested-codex-homes"}},
)
print(json.dumps({{"command": command}}))
"#,
        agent_path = codex_pair_agent_path_json(),
    );
    let output = run_python_json(&script);
    let command = output["command"]
        .as_array()
        .expect("command should be an array");

    assert_eq!(command_arg_after(command, "--model"), Some("gpt-5.5"));
    assert!(command_contains(
        command,
        "model_reasoning_effort=\"xhigh\""
    ));
    assert!(command_contains(command, "model_context_window=256000"));
    assert!(command_contains(command, "features.multi_agent=true"));
    assert!(command_contains(command, "skills.bundled.enabled=false"));
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
fn codex_pair_agent_writes_stateful_mcp_config_and_skill_for_nested_stateful_run() {
    let temp_dir = target_temp_dir("stateful-bench-codex-pair-agent-stateful-config");
    let script = format!(
        r#"
import importlib.util
import json
from pathlib import Path

spec = importlib.util.spec_from_file_location("codex_pair_agent_stateful_config_test", {agent_path})
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)

root = Path({temp_dir})
workspace = root / "runs" / "pair-one" / "workspace"
task_path = workspace / ".stateful_bench" / "task-a.json"
source_env = {{
    "PATH": "/bin",
    "STATEFUL_NESTED_CODEX_HOME_ROOT": str(root / "nested-codex-homes"),
}}
env = module.codex_environment(task_path=task_path, workspace=workspace, base_env=source_env)
module.prepare_codex_environment(
    env,
    source_env=source_env,
    enable_stateful=True,
    stateful_binary="/tmp/stateful",
)

codex_home = Path(env["CODEX_HOME"])
config = (codex_home / "config.toml").read_text()
skill = (codex_home / "skills" / "stateful-command-policy" / "SKILL.md").read_text()
print(json.dumps({{
    "config": config,
    "skill": skill,
    "auth_exists": (codex_home / "auth.json").exists(),
}}, sort_keys=True))
"#,
        agent_path = codex_pair_agent_path_json(),
        temp_dir = serde_json::to_string(&temp_dir.to_string_lossy())
            .expect("temp dir should encode as json"),
    );
    let output = run_python_json(&script);

    let config = output["config"].as_str().expect("config should be text");
    assert!(config.contains("[mcp_servers.stateful]"));
    assert!(config.contains("command = \"/tmp/stateful\""));
    assert!(config.contains("args = [\"mcp\", \"serve\"]"));
    assert!(config.contains("[[hooks.SessionStart]]"));
    assert!(config.contains("[[hooks.PreToolUse]]"));
    assert!(config.contains("[[hooks.Stop]]"));

    let skill = output["skill"].as_str().expect("skill should be text");
    assert!(skill.contains("name: stateful-command-policy"));
    assert!(skill.contains("Use MCP tools such as `state_intent_declare`"));
    assert_eq!(output["auth_exists"], false);

    fs::remove_dir_all(temp_dir).expect("temp dir should clean up");
}

#[test]
fn codex_pair_agent_hooks_only_stateful_run_does_not_write_mcp_config_or_skill() {
    let temp_dir = target_temp_dir("stateful-bench-codex-pair-agent-hooks-only-config");
    let script = format!(
        r#"
import importlib.util
import json
from pathlib import Path

spec = importlib.util.spec_from_file_location("codex_pair_agent_hooks_only_config_test", {agent_path})
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)

root = Path({temp_dir})
workspace = root / "runs" / "pair-one" / "workspace"
task_path = workspace / ".stateful_bench" / "task-a.json"
source_env = {{
    "PATH": "/bin",
    "STATEFUL_NESTED_CODEX_HOME_ROOT": str(root / "nested-codex-homes"),
}}
env = module.codex_environment(task_path=task_path, workspace=workspace, base_env=source_env)
module.prepare_codex_environment(
    env,
    source_env=source_env,
    enable_stateful=True,
    stateful_binary="/tmp/stateful",
    stateful_integration="hooks-only",
)

codex_home = Path(env["CODEX_HOME"])
config = (codex_home / "config.toml").read_text()
print(json.dumps({{
    "config": config,
    "skill_exists": (codex_home / "skills" / "stateful-command-policy" / "SKILL.md").exists(),
}}, sort_keys=True))
"#,
        agent_path = codex_pair_agent_path_json(),
        temp_dir = serde_json::to_string(&temp_dir.to_string_lossy())
            .expect("temp dir should encode as json"),
    );
    let output = run_python_json(&script);

    let config = output["config"].as_str().expect("config should be text");
    assert!(!config.contains("[mcp_servers.stateful]"));
    assert!(config.contains("[features]"));
    assert!(config.contains("hooks = true"));
    assert!(config.contains("[[hooks.SessionStart]]"));
    assert!(config.contains("[[hooks.PreToolUse]]"));
    assert!(config.contains("[[hooks.Stop]]"));
    assert_eq!(output["skill_exists"], false);

    fs::remove_dir_all(temp_dir).expect("temp dir should clean up");
}

#[test]
fn codex_pair_agent_nested_no_state_does_not_write_stateful_mcp_config_or_skill() {
    let temp_dir = target_temp_dir("stateful-bench-codex-pair-agent-no-state-config");
    let script = format!(
        r#"
import importlib.util
import json
from pathlib import Path

spec = importlib.util.spec_from_file_location("codex_pair_agent_no_state_config_test", {agent_path})
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)

root = Path({temp_dir})
workspace = root / "runs" / "pair-one" / "workspace"
task_path = workspace / ".stateful_bench" / "task-a.json"
source_env = {{
    "PATH": "/bin",
    "STATEFUL_NESTED_CODEX_HOME_ROOT": str(root / "nested-codex-homes"),
}}
env = module.codex_environment(task_path=task_path, workspace=workspace, base_env=source_env)
codex_home = Path(env["CODEX_HOME"])
(codex_home / "skills" / "stateful-command-policy").mkdir(parents=True, exist_ok=True)
(codex_home / "config.toml").write_text('# stateful-bench nested Codex integration\nstale = true\n')
(codex_home / "skills" / "stateful-command-policy" / "SKILL.md").write_text("stale skill")
module.prepare_codex_environment(env, source_env=source_env)

print(json.dumps({{
    "config_exists": (codex_home / "config.toml").exists(),
    "skill_exists": (codex_home / "skills" / "stateful-command-policy" / "SKILL.md").exists(),
}}, sort_keys=True))
"#,
        agent_path = codex_pair_agent_path_json(),
        temp_dir = serde_json::to_string(&temp_dir.to_string_lossy())
            .expect("temp dir should encode as json"),
    );
    let output = run_python_json(&script);

    assert_eq!(output["config_exists"], false);
    assert_eq!(output["skill_exists"], false);

    fs::remove_dir_all(temp_dir).expect("temp dir should clean up");
}

#[test]
fn codex_pair_agent_source_env_sets_stateful_session_only_for_stateful_mode() {
    let script = format!(
        r#"
import importlib.util
import json

spec = importlib.util.spec_from_file_location("codex_pair_agent_source_env_test", {agent_path})
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)

stateful = module.benchmark_source_env(
    mode="stateful",
    session_id="pair-one-agent-a",
    base_env={{"PATH": "/bin"}},
)
no_state = module.benchmark_source_env(
    mode="no-state",
    session_id=None,
    base_env={{"PATH": "/bin"}},
)
print(json.dumps({{
    "stateful": stateful,
    "no_state": no_state,
}}, sort_keys=True))
"#,
        agent_path = codex_pair_agent_path_json(),
    );
    let output = run_python_json(&script);

    assert_eq!(
        output["stateful"]["STATEFUL_SESSION_ID"],
        "pair-one-agent-a"
    );
    assert_eq!(output["stateful"]["PATH"], "/bin");
    assert_eq!(
        output["no_state"]
            .as_object()
            .expect("no-state env should be an object")
            .contains_key("STATEFUL_SESSION_ID"),
        false
    );
    assert_eq!(output["no_state"]["PATH"], "/bin");
}

#[test]
fn codex_pair_agent_resumes_after_context_token_failure() {
    let script = format!(
        r#"
import importlib.util
import io
import json
import sys
from pathlib import Path

spec = importlib.util.spec_from_file_location("codex_pair_agent_resume_test", {agent_path})
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)

class Completed:
    def __init__(self, returncode, stdout, stderr=""):
        self.returncode = returncode
        self.stdout = stdout
        self.stderr = stderr

calls = []

def fake_run(command, input, text, cwd, check, env, stdout, stderr):
    calls.append({{
        "command": command,
        "input": input,
        "cwd": str(cwd),
        "env_path": env.get("PATH"),
        "stdout_pipe": stdout is module.subprocess.PIPE,
        "stderr_pipe": stderr is module.subprocess.PIPE,
    }})
    if len(calls) == 1:
        return Completed(
            1,
            '{{"type":"session_meta","payload":{{"id":"session-123"}}}}\n'
            '{{"type":"turn.failed","error":{{"message":"context_length_exceeded: input tokens exceed the model context window"}}}}\n'
        )
    return Completed(
        0,
        '{{"type":"token_count","info":{{"total_token_usage":{{"total_tokens":42}}}}}}\n'
    )

captured_stdout = io.StringIO()
captured_stderr = io.StringIO()
original_stdout = sys.stdout
original_stderr = sys.stderr
sys.stdout = captured_stdout
sys.stderr = captured_stderr
try:
    code = module.run_codex_with_resume(
        ["codex", "--model", "gpt-5.5", "exec", "--json", "--dangerously-bypass-hook-trust", "--cd", "/repo/work", "--sandbox", "workspace-write", "-"],
        "initial prompt",
        Path("/repo/work"),
        {{"PATH": "/bin"}},
        max_resumes=1,
        runner=fake_run,
    )
finally:
    sys.stdout = original_stdout
    sys.stderr = original_stderr

print(json.dumps({{
    "code": code,
    "calls": calls,
    "stdout": captured_stdout.getvalue(),
    "stderr": captured_stderr.getvalue(),
}}, sort_keys=True))
"#,
        agent_path = codex_pair_agent_path_json(),
    );
    let output = run_python_json(&script);

    assert_eq!(output["code"], 0);
    let calls = output["calls"]
        .as_array()
        .expect("calls should be an array");
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[0]["input"], "initial prompt");
    assert!(
        calls[1]["input"]
            .as_str()
            .expect("resume prompt should be text")
            .contains("Continue the same benchmark task")
    );
    let resume_command = calls[1]["command"]
        .as_array()
        .expect("resume command should be an array");
    assert!(command_contains(resume_command, "resume"));
    assert!(command_contains(resume_command, "session-123"));
    assert!(!command_contains(resume_command, "--cd"));
    assert!(!command_contains(resume_command, "--sandbox"));
    assert!(
        !output["stdout"]
            .as_str()
            .expect("stdout should be text")
            .contains("turn.failed")
    );
    assert!(
        output["stdout"]
            .as_str()
            .expect("stdout should be text")
            .contains("stateful_bench.resume")
    );
    assert!(
        output["stdout"]
            .as_str()
            .expect("stdout should be text")
            .contains("token_count")
    );
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

#[test]
fn codex_synthetic_agent_switches_inner_sandbox_for_nested_benchmark() {
    let script = format!(
        r#"
import importlib.util
import json
from pathlib import Path

spec = importlib.util.spec_from_file_location("codex_synthetic_agent_for_test", {agent_path})
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
        agent_path = codex_synthetic_agent_path_json(),
    );
    let output = run_python_json(&script);
    let default_command = output["default"].as_array().expect("default command");
    let nested_command = output["nested"].as_array().expect("nested command");

    assert_eq!(
        command_arg_after(default_command, "--sandbox"),
        Some("workspace-write")
    );
    assert_eq!(
        command_arg_after(nested_command, "--sandbox"),
        Some("danger-full-access")
    );
}

#[test]
fn codex_synthetic_agent_builds_nested_codex_environment_with_system_cert() {
    let script = format!(
        r#"
import importlib.util
import json
from pathlib import Path

spec = importlib.util.spec_from_file_location("codex_synthetic_agent_env_test", {agent_path})
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)

env = module.codex_environment(
    pair_id="pair/one",
    agent_id="agent-a",
    base_env={{
        "PATH": "/bin",
        "STATEFUL_NESTED_CODEX_HOME_ROOT": "/repo/target/nested-codex-homes",
    }},
)
system_cert = Path("/etc/ssl/cert.pem")
print(json.dumps({{
    "env": env,
    "system_cert_exists": system_cert.is_file(),
}}, sort_keys=True))
"#,
        agent_path = codex_synthetic_agent_path_json(),
    );
    let output = run_python_json(&script);
    let env = &output["env"];

    assert_eq!(
        env["HOME"],
        "/repo/target/nested-codex-homes/pair-one/agent-a/home"
    );
    assert_eq!(
        env["CODEX_HOME"],
        "/repo/target/nested-codex-homes/pair-one/agent-a/home/.codex"
    );
    if output["system_cert_exists"].as_bool() == Some(true) {
        assert_eq!(env["SSL_CERT_FILE"], "/etc/ssl/cert.pem");
    }
}

#[test]
fn codex_synthetic_agent_seeds_and_cleans_nested_auth() {
    let temp_dir = target_temp_dir("stateful-bench-codex-synthetic-agent-auth");
    let script = format!(
        r#"
import importlib.util
import json
from pathlib import Path

spec = importlib.util.spec_from_file_location("codex_synthetic_agent_auth_test", {agent_path})
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)

root = Path({temp_dir})
source_home = root / "source-home"
source_auth = source_home / ".codex" / "auth.json"
source_auth.parent.mkdir(parents=True, exist_ok=True)
source_auth.write_text("{{\"token\":\"source\"}}")

source_env = {{
    "PATH": "/bin",
    "HOME": str(source_home),
    "STATEFUL_NESTED_CODEX_HOME_ROOT": str(root / "nested-codex-homes"),
}}
env = module.codex_environment(pair_id="pair-one", agent_id="agent-a", base_env=source_env)
seeded = module.prepare_codex_environment(env, source_env=source_env)
target_auth = Path(env["CODEX_HOME"]) / "auth.json"
copied = target_auth.read_text()
module.cleanup_seeded_auth(seeded)
print(json.dumps({{
    "copied": copied,
    "target_exists_after_cleanup": target_auth.exists(),
    "source_exists_after_cleanup": source_auth.exists(),
}}))
"#,
        agent_path = codex_synthetic_agent_path_json(),
        temp_dir = serde_json::to_string(&temp_dir.to_string_lossy())
            .expect("temp dir should encode as json"),
    );
    let output = run_python_json(&script);

    assert_eq!(output["copied"], "{\"token\":\"source\"}");
    assert_eq!(output["target_exists_after_cleanup"], false);
    assert_eq!(output["source_exists_after_cleanup"], true);

    fs::remove_dir_all(temp_dir).expect("temp dir should clean up");
}

fn codex_pair_agent_path_json() -> String {
    serde_json::to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/scripts/codex_pair_agent.py"
    ))
    .expect("agent path should encode as json")
}

fn denovo_codex_agent_path_json() -> String {
    serde_json::to_string(&format!(
        "{}/scripts/denovo_codex_agent.py",
        env!("CARGO_MANIFEST_DIR")
    ))
    .expect("path should serialize")
}

fn codex_synthetic_agent_path_json() -> String {
    serde_json::to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../.stateful_bench/agent_synthetic/codex_synthetic_agent.py"
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
    let target_dir = std::env::var_os("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target"));
    let dir = target_dir.join(format!("{name}-{}", std::process::id()));
    if dir.exists() {
        fs::remove_dir_all(&dir).expect("old temp dir should clean up");
    }
    fs::create_dir_all(&dir).expect("temp dir should be created");
    dir
}
