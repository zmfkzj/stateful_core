use clap::Parser;
use stateful_bench::programbench::{
    ProgramBenchDiscoveredInstance, ProgramBenchInstanceReport,
    run_programbench_matrix_with_instances,
};
use stateful_bench::{
    Cli, Command, ProgramBenchAgentKind, ProgramBenchCommand, ProgramBenchCondition,
    ProgramBenchConditionMetadata, ProgramBenchConditionReport, ProgramBenchEvalOptions,
    ProgramBenchInstanceMetadata, ProgramBenchInstanceRunOptions, ProgramBenchRunOptions,
    ProgramBenchTokenUsage, ReportFormat, build_programbench_agent_command,
    build_programbench_condition_report, build_programbench_eval_commands,
    compare_programbench_reports, default_programbench_conditions, parse_programbench_condition,
    planned_programbench_conditions,
};
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::{
    collections::BTreeMap,
    fs,
    io::Write,
    path::{Path, PathBuf},
    process::Command as ProcessCommand,
    time::{SystemTime, UNIX_EPOCH},
};

#[test]
fn programbench_metadata_schema_uses_required_instance_fields() {
    fn accepts_usize(value: usize) -> usize {
        value
    }

    let token_usage = ProgramBenchTokenUsage {
        turns: 3,
        input_tokens: 10,
        cached_input_tokens: 4,
        output_tokens: 6,
        reasoning_output_tokens: 2,
        input_plus_output_tokens: 16,
        uncached_input_tokens: 6,
        uncached_input_plus_output_tokens: 12,
    };

    assert_eq!(accepts_usize(token_usage.turns), 3);

    let metadata = ProgramBenchInstanceMetadata {
        instance_id: "BurntSushi__ripgrep.abc123".to_string(),
        condition_id: "stateful-on_subagent-off".to_string(),
        agent: ProgramBenchAgentKind::CodexCli,
        started_at_ms: 1,
        finished_at_ms: 2,
        running_time_ms: 1,
        submission_path: "submission.tar.gz".to_string(),
        exit_code: None,
        error: None,
        subagent_used: None,
        token_usage: Some(token_usage),
    };

    assert_eq!(metadata.finished_at_ms - metadata.started_at_ms, 1);
    assert_eq!(metadata.running_time_ms, 1);
    assert_eq!(metadata.submission_path, "submission.tar.gz");
}

#[test]
fn programbench_run_command_parses_defaultable_options() {
    let cli = Cli::try_parse_from([
        "stateful-bench",
        "programbench",
        "run",
        "--output-dir",
        ".stateful_bench/programbench/runs",
        "--run-id",
        "pb-dev",
        "--agent",
        "codex-cli",
        "--condition",
        "stateful:on,subagent:off",
        "--model",
        "gpt-5.4-mini",
        "--benchmark-max-turns",
        "500",
        "--timeout-seconds",
        "7200",
        "--filter",
        "ripgrep.*",
        "--slice",
        "0:2",
        "--max-instances",
        "2",
        "--programbench-bin",
        "programbench",
        "--docker-bin",
        "docker",
        "--image-tag",
        "task_cleanroom_v6",
        "--stateful-binary",
        "target/debug/stateful",
    ])
    .expect("programbench run command should parse");

    assert!(matches!(
        cli.command,
        Command::Programbench {
            command: ProgramBenchCommand::Run {
                ref output_dir,
                ref run_id,
                agent: ProgramBenchAgentKind::CodexCli,
                ref condition,
                ref model,
                benchmark_max_turns: 500,
                timeout_seconds: 7200,
                ref filter,
                ref slice,
                max_instances: Some(2),
                ref programbench_bin,
                ref docker_bin,
                ref image_tag,
                ref stateful_binary,
                ..
            }
        } if output_dir == Path::new(".stateful_bench/programbench/runs")
            && run_id == "pb-dev"
            && condition == &vec!["stateful:on,subagent:off".to_string()]
            && model.as_deref() == Some("gpt-5.4-mini")
            && filter.as_deref() == Some("ripgrep.*")
            && slice.as_deref() == Some("0:2")
            && programbench_bin == "programbench"
            && docker_bin == "docker"
            && image_tag == "task_cleanroom_v6"
            && stateful_binary == "target/debug/stateful"
    ));
}

#[test]
fn programbench_eval_report_compare_commands_parse() {
    let eval = Cli::try_parse_from([
        "stateful-bench",
        "programbench",
        "eval",
        "--run-dir",
        ".stateful_bench/programbench/runs/pb-dev",
        "--workers",
        "4",
        "--branch-workers",
        "2",
        "--docker-cpus",
        "8",
        "--force",
        "--no-package",
    ])
    .expect("programbench eval should parse");
    assert!(matches!(
        eval.command,
        Command::Programbench {
            command: ProgramBenchCommand::Eval {
                ref run_dir,
                workers: 4,
                branch_workers: 2,
                docker_cpus: 8,
                force: true,
                no_package: true,
                ..
            }
        } if run_dir == Path::new(".stateful_bench/programbench/runs/pb-dev")
    ));

    let report = Cli::try_parse_from([
        "stateful-bench",
        "programbench",
        "report",
        "--condition-dir",
        "runs/pb-dev/conditions/stateful-on_subagent-off",
        "--format",
        "markdown",
    ])
    .expect("programbench report should parse");
    assert!(matches!(
        report.command,
        Command::Programbench {
            command: ProgramBenchCommand::Report {
                ref condition_dir,
                format: ReportFormat::Markdown,
                ..
            }
        } if condition_dir == Path::new("runs/pb-dev/conditions/stateful-on_subagent-off")
    ));

    let compare = Cli::try_parse_from([
        "stateful-bench",
        "programbench",
        "compare",
        "--report",
        "stateful-off.json",
        "--report",
        "stateful-on.json",
    ])
    .expect("programbench compare should parse");
    assert!(matches!(
        compare.command,
        Command::Programbench {
            command: ProgramBenchCommand::Compare { ref report, .. }
        } if report == &vec![PathBuf::from("stateful-off.json"), PathBuf::from("stateful-on.json")]
    ));
}

