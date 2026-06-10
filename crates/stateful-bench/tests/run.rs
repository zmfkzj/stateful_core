use std::{
    fs,
    io::Write,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
};

use stateful_bench::{
    PairClass, PairEligibility, PairManifestEntry, RunMode, RunOptions, SweBenchInstance,
    build_report, run_pairs, write_jsonl,
};

#[test]
fn run_pairs_executes_manifest_agents_in_one_workspace_and_reports_harness_result() {
    let root = temp_root("stateful-bench-run");
    let pairs_path = root.join("pairs.jsonl");
    let output_dir = root.join("runs");
    let pair = pair_with_agent_ids(&["agent-a", "agent-b", "agent-c", "agent-d", "agent-e"]);
    write_jsonl(&pairs_path, &[pair]).expect("pair manifest should write");

    let metadata = run_pairs(RunOptions {
        pairs: pairs_path,
        mode: RunMode::NoState,
        run_id: "synthetic-no-state".to_string(),
        agent_cmd_template: "printf changed-{agent_id} > {workspace}/{agent_id}.txt".to_string(),
        output_dir: output_dir.clone(),
        timeout_seconds: 10,
        max_pairs: None,
        pair_ids: Vec::new(),
        jobs: 1,
        auth_check_cmd_template: None,
        budget_check_cmd_template: None,
        setup_cmd_template: Some(
            "git init {workspace} && git -C {workspace} config user.email test@example.invalid && git -C {workspace} config user.name test && for agent in agent-a agent-b agent-c agent-d agent-e; do printf initial > {workspace}/$agent.txt; done && git -C {workspace} add . && git -C {workspace} commit -m initial"
                .to_string(),
        ),
        harness_cmd_template: Some(
            "printf '%s\n' '{\"task_results\":[{\"status\":\"passed\"},{\"status\":\"passed\"}]}'"
                .to_string(),
        ),
        stateful_binary: "stateful".to_string(),
    })
    .expect("synthetic run should complete");

    assert_eq!(metadata.run_id, "synthetic-no-state");

    let run_dir = output_dir.join("synthetic-no-state");
    let task_input: serde_json::Value = serde_json::from_str(
        &fs::read_to_string(run_dir.join("pair-1-pair-2/workspace/.stateful_bench/task-a.json"))
            .expect("agent task input should exist"),
    )
    .expect("agent task input should parse");
    assert_eq!(task_input["problem_statement"], "Edit a file");
    assert!(task_input.get("patch").is_none());
    assert!(task_input.get("test_patch").is_none());
    assert!(
        run_dir
            .join("pair-1-pair-2/workspace/.stateful_bench/task-c.json")
            .is_file()
    );

    let combined_patch = fs::read_to_string(run_dir.join("pair-1-pair-2/combined.patch"))
        .expect("combined diff should exist");
    assert!(combined_patch.contains("changed-agent-a"));
    assert!(combined_patch.contains("changed-agent-b"));
    assert!(combined_patch.contains("changed-agent-c"));
    assert!(combined_patch.contains("changed-agent-d"));
    assert!(combined_patch.contains("changed-agent-e"));

    let pair_run: serde_json::Value = serde_json::from_str(
        &fs::read_to_string(run_dir.join("pair-1-pair-2/pair-run.json"))
            .expect("pair run should exist"),
    )
    .expect("pair run should parse");
    let agents = pair_run["agents"]
        .as_array()
        .expect("pair run should record all manifest agents");
    assert_eq!(agents.len(), 5);
    assert_eq!(
        agents
            .iter()
            .map(|agent| agent["agent_id"].as_str().expect("agent id"))
            .collect::<Vec<_>>(),
        ["agent-a", "agent-b", "agent-c", "agent-d", "agent-e"]
    );

    let report = build_report(&run_dir).expect("report should build");
    assert_eq!(report.summary.pairs_scored, 1);
    assert_eq!(report.summary.task_passed, 2);
    assert_eq!(report.summary.composite_coordination_score, 1.0);

    fs::remove_dir_all(root).expect("temp root should clean up");
}

