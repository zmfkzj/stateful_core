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
        archive_error: Some("archive failed".to_string()),
        workspace_copy_error: None,
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
fn programbench_condition_parser_accepts_axes_and_defaults_to_subagent_on() {
    let explicit_off =
        parse_programbench_condition("stateful:on,subagent:off").expect("condition should parse");

    assert!(explicit_off.stateful);
    assert!(!explicit_off.subagent);
    assert_eq!(explicit_off.id(), "stateful-on_subagent-off");

    let default_subagent =
        parse_programbench_condition("stateful:off").expect("condition should parse");
    assert!(!default_subagent.stateful);
    assert!(default_subagent.subagent);
    assert_eq!(default_subagent.id(), "stateful-off_subagent-on");

    assert_eq!(
        default_programbench_conditions()
            .iter()
            .map(ProgramBenchCondition::id)
            .collect::<Vec<_>>(),
        vec!["stateful-off_subagent-on", "stateful-on_subagent-on"]
    );
}

#[test]
fn programbench_run_uses_subagent_on_stateful_axis_when_no_conditions_passed() {
    let conditions = planned_programbench_conditions(&[]).expect("default conditions should build");
    assert_eq!(
        conditions
            .iter()
            .map(ProgramBenchCondition::id)
            .collect::<Vec<_>>(),
        vec!["stateful-off_subagent-on", "stateful-on_subagent-on"]
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
    exit 0
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
printf '#!/bin/sh\nprintf ok > executable\n' > compile.sh
printf '{{"usage":{{"input_tokens":7,"output_tokens":5}}}}\n'
"#,
            codex_log.display()
        ),
    );
    let stateful = fake_executable(
        &root,
        "fake-stateful",
        r#"#!/bin/sh
exit 0
"#,
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
            stateful_binary: stateful.to_string_lossy().into_owned(),
            codex_bin: codex.to_string_lossy().into_owned(),
            omp_bin: "omp".to_string(),
            agent_docker_image: None,
            agent_docker_omp_bin: "omp".to_string(),
            agent_docker_stateful_binary: "/usr/local/bin/stateful".to_string(),
            agent_docker_home: "/home/stateful".to_string(),
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
        "run -d --init --network none -w /workspace --name stateful-bench-pb-dev-stateful-on_subagent-off-owner__repo-abc123 programbench/fake-image:task_cleanroom_v6 sleep infinity"
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
    exit 0
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
        agent_docker_image: None,
        agent_docker_omp_bin: "omp".to_string(),
        agent_docker_stateful_binary: "/usr/local/bin/stateful".to_string(),
        agent_docker_home: "/home/stateful".to_string(),
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
        agent_docker_image: None,
        agent_docker_omp_bin: "omp".to_string(),
        agent_docker_stateful_binary: "/usr/local/bin/stateful".to_string(),
        agent_docker_home: "/home/stateful".to_string(),
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
        agent_docker_image: None,
        agent_docker_omp_bin: "omp".to_string(),
        agent_docker_stateful_binary: "/usr/local/bin/stateful".to_string(),
        agent_docker_home: "/home/stateful".to_string(),
    })
    .expect("omp agent command should build");

    assert!(command.program.ends_with("programbench_omp_agent.py"));
    assert!(has_arg(&command.args, "--subagent"));
    assert!(!has_arg(&command.args, "--stateful"));
}

#[test]
fn programbench_omp_agent_command_passes_agent_docker_options() {
    let command = build_programbench_agent_command(ProgramBenchInstanceRunOptions {
        agent: ProgramBenchAgentKind::OmpCli,
        condition: ProgramBenchCondition::new(true, false),
        instance_id: "ajeetdsouza__zoxide.67ca1bc".to_string(),
        container_id: "programbench-container".to_string(),
        condition_dir: "runs/pb/conditions/stateful-on_subagent-off".into(),
        docker_bin: "docker".to_string(),
        codex_bin: "codex".to_string(),
        omp_bin: "host-omp".to_string(),
        stateful_binary: "/host/stateful".to_string(),
        model: None,
        benchmark_max_turns: 500,
        timeout_seconds: 7200,
        subagent_min_count: 3,
        agent_docker_image: Some("stateful/omp-agent:test".to_string()),
        agent_docker_omp_bin: "/opt/omp/bin/omp".to_string(),
        agent_docker_stateful_binary: "/opt/stateful/bin/stateful".to_string(),
        agent_docker_home: "/home/bench-agent".to_string(),
    })
    .expect("omp agent command should build");

    assert!(command.program.ends_with("programbench_omp_agent.py"));
    assert!(has_arg_pair(
        &command.args,
        "--agent-docker-image",
        "stateful/omp-agent:test",
    ));
    assert!(has_arg_pair(
        &command.args,
        "--agent-docker-omp-bin",
        "/opt/omp/bin/omp",
    ));
    assert!(has_arg_pair(
        &command.args,
        "--agent-docker-stateful-binary",
        "/opt/stateful/bin/stateful",
    ));
    assert!(has_arg_pair(
        &command.args,
        "--agent-docker-home",
        "/home/bench-agent",
    ));
}

#[test]
fn programbench_codex_adapter_runs_host_agent_and_container_smoke_compile() {
    let output = run_python_adapter(
        &programbench_codex_agent_path(),
        r#"import json
import subprocess
import types

calls = []

def fake_run(command, **kwargs):
    calls.append({
        "command": command,
        "timeout": kwargs.get("timeout"),
        "cwd": str(kwargs.get("cwd")),
        "home": (kwargs.get("env") or {}).get("HOME"),
        "stateful_home": (kwargs.get("env") or {}).get("STATEFUL_HOME"),
    })
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
agent_call_record = next(call for call in calls if call["command"] and call["command"][0] == "codex")
print(json.dumps({
    "calls": calls,
    "agent_command": agent_call_record["command"],
    "agent_cwd": agent_call_record["cwd"],
    "agent_home": agent_call_record["home"],
    "agent_stateful_home": agent_call_record["stateful_home"],
    "prompt": prompt,
    "returncode": result.returncode,
}))
"#,
    );
    let observed: serde_json::Value =
        serde_json::from_str(&output).expect("captured calls should be JSON");

    assert_eq!(
        observed["calls"][0]["command"],
        serde_json::json!([
            "/usr/local/bin/stateful",
            "install",
            "--agent",
            "codex",
            "--yes"
        ])
    );
    assert_eq!(
        observed["calls"][1]["command"],
        serde_json::json!(["git", "init", "-q"])
    );
    assert_eq!(
        observed["calls"][2]["command"],
        serde_json::json!([
            "/usr/local/bin/stateful",
            "enable",
            "--repo",
            "/tmp/programbench-airlock"
        ])
    );
    assert_eq!(
        observed["agent_command"],
        serde_json::json!([
            "codex",
            "-c",
            "sandbox_workspace_write.network_access=false",
            "exec",
            "--json",
            "--ignore-rules",
            "--skip-git-repo-check",
            "--ephemeral",
            "--cd",
            "/tmp/programbench-airlock",
            "--sandbox",
            "workspace-write",
            "--model",
            "gpt-5.4-mini",
            observed["prompt"]
        ])
    );
    assert_eq!(observed["agent_cwd"], "/tmp/programbench-airlock");
    assert_eq!(observed["agent_home"], "/tmp/programbench-airlock");
    assert_eq!(
        observed["agent_stateful_home"],
        "/tmp/programbench-airlock/.stateful"
    );
    assert!(
        observed["prompt"]
            .as_str()
            .expect("prompt should be a string")
            .contains("Benchmark max turns: 321.")
    );
    assert_eq!(observed["returncode"], 0);
}

