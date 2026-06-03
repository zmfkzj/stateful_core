use std::{fs, path::Path};

use stateful_bench::{SyntheticOptions, read_jsonl, run_synthetic_benchmark};

#[test]
fn synthetic_benchmark_runs_both_modes_and_compares_current_state_coordination() {
    let root = temp_root("stateful-bench-synthetic");

    let artifacts = run_synthetic_benchmark(SyntheticOptions {
        output_dir: root.clone(),
        run_id: "synthetic-dev".to_string(),
    })
    .expect("synthetic benchmark should run");

    assert_eq!(
        artifacts.stateful_run_dir,
        root.join("synthetic-dev-stateful")
    );
    assert_eq!(
        artifacts.no_state_run_dir,
        root.join("synthetic-dev-no-state")
    );
    assert!(artifacts.manifest_path.is_file());
    assert!(artifacts.stateful_run_dir.join("run.json").is_file());
    assert!(artifacts.no_state_run_dir.join("run.json").is_file());

    let manifest: Vec<stateful_bench::PairManifestEntry> =
        read_jsonl(&artifacts.manifest_path).expect("manifest should parse");
    assert_eq!(manifest.len(), 5);
    assert!(
        manifest
            .iter()
            .any(|pair| pair.pair_id == "same-position-insert")
    );
    assert!(
        manifest
            .iter()
            .any(|pair| pair.pair_id == "offline-reconnect-sync")
    );
    assert!(
        manifest
            .iter()
            .any(|pair| pair.pair_id == "duplicate-message-idempotency")
    );
    assert!(
        manifest
            .iter()
            .any(|pair| pair.pair_id == "save-reload-consistency")
    );

    let comparison = artifacts.comparison;
    assert_eq!(comparison.manifest_pairs, 5);
    assert_eq!(comparison.paired.paired_valid_pairs, 5);
    assert_eq!(comparison.stateful.task_passed, 10);
    assert_eq!(comparison.no_state.task_failed, 5);
    assert_eq!(comparison.no_state.lost_edit_events, 4);
    assert_eq!(comparison.no_state.uncoordinated_same_file_collisions, 4);
    assert_eq!(comparison.stateful.lost_edit_events, 0);
    assert_eq!(comparison.stateful.coordinated_blocks, 5);
    assert_eq!(comparison.stateful.denied_writes, 5);
    assert!(
        comparison.stateful.raw_manifest_score > comparison.no_state.raw_manifest_score,
        "stateful synthetic score should be higher than no-state"
    );

    fs::remove_dir_all(root).expect("temp root should clean up");
}

#[test]
fn synthetic_benchmark_records_idempotency_and_reload_failures_as_no_state_regressions() {
    let root = temp_root("stateful-bench-synthetic-regressions");

    let artifacts = run_synthetic_benchmark(SyntheticOptions {
        output_dir: root.clone(),
        run_id: "synthetic-regressions".to_string(),
    })
    .expect("synthetic benchmark should run");

    let duplicate_no_state = read_harness_statuses(
        &artifacts
            .no_state_run_dir
            .join("duplicate-message-idempotency/harness-result.json"),
    );
    let duplicate_stateful = read_harness_statuses(
        &artifacts
            .stateful_run_dir
            .join("duplicate-message-idempotency/harness-result.json"),
    );
    let reload_no_state = read_harness_statuses(
        &artifacts
            .no_state_run_dir
            .join("save-reload-consistency/harness-result.json"),
    );

    assert_eq!(duplicate_no_state, vec!["failed", "passed"]);
    assert_eq!(duplicate_stateful, vec!["passed", "passed"]);
    assert_eq!(reload_no_state, vec!["passed", "failed"]);

    let same_position = read_json(
        &artifacts
            .no_state_run_dir
            .join("same-position-insert/harness-result.json"),
    );
    assert_eq!(same_position["synthetic"]["expected_document"], "AB\n");
    assert_eq!(same_position["synthetic"]["live_document"], "B\n");
    assert_eq!(same_position["synthetic"]["canonical_match"], false);

    let duplicate_metrics = read_json(
        &artifacts
            .no_state_run_dir
            .join("duplicate-message-idempotency/harness-result.json"),
    );
    assert_eq!(
        duplicate_metrics["metrics"]["duplicate_messages_applied"]
            .as_u64()
            .expect("duplicate metric should exist"),
        1
    );

    fs::remove_dir_all(root).expect("temp root should clean up");
}

fn read_harness_statuses(path: &Path) -> Vec<String> {
    let value = read_json(path);
    value["task_results"]
        .as_array()
        .expect("task_results should be an array")
        .iter()
        .map(|item| {
            item["status"]
                .as_str()
                .expect("status should be a string")
                .to_string()
        })
        .collect()
}

fn read_json(path: &Path) -> serde_json::Value {
    serde_json::from_str(&fs::read_to_string(path).expect("json file should read"))
        .expect("json file should parse")
}

fn temp_root(name: &str) -> std::path::PathBuf {
    let root = std::env::temp_dir().join(format!("{name}-{}", std::process::id()));
    if Path::new(&root).exists() {
        fs::remove_dir_all(&root).expect("old temp root should clean up");
    }
    root
}
