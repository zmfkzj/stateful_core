use clap::Parser;
use stateful_bench::{
    parse_denovo_condition, run_denovo_matrix, Cli, Command, DeNovoAgentKind, DeNovoCommand,
    DeNovoMatrixRunOptions, DeNovoRunMode, ReportFormat, RunMode,
};
use std::{fs, path::PathBuf, process::Command as ProcessCommand, time::Duration};

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
        "/opt/stateful/bin/stateful",
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
                subagent_min_count: 3,
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
            && stateful_binary == "/opt/stateful/bin/stateful"
            && benchmark_model.as_deref() == Some("gpt-5.4-mini")
            && benchmark_reasoning_effort == "low"
            && benchmark_temperature == "1"
            && codex_adapter_script.as_deref()
                == Some(std::path::Path::new("crates/stateful-bench/scripts/denovo_codex_agent.py"))
    ));
}

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
        "/opt/stateful/bin/stateful",
        "--agent-docker-image",
        "ghcr.io/stateful/omp-agent:latest",
        "--agent-docker-stateful-binary",
        "/usr/local/bin/stateful",
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
                ref agent_docker_image,
                ref agent_docker_stateful_binary,
                ref benchmark_model,
                ..
            }
        } if run_id == "dev-denovo-omp"
            && condition == &vec![
                "stateful:off,subagent:on".to_string(),
                "stateful:on,subagent:on".to_string(),
            ]
            && omp_bin == "/opt/homebrew/bin/omp"
            && agent_docker_image.as_deref() == Some("ghcr.io/stateful/omp-agent:latest")
            && agent_docker_stateful_binary == "/usr/local/bin/stateful"
            && benchmark_model.as_deref() == Some("deepseek-v4-flash")
    ));
}

#[test]
fn denovo_run_command_rejects_zero_subagent_min_count() {
    let error = Cli::try_parse_from([
        "stateful-bench",
        "denovo",
        "run",
        "--agent",
        "codex-cli",
        "--data-file",
        ".stateful_bench/denovo/extracts/dev/results.jsonl",
        "--subagent-min-count",
        "0",
    ])
    .expect_err("zero subagent minimum should be rejected");

    assert!(error.to_string().contains("expected a positive integer"));
}

#[test]
fn denovo_matrix_runs_all_conditions_for_one_instance_before_next_instance() {
    let temp_dir = target_temp_dir("stateful-bench-denovo-matrix-instance-major");
    let aweagent_root = temp_dir.join("AweAgent");
    fs::create_dir_all(&aweagent_root).expect("fake AweAgent root should be created");
    let data_file = temp_dir.join("denovo.jsonl");
    fs::write(
        &data_file,
        [
            r#"{"instance_id":"case-a"}"#,
            r#"{"instance_id":"case-b"}"#,
            "",
        ]
        .join("\n"),
    )
    .expect("data file should be written");
    let log_path = temp_dir.join("order.log");
    let adapter_script = temp_dir.join("fake_denovo_adapter.py");
    fs::write(
        &adapter_script,
        format!(
            r#"
import argparse
import json
import os
from pathlib import Path

parser = argparse.ArgumentParser()
parser.add_argument("--output", required=True)
parser.add_argument("--agent-mode", required=True)
parser.add_argument("--subagent", required=True)
parser.add_argument("--instance-id", action="append", default=[])
parser.add_argument("--del-done-images", dest="del_done_images", action="store_true", default=True)
parser.add_argument("--keep-done-images", dest="del_done_images", action="store_false")
args, _ = parser.parse_known_args()

log_path = Path(os.environ["DENOVO_ORDER_LOG"])
with log_path.open("a", encoding="utf-8") as log:
    for instance_id in args.instance_id:
        image_action = "delete" if args.del_done_images else "keep"
        log.write(f"{{args.agent_mode}}/{{args.subagent}}/{{instance_id}}/{{image_action}}\n")

output = Path(args.output)
results = output / "_" / "results.jsonl"
results.parent.mkdir(parents=True, exist_ok=True)
with results.open("w", encoding="utf-8") as handle:
    for instance_id in args.instance_id:
        handle.write(json.dumps({{
            "instance_id": instance_id,
            "success": True,
            "score": 1.0,
            "eval_result": {{"details": {{"pass_rate": 1.0}}}},
        }}) + "\n")
"#
        ),
    )
    .expect("fake adapter should be written");

    let order_env = log_path.to_string_lossy();
    let reports = run_denovo_matrix(DeNovoMatrixRunOptions {
        run_id: "dev-denovo-instance-major".to_string(),
        aweagent_root,
        python: "python3".to_string(),
        data_file,
        run_dir: temp_dir.join("runs"),
        base_config: PathBuf::from("configs/tasks/denovoswe.yaml"),
        conditions: vec![
            parse_denovo_condition(&format!(
                "stateful:off,subagent:on,env:DENOVO_ORDER_LOG={order_env}"
            ))
            .expect("off condition should parse"),
            parse_denovo_condition(&format!(
                "stateful:on,subagent:on,env:DENOVO_ORDER_LOG={order_env}"
            ))
            .expect("on condition should parse"),
        ],
        agent: DeNovoAgentKind::CodexCli,
        codex_bin: "codex".to_string(),
        omp_bin: "omp".to_string(),
        stateful_binary: "stateful".to_string(),
        agent_docker_image: None,
        agent_docker_stateful_binary: "/usr/local/bin/stateful".to_string(),
        benchmark_model: "gpt-5.4-mini".to_string(),
        benchmark_reasoning_effort: "low".to_string(),
        benchmark_model_context_window: 256000,
        benchmark_temperature: "1".to_string(),
        benchmark_max_turns: 500,
        subagent_min_count: 3,
        max_resumes: 1,
        codex_timeout_seconds: 7200,
        codex_adapter_script: Some(adapter_script),
        mode: DeNovoRunMode::Batch,
        instance_ids: vec!["case-a".to_string(), "case-b".to_string()],
        llm_config: None,
        model: None,
        max_steps: None,
        max_concurrent: None,
        search_override: None,
        skip_eval: false,
        validate_run: false,
        eval_iters: 1,
        del_done_images: true,
        dump_clean_snapshot: None,
        prompt_version: "v1".to_string(),
        verbose: false,
    })
    .expect("matrix run should complete");

    let order = fs::read_to_string(&log_path).expect("order log should be readable");
    assert_eq!(
        order.lines().collect::<Vec<_>>(),
        vec![
            "no-state/on/case-a/keep",
            "stateful/on/case-a/delete",
            "no-state/on/case-b/keep",
            "stateful/on/case-b/delete",
        ]
    );
    assert_eq!(reports.len(), 2);
    assert!(reports.iter().all(|report| report.total_instances == 2));

    fs::remove_dir_all(temp_dir).expect("temp dir should clean up");
}

#[test]
fn denovo_matrix_batches_cli_instances_when_max_concurrent_is_set() {
    let temp_dir = target_temp_dir("stateful-bench-denovo-matrix-cli-batch");
    let aweagent_root = temp_dir.join("AweAgent");
    fs::create_dir_all(&aweagent_root).expect("fake AweAgent root should be created");
    let data_file = temp_dir.join("denovo.jsonl");
    fs::write(
        &data_file,
        [
            r#"{"instance_id":"case-a"}"#,
            r#"{"instance_id":"case-b"}"#,
            "",
        ]
        .join("\n"),
    )
    .expect("data file should be written");
    let log_path = temp_dir.join("order.log");
    let adapter_script = temp_dir.join("fake_denovo_adapter.py");
    fs::write(
        &adapter_script,
        format!(
            r#"
import argparse
import json
import os
from pathlib import Path

parser = argparse.ArgumentParser()
parser.add_argument("--output", required=True)
parser.add_argument("--agent-mode", required=True)
parser.add_argument("--subagent", required=True)
parser.add_argument("--instance-id", action="append", default=[])
parser.add_argument("--del-done-images", dest="del_done_images", action="store_true", default=True)
parser.add_argument("--keep-done-images", dest="del_done_images", action="store_false")
args, _ = parser.parse_known_args()

log_path = Path(os.environ["DENOVO_ORDER_LOG"])
image_action = "delete" if args.del_done_images else "keep"
with log_path.open("a", encoding="utf-8") as log:
    log.write(f"{{args.agent_mode}}/{{args.subagent}}/{{','.join(args.instance_id)}}/{{image_action}}\n")

output = Path(args.output)
results = output / "_" / "results.jsonl"
results.parent.mkdir(parents=True, exist_ok=True)
with results.open("w", encoding="utf-8") as handle:
    for instance_id in args.instance_id:
        handle.write(json.dumps({{
            "instance_id": instance_id,
            "success": True,
            "score": 1.0,
            "eval_result": {{"details": {{"pass_rate": 1.0}}}},
        }}) + "\n")
"#
        ),
    )
    .expect("fake adapter should be written");

    let order_env = log_path.to_string_lossy();
    let reports = run_denovo_matrix(DeNovoMatrixRunOptions {
        run_id: "dev-denovo-cli-batch".to_string(),
        aweagent_root,
        python: "python3".to_string(),
        data_file,
        run_dir: temp_dir.join("runs"),
        base_config: PathBuf::from("configs/tasks/denovoswe.yaml"),
        conditions: vec![
            parse_denovo_condition(&format!(
                "stateful:off,subagent:on,env:DENOVO_ORDER_LOG={order_env}"
            ))
            .expect("off condition should parse"),
            parse_denovo_condition(&format!(
                "stateful:on,subagent:on,env:DENOVO_ORDER_LOG={order_env}"
            ))
            .expect("on condition should parse"),
        ],
        agent: DeNovoAgentKind::CodexCli,
        codex_bin: "codex".to_string(),
        omp_bin: "omp".to_string(),
        stateful_binary: "stateful".to_string(),
        agent_docker_image: None,
        agent_docker_stateful_binary: "/usr/local/bin/stateful".to_string(),
        benchmark_model: "gpt-5.4-mini".to_string(),
        benchmark_reasoning_effort: "low".to_string(),
        benchmark_model_context_window: 256000,
        benchmark_temperature: "1".to_string(),
        benchmark_max_turns: 500,
        subagent_min_count: 3,
        max_resumes: 1,
        codex_timeout_seconds: 7200,
        codex_adapter_script: Some(adapter_script),
        mode: DeNovoRunMode::Batch,
        instance_ids: vec!["case-a".to_string(), "case-b".to_string()],
        llm_config: None,
        model: None,
        max_steps: None,
        max_concurrent: Some(2),
        search_override: None,
        skip_eval: false,
        validate_run: false,
        eval_iters: 1,
        del_done_images: true,
        dump_clean_snapshot: None,
        prompt_version: "v1".to_string(),
        verbose: false,
    })
    .expect("matrix run should complete");

    let order = fs::read_to_string(&log_path).expect("order log should be readable");
    assert_eq!(
        order.lines().collect::<Vec<_>>(),
        vec![
            "no-state/on/case-a,case-b/keep",
            "stateful/on/case-a,case-b/delete",
        ]
    );
    assert_eq!(reports.len(), 2);
    assert!(reports.iter().all(|report| report.total_instances == 2));

    fs::remove_dir_all(temp_dir).expect("temp dir should clean up");
}