#[test]
fn programbench_codex_adapter_enables_native_subagents_when_requested() {
    let output = run_python_adapter(
        &programbench_codex_agent_path(),
        r#"import json
import subprocess
import types

def fake_run(command, **kwargs):
    return subprocess.CompletedProcess(command, 0, stdout='{"usage":{"input_tokens":1}}\n', stderr="")

mod.subprocess.run = fake_run
args = types.SimpleNamespace(
    docker_bin="docker",
    container_id="programbench-container",
    codex_bin="codex",
    stateful_binary="/usr/local/bin/stateful",
    model=None,
    benchmark_max_turns=321,
    timeout_seconds=123,
    stateful=False,
    subagent=True,
    subagent_min_count=3,
)
result = mod.run_agent(args, mod.prompt_for_args(args))
print(json.dumps({"command": result.args}))
"#,
    );
    let observed: serde_json::Value =
        serde_json::from_str(&output).expect("captured command should be JSON");
    assert!(
        observed["command"]
            .as_array()
            .expect("command should be array")
            .iter()
            .any(|arg| arg == "features.multi_agent=true")
    );
}

#[test]
fn programbench_adapter_resolves_stateful_binary_before_airlock_chdir() {
    let output = run_python_adapter(
        &programbench_codex_agent_path(),
        r#"import json
import subprocess
import types

calls = []

def fake_run(command, **kwargs):
    calls.append({"command": command})
    return subprocess.CompletedProcess(command, 0, stdout="", stderr="")

mod.subprocess.run = fake_run
args = types.SimpleNamespace(stateful_binary="./stateful", timeout_seconds=123)
mod.run_stateful_command(args, "/tmp/programbench-airlock", "install")
print(json.dumps(calls[-1]["command"]))
"#,
    );
    let observed: serde_json::Value =
        serde_json::from_str(&output).expect("stateful command should be JSON");
    let binary = observed[0].as_str().expect("binary should be a string");
    assert_ne!(binary, "./stateful");
    assert!(binary.ends_with("/stateful"));
}

#[test]
fn programbench_adapter_leaves_bare_stateful_binary_for_path_lookup() {
    let output = run_python_adapter(
        &programbench_codex_agent_path(),
        r#"import json
import subprocess
import types

calls = []

def fake_run(command, **kwargs):
    calls.append({"command": command})
    return subprocess.CompletedProcess(command, 0, stdout="", stderr="")

mod.subprocess.run = fake_run
args = types.SimpleNamespace(stateful_binary="stateful", timeout_seconds=123)
mod.run_stateful_command(args, "/tmp/programbench-airlock", "install")
print(json.dumps(calls[-1]["command"]))
"#,
    );
    let observed: serde_json::Value =
        serde_json::from_str(&output).expect("stateful command should be JSON");
    assert_eq!(observed[0], "stateful");
}

#[test]
fn programbench_airlock_env_scrubs_parent_stateful_values() {
    let output = run_python_adapter(
        &programbench_codex_agent_path(),
        r#"import json
import os

for key in [
    "STATEFUL_SERVER_URL",
    "STATEFUL_SERVER_TOKEN",
]:
    os.environ[key] = "parent"

env = mod.airlock_env("/tmp/programbench-airlock")
print(json.dumps({
    key: env.get(key)
    for key in [
        "HOME",
        "STATEFUL_HOME",
        "CODEX_HOME",
        "STATEFUL_SERVER_URL",
        "STATEFUL_SERVER_TOKEN",
    ]
}, sort_keys=True))
"#,
    );
    let observed: serde_json::Value =
        serde_json::from_str(&output).expect("airlock env should be JSON");

    assert_eq!(observed["HOME"], "/tmp/programbench-airlock");
    assert_eq!(
        observed["STATEFUL_HOME"],
        "/tmp/programbench-airlock/.stateful"
    );
    assert_eq!(observed["CODEX_HOME"], "/tmp/programbench-airlock/.codex");
    assert_eq!(observed["STATEFUL_SERVER_URL"], serde_json::Value::Null);
    assert_eq!(observed["STATEFUL_SERVER_TOKEN"], serde_json::Value::Null);
}

#[test]
fn programbench_prompt_omits_host_docker_command() {
    let output = run_python_adapter(
        &programbench_codex_agent_path(),
        r#"import json
import types

args = types.SimpleNamespace(
    docker_bin="./docker-shim",
    container_id="programbench-container",
    benchmark_max_turns=None,
    subagent=False,
)
print(json.dumps({"prompt": mod.prompt_for_args(args)}))
"#,
    );
    let observed: serde_json::Value = serde_json::from_str(&output).expect("prompt should be JSON");
    let prompt = observed["prompt"]
        .as_str()
        .expect("prompt should be a string");
    assert!(!prompt.contains("docker-shim"));
    assert!(prompt.contains("The current directory is the ProgramBench workspace"));
}

#[test]
fn programbench_codex_adapter_resolves_relative_codex_binary_before_airlock_chdir() {
    let output = run_python_adapter(
        &programbench_codex_agent_path(),
        r#"import json
import subprocess
import types

def fake_run(command, **kwargs):
    return subprocess.CompletedProcess(command, 0, stdout='{"usage":{"input_tokens":1}}\n', stderr="")

mod.subprocess.run = fake_run
args = types.SimpleNamespace(
    docker_bin="docker",
    container_id="programbench-container",
    codex_bin="./codex",
    stateful_binary="/usr/local/bin/stateful",
    model=None,
    benchmark_max_turns=321,
    timeout_seconds=123,
    stateful=False,
    subagent=False,
    subagent_min_count=3,
)
result = mod.run_agent(args, mod.prompt_for_args(args))
print(json.dumps({"command": result.args}))
"#,
    );
    let observed: serde_json::Value =
        serde_json::from_str(&output).expect("captured command should be JSON");
    let binary = observed["command"][0]
        .as_str()
        .expect("binary should be a string");
    assert_ne!(binary, "./codex");
    assert!(binary.ends_with("/codex"));
}

#[test]
fn programbench_omp_adapter_leaves_relative_omp_binary_for_path_lookup() {
    let output = run_python_adapter(
        &programbench_omp_agent_path(),
        r#"import json
import subprocess
import types

def fake_run(command, **kwargs):
    return subprocess.CompletedProcess(command, 0, stdout='{"usage":{"input_tokens":1}}\n', stderr="")


def fake_omp(command, *, cwd, env, timeout_seconds):
    return subprocess.CompletedProcess(command, 0, stdout='{"usage":{"input_tokens":1}}\n', stderr="")

mod.subprocess.run = fake_run
mod.run_omp_command = fake_omp
mod.omp_auth_source_agent_dir = lambda env: None
args = types.SimpleNamespace(
    docker_bin="docker",
    container_id="programbench-container",
    omp_bin="./omp",
    stateful_binary="/usr/local/bin/stateful",
    model=None,
    benchmark_max_turns=321,
    timeout_seconds=123,
    stateful=False,
    subagent=False,
    subagent_min_count=3,
)
result = mod.run_agent(args, mod.prompt_for_args(args))
print(json.dumps({"command": result.args}))
"#,
    );
    let observed: serde_json::Value =
        serde_json::from_str(&output).expect("captured command should be JSON");
    assert_eq!(observed["command"][0], "./omp");
}

#[test]
fn programbench_adapter_initializes_airlock_git_repo_before_stateful_enable() {
    let output = run_python_adapter(
        &programbench_codex_agent_path(),
        r#"import json
import subprocess
import types

calls = []

def fake_run(command, **kwargs):
    calls.append({
        "command": command,
        "cwd": str(kwargs.get("cwd")),
        "home": kwargs.get("env", {}).get("HOME"),
    })
    return subprocess.CompletedProcess(command, 0, stdout="", stderr="")

mod.subprocess.run = fake_run
args = types.SimpleNamespace(stateful_binary="/usr/local/bin/stateful", timeout_seconds=123)
mod.enable_stateful_repo(args, "/tmp/programbench-airlock")
print(json.dumps(calls))
"#,
    );
    let observed: serde_json::Value =
        serde_json::from_str(&output).expect("stateful setup calls should be JSON");
    let calls = observed.as_array().expect("calls should be array");

    assert_eq!(
        calls[0]["command"],
        serde_json::json!(["git", "init", "-q"])
    );
    assert_eq!(calls[0]["cwd"], "/tmp/programbench-airlock");
    assert_eq!(calls[0]["home"], "/tmp/programbench-airlock");
    assert_eq!(
        calls[1]["command"],
        serde_json::json!([
            "/usr/local/bin/stateful",
            "enable",
            "--repo",
            "/tmp/programbench-airlock"
        ])
    );
}