#[test]
fn run_pairs_templates_use_absolute_paths_when_output_dir_is_relative() {
    let root = temp_root("stateful-bench-run-relative-output");
    let pairs_path = root.join("pairs.jsonl");
    let output_dir = PathBuf::from(format!(
        "../../target/stateful-bench-run-relative-output-{}/runs",
        std::process::id()
    ));
    let output_root = std::env::current_dir()
        .expect("current dir should resolve")
        .join(format!(
            "../../target/stateful-bench-run-relative-output-{}",
            std::process::id()
        ));
    if output_root.exists() {
        fs::remove_dir_all(&output_root).expect("old relative output root should clean up");
    }
    write_jsonl(&pairs_path, &[pair()]).expect("pair manifest should write");

    run_pairs(RunOptions {
        pairs: pairs_path,
        mode: RunMode::NoState,
        run_id: "synthetic-relative-output".to_string(),
        agent_cmd_template:
            "test -f {task_json} && printf changed-{agent_id} > {workspace}/{agent_id}.txt"
                .to_string(),
        output_dir: output_dir.clone(),
        timeout_seconds: 10,
        max_pairs: None,
        pair_ids: Vec::new(),
        jobs: 1,
        auth_check_cmd_template: None,
        budget_check_cmd_template: None,
        setup_cmd_template: Some(
            "test -f {pair_json} && test -d {workspace} && git init {workspace} && git -C {workspace} config user.email test@example.invalid && git -C {workspace} config user.name test && printf initial > {workspace}/agent-a.txt && printf initial > {workspace}/agent-b.txt && git -C {workspace} add . && git -C {workspace} commit -m initial"
                .to_string(),
        ),
        harness_cmd_template: Some(
            "test -f {combined_patch} && printf '%s\n' '{\"task_results\":[{\"status\":\"passed\"},{\"status\":\"passed\"}]}'"
                .to_string(),
        ),
        stateful_binary: "stateful".to_string(),
    })
    .expect("relative output dir run should complete");

    let run_dir = output_dir.join("synthetic-relative-output");
    let pair_run: serde_json::Value = serde_json::from_str(
        &fs::read_to_string(run_dir.join("pair-1-pair-2/pair-run.json"))
            .expect("pair run should exist"),
    )
    .expect("pair run should parse");
    assert_eq!(pair_run["agent_a"]["outcome"], "succeeded");
    assert_eq!(pair_run["agent_b"]["outcome"], "succeeded");
    assert!(pair_run.get("error").is_none() || pair_run["error"].is_null());

    let report = build_report(&run_dir).expect("report should build");
    assert_eq!(report.summary.setup_errors, 0);
    assert_eq!(report.summary.pairs_scored, 1);

    fs::remove_dir_all(root).expect("temp root should clean up");
    fs::remove_dir_all(output_root).expect("relative output root should clean up");
}

#[test]
fn run_pairs_filters_to_explicit_pair_ids_before_execution() {
    let root = temp_root("stateful-bench-run-pair-id-filter");
    let pairs_path = root.join("pairs.jsonl");
    let output_dir = root.join("runs");
    write_jsonl(
        &pairs_path,
        &[
            pair_with_id("pair-1/pair-2"),
            pair_with_id("pair-3/pair-4"),
            pair_with_id("pair-5/pair-6"),
        ],
    )
    .expect("pair manifest should write");

    let metadata = run_pairs(RunOptions {
        pairs: pairs_path,
        mode: RunMode::NoState,
        run_id: "synthetic-pair-id-filter".to_string(),
        agent_cmd_template: "printf changed-{agent_id} > {workspace}/{agent_id}.txt".to_string(),
        output_dir: output_dir.clone(),
        timeout_seconds: 10,
        max_pairs: None,
        pair_ids: vec!["pair-3/pair-4".to_string(), "pair-5/pair-6".to_string()],
        jobs: 1,
        auth_check_cmd_template: None,
        budget_check_cmd_template: None,
        setup_cmd_template: Some("mkdir -p {workspace}".to_string()),
        harness_cmd_template: None,
        stateful_binary: "stateful".to_string(),
    })
    .expect("filtered run should complete");

    assert_eq!(metadata.run_id, "synthetic-pair-id-filter");
    let run_dir = output_dir.join("synthetic-pair-id-filter");
    assert!(!run_dir.join("pair-1-pair-2").exists());
    assert!(run_dir.join("pair-3-pair-4/pair-run.json").is_file());
    assert!(run_dir.join("pair-5-pair-6/pair-run.json").is_file());

    fs::remove_dir_all(root).expect("temp root should clean up");
}