#[test]
fn programbench_condition_parser_accepts_axes_and_defaults_cover_four_conditions() {
    let condition =
        parse_programbench_condition("stateful:on,subagent:off").expect("condition should parse");

    assert!(condition.stateful);
    assert!(!condition.subagent);
    assert_eq!(condition.id(), "stateful-on_subagent-off");

    assert_eq!(
        default_programbench_conditions()
            .iter()
            .map(ProgramBenchCondition::id)
            .collect::<Vec<_>>(),
        vec![
            "stateful-off_subagent-off",
            "stateful-on_subagent-off",
            "stateful-off_subagent-on",
            "stateful-on_subagent-on",
        ]
    );
}

#[test]
fn programbench_run_uses_default_four_axis_matrix_when_no_conditions_passed() {
    let conditions = planned_programbench_conditions(&[]).expect("default conditions should build");
    assert_eq!(
        conditions
            .iter()
            .map(ProgramBenchCondition::id)
            .collect::<Vec<_>>(),
        vec![
            "stateful-off_subagent-off",
            "stateful-on_subagent-off",
            "stateful-off_subagent-on",
            "stateful-on_subagent-on",
        ]
    );
}

#[test]
fn programbench_eval_commands_target_each_condition_directory() {
    let run_dir = temp_root("programbench-eval-conditions").join("pb-dev");
    let first_condition = run_dir.join("conditions").join("stateful-off_subagent-off");
    let second_condition = run_dir.join("conditions").join("stateful-on_subagent-off");
    fs::create_dir_all(&second_condition).expect("second condition dir should exist");
    fs::create_dir_all(&first_condition).expect("first condition dir should exist");

    let commands = build_programbench_eval_commands(ProgramBenchEvalOptions {
        run_dir,
        programbench_bin: "programbench".to_string(),
        workers: 4,
        branch_workers: 2,
        docker_cpus: 8,
        force: true,
        no_package: false,
    })
    .expect("commands should build");
    let rendered = commands
        .iter()
        .map(|command| format!("{} {}", command.program, command.args.join(" ")))
        .collect::<Vec<_>>();
    let first_condition = first_condition.to_string_lossy();
    let second_condition = second_condition.to_string_lossy();

    assert_eq!(
        rendered,
        vec![
            format!(
                "programbench eval {first_condition} --workers 4 --branch-workers 2 --docker-cpus 8 --force"
            ),
            format!("programbench info {first_condition}"),
            format!("programbench submit package {first_condition}"),
            format!(
                "programbench eval {second_condition} --workers 4 --branch-workers 2 --docker-cpus 8 --force"
            ),
            format!("programbench info {second_condition}"),
            format!("programbench submit package {second_condition}"),
        ]
    );
    assert!(
        rendered
            .iter()
            .all(|command| !command.contains("programbench eval runs/pb-dev"))
    );
}

#[cfg(unix)]
#[test]
fn programbench_run_matrix_executes_selected_instance_and_records_metadata() {
    let root = temp_root("programbench-run-matrix");
    fs::create_dir_all(&root).expect("temp root should exist");
    let output_dir = root.join("runs");
    let docker_log = root.join("docker.log");
    let codex_log = root.join("codex.log");
    let docker = fake_executable(
        &root,
        "fake-docker",
        &format!(
            r#"#!/bin/sh
set -eu
printf '%s\n' "$*" >> "{}"
case "$1" in
  run)
    printf 'fake-container-id\n'
    ;;
  cp)
    : > "$3"
    ;;
  exec|rm)
    exit 0
    ;;
esac
"#,
            docker_log.display()
        ),
    );
    let codex = fake_executable(
        &root,
        "fake-codex",
        &format!(
            r#"#!/bin/sh
set -eu
printf '%s\n' "$*" >> "{}"
printf '{{"usage":{{"input_tokens":7,"output_tokens":5}}}}\n'
"#,
            codex_log.display()
        ),
    );

    let metadata = run_programbench_matrix_with_instances(
        ProgramBenchRunOptions {
            output_dir: output_dir.clone(),
            run_id: "pb-dev".to_string(),
            agent: ProgramBenchAgentKind::CodexCli,
            conditions: vec![ProgramBenchCondition::new(true, false)],
            model: None,
            benchmark_max_turns: 500,
            timeout_seconds: 17,
            filter: None,
            slice: None,
            max_instances: None,
            programbench_bin: "programbench".to_string(),
            docker_bin: docker.to_string_lossy().into_owned(),
            image_tag: "task_cleanroom_v6".to_string(),
            stateful_binary: "stateful".to_string(),
            codex_bin: codex.to_string_lossy().into_owned(),
            omp_bin: "omp".to_string(),
        },
        vec![ProgramBenchDiscoveredInstance {
            instance_id: "owner__repo.abc123".to_string(),
            image_name: "programbench/fake-image".to_string(),
        }],
    )
    .expect("matrix metadata should be written");

    assert_eq!(metadata.len(), 1);
    assert_eq!(metadata[0].condition_id, "stateful-on_subagent-off");
    assert_eq!(metadata[0].instances.len(), 1);
    assert_eq!(metadata[0].instances[0].instance_id, "owner__repo.abc123");
    assert_eq!(metadata[0].instances[0].exit_code, Some(0));
    assert_eq!(metadata[0].instances[0].error, None);

    let condition_path = output_dir
        .join("pb-dev")
        .join("conditions")
        .join("stateful-on_subagent-off")
        .join("condition.json");
    let written: ProgramBenchConditionMetadata =
        serde_json::from_str(&fs::read_to_string(condition_path).expect("metadata should exist"))
            .expect("metadata should parse");
    assert_eq!(written, metadata[0]);
    assert_eq!(written.instances.len(), 1);

    let instance_path = output_dir
        .join("pb-dev")
        .join("conditions")
        .join("stateful-on_subagent-off")
        .join("owner__repo.abc123")
        .join("instance.json");
    assert!(instance_path.exists(), "instance metadata should exist");

    let docker_calls = fs::read_to_string(docker_log).expect("docker log should exist");
    assert!(docker_calls.contains(
        "run -d --init --network none -w /workspace --name stateful-bench-pb-dev-stateful-on_subagent-off-owner__repo-abc123 programbench/fake-image:task_cleanroom_v6 sleep 17s"
    ));
    assert!(docker_calls.contains("rm -f fake-container-id"));
}

