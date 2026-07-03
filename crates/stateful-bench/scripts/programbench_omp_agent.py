#!/usr/bin/env python3
"""ProgramBench adapter for OMP CLI."""

from __future__ import annotations

import os
import sqlite3
import subprocess  # noqa: E402
import tempfile
import urllib.parse
import sys
from pathlib import Path

SCRIPT_DIR = Path(__file__).resolve().parent
if str(SCRIPT_DIR) not in sys.path:
    sys.path.insert(0, str(SCRIPT_DIR))

from programbench_codex_agent import (  # noqa: E402
    CONTAINER_WORKSPACE,
    add_token_usage,
    airlock_env,
    build_base_parser,
    copy_workspace_from_container,
    docker_exec_env_args,
    enable_stateful_repo,
    install_stateful_for_agent,
    iter_json_events,
    resolve_host_binary,
    output_text,
    prompt_for_args,
    run_main,
    smoke_compile_airlock,
    stop_stateful_server,
    token_usage_from_value,
)



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

AGENT_DOCKER_ENV_ALLOWLIST = {
    "ANTHROPIC_API_KEY",
    "DEEPSEEK_API_KEY",
    "GEMINI_API_KEY",
    "GOOGLE_API_KEY",
    "HTTPS_PROXY",
    "HTTP_PROXY",
    "NO_PROXY",
    "OPENAI_API_KEY",
    "OPENAI_BASE_URL",
    "OPENROUTER_API_KEY",
    "SSL_CERT_FILE",
    "STATEFUL_SERVER_TOKEN",
    "STATEFUL_SERVER_URL",
}
AGENT_DOCKER_ENV_PREFIXES = ("OMP_",)


def docker_host_url(value: str) -> str:
    parsed = urllib.parse.urlparse(value)
    if parsed.hostname not in {"127.0.0.1", "localhost"}:
        return value
    netloc = "host.docker.internal"
    if parsed.port is not None:
        netloc = f"{netloc}:{parsed.port}"
    return urllib.parse.urlunparse(parsed._replace(netloc=netloc))


def agent_docker_env(args, base_env: dict[str, str]) -> dict[str, str]:
    home = args.agent_docker_home.rstrip("/") or "/home/stateful"
    env = {
        "HOME": home,
        "PI_CODING_AGENT_DIR": f"{home}/.omp/profiles/stateful/agent",
        "STATEFUL_HOME": f"{home}/.stateful",
        "XDG_CACHE_HOME": f"{home}/.cache",
        "XDG_CONFIG_HOME": f"{home}/.config",
    }
    if getattr(args, "agent_docker_sandbox", "off") == "off":
        env["STATEFUL_OMP_SANDBOX"] = "off"
    for key, value in base_env.items():
        if key in AGENT_DOCKER_ENV_ALLOWLIST or key.startswith(AGENT_DOCKER_ENV_PREFIXES):
            env[key] = docker_host_url(value) if key == "STATEFUL_SERVER_URL" else value
    return env


def docker_agent_exec_command(args, agent_container_id: str, *inner: str, env: dict[str, str] | None = None) -> list[str]:
    return [
        resolve_host_binary(args.docker_bin),
        "exec",
        "-w",
        CONTAINER_WORKSPACE,
        *docker_exec_env_args(env),
        agent_container_id,
        *inner,
    ]


def start_agent_docker_container(args) -> str:
    completed = subprocess.run(
        [
            resolve_host_binary(args.docker_bin),
            "run",
            "-d",
            "--init",
            "--network",
            "bridge",
            "-w",
            CONTAINER_WORKSPACE,
            args.agent_docker_image,
            "sleep",
            "infinity",
        ],
        check=True,
        capture_output=True,
        text=True,
        timeout=args.timeout_seconds,
    )
    agent_container_id = completed.stdout.strip()
    if not agent_container_id:
        raise RuntimeError(f"Docker did not return an agent container id for {args.agent_docker_image}")
    return agent_container_id