#[test]
fn programbench_omp_adapter_runs_host_agent_and_container_smoke_compile() {
    let output = run_python_adapter(
        &programbench_omp_agent_path(),
        r#"import json
import subprocess
import types

calls = []

def fake_run(command, **kwargs):
    calls.append({
        "command": command,
        "timeout": kwargs.get("timeout"),
        "cwd": str(kwargs.get("cwd")),
    })
    return subprocess.CompletedProcess(command, 0, stdout='{"usage":{"input_tokens":1}}\n', stderr="")

def fake_omp(command, *, cwd, env, timeout_seconds):
    calls.append({
        "command": command,
        "timeout": timeout_seconds,
        "cwd": str(cwd),
        "env_home": env.get("HOME"),
        "env_stateful_home": env.get("STATEFUL_HOME"),
        "env_agent_dir": env.get("PI_CODING_AGENT_DIR"),
    })
    return subprocess.CompletedProcess(command, 0, stdout='{"usage":{"input_tokens":1}}\n', stderr="")

mod.subprocess.run = fake_run
mod.run_omp_command = fake_omp
mod.omp_auth_source_agent_dir = lambda env: None
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
agent_call_record = next(call for call in calls if call["command"] and call["command"][0] == "omp")
print(json.dumps({
    "calls": calls,
    "agent_command": agent_call_record["command"],
    "prompt": prompt,
    "returncode": result.returncode,
    "agent_cwd": agent_call_record["cwd"],
    "agent_home": agent_call_record["env_home"],
    "agent_stateful_home": agent_call_record["env_stateful_home"],
    "agent_dir": agent_call_record["env_agent_dir"],
}))
"#,
    );
    let observed: serde_json::Value =
        serde_json::from_str(&output).expect("captured calls should be JSON");

    assert_eq!(
        observed["calls"][0]["command"],
        serde_json::json!([
            "/usr/local/bin/stateful",
            "install",
            "--agent",
            "omp",
            "--yes"
        ])
    );
    assert_eq!(
        observed["calls"][1]["command"],
        serde_json::json!(["git", "init", "-q"])
    );
    assert_eq!(
        observed["calls"][2]["command"],
        serde_json::json!([
            "/usr/local/bin/stateful",
            "enable",
            "--repo",
            "/tmp/programbench-airlock"
        ])
    );
    assert_eq!(
        observed["agent_command"],
        serde_json::json!([
            "omp",
            "--cwd",
            "/tmp/programbench-airlock",
            "--mode",
            "json",
            "--no-session",
            "--approval-mode",
            "yolo",
            "--profile",
            "stateful",
            "--model",
            "gpt-5.4-mini",
            "-p",
            observed["prompt"]
        ])
    );
    assert_eq!(observed["agent_cwd"], "/tmp/programbench-airlock");
    assert_eq!(observed["agent_home"], "/tmp/programbench-airlock");
    assert_eq!(
        observed["agent_stateful_home"],
        "/tmp/programbench-airlock/.stateful"
    );
    assert_eq!(
        observed["agent_dir"],
        "/tmp/programbench-airlock/.omp/profiles/stateful/agent"
    );
    assert!(
        observed["prompt"]
            .as_str()
            .expect("prompt should be a string")
            .contains("Benchmark max turns: 654.")
    );
    assert_eq!(observed["returncode"], 0);
}

#[test]
fn programbench_omp_adapter_runs_agent_docker_container_when_configured() {
    let output = run_python_adapter(
        &programbench_omp_agent_path(),
        r#"import json
import subprocess
import types

docker_calls = []
host_commands = []
host_omp_calls = []

def fake_run(command, **kwargs):
    if command and command[0] == "docker":
        docker_calls.append(command)
        if command[1] == "run":
            return subprocess.CompletedProcess(command, 0, stdout="agent-container-id\n", stderr="")
        if command[1] == "exec" and "/opt/omp/bin/omp" in command:
            return subprocess.CompletedProcess(command, 0, stdout='{"usage":{"input_tokens":11,"output_tokens":7}}\n', stderr="")
        return subprocess.CompletedProcess(command, 0, stdout="", stderr="")
    host_commands.append(command)
    return subprocess.CompletedProcess(command, 0, stdout="", stderr="")

def fake_omp(command, *, cwd, env, timeout_seconds):
    host_omp_calls.append(command)
    return subprocess.CompletedProcess(command, 0, stdout='{"usage":{"input_tokens":11,"output_tokens":7}}\n', stderr="")

mod.subprocess.run = fake_run
mod.run_omp_command = fake_omp
mod.omp_auth_source_agent_dir = lambda env: None
args = types.SimpleNamespace(
    docker_bin="docker",
    container_id="programbench-container",
    omp_bin="host-omp",
    stateful_binary="/host/stateful",
    model="gpt-5.4-mini",
    benchmark_max_turns=654,
    timeout_seconds=456,
    stateful=True,
    subagent=False,
    subagent_min_count=3,
    airlock="/tmp/programbench-airlock",
    agent_docker_image="stateful/omp-agent:test",
    agent_docker_omp_bin="/opt/omp/bin/omp",
    agent_docker_stateful_binary="/opt/stateful/bin/stateful",
    agent_docker_home="/home/bench-agent",
)
result = mod.run_agent(args, mod.prompt_for_args(args))
print(json.dumps({
    "docker_calls": docker_calls,
    "host_commands": host_commands,
    "host_omp_calls": host_omp_calls,
    "returncode": result.returncode,
    "usage": mod.omp_token_usage_from_output(result.stdout),
}))
"#,
    );
    let observed: serde_json::Value =
        serde_json::from_str(&output).expect("captured calls should be JSON");
    let docker_calls = observed["docker_calls"]
        .as_array()
        .expect("docker calls should be array");

    assert_eq!(observed["host_omp_calls"], serde_json::json!([]));
    assert_eq!(observed["host_commands"], serde_json::json!([]));
    assert!(docker_calls.iter().any(|call| call
        == &serde_json::json!([
            "docker",
            "cp",
            "programbench-container:/workspace/.",
            "/tmp/programbench-airlock"
        ])));
    assert!(docker_calls.iter().any(|call| {
        docker_call_starts_with(call, &["docker", "run"])
            && docker_call_contains_arg(call, "stateful/omp-agent:test")
            && docker_call_has_sequence(call, &["sleep", "infinity"])
    }));
    assert!(docker_calls.iter().any(|call| call
        == &serde_json::json!([
            "docker",
            "cp",
            "/tmp/programbench-airlock/.",
            "agent-container-id:/workspace/"
        ])));
    assert!(docker_calls.iter().any(|call| docker_call_execs(
        call,
        "agent-container-id",
        &[
            "/opt/stateful/bin/stateful",
            "install",
            "--agent",
            "omp",
            "--yes"
        ],
    )));
    assert!(docker_calls.iter().any(|call| docker_call_execs(
        call,
        "agent-container-id",
        &["git", "init", "-q"],
    )));
    assert!(docker_calls.iter().any(|call| docker_call_execs(
        call,
        "agent-container-id",
        &[
            "/opt/stateful/bin/stateful",
            "enable",
            "--repo",
            "/workspace"
        ],
    )));
    assert!(docker_calls.iter().any(|call| docker_call_execs(
        call,
        "agent-container-id",
        &[
            "/opt/omp/bin/omp",
            "--cwd",
            "/workspace",
            "--mode",
            "json",
            "--no-session",
            "--approval-mode",
            "yolo",
            "--profile",
            "stateful",
        ],
    )));
    assert!(
        docker_calls
            .iter()
            .any(|call| docker_call_contains_env(call, "HOME=/home/bench-agent"))
    );
    assert!(docker_calls.iter().any(|call| call
        == &serde_json::json!([
            "docker",
            "cp",
            "agent-container-id:/workspace/.",
            "/tmp/programbench-airlock"
        ])));
    assert!(
        docker_calls
            .iter()
            .any(|call| call == &serde_json::json!(["docker", "rm", "-f", "agent-container-id"]))
    );
    assert_eq!(observed["returncode"], 0);
    assert_eq!(observed["usage"]["input_tokens"], 11);
    assert_eq!(observed["usage"]["output_tokens"], 7);
}

