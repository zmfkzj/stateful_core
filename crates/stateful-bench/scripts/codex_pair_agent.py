#!/usr/bin/env python3
"""Launch one Codex agent for a SWE-bench pair task."""

from __future__ import annotations

import argparse
import json
import subprocess
import sys
from pathlib import Path


def toml_string(value: str) -> str:
    return json.dumps(value)


def hook_override(event_name: str, command: str, status_message: str, matcher: str | None = None) -> str:
    fields = []
    if matcher is not None:
        fields.append(f"matcher = {toml_string(matcher)}")
    fields.append(
        "hooks = [{ "
        f"type = {toml_string('command')}, "
        f"command = {toml_string(command)}, "
        f"statusMessage = {toml_string(status_message)} "
        "}]"
    )
    return f"hooks.{event_name}=[{{ {', '.join(fields)} }}]"


def stateful_hook_overrides(stateful_binary: str) -> list[str]:
    hook_prefix = f'"{stateful_binary}" hook'
    return [
        "features.hooks=true",
        hook_override(
            "PreToolUse",
            f"{hook_prefix} pre-tool-use",
            "Authorizing stateful tool use",
            "Bash|apply_patch|Edit|Write|mcp__filesystem__.*",
        ),
        hook_override(
            "PostToolUse",
            f"{hook_prefix} post-tool-use",
            "Recording stateful activity",
            "Bash|apply_patch|Edit|Write|mcp__filesystem__.*",
        ),
    ]


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--task-json", required=True)
    parser.add_argument("--workspace", required=True)
    parser.add_argument("--mode", choices=["stateful", "no-state"], required=True)
    parser.add_argument("--stateful-binary", required=True)
    parser.add_argument("--session-id")
    parser.add_argument("--workspace-id")
    args = parser.parse_args()

    if args.mode == "stateful" and (not args.session_id or not args.workspace_id):
        parser.error("--session-id and --workspace-id are required in stateful mode")

    task_path = Path(args.task_json).resolve()
    workspace = Path(args.workspace).resolve()
    task = json.loads(task_path.read_text())

    stateful_instruction = ""
    if args.mode == "stateful":
        stateful_instruction = f"""
Before any file modification, inspect the code enough to identify the production
file or files you plan to edit, then run:

    {args.stateful_binary} intent declare --session-id {args.session_id} --workspace-id {args.workspace_id} <planned production files>

Use this exact session id and workspace id. If intent declaration fails, stop
without editing.
"""

    prompt = f"""
You are one of two concurrent agents in a shared SWE-bench workspace.

Task JSON path:
{task_path}

Task:
{task["problem_statement"]}

Constraints:
- Solve only this task. Do not inspect pair.json, the other task JSON, run
  artifacts, gold patches, or benchmark metadata outside the task JSON above.
- Edit only production source files needed for the fix.
- Do not edit tests, documentation, generated files, package metadata, or
  benchmark artifacts.
- Use apply_patch for code edits. Do not use Bash, Python, Perl, sed, tee, or
  shell redirection to modify code.
- Bash is allowed for read-only inspection and test commands.
{stateful_instruction}
When finished, leave the working tree with only the production code fix for this
task.
""".strip()

    command = [
        "codex",
        "--ask-for-approval",
        "never",
        "exec",
        "--json",
        "--dangerously-bypass-hook-trust",
        "--cd",
        str(workspace),
        "--sandbox",
        "workspace-write",
        "-c",
        "sandbox_workspace_write.network_access=true",
    ]
    if args.mode == "stateful":
        for override in stateful_hook_overrides(args.stateful_binary):
            command.extend(["-c", override])
    command.append("-")
    completed = subprocess.run(
        command,
        input=prompt,
        text=True,
        cwd=workspace,
        check=False,
    )
    return completed.returncode


if __name__ == "__main__":
    sys.exit(main())