#[cfg(unix)]
#[test]
fn programbench_run_matrix_discovers_instances_with_configured_programbench_executable() {
    let root = temp_root("programbench-discovery-bin");
    fs::create_dir_all(&root).expect("temp root should exist");
    let output_dir = root.join("runs");
    let discovery_log = root.join("discovery.log");
    let docker_log = root.join("docker.log");
    let codex_log = root.join("codex.log");
    let discovery_python = fake_executable(
        &root,
        "fake-programbench-python",
        &format!(
            r#"#!/bin/sh
set -eu
printf '%s\n' "$*" >> "{}"
printf '[{{"instance_id":"stateful_bench__fake_discovery.deadbee","image_name":"programbench/fake-discovery"}}]\n'
"#,
            discovery_log.display()
        ),
    );
    let programbench = fake_executable(
        &root,
        "fake-programbench",
        &format!(
            r#"#!{}
# fake ProgramBench console script; discovery should use this shebang.
"#,
            discovery_python.display()
        ),
    );
    let docker = fake_executable(
        &root,
        "fake-docker",
        &format!(
            r#"#!/bin/sh
set -eu
printf '%s\n' "$*" >> "{}"
case "$1" in
  run)
    printf 'fake-container-id\n'
    ;;
  cp)
    : > "$3"
    ;;
  exec|rm)
    exit 0
    ;;
esac
"#,
            docker_log.display()
        ),
    );
    let codex = fake_executable(
        &root,
        "fake-codex",
        &format!(
            r#"#!/bin/sh
set -eu
printf '%s\n' "$*" >> "{}"
printf '{{"usage":{{"input_tokens":7,"output_tokens":5}}}}\n'
"#,
            codex_log.display()
        ),
    );

    let metadata = stateful_bench::programbench::run_programbench_matrix(ProgramBenchRunOptions {
        output_dir,
        run_id: "pb-dev".to_string(),
        agent: ProgramBenchAgentKind::CodexCli,
        conditions: vec![ProgramBenchCondition::new(false, false)],
        model: None,
        benchmark_max_turns: 500,
        timeout_seconds: 17,
        filter: Some("stateful_bench__fake_discovery\\.deadbee$".to_string()),
        slice: Some("0:1".to_string()),
        max_instances: Some(1),
        programbench_bin: programbench.to_string_lossy().into_owned(),
        docker_bin: docker.to_string_lossy().into_owned(),
        image_tag: "task_cleanroom_v6".to_string(),
        stateful_binary: "stateful".to_string(),
        codex_bin: codex.to_string_lossy().into_owned(),
        omp_bin: "omp".to_string(),
    })
    .expect("matrix should discover through configured ProgramBench executable");

    assert_eq!(metadata.len(), 1);
    assert_eq!(metadata[0].instances.len(), 1);
    assert_eq!(
        metadata[0].instances[0].instance_id,
        "stateful_bench__fake_discovery.deadbee"
    );
    let discovery_calls = fs::read_to_string(discovery_log)
        .expect("configured ProgramBench discovery executable should be invoked");
    assert!(discovery_calls.contains("-c"));
    assert!(discovery_calls.contains("--filter"));
    assert!(discovery_calls.contains("stateful_bench__fake_discovery\\.deadbee$"));
    assert!(discovery_calls.contains("--slice"));
    assert!(discovery_calls.contains("0:1"));
    assert!(discovery_calls.contains("--max-instances"));
    assert!(discovery_calls.contains("1"));
}

#[test]
fn programbench_condition_parser_rejects_unknown_keys() {
    let error = parse_programbench_condition("stateful:on,unknown:off")
        .expect_err("unknown condition key should fail");

    assert!(
        error
            .to_string()
            .contains("unknown ProgramBench condition key `unknown`"),
        "unexpected error: {error}"
    );
}

#[test]
fn programbench_codex_agent_command_contains_condition_and_paths() {
    let command = build_programbench_agent_command(ProgramBenchInstanceRunOptions {
        agent: ProgramBenchAgentKind::CodexCli,
        condition: ProgramBenchCondition::new(true, false),
        instance_id: "BurntSushi__ripgrep.abc123".to_string(),
        container_id: "programbench-container".to_string(),
        condition_dir: "runs/pb/conditions/stateful-on_subagent-off".into(),
        docker_bin: "docker".to_string(),
        codex_bin: "codex".to_string(),
        omp_bin: "omp".to_string(),
        stateful_binary: "target/debug/stateful".to_string(),
        model: Some("gpt-5.4-mini".to_string()),
        benchmark_max_turns: 500,
        timeout_seconds: 7200,
        subagent_min_count: 3,
    })
    .expect("codex agent command should build");

    assert!(command.program.ends_with("programbench_codex_agent.py"));
    assert!(has_arg(&command.args, "--container-id"));
    assert!(has_arg(&command.args, "programbench-container"));
    assert!(has_arg(&command.args, "--condition-id"));
    assert!(has_arg(&command.args, "stateful-on_subagent-off"));
    assert!(has_arg(&command.args, "--stateful"));
    assert!(!has_arg(&command.args, "--subagent"));
}