#[test]
fn programbench_omp_adapter_disables_nested_omp_sandbox_in_agent_docker() {
    let output = run_python_adapter(
        &programbench_omp_agent_path(),
        r#"import json
import subprocess
import types

docker_calls = []

def fake_run(command, **kwargs):
    if command and command[0] == "docker":
        docker_calls.append(command)
        if command[:2] == ["docker", "run"]:
            return subprocess.CompletedProcess(command, 0, stdout="agent-container-id\n", stderr="")
        if command[:2] == ["docker", "exec"] and "/opt/omp/bin/omp" in command:
            return subprocess.CompletedProcess(command, 0, stdout='{"usage":{"input_tokens":1}}\n', stderr="")
    return subprocess.CompletedProcess(command, 0, stdout="", stderr="")

mod.subprocess.run = fake_run
mod.omp_auth_source_agent_dir = lambda env: None
args = types.SimpleNamespace(
    docker_bin="docker",
    container_id="programbench-container",
    omp_bin="host-omp",
    stateful_binary="/host/stateful",
    model="gpt-5.4-mini",
    benchmark_max_turns=123,
    timeout_seconds=456,
    stateful=True,
    subagent=False,
    subagent_min_count=3,
    airlock="/tmp/programbench-airlock",
    agent_docker_image="stateful/omp-agent:test",
    agent_docker_omp_bin="/opt/omp/bin/omp",
    agent_docker_stateful_binary="/opt/stateful/bin/stateful",
    agent_docker_home="/home/bench-agent",
)
mod.run_agent(args, mod.prompt_for_args(args))
print(json.dumps(docker_calls))
"#,
    );
    let docker_calls: Vec<serde_json::Value> =
        serde_json::from_str(&output).expect("docker calls should be JSON");

    assert!(docker_calls.iter().any(|call| docker_call_execs(
        call,
        "agent-container-id",
        &[
            "/opt/stateful/bin/stateful",
            "install",
            "--agent",
            "omp",
            "--yes"
        ],
    )));
    assert!(docker_calls.iter().any(|call| docker_call_execs(
        call,
        "agent-container-id",
        &[
            "/opt/stateful/bin/stateful",
            "enable",
            "--repo",
            "/workspace"
        ],
    )));
    assert!(docker_calls.iter().any(|call| {
        docker_call_execs(
            call,
            "agent-container-id",
            &["/opt/omp/bin/omp", "--cwd", "/workspace"],
        ) && docker_call_contains_env(call, "STATEFUL_OMP_SANDBOX=off")
    }));
}

#[test]
fn programbench_omp_adapter_records_agent_docker_copy_back_error_separately() {
    let output = run_python_adapter(
        &programbench_omp_agent_path(),
        r#"import json
import subprocess
import tempfile
import types
from pathlib import Path

def fake_run(command, **kwargs):
    if command and command[0] == "docker":
        if command[:2] == ["docker", "run"]:
            return subprocess.CompletedProcess(command, 0, stdout="agent-container-id\n", stderr="")
        if command[:2] == ["docker", "cp"] and command[2] == "agent-container-id:/workspace/.":
            raise RuntimeError("copy denied from agent")
        if command[:2] == ["docker", "exec"] and "/opt/omp/bin/omp" in command:
            return subprocess.CompletedProcess(command, 137, stdout="", stderr="killed\n")
    return subprocess.CompletedProcess(command, 0, stdout="", stderr="")

mod.subprocess.run = fake_run
mod.omp_auth_source_agent_dir = lambda env: None

with tempfile.TemporaryDirectory() as condition_dir:
    args = types.SimpleNamespace(
        condition_dir=condition_dir,
        instance_id="owner__repo.abc123",
        condition_id="stateful-on_subagent-off",
        container_id="programbench-container",
        docker_bin="docker",
        omp_bin="host-omp",
        stateful_binary="/host/stateful",
        model="gpt-5.4-mini",
        benchmark_max_turns=123,
        timeout_seconds=456,
        stateful=True,
        subagent=False,
        subagent_min_count=3,
        agent_docker_image="stateful/omp-agent:test",
        agent_docker_omp_bin="/opt/omp/bin/omp",
        agent_docker_stateful_binary="/opt/stateful/bin/stateful",
        agent_docker_home="/home/bench-agent",
    )
    exit_code = mod.run_main(
        args,
        agent_name="omp-cli",
        exited_error_prefix="omp",
        token_usage_from_output=mod.omp_token_usage_from_output,
        run_agent_func=mod.run_agent,
    )
    metadata = json.loads((Path(condition_dir) / "owner__repo.abc123" / "instance.json").read_text())
    observed = {
        "exit_code": exit_code,
        "metadata_exit_code": metadata.get("exit_code"),
        "error": metadata.get("error"),
        "workspace_copy_error": metadata.get("workspace_copy_error"),
        "smoke_compile_error": metadata.get("smoke_compile_error"),
    }
print(json.dumps(observed))
"#,
    );
    let observed: serde_json::Value =
        serde_json::from_str(&output).expect("adapter observation should be JSON");

    assert_eq!(observed["exit_code"], 137);
    assert_eq!(observed["metadata_exit_code"], 137);
    assert_eq!(observed["error"], "omp exited 137");
    assert_eq!(observed["workspace_copy_error"], "copy denied from agent");
    assert_eq!(observed["smoke_compile_error"], serde_json::Value::Null);
}

#[test]
fn programbench_omp_adapter_does_not_pass_parent_stateful_runtime_to_agent_docker() {
    let output = run_python_adapter(
        &programbench_omp_agent_path(),
        r#"import json
import os
import subprocess
import types

docker_exec_envs = []

def exec_env(command):
    env = {}
    index = 0
    while index < len(command) - 1:
        if command[index] == "-e":
            key, _, value = command[index + 1].partition("=")
            env[key] = value
            index += 2
        else:
            index += 1
    return env

def fake_run(command, **kwargs):
    if command[:2] == ["docker", "run"]:
        return subprocess.CompletedProcess(command, 0, stdout="agent-container-id\n", stderr="")
    if command[:2] == ["docker", "exec"] and (
        "/opt/stateful/bin/stateful" in command or "/opt/omp/bin/omp" in command
    ):
        docker_exec_envs.append(exec_env(command))
        if "/opt/omp/bin/omp" in command:
            return subprocess.CompletedProcess(command, 0, stdout='{"usage":{"input_tokens":1}}\n', stderr="")
    return subprocess.CompletedProcess(command, 0, stdout="", stderr="")

mod.subprocess.run = fake_run
mod.omp_auth_source_agent_dir = lambda env: None
os.environ["STATEFUL_SERVER_URL"] = "http://127.0.0.1:43873"
os.environ["STATEFUL_SERVER_TOKEN"] = "parent-token"

args = types.SimpleNamespace(
    docker_bin="docker",
    container_id="programbench-container",
    omp_bin="host-omp",
    stateful_binary="/host/stateful",
    model="gpt-5.4-mini",
    benchmark_max_turns=123,
    timeout_seconds=456,
    stateful=True,
    subagent=False,
    subagent_min_count=3,
    airlock="/tmp/programbench-airlock",
    agent_docker_image="stateful/omp-agent:test",
    agent_docker_omp_bin="/opt/omp/bin/omp",
    agent_docker_stateful_binary="/opt/stateful/bin/stateful",
    agent_docker_home="/home/bench-agent",
)
mod.run_agent(args, mod.prompt_for_args(args))
print(json.dumps(docker_exec_envs))
"#,
    );
    let docker_exec_envs: Vec<serde_json::Value> =
        serde_json::from_str(&output).expect("captured docker exec envs should be JSON");

    assert_eq!(docker_exec_envs.len(), 3);
    for env in docker_exec_envs {
        assert_eq!(env["HOME"], "/home/bench-agent");
        assert_eq!(
            env["PI_CODING_AGENT_DIR"],
            "/home/bench-agent/.omp/profiles/stateful/agent"
        );
        assert_eq!(env["STATEFUL_HOME"], "/home/bench-agent/.stateful");
        assert_eq!(env["STATEFUL_SERVER_URL"], serde_json::Value::Null);
        assert_eq!(env["STATEFUL_SERVER_TOKEN"], serde_json::Value::Null);
    }
}

