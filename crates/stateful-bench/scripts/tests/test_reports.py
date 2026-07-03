from __future__ import annotations

import json
import sys
from pathlib import Path

from conftest import load_script, write_jsonl


def condition(summary: dict, condition_id: str) -> dict:
    return next(item for item in summary["conditions"] if item["condition_id"] == condition_id)


def test_progress_report_aggregates_in_progress_shards_from_results_jsonl(tmp_path):
    mod = load_script("denovo_progress_report.py")
    shard_a = tmp_path / "runs/r38-denovo-shard-a"
    shard_b = tmp_path / "runs/r38-denovo-shard-b"
    shard_a_off = shard_a / "conditions/stateful-off_subagent-on/codex-cli/_"
    shard_a_on = shard_a / "conditions/stateful-on_subagent-on/codex-cli/_"
    shard_b_off = shard_b / "conditions/stateful-off_subagent-on/codex-cli/_"
    shard_b_on = shard_b / "conditions/stateful-on_subagent-on/codex-cli/_"
    write_jsonl(shard_a_off / "results.jsonl", [
        '{"instance_id":"a-1","success":true,"score":1.0,"finish_reason":"stop","subagent_used":true,"orchestration_trace":{"trace_captured":true,"reservation_events":2,"claim_events":1,"conflict_events":0,"event_count":6,"event_types":{"SessionHeartbeat":4,"AuthorizationDenied":1,"ReservationDeclared":1},"heartbeat_events":4,"heartbeat_windows":2,"heartbeat_max_gap_ms":40000,"denial_events":1,"denial_paths":{"src/pkg.py":1},"denial_messages":{"Target existence changed since the supplied base observation.":1}}}',
        '{"instance_id":"a-2","success":false,"score":0.5,"finish_reason":"setup-error","subagent_used":false,"orchestration_trace":{"trace_captured":false,"reservation_events":0,"claim_events":0,"conflict_events":0}}',
    ])
    write_jsonl(shard_a_on / "results.jsonl", ['{"instance_id":"a-1","success":false,"score":0.0,"finish_reason":"setup-error","error":"stateful Codex benchmark requires STATEFUL_SERVER_URL and STATEFUL_SERVER_TOKEN","subagent_usage":{"subagent_used":true}}'])
    write_jsonl(shard_b_off / "results.jsonl", ['{"instance_id":"b-1","success":false,"score":0.25,"finish_reason":"context-limit","subagent_usage":{"subagent_used":true}}'])
    shard_b_on.mkdir(parents=True)
    (shard_b_on / "results.jsonl").write_text("", encoding="utf-8")

    summary = mod.collect_progress([shard_a, shard_b], expected_instances_per_condition=4)
    assert summary["run_count"] == 2
    assert summary["total_result_rows"] == 4
    assert summary["expected_instances_per_condition"] == 4
    off = condition(summary, "stateful-off_subagent-on")
    assert off["rows"] == 3
    assert off["success_count"] == 1
    assert off["setup_errors"] == 1
    assert off["finish_reasons"]["setup-error"] == 1
    assert off["finish_reasons"]["context-limit"] == 1
    assert off["subagent_used_count"] == 2
    assert off["orchestration_trace_observed"] == 2
    assert off["orchestration_trace_captured"] == 1
    assert off["orchestration_reservation_events"] == 2
    assert off["orchestration_event_types"]["SessionHeartbeat"] == 4
    assert off["orchestration_denial_paths"]["src/pkg.py"] == 1
    assert off["progress_rate"] == 0.75
    assert abs(off["average_score"] - 0.5833333333333334) < 0.000001
    on = condition(summary, "stateful-on_subagent-on")
    assert on["rows"] == 1
    assert on["setup_errors"] == 1
    assert on["subagent_used_count"] == 1
    assert on["progress_rate"] == 0.25
    assert any(run["run_id"] == "r38-denovo-shard-b" and run["condition_id"] == "stateful-on_subagent-on" and run["rows"] == 0 for run in summary["runs"])