#[test]
fn programbench_omp_agent_command_marks_subagent_condition() {
    let command = build_programbench_agent_command(ProgramBenchInstanceRunOptions {
        agent: ProgramBenchAgentKind::OmpCli,
        condition: ProgramBenchCondition::new(false, true),
        instance_id: "ajeetdsouza__zoxide.67ca1bc".to_string(),
        container_id: "programbench-container".to_string(),
        condition_dir: "runs/pb/conditions/stateful-off_subagent-on".into(),
        docker_bin: "docker".to_string(),
        codex_bin: "codex".to_string(),
        omp_bin: "omp".to_string(),
        stateful_binary: "stateful".to_string(),
        model: None,
        benchmark_max_turns: 500,
        timeout_seconds: 7200,
        subagent_min_count: 3,
    })
    .expect("omp agent command should build");

    assert!(command.program.ends_with("programbench_omp_agent.py"));
    assert!(has_arg(&command.args, "--subagent"));
    assert!(!has_arg(&command.args, "--stateful"));
}

#[test]
fn programbench_codex_adapter_executes_stateful_codex_inside_container() {
    let output = run_python_adapter(
        &programbench_codex_agent_path(),
        r#"import json
import subprocess
import types

calls = []

def fake_run(command, **kwargs):
    calls.append({"command": command, "timeout": kwargs.get("timeout")})
    return subprocess.CompletedProcess(command, 0, stdout='{"usage":{"input_tokens":1}}\n', stderr="")

mod.subprocess.run = fake_run
args = types.SimpleNamespace(
    docker_bin="docker",
    container_id="programbench-container",
    codex_bin="codex",
    stateful_binary="/usr/local/bin/stateful",
    model="gpt-5.4-mini",
    benchmark_max_turns=321,
    timeout_seconds=123,
    stateful=True,
    subagent=False,
    subagent_min_count=3,
)
prompt = mod.prompt_for_args(args)
result = mod.run_agent(args, prompt)
print(json.dumps({"calls": calls, "prompt": prompt, "returncode": result.returncode}))
"#,
    );
    let observed: serde_json::Value =
        serde_json::from_str(&output).expect("captured calls should be JSON");

    assert_eq!(
        observed["calls"][0]["command"],
        serde_json::json!([
            "docker",
            "exec",
            "-w",
            "/workspace",
            "programbench-container",
            "/usr/local/bin/stateful",
            "install",
            "--agent",
            "codex",
            "--yes"
        ])
    );
    assert_eq!(
        observed["calls"][1]["command"],
        serde_json::json!([
            "docker",
            "exec",
            "-w",
            "/workspace",
            "programbench-container",
            "/usr/local/bin/stateful",
            "enable",
            "--repo",
            "/workspace"
        ])
    );
    assert_eq!(
        observed["calls"][2]["command"],
        serde_json::json!([
            "docker",
            "exec",
            "-w",
            "/workspace",
            "programbench-container",
            "codex",
            "exec",
            "--json",
            "--cd",
            "/workspace",
            "--model",
            "gpt-5.4-mini",
            observed["prompt"]
        ])
    );
    assert_eq!(observed["calls"][2]["timeout"], 123);
    assert!(
        observed["prompt"]
            .as_str()
            .expect("prompt should be a string")
            .contains("Benchmark max turns: 321."),
        "prompt should include benchmark max turns: {observed}"
    );
    assert_eq!(observed["returncode"], 0);
}

#[test]
fn programbench_omp_adapter_executes_stateful_profile_inside_container() {
    let output = run_python_adapter(
        &programbench_omp_agent_path(),
        r#"import json
import subprocess
import types

calls = []

def fake_run(command, **kwargs):
    calls.append({"command": command, "timeout": kwargs.get("timeout")})
    return subprocess.CompletedProcess(command, 0, stdout='{"usage":{"input_tokens":1}}\n', stderr="")

mod.subprocess.run = fake_run
args = types.SimpleNamespace(
    docker_bin="docker",
    container_id="programbench-container",
    omp_bin="omp",
    stateful_binary="/usr/local/bin/stateful",
    model="gpt-5.4-mini",
    benchmark_max_turns=654,
    timeout_seconds=456,
    stateful=True,
    subagent=False,
    subagent_min_count=3,
)
prompt = mod.prompt_for_args(args)
result = mod.run_agent(args, prompt)
print(json.dumps({"calls": calls, "prompt": prompt, "returncode": result.returncode}))
"#,
    );
    let observed: serde_json::Value =
        serde_json::from_str(&output).expect("captured calls should be JSON");

    assert_eq!(
        observed["calls"][0]["command"],
        serde_json::json!([
            "docker",
            "exec",
            "-w",
            "/workspace",
            "programbench-container",
            "/usr/local/bin/stateful",
            "install",
            "--agent",
            "omp",
            "--yes"
        ])
    );
    assert_eq!(
        observed["calls"][1]["command"],
        serde_json::json!([
            "docker",
            "exec",
            "-w",
            "/workspace",
            "programbench-container",
            "/usr/local/bin/stateful",
            "enable",
            "--repo",
            "/workspace"
        ])
    );
    assert_eq!(
        observed["calls"][2]["command"],
        serde_json::json!([
            "docker",
            "exec",
            "-w",
            "/workspace",
            "programbench-container",
            "omp",
            "--cwd",
            "/workspace",
            "--profile",
            "stateful",
            "--model",
            "gpt-5.4-mini",
            "--prompt",
            observed["prompt"]
        ])
    );
    assert_eq!(observed["calls"][2]["timeout"], 456);
    assert!(
        observed["prompt"]
            .as_str()
            .expect("prompt should be a string")
            .contains("Benchmark max turns: 654."),
        "prompt should include benchmark max turns: {observed}"
    );
    assert_eq!(observed["returncode"], 0);
}