#[cfg(unix)]
#[test]
fn programbench_omp_adapter_preserves_execute_only_executable_when_copying_to_agent_container() {
    let output = run_python_adapter(
        &programbench_omp_agent_path(),
        r##"import json
import pathlib
import subprocess
import tempfile
import types

docker_calls = []
cp_modes = []

def fake_run(command, **kwargs):
    docker_calls.append(command)
    if command[:2] == ["docker", "cp"] and command[2].endswith("/."):
        cp_modes.append(executable.stat().st_mode & 0o777)
    return subprocess.CompletedProcess(command, 0, stdout="", stderr="")

mod.subprocess.run = fake_run
args = types.SimpleNamespace(
    docker_bin="docker",
    timeout_seconds=123,
)

with tempfile.TemporaryDirectory() as tmp:
    airlock = pathlib.Path(tmp)
    executable = airlock / "executable"
    executable.write_text("#!/bin/sh\nexit 0\n")
    executable.chmod(0o111)
    (airlock / "README.md").write_text("readable\n")

    mod.copy_airlock_to_agent_container(args, str(airlock), "agent-container-id")

    print(json.dumps({
        "cp_modes": cp_modes,
        "docker_calls": docker_calls,
        "restored_mode": executable.stat().st_mode & 0o777,
    }))
"##,
    );
    let observed: serde_json::Value =
        serde_json::from_str(&output).expect("copy result should be JSON");
    let docker_calls = observed["docker_calls"]
        .as_array()
        .expect("docker calls should be array");

    assert!(
        docker_calls.iter().any(|call| {
            docker_call_starts_with(call, &["docker", "cp"])
                && call
                    .as_array()
                    .and_then(|args| args.get(3))
                    .and_then(serde_json::Value::as_str)
                    == Some("agent-container-id:/workspace/")
        }),
        "execute-only executable should not prevent copying the airlock into the agent container"
    );
    assert_eq!(
        observed["cp_modes"],
        serde_json::json!([0o511]),
        "host executable should be owner-readable only while docker cp reads it"
    );
    assert_eq!(
        observed["restored_mode"],
        serde_json::json!(0o111),
        "host executable mode should be restored after docker cp"
    );
    assert!(
        docker_calls.iter().any(|call| docker_call_execs(
            call,
            "agent-container-id",
            &["chmod", "111", "/workspace/executable"],
        )),
        "copied executable should be chmodded back to its original mode inside the agent container"
    );
}

#[test]
fn programbench_omp_adapter_prepends_stateful_binary_dir_to_agent_path() {
    let output = run_python_adapter(
        &programbench_omp_agent_path(),
        r#"import json
import subprocess
import types

def fake_run(command, **kwargs):
    return subprocess.CompletedProcess(command, 0, stdout="", stderr="")

def fake_omp(command, *, cwd, env, timeout_seconds):
    return subprocess.CompletedProcess(command, 0, stdout=json.dumps({
        "path": env.get("PATH", ""),
        "stateful_dir": "/opt/stateful/bin",
    }), stderr="")

mod.subprocess.run = fake_run
mod.run_omp_command = fake_omp
mod.omp_auth_source_agent_dir = lambda env: None
args = types.SimpleNamespace(
    docker_bin="docker",
    container_id="programbench-container",
    omp_bin="omp",
    stateful_binary="/opt/stateful/bin/stateful",
    model=None,
    benchmark_max_turns=321,
    timeout_seconds=123,
    stateful=True,
    subagent=False,
    subagent_min_count=3,
)
result = mod.run_agent(args, mod.prompt_for_args(args))
print(result.stdout)
"#,
    );
    let observed: serde_json::Value =
        serde_json::from_str(&output).expect("captured env should be JSON");
    let path = observed["path"].as_str().expect("PATH should be a string");

    assert_eq!(
        path.split(':').next(),
        observed["stateful_dir"].as_str(),
        "agent PATH should prefer the stateful binary directory so bare `stateful` works"
    );
}

#[test]
fn programbench_omp_adapter_timeout_preserves_cleanup_denial() {
    let output = run_python_adapter(
        &programbench_omp_agent_path(),
        r#"import json
import subprocess

calls = []

class FakeProcess:
    returncode = None

    def communicate(self, timeout=None):
        raise subprocess.TimeoutExpired(
            ["omp"],
            timeout,
            output="partial stdout",
            stderr="partial stderr",
        )

    def kill(self):
        raise PermissionError(1, "Operation not permitted")

def fake_popen(command, **kwargs):
    calls.append({"command": command, "timeout": kwargs.get("timeout")})
    return FakeProcess()

mod.subprocess.Popen = fake_popen

try:
    mod.run_omp_command(
        ["omp"],
        cwd="/tmp/programbench-airlock",
        env={"HOME": "/tmp/programbench-airlock"},
        timeout_seconds=7,
    )
except subprocess.TimeoutExpired as exc:
    print(json.dumps({
        "output": exc.output,
        "stderr": exc.stderr,
        "cleanup_error": getattr(exc, "cleanup_error", None),
    }))
"#,
    );
    let observed: serde_json::Value =
        serde_json::from_str(&output).expect("timeout should be JSON");

    assert_eq!(observed["output"], "partial stdout");
    assert_eq!(observed["stderr"], "partial stderr");
    assert_eq!(
        observed["cleanup_error"],
        "[Errno 1] Operation not permitted"
    );
}

#[test]
fn programbench_omp_adapter_timeout_does_not_duplicate_cleanup_output() {
    let output = run_python_adapter(
        &programbench_omp_agent_path(),
        r#"import json
import subprocess

class FakeProcess:
    returncode = None
    calls = 0

    def communicate(self, timeout=None):
        self.calls += 1
        if self.calls == 1:
            raise subprocess.TimeoutExpired(
                ["omp"],
                timeout,
                output="partial stdout",
                stderr="partial stderr",
            )
        return ("partial stdout", "partial stderr")

    def kill(self):
        return None

def fake_popen(command, **kwargs):
    return FakeProcess()

mod.subprocess.Popen = fake_popen

try:
    mod.run_omp_command(
        ["omp"],
        cwd="/tmp/programbench-airlock",
        env={"HOME": "/tmp/programbench-airlock"},
        timeout_seconds=7,
    )
except subprocess.TimeoutExpired as exc:
    print(json.dumps({
        "output": exc.output,
        "stderr": exc.stderr,
        "cleanup_error": getattr(exc, "cleanup_error", None),
    }))
"#,
    );
    let observed: serde_json::Value =
        serde_json::from_str(&output).expect("timeout should be JSON");

    assert_eq!(observed["output"], "partial stdout");
    assert_eq!(observed["stderr"], "partial stderr");
    assert_eq!(observed["cleanup_error"], serde_json::Value::Null);
}