def remove_agent_docker_container(args, agent_container_id: str) -> None:
    try:
        subprocess.run(
            [resolve_host_binary(args.docker_bin), "rm", "-f", agent_container_id],
            capture_output=True,
            text=True,
            timeout=args.timeout_seconds,
        )
    except Exception:
        pass


def copy_airlock_to_agent_container(args, airlock: str, agent_container_id: str) -> None:
    executable = Path(airlock) / "executable"
    executable_mode = None
    if executable.is_file():
        executable_mode = executable.stat().st_mode & 0o7777
        if not os.access(executable, os.R_OK):
            os.chmod(executable, executable_mode | 0o400)
    try:
        subprocess.run(
            [
                resolve_host_binary(args.docker_bin),
                "cp",
                f"{airlock}/.",
                f"{agent_container_id}:{CONTAINER_WORKSPACE}/",
            ],
            check=True,
            capture_output=True,
            text=True,
            timeout=args.timeout_seconds,
        )
    finally:
        if executable_mode is not None:
            os.chmod(executable, executable_mode)
    if executable_mode is not None:
        subprocess.run(
            docker_agent_exec_command(
                args,
                agent_container_id,
                "chmod",
                f"{executable_mode:o}",
                f"{CONTAINER_WORKSPACE}/executable",
            ),
            check=True,
            capture_output=True,
            text=True,
            timeout=args.timeout_seconds,
        )


def copy_agent_workspace_to_airlock(args, airlock: str, agent_container_id: str) -> None:
    subprocess.run(
        [
            resolve_host_binary(args.docker_bin),
            "cp",
            f"{agent_container_id}:{CONTAINER_WORKSPACE}/.",
            airlock,
        ],
        check=True,
        capture_output=True,
        text=True,
        timeout=args.timeout_seconds,
    )


def run_agent_docker_stateful(args, agent_container_id: str, container_env: dict[str, str], *stateful_args: str) -> None:
    subprocess.run(
        docker_agent_exec_command(args, agent_container_id, args.agent_docker_stateful_binary, *stateful_args, env=container_env),
        check=True,
        capture_output=True,
        text=True,
        timeout=args.timeout_seconds,
    )


def seed_omp_auth_credentials_into_container(args, agent_container_id: str, auth_source_agent: str | None, container_env: dict[str, str]) -> None:
    if not auth_source_agent:
        return
    with tempfile.TemporaryDirectory(prefix="programbench-omp-auth-") as tmp:
        seed_env = {
            "OMP_AUTH_SOURCE_AGENT_DIR": auth_source_agent,
            "PI_CODING_AGENT_DIR": str(Path(tmp) / "agent"),
        }
        seed_omp_auth_credentials(seed_env)
        source_db = Path(seed_env["PI_CODING_AGENT_DIR"]) / "agent.db"
        if not source_db.exists():
            return
        target_dir = container_env["PI_CODING_AGENT_DIR"]
        subprocess.run(
            docker_agent_exec_command(args, agent_container_id, "mkdir", "-p", target_dir, env=container_env),
            check=True,
            capture_output=True,
            text=True,
            timeout=args.timeout_seconds,
        )
        subprocess.run(
            [
                resolve_host_binary(args.docker_bin),
                "cp",
                str(source_db),
                f"{agent_container_id}:{target_dir}/agent.db",
            ],
            check=True,
            capture_output=True,
            text=True,
            timeout=args.timeout_seconds,
        )


