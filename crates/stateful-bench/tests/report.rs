use std::{fs, path::Path};

use stateful_bench::{
    AgentRunRecord, PairRunRecord, ReportFormat, RunMode, build_report, render_report_markdown,
    write_jsonl,
};

#[test]
fn report_loads_pair_artifacts_and_renders_deterministic_summaries() {
    let root = temp_root("stateful-bench-report");
    let run_dir = root.join("runs/dev-stateful");
    let pair_dir = run_dir.join("pair-1");
    fs::create_dir_all(&pair_dir).expect("pair dir should exist");
    fs::write(
        run_dir.join("run.json"),
        r#"{"run_id":"dev-stateful","mode":"stateful"}"#,
    )
    .expect("run metadata should write");
    fs::write(
        pair_dir.join("pair-run.json"),
        serde_json::to_string(&PairRunRecord {
            pair_id: "pair-1".to_string(),
            mode: RunMode::Stateful,
            agent_a: AgentRunRecord::finished("agent-a", 0, 1000),
            agent_b: AgentRunRecord::finished("agent-b", 1, 1200),
            agents: vec![
                AgentRunRecord::finished("agent-a", 0, 1000),
                AgentRunRecord::finished("agent-b", 1, 1200),
            ],
            wall_time_ms: 1250,
            combined_patch_path: "combined.patch".to_string(),
            harness_result_path: Some("harness-result.json".to_string()),
            error: None,
        })
        .expect("pair record should serialize"),
    )
    .expect("pair record should write");
    fs::write(
        pair_dir.join("harness-result.json"),
        r#"{
          "task_results": [
            {"instance_id": "a", "status": "passed"},
            {"instance_id": "b", "status": "failed"}
          ],
          "metrics": {"token_count": 200, "tool_call_count": 12}
        }"#,
    )
    .expect("harness result should write");
    write_jsonl(
        pair_dir.join("observer-events.jsonl"),
        &[
            serde_json::json!({"event_type":"uncoordinated_same_file_write_collision","path":"src/a.py"}),
            serde_json::json!({"event_type":"denied_write","reason":"scope_mismatch"}),
        ],
    )
    .expect("observer events should write");

    let report = build_report(&run_dir).expect("report should build");
    assert_eq!(report.run_id, "dev-stateful");
    assert_eq!(report.mode, RunMode::Stateful);
    assert_eq!(report.pairs.len(), 1);
    assert_eq!(report.summary.pairs_scored, 1);
    assert_eq!(report.summary.task_passed, 1);
    assert_eq!(report.summary.task_failed, 1);
    assert_eq!(report.summary.uncoordinated_same_file_collisions, 1);

    let json = report
        .render(ReportFormat::Json)
        .expect("json report should render");
    assert!(json.contains("\"run_id\": \"dev-stateful\""));

    let markdown = render_report_markdown(&report);
    assert!(markdown.contains("# Stateful Bench Report: dev-stateful"));
    assert!(markdown.contains("| Composite coordination score | 0.620 |"));

    fs::remove_dir_all(root).expect("temp root should clean up");
}

fn temp_root(name: &str) -> std::path::PathBuf {
    let root = std::env::temp_dir().join(format!("{name}-{}", std::process::id()));
    if Path::new(&root).exists() {
        fs::remove_dir_all(&root).expect("old temp root should clean up");
    }
    root
}