#[test]
fn programbench_adapter_metadata_records_timeout_cleanup_error() {
    let output = run_python_adapter(
        &programbench_codex_agent_path(),
        r#"import json
import subprocess
import tempfile
import types
from pathlib import Path

def fake_prompt(args):
    return "prompt"

def fake_archive(args, instance_dir):
    return instance_dir / "submission.tar.gz"

def fake_run_agent(args, prompt):
    exc = subprocess.TimeoutExpired(
        ["omp"],
        args.timeout_seconds,
        output="partial stdout",
        stderr="partial stderr",
    )
    exc.cleanup_error = "[Errno 1] Operation not permitted"
    raise exc

mod.prompt_for_args = fake_prompt
mod.archive_workspace = fake_archive

with tempfile.TemporaryDirectory() as root:
    args = types.SimpleNamespace(
        condition_dir=root,
        instance_id="instance",
        condition_id="stateful-on_subagent-off",
        timeout_seconds=7,
    )
    exit_code = mod.run_main(
        args,
        agent_name="omp-cli",
        exited_error_prefix="omp",
        token_usage_from_output=lambda stdout: {},
        run_agent_func=fake_run_agent,
    )
    instance_dir = Path(root) / "instance"
    metadata = json.loads((instance_dir / "instance.json").read_text())
    print(json.dumps({
        "exit_code": exit_code,
        "metadata_exit_code": metadata["exit_code"],
        "error": metadata["error"],
        "cleanup_error": metadata.get("cleanup_error"),
        "stdout": (instance_dir / "agent.stdout.log").read_text(),
        "stderr": (instance_dir / "agent.stderr.log").read_text(),
    }))
"#,
    );
    let observed: serde_json::Value =
        serde_json::from_str(&output).expect("metadata should be JSON");

    assert_eq!(observed["exit_code"], 124);
    assert_eq!(observed["metadata_exit_code"], 124);
    assert_eq!(observed["error"], "omp timed out after 7s");
    assert_eq!(
        observed["cleanup_error"],
        "[Errno 1] Operation not permitted"
    );
    assert_eq!(observed["stdout"], "partial stdout");
    assert_eq!(observed["stderr"], "partial stderr");
}

#[test]
fn programbench_omp_adapter_passes_parent_stateful_runtime_to_agent() {
    let output = run_python_adapter(
        &programbench_omp_agent_path(),
        r#"import json
import os
import subprocess
import types

agent_env = {}

def fake_run(command, **kwargs):
    if command and command[0] == "omp":
        env = kwargs.get("env", {})
        agent_env.update({
            "stateful_server_url": env.get("STATEFUL_SERVER_URL"),
            "stateful_server_token": env.get("STATEFUL_SERVER_TOKEN"),
            "stateful_home": env.get("STATEFUL_HOME"),
        })
    return subprocess.CompletedProcess(command, 0, stdout='{"usage":{"input_tokens":1}}\n', stderr="")

def fake_omp(command, *, cwd, env, timeout_seconds):
    agent_env.update({
        "stateful_server_url": env.get("STATEFUL_SERVER_URL"),
        "stateful_server_token": env.get("STATEFUL_SERVER_TOKEN"),
        "stateful_home": env.get("STATEFUL_HOME"),
        "auth_source": env.get("OMP_AUTH_SOURCE_AGENT_DIR"),
    })
    return subprocess.CompletedProcess(command, 0, stdout='{"usage":{"input_tokens":1}}\n', stderr="")


mod.subprocess.run = fake_run
mod.run_omp_command = fake_omp
mod.omp_auth_source_agent_dir = lambda env: None
os.environ["STATEFUL_SERVER_URL"] = "http://127.0.0.1:43873"
os.environ["STATEFUL_SERVER_TOKEN"] = "parent-token"
os.environ["OMP_AUTH_SOURCE_AGENT_DIR"] = "/host/source-agent"

args = types.SimpleNamespace(
    docker_bin="docker",
    container_id="programbench-container",
    omp_bin="omp",
    stateful_binary="/usr/local/bin/stateful",
    model="gpt-5.4-mini",
    benchmark_max_turns=123,
    timeout_seconds=456,
    stateful=True,
    subagent=False,
    subagent_min_count=3,
)
mod.run_agent(args, mod.prompt_for_args(args))
print(json.dumps(agent_env))
"#,
    );
    let observed: serde_json::Value =
        serde_json::from_str(&output).expect("captured OMP env should be JSON");

    assert_eq!(observed["stateful_server_url"], "http://127.0.0.1:43873");
    assert_eq!(observed["stateful_server_token"], "parent-token");
    assert_eq!(
        observed["stateful_home"],
        "/tmp/programbench-airlock/.stateful"
    );
    assert_eq!(observed["auth_source"], serde_json::Value::Null);
}

#[test]
fn programbench_omp_adapter_seeds_openai_codex_auth_credentials() {
    let output = run_python_adapter(
        &programbench_omp_agent_path(),
        r#"import json
import os
import sqlite3
import tempfile
from pathlib import Path

with tempfile.TemporaryDirectory() as root:
    source_agent = Path(root) / "source-agent"
    target_agent = Path(root) / "target-agent"
    source_agent.mkdir()
    with sqlite3.connect(source_agent / "agent.db") as db:
        db.execute("""
            CREATE TABLE auth_credentials (
                provider TEXT,
                credential_type TEXT,
                data TEXT,
                disabled_cause TEXT,
                identity_key TEXT,
                created_at INTEGER,
                updated_at INTEGER
            )
        """)
        db.execute(
            """
            INSERT INTO auth_credentials
                (provider, credential_type, data, disabled_cause, identity_key, created_at, updated_at)
            VALUES (?, ?, ?, ?, ?, ?, ?)
            """,
            ("openai-codex", "oauth", "secret-json", None, "identity", 1, 2),
        )
        db.execute(
            """
            INSERT INTO auth_credentials
                (provider, credential_type, data, disabled_cause, identity_key, created_at, updated_at)
            VALUES (?, ?, ?, ?, ?, ?, ?)
            """,
            ("other", "oauth", "leak", None, "other", 1, 2),
        )
    env = {
        "OMP_AUTH_SOURCE_AGENT_DIR": str(source_agent),
        "PI_CODING_AGENT_DIR": str(target_agent),
    }
    mod.seed_omp_auth_credentials(env)
    with sqlite3.connect(target_agent / "agent.db") as db:
        rows = db.execute(
            "SELECT provider, credential_type, data, identity_key FROM auth_credentials"
        ).fetchall()
print(json.dumps(rows))
"#,
    );
    let rows: serde_json::Value =
        serde_json::from_str(&output).expect("seeded auth rows should be JSON");

    assert_eq!(
        rows,
        serde_json::json!([["openai-codex", "oauth", "secret-json", "identity"]])
    );
}

#[test]
fn programbench_archive_excludes_omp_credentials() {
    let output = run_python_adapter(
        &programbench_codex_agent_path(),
        r#"import json
import tarfile
import tempfile
from pathlib import Path

with tempfile.TemporaryDirectory() as root:
    root = Path(root)
    airlock = root / "airlock"
    instance_dir = root / "instance"
    (airlock / ".omp" / "profiles" / "stateful" / "agent").mkdir(parents=True)
    (airlock / ".omp" / "profiles" / "stateful" / "agent" / "agent.db").write_text("secret", encoding="utf-8")
    (airlock / ".stateful_core").mkdir()
    (airlock / ".stateful_core" / "runtime.json").write_text("stateful", encoding="utf-8")
    (airlock / "config.yml").write_text("stateful", encoding="utf-8")
    (airlock / "state.db").write_bytes(b"sqlite")
    (airlock / "repos").mkdir()
    (airlock / "repos" / "repo.db").write_text("stateful", encoding="utf-8")
    (airlock / "runtime").mkdir()
    (airlock / "runtime" / "server.pid").write_text("123", encoding="utf-8")
    (airlock / ".git").mkdir()
    (airlock / ".git" / "config").write_text("git", encoding="utf-8")
    (airlock / "Library" / "Caches" / "com.apple.python").mkdir(parents=True)
    (airlock / "Library" / "Caches" / "com.apple.python" / "cmatrix.pyc").write_bytes(b"pyc")
    (airlock / "src" / "__pycache__").mkdir(parents=True)
    (airlock / "src" / "__pycache__" / "cmatrix.cpython-39.pyc").write_bytes(b"pyc")
    (airlock / ".pytest_cache").mkdir()
    (airlock / ".pytest_cache" / "README.md").write_text("cache", encoding="utf-8")
    (airlock / "compile.sh").write_text("cc main.c -o executable\n", encoding="utf-8")
    (airlock / "main.c").write_text("int main(void){return 0;}\n", encoding="utf-8")
    instance_dir.mkdir()
    archive = mod.archive_airlock_workspace(str(airlock), instance_dir)
    with tarfile.open(archive, "r:gz") as tar:
        names = sorted(tar.getnames())
print(json.dumps(names))
"#,
    );
    let names: Vec<String> = serde_json::from_str(&output).expect("archive names should be JSON");

    assert!(names.iter().any(|name| name.ends_with("compile.sh")));
    assert!(names.iter().any(|name| name.ends_with("main.c")));
    assert!(names.iter().all(|name| {
        !name.ends_with("/executable")
            && !name.contains(".git")
            && !name.contains(".omp")
            && !name.contains(".stateful")
            && !name.contains("Library/Caches")
            && !name.contains("__pycache__")
            && !name.ends_with(".pyc")
            && !name.contains(".pytest_cache")
    }));
    assert!(names.iter().any(|name| name.ends_with("config.yml")));
    assert!(names.iter().any(|name| name.ends_with("state.db")));
    assert!(names.iter().any(|name| name.ends_with("repos/repo.db")));
    assert!(
        names
            .iter()
            .any(|name| name.ends_with("runtime/server.pid"))
    );
}

