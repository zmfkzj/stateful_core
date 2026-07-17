#!/usr/bin/env python3
from __future__ import annotations

import argparse
import json
from datetime import datetime
from pathlib import Path


_V2_EVENT_KINDS = {
    "authorization.warned": ("authorization", "warned"),
    "write_intent.committed": ("write_intent", "committed"),
    "wait.requested": ("wait", "requested"),
    "wait.became_claimable": ("wait", "became_claimable"),
}


def _v2_event(event: object) -> tuple[str, str, dict, datetime] | None:
    if not isinstance(event, dict):
        return None
    event_type = event.get("event_type")
    if not isinstance(event_type, str):
        return None
    expected_kind = _V2_EVENT_KINDS.get(event_type)
    payload = event.get("payload")
    agent_id = event.get("agent_id")
    if not expected_kind or not isinstance(payload, dict) or not isinstance(agent_id, str):
        return None
    typed_event = payload.get("event")
    if (
        payload.get("family") != expected_kind[0]
        or not isinstance(typed_event, dict)
        or typed_event.get("kind") != expected_kind[1]
    ):
        return None
    event_data = typed_event.get("data")
    if not isinstance(event_data, dict) or not isinstance(event_data.get("data"), dict):
        return None
    created_at = event.get("created_at")
    if not isinstance(created_at, str):
        return None
    try:
        timestamp = datetime.fromisoformat(created_at.removesuffix("Z") + ("+00:00" if created_at.endswith("Z") else ""))
    except ValueError:
        return None
    if timestamp.tzinfo is None:
        return None
    return event_type, agent_id, event_data["data"], timestamp


def _target_path(targets: object) -> str | None:
    if not isinstance(targets, list):
        return None
    for target in targets:
        if isinstance(target, dict) and isinstance(target.get("path"), str):
            return target["path"]
    return None


def _wait_data(data: dict) -> tuple[str, str, str | None] | None:
    wait = data.get("wait")
    if (
        not isinstance(wait, dict)
        or not isinstance(wait.get("wait_id"), str)
        or not isinstance(wait.get("agent_id"), str)
    ):
        return None
    return (
        wait["wait_id"],
        wait["agent_id"],
        wait.get("relative_path") if isinstance(wait.get("relative_path"), str) else None,
    )


def _milliseconds_between(start: datetime, end: datetime) -> int:
    elapsed = end - start
    return elapsed.days * 86_400_000 + elapsed.seconds * 1_000 + elapsed.microseconds // 1_000


def _coordination_observer_events(pair_dir: Path) -> tuple[list[dict], dict]:
    path = pair_dir / "coordination-events.jsonl"
    if not path.exists():
        return [], {"wait_time_ms": 0}
    source_events = []
    for line in path.read_text(encoding="utf-8").splitlines():
        if not line.strip():
            continue
        try:
            event = _v2_event(json.loads(line))
        except json.JSONDecodeError:
            continue
        if event is not None:
            source_events.append(event)
    source_events.reverse()
    source_events.sort(key=lambda event: event[3])
    observer_events: list[dict] = []
    metrics = {"wait_time_ms": 0}
    warned_operations: set[tuple[str, str]] = set()
    requested_waits: dict[str, datetime] = {}
    for event_type, agent_id, data, timestamp in source_events:
        if event_type == "authorization.warned":
            operation_id = data.get("operation_id")
            warning = {
                "event_type": "authorization_warning",
                "agent_id": agent_id,
                "path": _target_path(data.get("targets")),
            }
            if isinstance(operation_id, str) and operation_id:
                warned_operations.add((agent_id, operation_id))
                warning["operation_id"] = operation_id
            observer_events.append(warning)
        elif event_type == "write_intent.committed":
            intent = data.get("write_intent")
            if isinstance(intent, dict):
                operation_id = intent.get("operation_id")
                if isinstance(operation_id, str) and (agent_id, operation_id) in warned_operations:
                    observer_events.append(
                        {
                            "event_type": "warning_ignored_write",
                            "agent_id": agent_id,
                            "operation_id": operation_id,
                            "path": _target_path(intent.get("targets")),
                        }
                    )
        elif event_type == "wait.requested":
            wait = _wait_data(data)
            if wait is not None:
                requested_waits[wait[0]] = timestamp
        elif event_type == "wait.became_claimable":
            wait = _wait_data(data)
            if wait is not None and (requested := requested_waits.pop(wait[0], None)) is not None:
                wait_ms = _milliseconds_between(requested, timestamp)
                if wait_ms >= 0:
                    metrics["wait_time_ms"] += wait_ms
                    observer_events.append(
                        {
                            "event_type": "wait_event",
                            "agent_id": wait[1],
                            "wait_id": wait[0],
                            "path": wait[2],
                            "wait_ms": wait_ms,
                        }
                    )
    return observer_events, metrics


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