#[test]
fn denovo_matrix_preserves_aggregate_results_when_later_instance_fails() {
    let temp_dir = target_temp_dir("stateful-bench-denovo-matrix-error-preserves-results");
    let aweagent_root = temp_dir.join("AweAgent");
    fs::create_dir_all(&aweagent_root).expect("fake AweAgent root should be created");
    let data_file = temp_dir.join("denovo.jsonl");
    fs::write(
        &data_file,
        [
            r#"{"instance_id":"case-a"}"#,
            r#"{"instance_id":"case-b"}"#,
            "",
        ]
        .join("\n"),
    )
    .expect("data file should be written");
    let adapter_script = temp_dir.join("fake_denovo_adapter.py");
    fs::write(
        &adapter_script,
        r#"
import argparse
import json
from pathlib import Path

parser = argparse.ArgumentParser()
parser.add_argument("--output", required=True)
parser.add_argument("--instance-id", action="append", default=[])
args, _ = parser.parse_known_args()

output = Path(args.output)
results = output / "_" / "results.jsonl"
results.parent.mkdir(parents=True, exist_ok=True)
instance_id = args.instance_id[0]
if instance_id == "case-b":
    results.write_text("", encoding="utf-8")
    raise SystemExit("synthetic adapter failure")

with results.open("w", encoding="utf-8") as handle:
    handle.write(json.dumps({
        "instance_id": instance_id,
        "success": True,
        "score": 1.0,
        "eval_result": {"details": {"pass_rate": 1.0}},
    }) + "\n")
"#,
    )
    .expect("fake adapter should be written");

    let error = run_denovo_matrix(DeNovoMatrixRunOptions {
        run_id: "dev-denovo-error-preserves-results".to_string(),
        aweagent_root,
        python: "python3".to_string(),
        data_file,
        run_dir: temp_dir.join("runs"),
        base_config: PathBuf::from("configs/tasks/denovoswe.yaml"),
        conditions: vec![
            parse_denovo_condition("stateful:off,subagent:on").expect("condition should parse")
        ],
        agent: DeNovoAgentKind::CodexCli,
        codex_bin: "codex".to_string(),
        omp_bin: "omp".to_string(),
        stateful_binary: "stateful".to_string(),
        agent_docker_image: None,
        agent_docker_stateful_binary: "/usr/local/bin/stateful".to_string(),
        benchmark_model: "gpt-5.4-mini".to_string(),
        benchmark_reasoning_effort: "low".to_string(),
        benchmark_model_context_window: 256000,
        benchmark_temperature: "1".to_string(),
        benchmark_max_turns: 500,
        subagent_min_count: 3,
        max_resumes: 1,
        codex_timeout_seconds: 7200,
        codex_adapter_script: Some(adapter_script),
        mode: DeNovoRunMode::Batch,
        instance_ids: vec!["case-a".to_string(), "case-b".to_string()],
        llm_config: None,
        model: None,
        max_steps: None,
        max_concurrent: None,
        search_override: None,
        skip_eval: false,
        validate_run: false,
        eval_iters: 1,
        del_done_images: true,
        dump_clean_snapshot: None,
        prompt_version: "v1".to_string(),
        verbose: false,
    })
    .expect_err("second instance should fail");
    assert!(
        error.to_string().contains("DeNovoSWE command failed"),
        "{error:#}"
    );

    let aggregate_path = temp_dir
        .join("runs")
        .join("conditions")
        .join("stateful-off_subagent-on")
        .join("codex-cli")
        .join("_")
        .join("results.jsonl");
    let rows =
        fs::read_to_string(&aggregate_path).expect("aggregate results should remain readable");
    let rows = rows
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| serde_json::from_str::<serde_json::Value>(line).expect("row should be json"))
        .collect::<Vec<_>>();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["instance_id"], "case-a");

    let comparison_path = temp_dir.join("runs").join("comparison.json");
    let comparison = fs::read_to_string(&comparison_path)
        .expect("matrix comparison checkpoint should remain readable");
    let comparison = serde_json::from_str::<serde_json::Value>(&comparison)
        .expect("comparison checkpoint should be json");
    assert_eq!(comparison["conditions"][0]["total_instances"], 1);

    fs::remove_dir_all(temp_dir).expect("temp dir should clean up");
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
fn denovo_report_skips_stale_condition_report_when_results_were_reset() {
    let temp_dir = target_temp_dir("stateful-bench-denovo-report-stale-condition");
    let run_dir = temp_dir.join("runs").join("dev-denovo");
    let current_dir = run_dir.join("conditions").join("stateful-off_subagent-on");
    let stale_dir = run_dir.join("conditions").join("stateful-on_subagent-on");
    let current_official = current_dir.join("codex-cli");
    let stale_official = stale_dir.join("codex-cli");
    fs::create_dir_all(current_official.join("_")).expect("current results dir should exist");
    fs::create_dir_all(stale_official.join("_")).expect("stale results dir should exist");

    let current_results = current_official.join("_").join("results.jsonl");
    let stale_results = stale_official.join("_").join("results.jsonl");
    fs::write(&current_results, "{}\n").expect("current results should be written");
    fs::write(&stale_results, "{}\n").expect("stale results should be written");
    std::thread::sleep(Duration::from_millis(20));

    let current_report = current_dir.join("denovo-report.json");
    let stale_report = stale_dir.join("denovo-report.json");
    fs::write(
        &current_report,
        serde_json::json!({
            "run_id": "dev-denovo",
            "condition_id": "stateful-off_subagent-on",
            "condition": {"stateful": false, "subagent": true},
            "total_instances": 1,
            "completed_instances": 1,
            "scored_instances": 1,
            "pass_rate_instances": 1,
            "success_count": 1,
            "error_count": 0,
            "success_rate": 1.0,
            "average_score": 1.0,
            "average_pass_rate": 1.0,
            "correct_rate": 1.0,
            "almost_correct_rate": 1.0,
            "running_time_ms": 1,
            "average_running_time_ms": 1.0,
            "subagent_observed_instances": 1,
            "subagent_used_count": 1,
            "subagent_used_rate": 1.0
        })
        .to_string(),
    )
    .expect("current report should be written");
    fs::write(
        &stale_report,
        serde_json::json!({
            "run_id": "dev-denovo",
            "condition_id": "stateful-on_subagent-on",
            "condition": {"stateful": true, "subagent": true},
            "total_instances": 2,
            "completed_instances": 1,
            "scored_instances": 1,
            "pass_rate_instances": 1,
            "success_count": 0,
            "error_count": 1,
            "success_rate": 0.0,
            "average_score": 0.75,
            "average_pass_rate": 0.75,
            "correct_rate": 0.0,
            "almost_correct_rate": 0.5,
            "running_time_ms": 1,
            "average_running_time_ms": 1.0,
            "subagent_observed_instances": 1,
            "subagent_used_count": 1,
            "subagent_used_rate": 1.0
        })
        .to_string(),
    )
    .expect("stale report should be written");

    write_condition_metadata(
        &current_dir,
        &current_official,
        &current_results,
        &current_report,
        "stateful-off_subagent-on",
        false,
    );
    write_condition_metadata(
        &stale_dir,
        &stale_official,
        &stale_results,
        &stale_report,
        "stateful-on_subagent-on",
        true,
    );

    std::thread::sleep(Duration::from_millis(20));
    fs::write(&stale_results, "").expect("stale results should be reset after report");

    let output = run_dir.join("report.json");
    stateful_bench::denovo::run_denovo_cli(DeNovoCommand::Report {
        run_dir: run_dir.clone(),
        format: ReportFormat::Json,
        output: Some(output.clone()),
    })
    .expect("report command should succeed with current reports");

    let rendered = fs::read_to_string(&output).expect("report output should be readable");
    let reports =
        serde_json::from_str::<serde_json::Value>(&rendered).expect("report output should be json");
    let reports = reports.as_array().expect("report output should be array");
    assert_eq!(reports.len(), 1);
    assert_eq!(reports[0]["condition_id"], "stateful-off_subagent-on");

    fs::remove_dir_all(temp_dir).expect("temp dir should clean up");
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
    assert!(stdout.contains("--subagent-min-count"));
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
    stateful_binary="/opt/stateful/bin/stateful",
)
assert "Build a parser package." in prompt
assert "Benchmark max turns: 500" in prompt
assert "Maximum task steps: 500" in prompt
assert "Do not edit benchmark artifacts" in prompt
assert "Stateful command policy" not in prompt
assert "state_current_read" not in prompt
assert "search_tool_bm25" not in prompt
assert "/opt/stateful/bin/stateful" not in prompt
print(json.dumps({{"prompt": prompt}}))
"#,
        agent_path = denovo_codex_agent_path_json(),
    );
    let output = run_python_json(&script);
    assert!(output["prompt"]
        .as_str()
        .expect("prompt should be a string")
        .contains("fake-a"));
}

#[test]
fn denovo_codex_agent_git_diff_includes_new_and_modified_files() {
    let dir = target_temp_dir("denovo-codex-git-diff");
    let workspace = dir.join("workspace");
    let script = format!(
        r#"
import importlib.util
import io
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
    assert!(output["patch"]
        .as_str()
        .expect("patch should be a string")
        .contains("new_file.txt"));
    fs::remove_dir_all(&dir).expect("temp git diff workspace should clean up");
}

#[test]
fn denovo_codex_agent_git_diff_excludes_stateful_runtime_artifacts() {
    let dir = target_temp_dir("denovo-codex-git-diff-excludes-stateful-runtime");
    let workspace = dir.join("workspace");
    let script = format!(
        r#"
import importlib.util
import json
import shutil
import subprocess
import sys
from pathlib import Path

spec = importlib.util.spec_from_file_location("denovo_codex_agent_git_diff_exclude_test", {agent_path})
mod = importlib.util.module_from_spec(spec)
sys.modules[spec.name] = mod
spec.loader.exec_module(mod)

workspace = Path({workspace})
workspace.mkdir(parents=True, exist_ok=True)
subprocess.run(["git", "init"], cwd=workspace, check=True, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
subprocess.run(["git", "config", "user.email", "bench@example.test"], cwd=workspace, check=True)
subprocess.run(["git", "config", "user.name", "Bench Test"], cwd=workspace, check=True)
(workspace / "tracked.txt").write_text("old line\n", encoding="utf-8")
(workspace / "clean.sh").write_text("benchmark cleaner\n", encoding="utf-8")
subprocess.run(["git", "add", "tracked.txt", "clean.sh"], cwd=workspace, check=True)
subprocess.run(["git", "commit", "-m", "initial"], cwd=workspace, check=True, stdout=subprocess.PIPE, stderr=subprocess.PIPE)

(workspace / "tracked.txt").write_text("new line\n", encoding="utf-8")
(workspace / "new_file.txt").write_text("legitimate change\n", encoding="utf-8")
(workspace / ".stateful_core" / "runtime" / "sessions").mkdir(parents=True, exist_ok=True)
(workspace / ".stateful_core" / "runtime" / "session.json").write_text("{{}}\n", encoding="utf-8")
(workspace / ".stateful_core" / "runtime" / "sessions" / "session.json").write_text("{{}}\n", encoding="utf-8")
(workspace / ".stateful").mkdir(parents=True, exist_ok=True)
(workspace / ".stateful" / "config.yml").write_text("policy\n", encoding="utf-8")
(workspace / ".codex").mkdir(parents=True, exist_ok=True)
(workspace / ".codex" / "trace.json").write_text("{{}}\n", encoding="utf-8")
(workspace / "tmp" / "verify-supervisor" / ".stateful-tmp").mkdir(parents=True, exist_ok=True)
(workspace / "tmp" / "verify-supervisor" / ".stateful-tmp" / "xcrun_db").write_text("cache\n", encoding="utf-8")
(workspace / ".stateful-tmp").mkdir(parents=True, exist_ok=True)
(workspace / ".stateful-tmp" / "xcrun_db").write_text("cache\n", encoding="utf-8")
(workspace / ".pytest_cache" / "v" / "cache").mkdir(parents=True, exist_ok=True)
(workspace / ".pytest_cache" / "v" / "cache" / "nodeids").write_text("[]\n", encoding="utf-8")
(workspace / ".ruff_cache" / "content").mkdir(parents=True, exist_ok=True)
(workspace / ".ruff_cache" / "content" / "cache").write_text("cache\n", encoding="utf-8")
(workspace / ".mypy_cache" / "3.11").mkdir(parents=True, exist_ok=True)
(workspace / ".mypy_cache" / "3.11" / "module.json").write_text("{{}}\n", encoding="utf-8")
(workspace / "package" / "__pycache__").mkdir(parents=True, exist_ok=True)
(workspace / "package" / "__pycache__" / "module.cpython-311.pyc").write_text("bytecode\n", encoding="utf-8")
(workspace / ".coverage").write_text("coverage\n", encoding="utf-8")
(workspace / "target" / "debug").mkdir(parents=True, exist_ok=True)
(workspace / "target" / "debug" / "artifact").write_text("build artifact\n", encoding="utf-8")
(workspace / "upstream").mkdir(parents=True, exist_ok=True)
subprocess.run(["git", "init"], cwd=workspace / "upstream", check=True, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
(workspace / "upstream" / "README.md").write_text("source clone scratch\n", encoding="utf-8")
subprocess.run(["git", "add", "README.md"], cwd=workspace / "upstream", check=True)
subprocess.run(["git", "config", "user.email", "bench@example.test"], cwd=workspace / "upstream", check=True)
subprocess.run(["git", "config", "user.name", "Bench Test"], cwd=workspace / "upstream", check=True)
subprocess.run(["git", "commit", "-m", "upstream scratch"], cwd=workspace / "upstream", check=True, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
(workspace / "clean.sh").unlink()

patch = mod.git_diff(workspace)
assert "diff --git a/new_file.txt b/new_file.txt" in patch
assert "diff --git a/tracked.txt b/tracked.txt" in patch
assert ".stateful_core" not in patch
assert ".stateful/" not in patch
assert ".codex/" not in patch
assert "tmp/verify-supervisor" not in patch
assert ".stateful-tmp" not in patch
assert ".pytest_cache" not in patch
assert ".ruff_cache" not in patch
assert ".mypy_cache" not in patch
assert "__pycache__" not in patch
assert ".coverage" not in patch
assert "target/debug/artifact" not in patch
assert "diff --git a/clean.sh b/clean.sh" not in patch
assert "diff --git a/upstream b/upstream" not in patch
assert "Subproject commit" not in patch
print(json.dumps({{"patch": patch}}))
"#,
        agent_path = denovo_codex_agent_path_json(),
        workspace = serde_json::to_string(&workspace).expect("workspace path should serialize"),
    );
    let output = run_python_json(&script);
    let patch = output["patch"].as_str().expect("patch should be a string");
    assert!(patch.contains("new_file.txt"));
    fs::remove_dir_all(&dir).expect("temp git diff workspace should clean up");
}

#[test]
fn denovo_codex_agent_timeout_wrapper_bounds_run() {
    let script = format!(
        r#"
import importlib.util
import io
import json
import subprocess
import sys
from pathlib import Path

spec = importlib.util.spec_from_file_location("denovo_codex_agent_timeout_test", {agent_path})
mod = importlib.util.module_from_spec(spec)
sys.modules[spec.name] = mod
spec.loader.exec_module(mod)

def fast_runner(command, **kwargs):
    return subprocess.CompletedProcess(
        command,
        0,
        '{{"type":"turn.completed","usage":{{"input_tokens":100,"cached_input_tokens":40,"output_tokens":12,"reasoning_output_tokens":5}}}}\n'
        '{{"type":"turn.completed","usage":{{"input_tokens":10,"cached_input_tokens":4,"output_tokens":3,"reasoning_output_tokens":2}}}}\n',
        "",
    )

def timeout_runner(command, **kwargs):
    raise subprocess.TimeoutExpired(command, kwargs.get("timeout"))

captured_stdout = io.StringIO()
original_stdout = sys.stdout
sys.stdout = captured_stdout
try:
    fast = mod.run_codex_with_timeout(
        ["codex", "exec", "-"],
        "prompt",
        Path("/tmp"),
        None,
        max_resumes=0,
        timeout_seconds=1,
        runner=fast_runner,
    )
finally:
    sys.stdout = original_stdout
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

assert fast.returncode == 0
assert fast.token_usage == {{
    "turns": 2,
    "input_tokens": 110,
    "cached_input_tokens": 44,
    "output_tokens": 15,
    "reasoning_output_tokens": 7,
    "input_plus_output_tokens": 125,
    "uncached_input_tokens": 66,
    "uncached_input_plus_output_tokens": 81,
}}
assert captured_stdout.getvalue().count('"type":"turn.completed"') == 2
assert "codex timed out after 0.25s" in timeout_message
print(json.dumps({{"fast": fast.returncode, "token_usage": fast.token_usage, "emitted_stdout": captured_stdout.getvalue(), "timeout": timeout_message}}))
"#,
        agent_path = denovo_codex_agent_path_json(),
    );
    let output = run_python_json(&script);
    assert_eq!(output["fast"], 0);
    assert_eq!(output["token_usage"]["input_tokens"], 110);
    assert_eq!(output["token_usage"]["input_plus_output_tokens"], 125);
    assert_eq!(
        output["token_usage"]["uncached_input_plus_output_tokens"],
        81
    );
    assert!(output["timeout"]
        .as_str()
        .expect("timeout should be a string")
        .contains("0.25s"));
}

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
def runner(command, cwd, text, check, env, stdin, stdout, stderr, timeout):
    calls.append({{"command": command, "cwd": str(cwd), "timeout": timeout, "stdin_is_devnull": stdin == module.subprocess.DEVNULL}})
    class Result:
        returncode = 0
        stdout = '{{"type":"done"}}\n'
        stderr = ""
    return Result()

summary = module.run_omp_with_timeout(
    ["omp", "-p", "@/tmp/prompt.txt"],
    Path("target/workspace"),
    {{"HOME": "target/home"}},
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
    assert_eq!(output["calls"][0]["cwd"], "target/workspace");
    assert_eq!(output["calls"][0]["stdin_is_devnull"], true);
}

#[test]
fn denovo_codex_agent_safe_extract_allows_internal_symlink_members() {
    let dir = target_temp_dir("denovo-codex-safe-extract-internal-link");
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
    readme = b"changes"
    info = tarfile.TarInfo("repo/CHANGES.rst")
    info.size = len(readme)
    tar.addfile(info, io.BytesIO(readme))

    info = tarfile.TarInfo("repo/docs/source/changelog.rst")
    info.type = tarfile.SYMTYPE
    info.linkname = "../../CHANGES.rst"
    tar.addfile(info)

buffer.seek(0)
with tarfile.open(fileobj=buffer, mode="r") as tar:
    mod._safe_extract_tar(tar, destination)

link = destination / "repo" / "docs" / "source" / "changelog.rst"
print(json.dumps({{
    "is_symlink": link.is_symlink(),
    "target": link.readlink().as_posix(),
    "content": link.read_text(encoding="utf-8"),
}}))
"#,
        agent_path = denovo_codex_agent_path_json(),
        destination = serde_json::to_string(&dir).expect("destination should serialize"),
    );
    let output = run_python_json(&script);
    assert_eq!(output["is_symlink"], true);
    assert_eq!(output["target"], "../../CHANGES.rst");
    assert_eq!(output["content"], "changes");
    fs::remove_dir_all(&dir).expect("temp tar workspace should clean up");
}

#[test]
fn denovo_codex_agent_safe_extract_rejects_escaping_symlink_members() {
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
    assert!(output["message"]
        .as_str()
        .expect("message should be a string")
        .contains("link-out"));
    fs::remove_dir_all(&dir).expect("temp tar workspace should clean up");
}

#[test]
fn denovo_codex_agent_copy_exported_workspace_preserves_dangling_symlinks() {
    let dir = target_temp_dir("denovo-codex-copy-export-dangling-link");
    let script = format!(
        r#"
import importlib.util
import json
import sys
from pathlib import Path

spec = importlib.util.spec_from_file_location("denovo_codex_agent_copy_test", {agent_path})
mod = importlib.util.module_from_spec(spec)
sys.modules[spec.name] = mod
spec.loader.exec_module(mod)

root = Path({root})
source = root / "source"
workspace = root / "workspace"
(source / "docs").mkdir(parents=True)
(source / "docs" / "contributing.md").symlink_to("../missing/contributing.md")
mod.copy_exported_workspace(source, workspace)
link = workspace / "docs" / "contributing.md"
print(json.dumps({{
    "is_symlink": link.is_symlink(),
    "target": link.readlink().as_posix(),
    "exists": link.exists(),
}}))
"#,
        agent_path = denovo_codex_agent_path_json(),
        root = serde_json::to_string(&dir).expect("root should serialize"),
    );
    let output = run_python_json(&script);
    assert_eq!(output["is_symlink"], true);
    assert_eq!(output["target"], "../missing/contributing.md");
    assert_eq!(output["exists"], false);
    fs::remove_dir_all(&dir).expect("temp copy workspace should clean up");
}

#[test]
fn denovo_codex_agent_copy_exported_workspace_skips_local_stateful_artifacts() {
    let dir = target_temp_dir("denovo-codex-copy-export-skips-stateful-artifacts");
    let script = format!(
        r#"
import importlib.util
import json
import sys
from pathlib import Path

spec = importlib.util.spec_from_file_location("denovo_codex_agent_copy_ignore_test", {agent_path})
mod = importlib.util.module_from_spec(spec)
sys.modules[spec.name] = mod
spec.loader.exec_module(mod)

root = Path({root})
source = root / "source"
workspace = root / "workspace"
(source / ".stateful_bench" / "agent_synthetic").mkdir(parents=True)
(source / ".stateful_bench" / "agent_synthetic" / "codex_synthetic_agent.py").write_text("copied")
(source / "doc.txt").write_text("kept")
mod.copy_exported_workspace(source, workspace)
print(json.dumps({{
    "doc": (workspace / "doc.txt").read_text(),
    "stateful_bench_exists": (workspace / ".stateful_bench").exists(),
}}))
"#,
        agent_path = denovo_codex_agent_path_json(),
        root = serde_json::to_string(&dir).expect("root should serialize"),
    );
    let output = run_python_json(&script);
    assert_eq!(output["doc"], "kept");
    assert_eq!(output["stateful_bench_exists"], false);
    fs::remove_dir_all(&dir).expect("temp copy workspace should clean up");
}

#[test]
fn denovo_codex_agent_copy_exported_workspace_skips_upstream_checkout() {
    let dir = target_temp_dir("denovo-codex-copy-export-skips-upstream");
    let script = format!(
        r#"
import importlib.util
import json
import sys
from pathlib import Path

spec = importlib.util.spec_from_file_location("denovo_codex_agent_copy_upstream_test", {agent_path})
mod = importlib.util.module_from_spec(spec)
sys.modules[spec.name] = mod
spec.loader.exec_module(mod)

root = Path({root})
source = root / "source"
workspace = root / "workspace"
(source / "upstream" / "package").mkdir(parents=True)
(source / "upstream" / "package" / "answer.py").write_text("leaked")
(source / "README.md").write_text("kept")
mod.copy_exported_workspace(source, workspace)
print(json.dumps({{
    "readme": (workspace / "README.md").read_text(),
    "upstream_exists": (workspace / "upstream").exists(),
}}))
"#,
        agent_path = denovo_codex_agent_path_json(),
        root = serde_json::to_string(&dir).expect("root should serialize"),
    );
    let output = run_python_json(&script);
    assert_eq!(output["readme"], "kept");
    assert_eq!(output["upstream_exists"], false);
    fs::remove_dir_all(&dir).expect("temp copy workspace should clean up");
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
    "stateful_binary": "/opt/stateful/bin/stateful",
    "benchmark_model": "gpt-5.4-mini",
    "benchmark_reasoning_effort": "low",
    "benchmark_model_context_window": 256000,
    "benchmark_temperature": "1",
}}
no_state = module.codex_command_for_profile(agent_mode="no-state", **kwargs)
stateful = module.codex_command_for_profile(agent_mode="stateful", **kwargs)
nested_kwargs = dict(kwargs)
nested_kwargs["base_env"] = {{"STATEFUL_NESTED_CODEX_HOME_ROOT": "/repo/target/nested-codex-homes"}}
nested_no_state = module.codex_command_for_profile(agent_mode="no-state", **nested_kwargs)
print(json.dumps({{"no_state": no_state, "stateful": stateful, "nested_no_state": nested_no_state}}))
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
    let nested_no_state = output["nested_no_state"]
        .as_array()
        .expect("nested no-state command should be an array");

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

    assert_eq!(nested_no_state[0], "/opt/homebrew/bin/codex");
    assert!(!command_contains(nested_no_state, "--ignore-user-config"));
    assert!(command_contains(nested_no_state, "--ignore-rules"));
    assert!(command_contains(
        nested_no_state,
        "skills.bundled.enabled=false"
    ));
    assert!(command_contains(
        nested_no_state,
        "features.multi_agent=true"
    ));
}

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
relative_command = module.omp_command_for_profile(
    workspace=Path("/tmp/workspace"),
    prompt_path=Path("relative/prompt.txt"),
    omp_bin="/opt/homebrew/bin/omp",
    benchmark_model="deepseek-v4-flash",
)
native_command = module.omp_command_for_profile(
    workspace=Path("/tmp/workspace"),
    prompt_path=Path("/tmp/instance/prompt.txt"),
    omp_bin="/opt/homebrew/bin/omp",
    benchmark_model="deepseek-v4-flash",
    enable_native_subagent=True,
)
relative_prompt_arg = next(arg for arg in relative_command if arg.startswith("@"))
command_prompt_arg = next(arg for arg in command if arg.startswith("@"))
print(json.dumps({{"command": command, "native_command": native_command, "command_prompt_arg": command_prompt_arg, "relative_prompt_arg": relative_prompt_arg}}))
"#,
        agent_path = denovo_codex_agent_path_json(),
    );
    let output = run_python_json(&script);
    let command = output["command"]
        .as_array()
        .expect("command should be an array");
    let native_command = output["native_command"]
        .as_array()
        .expect("native command should be an array");
    let relative_prompt_arg = output["relative_prompt_arg"]
        .as_str()
        .expect("relative prompt arg should be a string");
    let command_prompt_arg = output["command_prompt_arg"]
        .as_str()
        .expect("command prompt arg should be a string");

    assert!(
        relative_prompt_arg.starts_with("@/"),
        "relative prompt arg should be absolute: {relative_prompt_arg}"
    );
    assert!(
        relative_prompt_arg.ends_with("/relative/prompt.txt"),
        "relative prompt arg should keep the prompt suffix: {relative_prompt_arg}"
    );

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
    assert!(
        command_prompt_arg.starts_with("@/"),
        "command prompt arg should be absolute: {command_prompt_arg}"
    );
    assert!(
        command_prompt_arg.ends_with("/tmp/instance/prompt.txt")
            || command_prompt_arg.ends_with("/private/tmp/instance/prompt.txt"),
        "command prompt arg should keep the prompt suffix: {command_prompt_arg}"
    );
    assert!(!command_contains(command, "exec"));
    assert!(!command_contains(command, "--json"));
    assert!(!command_contains(command, "--ignore-rules"));
    assert!(!command_contains(command, "--ignore-user-config"));
    assert!(!command_contains(
        command,
        "--dangerously-bypass-hook-trust"
    ));
    assert!(!command_contains(command, "features.multi_agent=true"));
    assert!(!command_contains(
        native_command,
        "features.multi_agent=true"
    ));
    assert!(command_arg_after(native_command, "--append-system-prompt")
        .expect("native system prompt should exist")
        .contains("Before implementation or broad repository exploration"));
    assert!(
        command_contains(native_command, "@/tmp/instance/prompt.txt")
            || command_contains(native_command, "@/private/tmp/instance/prompt.txt")
    );
}

