#!/usr/bin/env python3
"""ProgramBench adapter for Codex CLI."""

from __future__ import annotations

import argparse
import json
import subprocess
import sys
import time
from pathlib import Path
from typing import Any, Callable

PROGRAMBENCH_SYSTEM_PROMPT = """You are solving one ProgramBench instance.

You are given a compiled ./executable and bundled documentation. Rebuild an original source codebase from scratch so ./compile.sh creates a replacement ./executable with matching behavior.

Rules:
- Do not search the internet, clone repositories, or fetch package/source registry copies of the target project.
- Do not wrap, copy, chmod, or delegate to the provided ./executable.
- Do not decompile ./executable or use strace/ltrace on it.
- You may run ./executable normally and read bundled documentation.
- Leave a working ./compile.sh that builds ./executable.
""".strip()

TOKEN_KEYS = (
    "turns",
    "input_tokens",
    "cached_input_tokens",
    "output_tokens",
    "reasoning_output_tokens",
    "input_plus_output_tokens",
    "uncached_input_tokens",
    "uncached_input_plus_output_tokens",
)


def empty_token_usage() -> dict[str, int]:
    return {key: 0 for key in TOKEN_KEYS}


def iter_json_events(output: str):
    for line in output.splitlines():
        try:
            yield json.loads(line)
        except json.JSONDecodeError:
            continue


def int_field(value: Any) -> int:
    if isinstance(value, bool):
        return int(value)
    if isinstance(value, (int, float)):
        return int(value)
    return 0


def token_usage_from_value(value):
    if not isinstance(value, dict):
        return None

    input_tokens = int_field(value.get("input_tokens"))
    cached_input_tokens = int_field(value.get("cached_input_tokens"))
    output_tokens = int_field(value.get("output_tokens"))
    reasoning_output_tokens = int_field(value.get("reasoning_output_tokens"))
    total_tokens = int_field(value.get("total_tokens"))
    token_count = int_field(value.get("token_count"))

    input_details = value.get("input_tokens_details")
    if isinstance(input_details, dict):
        cached_input_tokens += int_field(input_details.get("cached_tokens"))

    output_details = value.get("output_tokens_details")
    if isinstance(output_details, dict):
        reasoning_output_tokens += int_field(output_details.get("reasoning_tokens"))

    direct_turns = int_field(value.get("turns"))
    if not any((direct_turns, input_tokens, cached_input_tokens, output_tokens, reasoning_output_tokens, total_tokens, token_count)):
        return None

    uncached_input_tokens = max(input_tokens - cached_input_tokens, 0)
    return {
        "turns": direct_turns or 1,
        "input_tokens": input_tokens,
        "cached_input_tokens": cached_input_tokens,
        "output_tokens": output_tokens,
        "reasoning_output_tokens": reasoning_output_tokens,
        "input_plus_output_tokens": input_tokens + output_tokens,
        "uncached_input_tokens": uncached_input_tokens,
        "uncached_input_plus_output_tokens": uncached_input_tokens + output_tokens,
    }


def usage_at(value: Any, path: tuple[str, ...]):
    current = value
    for key in path:
        if not isinstance(current, dict):
            return None
        current = current.get(key)
    return token_usage_from_value(current)


def add_token_usage(total: dict[str, int], usage: dict[str, int]) -> None:
    for key in TOKEN_KEYS:
        total[key] += usage.get(key, 0)


def codex_token_usage_from_output(output: str):
    total = empty_token_usage()
    for event in iter_json_events(output):
        for path in (
            ("usage",),
            ("info", "total_token_usage"),
            ("payload", "usage"),
            ("payload", "info", "total_token_usage"),
        ):
            usage = usage_at(event, path)
            if usage is not None:
                add_token_usage(total, usage)
                break
    return total


def archive_workspace(args, instance_dir: Path):
    submission_path = instance_dir / "submission.tar.gz"
    container_tar = "/tmp/programbench-submission.tar.gz"
    subprocess.run(
        [
            args.docker_bin,
            "exec",
            args.container_id,
            "tar",
            "-czf",
            container_tar,
            "-C",
            "/workspace",
            ".",
        ],
        check=True,
        capture_output=True,
        text=True,
    )
    subprocess.run(
        [args.docker_bin, "cp", f"{args.container_id}:{container_tar}", str(submission_path)],
        check=True,
        capture_output=True,
        text=True,
    )
    return submission_path


