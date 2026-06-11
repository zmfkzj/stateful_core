use std::{fs, path::Path};

use stateful_bench::{
    AgentRunRecord, CompareOptions, PairClass, PairComparisonStatus, PairEligibility,
    PairManifestEntry, PairRunRecord, RunMode, SweBenchInstance, compare_runs, write_jsonl,
};

#[test]
fn compare_runs_uses_paired_valid_denominator_and_reports_missing_invalid_pairs() {
    let root = temp_root("stateful-bench-compare");
    let manifest_path = root.join("pairs.jsonl");
    let stateful_dir = root.join("runs/stateful");
    let no_state_dir = root.join("runs/no-state");
    fs::create_dir_all(&stateful_dir).expect("stateful run dir should exist");
    fs::create_dir_all(&no_state_dir).expect("no-state run dir should exist");
    fs::write(
        stateful_dir.join("run.json"),
        r#"{"run_id":"stateful","mode":"stateful"}"#,
    )
    .expect("stateful metadata should write");
    fs::write(
        no_state_dir.join("run.json"),
        r#"{"run_id":"no-state","mode":"no-state"}"#,
    )
    .expect("no-state metadata should write");

    write_jsonl(
        &manifest_path,
        &[
            pair("pair-1"),
            pair("pair-2"),
            pair("pair-3"),
            pair("pair-4"),
            pair("pair-5"),
        ],
    )
    .expect("manifest should write");

    write_pair(
        &stateful_dir,
        RunMode::Stateful,
        "pair-1",
        "passed",
        "passed",
    );
    write_pair(
        &stateful_dir,
        RunMode::Stateful,
        "pair-2",
        "failed",
        "failed",
    );
    write_pair(
        &stateful_dir,
        RunMode::Stateful,
        "pair-3",
        "setup_error",
        "passed",
    );
    write_pair(
        &stateful_dir,
        RunMode::Stateful,
        "pair-4",
        "passed",
        "failed",
    );
    write_pair(
        &stateful_dir,
        RunMode::Stateful,
        "pair-5",
        "passed",
        "failed",
    );

    write_pair(
        &no_state_dir,
        RunMode::NoState,
        "pair-1",
        "passed",
        "failed",
    );
    fs::create_dir_all(no_state_dir.join("pair-2"))
        .expect("incomplete no-state pair dir should exist");
    write_pair(
        &no_state_dir,
        RunMode::NoState,
        "pair-3",
        "passed",
        "passed",
    );
    write_pair(
        &no_state_dir,
        RunMode::NoState,
        "pair-4",
        "mystery",
        "mystery",
    );
    write_pair_with_empty_harness(&no_state_dir, RunMode::NoState, "pair-5");

    let report = compare_runs(CompareOptions {
        stateful_run_dir: vec![stateful_dir],
        no_state_run_dir: vec![no_state_dir],
        manifest: manifest_path,
        max_pairs: None,
    })
    .expect("comparison should build");

    assert_eq!(report.manifest_pairs, 5);
    assert!(report.empirical_claim_allowed);
    assert!(report.evidence_notes.iter().any(|note| {
        note.contains("executed agent pairs") && note.contains("overhead reporting")
    }));
    assert_eq!(report.paired.paired_valid_pairs, 1);
    assert_eq!(report.paired.stateful_functional_score, Some(1.0));
    assert_eq!(report.paired.no_state_functional_score, Some(0.5));
    assert_eq!(report.paired.paired_valid_functional_delta, Some(0.5));
    assert_eq!(report.paired.raw_manifest_functional_delta, Some(0.1));

    assert_eq!(report.stateful.artifact_pairs, 5);
    assert_eq!(report.stateful.scored_pairs, 4);
    assert_eq!(report.stateful.setup_error_pairs, 1);
    assert_eq!(report.stateful.unknown_pairs, 0);
    assert_eq!(report.stateful.missing_artifacts, 0);
    assert_eq!(report.stateful.available_valid_score, Some(0.5));
    assert_eq!(report.stateful.raw_manifest_score, 0.4);

    assert_eq!(report.no_state.artifact_pairs, 4);
    assert_eq!(report.no_state.scored_pairs, 2);
    assert_eq!(report.no_state.setup_error_pairs, 0);
    assert_eq!(report.no_state.unknown_pairs, 2);
    assert_eq!(report.no_state.missing_artifacts, 1);
    assert_eq!(report.no_state.available_valid_score, Some(0.75));
    assert_eq!(report.no_state.raw_manifest_score, 0.3);

    assert_eq!(report.excluded_pairs.len(), 4);
    assert!(report.excluded_pairs.iter().any(|pair| {
        pair.pair_id == "pair-2" && pair.no_state_status == PairComparisonStatus::MissingArtifact
    }));
    assert!(report.excluded_pairs.iter().any(|pair| {
        pair.pair_id == "pair-3" && pair.stateful_status == PairComparisonStatus::SetupError
    }));
    assert!(report.excluded_pairs.iter().any(|pair| {
        pair.pair_id == "pair-4" && pair.no_state_status == PairComparisonStatus::Unknown
    }));
    assert!(report.excluded_pairs.iter().any(|pair| {
        pair.pair_id == "pair-5" && pair.no_state_status == PairComparisonStatus::Unknown
    }));

    let markdown = report.render_markdown();
    assert!(markdown.contains("# Stateful Bench Comparison"));
    assert!(markdown.contains("| Paired valid functional delta | 0.500 |"));
    assert!(markdown.contains("| Missing artifacts | 0 | 1 |"));

    fs::remove_dir_all(root).expect("temp root should clean up");
}

