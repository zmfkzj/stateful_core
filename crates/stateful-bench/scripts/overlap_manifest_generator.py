#!/usr/bin/env python3
from __future__ import annotations

import argparse
import json
import random
from pathlib import Path


def _edit(agent_id: str, index: int) -> dict:
    if index % 3 == 0:
        return {"op": "replace_line", "path": "doc.txt", "line_no": 2, "old": "TODO", "line": f"{agent_id}: replace TODO"}
    if index % 3 == 1:
        return {"op": "insert_after", "path": "doc.txt", "anchor": "TODO", "line": f"{agent_id}: add detail"}
    return {"op": "delete_line", "path": "doc.txt", "line": f"obsolete-{agent_id}"}


def _apply_edit(lines: list[str], edit: dict) -> None:
    if edit["op"] == "replace_line":
        pos = max(0, int(edit.get("line_no", 1)) - 1)
        if pos < len(lines):
            lines[pos] = edit["line"]
        return
    if edit["op"] == "insert_after":
        anchor = edit.get("anchor", "")
        try:
            pos = lines.index(anchor) + 1
        except ValueError:
            pos = len(lines)
        lines.insert(pos, edit["line"])
        return
    if edit["op"] == "delete_line":
        try:
            lines.remove(edit["line"])
        except ValueError:
            pass


def build_manifest(count: int = 15, seed: int = 42) -> list[dict]:
    rng = random.Random(seed)
    pairs = []
    for pair_index in range(count):
        agent_count = rng.choice([2, 3])
        agents = [f"agent-{chr(ord('a') + i)}" for i in range(agent_count)]
        base_lines = ["Title", "TODO", f"obsolete-{agents[-1]}", "Tail"]
        tasks = {}
        expected_lines = list(base_lines)
        for idx, agent_id in enumerate(agents):
            edit = _edit(agent_id, pair_index + idx)
            tasks[agent_id] = {
                "brief": f"Apply your assigned edit to doc.txt only: {edit['op']}.",
                "edits": [edit],
            }
            _apply_edit(expected_lines, edit)
        metadata = {
            "agents": agents,
            "base_document": "\n".join(base_lines) + "\n",
            "expected_document": "\n".join(expected_lines) + "\n",
            "tasks": tasks,
        }
        task_a = _instance(pair_index, "agent-a", metadata)
        task_b = _instance(pair_index, "agent-b", metadata)
        pairs.append(
            {
                "pair_id": f"overlap-{seed}-{pair_index:03d}",
                "repo": "synthetic/forced-overlap",
                "base_commit": "synthetic-base",
                "version": "synthetic-v1",
                "eligibility": "same_base_commit",
                "class": "exact_file_overlap",
                "task_a_files": ["doc.txt"],
                "task_b_files": ["doc.txt"],
                "task_a": task_a,
                "task_b": task_b,
            }
        )
    return pairs


def _instance(pair_index: int, agent_id: str, metadata: dict) -> dict:
    task = metadata["tasks"].get(agent_id, metadata["tasks"]["agent-a"])
    return {
        "instance_id": f"overlap-{pair_index:03d}-{agent_id}",
        "repo": "synthetic/forced-overlap",
        "base_commit": "synthetic-base",
        "problem_statement": task["brief"],
        "version": "synthetic-v1",
        "patch": "diff --git a/doc.txt b/doc.txt\n",
        "test_patch": json.dumps(metadata, sort_keys=True),
        "FAIL_TO_PASS": [],
        "PASS_TO_PASS": [],
        "difficulty": "synthetic",
    }


def write_manifest(output: Path, rows: list[dict]) -> None:
    output.parent.mkdir(parents=True, exist_ok=True)
    output.write_text("\n".join(json.dumps(row, sort_keys=True) for row in rows) + "\n", encoding="utf-8")
    metadata = json.loads(rows[0]["task_a"]["test_patch"]) if rows else {"base_document": ""}
    seed_dir = output.parent / "doc-seed"
    seed_dir.mkdir(parents=True, exist_ok=True)
    (seed_dir / "doc.txt").write_text(metadata.get("base_document", ""), encoding="utf-8")


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--seed", type=int, default=42)
    parser.add_argument("--count", type=int, default=15)
    parser.add_argument("--output", required=True, type=Path)
    args = parser.parse_args()
    write_manifest(args.output, build_manifest(count=args.count, seed=args.seed))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
