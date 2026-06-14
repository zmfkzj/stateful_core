use std::collections::BTreeMap;

use stateful_bench::{
    DeNovoCondition, DeNovoExtractRecipeOptions, DeNovoOfficialResult, DeNovoRunMode,
    DeNovoRunRecipeOptions, build_denovo_condition_report, build_denovo_extract_recipe_command,
    build_denovo_run_recipe_command, compare_denovo_reports, default_denovo_conditions,
    parse_denovo_condition,
};

#[test]
fn denovo_condition_parser_accepts_axes_config_and_env() {
    let condition = parse_denovo_condition(
        "stateful:on,subagent:off,config:configs/tasks/denovoswe-stateful.yaml,env:STATEFUL_HOME=/tmp/stateful,env:MODE=stateful",
    )
    .expect("condition should parse");

    assert!(condition.stateful);
    assert!(!condition.subagent);
    assert_eq!(
        condition.config_path.as_deref(),
        Some(std::path::Path::new(
            "configs/tasks/denovoswe-stateful.yaml"
        ))
    );
    assert_eq!(
        condition.env.get("STATEFUL_HOME").map(String::as_str),
        Some("/tmp/stateful")
    );
    assert_eq!(
        condition.env.get("MODE").map(String::as_str),
        Some("stateful")
    );
    assert_eq!(condition.id(), "stateful-on_subagent-off");
}

#[test]
fn denovo_condition_parser_rejects_unknown_keys() {
    let error = parse_denovo_condition("stateful:on,subagent:off,unknown:value")
        .expect_err("unknown key should fail");

    assert!(
        error
            .to_string()
            .contains("unknown DeNovoSWE condition key")
    );
}