def test_progress_report_prefers_cumulative_condition_report(tmp_path):
    mod = load_script("denovo_progress_report.py")
    run_dir = tmp_path / "runs/r38-denovo-shard-a"
    condition_dir = run_dir / "conditions/stateful-off_subagent-on"
    result_dir = condition_dir / "codex-cli/_"
    write_jsonl(result_dir / "results.jsonl", ['{"instance_id":"transient-current","success":false,"score":0.0,"finish_reason":"setup-error","orchestration_trace":{"trace_captured":true,"event_count":6,"event_types":{"SessionHeartbeat":4,"AuthorizationDenied":1,"ReservationDeclared":1},"heartbeat_events":4,"heartbeat_windows":2,"heartbeat_max_gap_ms":46000,"denial_events":1,"denial_paths":{"src/pkg.py":1},"denial_messages":{"Target existence changed since the supplied base observation.":1}}}'])
    (condition_dir / "denovo-report.json").write_text('{"condition_id":"stateful-off_subagent-on","total_instances":3,"success_count":2,"average_score":0.75,"completed_instances":3,"scored_instances":3,"error_count":0,"subagent_observed_instances":3,"subagent_used_count":2,"subagent_used_rate":0.6666666667,"orchestration_trace_observed":3,"orchestration_trace_captured":2,"orchestration_reservation_events":5,"orchestration_claim_events":4,"orchestration_conflict_events":1,"running_time_ms":1234}', encoding="utf-8")

    summary = mod.collect_progress([run_dir], expected_instances_per_condition=6)
    assert summary["total_result_rows"] == 3
    item = condition(summary, "stateful-off_subagent-on")
    assert item["rows"] == 3
    assert item["success_count"] == 2
    assert item["setup_errors"] == 1
    assert item["finish_reasons"]["setup-error"] == 1
    assert item["average_score"] == 0.75
    assert item["progress_rate"] == 0.5
    assert item["subagent_used_count"] == 2
    assert item["orchestration_trace_observed"] == 3
    assert item["orchestration_trace_captured"] == 2
    assert item["orchestration_reservation_events"] == 5
    assert item["orchestration_heartbeat_max_gap_ms"] == 46000
    assert item["orchestration_denial_paths"]["src/pkg.py"] == 1
    assert summary["runs"][0]["source"] == "denovo-report.json"
    assert summary["runs"][0]["rows"] == 3


def test_progress_report_treats_omitted_empty_trace_maps_as_empty(tmp_path):
    mod = load_script("denovo_progress_report.py")
    run_dir = tmp_path / "runs/r38-denovo-shard-a"
    condition_dir = run_dir / "conditions/stateful-off_subagent-on"
    result_dir = condition_dir / "codex-cli/_"
    write_jsonl(result_dir / "results.jsonl", ['{"instance_id":"stale-row","success":false,"score":0.0,"finish_reason":"setup-error","orchestration_trace":{"trace_captured":true,"event_count":9,"event_types":{"SessionHeartbeat":7,"AuthorizationDenied":2},"heartbeat_events":7,"heartbeat_windows":1,"heartbeat_max_gap_ms":90000,"denial_events":2,"denial_paths":{"src/stale.py":2},"denial_messages":{"stale denial":2}}}'])
    (condition_dir / "denovo-report.json").write_text('{"condition_id":"stateful-off_subagent-on","total_instances":1,"success_count":1,"average_score":1.0,"completed_instances":1,"scored_instances":1,"error_count":0,"finish_reasons":{"stop":1},"subagent_observed_instances":1,"subagent_used_count":0,"subagent_used_rate":0.0,"orchestration_trace_observed":1,"orchestration_trace_captured":0,"orchestration_reservation_events":0,"orchestration_claim_events":0,"orchestration_conflict_events":0,"orchestration_event_count":0,"orchestration_heartbeat_events":0,"orchestration_heartbeat_windows":0,"orchestration_denial_events":0,"running_time_ms":1234}', encoding="utf-8")

    item = condition(mod.collect_progress([run_dir], expected_instances_per_condition=1), "stateful-off_subagent-on")
    assert item["rows"] == 1
    assert item["orchestration_event_count"] == 0
    assert item["orchestration_event_types"] == {}
    assert item["orchestration_heartbeat_events"] == 0
    assert item["orchestration_heartbeat_windows"] == 0
    assert item["orchestration_heartbeat_max_gap_ms"] is None
    assert item["orchestration_denial_events"] == 0
    assert item["orchestration_denial_paths"] == {}
    assert item["orchestration_denial_messages"] == {}