#[test]
fn programbench_adapter_omits_subagent_used_without_observation() {
    let output = run_python_adapter(
        &programbench_codex_agent_path(),
        r#"import json
import subprocess
import tempfile
import types
from pathlib import Path

def fake_agent(args, prompt):
    return subprocess.CompletedProcess(["codex"], 0, stdout='{"usage":{"input_tokens":1}}\n', stderr="")

def fake_archive(args, instance_dir):
    return instance_dir / "submission.tar.gz"

mod.archive_workspace = fake_archive
with tempfile.TemporaryDirectory() as condition_dir:
    args = types.SimpleNamespace(
        condition_dir=condition_dir,
        instance_id="owner__repo.abc123",
        condition_id="stateful-on_subagent-on",
        timeout_seconds=123,
        subagent=True,
        subagent_min_count=3,
    )
    exit_code = mod.run_main(
        args,
        agent_name="codex-cli",
        exited_error_prefix="codex",
        token_usage_from_output=mod.codex_token_usage_from_output,
        run_agent_func=fake_agent,
    )
    metadata_path = Path(condition_dir) / "owner__repo.abc123" / "instance.json"
    metadata = json.loads(metadata_path.read_text())
print(json.dumps({"exit_code": exit_code, "has_subagent_used": "subagent_used" in metadata}))
"#,
    );
    let observed: serde_json::Value =
        serde_json::from_str(&output).expect("metadata observation should be JSON");

    assert_eq!(observed["exit_code"], 0);
    assert_eq!(observed["has_subagent_used"], false);
}

#[test]
fn programbench_codex_adapter_parses_token_usage_events() {
    let output = run_python_adapter(
        &programbench_codex_agent_path(),
        r#"import json
usage = mod.codex_token_usage_from_output('{"type":"turn.completed","usage":{"input_tokens":100,"cached_input_tokens":40,"output_tokens":12,"reasoning_output_tokens":5}}\n')
print(json.dumps(usage))
"#,
    );
    let usage: serde_json::Value =
        serde_json::from_str(&output).expect("codex usage should be JSON");

    assert_eq!(usage["input_tokens"], 100);
    assert_eq!(usage["cached_input_tokens"], 40);
    assert_eq!(usage["output_tokens"], 12);
    assert_eq!(usage["input_plus_output_tokens"], 112);
    assert_eq!(usage["uncached_input_plus_output_tokens"], 72);
}

#[test]
fn programbench_omp_adapter_parses_token_usage_events() {
    let output = run_python_adapter(
        &programbench_omp_agent_path(),
        r#"import json
usage = mod.omp_token_usage_from_output('{"usage":{"input_tokens":50,"cached_input_tokens":20,"output_tokens":5,"reasoning_output_tokens":2}}\n')
print(json.dumps(usage))
"#,
    );
    let usage: serde_json::Value = serde_json::from_str(&output).expect("omp usage should be JSON");

    assert_eq!(usage["input_tokens"], 50);
    assert_eq!(usage["input_plus_output_tokens"], 55);
    assert_eq!(usage["uncached_input_plus_output_tokens"], 35);
}

#[test]
fn programbench_codex_adapter_counts_first_usage_candidate_per_event() {
    let output = run_python_adapter(
        &programbench_codex_agent_path(),
        r#"import json
event = {
    "usage": {"input_tokens": 10, "cached_input_tokens": 3, "output_tokens": 2},
    "payload": {"usage": {"input_tokens": 10, "cached_input_tokens": 3, "output_tokens": 2}},
}
usage = mod.codex_token_usage_from_output(json.dumps(event) + "\n")
print(json.dumps(usage))
"#,
    );
    let usage: serde_json::Value =
        serde_json::from_str(&output).expect("codex duplicate usage should be JSON");

    assert_eq!(usage["turns"], 1);
    assert_eq!(usage["input_tokens"], 10);
    assert_eq!(usage["cached_input_tokens"], 3);
    assert_eq!(usage["output_tokens"], 2);
    assert_eq!(usage["input_plus_output_tokens"], 12);
    assert_eq!(usage["uncached_input_plus_output_tokens"], 9);
}

#[test]
fn programbench_omp_adapter_counts_first_usage_candidate_per_event() {
    let output = run_python_adapter(
        &programbench_omp_agent_path(),
        r#"import json
event = {
    "usage": {"input_tokens": 8, "cached_input_tokens": 2, "output_tokens": 4},
    "payload": {"usage": {"input_tokens": 8, "cached_input_tokens": 2, "output_tokens": 4}},
}
usage = mod.omp_token_usage_from_output(json.dumps(event) + "\n")
print(json.dumps(usage))
"#,
    );
    let usage: serde_json::Value =
        serde_json::from_str(&output).expect("omp duplicate usage should be JSON");

    assert_eq!(usage["turns"], 1);
    assert_eq!(usage["input_tokens"], 8);
    assert_eq!(usage["cached_input_tokens"], 2);
    assert_eq!(usage["output_tokens"], 4);
    assert_eq!(usage["input_plus_output_tokens"], 12);
    assert_eq!(usage["uncached_input_plus_output_tokens"], 10);
}

#[test]
fn programbench_adapter_observes_total_only_usage() {
    let output = run_python_adapter(
        &programbench_codex_agent_path(),
        r#"import json
usage = {
    "total_tokens": mod.token_usage_from_value({"total_tokens": 77}),
    "token_count": mod.token_usage_from_value({"token_count": 13}),
}
print(json.dumps(usage))
"#,
    );

    let usage: serde_json::Value =
        serde_json::from_str(&output).expect("total-only usage should be JSON");

    assert_eq!(usage["total_tokens"]["turns"], 1);
    assert_eq!(usage["total_tokens"]["input_plus_output_tokens"], 0);
    assert_eq!(usage["token_count"]["turns"], 1);
    assert_eq!(usage["token_count"]["input_plus_output_tokens"], 0);
}

