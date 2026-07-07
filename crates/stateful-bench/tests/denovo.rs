use std::{collections::BTreeMap, fs, path::Path, process::Command};

use stateful_bench::{
    DeNovoAgentDockerSandbox, DeNovoAgentKind, DeNovoCliRuntime, DeNovoCodexRunOptions,
    DeNovoComparisonReport, DeNovoCondition, DeNovoConditionRunOptions, DeNovoExtractOptions,
    DeNovoExtractRecipeOptions, DeNovoMatrixRunOptions, DeNovoOfficialResult, DeNovoRunMode,
    DeNovoRunRecipeOptions, build_denovo_codex_adapter_command, build_denovo_condition_report,
    build_denovo_extract_recipe_command, build_denovo_run_recipe_command, compare_denovo_reports,
    default_denovo_conditions, parse_denovo_condition, render_denovo_report_markdown,
    run_denovo_condition, run_denovo_extract, run_denovo_matrix,
};

#[test]
fn denovo_condition_parser_accepts_axes_config_and_env() {
    let condition = parse_denovo_condition(
        "stateful:on,subagent:off,config:configs/tasks/denovoswe-stateful.yaml,env:STATEFUL_HOME=target/stateful-home,env:MODE=stateful",
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
        Some("target/stateful-home")
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
fn instance_ids_filter_by_min_measured_files() {
    let root = temp_root("stateful-bench-denovo-min-measured-files");
    fs::create_dir_all(&root).expect("temp root should exist");
    let file = root.join("denovo.jsonl");
    fs::write(
        &file,
        [
            r#"{"instance_id":"one_file","measured_files_total":1}"#,
            r#"{"instance_id":"many_files","measured_files_total":4}"#,
            "",
        ]
        .join("\n"),
    )
    .expect("data file should be written");

    let ids = stateful_bench::denovo::denovo_matrix_instance_ids(
        &file,
        &[],
        DeNovoRunMode::Batch,
        Some(3),
    )
    .expect("ids should parse");

    assert_eq!(ids, vec!["many_files".to_string()]);
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
          "subagent_used": true,
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
    assert_eq!(result.subagent_used, Some(true));
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
            r#"{"instance_id":"a","success":true,"score":1.0,"subagent_used":true,"token_usage":{"turns":2,"input_tokens":100,"cached_input_tokens":40,"output_tokens":10,"reasoning_output_tokens":3},"eval_result":{"details":{"pass_rate":1.0}},"orchestration_trace":{"trace_captured":true,"reservation_events":2,"claim_events":1,"conflict_events":1,"event_count":6,"event_types":{"SessionHeartbeat":4,"AuthorizationDenied":1,"ReservationDeclared":1},"heartbeat_events":4,"heartbeat_windows":2,"heartbeat_max_gap_ms":40000,"denial_events":1,"denial_paths":{"src/pkg.py":1},"denial_messages":{"Target existence changed since the supplied base observation.":1}}}"#,
        )
        .expect("result a"),
        serde_json::from_str::<DeNovoOfficialResult>(
            r#"{"instance_id":"b","success":false,"score":0.75,"subagent_used":false,"token_usage":{"turns":1,"input_tokens":50,"cached_input_tokens":20,"output_tokens":5,"reasoning_output_tokens":2},"eval_result":{"details":{"pass_rate":0.5}}}"#,
        )
        .expect("result b"),
        serde_json::from_str::<DeNovoOfficialResult>(
            r#"{"instance_id":"c","success":false,"error":"agent failed"}"#,
        )
        .expect("result c"),
        serde_json::from_str::<DeNovoOfficialResult>(
            r#"{"instance_id":"d","success":false,"score":0.875,"token_usage":{"input_plus_output_tokens":35,"uncached_input_tokens":20,"uncached_input_plus_output_tokens":25}}"#,
        )
        .expect("result d"),
    ];

    let report = build_denovo_condition_report(
        "denovo-dev",
        condition,
        results,
        9000,
        Some("abc123".to_string()),
    );

    assert_eq!(report.condition_id, "stateful-on_subagent-off");
    assert_eq!(report.total_instances, 4);
    assert_eq!(report.completed_instances, 3);
    assert_eq!(report.scored_instances, 3);
    assert_eq!(report.pass_rate_instances, 2);
    assert_eq!(report.success_count, 1);
    assert_eq!(report.error_count, 1);
    assert_eq!(report.success_rate, Some(0.25));
    assert_eq!(report.average_score, Some(0.875));
    assert_eq!(report.average_pass_rate, Some(0.75));
    assert_eq!(report.correct_rate, Some(0.25));
    assert_eq!(report.almost_correct_rate, Some(0.5));
    assert_eq!(report.running_time_ms, 9000);
    assert_eq!(report.average_running_time_ms, Some(2250.0));
    assert_eq!(report.subagent_observed_instances, 2);
    assert_eq!(report.subagent_used_count, 1);
    assert_eq!(report.subagent_used_rate, Some(0.5));
    assert_eq!(report.token_observed_instances, 3);
    assert_eq!(report.token_usage_turns, 3);
    assert_eq!(report.token_input_tokens, 150);
    assert_eq!(report.token_cached_input_tokens, 60);
    assert_eq!(report.token_output_tokens, 15);
    assert_eq!(report.token_reasoning_output_tokens, 5);
    assert_eq!(report.token_input_plus_output_tokens, 200);
    assert_eq!(report.token_uncached_input_tokens, 110);
    assert_eq!(report.token_uncached_input_plus_output_tokens, 130);
    assert_eq!(report.average_input_plus_output_tokens, Some(66.667));
    assert_eq!(
        report.average_uncached_input_plus_output_tokens,
        Some(43.333)
    );
    assert_eq!(
        report.score_per_million_input_plus_output_tokens,
        Some(4375.000)
    );
    assert_eq!(
        report.score_per_million_uncached_input_plus_output_tokens,
        Some(6730.769)
    );
    assert_eq!(report.score_per_hour, Some(350.000));
    assert_eq!(report.orchestration_trace_observed, 1);
    assert_eq!(report.orchestration_trace_captured, 1);
    assert_eq!(report.orchestration_reservation_events, 2);
    assert_eq!(report.orchestration_claim_events, 1);
    assert_eq!(report.orchestration_conflict_events, 1);
    assert_eq!(report.orchestration_event_count, 6);
    assert_eq!(report.orchestration_event_types["SessionHeartbeat"], 4);
    assert_eq!(report.orchestration_heartbeat_events, 4);
    assert_eq!(report.orchestration_heartbeat_windows, 2);
    assert_eq!(report.orchestration_heartbeat_max_gap_ms, Some(40000));
    assert_eq!(report.orchestration_denial_events, 1);
    assert_eq!(report.orchestration_denial_paths["src/pkg.py"], 1);
    assert_eq!(
        report.orchestration_denial_messages["Target existence changed since the supplied base observation."],
        1
    );


    let zero_denominator_report = build_denovo_condition_report(
        "denovo-dev",
        DeNovoCondition {
            stateful: true,
            subagent: false,
            config_path: Some("configs/tasks/denovoswe-stateful.yaml".into()),
            env: BTreeMap::new(),
        },
        vec![
            serde_json::from_str::<DeNovoOfficialResult>(
                r#"{"instance_id":"no-usage","success":false,"score":0.5}"#,
            )
            .expect("zero denominator result"),
        ],
        0,
        None,
    );
    assert_eq!(
        zero_denominator_report.score_per_million_input_plus_output_tokens,
        None
    );
    assert_eq!(
        zero_denominator_report.score_per_million_uncached_input_plus_output_tokens,
        None
    );
    assert_eq!(zero_denominator_report.score_per_hour, None);
}

