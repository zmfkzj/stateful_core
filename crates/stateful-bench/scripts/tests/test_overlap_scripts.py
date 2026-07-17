from __future__ import annotations

import json
import subprocess
from pathlib import Path

from .conftest import arg_after, load_script


def test_overlap_manifest_generator_is_deterministic_and_pair_schema():
    mod = load_script("overlap_manifest_generator.py")

    first = mod.build_manifest(count=4, seed=7)
    second = mod.build_manifest(count=4, seed=7)

    assert first == second
    assert len(first) == 4
    record = first[0]
    assert record["class"] == "exact_file_overlap"
    assert record["task_a_files"] == ["doc.txt"]
    assert record["task_b_files"] == ["doc.txt"]
    metadata = json.loads(record["task_a"]["test_patch"])
    assert 2 <= len(metadata["agents"]) <= 3
    assert metadata["base_document"].endswith("\n")
    assert metadata["expected_document"].endswith("\n")
    for agent_id in metadata["agents"]:
        task = metadata["tasks"][agent_id]
        assert task["brief"]
        assert task["edits"]
        assert task["edits"][0]["op"] in {"replace_line", "insert_after", "delete_line"}


def test_overlap_harness_scores_documents_and_exports_warning_metrics(tmp_path):
    mod = load_script("overlap_harness.py")
    workspace = tmp_path / "workspace"
    pair_dir = tmp_path / "pair"
    workspace.mkdir()
    pair_dir.mkdir()
    metadata = {
        "agents": ["agent-a", "agent-b"],
        "base_document": "base\n",
        "expected_document": "base\nA\nB\n",
        "tasks": {
            "agent-a": {"brief": "insert A", "edits": [{"op": "insert_after", "path": "doc.txt", "anchor": "base", "line": "A"}]},
            "agent-b": {"brief": "insert B", "edits": [{"op": "insert_after", "path": "doc.txt", "anchor": "A", "line": "B"}]},
        },
    }
    pair_json = pair_dir / "pair.json"
    pair_json.write_text(json.dumps({"task_a": {"test_patch": json.dumps(metadata)}}), encoding="utf-8")

    (workspace / "doc.txt").write_text("base\nA\nB\n", encoding="utf-8")
    converged = mod.evaluate_pair(workspace, pair_json, pair_dir)
    assert [row["status"] for row in converged["task_results"]] == ["passed", "passed"]
    assert converged["metrics"]["preserved_edit_count"] == 2
    assert converged["metrics"]["missing_expected_line_count"] == 0

    (workspace / "doc.txt").write_text("base\nB\n", encoding="utf-8")
    collided = mod.evaluate_pair(workspace, pair_json, pair_dir)
    assert [row["status"] for row in collided["task_results"]] == ["failed", "passed"]
    assert collided["metrics"]["missing_expected_line_count"] == 1

    def v2_event(event_type, family, kind, aggregate_id, data, created_at, agent_id="agent-a"):
        return {
            "event_id": f"event-{aggregate_id}-{kind}",
            "event_type": event_type,
            "agent_id": agent_id,
            "workspace_id": "workspace-a",
            "payload": {
                "family": family,
                "event": {
                    "kind": kind,
                    "data": {"aggregate_id": aggregate_id, "repeated": False, "data": data},
                },
            },
            "created_at": created_at,
        }

    (pair_dir / "coordination-events.jsonl").write_text(
        "\n".join(
            json.dumps(event)
            for event in [
                v2_event(
                    "wait.became_claimable",
                    "wait",
                    "became_claimable",
                    "wait-zero",
                    {"wait": {"wait_id": "wait-zero", "agent_id": "waiter-zero", "relative_path": "zero.txt"}},
                    "2026-07-17T12:00:00.020Z",
                ),
                v2_event(
                    "wait.requested",
                    "wait",
                    "requested",
                    "wait-zero",
                    {"wait": {"wait_id": "wait-zero", "agent_id": "waiter-zero", "relative_path": "zero.txt"}},
                    "2026-07-17T12:00:00.020Z",
                ),
                v2_event(
                    "wait.became_claimable",
                    "wait",
                    "became_claimable",
                    "wait-a",
                    {"wait": {"wait_id": "wait-a", "agent_id": "waiter-a", "relative_path": "doc.txt"}},
                    "2026-07-17T12:00:00.013Z",
                    agent_id="blocker-a",
                ),
                v2_event(
                    "wait.became_claimable",
                    "wait",
                    "became_claimable",
                    "wait-unrelated",
                    {"wait": {"wait_id": "wait-unrelated", "agent_id": "waiter-unrelated", "relative_path": "other.txt"}},
                    "2026-07-17T12:00:00.010Z",
                ),
                v2_event(
                    "write_intent.committed",
                    "write_intent",
                    "committed",
                    "intent-a",
                    {"write_intent": {"operation_id": "operation-a", "targets": [{"path": "doc.txt"}]}},
                    "2026-07-17T12:00:00.009Z",
                ),
                v2_event(
                    "write_intent.committed",
                    "write_intent",
                    "committed",
                    "intent-unrelated",
                    {"write_intent": {"operation_id": "operation-a", "targets": [{"path": "other.txt"}]}},
                    "2026-07-17T12:00:00.008Z",
                    agent_id="agent-b",
                ),
                {"event_type": "AuthorizationWarned", "agent_id": "agent-a", "path": "legacy.txt"},
                {"event_type": "authorization.warned", "payload": {"event": {"data": {"data": {}}}}},
                {"event_type": [], "payload": {}},
                v2_event(
                    "wait.requested",
                    "wait",
                    "requested",
                    "wait-a",
                    {"wait": {"wait_id": "wait-a", "agent_id": "waiter-a", "relative_path": "doc.txt"}},
                    "2026-07-17T12:00:00Z",
                ),
                v2_event(
                    "authorization.warned",
                    "authorization",
                    "warned",
                    "reservation-warning",
                    {"targets": [{"path": "reservation.txt"}]},
                    "2026-07-17T11:59:59.998Z",
                ),
                v2_event(
                    "authorization.warned",
                    "authorization",
                    "warned",
                    "operation-a",
                    {"operation_id": "operation-a", "targets": [{"path": "doc.txt"}]},
                    "2026-07-17T11:59:59.999Z",
                ),
            ]
        )
        + "\n",
        encoding="utf-8",
    )
    (pair_dir / "agent-a.stdout.log").write_text(
        json.dumps({"message": {"usage": {"totalTokens": 21}}}) + "\n",
        encoding="utf-8",
    )
    warned = mod.evaluate_pair(workspace, pair_json, pair_dir)
    observer_events = [
        json.loads(line)
        for line in (pair_dir / "observer-events.jsonl").read_text(encoding="utf-8").splitlines()
    ]
    assert [event["event_type"] for event in observer_events] == [
        "authorization_warning",
        "authorization_warning",
        "warning_ignored_write",
        "wait_event",
        "wait_event",
    ]
    assert observer_events[0]["path"] == "reservation.txt"
    assert "operation_id" not in observer_events[0]
    assert observer_events[2]["operation_id"] == "operation-a"
    assert observer_events[2]["agent_id"] == "agent-a"
    assert not any(
        event["event_type"] == "warning_ignored_write" and event["agent_id"] == "agent-b"
        for event in observer_events
    )
    assert observer_events[3]["wait_id"] == "wait-a"
    assert observer_events[3]["agent_id"] == "waiter-a"
    assert observer_events[4] == {
        "event_type": "wait_event",
        "agent_id": "waiter-zero",
        "wait_id": "wait-zero",
        "path": "zero.txt",
        "wait_ms": 0,
    }
    assert warned["metrics"]["wait_time_ms"] == 13
    assert warned["metrics"]["total_tokens"] == 21


