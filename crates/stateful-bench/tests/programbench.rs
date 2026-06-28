use clap::Parser;
use stateful_bench::{
    Cli, Command, ProgramBenchAgentKind, ProgramBenchCommand, ProgramBenchCondition,
    ProgramBenchInstanceMetadata, ProgramBenchInstanceRunOptions, ProgramBenchTokenUsage,
    ReportFormat, build_programbench_agent_command, default_programbench_conditions,
    parse_programbench_condition,
};
use std::path::{Path, PathBuf};

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
    let condition = parse_programbench_condition("stateful:on,subagent:off")
        .expect("condition should parse");

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

fn has_arg(args: &[String], value: &str) -> bool {
    args.iter().any(|arg| arg == value)
}