#[test]
fn denovo_omp_agent_dockerfile_installs_bubblewrap_for_sandbox_tools() {
    let dockerfile = fs::read_to_string(
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("docker/denovo-omp-agent.Dockerfile"),
    )
    .expect("Dockerfile should read");

    assert!(dockerfile.contains("bubblewrap"));
    assert!(dockerfile.contains("command -v bwrap"));
}
#[test]
fn denovo_codex_agent_builds_dockerized_omp_command_with_minimal_mounts() {
    let temp_dir = target_temp_dir("stateful-bench-denovo-omp-docker-command");
    let script = format!(
        r#"
import importlib.util
import json
import sys
from pathlib import Path

spec = importlib.util.spec_from_file_location("denovo_omp_agent_docker_command_test", {agent_path})
module = importlib.util.module_from_spec(spec)
sys.modules[spec.name] = module
spec.loader.exec_module(module)

root = Path({temp_dir})
workspace = root / "workspace"
prompt = root / "instance" / "prompt.txt"
home = root / "home"
workspace.mkdir(parents=True)
prompt.parent.mkdir(parents=True)
prompt.write_text("prompt")
home.mkdir(parents=True)

command = module.docker_omp_command_for_profile(
    workspace=workspace,
    prompt_path=prompt,
    home=home,
    omp_bin="omp",
    benchmark_model="deepseek-v4-flash",
    docker_image="ghcr.io/stateful/omp-agent:latest",
    base_env={{
        "HOME": "host-home",
        "OPENAI_API_KEY": "sk-test",
        "STATEFUL_SERVER_TOKEN": "token-123",
        "STATEFUL_SERVER_URL": "http://127.0.0.1:43873",
    }},
    enable_native_subagent=True,
)
print(json.dumps({{
    "command": command,
    "env": [command[index + 1] for index, value in enumerate(command) if value == "--env"],
    "mounts": [command[index + 1] for index, value in enumerate(command) if value == "--mount"],
}}, sort_keys=True))
"#,
        agent_path = denovo_codex_agent_path_json(),
        temp_dir = serde_json::to_string(&temp_dir.to_string_lossy())
            .expect("temp dir should encode as json"),
    );
    let output = run_python_json(&script);
    let command = output["command"]
        .as_array()
        .expect("command should be an array");
    let env = output["env"].as_array().expect("env should be an array");
    let mounts = output["mounts"]
        .as_array()
        .expect("mounts should be an array");

    assert_eq!(command[0], "docker");
    assert!(command_contains(command, "run"));
    assert!(command_contains(command, "--rm"));
    assert_eq!(command_arg_after(command, "--network"), Some("bridge"));
    assert_eq!(command_arg_after(command, "--workdir"), Some("/workspace"));
    assert!(command_contains(
        command,
        "ghcr.io/stateful/omp-agent:latest"
    ));
    let image_index = command
        .iter()
        .position(|value| value.as_str() == Some("ghcr.io/stateful/omp-agent:latest"))
        .expect("docker image should be present");
    assert_eq!(command[image_index + 1].as_str(), Some("omp"));
    assert!(command_contains(command, "--cwd"));
    assert!(command_contains(command, "/workspace"));
    assert!(command_contains(command, "@/prompt.txt"));
    assert!(command_contains(command, "--approval-mode"));
    assert!(command_contains(command, "yolo"));
    assert!(!command_contains(command, "features.multi_agent=true"));
    assert!(command_arg_after(command, "--append-system-prompt")
        .expect("docker system prompt should exist")
        .contains("Before implementation or broad repository exploration"));

    assert!(env
        .iter()
        .any(|value| value.as_str() == Some("HOME=/home/stateful")));
    assert!(env
        .iter()
        .any(|value| value.as_str() == Some("OPENAI_API_KEY")));
    assert!(env.iter().any(|value| {
        value.as_str() == Some("STATEFUL_SERVER_URL=http://host.docker.internal:43873")
    }));
    assert!(env
        .iter()
        .any(|value| value.as_str() == Some("STATEFUL_SERVER_TOKEN")));
    assert!(!env
        .iter()
        .any(|value| value.as_str() == Some("OPENAI_API_KEY=sk-test")));
    assert!(!env
        .iter()
        .any(|value| value.as_str() == Some("STATEFUL_SERVER_TOKEN=token-123")));
    let command_text = serde_json::to_string(command).expect("command should encode");
    assert!(!command_text.contains("sk-test"));
    assert!(!command_text.contains("token-123"));
    assert!(!env
        .iter()
        .any(|value| value.as_str() == Some("HOME=host-home")));
    assert!(mounts.iter().any(|value| {
        value
            .as_str()
            .expect("mount should be text")
            .contains("target=/workspace")
    }));
    assert!(mounts.iter().any(|value| {
        let mount = value.as_str().expect("mount should be text");
        mount.contains("target=/prompt.txt") && mount.contains("readonly")
    }));
    assert!(mounts.iter().any(|value| {
        value
            .as_str()
            .expect("mount should be text")
            .contains("target=/home/stateful")
    }));

    fs::remove_dir_all(temp_dir).expect("temp dir should clean up");
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
    "STATEFUL_SERVER_URL": "http://127.0.0.1:43873",
    "STATEFUL_SERVER_TOKEN": "token-123",
    "CODEX_THREAD_ID": "outer-thread",
    "STATEFUL_CODEX_RUN_ID": "outer-run",
    "STATEFUL_SESSION_ID": "outer-session",
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
    preserve_stateful_session=True,
    stateful_session_id="denovo-issue-stateful-test",
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
    "no_state_has_thread": "CODEX_THREAD_ID" in no_state_env,
    "no_state_has_codex_run": "STATEFUL_CODEX_RUN_ID" in no_state_env,
    "no_state_has_session": "STATEFUL_SESSION_ID" in no_state_env,
    "no_state_config_exists": (no_state_home / "config.toml").exists(),
    "no_state_skill_exists": (no_state_home / "skills" / "stateful-command-policy" / "SKILL.md").exists(),
    "no_state_auth_exists": (no_state_home / "auth.json").exists(),
    "stateful_home": stateful_env["HOME"],
    "stateful_has_thread": "CODEX_THREAD_ID" in stateful_env,
    "stateful_has_codex_run": "STATEFUL_CODEX_RUN_ID" in stateful_env,
    "stateful_codex_run_id": stateful_env.get("STATEFUL_CODEX_RUN_ID"),
    "stateful_has_session": "STATEFUL_SESSION_ID" in stateful_env,
    "stateful_session_id": stateful_env.get("STATEFUL_SESSION_ID"),
    "stateful_server_url": stateful_env.get("STATEFUL_SERVER_URL"),
    "stateful_server_token": stateful_env.get("STATEFUL_SERVER_TOKEN"),
    "stateful_config": stateful_config,
    "stateful_skill_exists": (stateful_home / "skills" / "stateful-command-policy" / "SKILL.md").exists(),
    "stateful_auth_exists": (stateful_home / "auth.json").exists(),
    "generated_session_id": module.denovo_stateful_session_id(output, "owner/repo#1", task_path, workspace),
}}, sort_keys=True))
"#,
        agent_path = denovo_codex_agent_path_json(),
        temp_dir = serde_json::to_string(&temp_dir.to_string_lossy())
            .expect("temp dir should encode as json"),
    );
    let output = run_python_json(&script);

    assert!(output["no_state_home"]
        .as_str()
        .expect("no-state home should be text")
        .ends_with("adapter-output/codex-homes/issue-no-state/home"));
    assert_eq!(output["no_state_has_session"], false);
    assert_eq!(output["no_state_has_thread"], false);
    assert_eq!(output["no_state_has_codex_run"], false);
    assert_eq!(output["no_state_config_exists"], false);
    assert_eq!(output["no_state_skill_exists"], false);
    assert_eq!(output["no_state_auth_exists"], true);

    assert!(output["stateful_home"]
        .as_str()
        .expect("stateful home should be text")
        .ends_with("adapter-output/codex-homes/issue-stateful/home"));
    let config = output["stateful_config"]
        .as_str()
        .expect("stateful config should be text");
    assert!(config.contains("[mcp_servers.stateful]"));
    assert!(config.contains("command = \"/tmp/stateful\""));
    assert!(config.contains(
        "env_vars = [\"CODEX_THREAD_ID\", \"STATEFUL_CODEX_RUN_ID\", \"STATEFUL_SERVER_URL\", \"STATEFUL_SERVER_TOKEN\", \"STATEFUL_SESSION_ID\"]"
    ));
    assert!(config.contains("[[hooks.SessionStart]]"));
    assert_eq!(output["stateful_has_session"], true);
    assert_eq!(output["stateful_has_thread"], false);
    assert_eq!(output["stateful_has_codex_run"], true);
    assert_eq!(output["stateful_session_id"], "denovo-issue-stateful-test");
    assert_eq!(
        output["stateful_codex_run_id"],
        "denovo-issue-stateful-test"
    );
    assert_eq!(output["stateful_server_url"], "http://127.0.0.1:43873");
    assert_eq!(output["stateful_server_token"], "token-123");
    assert_eq!(output["stateful_skill_exists"], true);
    assert_eq!(output["stateful_auth_exists"], true);
    let generated_session_id = output["generated_session_id"]
        .as_str()
        .expect("generated session id should be text");
    assert!(generated_session_id.starts_with("denovo-owner-repo-1-"));
    assert!(generated_session_id
        .bytes()
        .all(|byte| { byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_' }));

    fs::remove_dir_all(temp_dir).expect("temp dir should clean up");
}

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
    "XDG_CONFIG_HOME": str(root / "host-config"),
    "XDG_CACHE_HOME": str(root / "host-cache"),
}}
output = root / "adapter-output"
workspace = root / "workspace"
workspace.mkdir(parents=True, exist_ok=True)
task_path = root / "extracts" / "results.jsonl"

