#!/usr/bin/env python3
from __future__ import annotations

import argparse
import json
import os
import shutil
import subprocess
from pathlib import Path
from typing import Callable

Runner = Callable[..., subprocess.CompletedProcess]


def omp_environment(pair_dir: Path, agent_id: str, base_env: dict[str, str] | None = None) -> dict[str, str]:
    base = dict(base_env or os.environ)
    home = pair_dir / "omp-homes" / agent_id / "home"
    env = dict(base)
    env["HOME"] = str(home)
    env["STATEFUL_HOME"] = str(home)
    env["PI_CODING_AGENT_DIR"] = f"{home}/.omp/profiles/stateful/agent"
    env["XDG_CONFIG_HOME"] = f"{home}/.config"
    env["XDG_DATA_HOME"] = f"{home}/.local/share"
    env["XDG_CACHE_HOME"] = f"{home}/.cache"
    return env


def omp_command(workspace: Path, prompt: Path, omp_bin: str, model: str, thinking: str) -> list[str]:
    return [
        omp_bin,
        "-p",
        "--mode",
        "json",
        "--model",
        model,
        "--thinking",
        thinking,
        "--cwd",
        str(workspace),
        "--approval-mode",
        "yolo",
        "--no-title",
        f"@{prompt.resolve()}",
    ]


def prepare_environment(
    env: dict[str, str],
    workspace: Path,
    mode: str,
    stateful_binary: str,
    runner: Runner = subprocess.run,
) -> None:
    Path(env["HOME"]).mkdir(parents=True, exist_ok=True)
    if mode == "no-state":
        return
    runner([stateful_binary, "install", "--agent", "omp", "--yes", "--binary", stateful_binary], env=env, check=True)
    runner([stateful_binary, "enable", "--repo", str(workspace)], env=env, check=True)


def agent_prompt(task_json: Path, pair_json: Path, agent_id: str) -> str:
    task = json.loads(task_json.read_text(encoding="utf-8"))
    pair = json.loads(pair_json.read_text(encoding="utf-8"))
    metadata = json.loads(pair["task_a"]["test_patch"])
    assignment = metadata["tasks"].get(agent_id, metadata["tasks"].get("agent-a", {}))
    edits = json.dumps(assignment.get("edits", []), indent=2)
    return "\n".join(
        [
            "Edit doc.txt only. Preserve other content.",
            task.get("problem_statement") or assignment.get("brief", "Apply assigned edit."),
            "Assigned edits:",
            edits,
            "Do not coordinate outside the provided Stateful workflow.",
        ]
    )


def copy_openai_codex_auth(source_home: Path, dest_home: Path) -> None:
    source = source_home / ".codex" / "auth.json"
    if not source.exists():
        return
    target = dest_home / ".codex" / "auth.json"
    target.parent.mkdir(parents=True, exist_ok=True)
    shutil.copy2(source, target)


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--task-json", required=True, type=Path)
    parser.add_argument("--pair-json", required=True, type=Path)
    parser.add_argument("--workspace", required=True, type=Path)
    parser.add_argument("--agent-id", required=True)
    parser.add_argument("--mode", required=True, choices=["no-state", "awareness", "stateful"])
    parser.add_argument("--stateful-binary", required=True)
    parser.add_argument("--session-id", required=True)
    parser.add_argument("--workspace-id", required=True)
    parser.add_argument("--model", default="deepseek-v4-flash")
    parser.add_argument("--thinking", default="high")
    parser.add_argument("--omp-bin", default="omp")
    args = parser.parse_args()

    pair_dir = args.pair_json.resolve().parent
    env = omp_environment(pair_dir, args.agent_id)
    copy_openai_codex_auth(Path(os.environ.get("HOME", "")), Path(env["HOME"]))
    prepare_environment(env, args.workspace, args.mode, args.stateful_binary)

    prompt = pair_dir / f"{args.agent_id}.prompt.txt"
    prompt.write_text(agent_prompt(args.task_json, args.pair_json, args.agent_id), encoding="utf-8")
    completed = subprocess.run(omp_command(args.workspace, prompt, args.omp_bin, args.model, args.thinking), env=env)
    return completed.returncode


if __name__ == "__main__":
    raise SystemExit(main())