def run_agent_in_docker(args, prompt: str, airlock: str, base_env: dict[str, str], auth_source_agent: str | None):
    agent_container_id = start_agent_docker_container(args)
    container_env = agent_docker_env(args, base_env)
    try:
        copy_airlock_to_agent_container(args, airlock, agent_container_id)
        if args.stateful:
            run_agent_docker_stateful(args, agent_container_id, container_env, "install", "--agent", "omp", "--yes")
        seed_omp_auth_credentials_into_container(args, agent_container_id, auth_source_agent, container_env)
        if args.stateful:
            subprocess.run(
                docker_agent_exec_command(args, agent_container_id, "git", "init", "-q", env=container_env),
                check=True,
                capture_output=True,
                text=True,
                timeout=args.timeout_seconds,
            )
            run_agent_docker_stateful(args, agent_container_id, container_env, "enable", "--repo", CONTAINER_WORKSPACE)

        command = docker_agent_exec_command(
            args,
            agent_container_id,
            args.agent_docker_omp_bin,
            "--cwd",
            CONTAINER_WORKSPACE,
            "--mode",
            "json",
            "--no-session",
            "--approval-mode",
            "yolo",
            *(["--profile", "stateful"] if args.stateful else []),
            *(["--model", args.model] if args.model else []),
            *(["--thinking", args.thinking] if getattr(args, "thinking", None) else []),
            "-p",
            prompt,
            env=container_env,
        )
        try:
            return subprocess.run(
                command,
                capture_output=True,
                text=True,
                timeout=args.timeout_seconds,
            )
        finally:
            try:
                copy_agent_workspace_to_airlock(args, airlock, agent_container_id)
            except Exception as exc:  # noqa: BLE001 - preserve primary agent failure separately.
                args.workspace_copy_error = str(exc)
            else:
                if hasattr(args, "condition_dir"):
                    try:
                        smoke_compile_airlock(airlock, args)
                    except Exception as exc:  # noqa: BLE001 - preserve submission for failed compile diagnostics.
                        args.smoke_compile_error = str(exc)
    finally:
        remove_agent_docker_container(args, agent_container_id)


def run_agent(args, prompt):
    airlock = getattr(args, "airlock", "/tmp/programbench-airlock")
    env = airlock_env(airlock, args.stateful_binary if args.stateful else None)
    env.pop("OMP_AUTH_SOURCE_AGENT_DIR", None)
    if hasattr(args, "airlock") and hasattr(args, "container_id"):
        copy_workspace_from_container(args, airlock)
    env["PI_CODING_AGENT_DIR"] = str(Path(airlock) / ".omp" / "profiles" / "stateful" / "agent")
    auth_source_agent = omp_auth_source_agent_dir(os.environ)
    if args.stateful:
        inherit_parent_stateful_runtime(env, os.environ)
    if getattr(args, "agent_docker_image", None):
        return run_agent_in_docker(args, prompt, airlock, env, auth_source_agent)
    try:
        if args.stateful:
            install_stateful_for_agent(args, airlock, "omp")
        seed_env = {**env, **({"OMP_AUTH_SOURCE_AGENT_DIR": auth_source_agent} if auth_source_agent else {})}
        seed_omp_auth_credentials(seed_env)
        if args.stateful:
            enable_stateful_repo(args, airlock)

        command = [
            args.omp_bin,
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
        if getattr(args, "thinking", None):
            command.extend(["--thinking", args.thinking])
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
                try:
                    smoke_compile_airlock(airlock, args)
                except Exception as exc:  # noqa: BLE001 - preserve submission for failed compile diagnostics.
                    args.smoke_compile_error = str(exc)
    finally:
        if args.stateful:
            stop_stateful_server(args, airlock)


def parse_args(argv: list[str] | None = None):
    parser = build_base_parser()
    parser.add_argument("--omp-bin", required=True)
    parser.add_argument("--thinking")
    parser.add_argument("--agent-docker-image")
    parser.add_argument("--agent-docker-omp-bin", default="omp")
    parser.add_argument("--agent-docker-stateful-binary", default="/usr/local/bin/stateful")
    parser.add_argument("--agent-docker-home", default="/home/stateful")
    parser.add_argument("--agent-docker-sandbox", choices=["on", "off"], default="off")
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
