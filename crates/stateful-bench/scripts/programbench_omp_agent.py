#!/usr/bin/env python3
"""ProgramBench adapter for OMP CLI."""

from __future__ import annotations

import os
import sqlite3
import sys
import tempfile
from pathlib import Path

SCRIPT_DIR = Path(__file__).resolve().parent
if str(SCRIPT_DIR) not in sys.path:
    sys.path.insert(0, str(SCRIPT_DIR))

from programbench_codex_agent import (  # noqa: E402
    add_token_usage,
    airlock_env,
    build_base_parser,
    archive_airlock_workspace,
    copy_workspace_from_container,
    enable_stateful_repo,
    install_stateful_for_agent,
    iter_json_events,
    output_text,
    prompt_for_args,
    resolve_host_binary,
    run_main,
    stop_stateful_server,
    smoke_compile_airlock,
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
        for path in (("usage",), ("message", "usage"), ("payload", "usage")):
            usage = usage_at(event, path)
            if usage is not None:
                add_token_usage(total, usage)
                break
    return total


def omp_auth_source_agent_dir(source_env: dict[str, str]) -> str | None:
    explicit = source_env.get("OMP_AUTH_SOURCE_AGENT_DIR")
    if explicit:
        return explicit
    source_home = Path(source_env.get("HOME", "")).expanduser()
    for candidate in (
        source_home / ".omp" / "profiles" / "stateful" / "agent",
        source_home / ".omp" / "agent",
    ):
        if (candidate / "agent.db").exists():
            return str(candidate)
    return None


def seed_omp_auth_credentials(env: dict[str, str]) -> None:
    source_agent = env.get("OMP_AUTH_SOURCE_AGENT_DIR")
    if not source_agent:
        return
    source_db = Path(source_agent) / "agent.db"
    target_db = Path(env["PI_CODING_AGENT_DIR"]) / "agent.db"
    if not source_db.exists():
        return
    with sqlite3.connect(source_db) as source:
        auth_schema = source.execute(
            "SELECT sql FROM sqlite_master WHERE type = 'table' AND name = 'auth_credentials'"
        ).fetchone()
        rows = source.execute(
            """
            SELECT provider, credential_type, data, disabled_cause, identity_key, created_at, updated_at
            FROM auth_credentials
            WHERE provider = 'openai-codex' AND credential_type = 'oauth'
            """
        ).fetchall()
    if not rows:
        return
    target_db.parent.mkdir(parents=True, exist_ok=True)
    with sqlite3.connect(target_db) as target:
        has_auth = target.execute(
            "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'auth_credentials'"
        ).fetchone()
        if auth_schema is not None and has_auth is None:
            target.execute(auth_schema[0])
        target.execute(
            "DELETE FROM auth_credentials WHERE provider = 'openai-codex' AND credential_type = 'oauth'"
        )
        target.executemany(
            """
            INSERT INTO auth_credentials
                (provider, credential_type, data, disabled_cause, identity_key, created_at, updated_at)
            VALUES (?, ?, ?, ?, ?, ?, ?)
            """,
            rows,
        )



def inherit_parent_stateful_runtime(target_env: dict[str, str], source_env: dict[str, str]) -> None:
    server_url = source_env.get("STATEFUL_SERVER_URL")
    server_token = source_env.get("STATEFUL_SERVER_TOKEN")
    if server_url and server_token:
        target_env["STATEFUL_SERVER_URL"] = server_url
        target_env["STATEFUL_SERVER_TOKEN"] = server_token


def run_omp_command(command, *, cwd: str, env: dict[str, str], timeout_seconds: int):
    process = subprocess.Popen(
        command,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        cwd=cwd,
        env=env,
        text=True,
    )
    try:
        stdout, stderr = process.communicate(timeout=timeout_seconds)
        return subprocess.CompletedProcess(command, process.returncode, stdout=stdout, stderr=stderr)
    except subprocess.TimeoutExpired as exc:
        cleanup_error = None
        try:
            process.kill()
        except Exception as kill_exc:  # noqa: BLE001 - sandbox may deny signals; preserve the timeout as primary.
            cleanup_error = str(kill_exc)
        else:
            try:
                stdout, stderr = process.communicate()
                exc.output = output_text(stdout) or output_text(exc.output)
                exc.stderr = output_text(stderr) or output_text(exc.stderr)
            except Exception as wait_exc:  # noqa: BLE001 - cleanup failure is secondary to timeout.
                cleanup_error = str(wait_exc)
        if cleanup_error is not None:
            exc.cleanup_error = cleanup_error
        raise


def run_agent(args, prompt):
    with tempfile.TemporaryDirectory(prefix="programbench-airlock-") as airlock:
        env = airlock_env(airlock)
        if args.stateful:
            inherit_parent_stateful_runtime(env, os.environ)
        env["PI_CODING_AGENT_DIR"] = str(Path(airlock) / ".omp" / "profiles" / "stateful" / "agent")
        auth_source_agent = omp_auth_source_agent_dir(os.environ)
        if auth_source_agent:
            env["OMP_AUTH_SOURCE_AGENT_DIR"] = auth_source_agent
        copy_workspace_from_container(args, airlock)
        try:
            if args.stateful:
                install_stateful_for_agent(args, airlock, "omp")
            seed_omp_auth_credentials(env)
            if args.stateful:
                enable_stateful_repo(args, airlock)

            command = [
                resolve_host_binary(args.omp_bin),
                "--cwd",
                airlock,
                "--mode",
                "json",
                "--no-session",
                "--approval-mode",
                "yolo",
            ]
            if args.stateful:
                command.extend(["--profile", "stateful"])
            if args.model:
                command.extend(["--model", args.model])
            command.extend(["-p", prompt])
            try:
                return run_omp_command(
                    command,
                    cwd=airlock,
                    env=env,
                    timeout_seconds=args.timeout_seconds,
                )
            finally:
                if hasattr(args, "condition_dir"):
                    instance_dir = Path(args.condition_dir) / args.instance_id
                    try:
                        smoke_compile_airlock(airlock, args)
                    except Exception as exc:  # noqa: BLE001 - preserve submission for failed compile diagnostics.
                        args.smoke_compile_error = str(exc)
                    try:
                        args.submission_path = str(archive_airlock_workspace(airlock, instance_dir))
                    except Exception as exc:  # noqa: BLE001 - preserve OMP logs before reporting archive failure.
                        args.submission_path = str(instance_dir / "submission.tar.gz")
                        args.archive_error = str(exc)
        finally:
            if args.stateful:
                stop_stateful_server(args, airlock)


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