#[test]
fn denovo_report_aggregates_orchestration_trace_value_fields() {
    let result = serde_json::from_str::<DeNovoOfficialResult>(
        r#"{"instance_id":"a","success":true,"score":1.0,"orchestration_trace":{"trace_captured":true,"true_collisions_prevented":2,"self_inflicted_denials":5,"scope_overlap_warnings":3}}"#,
    )
    .expect("result");

    let report = build_denovo_condition_report(
        "denovo-dev",
        DeNovoCondition::new(true, false),
        vec![result],
        1000,
        None,
    );

    assert_eq!(report.orchestration_true_collisions_prevented, 2);
    assert_eq!(report.orchestration_self_inflicted_denials, 5);
    assert_eq!(report.orchestration_scope_overlap_warnings, 3);

    let markdown = render_denovo_report_markdown(&[report]);
    assert!(markdown.contains("True collisions prevented"));
    assert!(markdown.contains("Self-inflicted denials"));
    assert!(markdown.contains("Scope overlap warnings"));
    assert!(markdown.contains("| stateful-on_subagent-off | on | off | 1 | 1.000 | 1.000"));
    assert!(markdown.contains("| 2 | 5 | 3 |\n"));
}

#[test]
fn denovo_condition_report_aggregates_observed_agent_time_separately_from_elapsed_runtime() {
    let report = build_denovo_condition_report(
        "denovo-dev",
        DeNovoCondition::new(true, false),
        vec![
            serde_json::from_str::<DeNovoOfficialResult>(
                r#"{"instance_id":"a","success":true,"score":1.0,"agent_running_time_ms":1000}"#,
            )
            .expect("result a"),
            serde_json::from_str::<DeNovoOfficialResult>(
                r#"{"instance_id":"b","success":false,"score":0.5,"agent_running_time_ms":3000}"#,
            )
            .expect("result b"),
            serde_json::from_str::<DeNovoOfficialResult>(
                r#"{"instance_id":"c","success":false,"score":0.0}"#,
            )
            .expect("result c"),
        ],
        9000,
        None,
    );

    assert_eq!(report.agent_running_time_ms, Some(4000));
    assert_eq!(report.average_agent_running_time_ms, Some(2000.0));
    assert_eq!(report.score_per_agent_hour, Some(450.0));
    assert_eq!(report.running_time_ms, 9000);
    assert_eq!(report.score_per_hour, Some(200.0));
}

#[test]
fn denovo_condition_report_leaves_agent_time_absent_for_historical_results() {
    let result = serde_json::from_str::<DeNovoOfficialResult>(
        r#"{"instance_id":"historical","success":true,"score":0.5}"#,
    )
    .expect("historical result");
    assert_eq!(result.agent_running_time_ms, None);

    let report = build_denovo_condition_report(
        "denovo-dev",
        DeNovoCondition::new(false, false),
        vec![result],
        7200,
        None,
    );

    assert_eq!(report.agent_running_time_ms, None);
    assert_eq!(report.average_agent_running_time_ms, None);
    assert_eq!(report.score_per_agent_hour, None);
    assert_eq!(report.running_time_ms, 7200);
    assert_eq!(report.score_per_hour, Some(250.0));

    let serialized = serde_json::to_value(&report).expect("report should serialize");
    assert!(serialized.get("agent_running_time_ms").is_none());
    assert!(serialized.get("average_agent_running_time_ms").is_none());
    assert!(serialized.get("score_per_agent_hour").is_none());
    assert_eq!(serialized["running_time_ms"], serde_json::json!(7200));
    assert_eq!(serialized["score_per_hour"], serde_json::json!(250.0));
}