#[test]
fn default_denovo_conditions_cover_four_axis_combinations() {
    assert_eq!(
        default_denovo_conditions()
            .iter()
            .map(DeNovoCondition::id)
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
fn denovo_official_result_deserializer_accepts_extra_fields() {
    let result: DeNovoOfficialResult = serde_json::from_str(
        r#"{
          "instance_id": "PyCQA_pep8_pr970",
          "success": true,
          "score": 0.96,
          "eval_result": {
            "details": {"pass_rate": 0.958, "passed": 92, "failed": 4, "duration_ms": 12}
          },
          "new_field_from_aweagent": {"kept": true}
        }"#,
    )
    .expect("official result should deserialize");

    assert_eq!(result.instance_id, "PyCQA_pep8_pr970");
    assert_eq!(result.success, Some(true));
    assert_eq!(result.score, Some(0.96));
    assert_eq!(
        result
            .extra
            .get("new_field_from_aweagent")
            .and_then(|value| value.get("kept"))
            .and_then(serde_json::Value::as_bool),
        Some(true)
    );
    assert_eq!(
        result
            .eval_result
            .as_ref()
            .and_then(|eval| eval.details.as_ref())
            .and_then(|details| details.pass_rate),
        Some(0.958)
    );
    assert_eq!(
        result
            .eval_result
            .as_ref()
            .and_then(|eval| eval.details.as_ref())
            .and_then(|details| details.extra.get("duration_ms"))
            .and_then(serde_json::Value::as_u64),
        Some(12)
    );
}

#[test]
fn denovo_report_aggregates_scores_pass_rates_errors_and_runtime() {
    let condition = DeNovoCondition {
        stateful: true,
        subagent: false,
        config_path: Some("configs/tasks/denovoswe-stateful.yaml".into()),
        env: BTreeMap::new(),
    };
    let results = vec![
        serde_json::from_str::<DeNovoOfficialResult>(
            r#"{"instance_id":"a","success":true,"score":1.0,"eval_result":{"details":{"pass_rate":1.0}}}"#,
        )
        .expect("result a"),
        serde_json::from_str::<DeNovoOfficialResult>(
            r#"{"instance_id":"b","success":false,"score":0.75,"eval_result":{"details":{"pass_rate":0.5}}}"#,
        )
        .expect("result b"),
        serde_json::from_str::<DeNovoOfficialResult>(
            r#"{"instance_id":"c","success":false,"error":"agent failed"}"#,
        )
        .expect("result c"),
    ];

    let report = build_denovo_condition_report(
        "denovo-dev",
        condition,
        results,
        9000,
        Some("abc123".to_string()),
    );

    assert_eq!(report.condition_id, "stateful-on_subagent-off");
    assert_eq!(report.total_instances, 3);
    assert_eq!(report.completed_instances, 2);
    assert_eq!(report.scored_instances, 2);
    assert_eq!(report.pass_rate_instances, 2);
    assert_eq!(report.success_count, 1);
    assert_eq!(report.error_count, 1);
    assert_eq!(report.success_rate, Some(0.333));
    assert_eq!(report.average_score, Some(0.875));
    assert_eq!(report.average_pass_rate, Some(0.75));
    assert_eq!(report.correct_rate, Some(0.333));
    assert_eq!(report.almost_correct_rate, Some(0.333));
    assert_eq!(report.running_time_ms, 9000);
    assert_eq!(report.average_running_time_ms, Some(3000.0));
}

#[test]
fn denovo_comparison_indexes_reports_by_condition_and_computes_deltas() {
    let off_off = build_denovo_condition_report(
        "baseline",
        DeNovoCondition::new(false, false),
        vec![serde_json::from_str(r#"{"instance_id":"a","success":true,"score":0.5}"#).unwrap()],
        1000,
        None,
    );
    let on_off = build_denovo_condition_report(
        "stateful",
        DeNovoCondition::new(true, false),
        vec![serde_json::from_str(r#"{"instance_id":"a","success":true,"score":0.8}"#).unwrap()],
        1500,
        None,
    );
    let off_on = build_denovo_condition_report(
        "subagent",
        DeNovoCondition::new(false, true),
        vec![serde_json::from_str(r#"{"instance_id":"a","success":true,"score":0.7}"#).unwrap()],
        1200,
        None,
    );
    let on_on = build_denovo_condition_report(
        "combined",
        DeNovoCondition::new(true, true),
        vec![serde_json::from_str(r#"{"instance_id":"a","success":true,"score":0.9}"#).unwrap()],
        1800,
        None,
    );

    let comparison = compare_denovo_reports(vec![off_off, on_off, off_on, on_on]);

    assert_eq!(comparison.conditions.len(), 4);
    assert_eq!(comparison.stateful_score_delta_without_subagent, Some(0.3));
    assert_eq!(comparison.subagent_score_delta_without_stateful, Some(0.2));
    assert_eq!(comparison.combined_interaction_score_delta, Some(-0.1));
    assert_eq!(comparison.total_running_time_ms, 5500);
}

#[test]
fn denovo_condition_parser_rejects_duplicate_and_empty_values() {
    let duplicate = parse_denovo_condition("stateful:on,stateful:off,subagent:on")
        .expect_err("duplicate singleton axis should fail");
    assert!(
        duplicate
            .to_string()
            .contains("duplicate DeNovoSWE condition key")
    );

    let empty_config = parse_denovo_condition("stateful:on,subagent:off,config:")
        .expect_err("empty config should fail");
    assert!(
        empty_config
            .to_string()
            .contains("empty DeNovoSWE config path")
    );

    let empty_env_key = parse_denovo_condition("stateful:on,subagent:off,env:=value")
        .expect_err("empty env key should fail");
    assert!(
        empty_env_key
            .to_string()
            .contains("empty DeNovoSWE env key")
    );
}

#[test]
fn denovo_comparison_reports_duplicate_missing_and_mismatched_axes() {
    let baseline = build_denovo_condition_report(
        "baseline",
        DeNovoCondition::new(false, false),
        vec![serde_json::from_str(r#"{"instance_id":"a","success":true,"score":0.5}"#).unwrap()],
        1000,
        None,
    );
    let mut duplicate_baseline = build_denovo_condition_report(
        "duplicate-baseline",
        DeNovoCondition::new(false, false),
        vec![serde_json::from_str(r#"{"instance_id":"a","success":true,"score":0.9}"#).unwrap()],
        1000,
        None,
    );
    duplicate_baseline.condition_id = "wrong-condition-id".to_string();
    let stateful = build_denovo_condition_report(
        "stateful",
        DeNovoCondition::new(true, false),
        vec![serde_json::from_str(r#"{"instance_id":"a","success":true,"score":0.8}"#).unwrap()],
        1500,
        None,
    );
    let combined = build_denovo_condition_report(
        "combined",
        DeNovoCondition::new(true, true),
        vec![serde_json::from_str(r#"{"instance_id":"a","success":true,"score":0.95}"#).unwrap()],
        1800,
        None,
    );

    let comparison = compare_denovo_reports(vec![baseline, duplicate_baseline, stateful, combined]);

    assert_eq!(
        comparison.duplicate_axis_ids,
        vec!["stateful-off_subagent-off"]
    );
    assert_eq!(
        comparison.missing_axis_ids,
        vec!["stateful-off_subagent-on"]
    );
    assert_eq!(
        comparison.condition_id_mismatches,
        vec!["wrong-condition-id != stateful-off_subagent-off"]
    );
    assert_eq!(comparison.stateful_score_delta_without_subagent, None);
    assert_eq!(comparison.subagent_score_delta_without_stateful, None);
    assert_eq!(comparison.combined_interaction_score_delta, None);
}

#[test]
fn denovo_extract_command_uses_official_extract_patch_recipe() {
    let command = build_denovo_extract_recipe_command(DeNovoExtractRecipeOptions {
        aweagent_root: "../AweAgent".into(),
        python: "python3".to_string(),
        input: "ready_denovoswe.jsonl".into(),
        output: ".stateful_bench/denovo/extracts".into(),
        config: "configs/tasks/denovoswe.yaml".into(),
        max_concurrent: Some(10),
        instance_ids: vec!["PyCQA_pep8_pr970".to_string()],
        dry_run: true,
        del_done_images: true,
        no_extract_package_info: true,
    })
    .expect("extract command should build");

    assert_eq!(command.program, "python3");
    assert_eq!(command.cwd, std::path::PathBuf::from("../AweAgent"));
    assert_eq!(command.args[0], "recipes/denovo_swe/extract_patch.py");
    assert!(command.args.contains(&"--input".to_string()));
    assert!(command.args.contains(&"ready_denovoswe.jsonl".to_string()));
    assert!(command.args.contains(&"--dry-run".to_string()));
    assert!(command.args.contains(&"--del-done-images".to_string()));
    assert!(
        command
            .args
            .contains(&"--no-extract-package-info".to_string())
    );
}

#[test]
fn denovo_run_command_uses_official_run_recipe_and_condition_config() {
    let mut condition = DeNovoCondition::new(true, false);
    condition.config_path = Some("configs/tasks/denovoswe-stateful.yaml".into());
    condition
        .env
        .insert("STATEFUL_HOME".to_string(), "/tmp/stateful".to_string());

    let command = build_denovo_run_recipe_command(DeNovoRunRecipeOptions {
        aweagent_root: "../AweAgent".into(),
        python: "python3".to_string(),
        data_file: "denovoswe_with_patches.jsonl".into(),
        output: ".stateful_bench/denovo/runs/dev/official".into(),
        base_config: "configs/tasks/denovoswe.yaml".into(),
        condition,
        mode: DeNovoRunMode::Batch,
        instance_ids: vec!["PyCQA_pep8_pr970".to_string()],
        llm_config: Some("configs/llm/openai.yaml".into()),
        model: Some("gpt-5".to_string()),
        max_steps: Some(500),
        max_concurrent: Some(4),
        search_override: Some(false),
        skip_eval: false,
        validate_run: true,
        eval_iters: 2,
        del_done_images: true,
        dump_clean_snapshot: Some("snapshots.jsonl".into()),
        prompt_version: "v2".to_string(),
        verbose: true,
    })
    .expect("run command should build");

    assert_eq!(command.program, "python3");
    assert_eq!(command.cwd, std::path::PathBuf::from("../AweAgent"));
    assert_eq!(command.args[0], "recipes/denovo_swe/run.py");
    assert!(
        command
            .args
            .windows(2)
            .any(|pair| pair == ["--config", "configs/tasks/denovoswe-stateful.yaml"])
    );
    assert!(
        command
            .args
            .windows(2)
            .any(|pair| pair == ["--data-file", "denovoswe_with_patches.jsonl"])
    );
    assert!(
        command
            .args
            .windows(2)
            .any(|pair| pair == ["--mode", "batch"])
    );
    assert!(
        command
            .args
            .windows(2)
            .any(|pair| pair == ["--eval-iters", "2"])
    );
    assert!(command.args.contains(&"--no-search".to_string()));
    assert!(command.args.contains(&"--validate-run".to_string()));
    assert_eq!(
        command.env.get("STATEFUL_HOME").map(String::as_str),
        Some("/tmp/stateful")
    );
}
