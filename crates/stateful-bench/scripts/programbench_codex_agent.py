#!/usr/bin/env python3
"""ProgramBench adapter for Codex CLI."""

from __future__ import annotations

import argparse
import json
import os
import shlex
import subprocess
import sys
import tarfile
import tempfile
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

    input_tokens = int_field(value.get("input_tokens")) or int_field(value.get("input"))
    cached_input_tokens = int_field(value.get("cached_input_tokens")) or int_field(value.get("cacheRead"))
    output_tokens = int_field(value.get("output_tokens")) or int_field(value.get("output"))
    reasoning_output_tokens = int_field(value.get("reasoning_output_tokens")) or int_field(value.get("reasoning"))
    total_tokens = int_field(value.get("total_tokens")) or int_field(value.get("totalTokens"))
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


ARCHIVE_EXCLUDED_TOP_LEVEL = {
    ".cache",
    ".codex",
    ".config",
    ".git",
    ".omp",
    ".pytest_cache",
    ".stateful",
    ".stateful_core",
    ".stateful-tmp",
}

ARCHIVE_EXCLUDED_PARTS = {
    "__pycache__",
}

ARCHIVE_EXCLUDED_SUFFIXES = {
    ".pyc",
    ".pyo",
}


def archive_member_allowed(path: Path) -> bool:
    if not path.parts:
        return True
    first = path.parts[0]
    if first in ARCHIVE_EXCLUDED_TOP_LEVEL or first.startswith(".stateful"):
        return False
    if len(path.parts) >= 2 and path.parts[0] == "Library" and path.parts[1] == "Caches":
        return False
    if any(part in ARCHIVE_EXCLUDED_PARTS for part in path.parts):
        return False
    return path.suffix not in ARCHIVE_EXCLUDED_SUFFIXES


def archive_airlock_workspace(airlock: str, instance_dir: Path) -> Path:
    submission_path = instance_dir / "submission.tar.gz"
    root = Path(airlock)
    try:
        with tarfile.open(submission_path, "w:gz") as archive:
            for path in sorted(root.rglob("*")):
                relative = path.relative_to(root)
                if not archive_member_allowed(relative):
                    continue
                if path.is_file() and not os.access(path, os.R_OK):
                    if relative == Path("executable"):
                        continue
                    raise PermissionError(path)
                archive.add(path, arcname=f"./{relative}", recursive=False)
    except Exception:
        submission_path.unlink(missing_ok=True)
        raise
    return submission_path


