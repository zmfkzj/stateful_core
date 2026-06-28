#!/usr/bin/env python3
"""ProgramBench adapter for OMP CLI."""

from __future__ import annotations

import sys
from pathlib import Path

SCRIPT_DIR = Path(__file__).resolve().parent
if str(SCRIPT_DIR) not in sys.path:
    sys.path.insert(0, str(SCRIPT_DIR))

from programbench_codex_agent import (  # noqa: E402
    add_token_usage,
    build_base_parser,
    iter_json_events,
    run_main,
    token_usage_from_value,
)

import subprocess  # noqa: E402


def usage_at(value, path: tuple[str, ...]):
    current = value
    for key in path:
        if not isinstance(current, dict):
            return None
        current = current.get(key)
    return token_usage_from_value(current)


def omp_token_usage_from_output(output: str):
    from programbench_codex_agent import empty_token_usage

    total = empty_token_usage()
    for event in iter_json_events(output):
        for path in (("usage",), ("payload", "usage")):
            usage = usage_at(event, path)
            if usage is not None:
                add_token_usage(total, usage)
    return total


def run_agent(args, prompt):
    command = [args.omp_bin, "--cwd", "/workspace"]
    if args.stateful:
        command.extend(["--profile", "stateful"])
    if args.model:
        command.extend(["--model", args.model])
    command.extend(["--prompt", prompt])
    return subprocess.run(
        command,
        capture_output=True,
        text=True,
        timeout=args.timeout_seconds,
    )


def parse_args(argv: list[str] | None = None):
    parser = build_base_parser()
    parser.add_argument("--omp-bin", required=True)
    return parser.parse_args(argv)


def main() -> int:
    return run_main(
        parse_args(),
        agent_name="omp-cli",
        exited_error_prefix="omp",
        token_usage_from_output=omp_token_usage_from_output,
        run_agent_func=run_agent,
    )


if __name__ == "__main__":
    sys.exit(main())