#[test]
fn programbench_report_aggregates_official_score_and_efficiency() {
    let root = temp_root("stateful-bench-programbench-report");
    let condition_dir = root.join("conditions/stateful-on_subagent-on");
    fs::create_dir_all(condition_dir.join("instance-a")).expect("instance dir should exist");
    fs::create_dir_all(condition_dir.join("instance-b")).expect("instance dir should exist");
    fs::create_dir_all(condition_dir.join("_stats")).expect("stats dir should exist");

    let metadata = ProgramBenchConditionMetadata {
        run_id: "pb-dev".to_string(),
        condition_id: "stateful-on_subagent-on".to_string(),
        condition: ProgramBenchCondition::new(true, true),
        agent: ProgramBenchAgentKind::CodexCli,
        started_at_ms: 10,
        finished_at_ms: 4010,
        running_time_ms: 4000,
        instances: vec![
            instance_metadata(
                "instance-a",
                None,
                Some(true),
                token_usage(2, 100, 40, 12, 5),
            ),
            instance_metadata(
                "instance-b",
                Some("agent exited 1"),
                Some(false),
                token_usage(1, 50, 20, 5, 2),
            ),
        ],
    };
    fs::write(
        condition_dir.join("condition.json"),
        serde_json::to_string_pretty(&metadata).expect("metadata should serialize"),
    )
    .expect("condition metadata should write");
    fs::write(
        condition_dir.join("_stats/score.json"),
        r#"{
          "instance-a": {"test-a": true, "test-b": true},
          "instance-b": {"test-c": true, "test-d": false}
        }"#,
    )
    .expect("score should write");

    let report = build_programbench_condition_report(&condition_dir).expect("report should build");

    assert_eq!(report.condition_id, "stateful-on_subagent-on");
    assert_eq!(report.instances, 2);
    assert_eq!(report.evaluated_instances, 2);
    assert_eq!(report.agent_error_count, 1);
    assert_eq!(report.average_score, Some(0.75));
    assert_eq!(report.resolved_count, 1);
    assert_eq!(report.resolved_rate, Some(0.5));
    assert_eq!(report.running_time_ms, 4000);
    assert_eq!(report.average_running_time_ms, Some(2000.0));
    assert_eq!(report.token_observed_instances, 2);
    assert_eq!(report.token_usage_turns, 3);
    assert_eq!(report.token_input_plus_output_tokens, 167);
    assert_eq!(report.token_uncached_input_plus_output_tokens, 107);
    assert_eq!(report.average_input_plus_output_tokens, Some(83.5));
    assert_eq!(report.average_uncached_input_plus_output_tokens, Some(53.5));
    assert_eq!(report.subagent_observed_instances, 2);
    assert_eq!(report.subagent_used_count, 1);
    assert_eq!(report.subagent_used_rate, Some(0.5));
    assert_eq!(report.score_source, "score-json");
    assert_eq!(report.instance_reports.len(), 2);
    let instance_a = report
        .instance_reports
        .iter()
        .find(|instance| instance.instance_id == "instance-a")
        .expect("instance-a should be reported");
    assert_eq!(instance_a.score, Some(1.0));
    assert_eq!(instance_a.running_time_ms, 1000);
    assert_eq!(instance_a.token_input_plus_output_tokens, 112);
    assert_eq!(instance_a.token_uncached_input_plus_output_tokens, 72);

    fs::remove_dir_all(root).expect("temp root should clean up");
}

#[test]
fn programbench_report_does_not_resolve_rounded_partial_scores() {
    let root = temp_root("stateful-bench-programbench-rounding");
    let condition_dir = root.join("conditions/stateful-on_subagent-on");
    fs::create_dir_all(condition_dir.join("_stats")).expect("stats dir should exist");

    let metadata = ProgramBenchConditionMetadata {
        run_id: "pb-dev".to_string(),
        condition_id: "stateful-on_subagent-on".to_string(),
        condition: ProgramBenchCondition::new(true, true),
        agent: ProgramBenchAgentKind::CodexCli,
        started_at_ms: 10,
        finished_at_ms: 1010,
        running_time_ms: 1000,
        instances: vec![instance_metadata(
            "instance-near-perfect",
            None,
            Some(false),
            token_usage(1, 1, 0, 1, 0),
        )],
    };
    fs::write(
        condition_dir.join("condition.json"),
        serde_json::to_string_pretty(&metadata).expect("metadata should serialize"),
    )
    .expect("condition metadata should write");

    let mut tests = BTreeMap::new();
    for index in 0..2000 {
        tests.insert(format!("test-{index}"), index != 0);
    }
    let score = BTreeMap::from([("instance-near-perfect".to_string(), tests)]);
    fs::write(
        condition_dir.join("_stats/score.json"),
        serde_json::to_string_pretty(&score).expect("score should serialize"),
    )
    .expect("score should write");

    let report = build_programbench_condition_report(&condition_dir).expect("report should build");

    assert_eq!(report.average_score, Some(1.0));
    assert_eq!(report.resolved_count, 0);
    assert_eq!(report.resolved_rate, Some(0.0));

    fs::remove_dir_all(root).expect("temp root should clean up");
}

#[test]
fn programbench_compare_reports_score_time_and_token_deltas() {
    let off = condition_report(
        "stateful-off_subagent-off",
        false,
        false,
        0.5,
        6000,
        220,
        140,
    );
    let on = condition_report(
        "stateful-on_subagent-off",
        true,
        false,
        0.75,
        4000,
        167,
        107,
    );

    let comparison = compare_programbench_reports(vec![off, on]);

    assert_eq!(comparison.stateful_score_delta_without_subagent, Some(0.25));
    assert_eq!(
        comparison.stateful_running_time_ms_delta_without_subagent,
        Some(-2000)
    );
    assert_eq!(
        comparison.stateful_input_plus_output_tokens_delta_without_subagent,
        Some(-53)
    );
    assert_eq!(
        comparison.missing_axis_ids,
        vec![
            "stateful-off_subagent-on".to_string(),
            "stateful-on_subagent-on".to_string(),
        ]
    );
}

