use clap::Parser;
use stateful_bench::{Cli, Command, ReportFormat, RunMode};
use std::process::Command as ProcessCommand;

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