#[test]
fn compare_runs_can_limit_manifest_to_first_n_pairs() {
    let root = temp_root("stateful-bench-compare-limit");
    let manifest_path = root.join("pairs.jsonl");
    let stateful_dir = root.join("runs/stateful");
    let no_state_dir = root.join("runs/no-state");
    fs::create_dir_all(&stateful_dir).expect("stateful run dir should exist");
    fs::create_dir_all(&no_state_dir).expect("no-state run dir should exist");
    fs::write(
        stateful_dir.join("run.json"),
        r#"{"run_id":"stateful","mode":"stateful"}"#,
    )
    .expect("stateful metadata should write");
    fs::write(
        no_state_dir.join("run.json"),
        r#"{"run_id":"no-state","mode":"no-state"}"#,
    )
    .expect("no-state metadata should write");

    write_jsonl(
        &manifest_path,
        &[pair("pair-1"), pair("pair-2"), pair("pair-3")],
    )
    .expect("manifest should write");
    write_pair(
        &stateful_dir,
        RunMode::Stateful,
        "pair-1",
        "passed",
        "passed",
    );
    write_pair(
        &stateful_dir,
        RunMode::Stateful,
        "pair-2",
        "passed",
        "passed",
    );
    write_pair(
        &stateful_dir,
        RunMode::Stateful,
        "pair-3",
        "passed",
        "passed",
    );
    write_pair(
        &no_state_dir,
        RunMode::NoState,
        "pair-1",
        "passed",
        "passed",
    );
    write_pair(
        &no_state_dir,
        RunMode::NoState,
        "pair-2",
        "passed",
        "failed",
    );
    write_pair(
        &no_state_dir,
        RunMode::NoState,
        "pair-3",
        "failed",
        "failed",
    );

    let report = compare_runs(CompareOptions {
        stateful_run_dir: vec![stateful_dir],
        no_state_run_dir: vec![no_state_dir],
        manifest: manifest_path,
        max_pairs: Some(2),
    })
    .expect("comparison should build");

    assert_eq!(report.manifest_pairs, 2);
    assert_eq!(report.pairs.len(), 2);
    assert_eq!(report.pairs[0].pair_id, "pair-1");
    assert_eq!(report.pairs[1].pair_id, "pair-2");
    assert!(report.pairs.iter().all(|pair| pair.pair_id != "pair-3"));
    assert_eq!(report.no_state.raw_manifest_score, 0.75);

    fs::remove_dir_all(root).expect("temp root should clean up");
}