commands = []
def fake_runner(command, text, check, env, stdout, stderr):
    commands.append({{"command": command, "home": env.get("HOME"), "stateful_home": env.get("STATEFUL_HOME")}})
    agent = Path(env["PI_CODING_AGENT_DIR"])
    agent.mkdir(parents=True, exist_ok=True)
    extension = agent / "extensions" / "stateful-omp-extension.js"
    extension.parent.mkdir(parents=True, exist_ok=True)
    extension.write_text("extension")
    (agent / "config.yml").write_text(f"extensions:\n  - {{extension}}\ntools:\n  approvalMode: yolo\n")
    class Result:
        returncode = 0
        stdout = ""
        stderr = ""
    return Result()

no_state_env = module.denovo_omp_environment(output, "issue/no-state", task_path, workspace, source_env)
module.prepare_omp_environment(no_state_env, enable_stateful=False, stateful_binary="/tmp/stateful", runner=fake_runner)
no_state_agent = Path(no_state_env["PI_CODING_AGENT_DIR"])

stateful_env = module.denovo_omp_environment(output, "issue/stateful", task_path, workspace, source_env)
module.prepare_omp_environment(
    stateful_env,
    enable_stateful=True,
    stateful_binary="/tmp/stateful",
    runner=fake_runner,
    runtime_stateful_binary="/container/stateful",
    runtime_omp_home="/home/stateful",
    omp_bin="/tmp/omp",
    enable_native_subagent=True,
    agent_docker_image="ghcr.io/stateful/omp-agent:latest",
)
stateful_agent = Path(stateful_env["PI_CODING_AGENT_DIR"])
explicit_stateful_env = module.denovo_omp_environment(
    output,
    "issue/explicit-stateful",
    task_path,
    workspace,
    source_env,
    stateful_session_id="denovo-issue-stateful-test",
)
expected_no_state_home = output / "omp-homes" / module.path_fragment("issue/no-state") / "home"
expected_stateful_home = output / "omp-homes" / module.path_fragment("issue/stateful") / "home"
expected_no_state_config_home = expected_no_state_home / ".config"
expected_no_state_cache_home = expected_no_state_home / ".cache"
expected_stateful_config_home = expected_stateful_home / ".config"
expected_stateful_cache_home = expected_stateful_home / ".cache"


stateful_config = (stateful_agent / "config.yml").read_text()

print(json.dumps({{
    "no_state_home": no_state_env["HOME"],
    "no_state_agent": no_state_env["PI_CODING_AGENT_DIR"],
    "no_state_has_codex_home": "CODEX_HOME" in no_state_env,
    "no_state_has_codex_thread": "CODEX_THREAD_ID" in no_state_env,
    "no_state_has_codex_run": "STATEFUL_CODEX_RUN_ID" in no_state_env,
    "no_state_has_session": "STATEFUL_SESSION_ID" in no_state_env,
    "no_state_config_exists": (no_state_agent / "config.yml").exists(),
    "no_state_xdg_config_home": no_state_env["XDG_CONFIG_HOME"],
    "no_state_xdg_cache_home": no_state_env["XDG_CACHE_HOME"],
    "stateful_home": stateful_env["HOME"],
    "stateful_agent": stateful_env["PI_CODING_AGENT_DIR"],
    "stateful_has_codex_home": "CODEX_HOME" in stateful_env,
    "stateful_has_codex_thread": "CODEX_THREAD_ID" in stateful_env,
    "stateful_has_codex_run": "STATEFUL_CODEX_RUN_ID" in stateful_env,
    "stateful_has_session": "STATEFUL_SESSION_ID" in stateful_env,
    "stateful_config": stateful_config,
    "stateful_config_exists": (stateful_agent / "config.yml").exists(),
    "stateful_xdg_config_home": stateful_env["XDG_CONFIG_HOME"],
    "stateful_xdg_cache_home": stateful_env["XDG_CACHE_HOME"],
    "explicit_stateful_has_session": "STATEFUL_SESSION_ID" in explicit_stateful_env,
    "explicit_stateful_session_id": explicit_stateful_env.get("STATEFUL_SESSION_ID"),
    "explicit_stateful_has_codex_run": "STATEFUL_CODEX_RUN_ID" in explicit_stateful_env,
    "explicit_stateful_codex_run_id": explicit_stateful_env.get("STATEFUL_CODEX_RUN_ID"),
    "unpack_command": commands[0]["command"] if commands else [],
    "install_command": commands[1]["command"] if len(commands) > 1 else [],
    "expected_no_state_home": str(expected_no_state_home),
    "expected_stateful_home": str(expected_stateful_home),
    "expected_no_state_config_home": str(expected_no_state_config_home),
    "expected_no_state_cache_home": str(expected_no_state_cache_home),
    "expected_stateful_config_home": str(expected_stateful_config_home),
    "expected_stateful_cache_home": str(expected_stateful_cache_home),
}}, sort_keys=True))
"#,
        agent_path = denovo_codex_agent_path_json(),
        temp_dir = serde_json::to_string(&temp_dir.to_string_lossy())
            .expect("temp dir should encode as json"),
    );
    let output = run_python_json(&script);

    assert_eq!(output["no_state_home"], output["expected_no_state_home"]);
    assert_eq!(output["stateful_home"], output["expected_stateful_home"]);
    let expected_no_state_agent = format!(
        "{}/.omp/profiles/stateful/agent",
        output["expected_no_state_home"]
            .as_str()
            .expect("expected no-state home should be text")
    );
    assert_eq!(
        output["no_state_agent"]
            .as_str()
            .expect("no-state agent should be text"),
        expected_no_state_agent
    );
    let expected_stateful_agent = format!(
        "{}/.omp/profiles/stateful/agent",
        output["expected_stateful_home"]
            .as_str()
            .expect("expected stateful home should be text")
    );
    assert_eq!(
        output["stateful_agent"]
            .as_str()
            .expect("stateful agent should be text"),
        expected_stateful_agent
    );
    assert_eq!(output["no_state_has_codex_home"], false);
    assert_eq!(output["stateful_has_codex_home"], false);
    assert_eq!(output["no_state_has_codex_thread"], false);
    assert_eq!(output["stateful_has_codex_thread"], false);
    assert_eq!(output["no_state_has_codex_run"], false);
    assert_eq!(output["stateful_has_codex_run"], false);
    assert_eq!(output["no_state_has_session"], false);
    assert_eq!(output["stateful_has_session"], false);
    assert_eq!(
        output["no_state_xdg_config_home"],
        output["expected_no_state_config_home"]
    );
    assert_eq!(
        output["no_state_xdg_cache_home"],
        output["expected_no_state_cache_home"]
    );
    assert_eq!(
        output["stateful_xdg_config_home"],
        output["expected_stateful_config_home"]
    );
    assert_eq!(
        output["stateful_xdg_cache_home"],
        output["expected_stateful_cache_home"]
    );
    assert_eq!(output["explicit_stateful_has_session"], true);
    assert_eq!(
        output["explicit_stateful_session_id"],
        "denovo-issue-stateful-test"
    );
    assert_eq!(output["explicit_stateful_has_codex_run"], false);
    assert!(output["explicit_stateful_codex_run_id"].is_null());
    assert_eq!(output["no_state_config_exists"], false);
    assert_eq!(output["stateful_config_exists"], true);
    let stateful_config = output["stateful_config"]
        .as_str()
        .expect("stateful config should be text");
    assert!(
        stateful_config.contains(
            "/home/stateful/.omp/profiles/stateful/agent/extensions/stateful-omp-extension.js"
        ),
        "Docker OMP config should use container-visible extension path: {stateful_config}"
    );
    assert!(
        !stateful_config.contains(
            output["expected_stateful_home"]
                .as_str()
                .expect("expected stateful home should be text")
        ),
        "Docker OMP config must not keep host-only extension paths: {stateful_config}"
    );
    let unpack_command = output["unpack_command"]
        .as_array()
        .expect("unpack command should be captured");
    assert!(command_contains(unpack_command, "docker"));
    assert!(command_contains(unpack_command, "run"));
    assert!(command_contains(
        unpack_command,
        "ghcr.io/stateful/omp-agent:latest"
    ));
    assert!(command_contains(unpack_command, "/tmp/omp"));
    assert!(command_contains(unpack_command, "agents"));
    assert!(command_contains(unpack_command, "unpack"));
    assert!(command_contains(unpack_command, "--force"));
    let install_command = output["install_command"]
        .as_array()
        .expect("install command should be captured");
    assert!(command_contains(install_command, "install"));
    assert!(command_contains(install_command, "--agent"));
    assert!(command_contains(install_command, "omp"));
    assert!(command_contains(install_command, "--yes"));
    assert!(command_contains(install_command, "--binary"));
    assert!(command_contains(install_command, "/container/stateful"));

    fs::remove_dir_all(temp_dir).expect("temp dir should clean up");
}

#[test]
fn denovo_codex_agent_requires_stateful_runtime_env_for_stateful_profile() {
    let script = format!(
        r#"
import importlib.util
import json
import sys

spec = importlib.util.spec_from_file_location("denovo_codex_agent_stateful_runtime_env_test", {agent_path})
module = importlib.util.module_from_spec(spec)
sys.modules[spec.name] = module
spec.loader.exec_module(module)

missing_env = {{
    "PATH": "/bin",
}}
complete_env = {{
    "PATH": "/bin",
    "STATEFUL_SERVER_URL": "http://127.0.0.1:43873",
    "STATEFUL_SERVER_TOKEN": "token-123",
}}

if hasattr(module, "stateful_runtime_env_error"):
    missing_error = module.stateful_runtime_env_error(missing_env)
    complete_error = module.stateful_runtime_env_error(complete_env)
else:
    missing_error = None
    complete_error = "missing helper"

print(json.dumps({{
    "missing_error": missing_error,
    "complete_error": complete_error,
}}, sort_keys=True))
"#,
        agent_path = denovo_codex_agent_path_json(),
    );
    let output = run_python_json(&script);

    assert_eq!(
        output["missing_error"],
        "stateful Codex benchmark requires STATEFUL_SERVER_URL and STATEFUL_SERVER_TOKEN"
    );
    assert_eq!(output["complete_error"], serde_json::Value::Null);
}

#[test]
fn denovo_progress_report_aggregates_in_progress_shards_from_results_jsonl() {
    let temp_dir = target_temp_dir("stateful-bench-denovo-progress-report");
    let runs_root = temp_dir.join("runs");
    let shard_a = runs_root.join("r38-denovo-shard-a");
    let shard_b = runs_root.join("r38-denovo-shard-b");

    let shard_a_off = shard_a
        .join("conditions")
        .join("stateful-off_subagent-on")
        .join("codex-cli")
        .join("_");
    let shard_a_on = shard_a
        .join("conditions")
        .join("stateful-on_subagent-on")
        .join("codex-cli")
        .join("_");
    let shard_b_off = shard_b
        .join("conditions")
        .join("stateful-off_subagent-on")
        .join("codex-cli")
        .join("_");
    let shard_b_on = shard_b
        .join("conditions")
        .join("stateful-on_subagent-on")
        .join("codex-cli")
        .join("_");
    for dir in [&shard_a_off, &shard_a_on, &shard_b_off, &shard_b_on] {
        fs::create_dir_all(dir).expect("fixture result dir should be created");
    }
    fs::write(
        shard_a_off.join("results.jsonl"),
        [
            r#"{"instance_id":"a-1","success":true,"score":1.0,"finish_reason":"stop","subagent_used":true,"orchestration_trace":{"trace_captured":true,"reservation_events":2,"claim_events":1,"conflict_events":0}}"#,
            r#"{"instance_id":"a-2","success":false,"score":0.5,"finish_reason":"setup-error","subagent_used":false,"orchestration_trace":{"trace_captured":false,"reservation_events":0,"claim_events":0,"conflict_events":0}}"#,
        ]
        .join("\n")
            + "\n",
    )
    .expect("fixture results should be written");
    fs::write(
        shard_a_on.join("results.jsonl"),
        r#"{"instance_id":"a-1","success":false,"score":0.0,"finish_reason":"setup-error","error":"stateful Codex benchmark requires STATEFUL_SERVER_URL and STATEFUL_SERVER_TOKEN","subagent_usage":{"subagent_used":true}}"#,
    )
    .expect("fixture results should be written");
    fs::write(
        shard_b_off.join("results.jsonl"),
        r#"{"instance_id":"b-1","success":false,"score":0.25,"finish_reason":"context-limit","subagent_usage":{"subagent_used":true}}"#,
    )
    .expect("fixture results should be written");
    fs::write(shard_b_on.join("results.jsonl"), "").expect("empty fixture should be written");

    let script = format!(
        r#"
import importlib.util
import json
import sys
from pathlib import Path

spec = importlib.util.spec_from_file_location("denovo_progress_report_test", {script_path})
module = importlib.util.module_from_spec(spec)
sys.modules[spec.name] = module
spec.loader.exec_module(module)

summary = module.collect_progress(
    [Path({shard_a}), Path({shard_b})],
    expected_instances_per_condition=4,
)
print(json.dumps(summary, sort_keys=True))
"#,
        script_path = denovo_progress_report_path_json(),
        shard_a = serde_json::to_string(&shard_a.to_string_lossy())
            .expect("shard path should encode as json"),
        shard_b = serde_json::to_string(&shard_b.to_string_lossy())
            .expect("shard path should encode as json"),
    );
    let output = run_python_json(&script);

    assert_eq!(output["run_count"], 2);
    assert_eq!(output["total_result_rows"], 4);
    assert_eq!(output["expected_instances_per_condition"], 4);

    let conditions = output["conditions"]
        .as_array()
        .expect("conditions should be an array");
    let off = conditions
        .iter()
        .find(|condition| condition["condition_id"] == "stateful-off_subagent-on")
        .expect("off condition should be summarized");
    assert_eq!(off["rows"], 3);
    assert_eq!(off["success_count"], 1);
    assert_eq!(off["setup_errors"], 1);
    assert_eq!(off["finish_reasons"]["setup-error"], 1);
    assert_eq!(off["finish_reasons"]["context-limit"], 1);
    assert_eq!(off["subagent_used_count"], 2);
    assert_eq!(off["subagent_observed"], 3);
    assert_eq!(off["orchestration_trace_observed"], 2);
    assert_eq!(off["orchestration_trace_captured"], 1);
    assert_eq!(off["orchestration_reservation_events"], 2);
    assert_eq!(off["orchestration_claim_events"], 1);
    assert_eq!(off["orchestration_conflict_events"], 0);
    assert_eq!(off["progress_rate"], 0.75);
    assert!(
        (off["average_score"]
            .as_f64()
            .expect("average score should be numeric")
            - 0.5833333333333334)
            .abs()
            < 0.000001
    );

    let on = conditions
        .iter()
        .find(|condition| condition["condition_id"] == "stateful-on_subagent-on")
        .expect("on condition should be summarized");
    assert_eq!(on["rows"], 1);
    assert_eq!(on["setup_errors"], 1);
    assert_eq!(on["subagent_used_count"], 1);
    assert_eq!(on["progress_rate"], 0.25);

    let runs = output["runs"].as_array().expect("runs should be an array");
    assert!(
        runs.iter().any(|run| run["run_id"] == "r38-denovo-shard-b"
            && run["condition_id"] == "stateful-on_subagent-on"
            && run["rows"] == 0),
        "empty in-progress result files should be represented"
    );

    fs::remove_dir_all(temp_dir).expect("temp dir should clean up");
}

