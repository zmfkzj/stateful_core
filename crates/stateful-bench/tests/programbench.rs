use clap::Parser;
use stateful_bench::{
    Cli, Command, ProgramBenchAgentKind, ProgramBenchCommand, ProgramBenchCondition,
    ProgramBenchConditionMetadata, ProgramBenchConditionReport, ProgramBenchEvalOptions,
    ProgramBenchInstanceMetadata, ProgramBenchInstanceRunOptions, ProgramBenchRunOptions,
    ProgramBenchTokenUsage, ReportFormat, build_programbench_agent_command,
    build_programbench_condition_report, build_programbench_eval_commands,
    compare_programbench_reports, default_programbench_conditions, parse_programbench_condition,
    planned_programbench_conditions, run_programbench_matrix,
};
use std::{
    collections::BTreeMap,
    fs,
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
fn programbench_eval_commands_run_eval_info_and_package_by_default() {
    let commands = build_programbench_eval_commands(ProgramBenchEvalOptions {
        run_dir: "runs/pb-dev".into(),
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
    assert_eq!(
        rendered[0],
        "programbench eval runs/pb-dev --workers 4 --branch-workers 2 --docker-cpus 8 --force"
    );
    assert_eq!(rendered[1], "programbench info runs/pb-dev");
    assert_eq!(rendered[2], "programbench submit package runs/pb-dev");
}

#[test]
fn programbench_run_matrix_writes_condition_metadata_without_launching_tools() {
    let output_dir = temp_root("programbench-run-matrix");
    let metadata = run_programbench_matrix(ProgramBenchRunOptions {
        output_dir: output_dir.clone(),
        run_id: "pb-dev".to_string(),
        agent: ProgramBenchAgentKind::CodexCli,
        conditions: vec![ProgramBenchCondition::new(true, false)],
        model: None,
        benchmark_max_turns: 500,
        timeout_seconds: 7200,
        filter: None,
        slice: None,
        max_instances: None,
        programbench_bin: "programbench".to_string(),
        docker_bin: "docker".to_string(),
        image_tag: "task_cleanroom_v6".to_string(),
        stateful_binary: "stateful".to_string(),
        codex_bin: "codex".to_string(),
        omp_bin: "omp".to_string(),
    })
    .expect("matrix metadata should be written");

    assert_eq!(metadata.len(), 1);
    assert_eq!(metadata[0].condition_id, "stateful-on_subagent-off");
    assert!(metadata[0].instances.is_empty());

    let condition_path = output_dir
        .join("pb-dev")
        .join("conditions")
        .join("stateful-on_subagent-off")
        .join("condition.json");
    let written: ProgramBenchConditionMetadata =
        serde_json::from_str(&fs::read_to_string(condition_path).expect("metadata should exist"))
            .expect("metadata should parse");
    assert_eq!(written, metadata[0]);
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
    ProgramBenchConditionReport {
        run_id: "pb-dev".to_string(),
        condition_id: condition_id.to_string(),
        condition: ProgramBenchCondition::new(stateful, subagent),
        instances: 2,
        attempted_instances: 2,
        evaluated_instances: 2,
        average_score: Some(score),
        resolved_count: 0,
        resolved_rate: Some(0.0),
        eval_error_count: 0,
        agent_error_count: 0,
        timeout_count: 0,
        running_time_ms,
        average_running_time_ms: Some(running_time_ms as f64 / 2.0),
        token_observed_instances: 2,
        token_usage_turns: 4,
        token_input_tokens: 0,
        token_cached_input_tokens: 0,
        token_output_tokens: 0,
        token_reasoning_output_tokens: 0,
        token_input_plus_output_tokens: input_plus_output,
        token_uncached_input_tokens: 0,
        token_uncached_input_plus_output_tokens: uncached_input_plus_output,
        average_input_plus_output_tokens: Some(input_plus_output as f64 / 2.0),
        average_uncached_input_plus_output_tokens: Some(uncached_input_plus_output as f64 / 2.0),
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