#[test]
fn programbench_archive_failure_removes_partial_submission() {
    let output = run_python_adapter(
        &programbench_codex_agent_path(),
        r#"import json
import os
import tempfile
from pathlib import Path

with tempfile.TemporaryDirectory() as root:
    root = Path(root)
    airlock = root / "airlock"
    instance_dir = root / "instance"
    airlock.mkdir()
    instance_dir.mkdir()
    (airlock / "compile.sh").write_text("cc main.c -o executable\n", encoding="utf-8")
    blocked = airlock / "secret.txt"
    blocked.write_text("secret", encoding="utf-8")
    os.chmod(blocked, 0)
    try:
        try:
            mod.archive_airlock_workspace(str(airlock), instance_dir)
        except PermissionError:
            pass
        observed = (instance_dir / "submission.tar.gz").exists()
    finally:
        os.chmod(blocked, 0o600)
print(json.dumps({"partial_exists": observed}))
"#,
    );
    let observed: serde_json::Value =
        serde_json::from_str(&output).expect("archive observation should be JSON");

    assert_eq!(observed["partial_exists"], false);
}

#[test]
fn programbench_adapter_records_archive_error_without_losing_agent_logs() {
    let output = run_python_adapter(
        &programbench_codex_agent_path(),
        r#"import json
import subprocess
import tempfile
import types
from pathlib import Path

def fake_agent(args, prompt):
    args.archive_error = "permission denied reading executable"
    args.submission_path = str(Path(args.condition_dir) / args.instance_id / "submission.tar.gz")
    return subprocess.CompletedProcess(["codex"], 0, stdout="agent stdout\n", stderr="agent stderr\n")

with tempfile.TemporaryDirectory() as condition_dir:
    args = types.SimpleNamespace(
        condition_dir=condition_dir,
        instance_id="owner__repo.abc123",
        condition_id="stateful-off_subagent-off",
        container_id="programbench-container",
        docker_bin="docker",
        timeout_seconds=123,
        subagent=False,
        stateful=False,
        benchmark_max_turns=500,
    )
    exit_code = mod.run_main(
        args,
        agent_name="codex-cli",
        exited_error_prefix="codex",
        token_usage_from_output=mod.codex_token_usage_from_output,
        run_agent_func=fake_agent,
    )
    instance_dir = Path(condition_dir) / "owner__repo.abc123"
    metadata = json.loads((instance_dir / "instance.json").read_text())
    observed = {
        "exit_code": exit_code,
        "stdout": (instance_dir / "agent.stdout.log").read_text(),
        "stderr": (instance_dir / "agent.stderr.log").read_text(),
        "archive_error": metadata.get("archive_error"),
        "error": metadata.get("error"),
    }
print(json.dumps(observed))
"#,
    );
    let observed: serde_json::Value =
        serde_json::from_str(&output).expect("adapter observation should be JSON");

    assert_eq!(observed["exit_code"], 0);
    assert_eq!(observed["stdout"], "agent stdout\n");
    assert_eq!(observed["stderr"], "agent stderr\n");
    assert_eq!(
        observed["archive_error"],
        "permission denied reading executable"
    );
    assert_eq!(observed["error"], serde_json::Value::Null);
}

#[test]
fn programbench_adapter_marks_smoke_compile_failure() {
    let output = run_python_adapter(
        &programbench_codex_agent_path(),
        r#"import json
import subprocess
import tempfile
import types
from pathlib import Path

def fake_agent(args, prompt):
    args.smoke_compile_error = "compile.sh exited 1"
    args.submission_path = str(Path(args.condition_dir) / args.instance_id / "submission.tar.gz")
    return subprocess.CompletedProcess(["codex"], 0, stdout="agent stdout\n", stderr="")

with tempfile.TemporaryDirectory() as condition_dir:
    args = types.SimpleNamespace(
        condition_dir=condition_dir,
        instance_id="owner__repo.abc123",
        condition_id="stateful-off_subagent-on",
        container_id="programbench-container",
        docker_bin="docker",
        timeout_seconds=123,
        subagent=True,
        stateful=False,
        benchmark_max_turns=500,
        subagent_min_count=3,
    )
    exit_code = mod.run_main(
        args,
        agent_name="codex-cli",
        exited_error_prefix="codex",
        token_usage_from_output=mod.codex_token_usage_from_output,
        run_agent_func=fake_agent,
    )
    metadata = json.loads((Path(condition_dir) / "owner__repo.abc123" / "instance.json").read_text())
    observed = {
        "exit_code": exit_code,
        "metadata_exit_code": metadata.get("exit_code"),
        "error": metadata.get("error"),
        "smoke_compile_error": metadata.get("smoke_compile_error"),
    }
print(json.dumps(observed))
"#,
    );
    let observed: serde_json::Value =
        serde_json::from_str(&output).expect("adapter observation should be JSON");

    assert_eq!(observed["exit_code"], 1);
    assert_eq!(observed["metadata_exit_code"], 1);
    assert_eq!(
        observed["error"],
        "smoke compile failed: compile.sh exited 1"
    );
    assert_eq!(observed["smoke_compile_error"], "compile.sh exited 1");
}