#[test]
fn programbench_compare_reports_requires_common_instances_for_deltas() {
    let off_off = condition_report_with_instances(
        "stateful-off_subagent-off",
        false,
        false,
        0.25,
        1000,
        100,
        80,
        &["baseline-only"],
    );
    let on_off = condition_report_with_instances(
        "stateful-on_subagent-off",
        true,
        false,
        1.0,
        10,
        5,
        4,
        &["stateful-only"],
    );
    let off_on = condition_report_with_instances(
        "stateful-off_subagent-on",
        false,
        true,
        0.75,
        20,
        7,
        6,
        &["subagent-only"],
    );

    let comparison = compare_programbench_reports(vec![off_off, on_off, off_on]);

    assert_eq!(comparison.stateful_score_delta_without_subagent, None);
    assert_eq!(
        comparison.stateful_running_time_ms_delta_without_subagent,
        None
    );
    assert_eq!(
        comparison.stateful_input_plus_output_tokens_delta_without_subagent,
        None
    );
    assert_eq!(comparison.subagent_score_delta_without_stateful, None);
    assert_eq!(
        comparison.subagent_running_time_ms_delta_without_stateful,
        None
    );
    assert_eq!(
        comparison.subagent_input_plus_output_tokens_delta_without_stateful,
        None
    );
    assert_eq!(
        comparison.instance_set_mismatches,
        vec![
            "stateful-on_subagent-off vs stateful-off_subagent-off: 0 common instance(s), 1 left-only, 1 right-only".to_string(),
            "stateful-off_subagent-on vs stateful-off_subagent-off: 0 common instance(s), 1 left-only, 1 right-only".to_string(),
        ]
    );
    let markdown = comparison
        .render(ReportFormat::Markdown)
        .expect("comparison markdown should render");
    assert!(markdown.contains("- Instance set mismatches: stateful-on_subagent-off vs stateful-off_subagent-off: 0 common instance(s), 1 left-only, 1 right-only; stateful-off_subagent-on vs stateful-off_subagent-off: 0 common instance(s), 1 left-only, 1 right-only"));
}

#[test]
fn programbench_compare_reports_diagnoses_missing_instance_details() {
    let mut off = condition_report(
        "stateful-off_subagent-off",
        false,
        false,
        0.5,
        1000,
        100,
        80,
    );
    let mut on = condition_report("stateful-on_subagent-off", true, false, 1.0, 2000, 200, 160);
    off.instance_reports.clear();
    on.instance_reports.clear();

    let comparison = compare_programbench_reports(vec![off, on]);

    assert_eq!(comparison.stateful_score_delta_without_subagent, None);
    assert_eq!(
        comparison.stateful_running_time_ms_delta_without_subagent,
        None
    );
    assert_eq!(
        comparison.stateful_input_plus_output_tokens_delta_without_subagent,
        None
    );
    assert_eq!(
        comparison.instance_set_mismatches,
        vec![
            "stateful-on_subagent-off vs stateful-off_subagent-off: 0 common instance(s), 0 left-only, 0 right-only".to_string(),
        ]
    );
}

#[test]
fn programbench_compare_reports_diagnoses_combined_interaction_instance_mismatch() {
    let off_off = condition_report_with_instances(
        "stateful-off_subagent-off",
        false,
        false,
        0.25,
        1000,
        100,
        80,
        &["shared"],
    );
    let on_off = condition_report_with_instances(
        "stateful-on_subagent-off",
        true,
        false,
        0.5,
        1000,
        100,
        80,
        &["shared"],
    );
    let off_on = condition_report_with_instances(
        "stateful-off_subagent-on",
        false,
        true,
        0.75,
        1000,
        100,
        80,
        &["shared"],
    );
    let on_on = condition_report_with_instances(
        "stateful-on_subagent-on",
        true,
        true,
        1.0,
        1000,
        100,
        80,
        &["combined-only"],
    );

    let comparison = compare_programbench_reports(vec![off_off, on_off, off_on, on_on]);

    assert_eq!(comparison.combined_interaction_score_delta, None);
    assert!(comparison.instance_set_mismatches.contains(&"combined interaction: 0 common instance(s) across stateful-off_subagent-off, stateful-on_subagent-off, stateful-off_subagent-on, stateful-on_subagent-on".to_string()));
}

fn instance_metadata(
    instance_id: &str,
    error: Option<&str>,
    subagent_used: Option<bool>,
    token_usage: ProgramBenchTokenUsage,
) -> ProgramBenchInstanceMetadata {
    ProgramBenchInstanceMetadata {
        instance_id: instance_id.to_string(),
        condition_id: "stateful-on_subagent-on".to_string(),
        agent: ProgramBenchAgentKind::CodexCli,
        started_at_ms: 10,
        finished_at_ms: 1010,
        running_time_ms: 1000,
        submission_path: format!("{instance_id}/submission.tar.gz"),
        exit_code: Some(if error.is_some() { 1 } else { 0 }),
        error: error.map(str::to_string),
        subagent_used,
        token_usage: Some(token_usage),
    }
}

fn token_usage(
    turns: usize,
    input_tokens: u64,
    cached_input_tokens: u64,
    output_tokens: u64,
    reasoning_output_tokens: u64,
) -> ProgramBenchTokenUsage {
    ProgramBenchTokenUsage {
        turns,
        input_tokens,
        cached_input_tokens,
        output_tokens,
        reasoning_output_tokens,
        input_plus_output_tokens: input_tokens + output_tokens,
        uncached_input_tokens: input_tokens.saturating_sub(cached_input_tokens),
        uncached_input_plus_output_tokens: input_tokens.saturating_sub(cached_input_tokens)
            + output_tokens,
    }
}