#[test]
fn compare_runs_aggregates_discriminating_harness_metrics() {
    let root = temp_root("stateful-bench-compare-discriminating-metrics");
    let manifest_path = root.join("pairs.jsonl");
    let stateful_dir = root.join("runs/stateful");
    let no_state_dir = root.join("runs/no-state");
    fs::create_dir_all(&stateful_dir).expect("stateful run dir should exist");
    fs::create_dir_all(&no_state_dir).expect("no-state run dir should exist");
    fs::write(
        stateful_dir.join("run.json"),
        r#"{"run_id":"stateful","mode":"stateful"}"#,
    )
    .expect("stateful metadata should write");
    fs::write(
        no_state_dir.join("run.json"),
        r#"{"run_id":"no-state","mode":"no-state"}"#,
    )
    .expect("no-state metadata should write");

    write_jsonl(&manifest_path, &[pair("pair-1")]).expect("manifest should write");
    write_pair_with_metrics(
        &stateful_dir,
        RunMode::Stateful,
        "pair-1",
        "passed",
        "passed",
        serde_json::json!({
            "preserved_edit_count": 2,
            "missing_expected_line_count": 0,
            "false_block_count": 0,
            "missed_conflict_count": 0,
            "manual_intervention_count": 0,
            "time_to_converge_ms": 12
        }),
    );
    write_pair_with_metrics(
        &no_state_dir,
        RunMode::NoState,
        "pair-1",
        "failed",
        "passed",
        serde_json::json!({
            "preserved_edit_count": 1,
            "missing_expected_line_count": 1,
            "false_block_count": 0,
            "missed_conflict_count": 1,
            "manual_intervention_count": 2,
            "time_to_converge_ms": 40
        }),
    );

    let report = compare_runs(CompareOptions {
        stateful_run_dir: vec![stateful_dir],
        no_state_run_dir: vec![no_state_dir],
        manifest: manifest_path,
        max_pairs: None,
    })
    .expect("comparison should build");

    assert_eq!(report.stateful.preserved_edit_count, 2);
    assert_eq!(report.stateful.missing_expected_line_count, 0);
    assert_eq!(report.stateful.missed_conflict_count, 0);
    assert_eq!(report.stateful.manual_intervention_count, 0);
    assert_eq!(report.stateful.time_to_converge_ms, Some(12));
    assert_eq!(report.no_state.preserved_edit_count, 1);
    assert_eq!(report.no_state.missing_expected_line_count, 1);
    assert_eq!(report.no_state.missed_conflict_count, 1);
    assert_eq!(report.no_state.manual_intervention_count, 2);
    assert_eq!(report.no_state.time_to_converge_ms, Some(40));

    let markdown = report.render_markdown();
    assert!(markdown.contains("| Preserved edit count | 2 | 1 |"));
    assert!(markdown.contains("| Missed conflict count | 0 | 1 |"));

    fs::remove_dir_all(root).expect("temp root should clean up");
}

#[test]
fn compare_runs_reports_coordination_effect_and_friction_deltas() {
    let root = temp_root("stateful-bench-compare-coordination-effects");
    let manifest_path = root.join("pairs.jsonl");
    let stateful_dir = root.join("runs/stateful");
    let no_state_dir = root.join("runs/no-state");
    fs::create_dir_all(&stateful_dir).expect("stateful run dir should exist");
    fs::create_dir_all(&no_state_dir).expect("no-state run dir should exist");
    fs::write(
        stateful_dir.join("run.json"),
        r#"{"run_id":"stateful","mode":"stateful"}"#,
    )
    .expect("stateful metadata should write");
    fs::write(
        no_state_dir.join("run.json"),
        r#"{"run_id":"no-state","mode":"no-state"}"#,
    )
    .expect("no-state metadata should write");

    write_jsonl(&manifest_path, &[exact_overlap_pair("pair-1")]).expect("manifest should write");
    write_pair(
        &stateful_dir,
        RunMode::Stateful,
        "pair-1",
        "passed",
        "passed",
    );
    write_pair(
        &no_state_dir,
        RunMode::NoState,
        "pair-1",
        "passed",
        "passed",
    );
    write_observer_events(
        &stateful_dir,
        "pair-1",
        &[
            serde_json::json!({"event_type":"coordinated_block","path":"src/lib.rs"}),
            serde_json::json!({"event_type":"denied_write","path":"src/lib.rs"}),
            serde_json::json!({"event_type":"denied_write","path":"src/lib.rs"}),
            serde_json::json!({"event_type":"stale_intent","path":"src/lib.rs"}),
        ],
    );
    write_observer_events(
        &no_state_dir,
        "pair-1",
        &[
            serde_json::json!({"event_type":"uncoordinated_same_file_write_collision","path":"src/lib.rs"}),
            serde_json::json!({"event_type":"uncoordinated_same_file_write_collision","path":"src/lib.rs"}),
            serde_json::json!({"event_type":"lost_edit_event","path":"src/lib.rs"}),
        ],
    );

    let report = compare_runs(CompareOptions {
        stateful_run_dir: vec![stateful_dir],
        no_state_run_dir: vec![no_state_dir],
        manifest: manifest_path,
        max_pairs: None,
    })
    .expect("comparison should build");

    let value = serde_json::to_value(&report).expect("comparison report should serialize");
    assert_eq!(
        value["coordination_effects"]["prevented_uncoordinated_same_file_collisions"],
        2
    );
    assert_eq!(
        value["coordination_effects"]["prevented_lost_edit_events"],
        1
    );
    assert_eq!(
        value["coordination_effects"]["additional_coordinated_blocks"],
        1
    );
    assert_eq!(value["coordination_effects"]["additional_denied_writes"], 2);
    assert_eq!(
        value["coordination_effects"]["additional_coordination_friction_events"],
        4
    );

    let markdown = report.render_markdown();
    assert!(markdown.contains("## Coordination Effects"));
    assert!(markdown.contains("| Prevented uncoordinated same-file collisions | 2 |"));
    assert!(markdown.contains("| Prevented lost edit events | 1 |"));
    assert!(markdown.contains("| Additional coordination friction events | 4 |"));

    fs::remove_dir_all(root).expect("temp root should clean up");
}