#[test]
fn run_pairs_executes_multiple_pairs_with_multiple_jobs() {
    let root = temp_root("stateful-bench-run-jobs");
    let pairs_path = root.join("pairs.jsonl");
    let output_dir = root.join("runs");
    write_jsonl(
        &pairs_path,
        &[pair_with_id("pair-1/pair-2"), pair_with_id("pair-3/pair-4")],
    )
    .expect("pair manifest should write");

    let metadata = run_pairs(RunOptions {
        pairs: pairs_path,
        mode: RunMode::NoState,
        run_id: "synthetic-no-state-jobs".to_string(),
        agent_cmd_template: "printf changed-{agent_id} > {workspace}/{agent_id}.txt".to_string(),
        output_dir: output_dir.clone(),
        timeout_seconds: 10,
        max_pairs: None,
        pair_ids: Vec::new(),
        jobs: 2,
        auth_check_cmd_template: None,
        budget_check_cmd_template: None,
        setup_cmd_template: Some(
            "git init {workspace} && git -C {workspace} config user.email test@example.invalid && git -C {workspace} config user.name test && printf initial > {workspace}/agent-a.txt && printf initial > {workspace}/agent-b.txt && git -C {workspace} add . && git -C {workspace} commit -m initial"
                .to_string(),
        ),
        harness_cmd_template: None,
        stateful_binary: "stateful".to_string(),
    })
    .expect("synthetic run should complete");

    assert_eq!(metadata.run_id, "synthetic-no-state-jobs");
    assert!(
        output_dir
            .join("synthetic-no-state-jobs/pair-1-pair-2/pair-run.json")
            .is_file()
    );
    assert!(
        output_dir
            .join("synthetic-no-state-jobs/pair-3-pair-4/pair-run.json")
            .is_file()
    );

    fs::remove_dir_all(root).expect("temp root should clean up");
}

#[test]
fn run_pairs_records_pair_errors_and_continues() {
    let root = temp_root("stateful-bench-run-continue-after-error");
    let pairs_path = root.join("pairs.jsonl");
    let output_dir = root.join("runs");
    write_jsonl(
        &pairs_path,
        &[
            pair_with_id("pair-ok/pair-ok"),
            pair_with_id("pair-fail/pair-fail"),
        ],
    )
    .expect("pair manifest should write");

    let metadata = run_pairs(RunOptions {
        pairs: pairs_path,
        mode: RunMode::NoState,
        run_id: "synthetic-continue-after-error".to_string(),
        agent_cmd_template: "printf changed-{agent_id} > {workspace}/{agent_id}.txt".to_string(),
        output_dir: output_dir.clone(),
        timeout_seconds: 10,
        max_pairs: None,
        pair_ids: Vec::new(),
        jobs: 2,
        auth_check_cmd_template: None,
        budget_check_cmd_template: None,
        setup_cmd_template: Some(
            "case '{pair_id}' in pair-fail*) exit 42;; esac; git init {workspace} && git -C {workspace} config user.email test@example.invalid && git -C {workspace} config user.name test && printf initial > {workspace}/agent-a.txt && printf initial > {workspace}/agent-b.txt && git -C {workspace} add . && git -C {workspace} commit -m initial"
                .to_string(),
        ),
        harness_cmd_template: Some(
            "printf '%s\n' '{\"task_results\":[{\"status\":\"passed\"},{\"status\":\"passed\"}]}'"
                .to_string(),
        ),
        stateful_binary: "stateful".to_string(),
    })
    .expect("run should continue after pair-level errors");

    assert_eq!(metadata.run_id, "synthetic-continue-after-error");

    let run_dir = output_dir.join("synthetic-continue-after-error");
    let failed_pair_run: serde_json::Value = serde_json::from_str(
        &fs::read_to_string(run_dir.join("pair-fail-pair-fail/pair-run.json"))
            .expect("failed pair run should exist"),
    )
    .expect("failed pair run should parse");
    assert_eq!(failed_pair_run["agent_a"]["outcome"], "failed");
    assert!(
        failed_pair_run["error"]
            .as_str()
            .expect("pair error should be recorded")
            .contains("command failed")
    );

    let report = build_report(&run_dir).expect("report should build");
    assert_eq!(report.summary.pairs_total, 2);
    assert_eq!(report.summary.pairs_scored, 1);
    assert_eq!(report.summary.task_passed, 2);
    assert_eq!(report.summary.setup_errors, 2);

    fs::remove_dir_all(root).expect("temp root should clean up");
}