def archive_workspace(args, instance_dir: Path):
    submission_path = getattr(args, "submission_path", None)
    if submission_path is not None:
        return Path(submission_path)
    container_tar = "/tmp/programbench-submission.tar.gz"
    subprocess.run(
        [
            resolve_host_binary(args.docker_bin),
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
        [
            resolve_host_binary(args.docker_bin),
            "cp",
            f"{args.container_id}:{container_tar}",
            str(instance_dir / "submission.tar.gz"),
        ],
        check=True,
        capture_output=True,
        text=True,
    )
    return instance_dir / "submission.tar.gz"


def copy_workspace_from_container(args, airlock: str) -> None:
    subprocess.run(
        [resolve_host_binary(args.docker_bin), "cp", f"{args.container_id}:/workspace/.", airlock],
        check=True,
        capture_output=True,
        text=True,
        timeout=args.timeout_seconds,
    )


def docker_exec_command(args, *inner: str) -> list[str]:
    return [resolve_host_binary(args.docker_bin), "exec", "-w", "/workspace", args.container_id, *inner]


def airlock_env(airlock: str) -> dict[str, str]:
    env = os.environ.copy()
    for key in list(env):
        if key.startswith("STATEFUL_") or key == "CODEX_THREAD_ID":
            env.pop(key)
    env["HOME"] = airlock
    env["STATEFUL_HOME"] = airlock
    env["CODEX_HOME"] = str(Path(airlock) / ".codex")
    return env


def initialize_airlock_git_repo(args, airlock: str) -> None:
    subprocess.run(
        ["git", "init", "-q"],
        check=True,
        capture_output=True,
        text=True,
        timeout=args.timeout_seconds,
        cwd=airlock,
        env=airlock_env(airlock),
    )


def resolve_host_binary(binary: str) -> str:
    path = Path(binary)
    if path.parent == Path(".") and not binary.startswith("."):
        return binary
    return str(path.resolve())


def run_stateful_command(args, airlock: str, *stateful_args: str) -> None:
    stateful_binary = resolve_host_binary(args.stateful_binary)
    subprocess.run(
        [stateful_binary, *stateful_args],
        check=True,
        capture_output=True,
        text=True,
        timeout=args.timeout_seconds,
        cwd=airlock,
        env=airlock_env(airlock),
    )


def install_stateful_for_agent(args, airlock: str, agent: str) -> None:
    run_stateful_command(args, airlock, "install", "--agent", agent, "--yes")

def stop_stateful_server(args, airlock: str) -> None:
    subprocess.run(
        [resolve_host_binary(args.stateful_binary), "server", "stop"],
        capture_output=True,
        text=True,
        timeout=args.timeout_seconds,
        cwd=airlock,
        env=airlock_env(airlock),
    )


def enable_stateful_repo(args, airlock: str) -> None:
    initialize_airlock_git_repo(args, airlock)
    run_stateful_command(args, airlock, "enable", "--repo", airlock)


def install_stateful_for_codex(args, airlock: str) -> None:
    install_stateful_for_agent(args, airlock, "codex")


def run_agent(args, prompt):
    with tempfile.TemporaryDirectory(prefix="programbench-airlock-") as airlock:
        env = airlock_env(airlock)
        copy_workspace_from_container(args, airlock)
        try:
            if args.stateful:
                install_stateful_for_codex(args, airlock)
                enable_stateful_repo(args, airlock)

            command = [
                resolve_host_binary(args.codex_bin),
                "-c",
                "sandbox_workspace_write.network_access=false",
            ]
            if args.subagent:
                command.extend(["-c", "features.multi_agent=true"])
            command.extend(
                [
                    "exec",
                    "--json",
                    *([] if args.stateful else ["--ignore-user-config"]),
                    "--ignore-rules",
                    "--skip-git-repo-check",
                    "--ephemeral",
                    "--cd",
                    airlock,
                    "--sandbox",
                    "workspace-write",
                ]
            )
            if args.model:
                command.extend(["--model", args.model])
            command.append(prompt)
            try:
                return subprocess.run(
                    command,
                    cwd=airlock,
                    capture_output=True,
                    text=True,
                    timeout=args.timeout_seconds,
                    env=env,
                )
            finally:
                if hasattr(args, "condition_dir"):
                    instance_dir = Path(args.condition_dir) / args.instance_id
                    try:
                        args.submission_path = str(archive_airlock_workspace(airlock, instance_dir))
                    except Exception as exc:  # noqa: BLE001 - preserve agent logs before reporting archive failure.
                        args.submission_path = str(instance_dir / "submission.tar.gz")
                        args.archive_error = str(exc)
        finally:
            if args.stateful:
                stop_stateful_server(args, airlock)


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
    prompt += (
        "\n\nTarget workspace:\n"
        "- The current directory is the ProgramBench workspace and is the submitted source tree.\n"
        "- Run `./executable` normally and read bundled documentation.\n"
        "- Do not run internet, package-manager, source-control, or host filesystem commands."
    )
    max_turns = getattr(args, "benchmark_max_turns", None)
    if max_turns is not None:
        prompt += f"\n\nBenchmark max turns: {max_turns}."
    if args.subagent:
        prompt += f"\n\nUse at least {args.subagent_min_count} native subagents before implementation."
    return prompt


def observed_subagent_used(stdout: str, stderr: str) -> bool | None:
    for event in iter_json_events(stdout + "\n" + stderr):
        value = event.get("subagent_used")
        if isinstance(value, bool):
            return value
        usage = event.get("subagent_usage")
        if isinstance(usage, dict) and isinstance(usage.get("subagent_used"), bool):
            return usage["subagent_used"]
        for candidate in (event, event.get("payload"), event.get("item")):
            if not isinstance(candidate, dict):
                continue
            event_type = candidate.get("type")
            name = candidate.get("name") or candidate.get("tool_name")
            if (
                event_type in {"tool_call", "function_call", "custom_tool_call"}
                and isinstance(name, str)
                and name.split(".")[-1] == "task"
            ):
                return True
        message = event.get("message")
        if isinstance(message, dict):
            content = message.get("content")
            if isinstance(content, list):
                for item in content:
                    if (
                        isinstance(item, dict)
                        and item.get("type") == "toolCall"
                        and item.get("name") == "task"
                    ):
                        return True
    return None


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

    archive_error = getattr(args, "archive_error", None)
    submission_path = instance_dir / "submission.tar.gz"
    try:
        submission_path = archive_workspace(args, instance_dir)
    except Exception as exc:  # noqa: BLE001 - archive failures belong in metadata.
        archive_error = str(exc)
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
        "token_usage": token_usage_from_output(stdout),
    }
    if archive_error is not None:
        metadata["archive_error"] = archive_error
    subagent_used = observed_subagent_used(stdout, stderr)
    if subagent_used is not None:
        metadata["subagent_used"] = subagent_used
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