fn write_pair(run_dir: &Path, mode: RunMode, pair_id: &str, task_a: &str, task_b: &str) {
    write_pair_with_metrics(
        run_dir,
        mode,
        pair_id,
        task_a,
        task_b,
        serde_json::json!({}),
    );
}

fn write_pair_with_metrics(
    run_dir: &Path,
    mode: RunMode,
    pair_id: &str,
    task_a: &str,
    task_b: &str,
    metrics: serde_json::Value,
) {
    let pair_dir = run_dir.join(pair_id);
    fs::create_dir_all(&pair_dir).expect("pair dir should exist");
    fs::write(
        pair_dir.join("pair-run.json"),
        serde_json::to_string(&PairRunRecord {
            pair_id: pair_id.to_string(),
            mode,
            agent_a: AgentRunRecord::finished("agent-a", 0, 100),
            agent_b: AgentRunRecord::finished("agent-b", 0, 100),
            agents: vec![
                AgentRunRecord::finished("agent-a", 0, 100),
                AgentRunRecord::finished("agent-b", 0, 100),
            ],
            wall_time_ms: 125,
            combined_patch_path: "combined.patch".to_string(),
            harness_result_path: Some("harness-result.json".to_string()),
            error: None,
        })
        .expect("pair run should serialize"),
    )
    .expect("pair run should write");
    fs::write(
        pair_dir.join("harness-result.json"),
        serde_json::json!({
            "task_results": [
                {"status": task_a, "setup_error": task_a == "setup_error"},
                {"status": task_b, "setup_error": task_b == "setup_error"}
            ],
            "metrics": metrics
        })
        .to_string(),
    )
    .expect("harness result should write");
}

fn write_observer_events(run_dir: &Path, pair_id: &str, events: &[serde_json::Value]) {
    write_jsonl(run_dir.join(pair_id).join("observer-events.jsonl"), events)
        .expect("observer events should write");
}

fn write_pair_with_empty_harness(run_dir: &Path, mode: RunMode, pair_id: &str) {
    let pair_dir = run_dir.join(pair_id);
    fs::create_dir_all(&pair_dir).expect("pair dir should exist");
    fs::write(
        pair_dir.join("pair-run.json"),
        serde_json::to_string(&PairRunRecord {
            pair_id: pair_id.to_string(),
            mode,
            agent_a: AgentRunRecord::finished("agent-a", 0, 100),
            agent_b: AgentRunRecord::finished("agent-b", 0, 100),
            agents: vec![
                AgentRunRecord::finished("agent-a", 0, 100),
                AgentRunRecord::finished("agent-b", 0, 100),
            ],
            wall_time_ms: 125,
            combined_patch_path: "combined.patch".to_string(),
            harness_result_path: Some("harness-result.json".to_string()),
            error: None,
        })
        .expect("pair run should serialize"),
    )
    .expect("pair run should write");
    fs::write(pair_dir.join("harness-result.json"), "").expect("empty harness result should write");
}

fn pair(pair_id: &str) -> PairManifestEntry {
    PairManifestEntry {
        pair_id: pair_id.to_string(),
        repo: "example/repo".to_string(),
        base_commit: Some("base".to_string()),
        version: Some("1.0".to_string()),
        eligibility: PairEligibility::SameBaseCommit,
        class: PairClass::SameRepoDisjoint,
        task_a_files: vec!["a.py".to_string()],
        task_b_files: vec!["b.py".to_string()],
        task_a: instance(&format!("{pair_id}-a")),
        task_b: instance(&format!("{pair_id}-b")),
    }
}

fn exact_overlap_pair(pair_id: &str) -> PairManifestEntry {
    PairManifestEntry {
        class: PairClass::ExactFileOverlap,
        task_b_files: vec!["a.py".to_string()],
        ..pair(pair_id)
    }
}

fn instance(instance_id: &str) -> SweBenchInstance {
    SweBenchInstance {
        instance_id: instance_id.to_string(),
        repo: "example/repo".to_string(),
        base_commit: "base".to_string(),
        problem_statement: "Fix it".to_string(),
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