def test_overlap_omp_agent_assembles_environment_and_commands(tmp_path):
    mod = load_script("overlap_omp_agent.py")
    output = tmp_path / "pair"
    workspace = tmp_path / "workspace"
    workspace.mkdir()

    env = mod.omp_environment(output, "agent-a", {"PATH": "/bin", "HOME": str(tmp_path / "source")})
    assert env["HOME"].endswith("/omp-homes/agent-a/home")
    assert env["STATEFUL_HOME"] == env["HOME"]
    assert env["PI_CODING_AGENT_DIR"] == f"{env['HOME']}/.omp/profiles/stateful/agent"
    assert env["XDG_CONFIG_HOME"] == f"{env['HOME']}/.config"
    assert env["PATH"] == "/bin"

    prompt = tmp_path / "prompt.txt"
    prompt.write_text("brief", encoding="utf-8")
    command = mod.omp_command(workspace, prompt, "omp", "model-x", "high")
    assert command[:4] == ["omp", "-p", "--mode", "json"]
    assert arg_after(command, "--model") == "model-x"
    assert arg_after(command, "--thinking") == "high"
    assert arg_after(command, "--cwd") == str(workspace)
    assert command[-1] == f"@{prompt.resolve()}"

    calls = []

    def runner(command, **kwargs):
        calls.append(command)
        return subprocess.CompletedProcess(command, 0, "", "")

    mod.prepare_environment(env, workspace, "awareness", "/tmp/stateful", runner=runner)
    assert calls[0] == ["/tmp/stateful", "install", "--agent", "omp", "--yes", "--binary", "/tmp/stateful"]
    assert calls[1] == ["/tmp/stateful", "enable", "--repo", str(workspace)]

    calls.clear()
    mod.prepare_environment(env, workspace, "no-state", "/tmp/stateful", runner=runner)
    assert calls == []