def test_progress_report_uses_results_jsonl_for_report_finish_reasons(tmp_path):
    mod = load_script("denovo_progress_report.py")
    run_dir = tmp_path / "runs/r38-denovo-shard-a"
    condition_dir = run_dir / "conditions/stateful-off_subagent-on"
    result_dir = condition_dir / "codex-cli/_"
    write_jsonl(result_dir / "results.jsonl", [
        '{"instance_id":"case-a","success":false,"score":0.0,"finish_reason":"setup-error"}',
        '{"instance_id":"case-b","success":false,"score":0.0,"finish_reason":"codex-error"}',
        '{"instance_id":"case-c","success":true,"score":1.0,"finish_reason":"stop"}',
    ])
    (condition_dir / "denovo-report.json").write_text('{"condition_id":"stateful-off_subagent-on","total_instances":3,"success_count":1,"average_score":0.3333333333,"completed_instances":3,"scored_instances":3,"error_count":2,"subagent_observed_instances":0,"subagent_used_count":0,"running_time_ms":1234}', encoding="utf-8")

    summary = mod.collect_progress([run_dir], expected_instances_per_condition=3)
    item = condition(summary, "stateful-off_subagent-on")
    assert item["rows"] == 3
    assert item["setup_errors"] == 1
    assert item["finish_reasons"] == {"setup-error": 1, "codex-error": 1, "stop": 1}
    assert summary["runs"][0]["source"] == "denovo-report.json"
    assert summary["runs"][0]["setup_errors"] == 1
    assert summary["runs"][0]["finish_reasons"] == {"setup-error": 1, "codex-error": 1, "stop": 1}


def test_retry_overlay_report_replaces_only_codex_errors(tmp_path):
    mod = load_script("denovo_retry_overlay_report.py")
    runs_root = tmp_path / "runs"
    base_run = runs_root / "r-base-denovo-12-t3-shard-a"
    retry_run = runs_root / "r-retry-denovo-12-t3-codex-error-rerun-shard-a"
    base_off = base_run / "conditions/stateful-off_subagent-on/codex-cli/_"
    base_on = base_run / "conditions/stateful-on_subagent-on/codex-cli/_"
    retry_off = retry_run / "conditions/stateful-off_subagent-on/codex-cli/_"
    retry_on = retry_run / "conditions/stateful-on_subagent-on/codex-cli/_"
    write_jsonl(base_off / "results.jsonl", [
        '{"instance_id":"case-a","success":false,"finish_reason":"codex-error","error":"codex exited 1","subagent_used":false}',
        '{"instance_id":"case-b","success":false,"finish_reason":"missing-runtime-image","error":"image missing"}',
        '{"instance_id":"case-c","success":true,"score":1.0,"finish_reason":"stop","subagent_used":true}',
    ])
    write_jsonl(base_on / "results.jsonl", ['{"instance_id":"case-a","success":false,"finish_reason":"codex-error","error":"codex exited 1","subagent_used":false}'])
    write_jsonl(retry_off / "results.jsonl", [
        '{"instance_id":"case-a","success":true,"score":0.75,"finish_reason":"stop","subagent_used":true}',
        '{"instance_id":"case-b","success":true,"score":1.0,"finish_reason":"stop","subagent_used":true}',
    ])
    write_jsonl(retry_on / "results.jsonl", ['{"instance_id":"case-a","success":false,"score":0.25,"finish_reason":"stop","subagent_used":true}'])

    summary = mod.collect_overlay_summary(runs_root=runs_root, trials=[mod.TrialSpec("t3", ["r-base-denovo-12-t3-shard"], ["r-retry-denovo-12-t3-codex-error-rerun-shard"])], expected_instances_per_condition=3)
    assert summary["trial_count"] == 1
    assert summary["total_base_rows"] == 4
    assert summary["total_effective_rows"] == 4
    assert summary["total_replacements"] == 2
    assert summary["unused_retry_rows"] == 1
    off = condition(summary, "stateful-off_subagent-on")
    assert off["rows"] == 3
    assert off["success_count"] == 2
    assert off["scored_count"] == 2
    assert "codex-error" not in off["finish_reasons"]
    assert off["finish_reasons"]["missing-runtime-image"] == 1
    assert off["replacement_count"] == 1
    assert off["average_score"] > 0.87
    on = condition(summary, "stateful-on_subagent-on")
    assert on["rows"] == 1
    assert on["success_count"] == 0
    assert on["scored_count"] == 1
    assert on["replacement_count"] == 1
    assert on["finish_reasons"]["stop"] == 1