#[test]
fn run_pairs_records_agent_infra_startup_failures_as_setup_errors() {
    let root = temp_root("stateful-bench-run-agent-infra-failure");
    let pairs_path = root.join("pairs.jsonl");
    let output_dir = root.join("runs");
    write_jsonl(&pairs_path, &[pair()]).expect("pair manifest should write");

    run_pairs(RunOptions {
        pairs: pairs_path,
        mode: RunMode::NoState,
        run_id: "synthetic-agent-infra-failure".to_string(),
        agent_cmd_template:
            "printf '%s\n' 'Error: failed to initialize in-process app-server client: Operation not permitted (os error 1)' >&2; exit 1"
                .to_string(),
        output_dir: output_dir.clone(),
        timeout_seconds: 10,
        max_pairs: None,
        pair_ids: Vec::new(),
        jobs: 1,
        auth_check_cmd_template: None,
        budget_check_cmd_template: None,
        setup_cmd_template: Some("mkdir -p {workspace}".to_string()),
        harness_cmd_template: Some(
            "printf '%s\n' '{\"task_results\":[{\"status\":\"passed\"},{\"status\":\"passed\"}]}'"
                .to_string(),
        ),
        stateful_binary: "stateful".to_string(),
    })
    .expect("nonfatal agent infra failures should be recorded as pair errors");

    let run_dir = output_dir.join("synthetic-agent-infra-failure");
    let pair_run: serde_json::Value = serde_json::from_str(
        &fs::read_to_string(run_dir.join("pair-1-pair-2/pair-run.json"))
            .expect("pair run should exist"),
    )
    .expect("pair run should parse");
    assert!(
        pair_run["error"]
            .as_str()
            .expect("infra error should be recorded")
            .contains("agent infrastructure failure")
    );

    let report = build_report(&run_dir).expect("report should build");
    assert_eq!(report.summary.pairs_scored, 0);
    assert_eq!(report.summary.setup_errors, 2);
    assert_eq!(report.summary.task_passed, 0);
    assert_eq!(report.summary.task_failed, 0);

    fs::remove_dir_all(root).expect("temp root should clean up");
}