#[test]
fn denovo_comparison_reports_agent_time_deltas_only_for_observed_pairs() {
    let off_off =
        build_denovo_condition_report(
            "baseline",
            DeNovoCondition::new(false, false),
            vec![serde_json::from_str(
            r#"{"instance_id":"a","success":true,"score":0.5,"agent_running_time_ms":1000}"#,
        )
        .expect("baseline result")],
            100,
            None,
        );
    let on_off =
        build_denovo_condition_report(
            "stateful",
            DeNovoCondition::new(true, false),
            vec![serde_json::from_str(
            r#"{"instance_id":"a","success":true,"score":0.8,"agent_running_time_ms":1500}"#,
        )
        .expect("stateful result")],
            200,
            None,
        );
    let off_on = build_denovo_condition_report(
        "subagent",
        DeNovoCondition::new(false, true),
        vec![
            serde_json::from_str(r#"{"instance_id":"a","success":true,"score":0.7}"#)
                .expect("subagent result"),
        ],
        300,
        None,
    );
    let on_on =
        build_denovo_condition_report(
            "combined",
            DeNovoCondition::new(true, true),
            vec![serde_json::from_str(
            r#"{"instance_id":"a","success":true,"score":0.9,"agent_running_time_ms":2400}"#,
        )
        .expect("combined result")],
            400,
            None,
        );

    let comparison = compare_denovo_reports(vec![off_off, on_off, off_on, on_on]);

    assert_eq!(comparison.total_agent_running_time_ms, Some(4900));
    assert_eq!(
        comparison.stateful_agent_running_time_ms_delta_without_subagent,
        Some(500)
    );
    assert_eq!(
        comparison.subagent_agent_running_time_ms_delta_without_stateful,
        None
    );
    assert_eq!(
        comparison.combined_interaction_agent_running_time_ms_delta,
        None
    );
    assert_eq!(comparison.total_running_time_ms, 1000);
}

#[test]
fn denovo_markdown_places_agent_time_efficiency_before_elapsed_efficiency() {
    let report =
        build_denovo_condition_report(
            "denovo-dev",
            DeNovoCondition::new(true, true),
            vec![serde_json::from_str::<DeNovoOfficialResult>(
            r#"{"instance_id":"a","success":true,"score":0.5,"agent_running_time_ms":2000}"#,
        )
        .expect("result")],
            3000,
            None,
        );

    let markdown = render_denovo_report_markdown(&[report]);
    let header = markdown
        .lines()
        .find(|line| line.starts_with("| Condition |"))
        .expect("markdown table header");

    let agent_time = header
        .find("Agent running time ms")
        .expect("agent running time column");
    let score_per_agent_hour = header
        .find("Score per agent hour")
        .expect("score per agent hour column");
    let elapsed_time = header
        .find("Running time ms")
        .expect("elapsed running time column");
    let score_per_elapsed_hour = header
        .find("Score per hour")
        .expect("score per hour column");

    assert!(agent_time < elapsed_time);
    assert!(score_per_agent_hour < elapsed_time);
    assert!(score_per_agent_hour < score_per_elapsed_hour);
}

#[test]
fn denovo_comparison_indexes_reports_by_condition_and_computes_deltas() {
    let off_off = build_denovo_condition_report(
        "baseline",
        DeNovoCondition::new(false, false),
        vec![
            serde_json::from_str(r#"{"instance_id":"a","success":true,"score":0.5}"#)
                .expect("denovo result fixture should parse"),
        ],
        1000,
        None,
    );
    let on_off = build_denovo_condition_report(
        "stateful",
        DeNovoCondition::new(true, false),
        vec![
            serde_json::from_str(r#"{"instance_id":"a","success":true,"score":0.8}"#)
                .expect("denovo result fixture should parse"),
        ],
        1500,
        None,
    );
    let off_on = build_denovo_condition_report(
        "subagent",
        DeNovoCondition::new(false, true),
        vec![
            serde_json::from_str(r#"{"instance_id":"a","success":true,"score":0.7}"#)
                .expect("denovo result fixture should parse"),
        ],
        1200,
        None,
    );
    let on_on = build_denovo_condition_report(
        "combined",
        DeNovoCondition::new(true, true),
        vec![
            serde_json::from_str(r#"{"instance_id":"a","success":true,"score":0.9}"#)
                .expect("denovo result fixture should parse"),
        ],
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
        vec![
            serde_json::from_str(r#"{"instance_id":"a","success":true,"score":0.5}"#)
                .expect("denovo result fixture should parse"),
        ],
        1000,
        None,
    );
    let mut duplicate_baseline = build_denovo_condition_report(
        "duplicate-baseline",
        DeNovoCondition::new(false, false),
        vec![
            serde_json::from_str(r#"{"instance_id":"a","success":true,"score":0.9}"#)
                .expect("denovo result fixture should parse"),
        ],
        1000,
        None,
    );
    duplicate_baseline.condition_id = "wrong-condition-id".to_string();
    let stateful = build_denovo_condition_report(
        "stateful",
        DeNovoCondition::new(true, false),
        vec![
            serde_json::from_str(r#"{"instance_id":"a","success":true,"score":0.8}"#)
                .expect("denovo result fixture should parse"),
        ],
        1500,
        None,
    );
    let combined = build_denovo_condition_report(
        "combined",
        DeNovoCondition::new(true, true),
        vec![
            serde_json::from_str(r#"{"instance_id":"a","success":true,"score":0.95}"#)
                .expect("denovo result fixture should parse"),
        ],
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
    condition.env.insert(
        "STATEFUL_HOME".to_string(),
        "target/stateful-home".to_string(),
    );

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
        Some("target/stateful-home")
    );
}

#[test]
fn denovo_codex_adapter_command_uses_stateful_adapter_and_condition_axes() {
    let command = build_denovo_codex_adapter_command(DeNovoCodexRunOptions {
        aweagent_root: "../AweAgent".into(),
        python: "python3".to_string(),
        data_file: "denovoswe_with_patches.jsonl".into(),
        output: "target/stateful-bench/denovo/runs/dev/codex-cli".into(),
        base_config: "configs/tasks/denovoswe.yaml".into(),
        condition: DeNovoCondition::new(true, true),
        mode: DeNovoRunMode::Batch,
        instance_ids: vec!["PyCQA_pep8_pr970".to_string()],
        max_steps: Some(500),
        max_concurrent: Some(1),
        skip_eval: false,
        validate_run: true,
        eval_iters: 1,
        del_done_images: false,
        dump_clean_snapshot: None,
        prompt_version: "v1".to_string(),
        verbose: true,
        codex_bin: "/opt/homebrew/bin/codex".to_string(),
        omp_bin: "omp".to_string(),
        stateful_binary: "/opt/stateful/bin/stateful".to_string(),
        agent_docker_image: None,
        agent_docker_stateful_binary: "/usr/local/bin/stateful".to_string(),
        agent_docker_sandbox: DeNovoAgentDockerSandbox::On,
        benchmark_model: "gpt-5.4-mini".to_string(),
        benchmark_reasoning_effort: "low".to_string(),
        benchmark_model_context_window: 256000,
        benchmark_temperature: "1".to_string(),
        benchmark_max_turns: 500,
        subagent_min_count: 4,
        max_resumes: 2,
        codex_timeout_seconds: 7200,
        adapter_script: Some("crates/stateful-bench/scripts/denovo_codex_agent.py".into()),
        cli_runtime: DeNovoCliRuntime::Codex,
    })
    .expect("codex adapter command should build");

    assert_eq!(command.program, "python3");
    assert_eq!(command.cwd, std::path::PathBuf::from("../AweAgent"));
    assert_eq!(
        command.args[0],
        "crates/stateful-bench/scripts/denovo_codex_agent.py"
    );
    assert!(
        command
            .args
            .windows(2)
            .any(|pair| pair == ["--agent-mode", "stateful"])
    );
    assert!(
        command
            .args
            .windows(2)
            .any(|pair| pair == ["--subagent", "on"])
    );
    assert!(
        command
            .args
            .windows(2)
            .any(|pair| pair == ["--benchmark-model-context-window", "256000"])
    );
    assert!(
        command
            .args
            .windows(2)
            .any(|pair| pair == ["--benchmark-temperature", "1"])
    );
    assert!(
        command
            .args
            .windows(2)
            .any(|pair| pair == ["--benchmark-max-turns", "500"])
    );
    assert!(
        command
            .args
            .windows(2)
            .any(|pair| pair == ["--subagent-min-count", "4"])
    );
    assert!(
        command
            .args
            .windows(2)
            .any(|pair| pair == ["--instance-id", "PyCQA_pep8_pr970"])
    );
    assert!(command.args.contains(&"--validate-run".to_string()));
    assert!(command.args.contains(&"--verbose".to_string()));
}

#[test]
fn denovo_omp_adapter_command_uses_existing_adapter_with_omp_runtime() {
    let command = build_denovo_codex_adapter_command(DeNovoCodexRunOptions {
        aweagent_root: "../AweAgent".into(),
        python: "python3".to_string(),
        data_file: "denovoswe_with_patches.jsonl".into(),
        output: "target/stateful-bench/denovo/runs/dev/omp-cli".into(),
        base_config: "configs/tasks/denovoswe.yaml".into(),
        condition: DeNovoCondition::new(true, true),
        mode: DeNovoRunMode::Batch,
        instance_ids: vec!["PyCQA_pep8_pr970".to_string()],
        max_steps: Some(500),
        max_concurrent: Some(1),
        skip_eval: false,
        validate_run: true,
        eval_iters: 1,
        del_done_images: false,
        dump_clean_snapshot: None,
        prompt_version: "v2".to_string(),
        verbose: true,
        codex_bin: "/opt/homebrew/bin/codex".to_string(),
        omp_bin: "/opt/homebrew/bin/omp".to_string(),
        stateful_binary: "/opt/stateful/bin/stateful".to_string(),
        agent_docker_image: Some("ghcr.io/stateful/omp-agent:latest".to_string()),
        agent_docker_stateful_binary: "/usr/local/bin/stateful".to_string(),
        agent_docker_sandbox: DeNovoAgentDockerSandbox::Off,
        benchmark_model: "deepseek-v4-flash".to_string(),
        benchmark_reasoning_effort: "low".to_string(),
        benchmark_model_context_window: 256000,
        benchmark_temperature: "1".to_string(),
        benchmark_max_turns: 500,
        subagent_min_count: 4,
        max_resumes: 2,
        codex_timeout_seconds: 7200,
        adapter_script: Some("crates/stateful-bench/scripts/denovo_codex_agent.py".into()),
        cli_runtime: DeNovoCliRuntime::Omp,
    })
    .expect("omp adapter command should build");

    assert_eq!(command.program, "python3");
    assert!(
        command
            .args
            .windows(2)
            .any(|pair| pair == ["--cli-runtime", "omp"])
    );
    assert!(
        command
            .args
            .windows(2)
            .any(|pair| pair == ["--omp-bin", "/opt/homebrew/bin/omp"])
    );
    assert!(
        command
            .args
            .windows(2)
            .any(|pair| pair == ["--benchmark-model", "deepseek-v4-flash"])
    );
    assert!(
        command
            .args
            .windows(2)
            .any(|pair| pair == ["--agent-mode", "stateful"])
    );
    assert!(
        command
            .args
            .windows(2)
            .any(|pair| pair == ["--subagent", "on"])
    );
    assert!(
        command
            .args
            .windows(2)
            .any(|pair| pair == ["--agent-docker-image", "ghcr.io/stateful/omp-agent:latest"])
    );
    assert!(
        command
            .args
            .windows(2)
            .any(|pair| pair == ["--agent-docker-stateful-binary", "/usr/local/bin/stateful"])
    );
    assert!(
        command
            .args
            .windows(2)
            .any(|pair| pair == ["--agent-docker-sandbox", "off"])
    );
}

#[test]
fn denovo_condition_run_executes_fake_recipe_and_writes_metadata() {
    let root = temp_root("stateful-bench-denovo-fake-recipe");
    let aweagent = root.join("AweAgent");
    let recipe_dir = aweagent.join("recipes/denovo_swe");
    fs::create_dir_all(&recipe_dir).expect("recipe dir should exist");
    fs::write(
        recipe_dir.join("run.py"),
        r#"#!/usr/bin/env python3
import argparse
import json
from pathlib import Path
parser = argparse.ArgumentParser()
parser.add_argument("--data-file")
parser.add_argument("--config")
parser.add_argument("--mode")
parser.add_argument("--output")
parser.add_argument("--eval-iters")
parser.add_argument("--prompt-version")
args, extra = parser.parse_known_args()
out = Path(args.output) / "_"
out.mkdir(parents=True, exist_ok=True)
(out / "results.jsonl").write_text(json.dumps({"instance_id":"fake-a","success":True,"score":1.0,"eval_result":{"details":{"pass_rate":1.0}}}) + "\n")
(out / "run_config.json").write_text(json.dumps({"mode": args.mode, "extra": extra}))
"#,
    )
    .expect("fake run.py should write");

    let run_dir = root.join("runs/dev-denovo");
    let mut condition = DeNovoCondition::new(true, false);
    condition.config_path = Some("configs/tasks/denovoswe-stateful.yaml".into());

    let metadata = run_denovo_condition(DeNovoConditionRunOptions {
        run_id: "dev-denovo".to_string(),
        aweagent_root: aweagent,
        python: "python3".to_string(),
        data_file: "denovoswe_with_patches.jsonl".into(),
        run_dir: run_dir.clone(),
        base_config: "configs/tasks/denovoswe.yaml".into(),
        condition,
        agent: DeNovoAgentKind::Official,
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
        codex_adapter_script: None,
        mode: DeNovoRunMode::Batch,
        instance_ids: Vec::new(),
        llm_config: None,
        model: None,
        max_steps: None,
        max_concurrent: None,
        search_override: None,
        skip_eval: false,
        validate_run: false,
        eval_iters: 1,
        del_done_images: false,
        dump_clean_snapshot: None,
        prompt_version: "v2".to_string(),
        verbose: false,
    })
    .expect("condition should run");

    assert_eq!(metadata.condition_id, "stateful-on_subagent-off");
    assert_eq!(metadata.agent, DeNovoAgentKind::Official);
    assert!(metadata.running_time_ms > 0);
    assert!(
        run_dir
            .join("conditions/stateful-on_subagent-off/condition.json")
            .is_file()
    );
    assert!(
        run_dir
            .join("conditions/stateful-on_subagent-off/denovo-report.json")
            .is_file()
    );
    assert!(
        run_dir
            .join("conditions/stateful-on_subagent-off/official/_/results.jsonl")
            .is_file()
    );

    fs::remove_dir_all(root).expect("temp root should clean up");
}

#[test]
fn denovo_codex_agent_git_diff_ignores_gitignored_pytest_cache() {
    let root = temp_root("stateful-bench-denovo-git-diff-ignored-cache");
    let workspace = root.join("workspace");
    fs::create_dir_all(&workspace).expect("workspace should exist");

    run_git(&workspace, &["init"]);
    run_git(&workspace, &["config", "user.email", "codex@example.com"]);
    run_git(&workspace, &["config", "user.name", "Codex"]);
    fs::write(workspace.join(".gitignore"), ".pytest_cache/\n")
        .expect("gitignore should be written");
    fs::write(workspace.join("tracked.py"), "before\n").expect("tracked file should be written");
    run_git(&workspace, &["add", "."]);
    run_git(&workspace, &["commit", "-m", "initial"]);

    fs::write(workspace.join("tracked.py"), "after\n").expect("tracked file should be modified");
    fs::create_dir_all(workspace.join(".pytest_cache")).expect("pytest cache dir should exist");
    fs::write(workspace.join(".pytest_cache/README"), "cache\n")
        .expect("ignored cache file should be written");

    let adapter_script =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("scripts/denovo_codex_agent.py");
    let output = Command::new("python3")
        .arg("-c")
        .arg(
            r#"
import importlib.util
import pathlib
import sys

script = pathlib.Path(sys.argv[1])
workspace = pathlib.Path(sys.argv[2])
spec = importlib.util.spec_from_file_location("denovo_codex_agent", script)
module = importlib.util.module_from_spec(spec)
assert spec.loader is not None
sys.modules[spec.name] = module
spec.loader.exec_module(module)
print(module.git_diff(workspace))
"#,
        )
        .arg(adapter_script)
        .arg(&workspace)
        .output()
        .expect("python should run git_diff");

    assert!(
        output.status.success(),
        "git_diff should ignore gitignored pytest cache\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let patch = String::from_utf8(output.stdout).expect("patch should be utf8");
    assert!(patch.contains("tracked.py"));
    assert!(patch.contains("+after"));
    assert!(!patch.contains(".pytest_cache"));

    fs::remove_dir_all(root).expect("temp root should clean up");
}

#[test]
fn denovo_codex_agent_installs_target_proxy_for_docker_omp_runs() {
    let adapter_script =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("scripts/denovo_codex_agent.py");
    let output = Command::new("python3")
        .arg("-c")
        .arg(
            r#"
import importlib.util
import json
import pathlib
import sys
import socket

script = pathlib.Path(sys.argv[1])
spec = importlib.util.spec_from_file_location("denovo_codex_agent", script)
module = importlib.util.module_from_spec(spec)
assert spec.loader is not None
sys.modules[spec.name] = module
spec.loader.exec_module(module)
patterns = json.loads(module.benchmark_source_block_patterns_for_env("rushousley_pyasn1-alt-modules_pr92"))
url_patterns = module.benchmark_source_leak_url_patterns("rushousley_pyasn1-alt-modules_pr92")
proxy = module.start_target_upstream_deny_proxy("rushousley_pyasn1-alt-modules_pr92")
assert proxy is not None
try:
    port = int(proxy.url.rsplit(":", 1)[1])
    with socket.create_connection(("127.0.0.1", port), timeout=2) as sock:
        sock.sendall(b"CONNECT github.com:443 HTTP/1.1\r\nHost: github.com:443\r\n\r\n")
        connect_status = sock.recv(1024).decode("iso-8859-1").splitlines()[0]
    with socket.create_connection(("127.0.0.1", port), timeout=2) as sock:
        sock.sendall(b"GET http://raw.githubusercontent.com/rushousley/pyasn1-alt-modules/main/README.md HTTP/1.1\r\nHost: raw.githubusercontent.com\r\n\r\n")
        raw_status = sock.recv(1024).decode("iso-8859-1").splitlines()[0]
finally:
    proxy.close()
print(json.dumps({
    "docker": module.target_upstream_proxy_required("stateful-denovo-omp-agent:local"),
    "local": module.target_upstream_proxy_required(None),
    "patterns": patterns,
    "connect_hosts": list(module.BENCHMARK_SOURCE_LEAK_CONNECT_HOSTS),
    "command_upstream": module.benchmark_source_leak_command_pattern("git fetch upstream main", ()),
    "command_target_url": module.benchmark_source_leak_command_pattern(
        "git clone https://github.com/rushousley/pyasn1-alt-modules.git",
        url_patterns,
    ),
    "connect_status": connect_status,
    "raw_status": raw_status,
}))
"#,
        )
        .arg(adapter_script)
        .output()
        .expect("python should run target proxy check");

    assert!(
        output.status.success(),
        "target proxy check should run\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let decision: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("proxy decision should be json");
    assert_eq!(decision["docker"], serde_json::Value::Bool(true));
    assert_eq!(decision["local"], serde_json::Value::Bool(false));
    let patterns = decision["patterns"]
        .as_array()
        .expect("proxy patterns should be an array");
    assert!(
        patterns
            .iter()
            .any(|pattern| pattern.as_str().is_some_and(|pattern| pattern
                .contains("raw.githubusercontent.com/rushousley/pyasn1-alt-modules")))
    );
    assert!(
        patterns
            .iter()
            .any(|pattern| pattern.as_str() == Some("upstream/"))
    );
    let connect_hosts = decision["connect_hosts"]
        .as_array()
        .expect("connect hosts should be an array");
    assert!(
        connect_hosts
            .iter()
            .any(|host| host.as_str() == Some("github.com"))
    );
    assert!(
        connect_hosts
            .iter()
            .any(|host| host.as_str() == Some("api.github.com"))
    );
    assert!(
        decision["connect_status"]
            .as_str()
            .is_some_and(|status| status.contains("403")),
        "CONNECT to a target host should be denied"
    );
    assert!(
        decision["raw_status"]
            .as_str()
            .is_some_and(|status| status.contains("403")),
        "absolute-form HTTP target source URLs should be denied"
    );
    assert_eq!(decision["command_upstream"], "git fetch");
    assert_eq!(decision["command_target_url"], "git clone");
}

#[test]
fn denovo_condition_run_routes_codex_cli_to_adapter_and_writes_metadata() {
    let root = temp_root("stateful-bench-denovo-codex-cli-adapter");
    let aweagent = root.join("AweAgent");
    fs::create_dir_all(&aweagent).expect("AweAgent dir should exist");
    let adapter = root.join("denovo_codex_agent.py");
    fs::write(
        &adapter,
        r#"#!/usr/bin/env python3
import argparse
import json
from pathlib import Path
parser = argparse.ArgumentParser(allow_abbrev=False)
parser.add_argument("--output")
parser.add_argument("--agent-mode")
parser.add_argument("--subagent")
parser.add_argument("--llm-config")
parser.add_argument("--model")
args, extra = parser.parse_known_args()
out = Path(args.output) / "_"
out.mkdir(parents=True, exist_ok=True)
(out / "results.jsonl").write_text(json.dumps({"instance_id":"fake-a","success":True,"score":1.0,"finish_reason":"fake","eval_result":{"details":{"pass_rate":1.0}}}) + "\n")
(out / "adapter_args.json").write_text(json.dumps({"agent_mode": args.agent_mode, "subagent": args.subagent, "llm_config": args.llm_config, "model": args.model, "extra": extra}))
"#,
    )
    .expect("fake adapter should write");

    let run_dir = root.join("runs/dev-denovo-codex");
    let metadata = run_denovo_condition(DeNovoConditionRunOptions {
        run_id: "dev-denovo-codex".to_string(),
        aweagent_root: aweagent.clone(),
        python: "python3".to_string(),
        data_file: "denovoswe_with_patches.jsonl".into(),
        run_dir: run_dir.clone(),
        base_config: "configs/tasks/denovoswe.yaml".into(),
        condition: DeNovoCondition::new(false, true),
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
        codex_adapter_script: Some(adapter),
        mode: DeNovoRunMode::Batch,
        instance_ids: Vec::new(),
        llm_config: Some("configs/llm/should-not-be-forwarded.yaml".into()),
        model: Some("should-not-be-forwarded".to_string()),
        max_steps: None,
        max_concurrent: None,
        search_override: None,
        skip_eval: false,
        validate_run: false,
        eval_iters: 1,
        del_done_images: false,
        dump_clean_snapshot: None,
        prompt_version: "v2".to_string(),
        verbose: false,
    })
    .expect("Codex CLI adapter should run");

    assert_eq!(metadata.agent, DeNovoAgentKind::CodexCli);
    assert_eq!(metadata.condition_id, "stateful-off_subagent-on");
    assert_eq!(metadata.command.cwd, aweagent);
    assert!(metadata.official_dir.ends_with("codex-cli"));
    assert!(
        run_dir
            .join("conditions/stateful-off_subagent-on/codex-cli/_/results.jsonl")
            .is_file()
    );
    assert!(
        run_dir
            .join("conditions/stateful-off_subagent-on/denovo-report.json")
            .is_file()
    );
    let result: DeNovoOfficialResult = serde_json::from_str(
        fs::read_to_string(
            run_dir.join("conditions/stateful-off_subagent-on/codex-cli/_/results.jsonl"),
        )
        .expect("results jsonl should exist")
        .lines()
        .next()
        .expect("results jsonl should contain a row"),
    )
    .expect("fake result should parse");
    assert_eq!(
        result.extra.get("finish_reason"),
        Some(&serde_json::Value::String("fake".to_string()))
    );
    assert_eq!(
        result
            .eval_result
            .as_ref()
            .and_then(|eval| eval.details.as_ref())
            .and_then(|details| details.pass_rate),
        Some(1.0)
    );
    let adapter_args: serde_json::Value = serde_json::from_str(
        &fs::read_to_string(
            run_dir.join("conditions/stateful-off_subagent-on/codex-cli/_/adapter_args.json"),
        )
        .expect("adapter args should exist"),
    )
    .expect("adapter args should parse");
    assert_eq!(adapter_args["agent_mode"], "no-state");
    assert_eq!(adapter_args["subagent"], "on");
    assert_eq!(adapter_args["llm_config"], serde_json::Value::Null);
    assert_eq!(adapter_args["model"], serde_json::Value::Null);

    fs::remove_dir_all(root).expect("temp root should clean up");
}

#[test]
fn denovo_condition_run_resolves_relative_codex_adapter_script_from_caller_cwd() {
    let root = temp_root("stateful-bench-denovo-relative-adapter");
    if root.exists() {
        fs::remove_dir_all(&root).expect("old temp root should clean up");
    }
    let aweagent = root.join("AweAgent");
    fs::create_dir_all(&aweagent).expect("AweAgent dir should exist");
    let adapter = root.join("relative_denovo_adapter.py");
    fs::write(
        &adapter,
        r#"#!/usr/bin/env python3
import argparse
import json
from pathlib import Path
parser = argparse.ArgumentParser(allow_abbrev=False)
parser.add_argument("--output")
parser.add_argument("--agent-mode")
parser.add_argument("--subagent")
args, extra = parser.parse_known_args()
out = Path(args.output) / "_"
out.mkdir(parents=True, exist_ok=True)
(out / "results.jsonl").write_text(json.dumps({"instance_id":"fake-relative","success":True,"score":1.0,"finish_reason":"fake","eval_result":{"details":{"pass_rate":1.0}}}) + "\n")
"#,
    )
    .expect("fake relative adapter should write");

    let run_dir = root.join("runs/dev-denovo-codex-relative");
    let metadata = run_denovo_condition(DeNovoConditionRunOptions {
        run_id: "dev-denovo-codex-relative".to_string(),
        aweagent_root: aweagent.clone(),
        python: "python3".to_string(),
        data_file: "denovoswe_with_patches.jsonl".into(),
        run_dir: run_dir.clone(),
        base_config: "configs/tasks/denovoswe.yaml".into(),
        condition: DeNovoCondition::new(false, true),
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
        codex_adapter_script: Some(adapter.clone()),
        mode: DeNovoRunMode::Batch,
        instance_ids: Vec::new(),
        llm_config: None,
        model: None,
        max_steps: None,
        max_concurrent: None,
        search_override: None,
        skip_eval: false,
        validate_run: false,
        eval_iters: 1,
        del_done_images: false,
        dump_clean_snapshot: None,
        prompt_version: "v2".to_string(),
        verbose: false,
    })
    .expect("relative Codex CLI adapter should run from caller cwd");

    assert_eq!(metadata.command.cwd, aweagent);
    assert!(
        Path::new(&metadata.command.args[0]).is_absolute(),
        "adapter script path should be absolutized before changing cwd"
    );
    assert!(
        run_dir
            .join("conditions/stateful-off_subagent-on/denovo-report.json")
            .is_file()
    );

    fs::remove_dir_all(root).expect("temp root should clean up");
}

#[test]
fn denovo_condition_run_reports_neutral_command_failure_for_codex_adapter() {
    let root = temp_root("stateful-bench-denovo-codex-cli-failure");
    let aweagent = root.join("AweAgent");
    fs::create_dir_all(&aweagent).expect("AweAgent dir should exist");
    let adapter = root.join("denovo_codex_agent.py");
    fs::write(
        &adapter,
        r#"#!/usr/bin/env python3
import sys
print("adapter stdout before failure")
print("adapter stderr before failure", file=sys.stderr)
sys.exit(2)
"#,
    )
    .expect("fake adapter should write");

    let run_dir = root.join("runs/dev-denovo-codex-failure");
    let error = run_denovo_condition(DeNovoConditionRunOptions {
        run_id: "dev-denovo-codex-failure".to_string(),
        aweagent_root: aweagent,
        python: "python3".to_string(),
        data_file: "denovoswe_with_patches.jsonl".into(),
        run_dir: run_dir.clone(),
        base_config: "configs/tasks/denovoswe.yaml".into(),
        condition: DeNovoCondition::new(false, true),
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
        codex_adapter_script: Some(adapter),
        mode: DeNovoRunMode::Batch,
        instance_ids: Vec::new(),
        llm_config: None,
        model: None,
        max_steps: None,
        max_concurrent: None,
        search_override: None,
        skip_eval: false,
        validate_run: false,
        eval_iters: 1,
        del_done_images: false,
        dump_clean_snapshot: None,
        prompt_version: "v2".to_string(),
        verbose: false,
    })
    .expect_err("Codex adapter failure should propagate");

    let message = error.to_string();
    assert!(message.contains("DeNovoSWE command failed with status"));
    assert!(!message.contains("official DeNovoSWE recipe failed"));
    assert!(message.contains("command.stderr.log"));

    let condition_dir = run_dir.join("conditions/stateful-off_subagent-on");
    assert_eq!(
        fs::read_to_string(condition_dir.join("command.stdout.log"))
            .expect("stdout log should be written"),
        "adapter stdout before failure\n"
    );
    assert_eq!(
        fs::read_to_string(condition_dir.join("command.stderr.log"))
            .expect("stderr log should be written"),
        "adapter stderr before failure\n"
    );
    let metadata: serde_json::Value = serde_json::from_str(
        &fs::read_to_string(condition_dir.join("condition.json"))
            .expect("failure metadata should be written"),
    )
    .expect("failure metadata should parse");
    assert!(
        metadata["error"]
            .as_str()
            .expect("error should be recorded")
            .contains("command.stderr.log")
    );
    assert!(
        metadata["stdout_log"]
            .as_str()
            .expect("stdout log path should be recorded")
            .ends_with("command.stdout.log")
    );
    assert!(
        metadata["stderr_log"]
            .as_str()
            .expect("stderr log path should be recorded")
            .ends_with("command.stderr.log")
    );

    fs::remove_dir_all(root).expect("temp root should clean up");
}

#[test]
fn denovo_matrix_run_writes_condition_reports_and_comparison() {
    let root = temp_root("stateful-bench-denovo-matrix");
    let aweagent = root.join("AweAgent");
    let recipe_dir = aweagent.join("recipes/denovo_swe");
    fs::create_dir_all(&recipe_dir).expect("recipe dir should exist");
    fs::write(
        recipe_dir.join("run.py"),
        r#"#!/usr/bin/env python3
import argparse
import json
from pathlib import Path
parser = argparse.ArgumentParser()
parser.add_argument("--output")
parser.add_argument("--config")
args, extra = parser.parse_known_args()
out = Path(args.output) / "_"
out.mkdir(parents=True, exist_ok=True)
score = 1.0 if "stateful" in args.config else 0.5
(out / "results.jsonl").write_text(json.dumps({"instance_id":"fake-a","success":score == 1.0,"score":score,"eval_result":{"details":{"pass_rate":score}}}) + "\n")
"#,
    )
    .expect("fake run.py should write");

    let data_file = root.join("denovo.jsonl");
    fs::write(&data_file, "{\"instance_id\":\"fake-a\"}\n").expect("matrix data file should write");
    let run_dir = root.join("runs/dev-denovo");
    let reports = run_denovo_matrix(DeNovoMatrixRunOptions {
        run_id: "dev-denovo".to_string(),
        aweagent_root: aweagent,
        python: "python3".to_string(),
        data_file,
        run_dir: run_dir.clone(),
        base_config: "configs/tasks/denovoswe.yaml".into(),
        conditions: vec![
            parse_denovo_condition("stateful:off,subagent:off,config:configs/tasks/denovoswe.yaml")
                .expect("off condition fixture should parse"),
            parse_denovo_condition(
                "stateful:on,subagent:off,config:configs/tasks/denovoswe-stateful.yaml",
            )
            .expect("stateful condition fixture should parse"),
        ],
        agent: DeNovoAgentKind::Official,
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
        codex_adapter_script: None,
        mode: DeNovoRunMode::Batch,
        instance_ids: Vec::new(),
        llm_config: None,
        model: None,
        max_steps: None,
        max_concurrent: None,
        min_measured_files: None,
        search_override: None,
        skip_eval: false,
        validate_run: false,
        eval_iters: 1,
        del_done_images: false,
        dump_clean_snapshot: None,
        prompt_version: "v2".to_string(),
        verbose: false,
    })
    .expect("matrix should run");

    assert_eq!(reports.len(), 2);
    assert!(run_dir.join("run.json").is_file());
    assert!(run_dir.join("comparison.json").is_file());
    let comparison: DeNovoComparisonReport = serde_json::from_str(
        &fs::read_to_string(run_dir.join("comparison.json")).expect("comparison should exist"),
    )
    .expect("comparison should parse");
    assert_eq!(comparison.conditions.len(), 2);

    fs::remove_dir_all(root).expect("temp root should clean up");
}

#[test]
fn denovo_extract_executes_fake_official_extract_recipe() {
    let root = temp_root("stateful-bench-denovo-extract");
    let aweagent = root.join("AweAgent");
    let recipe_dir = aweagent.join("recipes/denovo_swe");
    fs::create_dir_all(&recipe_dir).expect("recipe dir should exist");
    fs::write(
        recipe_dir.join("extract_patch.py"),
        r#"#!/usr/bin/env python3
import argparse
import json
from pathlib import Path
parser = argparse.ArgumentParser()
parser.add_argument("--input")
parser.add_argument("--output")
parser.add_argument("--config")
args, extra = parser.parse_known_args()
out = Path(args.output) / "extract_patch_fake"
out.mkdir(parents=True, exist_ok=True)
(out / "results.jsonl").write_text(json.dumps({"instance_id":"fake-a","test_patch":"diff --git a/test.py b/test.py\n","test_binary_archive_b64":"","test_binary_files":[]}) + "\n")
(out / "status.jsonl").write_text(json.dumps({"instance_id":"fake-a","status":"success"}) + "\n")
"#,
    )
    .expect("fake extract_patch.py should write");

    let metadata = run_denovo_extract(DeNovoExtractOptions {
        aweagent_root: aweagent,
        python: "python3".to_string(),
        input: "ready_denovoswe.jsonl".into(),
        output: root.join("extracts"),
        config: "configs/tasks/denovoswe.yaml".into(),
        max_concurrent: None,
        instance_ids: Vec::new(),
        dry_run: false,
        del_done_images: false,
        no_extract_package_info: false,
    })
    .expect("extract should run");

    assert!(metadata.running_time_ms > 0);
    assert!(metadata.results_jsonl.is_file());
    assert!(root.join("extracts/denovo-extract.json").is_file());

    fs::remove_dir_all(root).expect("temp root should clean up");
}

#[test]
fn denovo_extract_relative_paths_are_resolved_from_caller_cwd() {
    let root = temp_root("stateful-bench-denovo-extract-relative");
    if root.exists() {
        fs::remove_dir_all(&root).expect("old temp root should clean up");
    }
    let aweagent = root.join("AweAgent");
    let recipe_dir = aweagent.join("recipes/denovo_swe");
    fs::create_dir_all(&recipe_dir).expect("recipe dir should exist");
    fs::write(
        recipe_dir.join("extract_patch.py"),
        r#"#!/usr/bin/env python3
import argparse
import json
from pathlib import Path
parser = argparse.ArgumentParser()
parser.add_argument("--input")
parser.add_argument("--output")
parser.add_argument("--config")
args, extra = parser.parse_known_args()
out = Path(args.output) / "extract_patch_fake"
out.mkdir(parents=True, exist_ok=True)
(out / "results.jsonl").write_text(json.dumps({"instance_id":"fake-a","test_patch":"diff --git a/test.py b/test.py\n","test_binary_archive_b64":"","test_binary_files":[]}) + "\n")
(out / "received.json").write_text(json.dumps({"input": args.input, "output": args.output, "config": args.config}))
"#,
    )
    .expect("fake extract_patch.py should write");

    let input = root.join("ready_denovoswe.jsonl");
    let output = root.join("extracts");
    let config = root.join("configs/tasks/denovoswe.yaml");
    fs::create_dir_all(config.parent().expect("config should have parent"))
        .expect("config dir should exist");
    fs::write(&input, "{}\n").expect("input should exist");
    fs::write(&config, "config: true\n").expect("config should exist");

    let metadata = run_denovo_extract(DeNovoExtractOptions {
        aweagent_root: aweagent,
        python: "python3".to_string(),
        input: input.clone(),
        output: output.clone(),
        config: config.clone(),
        max_concurrent: None,
        instance_ids: Vec::new(),
        dry_run: false,
        del_done_images: false,
        no_extract_package_info: false,
    })
    .expect("extract should run");

    assert!(metadata.results_jsonl.starts_with(&output));
    assert!(output.join("extract_patch_fake/results.jsonl").is_file());
    assert!(!root.join("AweAgent/extracts").exists());

    let received: serde_json::Value = serde_json::from_str(
        &fs::read_to_string(output.join("extract_patch_fake/received.json"))
            .expect("received args should exist"),
    )
    .expect("received args should parse");
    assert!(Path::new(received["input"].as_str().expect("input should be text")).is_absolute());
    assert!(Path::new(received["output"].as_str().expect("output should be text")).is_absolute());
    assert!(Path::new(received["config"].as_str().expect("config should be text")).is_absolute());

    fs::remove_dir_all(root).expect("temp root should clean up");
}

fn temp_root(name: &str) -> std::path::PathBuf {
    let root = std::env::temp_dir().join(format!("{name}-{}", std::process::id()));
    if Path::new(&root).exists() {
        fs::remove_dir_all(&root).expect("old temp root should clean up");
    }
    root
}

fn run_git(workspace: &Path, args: &[&str]) {
    let output = Command::new("git")
        .args(args)
        .current_dir(workspace)
        .output()
        .expect("git should run");
    assert!(
        output.status.success(),
        "git {args:?} failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}