#[test]
fn denovo_progress_report_prefers_cumulative_condition_report() {
    let temp_dir = target_temp_dir("stateful-bench-denovo-progress-report-cumulative");
    let run_dir = temp_dir.join("runs").join("r38-denovo-shard-a");
    let condition_dir = run_dir.join("conditions").join("stateful-off_subagent-on");
    let result_dir = condition_dir.join("codex-cli").join("_");
    fs::create_dir_all(&result_dir).expect("fixture result dir should be created");
    fs::write(
        result_dir.join("results.jsonl"),
        r#"{"instance_id":"transient-current","success":false,"score":0.0,"finish_reason":"setup-error"}"#,
    )
    .expect("fixture results should be written");
    fs::write(
        condition_dir.join("denovo-report.json"),
        r#"{"condition_id":"stateful-off_subagent-on","total_instances":3,"success_count":2,"average_score":0.75,"completed_instances":3,"scored_instances":3,"error_count":0,"subagent_observed_instances":3,"subagent_used_count":2,"subagent_used_rate":0.6666666667,"orchestration_trace_observed":3,"orchestration_trace_captured":2,"orchestration_reservation_events":5,"orchestration_claim_events":4,"orchestration_conflict_events":1,"running_time_ms":1234}"#,
    )
    .expect("fixture cumulative report should be written");

    let script = format!(
        r#"
import importlib.util
import json
import sys
from pathlib import Path

spec = importlib.util.spec_from_file_location("denovo_progress_report_cumulative_test", {script_path})
module = importlib.util.module_from_spec(spec)
sys.modules[spec.name] = module
spec.loader.exec_module(module)

summary = module.collect_progress(
    [Path({run_dir})],
    expected_instances_per_condition=6,
)
print(json.dumps(summary, sort_keys=True))
"#,
        script_path = denovo_progress_report_path_json(),
        run_dir = serde_json::to_string(&run_dir.to_string_lossy())
            .expect("run path should encode as json"),
    );
    let output = run_python_json(&script);
    assert_eq!(output["total_result_rows"], 3);

    let condition = output["conditions"]
        .as_array()
        .expect("conditions should be an array")
        .iter()
        .find(|condition| condition["condition_id"] == "stateful-off_subagent-on")
        .expect("condition should be summarized");
    assert_eq!(condition["rows"], 3);
    assert_eq!(condition["success_count"], 2);
    assert_eq!(condition["setup_errors"], 1);
    assert_eq!(condition["finish_reasons"]["setup-error"], 1);
    assert_eq!(condition["average_score"], 0.75);
    assert_eq!(condition["progress_rate"], 0.5);
    assert_eq!(condition["subagent_used_count"], 2);
    assert_eq!(condition["subagent_observed"], 3);
    assert_eq!(condition["orchestration_trace_observed"], 3);
    assert_eq!(condition["orchestration_trace_captured"], 2);
    assert_eq!(condition["orchestration_reservation_events"], 5);
    assert_eq!(condition["orchestration_claim_events"], 4);
    assert_eq!(condition["orchestration_conflict_events"], 1);

    let run = output["runs"]
        .as_array()
        .expect("runs should be an array")
        .first()
        .expect("run summary should exist");
    assert_eq!(run["source"], "denovo-report.json");
    assert_eq!(run["rows"], 3);
    assert_eq!(run["orchestration_trace_observed"], 3);
    assert_eq!(run["orchestration_trace_captured"], 2);

    fs::remove_dir_all(temp_dir).expect("temp dir should clean up");
}

#[test]
fn denovo_progress_report_uses_results_jsonl_for_report_finish_reasons() {
    let temp_dir = target_temp_dir("stateful-bench-denovo-progress-report-finish-reasons");
    let run_dir = temp_dir.join("runs").join("r38-denovo-shard-a");
    let condition_dir = run_dir.join("conditions").join("stateful-off_subagent-on");
    let result_dir = condition_dir.join("codex-cli").join("_");
    fs::create_dir_all(&result_dir).expect("fixture result dir should be created");
    fs::write(
        result_dir.join("results.jsonl"),
        [
            r#"{"instance_id":"case-a","success":false,"score":0.0,"finish_reason":"setup-error"}"#,
            r#"{"instance_id":"case-b","success":false,"score":0.0,"finish_reason":"codex-error"}"#,
            r#"{"instance_id":"case-c","success":true,"score":1.0,"finish_reason":"stop"}"#,
        ]
        .join("\n")
            + "\n",
    )
    .expect("fixture results should be written");
    fs::write(
        condition_dir.join("denovo-report.json"),
        r#"{"condition_id":"stateful-off_subagent-on","total_instances":3,"success_count":1,"average_score":0.3333333333,"completed_instances":3,"scored_instances":3,"error_count":2,"subagent_observed_instances":0,"subagent_used_count":0,"running_time_ms":1234}"#,
    )
    .expect("fixture cumulative report should be written");

    let script = format!(
        r#"
import importlib.util
import json
import sys
from pathlib import Path

spec = importlib.util.spec_from_file_location("denovo_progress_report_finish_reasons_test", {script_path})
module = importlib.util.module_from_spec(spec)
sys.modules[spec.name] = module
spec.loader.exec_module(module)

summary = module.collect_progress(
    [Path({run_dir})],
    expected_instances_per_condition=3,
)
print(json.dumps(summary, sort_keys=True))
"#,
        script_path = denovo_progress_report_path_json(),
        run_dir = serde_json::to_string(&run_dir.to_string_lossy())
            .expect("run path should encode as json"),
    );
    let output = run_python_json(&script);

    let condition = output["conditions"]
        .as_array()
        .expect("conditions should be an array")
        .iter()
        .find(|condition| condition["condition_id"] == "stateful-off_subagent-on")
        .expect("condition should be summarized");
    assert_eq!(condition["rows"], 3);
    assert_eq!(condition["setup_errors"], 1);
    assert_eq!(condition["finish_reasons"]["setup-error"], 1);
    assert_eq!(condition["finish_reasons"]["codex-error"], 1);
    assert_eq!(condition["finish_reasons"]["stop"], 1);

    let run = output["runs"]
        .as_array()
        .expect("runs should be an array")
        .first()
        .expect("run summary should exist");
    assert_eq!(run["source"], "denovo-report.json");
    assert_eq!(run["setup_errors"], 1);
    assert_eq!(run["finish_reasons"]["setup-error"], 1);
    assert_eq!(run["finish_reasons"]["codex-error"], 1);
    assert_eq!(run["finish_reasons"]["stop"], 1);

    fs::remove_dir_all(temp_dir).expect("temp dir should clean up");
}

#[test]
fn denovo_retry_overlay_report_replaces_only_codex_errors() {
    let temp_dir = target_temp_dir("stateful-bench-denovo-retry-overlay-report");
    let runs_root = temp_dir.join("runs");
    let base_run = runs_root.join("r-base-denovo-12-t3-shard-a");
    let retry_run = runs_root.join("r-retry-denovo-12-t3-codex-error-rerun-shard-a");
    let base_off = base_run
        .join("conditions")
        .join("stateful-off_subagent-on")
        .join("codex-cli")
        .join("_");
    let base_on = base_run
        .join("conditions")
        .join("stateful-on_subagent-on")
        .join("codex-cli")
        .join("_");
    let retry_off = retry_run
        .join("conditions")
        .join("stateful-off_subagent-on")
        .join("codex-cli")
        .join("_");
    let retry_on = retry_run
        .join("conditions")
        .join("stateful-on_subagent-on")
        .join("codex-cli")
        .join("_");
    for dir in [&base_off, &base_on, &retry_off, &retry_on] {
        fs::create_dir_all(dir).expect("fixture result dir should be created");
    }
    fs::write(
        base_off.join("results.jsonl"),
        [
            r#"{"instance_id":"case-a","success":false,"finish_reason":"codex-error","error":"codex exited 1","subagent_used":false}"#,
            r#"{"instance_id":"case-b","success":false,"finish_reason":"missing-runtime-image","error":"image missing"}"#,
            r#"{"instance_id":"case-c","success":true,"score":1.0,"finish_reason":"stop","subagent_used":true}"#,
        ]
        .join("\n")
            + "\n",
    )
    .expect("base off results should be written");
    fs::write(
        base_on.join("results.jsonl"),
        r#"{"instance_id":"case-a","success":false,"finish_reason":"codex-error","error":"codex exited 1","subagent_used":false}"#,
    )
    .expect("base on results should be written");
    fs::write(
        retry_off.join("results.jsonl"),
        [
            r#"{"instance_id":"case-a","success":true,"score":0.75,"finish_reason":"stop","subagent_used":true}"#,
            r#"{"instance_id":"case-b","success":true,"score":1.0,"finish_reason":"stop","subagent_used":true}"#,
        ]
        .join("\n")
            + "\n",
    )
    .expect("retry off results should be written");
    fs::write(
        retry_on.join("results.jsonl"),
        r#"{"instance_id":"case-a","success":false,"score":0.25,"finish_reason":"stop","subagent_used":true}"#,
    )
    .expect("retry on results should be written");

    let script = format!(
        r#"
import importlib.util
import json
import sys
from pathlib import Path

spec = importlib.util.spec_from_file_location("denovo_retry_overlay_report_test", {script_path})
module = importlib.util.module_from_spec(spec)
sys.modules[spec.name] = module
spec.loader.exec_module(module)

summary = module.collect_overlay_summary(
    runs_root=Path({runs_root}),
    trials=[module.TrialSpec("t3", ["r-base-denovo-12-t3-shard"], ["r-retry-denovo-12-t3-codex-error-rerun-shard"])],
    expected_instances_per_condition=3,
)
print(json.dumps(summary, sort_keys=True))
"#,
        script_path = denovo_retry_overlay_report_path_json(),
        runs_root = serde_json::to_string(&runs_root.to_string_lossy())
            .expect("runs root should encode as json"),
    );
    let output = run_python_json(&script);

    assert_eq!(output["trial_count"], 1);
    assert_eq!(output["total_base_rows"], 4);
    assert_eq!(output["total_effective_rows"], 4);
    assert_eq!(output["total_replacements"], 2);
    assert_eq!(output["unused_retry_rows"], 1);

    let off = output["conditions"]
        .as_array()
        .expect("conditions should be an array")
        .iter()
        .find(|condition| condition["condition_id"] == "stateful-off_subagent-on")
        .expect("off condition should be present");
    assert_eq!(off["rows"], 3);
    assert_eq!(off["success_count"], 2);
    assert_eq!(off["scored_count"], 2);
    assert_eq!(
        off["finish_reasons"]["codex-error"],
        serde_json::Value::Null
    );
    assert_eq!(off["finish_reasons"]["missing-runtime-image"], 1);
    assert_eq!(off["replacement_count"], 1);
    assert!(
        off["average_score"]
            .as_f64()
            .expect("average score should be numeric")
            > 0.87
    );

    let on = output["conditions"]
        .as_array()
        .expect("conditions should be an array")
        .iter()
        .find(|condition| condition["condition_id"] == "stateful-on_subagent-on")
        .expect("on condition should be present");
    assert_eq!(on["rows"], 1);
    assert_eq!(on["success_count"], 0);
    assert_eq!(on["scored_count"], 1);
    assert_eq!(on["replacement_count"], 1);
    assert_eq!(on["finish_reasons"]["stop"], 1);

    fs::remove_dir_all(temp_dir).expect("temp dir should clean up");
}

#[test]
fn denovo_retry_overlay_report_uses_all_trials_for_condition_progress() {
    let temp_dir = target_temp_dir("stateful-bench-denovo-retry-overlay-progress");
    let runs_root = temp_dir.join("runs");
    for (trial_id, prefix) in [("t1", "r-base-denovo-12-t1"), ("t2", "r-base-denovo-12-t2")] {
        let result_dir = runs_root
            .join(prefix)
            .join("conditions")
            .join("stateful-off_subagent-on")
            .join("codex-cli")
            .join("_");
        fs::create_dir_all(&result_dir).expect("fixture result dir should be created");
        fs::write(
            result_dir.join("results.jsonl"),
            [
                format!(
                    r#"{{"instance_id":"{trial_id}-case-a","success":true,"score":1.0,"finish_reason":"stop"}}"#
                ),
                format!(
                    r#"{{"instance_id":"{trial_id}-case-b","success":true,"score":1.0,"finish_reason":"stop"}}"#
                ),
            ]
            .join("\n")
                + "\n",
        )
        .expect("fixture results should be written");
    }

    let script = format!(
        r#"
import importlib.util
import json
import sys
from pathlib import Path

spec = importlib.util.spec_from_file_location("denovo_retry_overlay_report_progress_test", {script_path})
module = importlib.util.module_from_spec(spec)
sys.modules[spec.name] = module
spec.loader.exec_module(module)

summary = module.collect_overlay_summary(
    runs_root=Path({runs_root}),
    trials=[
        module.TrialSpec("t1", ["r-base-denovo-12-t1"], []),
        module.TrialSpec("t2", ["r-base-denovo-12-t2"], []),
    ],
    expected_instances_per_condition=2,
)
print(json.dumps(summary, sort_keys=True))
"#,
        script_path = denovo_retry_overlay_report_path_json(),
        runs_root = serde_json::to_string(&runs_root.to_string_lossy())
            .expect("runs root should encode as json"),
    );
    let output = run_python_json(&script);

    let condition = output["conditions"]
        .as_array()
        .expect("conditions should be an array")
        .iter()
        .find(|condition| condition["condition_id"] == "stateful-off_subagent-on")
        .expect("condition should be present");
    assert_eq!(condition["rows"], 4);
    assert_eq!(condition["progress_rate"], 1.0);

    let trials = output["trials"]
        .as_array()
        .expect("trials should be an array");
    assert_eq!(trials.len(), 2);
    for trial in trials {
        assert_eq!(trial["rows"], 2);
        assert_eq!(trial["progress_rate"], 1.0);
    }

    fs::remove_dir_all(temp_dir).expect("temp dir should clean up");
}