#[test]
fn run_pairs_aborts_when_agent_hits_usage_limit() {
    let root = temp_root("stateful-bench-run-quota-abort");
    let pairs_path = root.join("pairs.jsonl");
    let output_dir = root.join("runs");
    write_jsonl(
        &pairs_path,
        &[
            pair_with_id("pair-quota/pair-quota"),
            pair_with_id("pair-slow/pair-slow"),
            pair_with_id("pair-should-not-start/pair-should-not-start"),
        ],
    )
    .expect("pair manifest should write");

    let error = run_pairs(RunOptions {
        pairs: pairs_path,
        mode: RunMode::NoState,
        run_id: "synthetic-quota-abort".to_string(),
        agent_cmd_template: r#"case '{pair_id}' in pair-quota*) sleep 0.1; printf '%s\n' '{"type":"error","message":"You'\''ve hit your usage limit."}'; exit 0;; pair-slow*) (trap "" TERM HUP INT; sleep 0.7; printf leaked > {run_dir}/leaked-descendant.txt) & wait;; *) printf changed-{agent_id} > {workspace}/{agent_id}.txt;; esac"#.to_string(),
        output_dir: output_dir.clone(),
        timeout_seconds: 10,
        max_pairs: None,
        pair_ids: Vec::new(),
        jobs: 2,
        auth_check_cmd_template: None,
        budget_check_cmd_template: None,
        setup_cmd_template: Some("mkdir -p {workspace}".to_string()),
        harness_cmd_template: None,
        stateful_binary: "stateful".to_string(),
    })
    .expect_err("usage limit should abort the run");

    assert!(error.to_string().contains("agent usage limit"));

    let run_dir = output_dir.join("synthetic-quota-abort");
    assert!(run_dir.join("fatal-error.txt").is_file());
    assert!(
        !run_dir
            .join("pair-should-not-start-pair-should-not-start")
            .exists()
    );
    std::thread::sleep(std::time::Duration::from_millis(1000));
    assert!(
        !run_dir.join("leaked-descendant.txt").exists(),
        "aborted agent descendants should not outlive process group cleanup"
    );

    fs::remove_dir_all(root).expect("temp root should clean up");
}

#[test]
fn run_pairs_aborts_when_codex_turn_failed_without_limit_text() {
    let root = temp_root("stateful-bench-run-turn-failed-abort");
    let pairs_path = root.join("pairs.jsonl");
    let output_dir = root.join("runs");
    write_jsonl(
        &pairs_path,
        &[
            pair_with_id("pair-turn-failed/pair-turn-failed"),
            pair_with_id("pair-should-not-start/pair-should-not-start"),
        ],
    )
    .expect("pair manifest should write");

    let error = run_pairs(RunOptions {
        pairs: pairs_path,
        mode: RunMode::NoState,
        run_id: "synthetic-turn-failed-abort".to_string(),
        agent_cmd_template: r#"case '{pair_id}' in pair-turn-failed*) printf '%s\n' '{"type":"turn.failed","error":{"message":"stream closed before response.completed"}}'; exit 0;; *) printf changed-{agent_id} > {workspace}/{agent_id}.txt;; esac"#.to_string(),
        output_dir: output_dir.clone(),
        timeout_seconds: 10,
        max_pairs: None,
        pair_ids: Vec::new(),
        jobs: 1,
        auth_check_cmd_template: None,
        budget_check_cmd_template: None,
        setup_cmd_template: Some("mkdir -p {workspace}".to_string()),
        harness_cmd_template: None,
        stateful_binary: "stateful".to_string(),
    })
    .expect_err("Codex turn.failed should abort the run");

    assert!(error.to_string().contains("agent platform failure"));

    let run_dir = output_dir.join("synthetic-turn-failed-abort");
    assert!(run_dir.join("fatal-error.txt").is_file());
    assert!(
        !run_dir
            .join("pair-should-not-start-pair-should-not-start")
            .exists()
    );

    fs::remove_dir_all(root).expect("temp root should clean up");
}