def test_retry_overlay_report_uses_all_trials_for_condition_progress(tmp_path):
    mod = load_script("denovo_retry_overlay_report.py")
    runs_root = tmp_path / "runs"
    for trial_id, prefix in [("t1", "r-base-denovo-12-t1"), ("t2", "r-base-denovo-12-t2")]:
        write_jsonl(runs_root / prefix / "conditions/stateful-off_subagent-on/codex-cli/_/results.jsonl", [
            json.dumps({"instance_id": f"{trial_id}-case-a", "success": True, "score": 1.0, "finish_reason": "stop"}),
            json.dumps({"instance_id": f"{trial_id}-case-b", "success": True, "score": 1.0, "finish_reason": "stop"}),
        ])
    summary = mod.collect_overlay_summary(runs_root=runs_root, trials=[mod.TrialSpec("t1", ["r-base-denovo-12-t1"], []), mod.TrialSpec("t2", ["r-base-denovo-12-t2"], [])], expected_instances_per_condition=2)
    item = condition(summary, "stateful-off_subagent-on")
    assert item["rows"] == 4
    assert item["progress_rate"] == 1.0
    assert len(summary["trials"]) == 2
    assert all(trial["rows"] == 2 and trial["progress_rate"] == 1.0 for trial in summary["trials"])


def test_overlay_instances_lists_only_negative_scored_deltas(tmp_path):
    retry_mod = load_script("denovo_retry_overlay_report.py")
    sys.modules["denovo_retry_overlay_report"] = retry_mod
    mod = load_script("denovo_overlay_instances.py")
    runs_root = tmp_path / "runs"
    base_run = runs_root / "r-base-denovo-12-t1"
    write_jsonl(base_run / "conditions/stateful-off_subagent-on/codex-cli/_/results.jsonl", [
        '{"instance_id":"negative","success":true,"score":1.0,"finish_reason":"stop"}',
        '{"instance_id":"zero","success":true,"score":0.5,"finish_reason":"stop"}',
        '{"instance_id":"positive","success":true,"score":0.25,"finish_reason":"stop"}',
    ])
    write_jsonl(base_run / "conditions/stateful-on_subagent-on/codex-cli/_/results.jsonl", [
        '{"instance_id":"negative","success":true,"score":0.5,"finish_reason":"stop"}',
        '{"instance_id":"zero","success":true,"score":0.5,"finish_reason":"stop"}',
        '{"instance_id":"positive","success":true,"score":0.75,"finish_reason":"stop"}',
    ])
    summary = mod.collect_instance_summary(runs_root, [mod.TrialSpec("t1", ["r-base-denovo-12-t1"], [])])
    negative = summary["negative_scored_deltas"]
    assert len(negative) == 1
    assert negative[0]["instance_id"] == "negative"
    assert negative[0]["score_delta_on_minus_off"] == -0.5
