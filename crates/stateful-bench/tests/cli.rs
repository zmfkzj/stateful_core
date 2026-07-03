use clap::Parser;
use stateful_bench::{
    Cli, Command, DeNovoAgentDockerSandbox, DeNovoAgentKind, DeNovoCommand, DeNovoMatrixRunOptions,
    DeNovoRunMode, ReportFormat, RunMode, parse_denovo_condition, run_denovo_matrix,
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
        "--agent-docker-sandbox",
        "off",
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
                agent_docker_sandbox,
                ref benchmark_model,
                ref benchmark_reasoning_effort,
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
            && agent_docker_sandbox == DeNovoAgentDockerSandbox::Off
            && benchmark_model.as_deref() == Some("deepseek-v4-flash")
            && benchmark_reasoning_effort == "high"
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
#[expect(
    clippy::useless_format,
    reason = "Python fixture keeps literal braces escaped"
)]
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
        agent_docker_sandbox: DeNovoAgentDockerSandbox::On,
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
#[expect(
    clippy::useless_format,
    reason = "Python fixture keeps literal braces escaped"
)]
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
        agent_docker_sandbox: DeNovoAgentDockerSandbox::On,
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
#[expect(
    clippy::useless_format,
    reason = "Python fixture keeps literal braces escaped"
)]
fn denovo_matrix_checkpoint_skips_conditions_not_started_yet() {
    let temp_dir = target_temp_dir("stateful-bench-denovo-matrix-skip-pending");
    let aweagent_root = temp_dir.join("AweAgent");
    fs::create_dir_all(&aweagent_root).expect("fake AweAgent root should be created");
    let data_file = temp_dir.join("denovo.jsonl");
    fs::write(&data_file, r#"{"instance_id":"case-a"}"#).expect("data file should be written");
    let probe_path = temp_dir.join("pending-report-probe.txt");
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
parser.add_argument("--instance-id", action="append", default=[])
args, _ = parser.parse_known_args()

output = Path(args.output)
run_dir = output.parents[2]
if args.agent_mode == "stateful":
    pending_report = run_dir / "conditions" / "stateful-on_subagent-on" / "denovo-report.json"
    Path(os.environ["DENOVO_PENDING_PROBE"]).write_text(str(pending_report.exists()), encoding="utf-8")

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

    let probe_env = probe_path.to_string_lossy();
    run_denovo_matrix(DeNovoMatrixRunOptions {
        run_id: "dev-denovo-skip-pending".to_string(),
        aweagent_root,
        python: "python3".to_string(),
        data_file,
        run_dir: temp_dir.join("runs"),
        base_config: PathBuf::from("configs/tasks/denovoswe.yaml"),
        conditions: vec![
            parse_denovo_condition(&format!(
                "stateful:off,subagent:on,env:DENOVO_PENDING_PROBE={probe_env}"
            ))
            .expect("off condition should parse"),
            parse_denovo_condition(&format!(
                "stateful:on,subagent:on,env:DENOVO_PENDING_PROBE={probe_env}"
            ))
            .expect("on condition should parse"),
        ],
        agent: DeNovoAgentKind::CodexCli,
        codex_bin: "codex".to_string(),
        omp_bin: "omp".to_string(),
        stateful_binary: "stateful".to_string(),
        agent_docker_image: None,
        agent_docker_stateful_binary: "/usr/local/bin/stateful".to_string(),
        agent_docker_sandbox: DeNovoAgentDockerSandbox::On,
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
        instance_ids: vec!["case-a".to_string()],
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

    assert_eq!(
        fs::read_to_string(&probe_path).expect("probe should be written"),
        "False"
    );

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
            parse_denovo_condition("stateful:off,subagent:on").expect("condition should parse"),
        ],
        agent: DeNovoAgentKind::CodexCli,
        codex_bin: "codex".to_string(),
        omp_bin: "omp".to_string(),
        stateful_binary: "stateful".to_string(),
        agent_docker_image: None,
        agent_docker_stateful_binary: "/usr/local/bin/stateful".to_string(),
        agent_docker_sandbox: DeNovoAgentDockerSandbox::On,
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