#[test]
fn run_pairs_stateful_waits_for_state_server_before_spawning_agents() {
    let root = temp_root("stateful-bench-stateful-ready");
    fs::create_dir_all(&root).expect("temp root should be creatable");
    let stateful = fake_stateful_binary(&root);
    let pairs_path = root.join("pairs.jsonl");
    let output_dir = root.join("runs");
    write_jsonl(&pairs_path, &[pair()]).expect("pair manifest should write");

    run_pairs(RunOptions {
        pairs: pairs_path,
        mode: RunMode::Stateful,
        run_id: "synthetic-stateful".to_string(),
        agent_cmd_template:
            "test -f \"$STATEFUL_HOME/runtime/server.json\" && printf changed-{agent_id}-{stateful_workspace_id} > {workspace}/{agent_id}.txt"
                .to_string(),
        output_dir: output_dir.clone(),
        timeout_seconds: 10,
        max_pairs: None,
        pair_ids: Vec::new(),
        jobs: 1,
        auth_check_cmd_template: None,
        budget_check_cmd_template: None,
        setup_cmd_template: Some(
            "git init {workspace} && git -C {workspace} config user.email test@example.invalid && git -C {workspace} config user.name test && printf initial > {workspace}/agent-a.txt && printf initial > {workspace}/agent-b.txt && git -C {workspace} add . && git -C {workspace} commit -m initial"
                .to_string(),
        ),
        harness_cmd_template: Some(
            "printf '%s\n' '{\"task_results\":[{\"status\":\"passed\"},{\"status\":\"passed\"}]}'"
                .to_string(),
        ),
        stateful_binary: stateful.to_string_lossy().into_owned(),
    })
    .expect("stateful run should complete");

    let pair_run: serde_json::Value = serde_json::from_str(
        &fs::read_to_string(output_dir.join("synthetic-stateful/pair-1-pair-2/pair-run.json"))
            .expect("pair run should exist"),
    )
    .expect("pair run should parse");
    assert_eq!(pair_run["agent_a"]["outcome"], "succeeded");
    assert_eq!(pair_run["agent_b"]["outcome"], "succeeded");

    let combined_patch =
        fs::read_to_string(output_dir.join("synthetic-stateful/pair-1-pair-2/combined.patch"))
            .expect("combined diff should exist");
    assert!(combined_patch.contains("changed-agent-a"));
    assert!(combined_patch.contains("changed-agent-b"));
    assert!(combined_patch.contains("synthetic-stateful-pair-1/pair-2"));

    fs::remove_dir_all(root).expect("temp root should clean up");
}

#[test]
fn run_pairs_aborts_before_artifacts_when_auth_preflight_fails() {
    let root = temp_root("stateful-bench-auth-preflight");
    let pairs_path = root.join("pairs.jsonl");
    let output_dir = root.join("runs");
    write_jsonl(&pairs_path, &[pair()]).expect("pair manifest should write");

    let error = run_pairs(RunOptions {
        pairs: pairs_path,
        mode: RunMode::NoState,
        run_id: "should-not-start".to_string(),
        agent_cmd_template: "printf should-not-run > {workspace}/agent.txt".to_string(),
        output_dir: output_dir.clone(),
        timeout_seconds: 10,
        max_pairs: None,
        pair_ids: Vec::new(),
        jobs: 1,
        auth_check_cmd_template: Some("false".to_string()),
        budget_check_cmd_template: None,
        setup_cmd_template: Some("mkdir -p {workspace}".to_string()),
        harness_cmd_template: None,
        stateful_binary: "stateful".to_string(),
    })
    .expect_err("failed auth preflight should abort the run");

    assert!(error.to_string().contains("auth preflight failed"));
    assert!(!output_dir.join("should-not-start").exists());

    fs::remove_dir_all(root).expect("temp root should clean up");
}

#[test]
fn run_pairs_aborts_before_artifacts_when_budget_preflight_fails() {
    let root = temp_root("stateful-bench-budget-preflight");
    let pairs_path = root.join("pairs.jsonl");
    let output_dir = root.join("runs");
    write_jsonl(&pairs_path, &[pair()]).expect("pair manifest should write");

    let error = run_pairs(RunOptions {
        pairs: pairs_path,
        mode: RunMode::NoState,
        run_id: "should-not-start".to_string(),
        agent_cmd_template: "printf should-not-run > {workspace}/agent.txt".to_string(),
        output_dir: output_dir.clone(),
        timeout_seconds: 10,
        max_pairs: None,
        pair_ids: Vec::new(),
        jobs: 1,
        auth_check_cmd_template: Some("true".to_string()),
        budget_check_cmd_template: Some("false".to_string()),
        setup_cmd_template: Some("mkdir -p {workspace}".to_string()),
        harness_cmd_template: None,
        stateful_binary: "stateful".to_string(),
    })
    .expect_err("failed budget preflight should abort the run");

    assert!(error.to_string().contains("budget preflight failed"));
    assert!(!output_dir.join("should-not-start").exists());

    fs::remove_dir_all(root).expect("temp root should clean up");
}