def run_agent(args, prompt):
    command = [args.codex_bin, "exec", "--json", "--cd", "/workspace"]
    if args.model:
        command.extend(["--model", args.model])
    command.append(prompt)
    return subprocess.run(
        command,
        capture_output=True,
        text=True,
        timeout=args.timeout_seconds,
    )


def build_base_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser()
    parser.add_argument("--container-id", required=True)
    parser.add_argument("--instance-id", required=True)
    parser.add_argument("--condition-id", required=True)
    parser.add_argument("--condition-dir", required=True)
    parser.add_argument("--docker-bin", required=True)
    parser.add_argument("--stateful-binary", required=True)
    parser.add_argument("--benchmark-max-turns", type=int, required=True)
    parser.add_argument("--timeout-seconds", type=int, required=True)
    parser.add_argument("--subagent-min-count", type=int, required=True)
    parser.add_argument("--stateful", action="store_true")
    parser.add_argument("--subagent", action="store_true")
    parser.add_argument("--model")
    return parser


def now_ms() -> int:
    return int(time.time() * 1000)


def output_text(value: Any) -> str:
    if value is None:
        return ""
    if isinstance(value, bytes):
        return value.decode("utf-8", errors="replace")
    return str(value)


def prompt_for_args(args) -> str:
    prompt = PROGRAMBENCH_SYSTEM_PROMPT
    if args.subagent:
        prompt += f"\n\nUse at least {args.subagent_min_count} native subagents before implementation."
    return prompt


def run_main(
    args,
    *,
    agent_name: str,
    exited_error_prefix: str,
    token_usage_from_output: Callable[[str], dict[str, int]],
    run_agent_func: Callable[[Any, str], subprocess.CompletedProcess[str]],
) -> int:
    instance_dir = Path(args.condition_dir) / args.instance_id
    instance_dir.mkdir(parents=True, exist_ok=True)
    started_at_ms = now_ms()
    stdout = ""
    stderr = ""
    exit_code = 1
    error = None

    try:
        result = run_agent_func(args, prompt_for_args(args))
        stdout = output_text(result.stdout)
        stderr = output_text(result.stderr)
        exit_code = int(result.returncode)
        if exit_code != 0:
            error = f"{exited_error_prefix} exited {exit_code}"
    except subprocess.TimeoutExpired as exc:
        stdout = output_text(exc.stdout)
        stderr = output_text(exc.stderr)
        exit_code = 124
        error = f"{exited_error_prefix} timed out after {args.timeout_seconds}s"
    except Exception as exc:  # noqa: BLE001 - adapter must record unexpected runner failures.
        exit_code = 1
        error = str(exc)

    (instance_dir / "agent.stdout.log").write_text(stdout, encoding="utf-8")
    (instance_dir / "agent.stderr.log").write_text(stderr, encoding="utf-8")

    submission_path = instance_dir / "submission.tar.gz"
    try:
        submission_path = archive_workspace(args, instance_dir)
    except Exception as exc:  # noqa: BLE001 - archive failures belong in metadata.
        if error is None:
            error = f"archive failed: {exc}"

    finished_at_ms = now_ms()
    metadata = {
        "instance_id": args.instance_id,
        "condition_id": args.condition_id,
        "agent": agent_name,
        "started_at_ms": started_at_ms,
        "finished_at_ms": finished_at_ms,
        "running_time_ms": max(finished_at_ms - started_at_ms, 0),
        "submission_path": str(submission_path),
        "exit_code": exit_code,
        "error": error,
        "subagent_used": bool(args.subagent),
        "token_usage": token_usage_from_output(stdout),
    }
    (instance_dir / "instance.json").write_text(
        json.dumps(metadata, indent=2, sort_keys=True) + "\n",
        encoding="utf-8",
    )
    return exit_code


def parse_args(argv: list[str] | None = None):
    parser = build_base_parser()
    parser.add_argument("--codex-bin", required=True)
    return parser.parse_args(argv)


def main() -> int:
    return run_main(
        parse_args(),
        agent_name="codex-cli",
        exited_error_prefix="codex",
        token_usage_from_output=codex_token_usage_from_output,
        run_agent_func=run_agent,
    )


if __name__ == "__main__":
    sys.exit(main())