fn condition_report(
    condition_id: &str,
    stateful: bool,
    subagent: bool,
    score: f64,
    running_time_ms: u64,
    input_plus_output: u64,
    uncached_input_plus_output: u64,
) -> ProgramBenchConditionReport {
    condition_report_with_instances(
        condition_id,
        stateful,
        subagent,
        score,
        running_time_ms,
        input_plus_output,
        uncached_input_plus_output,
        &["instance-a", "instance-b"],
    )
}

fn condition_report_with_instances(
    condition_id: &str,
    stateful: bool,
    subagent: bool,
    score: f64,
    running_time_ms: u64,
    input_plus_output: u64,
    uncached_input_plus_output: u64,
    instance_ids: &[&str],
) -> ProgramBenchConditionReport {
    let instance_count = instance_ids.len();
    let first_running_time_ms = running_time_ms / instance_count as u64;
    let last_running_time_ms =
        running_time_ms - first_running_time_ms * (instance_count.saturating_sub(1) as u64);
    let first_input_plus_output = input_plus_output / instance_count as u64;
    let last_input_plus_output =
        input_plus_output - first_input_plus_output * (instance_count.saturating_sub(1) as u64);
    let first_uncached_input_plus_output = uncached_input_plus_output / instance_count as u64;
    let last_uncached_input_plus_output = uncached_input_plus_output
        - first_uncached_input_plus_output * (instance_count.saturating_sub(1) as u64);
    let instance_reports = instance_ids
        .iter()
        .enumerate()
        .map(|(index, instance_id)| ProgramBenchInstanceReport {
            instance_id: (*instance_id).to_string(),
            score: Some(score),
            running_time_ms: if index + 1 == instance_count {
                last_running_time_ms
            } else {
                first_running_time_ms
            },
            token_input_plus_output_tokens: if index + 1 == instance_count {
                last_input_plus_output
            } else {
                first_input_plus_output
            },
            token_uncached_input_plus_output_tokens: if index + 1 == instance_count {
                last_uncached_input_plus_output
            } else {
                first_uncached_input_plus_output
            },
            subagent_used: None,
        })
        .collect::<Vec<_>>();

    ProgramBenchConditionReport {
        run_id: "pb-dev".to_string(),
        condition_id: condition_id.to_string(),
        condition: ProgramBenchCondition::new(stateful, subagent),
        instances: instance_count,
        attempted_instances: instance_count,
        evaluated_instances: instance_count,
        average_score: Some(score),
        resolved_count: 0,
        resolved_rate: Some(0.0),
        eval_error_count: 0,
        agent_error_count: 0,
        timeout_count: 0,
        running_time_ms,
        average_running_time_ms: Some(running_time_ms as f64 / instance_count as f64),
        token_observed_instances: instance_count,
        token_usage_turns: instance_count * 2,
        token_input_tokens: 0,
        token_cached_input_tokens: 0,
        token_output_tokens: 0,
        token_reasoning_output_tokens: 0,
        token_input_plus_output_tokens: input_plus_output,
        token_uncached_input_tokens: 0,
        token_uncached_input_plus_output_tokens: uncached_input_plus_output,
        average_input_plus_output_tokens: Some(input_plus_output as f64 / instance_count as f64),
        average_uncached_input_plus_output_tokens: Some(
            uncached_input_plus_output as f64 / instance_count as f64,
        ),
        subagent_observed_instances: 0,
        subagent_used_count: 0,
        subagent_used_rate: None,
        score_per_million_input_plus_output_tokens: Some(
            score * 1_000_000.0 / input_plus_output as f64,
        ),
        score_per_million_uncached_input_plus_output_tokens: Some(
            score * 1_000_000.0 / uncached_input_plus_output as f64,
        ),
        score_per_hour: Some(score * 3_600_000.0 / running_time_ms as f64),
        score_source: "score-json".to_string(),
        instance_reports,
    }
}

fn programbench_codex_agent_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("scripts/programbench_codex_agent.py")
}

fn programbench_omp_agent_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("scripts/programbench_omp_agent.py")
}

fn run_python_adapter(script_path: &Path, body: &str) -> String {
    let python = format!(
        r#"import importlib.util
import pathlib
import sys

script_path = pathlib.Path({script_path:?})
spec = importlib.util.spec_from_file_location("programbench_agent_under_test", script_path)
mod = importlib.util.module_from_spec(spec)
assert spec.loader is not None
spec.loader.exec_module(mod)
{body}
"#,
        script_path = script_path,
        body = body,
    );
    let output = ProcessCommand::new("python3")
        .arg("-c")
        .arg(python)
        .output()
        .expect("python3 should run adapter import test");

    assert!(
        output.status.success(),
        "python adapter import test failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    String::from_utf8(output.stdout).expect("python stdout should be UTF-8")
}

fn fake_executable(root: &Path, name: &str, body: &str) -> PathBuf {
    let path = root.join(name);
    let mut file = fs::File::create(&path).expect("fake executable should create");
    file.write_all(body.as_bytes())
        .expect("fake executable should write");
    #[cfg(unix)]
    {
        let mut permissions = fs::metadata(&path)
            .expect("fake executable metadata should load")
            .permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&path, permissions).expect("fake executable should be executable");
    }
    path
}

fn temp_root(prefix: &str) -> PathBuf {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock should be after unix epoch")
        .as_nanos();
    std::env::temp_dir().join(format!("{prefix}-{unique}"))
}

fn has_arg(args: &[String], value: &str) -> bool {
    args.iter().any(|arg| arg == value)
}