fn pair() -> PairManifestEntry {
    pair_with_id("pair-1/pair-2")
}

fn pair_with_agent_ids(agent_ids: &[&str]) -> PairManifestEntry {
    let mut pair = pair();
    let encoded = serde_json::json!({ "agents": agent_ids }).to_string();
    pair.task_a.test_patch = encoded.clone();
    pair.task_b.test_patch = encoded;
    pair
}

fn pair_with_id(pair_id: &str) -> PairManifestEntry {
    PairManifestEntry {
        pair_id: pair_id.to_string(),
        repo: "example/repo".to_string(),
        base_commit: Some("base".to_string()),
        version: Some("1.0".to_string()),
        eligibility: PairEligibility::SameBaseCommit,
        class: PairClass::SameRepoDisjoint,
        task_a_files: vec!["agent-a.txt".to_string()],
        task_b_files: vec!["agent-b.txt".to_string()],
        task_a: instance(&format!("{pair_id}-a")),
        task_b: instance(&format!("{pair_id}-b")),
    }
}

fn instance(instance_id: &str) -> SweBenchInstance {
    SweBenchInstance {
        instance_id: instance_id.to_string(),
        repo: "example/repo".to_string(),
        base_commit: "base".to_string(),
        problem_statement: "Edit a file".to_string(),
        version: Some("1.0".to_string()),
        patch: String::new(),
        test_patch: String::new(),
        fail_to_pass: Vec::new(),
        pass_to_pass: Vec::new(),
        difficulty: None,
    }
}

fn temp_root(name: &str) -> std::path::PathBuf {
    let root = std::env::temp_dir().join(format!("{name}-{}", std::process::id()));
    if Path::new(&root).exists() {
        fs::remove_dir_all(&root).expect("old temp root should clean up");
    }
    root
}

fn fake_stateful_binary(root: &Path) -> PathBuf {
    let path = root.join("fake-stateful");
    let mut file = fs::File::create(&path).expect("fake stateful binary should create");
    file.write_all(
        br#"#!/bin/sh
set -eu
if [ "${1:-}" = "init" ]; then
  exit 0
fi
if [ "${1:-}" = "server" ]; then
  shift
  port=""
  workspace_id="local"
  while [ "$#" -gt 0 ]; do
    case "$1" in
      --port)
        port="$2"
        shift 2
        ;;
      --workspace-id)
        workspace_id="$2"
        shift 2
        ;;
      *)
        shift
        ;;
    esac
  done
  stateful_home="${STATEFUL_HOME:-"$(dirname "$0")/global-stateful-home"}"
  /usr/bin/python3 - "$port" "$workspace_id" "$stateful_home" <<'PY'
import http.server
import json
import os
import socketserver
import sys
import time

port = int(sys.argv[1])
workspace_id = sys.argv[2]
stateful_home = sys.argv[3]
runtime_dir = os.path.join(stateful_home, "runtime")
os.makedirs(runtime_dir, exist_ok=True)
with open(os.path.join(runtime_dir, "server.json"), "w", encoding="utf-8") as handle:
    json.dump({
        "base_url": f"http://127.0.0.1:{port}",
        "token": "fake-token",
        "pid": os.getpid(),
        "workspace_id": workspace_id,
        "protocol_version": "stateful.v1",
        "started_at": "2026-05-31T00:00:00Z",
    }, handle)

class Handler(http.server.BaseHTTPRequestHandler):
    def do_GET(self):
        body = b'{"current":{"active_intent_count":0,"event_count":0,"session_count":0},"status":"ok"}'
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)
    def log_message(self, format, *args):
        return

socketserver.TCPServer.allow_reuse_address = True
deadline = time.time() + 5
with socketserver.TCPServer(("127.0.0.1", port), Handler) as server:
    server.timeout = 0.1
    while time.time() < deadline:
        server.handle_request()
PY
  exit 0
fi
exit 0
"#,
    )
    .expect("fake stateful script should write");
    let mut permissions = fs::metadata(&path)
        .expect("fake stateful metadata should load")
        .permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&path, permissions).expect("fake stateful should be executable");
    path
}
