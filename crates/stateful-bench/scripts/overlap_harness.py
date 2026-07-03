#!/usr/bin/env python3
from __future__ import annotations

import argparse
import json
from pathlib import Path


def _metadata(pair_json: Path) -> dict:
    pair = json.loads(pair_json.read_text(encoding="utf-8"))
    return json.loads(pair["task_a"]["test_patch"])


def _line_present(document: str, line: str) -> bool:
    return line in document.splitlines()


def _edit_passed(document: str, edit: dict) -> bool:
    op = edit.get("op")
    if op in {"replace_line", "insert_after"}:
        return _line_present(document, str(edit.get("line", "")))
    if op == "delete_line":
        return not _line_present(document, str(edit.get("line", "")))
    return False


def _coordination_observer_events(pair_dir: Path) -> tuple[list[dict], dict]:
    path = pair_dir / "coordination-events.jsonl"
    if not path.exists():
        return [], {"wait_time_ms": 0}
    source_events = [json.loads(line) for line in path.read_text(encoding="utf-8").splitlines() if line.strip()]
    observer_events: list[dict] = []
    metrics = {"wait_time_ms": 0}
    warned: set[tuple[str | None, str | None]] = set()
    for event in source_events:
        event_type = event.get("event_type") or event.get("type")
        agent_id = event.get("agent_id")
        path_value = event.get("path") or event.get("resource") or event.get("target_resource")
        if event_type in {"AuthorizationWarned", "authorization_warned", "authorization_warning"}:
            warned.add((agent_id, path_value))
            observer_events.append({"event_type": "authorization_warning", "agent_id": agent_id, "path": path_value})
        elif event_type in {"WriteCompleted", "write_completed", "human_write_observed"}:
            if (agent_id, path_value) in warned:
                observer_events.append({"event_type": "warning_ignored_write", "agent_id": agent_id, "path": path_value})
        elif event_type in {"WaitStarted", "wait_started", "active_claim_conflict", "wait_event"} or event.get("reason") == "active_claim_conflict":
            wait_ms = int(event.get("wait_ms") or event.get("duration_ms") or 0)
            metrics["wait_time_ms"] += wait_ms
            observer_events.append({"event_type": "wait_event", "agent_id": agent_id, "path": path_value, "wait_ms": wait_ms})
        elif event_type in {
            "uncoordinated_same_file_write_collision",
            "lost_edit_event",
            "coordinated_block",
            "denied_write",
        }:
            observer_events.append(event)
    return observer_events, metrics


def _agent_usage(pair_dir: Path) -> dict:
    total_tokens = 0
    tool_calls = 0
    for log_path in pair_dir.glob("*.stdout.log"):
        for line in log_path.read_text(encoding="utf-8", errors="ignore").splitlines():
            try:
                value = json.loads(line)
            except json.JSONDecodeError:
                continue
            if not isinstance(value, dict):
                continue
            usage = value.get("message", {}).get("usage", {}) or value.get("usage", {})
            total_tokens += int(usage.get("totalTokens") or usage.get("total_tokens") or 0)
            tool_calls += int(usage.get("toolCalls") or usage.get("tool_calls") or 0)
    metrics = {}
    if total_tokens:
        metrics["total_tokens"] = total_tokens
        metrics["token_count"] = total_tokens
    if tool_calls:
        metrics["tool_call_count"] = tool_calls
    return metrics


def evaluate_pair(workspace: Path, pair_json: Path, pair_dir: Path) -> dict:
    metadata = _metadata(pair_json)
    document_path = workspace / "doc.txt"
    document = document_path.read_text(encoding="utf-8") if document_path.exists() else ""
    task_results = []
    preserved = 0
    missing = 0
    for agent_id in metadata.get("agents", ["agent-a", "agent-b"]):
        edits = metadata.get("tasks", {}).get(agent_id, {}).get("edits", [])
        passed_edits = sum(1 for edit in edits if _edit_passed(document, edit))
        preserved += passed_edits
        missing += max(0, len(edits) - passed_edits)
        status = "passed" if edits and passed_edits == len(edits) else "failed"
        task_results.append({"instance_id": agent_id, "agent": agent_id, "status": status})

    observer_events, coordination_metrics = _coordination_observer_events(pair_dir)
    if observer_events:
        (pair_dir / "observer-events.jsonl").write_text(
            "\n".join(json.dumps(event, separators=(",", ":")) for event in observer_events) + "\n",
            encoding="utf-8",
        )

    metrics = {
        "preserved_edit_count": preserved,
        "missing_expected_line_count": missing,
        "false_block_count": 0,
        "missed_conflict_count": 0,
        "manual_intervention_count": 0,
        **coordination_metrics,
        **_agent_usage(pair_dir),
    }
    return {"task_results": task_results, "metrics": metrics}


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--pair-json", required=True, type=Path)
    parser.add_argument("--run-dir", required=True, type=Path)
    parser.add_argument("--workspace", required=True, type=Path)
    args = parser.parse_args()
    result = evaluate_pair(args.workspace, args.pair_json, args.run_dir)
    args.run_dir.mkdir(parents=True, exist_ok=True)
    (args.run_dir / "harness-result.json").write_text(json.dumps(result, indent=2, sort_keys=True), encoding="utf-8")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