#[test]
fn denovo_overlay_instances_lists_only_negative_scored_deltas() {
    let temp_dir = target_temp_dir("stateful-bench-denovo-overlay-instances");
    let runs_root = temp_dir.join("runs");
    let base_run = runs_root.join("r-base-denovo-12-t1");
    let off_dir = base_run
        .join("conditions")
        .join("stateful-off_subagent-on")
        .join("codex-cli")
        .join("_");
    let on_dir = base_run
        .join("conditions")
        .join("stateful-on_subagent-on")
        .join("codex-cli")
        .join("_");
    fs::create_dir_all(&off_dir).expect("fixture off dir should be created");
    fs::create_dir_all(&on_dir).expect("fixture on dir should be created");
    fs::write(
        off_dir.join("results.jsonl"),
        [
            r#"{"instance_id":"negative","success":true,"score":1.0,"finish_reason":"stop"}"#,
            r#"{"instance_id":"zero","success":true,"score":0.5,"finish_reason":"stop"}"#,
            r#"{"instance_id":"positive","success":true,"score":0.25,"finish_reason":"stop"}"#,
        ]
        .join("\n")
            + "\n",
    )
    .expect("fixture off results should be written");
    fs::write(
        on_dir.join("results.jsonl"),
        [
            r#"{"instance_id":"negative","success":true,"score":0.5,"finish_reason":"stop"}"#,
            r#"{"instance_id":"zero","success":true,"score":0.5,"finish_reason":"stop"}"#,
            r#"{"instance_id":"positive","success":true,"score":0.75,"finish_reason":"stop"}"#,
        ]
        .join("\n")
            + "\n",
    )
    .expect("fixture on results should be written");

    let script = format!(
        r#"
import importlib.util
import json
import sys
from pathlib import Path

script_path = Path({script_path})
sys.path.insert(0, str(script_path.parent))
spec = importlib.util.spec_from_file_location("denovo_overlay_instances_test", script_path)
module = importlib.util.module_from_spec(spec)
sys.modules[spec.name] = module
spec.loader.exec_module(module)

summary = module.collect_instance_summary(
    Path({runs_root}),
    [module.TrialSpec("t1", ["r-base-denovo-12-t1"], [])],
)
print(json.dumps(summary, sort_keys=True))
"#,
        script_path = denovo_overlay_instances_path_json(),
        runs_root = serde_json::to_string(&runs_root.to_string_lossy())
            .expect("runs root should encode as json"),
    );
    let output = run_python_json(&script);

    let negative = output["negative_scored_deltas"]
        .as_array()
        .expect("negative deltas should be an array");
    assert_eq!(negative.len(), 1);
    assert_eq!(negative[0]["instance_id"], "negative");
    assert_eq!(negative[0]["score_delta_on_minus_off"], -0.5);

    fs::remove_dir_all(temp_dir).expect("temp dir should clean up");
}