#[test]
fn programbench_container_native_smoke_compile_runs_in_workspace() {
    let output = run_python_adapter(
        &programbench_codex_agent_path(),
        r##"import json
import subprocess
import tempfile
import types
from pathlib import Path

calls = []
copied_names = []

def fake_run(command, **kwargs):
    calls.append({
        "command": command,
        "cwd": str(kwargs.get("cwd")),
        "timeout": kwargs.get("timeout"),
    })
    if command[1] == "cp":
        copied_root = Path(command[2].removesuffix("/."))
        copied_names.extend(sorted(str(path.relative_to(copied_root)) for path in copied_root.rglob("*")))
    (airlock / "executable").write_text("replacement", encoding="utf-8")
    return subprocess.CompletedProcess(command, 0, stdout="", stderr="")

mod.subprocess.run = fake_run

with tempfile.TemporaryDirectory() as root:
    airlock = Path(root)
    (airlock / "executable").write_text("seed binary", encoding="utf-8")
    (airlock / "compile.sh").write_text("#!/bin/sh\nprintf replacement > executable\n", encoding="utf-8")
    (airlock / ".omp").mkdir()
    (airlock / ".omp" / "agent.db").write_text("secret", encoding="utf-8")
    (airlock / ".stateful").mkdir()
    (airlock / ".stateful" / "state.db").write_text("runtime", encoding="utf-8")
    (airlock / "main.c").write_text("int main(void){return 0;}\n", encoding="utf-8")
    args = types.SimpleNamespace(
        docker_bin="docker",
        container_id="programbench-container",
        timeout_seconds=5,
    )
    mod.smoke_compile_airlock(str(airlock), args)
    observed = {
        "calls": calls,
        "copied_names": copied_names,
        "executable": (airlock / "executable").read_text(encoding="utf-8"),
    }
print(json.dumps(observed))
"##,
    );
    let observed: serde_json::Value =
        serde_json::from_str(&output).expect("smoke compile observation should be JSON");

    assert_eq!(observed["calls"][0]["command"][0], "docker");
    assert_eq!(observed["calls"][0]["command"][1], "cp");
    assert!(
        observed["calls"][0]["command"][2]
            .as_str()
            .expect("copy source should be a string")
            .ends_with("/.")
    );
    assert_eq!(
        observed["calls"][0]["command"][3],
        "programbench-container:/workspace/"
    );
    assert_eq!(
        observed["calls"][1]["command"],
        serde_json::json!([
            "docker",
            "exec",
            "-w",
            "/workspace",
            "programbench-container",
            "sh",
            "./compile.sh"
        ])
    );
    assert_eq!(observed["calls"][1]["cwd"], "None");
    assert_eq!(observed["calls"][1]["timeout"], 5);
    assert_eq!(observed["executable"], "replacement");
    let copied_names = observed["copied_names"]
        .as_array()
        .expect("copied names should be array");
    assert!(
        copied_names
            .iter()
            .any(|name| name.as_str() == Some("compile.sh"))
    );
    assert!(
        copied_names
            .iter()
            .any(|name| name.as_str() == Some("main.c"))
    );
    assert!(
        copied_names
            .iter()
            .all(|name| !name.as_str().unwrap_or_default().contains(".omp")
                && !name.as_str().unwrap_or_default().contains(".stateful")
                && name.as_str() != Some("executable"))
    );
}

#[test]
fn programbench_report_markdown_labels_partial_score_and_resolved_count() {
    let report = condition_report(
        "stateful-off_subagent-off",
        false,
        false,
        0.6304347826086957,
        497_867,
        1,
        1,
    );
    let markdown = report
        .render(ReportFormat::Markdown)
        .expect("condition markdown should render");

    assert!(markdown.contains("Partial score"));
    assert!(markdown.contains("Resolved"));
    assert!(markdown.contains("| stateful-off_subagent-off | off | off | 2 | 2 | 0.630 | 0/2 |"));
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
        container_id="programbench-container",
        docker_bin="docker",
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
fn programbench_adapter_observes_native_task_tool_call() {
    let output = run_python_adapter(
        &programbench_codex_agent_path(),
        r#"import json
print(json.dumps({
    "codex": mod.observed_subagent_used('{"type":"tool_call","name":"task"}\n', ""),
    "codex_response_item": mod.observed_subagent_used('{"type":"response.item.completed","item":{"type":"function_call","name":"task"}}\n', ""),
    "omp": mod.observed_subagent_used('{"type":"tool_call","tool_name":"functions.task"}\n', ""),
    "omp_message": mod.observed_subagent_used('{"message":{"content":[{"type":"toolCall","name":"task","arguments":{"tasks":[{"assignment":"a"},{"assignment":"b"}]}}]}}\n', ""),
    "none": mod.observed_subagent_used('{"type":"turn.completed","name":"task"}\n', ""),
}))
"#,
    );
    let observed: serde_json::Value =
        serde_json::from_str(&output).expect("native task observation should be JSON");

    assert_eq!(observed["codex"], true);
    assert_eq!(observed["omp"], true);
    assert_eq!(observed["codex_response_item"], true);
    assert_eq!(observed["omp_message"], true);
    assert_eq!(observed["none"], serde_json::Value::Null);
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
fn programbench_omp_adapter_parses_current_usage_event_shape() {
    let output = run_python_adapter(
        &programbench_omp_agent_path(),
        r#"import json
usage = mod.omp_token_usage_from_output('{"type":"message_end","message":{"usage":{"input":50,"output":5,"cacheRead":20,"reasoning":2,"totalTokens":75}}}\n')
print(json.dumps(usage))
"#,
    );
    let usage: serde_json::Value = serde_json::from_str(&output).expect("omp usage should be JSON");

    assert_eq!(usage["input_tokens"], 50);
    assert_eq!(usage["cached_input_tokens"], 20);
    assert_eq!(usage["output_tokens"], 5);
    assert_eq!(usage["reasoning_output_tokens"], 2);
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
    let markdown = comparison
        .render(ReportFormat::Markdown)
        .expect("comparison markdown should render");
    assert!(markdown.contains("Partial score"));
    assert!(markdown.contains("Resolved"));
    assert!(markdown.contains("| stateful-on_subagent-off | on | off | 2 | 0.750 | 0/2 |"));
}

#[test]
fn programbench_compare_markdown_renders_subagent_and_interaction_deltas() {
    let off_off = condition_report(
        "stateful-off_subagent-off",
        false,
        false,
        0.5,
        6000,
        220,
        140,
    );
    let on_off = condition_report(
        "stateful-on_subagent-off",
        true,
        false,
        0.75,
        4000,
        167,
        107,
    );
    let off_on = condition_report("stateful-off_subagent-on", false, true, 0.6, 5000, 200, 120);
    let on_on = condition_report("stateful-on_subagent-on", true, true, 0.9, 3500, 150, 90);

    let markdown = compare_programbench_reports(vec![off_off, on_off, off_on, on_on])
        .render(ReportFormat::Markdown)
        .expect("comparison markdown should render");

    assert!(markdown.contains("- Subagent score delta without stateful: 0.1"));
    assert!(markdown.contains("- Subagent running time ms delta without stateful: -1000"));
    assert!(markdown.contains("- Subagent input+output token delta without stateful: -20"));
    assert!(markdown.contains("- Combined interaction score delta: 0.05"));
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
        archive_error: None,
        workspace_copy_error: None,
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

#[expect(
    clippy::too_many_arguments,
    reason = "test fixture builder mirrors report fields"
)]
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

fn has_arg_pair(args: &[String], key: &str, value: &str) -> bool {
    args.windows(2).any(|pair| pair == [key, value])
}

fn docker_call_starts_with(call: &serde_json::Value, prefix: &[&str]) -> bool {
    let Some(args) = call.as_array().map(|args| {
        args.iter()
            .filter_map(serde_json::Value::as_str)
            .collect::<Vec<_>>()
    }) else {
        return false;
    };
    args.starts_with(prefix)
}

fn docker_call_contains_arg(call: &serde_json::Value, expected: &str) -> bool {
    call.as_array().is_some_and(|args| {
        args.iter()
            .filter_map(serde_json::Value::as_str)
            .any(|arg| arg == expected)
    })
}

fn docker_call_has_sequence(call: &serde_json::Value, expected: &[&str]) -> bool {
    let Some(args) = call.as_array().map(|args| {
        args.iter()
            .filter_map(serde_json::Value::as_str)
            .collect::<Vec<_>>()
    }) else {
        return false;
    };
    args.windows(expected.len())
        .any(|window| window == expected)
}

fn docker_call_execs(call: &serde_json::Value, container_id: &str, inner_prefix: &[&str]) -> bool {
    let Some(args) = call.as_array().map(|args| {
        args.iter()
            .filter_map(serde_json::Value::as_str)
            .collect::<Vec<_>>()
    }) else {
        return false;
    };
    if args.len() < 3 || args[0] != "docker" || args[1] != "exec" {
        return false;
    }
    let Some(container_index) = args.iter().position(|arg| *arg == container_id) else {
        return false;
    };
    args[container_index + 1..].starts_with(inner_prefix)
}

fn docker_call_contains_env(call: &serde_json::Value, assignment: &str) -> bool {
    let Some(args) = call.as_array().map(|args| {
        args.iter()
            .filter_map(serde_json::Value::as_str)
            .collect::<Vec<_>>()
    }) else {
        return false;
    };
    args.windows(2).any(|pair| pair == ["-e", assignment])
}
