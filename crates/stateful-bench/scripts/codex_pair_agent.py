#!/usr/bin/env python3
"""Launch one Codex agent for a SWE-bench pair task."""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import shutil
import subprocess
import stat
import sys
from pathlib import Path


DEFAULT_BENCHMARK_MODEL = "gpt-5.4-mini"
DEFAULT_BENCHMARK_REASONING_EFFORT = "low"
NESTED_CODEX_HOME_ROOT_ENV = "STATEFUL_NESTED_CODEX_HOME_ROOT"
AUTH_FILE_NAME = "auth.json"


class SeededAuth:
    def __init__(self, path: Path, digest: str) -> None:
        self.path = path
        self.digest = digest


class UnsafeNestedCodexHome(RuntimeError):
    pass


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


def codex_command(
    workspace: Path,
    mode: str,
    stateful_binary: str,
    benchmark_model: str = DEFAULT_BENCHMARK_MODEL,
    benchmark_reasoning_effort: str = DEFAULT_BENCHMARK_REASONING_EFFORT,
    base_env: dict[str, str] | None = None,
) -> list[str]:
    source_env = os.environ if base_env is None else base_env
    nested_benchmark = bool(source_env.get(NESTED_CODEX_HOME_ROOT_ENV))
    sandbox = "danger-full-access" if nested_benchmark else "workspace-write"
    command = [
        "codex",
        "--model",
        benchmark_model,
        "-c",
        f"model_reasoning_effort={toml_string(benchmark_reasoning_effort)}",
        "--ask-for-approval",
        "never",
        "exec",
        "--json",
        "--dangerously-bypass-hook-trust",
        "--cd",
        str(workspace),
        "--sandbox",
        sandbox,
    ]
    if not nested_benchmark:
        command.extend(["-c", "sandbox_workspace_write.network_access=true"])
    if mode == "stateful":
        for override in stateful_hook_overrides(stateful_binary):
            command.extend(["-c", override])
    command.append("-")
    return command


def path_fragment(value: str) -> str:
    fragment = "".join(
        character if character.isalnum() or character in "._-" else "-"
        for character in str(value)
    ).strip(".-")
    return fragment or "item"


def codex_environment(
    task_path: Path,
    workspace: Path,
    base_env: dict[str, str] | None = None,
) -> dict[str, str] | None:
    source_env = os.environ if base_env is None else base_env
    root = source_env.get(NESTED_CODEX_HOME_ROOT_ENV)
    if not root:
        return None

    env = dict(source_env)
    pair_fragment = path_fragment(workspace.parent.name or workspace.name)
    agent_fragment = path_fragment(task_path.stem)
    home = Path(root) / pair_fragment / agent_fragment / "home"
    env["HOME"] = str(home)
    env["CODEX_HOME"] = str(home / ".codex")
    env["XDG_CONFIG_HOME"] = str(home / ".config")
    env["XDG_CACHE_HOME"] = str(home / ".cache")

    system_cert = Path("/etc/ssl/cert.pem")
    if not env.get("SSL_CERT_FILE") and system_cert.is_file():
        env["SSL_CERT_FILE"] = str(system_cert)

    return env


def source_codex_auth_path(source_env: dict[str, str]) -> Path | None:
    codex_home = source_env.get("CODEX_HOME")
    if codex_home:
        auth_path = Path(codex_home) / AUTH_FILE_NAME
        if auth_path.is_file():
            return auth_path

    home = source_env.get("HOME")
    if home:
        auth_path = Path(home) / ".codex" / AUTH_FILE_NAME
        if auth_path.is_file():
            return auth_path

    return None


def file_digest(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def ensure_safe_directory(path: Path) -> bool:
    cursor = Path(path.anchor) if path.is_absolute() else Path()
    parts = path.parts[1:] if path.is_absolute() else path.parts
    for part in parts:
        cursor = cursor / part
        try:
            metadata = cursor.lstat()
        except FileNotFoundError:
            try:
                cursor.mkdir()
            except FileExistsError:
                pass
            except OSError:
                return False
            try:
                metadata = cursor.lstat()
            except OSError:
                return False

        if stat.S_ISLNK(metadata.st_mode) or not stat.S_ISDIR(metadata.st_mode):
            return False
    return True


def prepare_codex_environment(
    env: dict[str, str] | None,
    source_env: dict[str, str] | None = None,
) -> SeededAuth | None:
    if env is None:
        return None

    for key in ["HOME", "CODEX_HOME", "XDG_CONFIG_HOME", "XDG_CACHE_HOME"]:
        if not ensure_safe_directory(Path(env[key])):
            raise UnsafeNestedCodexHome(f"unsafe nested Codex directory for {key}: {env[key]}")

    target_auth = Path(env["CODEX_HOME"]) / AUTH_FILE_NAME
    source = os.environ if source_env is None else source_env
    source_auth = source_codex_auth_path(source)
    if source_auth is None:
        remove_stale_nested_auth(target_auth)
        return None

    if source_auth.resolve() == target_auth.resolve():
        return None

    try:
        source_digest = file_digest(source_auth)
        remove_stale_nested_auth(target_auth)
        shutil.copy2(source_auth, target_auth)
        copied_auth = SeededAuth(path=target_auth, digest=source_digest)
    except OSError:
        return None
    return copied_auth


def remove_stale_nested_auth(path: Path) -> None:
    try:
        if path.exists() or path.is_symlink():
            path.unlink()
    except OSError:
        pass


def cleanup_seeded_auth(seeded_auth: SeededAuth | None) -> None:
    if seeded_auth is None:
        return
    path = seeded_auth.path
    if path.is_symlink():
        return
    try:
        if file_digest(path) == seeded_auth.digest:
            path.unlink()
    except (FileNotFoundError, OSError):
        pass


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--task-json", required=True)
    parser.add_argument("--workspace", required=True)
    parser.add_argument("--mode", choices=["stateful", "no-state"], required=True)
    parser.add_argument("--stateful-binary", required=True)
    parser.add_argument("--session-id")
    parser.add_argument("--workspace-id")
    parser.add_argument("--benchmark-model", default=DEFAULT_BENCHMARK_MODEL)
    parser.add_argument("--benchmark-reasoning-effort", default=DEFAULT_BENCHMARK_REASONING_EFFORT)
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

    {args.stateful_binary} intent declare --session-id {args.session_id} --workspace-id {args.workspace_id} --purpose "<purpose inferred from the benchmark task>" <planned production files>

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

    source_env = dict(os.environ)
    command = codex_command(
        workspace=workspace,
        mode=args.mode,
        stateful_binary=args.stateful_binary,
        benchmark_model=args.benchmark_model,
        benchmark_reasoning_effort=args.benchmark_reasoning_effort,
        base_env=source_env,
    )
    env = codex_environment(task_path=task_path, workspace=workspace, base_env=source_env)
    try:
        seeded_auth = prepare_codex_environment(env, source_env=source_env)
    except UnsafeNestedCodexHome as error:
        print(f"codex pair agent setup failed: {error}", file=sys.stderr)
        return 1
    try:
        completed = subprocess.run(
            command,
            input=prompt,
            text=True,
            cwd=workspace,
            check=False,
            env=env,
        )
        return completed.returncode
    finally:
        cleanup_seeded_auth(seeded_auth)


if __name__ == "__main__":
    sys.exit(main())