#[test]
fn denovo_codex_agent_scopes_nested_codex_home_by_condition_output() {
    let temp_dir = target_temp_dir("stateful-bench-denovo-codex-nested-condition-profiles");
    let script = format!(
        r#"
import importlib.util
import json
import sys
from pathlib import Path

spec = importlib.util.spec_from_file_location("denovo_codex_agent_nested_condition_profiles_test", {agent_path})
module = importlib.util.module_from_spec(spec)
sys.modules[spec.name] = module
spec.loader.exec_module(module)

root = Path({temp_dir})
source_env = {{
    "PATH": "/bin",
    "STATEFUL_NESTED_CODEX_HOME_ROOT": str(root / "nested-codex-homes"),
}}
task_path = root / "extracts" / "results.jsonl"
workspace_off = root / "runs" / "run-a" / "conditions" / "stateful-off_subagent-on" / "codex-cli" / "instances" / "owner-repo-pr1" / "workspace"
workspace_on = root / "runs" / "run-a" / "conditions" / "stateful-on_subagent-on" / "codex-cli" / "instances" / "owner-repo-pr1" / "workspace"
output_off = root / "runs" / "run-a" / "conditions" / "stateful-off_subagent-on" / "codex-cli"
output_on = root / "runs" / "run-a" / "conditions" / "stateful-on_subagent-on" / "codex-cli"

env_off = module.denovo_codex_environment(
    output=output_off,
    instance_id="owner/repo#1",
    task_path=task_path,
    workspace=workspace_off,
    base_env=source_env,
)
env_on = module.denovo_codex_environment(
    output=output_on,
    instance_id="owner/repo#1",
    task_path=task_path,
    workspace=workspace_on,
    base_env=source_env,
)

print(json.dumps({{
    "off_home": env_off["CODEX_HOME"],
    "on_home": env_on["CODEX_HOME"],
}}, sort_keys=True))
"#,
        agent_path = denovo_codex_agent_path_json(),
        temp_dir = serde_json::to_string(&temp_dir.to_string_lossy())
            .expect("temp dir should encode as json"),
    );
    let output = run_python_json(&script);

    let off_home = output["off_home"].as_str().expect("off home");
    let on_home = output["on_home"].as_str().expect("on home");
    assert_ne!(off_home, on_home);
    assert!(off_home.contains("stateful-off_subagent-on"));
    assert!(on_home.contains("stateful-on_subagent-on"));

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
    "STATEFUL_HOME": str(home),
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
    repos = Path(env["STATEFUL_HOME"]) / "repos"
    repos.mkdir(parents=True, exist_ok=True)
    (repos / "repo-test.json").write_text(json.dumps({{
        "repo_id": "repo-test",
        "root": str(workspace),
        "enabled": True,
        "policy_config_path": str(workspace / ".stateful" / "config.yml"),
    }}))
    (Path(env["STATEFUL_HOME"]) / "config.yml").write_text(
        "repos:\n"
        "- repo_id: repo-test\n"
        f"  root: {{workspace}}\n"
        "  enabled: true\n"
        f"  policy_config_path: {{workspace / '.stateful' / 'config.yml'}}\n"
    )
    return Completed()

module.enable_stateful_repo(
    env=env,
    workspace=workspace,
    stateful_binary="/tmp/stateful",
    runner=fake_runner,
    runtime_workspace="/workspace",
)
repo_metadata = json.loads((home / "repos" / "repo-test.json").read_text())
registry_config = (home / "config.yml").read_text()
print(json.dumps({{
    "calls": calls,
    "workspace": str(workspace),
    "home": str(home),
    "repo_metadata": repo_metadata,
    "registry_config": registry_config,
}}, sort_keys=True))
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

    assert_eq!(output["repo_metadata"]["root"], "/workspace");
    assert_eq!(
        output["repo_metadata"]["policy_config_path"],
        "/workspace/.stateful/config.yml"
    );
    let registry_config = output["registry_config"]
        .as_str()
        .expect("registry config should be text");
    assert!(
        registry_config.contains("root: /workspace"),
        "Docker registry config should use container workspace: {registry_config}"
    );
    assert!(
        registry_config.contains("policy_config_path: /workspace/.stateful/config.yml"),
        "Docker registry config should use container policy path: {registry_config}"
    );
    assert!(
        !registry_config.contains(
            output["workspace"]
                .as_str()
                .expect("workspace should be text")
        ),
        "Docker registry config must not keep host-only paths: {registry_config}"
    );
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
fn denovo_codex_agent_classifies_missing_runtime_images() {
    let script = format!(
        r#"
import importlib.util
import json
import sys

spec = importlib.util.spec_from_file_location("denovo_codex_agent_missing_image_test", {agent_path})
module = importlib.util.module_from_spec(spec)
sys.modules[spec.name] = module
spec.loader.exec_module(module)

class ReprError(Exception):
    def __init__(self, repr_text):
        self.repr_text = repr_text
    def __repr__(self):
        return self.repr_text

local_missing = ReprError("ImageNotFound(HTTPError('404 Client Error: Not Found for url: http+docker://localhost/v1.53/images/aweaiteam/denovoswe:case-a/json'))")
remote_missing = ReprError("NotFound(HTTPError('404 Client Error: Not Found for url: http+docker://localhost/v1.53/images/create?tag=case-b&fromImage=aweaiteam%2Fdenovoswe'))")
other = RuntimeError("unsafe archive link")

print(json.dumps({{
    "local": module.instance_result_row(module.instance_setup_exception_result("case-a", local_missing)),
    "remote": module.instance_result_row(module.instance_setup_exception_result("case-b", remote_missing)),
    "other": module.instance_result_row(module.instance_setup_exception_result("case-c", other)),
}}, sort_keys=True))
"#,
        agent_path = denovo_codex_agent_path_json(),
    );
    let output = run_python_json(&script);

    assert_eq!(output["local"]["finish_reason"], "missing-runtime-image");
    assert!(output["local"]["error"]
        .as_str()
        .expect("local error should be text")
        .contains("aweaiteam/denovoswe:case-a"));
    assert_eq!(output["remote"]["finish_reason"], "missing-runtime-image");
    assert!(output["remote"]["error"]
        .as_str()
        .expect("remote error should be text")
        .contains("aweaiteam/denovoswe:case-b"));
    assert_eq!(output["other"]["finish_reason"], "adapter-error");
}

#[test]
fn denovo_codex_agent_manages_runtime_image_once_per_instance() {
    let script = format!(
        r#"
import asyncio
import importlib.util
import json
import sys
from argparse import Namespace

spec = importlib.util.spec_from_file_location("denovo_codex_agent_image_lifecycle_test", {agent_path})
module = importlib.util.module_from_spec(spec)
sys.modules[spec.name] = module
spec.loader.exec_module(module)

class FakeDockerConfig:
    def __init__(self, pull_policy):
        self.pull_policy = pull_policy

    def model_copy(self, update):
        copied = FakeDockerConfig(self.pull_policy)
        for key, value in update.items():
            setattr(copied, key, value)
        return copied

class FakeRuntimeConfig:
    def __init__(self, backend="docker", pull_policy="if_not_present"):
        self.backend = backend
        self.docker = FakeDockerConfig(pull_policy)
        self.image = ""
        self.workdir = ""

    def model_copy(self, update):
        copied = FakeRuntimeConfig(self.backend, self.docker.pull_policy)
        copied.image = self.image
        copied.workdir = self.workdir
        copied.docker = self.docker
        for key, value in update.items():
            setattr(copied, key, value)
        return copied

class FakeImages:
    def __init__(self):
        self.calls = []

    def get(self, image):
        self.calls.append(["get", image])
        raise RuntimeError("missing")

    def pull(self, image):
        self.calls.append(["pull", image])

    def remove(self, image, force=False):
        self.calls.append(["remove", image, force])

class FakeClient:
    def __init__(self):
        self.images = FakeImages()

class FakeEvaluator:
    def __init__(self, **kwargs):
        self.kwargs = kwargs

async def main():
    client = FakeClient()
    base = FakeRuntimeConfig()
    await module.ensure_runtime_image_available(
        base,
        "aweaiteam/denovoswe:case-a",
        client_factory=lambda: client,
    )
    local = module.runtime_config_for_local_image(base, "aweaiteam/denovoswe:case-a", "/workspace/case-a")
    evaluator = module.build_denovo_evaluator(
        FakeEvaluator,
        Namespace(validate_run=False, del_done_images=True, eval_iters=3),
        Namespace(eval=Namespace(timeout=123)),
    )
    await module.delete_runtime_image_after_instance(
        base,
        "aweaiteam/denovoswe:case-a",
        enabled=True,
        client_factory=lambda: client,
    )
    print(json.dumps({{
        "calls": client.images.calls,
        "base_pull_policy": base.docker.pull_policy,
        "local_pull_policy": local.docker.pull_policy,
        "local_image": local.image,
        "local_workdir": local.workdir,
        "evaluator_del_done_images": evaluator.kwargs["del_done_images"],
        "evaluator_eval_iters": evaluator.kwargs["eval_iters"],
    }}, sort_keys=True))

asyncio.run(main())
"#,
        agent_path = denovo_codex_agent_path_json(),
    );
    let output = run_python_json(&script);

    assert_eq!(
        output["calls"],
        serde_json::json!([
            ["get", "aweaiteam/denovoswe:case-a"],
            ["pull", "aweaiteam/denovoswe:case-a"],
            ["remove", "aweaiteam/denovoswe:case-a", true],
        ])
    );
    assert_eq!(output["base_pull_policy"], "if_not_present");
    assert_eq!(output["local_pull_policy"], "never");
    assert_eq!(output["local_image"], "aweaiteam/denovoswe:case-a");
    assert_eq!(output["local_workdir"], "/workspace/case-a");
    assert_eq!(output["evaluator_del_done_images"], false);
    assert_eq!(output["evaluator_eval_iters"], 3);
}

#[test]
fn denovo_codex_agent_preflights_missing_runtime_images_without_pulling() {
    let script = format!(
        r#"
import asyncio
import importlib.util
import json
import sys

spec = importlib.util.spec_from_file_location("denovo_codex_agent_image_preflight_test", {agent_path})
module = importlib.util.module_from_spec(spec)
sys.modules[spec.name] = module
spec.loader.exec_module(module)

class FakeDockerConfig:
    def __init__(self, pull_policy):
        self.pull_policy = pull_policy

class FakeRuntimeConfig:
    def __init__(self, backend="docker", pull_policy="if_not_present"):
        self.backend = backend
        self.docker = FakeDockerConfig(pull_policy)

class NotFoundError(Exception):
    def __repr__(self):
        return "NotFound(HTTPError('404 Client Error: Not Found for url: https://registry-1.docker.io/v2/aweaiteam/denovoswe/manifests/case-missing'))"

class FakeImages:
    def __init__(self):
        self.calls = []

    def get(self, image):
        self.calls.append(["get", image])
        raise NotFoundError()

    def get_registry_data(self, image):
        self.calls.append(["get_registry_data", image])
        raise NotFoundError()

    def pull(self, image):
        self.calls.append(["pull", image])

class FakeClient:
    def __init__(self):
        self.images = FakeImages()

async def main():
    client = FakeClient()
    try:
        await module.preflight_runtime_image_available(
            FakeRuntimeConfig(),
            "aweaiteam/denovoswe:case-missing",
            client_factory=lambda: client,
        )
        result = {{"raised": False}}
    except Exception as error:
        row = module.instance_result_row(
            module.instance_setup_exception_result("case-missing", error)
        )
        result = {{
            "raised": True,
            "row": row,
            "calls": client.images.calls,
        }}
    print(json.dumps(result, sort_keys=True))

asyncio.run(main())
"#,
        agent_path = denovo_codex_agent_path_json(),
    );
    let output = run_python_json(&script);

    assert_eq!(output["raised"], true);
    assert_eq!(output["row"]["finish_reason"], "missing-runtime-image");
    assert!(output["row"]["error"]
        .as_str()
        .expect("error should be text")
        .contains("aweaiteam/denovoswe:case-missing"));
    assert_eq!(
        output["calls"],
        serde_json::json!([
            ["get", "aweaiteam/denovoswe:case-missing"],
            ["get_registry_data", "aweaiteam/denovoswe:case-missing"],
        ])
    );
}

#[test]
fn denovo_codex_agent_harvests_repo_tmp_changes() {
    let temp_dir = target_temp_dir("stateful-bench-denovo-harvest-repo-tmp");
    let workspace = temp_dir.join("workspace");
    fs::create_dir_all(workspace.join("tmp")).expect("tmp dir should be created");
    fs::write(workspace.join("tmp/kept.txt"), "kept\n").expect("tmp file should write");
    ProcessCommand::new("git")
        .args(["init"])
        .current_dir(&workspace)
        .output()
        .expect("git init should run");
    fs::write(workspace.join("README.md"), "base\n").expect("base file should write");
    ProcessCommand::new("git")
        .args(["add", "README.md"])
        .current_dir(&workspace)
        .output()
        .expect("git add should run");
    ProcessCommand::new("git")
        .args([
            "-c",
            "user.email=test@example.invalid",
            "-c",
            "user.name=Stateful Test",
            "commit",
            "-m",
            "base",
        ])
        .current_dir(&workspace)
        .output()
        .expect("git commit should run");

    let script = format!(
        r#"
import importlib.util
import json
import sys
from pathlib import Path

spec = importlib.util.spec_from_file_location("denovo_codex_agent_harvest_tmp_test", {agent_path})
module = importlib.util.module_from_spec(spec)
sys.modules[spec.name] = module
spec.loader.exec_module(module)

patch = module.git_diff(Path({workspace}))
print(json.dumps({{"contains_tmp": "tmp/kept.txt" in patch}}, sort_keys=True))
"#,
        agent_path = denovo_codex_agent_path_json(),
        workspace = serde_json::to_string(&workspace).expect("workspace path should encode"),
    );
    let output = run_python_json(&script);

    assert_eq!(output["contains_tmp"], true);

    fs::remove_dir_all(temp_dir).expect("temp dir should be removable");
}

#[test]
fn denovo_codex_agent_cleans_nested_home_caches_without_removing_codex_logs() {
    let temp_dir = target_temp_dir("stateful-bench-denovo-codex-cache-cleanup");
    let script = format!(
        r#"
import importlib.util
import json
import sys
from pathlib import Path

spec = importlib.util.spec_from_file_location("denovo_codex_agent_cache_cleanup_test", {agent_path})
module = importlib.util.module_from_spec(spec)
sys.modules[spec.name] = module
spec.loader.exec_module(module)

home = Path({home})
cache = home / ".cache"
library_cache = home / "Library" / "Caches"
codex_home = home / ".codex"
cache.mkdir(parents=True)
library_cache.mkdir(parents=True)
codex_home.mkdir(parents=True)
(cache / "blob").write_text("cache", encoding="utf-8")
(library_cache / "blob").write_text("cache", encoding="utf-8")
(codex_home / "session.jsonl").write_text("log", encoding="utf-8")

removed = module.cleanup_codex_home_caches({{
    "HOME": str(home),
    "XDG_CACHE_HOME": str(cache),
    "CODEX_HOME": str(codex_home),
}})
print(json.dumps({{
    "removed": sorted(Path(path).name for path in removed),
    "cache_exists": cache.exists(),
    "library_cache_exists": library_cache.exists(),
    "codex_log_exists": (codex_home / "session.jsonl").exists(),
}}, sort_keys=True))
"#,
        agent_path = denovo_codex_agent_path_json(),
        home = serde_json::to_string(&temp_dir.join("home").to_string_lossy())
            .expect("home path should encode as json"),
    );
    let output = run_python_json(&script);

    assert_eq!(output["removed"], serde_json::json!([".cache", "Caches"]));
    assert_eq!(output["cache_exists"], false);
    assert_eq!(output["library_cache_exists"], false);
    assert_eq!(output["codex_log_exists"], true);

    fs::remove_dir_all(temp_dir).expect("temp dir should clean up");
}

#[test]
fn denovo_codex_agent_reports_low_disk_space_before_starting_instance() {
    let script = format!(
        r#"
import importlib.util
import json
import sys
from pathlib import Path
from types import SimpleNamespace

spec = importlib.util.spec_from_file_location("denovo_codex_agent_low_disk_test", {agent_path})
module = importlib.util.module_from_spec(spec)
sys.modules[spec.name] = module
spec.loader.exec_module(module)

result = module.low_disk_space_result(
    "case-a",
    Path("/tmp/output"),
    min_free_bytes=100,
    disk_usage=lambda path: SimpleNamespace(free=40),
)
print(json.dumps(module.instance_result_row(result), sort_keys=True))
"#,
        agent_path = denovo_codex_agent_path_json(),
    );
    let output = run_python_json(&script);

    assert_eq!(output["instance_id"], "case-a");
    assert_eq!(output["success"], false);
    assert_eq!(output["finish_reason"], "disk-space-low");
    assert!(output["error"]
        .as_str()
        .expect("error should be text")
        .contains("free disk space 40 bytes is below required 100 bytes"));
}

#[test]
fn denovo_codex_agent_appends_intermediate_result_rows() {
    let temp_dir = std::env::temp_dir().join(format!(
        "stateful-bench-denovo-codex-intermediate-results-{}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&temp_dir);
    let script = format!(
        r#"
import importlib.util
import json
import sys
from pathlib import Path

spec = importlib.util.spec_from_file_location("denovo_codex_agent_result_append_test", {agent_path})
module = importlib.util.module_from_spec(spec)
sys.modules[spec.name] = module
spec.loader.exec_module(module)

results_path = Path({results_path})
first = module.InstanceResult(
    instance_id="case-a",
    success=False,
    score=None,
    finish_reason="adapter-error",
    error="setup failed",
    eval_result=None,
)
second = module.InstanceResult(
    instance_id="case-b",
    success=True,
    score=1.0,
    finish_reason="stop",
    error=None,
    eval_result={{"details": {{"pass_rate": 1.0}}}},
)
module.append_result_jsonl(results_path, first)
module.append_result_jsonl(results_path, second)
rows = [
    json.loads(line)
    for line in results_path.read_text(encoding="utf-8").splitlines()
    if line.strip()
]
print(json.dumps({{"rows": rows}}, sort_keys=True))
"#,
        agent_path = denovo_codex_agent_path_json(),
        results_path = serde_json::to_string(
            &temp_dir
                .join("codex-cli")
                .join("_")
                .join("results.jsonl")
                .to_string_lossy()
        )
        .expect("results path should encode as json"),
    );
    let output = run_python_json(&script);

    let rows = output["rows"].as_array().expect("rows should be an array");
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0]["instance_id"], "case-a");
    assert_eq!(rows[0]["error"], "setup failed");
    assert_eq!(rows[1]["instance_id"], "case-b");
    assert_eq!(rows[1]["success"], true);

    fs::remove_dir_all(temp_dir).expect("temp dir should clean up");
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
    assert!(output.get("host_worker_count").is_none());
    assert_eq!(output["subagent_mode"], "native_codex_subagents");
    assert_eq!(output["native_subagent_required"], true);
    assert_eq!(output["eval_feedback_loop"], false);
    assert_eq!(output["eval_feedback_attempts"], 0);
    assert_eq!(output["resume_policy"], "context_or_token_failure_only");
    assert_eq!(output["subagent_required"], true);
    assert_eq!(output["stateful_mcp"], true);
}

#[test]
fn denovo_codex_agent_metadata_requires_omp_subagent_axis() {
    let script = format!(
        r#"
import importlib.util
import json
import sys

spec = importlib.util.spec_from_file_location("denovo_omp_agent_protocol_test", {agent_path})
module = importlib.util.module_from_spec(spec)
sys.modules[spec.name] = module
spec.loader.exec_module(module)

print(json.dumps(module.profile_metadata("stateful", "on", cli_runtime="omp"), sort_keys=True))
"#,
        agent_path = denovo_codex_agent_path_json(),
    );
    let output = run_python_json(&script);

    assert_eq!(output["agent_kind"], "omp-cli");
    assert_eq!(output["subagent"], "on");
    assert_eq!(output["subagent_mode"], "native_omp_subagents");
    assert_eq!(output["native_subagent_required"], true);
    assert_eq!(output["subagent_required"], true);
}

#[test]
fn denovo_codex_agent_prompt_requires_native_subagents_when_subagent_on() {
    let script = format!(
        r#"
import importlib.util
import json
import sys

spec = importlib.util.spec_from_file_location("denovo_codex_agent_prompt_test", {agent_path})
module = importlib.util.module_from_spec(spec)
sys.modules[spec.name] = module
spec.loader.exec_module(module)

off = module.build_codex_prompt(
    instance_id="i1",
    document="doc",
    benchmark_max_turns=500,
    max_steps=None,
    prompt_version="v1",
    subagent="off",
)
on = module.build_codex_prompt(
    instance_id="i1",
    document="doc",
    benchmark_max_turns=500,
    max_steps=None,
    prompt_version="v1",
    subagent="on",
    subagent_min_count=3,
)
print(json.dumps({{"off": off, "on": on}}, sort_keys=True))
"#,
        agent_path = denovo_codex_agent_path_json(),
    );
    let output = run_python_json(&script);

    let off = output["off"].as_str().expect("off prompt");
    let on = output["on"].as_str().expect("on prompt");
    assert!(!off.contains("Native Codex/OMP subagent requirements"));
    assert!(on.contains("Native Codex/OMP subagent requirements"));
    assert!(on.contains("MUST use native subagents"));
    assert!(on.contains("Before implementation or broad repository exploration"));
    assert!(on.contains("after any narrow setup needed"));
    assert!(on.contains("dispatching-parallel-agents"));
    assert!(!on.contains("FIRST ACTION"));
    assert!(on.contains("the current native subagent tool is `task`"));
    assert!(on.contains("tasks` array containing at least 3 implementation subagents"));
    assert!(on.contains("multi_agent_v1spawn_agent"));
    assert!(on.contains("Use all 3 native subagents for repository editing"));
    assert!(off.contains("Benchmark isolation requirements"));
    assert!(on.contains("Benchmark isolation requirements"));
    assert!(on.contains("Do not fetch, clone, open, or inspect the upstream repository"));
    assert!(on.contains("Do not create or use an `upstream` checkout"));
    assert!(
        on.find("Native Codex/OMP subagent requirements").unwrap()
            < on.find("Repository specification:").unwrap()
    );
    assert!(on.contains("Do not leave any native subagent as analysis-only"));
    assert!(on.contains("Wait for each spawned subagent"));
    assert!(on.contains("explicitly report that blocker"));
    assert_ne!(off, on);
}

#[test]
fn denovo_codex_agent_detects_upstream_source_access_in_session_artifacts() {
    let dir = target_temp_dir("denovo-codex-detects-source-leak");
    let script = format!(
        r#"
import importlib.util
import json
import sys
from pathlib import Path

spec = importlib.util.spec_from_file_location("denovo_codex_agent_contamination_test", {agent_path})
module = importlib.util.module_from_spec(spec)
sys.modules[spec.name] = module
spec.loader.exec_module(module)

root = Path({root})
workspace = root / "workspace"
workspace.mkdir(parents=True)
codex_home = root / "home" / ".omp" / "profiles" / "stateful" / "agent"
session_dir = codex_home / "sessions" / "--workspace--"
session_dir.mkdir(parents=True)
(session_dir / "rollout.jsonl").write_text(
    json.dumps({{"type": "message", "text": "read https://github.com/thebjorn/pydeps/pull/233/files"}}) + "\n",
    encoding="utf-8",
)
clean_home = root / "clean-home"
clean_home.mkdir(parents=True)
(workspace / "upstream").mkdir()
upstream = module.benchmark_contamination_record("thebjorn_pydeps_pr233", workspace, clean_home)
(workspace / "upstream").rmdir()
source = module.benchmark_contamination_record("thebjorn_pydeps_pr233", workspace, codex_home)
clean = module.benchmark_contamination_record("thebjorn_pydeps_pr233", workspace, clean_home)
print(json.dumps({{"upstream": upstream, "source": source, "clean": clean}}, sort_keys=True))
"#,
        agent_path = denovo_codex_agent_path_json(),
        root = serde_json::to_string(&dir).expect("root should serialize"),
    );
    let output = run_python_json(&script);

    assert_eq!(output["upstream"]["kind"], "upstream-worktree");
    assert_eq!(output["source"]["kind"], "upstream-source-access");
    assert_eq!(output["source"]["pattern"], "github.com/thebjorn/pydeps");
    assert_eq!(output["clean"], serde_json::Value::Null);

    fs::remove_dir_all(dir).expect("temp dir should clean up");
}

#[test]
fn denovo_codex_agent_detects_native_subagent_usage_from_codex_home() {
    let script = format!(
        r#"
import importlib.util
import json
import sqlite3
import sys
import tempfile
from pathlib import Path

spec = importlib.util.spec_from_file_location("denovo_codex_agent_subagent_usage_test", {agent_path})
module = importlib.util.module_from_spec(spec)
sys.modules[spec.name] = module
spec.loader.exec_module(module)

with tempfile.TemporaryDirectory() as tmp:
    codex_home = Path(tmp) / ".codex"
    session_dir = codex_home / "sessions" / "2026" / "06" / "14"
    session_dir.mkdir(parents=True)
    (session_dir / "rollout.jsonl").write_text(
        json.dumps({{"type": "response_item", "payload": {{"type": "function_call", "name": "multi_agent_v1spawn_agent"}}}}) + "\n"
        + json.dumps({{"type": "response_item", "payload": {{"type": "function_call", "name": "wait_agent"}}}}) + "\n",
        encoding="utf-8",
    )
    omp_home = Path(tmp) / "omp-agent"
    omp_session_dir = omp_home / "sessions" / "--workspace--"
    omp_session_dir.mkdir(parents=True)
    (omp_session_dir / "session.jsonl").write_text(
        json.dumps({{"type": "message", "message": {{"role": "assistant", "content": [
            {{"type": "toolCall", "name": "task", "arguments": {{"tasks": [
                {{"assignment": "one"}},
                {{"assignment": "two"}},
                {{"assignment": "three"}},
            ]}}}}
        ]}}}}) + "\n",
        encoding="utf-8",
    )
    db = sqlite3.connect(codex_home / "state_5.sqlite")
    db.execute("create table agent_jobs(id integer primary key)")
    db.execute("insert into agent_jobs(id) values (1)")
    db.execute("create table agent_job_items(id integer primary key)")
    db.execute("insert into agent_job_items(id) values (1)")
    db.execute("create table thread_spawn_edges(id integer primary key)")
    db.execute("insert into thread_spawn_edges(id) values (1)")
    db.execute("create table thread_dynamic_tools(id integer primary key)")
    db.execute("insert into thread_dynamic_tools(id) values (1)")
    db.commit()
    db.close()

    used = module.detect_native_subagent_usage(codex_home)
    omp_usage = module.native_subagent_usage("on", 3, omp_home, cli_runtime="omp")
    empty = module.detect_native_subagent_usage(Path(tmp) / "empty" / ".codex")

print(json.dumps({{"used": used, "omp_usage": omp_usage, "empty": empty}}, sort_keys=True))
"#,
        agent_path = denovo_codex_agent_path_json(),
    );
    let output = run_python_json(&script);

    assert_eq!(output["used"]["subagent_used"], true);
    assert_eq!(output["used"]["counts"]["spawn_agent_calls"], 1);
    assert_eq!(output["used"]["counts"]["wait_agent_calls"], 1);
    assert_eq!(output["used"]["counts"]["agent_jobs"], 1);
    assert_eq!(output["used"]["counts"]["thread_spawn_edges"], 1);
    assert_eq!(output["omp_usage"]["mode"], "native_omp_subagents");
    assert_eq!(
        output["omp_usage"]["native_subagent"]["subagent_spawn_count"],
        3
    );
    assert_eq!(
        output["omp_usage"]["native_subagent"]["counts"]["spawn_agent_calls"],
        3
    );
    assert_eq!(output["omp_usage"]["subagent_requirement_met"], true);
    assert_eq!(output["empty"]["subagent_used"], false);
}

#[test]
fn denovo_codex_agent_result_row_includes_orchestration_trace_metadata() {
    let script = format!(
        r#"
import importlib.util
import json
import sys

spec = importlib.util.spec_from_file_location("denovo_codex_agent_trace_test", {agent_path})
module = importlib.util.module_from_spec(spec)
sys.modules[spec.name] = module
spec.loader.exec_module(module)

result = module.InstanceResult(
    instance_id="fake-a",
    success=True,
    score=1.0,
    finish_reason="skip-eval",
    error=None,
    eval_result={{"details": {{"pass_rate": 1.0}}}},
    orchestration_trace={{
        "trace_path": "fake-a/orchestration-trace.json",
        "trace_captured": True,
        "reservation_events": 2,
        "claim_events": 1,
        "conflict_events": 0,
    }},
)
print(json.dumps(module.instance_result_row(result), sort_keys=True))
"#,
        agent_path = denovo_codex_agent_path_json(),
    );
    let output = run_python_json(&script);

    assert_eq!(
        output["orchestration_trace"]["trace_path"],
        "fake-a/orchestration-trace.json"
    );
    assert_eq!(output["orchestration_trace"]["trace_captured"], true);
    assert_eq!(output["orchestration_trace"]["reservation_events"], 2);
    assert_eq!(output["orchestration_trace"]["claim_events"], 1);
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
fn denovo_codex_agent_deletes_done_images_by_default() {
    let script = format!(
        r#"
import importlib.util
import json
import sys

spec = importlib.util.spec_from_file_location("denovo_codex_agent_parse_defaults_test", {agent_path})
module = importlib.util.module_from_spec(spec)
sys.modules[spec.name] = module
spec.loader.exec_module(module)

base = [
    "--data-file", "data.jsonl",
    "--config", "configs/tasks/denovoswe.yaml",
    "--mode", "batch",
    "--output", "out",
    "--agent-mode", "stateful",
    "--subagent", "on",
    "--aweagent-root", "AweAgent",
    "--codex-bin", "codex",
    "--stateful-binary", "stateful",
    "--benchmark-model", "gpt-5.4-mini",
    "--benchmark-reasoning-effort", "low",
    "--benchmark-model-context-window", "256000",
    "--benchmark-temperature", "1",
    "--benchmark-max-turns", "500",
    "--max-resumes", "1",
    "--codex-timeout-seconds", "7200",
    "--eval-iters", "1",
    "--prompt-version", "v1",
]

default_args = module.parse_args(base)
keep_args = module.parse_args(base + ["--keep-done-images"])
print(json.dumps({{
    "default": default_args.del_done_images,
    "keep": keep_args.del_done_images,
}}, sort_keys=True))
"#,
        agent_path = denovo_codex_agent_path_json(),
    );
    let output = run_python_json(&script);

    assert_eq!(output["default"], true);
    assert_eq!(output["keep"], false);
}

#[test]
fn denovo_codex_agent_accepts_max_concurrent_over_one() {
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

print(json.dumps({{
    "default": module.max_concurrent_limit(Namespace(max_concurrent=None)),
    "zero": module.max_concurrent_limit(Namespace(max_concurrent=0)),
    "six": module.max_concurrent_limit(Namespace(max_concurrent=6)),
}}, sort_keys=True))
"#,
        agent_path = denovo_codex_agent_path_json(),
    );
    let output = run_python_json(&script);

    assert_eq!(output["default"], 1);
    assert_eq!(output["zero"], 1);
    assert_eq!(output["six"], 6);
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
fn codex_pair_agent_prompt_requires_native_subagents_when_enabled() {
    let script = format!(
        r#"
import importlib.util
import json

spec = importlib.util.spec_from_file_location("codex_pair_agent_prompt_test", {agent_path})
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)

print(json.dumps({{
    "off": module.native_subagent_prompt_instruction(False),
    "on": module.native_subagent_prompt_instruction(True, 3),
}}, sort_keys=True))
"#,
        agent_path = codex_pair_agent_path_json(),
    );
    let output = run_python_json(&script);

    let off = output["off"].as_str().expect("off prompt");
    let on = output["on"].as_str().expect("on prompt");
    assert_eq!(off, "");
    assert!(on.contains("Native Codex subagent requirements"));
    assert!(on.contains("MUST use native Codex subagents"));
    assert!(on.contains("Spawn at least 3 native subagents"));
    assert!(on.contains("dispatching-parallel-agents"));
    assert!(on.contains("Use all 3 native subagents for repository editing"));
    assert!(on.contains("Do not leave any native subagent as analysis-only"));
    assert!(on.contains("Wait for each spawned subagent"));
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

    let home = output["HOME"].as_str().expect("HOME should be string");
    assert!(home.starts_with("/repo/target/nested-codex-homes/pair-one/task-a/"));
    assert!(home.ends_with("/home"));
    assert_eq!(output["CODEX_HOME"], format!("{home}/.codex"));
    assert_eq!(output["XDG_CONFIG_HOME"], format!("{home}/.config"));
    assert_eq!(output["XDG_CACHE_HOME"], format!("{home}/.cache"));
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
source_home = root / "source-home"
source_config_toml = source_home / ".codex" / "config.toml"
source_config_toml.parent.mkdir(parents=True, exist_ok=True)
source_config_toml.write_text('''model_provider = "codex-lb"

[model_providers.codex-lb]
base_url = "http://127.0.0.1:2455/backend-api/codex"
wire_api = "responses"
websocket = true
websocker = true

[features]
goals = true
websocket = true
websocker = true

[mcp_servers.stateful]
command = "stale-stateful"
''')
workspace = root / "runs" / "pair-one" / "workspace"
task_path = workspace / ".stateful_bench" / "task-a.json"
source_env = {{
    "PATH": "/bin",
    "HOME": str(source_home),
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
    "feature_table_count": config.count("[features]"),
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
    assert!(config.contains("model_provider = \"codex-lb\""));
    assert!(config.contains("[model_providers.codex-lb]"));
    assert!(config.contains("base_url = \"http://127.0.0.1:2455/backend-api/codex\""));
    assert!(config.contains("wire_api = \"responses\""));
    assert_eq!(output["feature_table_count"], 1);
    assert!(!config.contains("goals = true"));
    assert!(!config.contains("websocket = true"));
    assert!(!config.contains("websocker = true"));
    assert!(!config.contains("stale-stateful"));
    assert!(config.contains("[mcp_servers.stateful]"));
    assert!(config.contains("command = \"/tmp/stateful\""));
    assert!(config.contains("args = [\"mcp\", \"serve\"]"));
    assert!(config.contains(
        "env_vars = [\"CODEX_THREAD_ID\", \"STATEFUL_CODEX_RUN_ID\", \"STATEFUL_SERVER_URL\", \"STATEFUL_SERVER_TOKEN\", \"STATEFUL_SESSION_ID\"]"
    ));
    assert!(config.contains("[[hooks.SessionStart]]"));
    assert!(config.contains("[[hooks.PreToolUse]]"));
    assert!(config.contains("[[hooks.Stop]]"));

    let skill = output["skill"].as_str().expect("skill should be text");
    assert!(skill.contains("name: stateful-command-policy"));
    assert!(skill.contains("Use canonical Stateful MCP tool names"));
    assert!(skill.contains("state_reservation_declare"));
    assert!(skill.contains("state_claim_acquire"));
    assert!(skill.contains("runtime-specific tool names"));
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
    assert!(config.contains("/tmp/stateful hook codex session-start"));
    assert!(config.contains("/tmp/stateful hook codex pre-tool-use"));
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
fn codex_pair_agent_source_env_sets_stateful_session_id_for_stateful_mode() {
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
stateful_with_native_subagent = module.benchmark_source_env(
    mode="stateful",
    session_id="pair-one-agent-a",
    base_env={{"PATH": "/bin", "STATEFUL_SESSION_ID": "outer-session"}},
    preserve_stateful_session=False,
)
no_state = module.benchmark_source_env(
    mode="no-state",
    session_id=None,
    base_env={{"PATH": "/bin"}},
)
print(json.dumps({{
    "stateful": stateful,
    "stateful_with_native_subagent": stateful_with_native_subagent,
    "no_state": no_state,
}}, sort_keys=True))
"#,
        agent_path = codex_pair_agent_path_json(),
    );
    let output = run_python_json(&script);

    assert_eq!(output["stateful"]["PATH"], "/bin");
    assert_eq!(
        output["stateful"]
            .as_object()
            .expect("stateful env should be an object")
            .get("STATEFUL_SESSION_ID"),
        Some(&serde_json::Value::String("pair-one-agent-a".to_string()))
    );
    assert_eq!(
        output["stateful_with_native_subagent"]
            .as_object()
            .expect("native subagent env should be an object")
            .contains_key("STATEFUL_SESSION_ID"),
        false
    );
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
observed = []

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
        result_observer=observed.append,
    )
finally:
    sys.stdout = original_stdout
    sys.stderr = original_stderr

print(json.dumps({{
    "code": code,
    "calls": calls,
    "observed": [
        {{
            "returncode": result.returncode,
            "session_id": result.session_id,
            "resumeable_token_failure": result.resumeable_token_failure,
        }}
        for result in observed
    ],
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
    let observed = output["observed"]
        .as_array()
        .expect("observed results should be an array");
    assert_eq!(observed.len(), 2);
    assert_eq!(observed[0]["session_id"], "session-123");
    assert_eq!(observed[0]["resumeable_token_failure"], true);
    assert_eq!(observed[1]["returncode"], 0);
    assert_eq!(calls[0]["input"], "initial prompt");
    assert!(calls[1]["input"]
        .as_str()
        .expect("resume prompt should be text")
        .contains("Continue the same benchmark task"));
    let resume_command = calls[1]["command"]
        .as_array()
        .expect("resume command should be an array");
    assert!(command_contains(resume_command, "resume"));
    assert!(command_contains(resume_command, "session-123"));
    assert!(!command_contains(resume_command, "--cd"));
    assert!(!command_contains(resume_command, "--sandbox"));
    assert!(!output["stdout"]
        .as_str()
        .expect("stdout should be text")
        .contains("turn.failed"));
    assert!(output["stdout"]
        .as_str()
        .expect("stdout should be text")
        .contains("stateful_bench.resume"));
    assert!(output["stdout"]
        .as_str()
        .expect("stdout should be text")
        .contains("token_count"));
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
source_config = source_home / ".codex" / "config.json"
source_config.write_text("{{\"provider\":\"codex_lb\"}}")
source_config_toml = source_home / ".codex" / "config.toml"
source_config_toml.write_text('''model_provider = "codex-lb"

[model_providers.codex-lb]
base_url = "http://127.0.0.1:2455/backend-api/codex"

[mcp_servers.stateful]
command = "stale-stateful"

[features]
hooks = true

[[hooks.PreToolUse]]
matcher = ".*"
''')

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
target_config = Path(env["CODEX_HOME"]) / "config.json"
target_config_toml = Path(env["CODEX_HOME"]) / "config.toml"
copied = target_auth.read_text()
copied_config = target_config.read_text()
copied_config_toml = target_config_toml.read_text()
module.cleanup_seeded_auth(seeded)
print(json.dumps({{
    "copied": copied,
    "copied_config": copied_config,
    "copied_config_toml": copied_config_toml,
    "seeded": str(seeded.path if seeded else None),
    "target_exists_after_cleanup": target_auth.exists(),
    "target_config_exists_after_cleanup": target_config.exists(),
    "target_config_toml_exists_after_cleanup": target_config_toml.exists(),
    "source_exists_after_cleanup": source_auth.exists(),
    "source_config_exists_after_cleanup": source_config.exists(),
    "source_config_toml_exists_after_cleanup": source_config_toml.exists(),
}}))
"#,
        agent_path = codex_pair_agent_path_json(),
        temp_dir = serde_json::to_string(&temp_dir.to_string_lossy())
            .expect("temp dir should encode as json"),
    );
    let output = run_python_json(&script);

    assert_eq!(output["copied"], "{\"token\":\"source\"}");
    assert_eq!(output["copied_config"], "{\"provider\":\"codex_lb\"}");
    assert_eq!(
        output["copied_config_toml"],
        "model_provider = \"codex-lb\"\n\n[model_providers.codex-lb]\nbase_url = \"http://127.0.0.1:2455/backend-api/codex\"\n"
    );
    assert_eq!(output["target_exists_after_cleanup"], false);
    assert_eq!(output["target_config_exists_after_cleanup"], false);
    assert_eq!(output["target_config_toml_exists_after_cleanup"], false);
    assert_eq!(output["source_exists_after_cleanup"], true);
    assert_eq!(output["source_config_exists_after_cleanup"], true);
    assert_eq!(output["source_config_toml_exists_after_cleanup"], true);

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

fn denovo_progress_report_path_json() -> String {
    serde_json::to_string(&format!(
        "{}/scripts/denovo_progress_report.py",
        env!("CARGO_MANIFEST_DIR")
    ))
    .expect("path should serialize")
}

fn denovo_retry_overlay_report_path_json() -> String {
    serde_json::to_string(&format!(
        "{}/scripts/denovo_retry_overlay_report.py",
        env!("CARGO_MANIFEST_DIR")
    ))
    .expect("path should serialize")
}

fn denovo_overlay_instances_path_json() -> String {
    serde_json::to_string(&format!(
        "{}/scripts/denovo_overlay_instances.py",
        env!("CARGO_MANIFEST_DIR")
    ))
    .expect("path should serialize")
}

fn codex_synthetic_agent_path_json() -> String {
    serde_json::to_string(&format!(
        "{}/scripts/codex_synthetic_agent.py",
        env!("CARGO_MANIFEST_DIR")
    ))
    .expect("agent path should encode as json")
}

fn write_condition_metadata(
    condition_dir: &std::path::Path,
    official_dir: &std::path::Path,
    results_jsonl: &std::path::Path,
    report_json: &std::path::Path,
    condition_id: &str,
    stateful: bool,
) {
    let metadata = serde_json::json!({
        "run_id": "dev-denovo",
        "condition_id": condition_id,
        "condition": {"stateful": stateful, "subagent": true},
        "agent": "codex-cli",
        "command": {
            "program": "python3",
            "args": [],
            "cwd": official_dir,
            "env": {}
        },
        "official_dir": official_dir,
        "results_jsonl": results_jsonl,
        "report_json": report_json,
        "started_at_ms": 1,
        "finished_at_ms": 2,
        "running_time_ms": 1
    });
    fs::write(condition_dir.join("condition.json"), metadata.to_string())
        .expect("condition metadata should be written");
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
