#!/usr/bin/env python3
"""Run DeNovoSWE instances with host Codex CLI."""

from __future__ import annotations

import argparse
import asyncio
import http.server
import json
import os
import re
import select
import shutil
import sqlite3
import socket
import socketserver
import subprocess
import sys
import tarfile
import tempfile
import threading
import time
import urllib.parse
import urllib.request
import uuid
from collections import Counter
from datetime import datetime, timezone
from dataclasses import dataclass
from pathlib import Path, PurePosixPath
from typing import Any, Iterable

SCRIPT_DIR = Path(__file__).resolve().parent
if str(SCRIPT_DIR) not in sys.path:
    sys.path.insert(0, str(SCRIPT_DIR))

from codex_pair_agent import (  # noqa: E402
    NESTED_CODEX_HOME_ROOT_ENV,
    STATEFUL_INTEGRATION_FULL,
    STATEFUL_INTEGRATION_NONE,
    UnsafeNestedCodexHome,
    cleanup_seeded_auth,
    path_fragment,
    path_scope_digest,
    prepare_codex_environment,
    iter_json_events,
    run_codex_with_resume,
    toml_string,
)


OFFICIAL_BENCHMARK_PROTOCOL = "denovo_swe_single_rollout"
RESUME_POLICY_CONTEXT_OR_TOKEN_ONLY = "context_or_token_failure_only"
DEFAULT_SUBAGENT_MIN_COUNT = 3
DEFAULT_OMP_REASONING_EFFORT = "high"
ORCHESTRATION_TRACE_EVENT_LIMIT = 10_000
CODEX_EMPTY_STOP_EXIT_CODE = 2


def cli_runtime_failure(returncode: int, cli_runtime: str) -> tuple[str, str]:
    if returncode == CODEX_EMPTY_STOP_EXIT_CODE:
        return (
            f"{cli_runtime}-empty-stop",
            f"{cli_runtime} returned an empty stop after retry cap",
        )
    return (f"{cli_runtime}-error", f"{cli_runtime} exited {returncode}")

DEFAULT_MIN_FREE_DISK_GB = 20.0
BYTES_PER_GIB = 1024**3
DEFAULT_OMP_AGENT_DOCKER_STATEFUL_BINARY = "/usr/local/bin/stateful"
OMP_AGENT_DOCKER_WORKSPACE = "/workspace"
OMP_AGENT_DOCKER_PROMPT = "/prompt.txt"
OMP_AGENT_DOCKER_HOME = "/home/stateful"
OMP_AGENT_DOCKER_ENV_ALLOWLIST = {
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
    "STATEFUL_BENCHMARK_SOURCE_BLOCK_PATTERNS",
    "STATEFUL_SERVER_TOKEN",
    "STATEFUL_SERVER_URL",
}
OMP_AGENT_DOCKER_ENV_PREFIXES = ("OMP_",)
OMP_AGENT_DOCKER_ARG_VALUE_ENV = {
    "HOME",
    "PI_CODING_AGENT_DIR",
    "STATEFUL_HOME",
    "STATEFUL_SERVER_URL",
    "STATEFUL_OMP_SANDBOX",
    "XDG_CACHE_HOME",
    "XDG_CONFIG_HOME",
}
DIFF_EXCLUDED_PATHS = (
    ".codex",
    ".codex/**",
    ".stateful",
    ".stateful/**",
    ".stateful_core",
    ".stateful_core/**",
    ".stateful-tmp",
    ".stateful-tmp/**",
    "**/.stateful-tmp",
    "**/.stateful-tmp/**",
    "upstream",
    "upstream/**",
    ".cache",
    ".cache/**",
    ".pytest_cache",
    ".pytest_cache/**",
    ".ruff_cache",
    ".ruff_cache/**",
    ".mypy_cache",
    ".mypy_cache/**",
    "__pycache__",
    "__pycache__/**",
    "**/__pycache__",
    "**/__pycache__/**",
    ".coverage",
    "coverage.xml",
    "htmlcov",
    "htmlcov/**",
    "target",
    "target/**",
    "clean.sh",
)

WORKSPACE_COPY_IGNORE_PATTERNS = (
    ".codex",
    ".stateful",
    ".stateful_bench",
    ".stateful_core",
    "upstream",
)
BENCHMARK_SOURCE_LEAK_COMMAND_PATTERNS = (
    "git clone",
    "git fetch",
    "git pull",
    "gh pr",
    "gh issue",
)
BENCHMARK_SOURCE_LEAK_HOST_PATTERNS = (
    "github.com",
    "raw.githubusercontent.com",
    "patch-diff.githubusercontent.com",
    "api.github.com/repos",
)
BENCHMARK_SOURCE_LEAK_CONNECT_HOSTS = tuple(
    sorted({pattern.split("/", 1)[0] for pattern in BENCHMARK_SOURCE_LEAK_HOST_PATTERNS})
)


@dataclass
class InstanceResult:
    instance_id: str
    success: bool | None
    score: float | None
    finish_reason: str | None
    error: str | None
    eval_result: dict[str, Any] | None
    subagent_used: bool | None = None
    subagent_usage: dict[str, Any] | None = None
    token_usage: dict[str, int] | None = None
    orchestration_trace: dict[str, Any] | None = None


@dataclass
class CodexExecutionSummary:
    returncode: int
    token_usage: dict[str, int]


class CodexTimeoutError(TimeoutError):
    pass


class StatefulRepoEnableError(RuntimeError):
    pass


class MissingRuntimeImageError(RuntimeError):
    def __init__(self, image: str, error: BaseException):
        self.image = image
        self.source_error = error
        super().__init__(f"runtime image unavailable: {image}; {repr(error)}")


@dataclass
class StatefulRepoEnableCleanup:
    created_stateful_dir: bool
    created_policy_config: bool


def native_subagent_prompt_instruction(subagent: str, subagent_min_count: int) -> str:
    if subagent != "on":
        return ""
    return "orchestrate"


def benchmark_isolation_prompt_instruction() -> str:
    return """

Benchmark isolation requirements:
- Reconstruct the package from the provided workspace and repository specification only.
- Do not fetch, clone, open, or inspect the upstream repository, pull request, issue, patch, commit, or raw source for this instance.
- Do not create or use an `upstream` checkout, mirror, or source-copy directory.

<ANTI_CHEAT_CONSTRAINT>
ABSOLUTE RULE: DO NOT DOWNLOAD THE TARGET PACKAGE'S SOURCE CODE FROM THE INTERNET.
You MUST implement the target project from scratch based solely on the repository specification above.
This does not prohibit non-target third-party dependency research.
It is a hard violation to look up, download, install, clone, or copy the target package's source code from any external source, including PyPI, GitHub, GitLab, Bitbucket, Codeberg, conda-forge, mirrors, caches, wheels, sdists, patch files, issues, pull requests, commits, or raw-source URLs.

Specifically forbidden for the target package:
- Package-manager source recovery such as `pip install <target>`, `pip download <target>`, `pip show <target>`, `python -m pip install <target>`, `uv add <target>`, `poetry add <target>`, `conda install <target>`, or equivalent commands.
- VCS or source-control access such as `git clone`, `git fetch`, `git pull`, `git submodule add`, `gh pr`, or `gh issue` for the target repository, pull request, issue, patch, commit, or raw source.
- Direct downloads from PyPI-family hosts, repository hosts, patch-diff hosts, or URLs containing the target package or upstream repository name.
- Python introspection or on-disk artifact extraction that dumps the installed upstream source.

The following are ALLOWED:
- Installing *third-party* dependencies your own implementation needs.
- Running `pip install -e .` on the code you yourself wrote inside the workspace.
- Reading files already present in the workspace at the start of the session, except benchmark artifacts and generated metadata.
</ANTI_CHEAT_CONSTRAINT>
""".rstrip()




def build_codex_prompt(
    instance_id: str,
    document: str,
    benchmark_max_turns: int,
    max_steps: int | None,
    prompt_version: str,
    subagent: str = "off",
    subagent_min_count: int = DEFAULT_SUBAGENT_MIN_COUNT,
    stateful_binary: str | None = None,
) -> str:
    step_line = f"- Maximum task steps: {max_steps}.\n" if max_steps is not None else ""
    subagent_instruction = native_subagent_prompt_instruction(subagent, subagent_min_count)
    isolation_instruction = benchmark_isolation_prompt_instruction()
    return f"""
You are solving one DeNovoSWE benchmark instance.

Instance id:
{instance_id}
{isolation_instruction}
{subagent_instruction}


Repository specification:
{document}

Constraints:
- Solve only this DeNovoSWE instance.
- Edit only files in the provided workspace.
- Do not edit benchmark artifacts, result files, Codex logs, auth files, or generated metadata.
- Leave the workspace containing the final code changes.
- Benchmark max turns: {benchmark_max_turns}.
{step_line}- Prompt version: {prompt_version}.
""".strip()


def git_diff(workspace: Path) -> str:
    add_completed = subprocess.run(
        ["git", "add", "-A", "--", "."],
        cwd=workspace,
        text=True,
        check=False,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
    )
    if add_completed.returncode != 0:
        raise RuntimeError(add_completed.stderr.strip() or "git add -A failed")

    diff_completed = subprocess.run(
        [
            "git",
            "diff",
            "--cached",
            "--binary",
            "--",
            ".",
            *(f":(exclude){path}" for path in DIFF_EXCLUDED_PATHS),
        ],
        cwd=workspace,
        text=True,
        check=False,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
    )
    if diff_completed.returncode != 0:
        raise RuntimeError(diff_completed.stderr.strip() or "git diff --cached --binary failed")
    reset_completed = subprocess.run(
        ["git", "reset", "-q", "HEAD"],
        cwd=workspace,
        text=True,
        check=False,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
    )
    if reset_completed.returncode != 0:
        raise RuntimeError(reset_completed.stderr.strip() or "git reset -q HEAD failed")
    return diff_completed.stdout


def run_codex_with_timeout(
    command: list[str],
    prompt: str,
    workspace: Path,
    env: dict[str, str] | None,
    max_resumes: int,
    timeout_seconds: float,
    runner: Any = subprocess.run,
) -> CodexExecutionSummary:
    if timeout_seconds <= 0:
        raise CodexTimeoutError(f"codex timed out after {timeout_seconds:g}s")

    deadline = time.monotonic() + timeout_seconds

    def bounded_runner(*args: Any, **kwargs: Any) -> Any:
        remaining = deadline - time.monotonic()
        if remaining <= 0:
            raise CodexTimeoutError(f"codex timed out after {timeout_seconds:g}s")
        kwargs["timeout"] = remaining
        try:
            return runner(*args, **kwargs)
        except subprocess.TimeoutExpired as error:
            raise CodexTimeoutError(f"codex timed out after {timeout_seconds:g}s") from error

    token_usage = empty_codex_token_usage()

    def observe_result(result: Any) -> None:
        add_codex_token_usage(token_usage, codex_token_usage_from_output(result.stdout))

    returncode = run_codex_with_resume(
        command,
        prompt,
        workspace,
        env,
        max_resumes=max_resumes,
        runner=bounded_runner,
        result_observer=observe_result,
    )
    return CodexExecutionSummary(returncode=returncode, token_usage=token_usage)


def run_omp_with_timeout(
    command: list[str],
    workspace: Path,
    env: dict[str, str] | None,
    timeout_seconds: float,
    runner: Any = subprocess.run,
) -> CodexExecutionSummary:
    if timeout_seconds <= 0:
        raise CodexTimeoutError(f"omp timed out after {timeout_seconds:g}s")
    try:
        completed = runner(
            command,
            cwd=workspace,
            text=True,
            check=False,
            env=env,
            stdin=subprocess.DEVNULL,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            timeout=timeout_seconds,
        )
    except subprocess.TimeoutExpired as error:
        raise CodexTimeoutError(f"omp timed out after {timeout_seconds:g}s") from error
    return CodexExecutionSummary(
        returncode=completed.returncode,
        token_usage=omp_token_usage_from_output(completed.stdout),
    )


def empty_codex_token_usage() -> dict[str, int]:
    return {
        "turns": 0,
        "input_tokens": 0,
        "cached_input_tokens": 0,
        "output_tokens": 0,
        "reasoning_output_tokens": 0,
        "input_plus_output_tokens": 0,
        "uncached_input_tokens": 0,
        "uncached_input_plus_output_tokens": 0,
    }


def add_codex_token_usage(total: dict[str, int], update: dict[str, int]) -> None:
    for key in total:
        total[key] += int(update.get(key, 0) or 0)


def codex_token_usage_from_output(output: str) -> dict[str, int]:
    total = empty_codex_token_usage()
    total_event: dict[str, int] | None = None
    for event in iter_json_events(output):
        if not isinstance(event, dict):
            continue
        usage = codex_usage_from_event(event)
        if usage is None:
            continue
        if event.get("type") == "turn.completed":
            add_codex_token_usage(total, usage)
        else:
            total_event = usage
    if total["turns"] == 0 and total_event is not None:
        return total_event
    return total


def omp_token_usage_from_output(output: str) -> dict[str, int]:
    latest = None
    for event in iter_json_events(output):
        if not isinstance(event, dict):
            continue
        usage = omp_usage_from_event(event)
        if usage is not None:
            latest = usage
    return latest or empty_codex_token_usage()


def omp_usage_from_event(event: dict[str, Any]) -> dict[str, int] | None:
    usage = first_dict(pointer(event, "message", "usage"))
    if usage is None:
        return None
    uncached_input_tokens = int(usage.get("input", 0) or 0)
    cached_input_tokens = int(usage.get("cacheRead", 0) or 0)
    input_tokens = uncached_input_tokens + cached_input_tokens
    output_tokens = int(usage.get("output", 0) or 0)
    reasoning_output_tokens = int(usage.get("reasoningTokens", 0) or 0)
    input_plus_output_tokens = (
        int(usage.get("totalTokens", 0) or 0) or input_tokens + output_tokens
    )
    return {
        "turns": 1,
        "input_tokens": input_tokens,
        "cached_input_tokens": cached_input_tokens,
        "output_tokens": output_tokens,
        "reasoning_output_tokens": reasoning_output_tokens,
        "input_plus_output_tokens": input_plus_output_tokens,
        "uncached_input_tokens": uncached_input_tokens,
        "uncached_input_plus_output_tokens": uncached_input_tokens + output_tokens,
    }


def codex_usage_from_event(event: dict[str, Any]) -> dict[str, int] | None:
    usage = first_dict(
        event.get("usage"),
        pointer(event, "info", "total_token_usage"),
        pointer(event, "payload", "info", "total_token_usage"),
        pointer(event, "payload", "usage"),
        pointer(event, "response", "usage"),
        pointer(event, "payload", "response", "usage"),
    )
    if usage is None:
        return None
    input_tokens = int(usage.get("input_tokens", 0) or 0)
    cached_input_tokens = int(
        usage.get("cached_input_tokens", 0)
        or pointer(usage, "input_tokens_details", "cached_tokens")
        or 0
    )
    output_tokens = int(usage.get("output_tokens", 0) or 0)
    reasoning_output_tokens = int(
        usage.get("reasoning_output_tokens", 0)
        or pointer(usage, "output_tokens_details", "reasoning_tokens")
        or 0
    )
    input_plus_output_tokens = (
        int(usage.get("total_tokens", 0) or 0) or input_tokens + output_tokens
    )
    uncached_input_tokens = max(0, input_tokens - cached_input_tokens)
    return {
        "turns": 1,
        "input_tokens": input_tokens,
        "cached_input_tokens": cached_input_tokens,
        "output_tokens": output_tokens,
        "reasoning_output_tokens": reasoning_output_tokens,
        "input_plus_output_tokens": input_plus_output_tokens,
        "uncached_input_tokens": uncached_input_tokens,
        "uncached_input_plus_output_tokens": uncached_input_tokens + output_tokens,
    }


def pointer(value: object, *path: str) -> object | None:
    for key in path:
        if not isinstance(value, dict):
            return None
        value = value.get(key)
    return value


def first_dict(*values: object) -> dict[str, Any] | None:
    return next((value for value in values if isinstance(value, dict)), None)


def add_aweagent_to_path(aweagent_root: Path) -> None:
    root = str(aweagent_root.resolve())
    if root not in sys.path:
        sys.path.insert(0, root)


def _safe_extract_tar(tar: tarfile.TarFile, destination: Path) -> None:
    destination_root = destination.resolve()

    def inside_destination(path: Path) -> bool:
        resolved = path.resolve()
        return resolved == destination_root or destination_root in resolved.parents

    for member in tar.getmembers():
        target = (destination / member.name).resolve()
        if not inside_destination(target):
            raise RuntimeError(f"unsafe archive member: {member.name}")
        if member.issym():
            link_target = Path(member.linkname)
            resolved_link = (
                link_target.resolve()
                if link_target.is_absolute()
                else (target.parent / link_target).resolve()
            )
            if not inside_destination(resolved_link):
                raise RuntimeError(
                    f"unsafe archive link: {member.name} -> {member.linkname}"
                )
        elif member.islnk():
            link_target = Path(member.linkname)
            resolved_link = (
                link_target.resolve()
                if link_target.is_absolute()
                else (destination / link_target).resolve()
            )
            if not inside_destination(resolved_link):
                raise RuntimeError(
                    f"unsafe archive link: {member.name} -> {member.linkname}"
                )
    tar.extractall(destination)


async def export_session_workspace(session: Any, remote_workdir: str, workspace: Path) -> None:
    """Export a prepared Docker runtime workdir into a host workspace."""

    container = getattr(session, "_container", None)
    if container is None:
        raise RuntimeError("runtime session does not expose a container archive export")

    workspace = workspace.resolve()
    workspace.parent.mkdir(parents=True, exist_ok=True)
    if workspace.exists():
        shutil.rmtree(workspace)

    def _export() -> None:
        with tempfile.TemporaryDirectory(prefix="workspace-export-", dir=workspace.parent) as tmp:
            tmp_path = Path(tmp)
            archive_path = tmp_path / "workspace.tar"
            bits, _ = container.get_archive(remote_workdir)
            with archive_path.open("wb") as handle:
                for chunk in bits:
                    handle.write(chunk)

            extract_path = tmp_path / "extract"
            extract_path.mkdir()
            with tarfile.open(archive_path) as tar:
                _safe_extract_tar(tar, extract_path)

            exported_root = extract_path / Path(remote_workdir).name
            source = exported_root if exported_root.is_dir() else extract_path
            copy_exported_workspace(source, workspace)

    await asyncio.to_thread(_export)


def copy_exported_workspace(source: Path, workspace: Path) -> None:
    shutil.copytree(
        source,
        workspace,
        symlinks=True,
        ignore=shutil.ignore_patterns(*WORKSPACE_COPY_IGNORE_PATTERNS),
    )


def runtime_backend(runtime_config: Any) -> str:
    return str(getattr(runtime_config, "backend", ""))


def runtime_pull_policy(runtime_config: Any) -> str:
    docker_config = getattr(runtime_config, "docker", None)
    return str(getattr(docker_config, "pull_policy", "if_not_present"))


def runtime_config_with_pull_policy(runtime_config: Any, pull_policy: str) -> Any:
    if runtime_backend(runtime_config) != "docker":
        return runtime_config
    docker_config = getattr(runtime_config, "docker", None)
    if docker_config is None:
        return runtime_config
    docker_copy = (
        docker_config.model_copy(update={"pull_policy": pull_policy})
        if hasattr(docker_config, "model_copy")
        else docker_config
    )
    if not hasattr(docker_config, "model_copy"):
        setattr(docker_copy, "pull_policy", pull_policy)
    return runtime_config.model_copy(update={"docker": docker_copy})


def runtime_config_for_local_image(
    runtime_config: Any,
    image: str,
    workdir: str,
) -> Any:
    configured = runtime_config.model_copy(update={"image": image, "workdir": workdir})
    return runtime_config_with_pull_policy(configured, "never")


def docker_client_from_env() -> Any:
    import docker

    return docker.from_env()


async def ensure_runtime_image_available(
    runtime_config: Any,
    image: str,
    client_factory: Any = docker_client_from_env,
) -> bool:
    if runtime_backend(runtime_config) != "docker" or not image:
        return False

    pull_policy = runtime_pull_policy(runtime_config)

    def _ensure() -> bool:
        client = client_factory()
        if pull_policy == "always":
            client.images.pull(image)
            return True
        if pull_policy == "never":
            client.images.get(image)
            return False
        if pull_policy != "if_not_present":
            raise ValueError(f"unsupported Docker pull policy: {pull_policy}")
        try:
            client.images.get(image)
            return False
        except Exception:
            client.images.pull(image)
            return True

    return await asyncio.to_thread(_ensure)


def is_missing_runtime_image_error(error: BaseException) -> bool:
    text = repr(error)
    return (
        "ImageNotFound" in text
        or "NotFound" in text
        or "404 Client Error" in text
    )


async def preflight_runtime_image_available(
    runtime_config: Any,
    image: str,
    client_factory: Any = docker_client_from_env,
) -> None:
    if runtime_backend(runtime_config) != "docker" or not image:
        return

    pull_policy = runtime_pull_policy(runtime_config)

    def _preflight() -> None:
        client = client_factory()
        if pull_policy == "never":
            client.images.get(image)
            return
        if pull_policy == "if_not_present":
            try:
                client.images.get(image)
                return
            except Exception as local_error:
                if not is_missing_runtime_image_error(local_error):
                    raise
        elif pull_policy != "always":
            raise ValueError(f"unsupported Docker pull policy: {pull_policy}")

        registry_lookup = getattr(client.images, "get_registry_data", None)
        if registry_lookup is None:
            return
        registry_lookup(image)

    try:
        await asyncio.to_thread(_preflight)
    except Exception as error:
        if is_missing_runtime_image_error(error):
            raise MissingRuntimeImageError(image, error) from error
        raise


async def delete_runtime_image_after_instance(
    runtime_config: Any,
    image: str | None,
    enabled: bool,
    client_factory: Any = docker_client_from_env,
) -> bool:
    if not enabled or runtime_backend(runtime_config) != "docker" or not image:
        return False

    def _delete() -> bool:
        client = client_factory()
        client.images.remove(image, force=True)
        return True

    try:
        return await asyncio.to_thread(_delete)
    except Exception:
        return False


def cleanup_codex_home_caches(env: dict[str, str]) -> list[str]:
    home_text = env.get("HOME")
    if not home_text:
        return []

    home = Path(home_text)
    home_resolved = home.resolve(strict=False)
    candidates = [
        Path(env["XDG_CACHE_HOME"]) if env.get("XDG_CACHE_HOME") else home / ".cache",
        home / "Library" / "Caches",
    ]
    removed = []
    seen = set()
    for candidate in candidates:
        resolved = candidate.resolve(strict=False)
        if resolved in seen:
            continue
        seen.add(resolved)
        if resolved == home_resolved or not resolved.is_relative_to(home_resolved):
            continue
        if not candidate.exists():
            continue
        if candidate.is_dir() and not candidate.is_symlink():
            shutil.rmtree(candidate, ignore_errors=True)
        else:
            try:
                candidate.unlink()
            except OSError:
                continue
        if not candidate.exists():
            removed.append(str(candidate))
    return removed


def disk_usage_probe_path(path: Path) -> Path:
    probe = path
    while not probe.exists() and probe.parent != probe:
        probe = probe.parent
    return probe


def low_disk_space_result(
    instance_id: str,
    output: Path,
    min_free_bytes: int,
    disk_usage: Any = shutil.disk_usage,
) -> InstanceResult | None:
    if min_free_bytes <= 0:
        return None
    probe = disk_usage_probe_path(output)
    free_bytes = disk_usage(probe).free
    if free_bytes >= min_free_bytes:
        return None
    return InstanceResult(
        instance_id,
        False,
        None,
        "disk-space-low",
        (
            f"free disk space {free_bytes} bytes is below required "
            f"{min_free_bytes} bytes at {probe}"
        ),
        None,
    )


def build_denovo_evaluator(
    evaluator_cls: Any,
    args: argparse.Namespace,
    config: Any,
) -> Any:
    # The adapter owns image cleanup so all eval iterations/test cases for an
    # instance can finish before the image is removed once.
    return evaluator_cls(
        timeout=config.eval.timeout,
        validate_run=args.validate_run,
        del_done_images=False,
        eval_iters=args.eval_iters,
    )


def codex_command_for_profile(
    workspace: Path,
    agent_mode: str,
    subagent: str,
    codex_bin: str,
    stateful_binary: str,
    benchmark_model: str,
    benchmark_reasoning_effort: str,
    benchmark_model_context_window: int,
    benchmark_temperature: str,
    base_env: dict[str, str] | None = None,
) -> list[str]:
    source_env = os.environ if base_env is None else base_env
    nested_benchmark = bool(source_env.get(NESTED_CODEX_HOME_ROOT_ENV))
    sandbox = "danger-full-access" if nested_benchmark else "workspace-write"
    command = [
        codex_bin,
        "--model",
        benchmark_model,
        "-c",
        f"model_reasoning_effort={toml_string(benchmark_reasoning_effort)}",
        "-c",
        f"model_context_window={benchmark_model_context_window}",
        "-c",
        f"temperature={benchmark_temperature}",
        "-c",
        "skills.bundled.enabled=false",
        "--ask-for-approval",
        "never",
        "exec",
        "--json",
        "--ignore-rules",
        "--dangerously-bypass-hook-trust",
        "--cd",
        str(workspace),
        "--sandbox",
        sandbox,
    ]
    if subagent == "on":
        command.extend(["-c", "features.multi_agent=true"])
    if agent_mode == "no-state":
        if not nested_benchmark:
            command.append("--ignore-user-config")
    elif agent_mode != "stateful":
        raise ValueError(f"unsupported agent_mode: {agent_mode}")
    if not nested_benchmark:
        command.extend(["-c", "sandbox_workspace_write.network_access=true"])
    command.append("-")
    return command


def omp_command_for_profile(
    workspace: Path,
    prompt_path: Path,
    omp_bin: str,
    benchmark_model: str,
    benchmark_reasoning_effort: str = DEFAULT_OMP_REASONING_EFFORT,
    enable_native_subagent: bool = False,
    subagent_min_count: int = DEFAULT_SUBAGENT_MIN_COUNT,
) -> list[str]:
    command = [
        omp_bin,
        "-p",
        "--mode",
        "json",
        "--model",
        benchmark_model,
        "--thinking",
        benchmark_reasoning_effort,
        "--cwd",
        str(workspace),
        "--approval-mode",
        "yolo",
        "--no-title",
        f"@{prompt_path.resolve()}",
    ]
    return command

def docker_host_url(value: str) -> str:
    parsed = urllib.parse.urlparse(value)
    if parsed.hostname not in {"127.0.0.1", "localhost"}:
        return value
    netloc = "host.docker.internal"
    if parsed.port is not None:
        netloc = f"{netloc}:{parsed.port}"
    return urllib.parse.urlunparse(parsed._replace(netloc=netloc))


def omp_agent_docker_env(base_env: dict[str, str]) -> dict[str, str]:
    env: dict[str, str] = {
        "HOME": OMP_AGENT_DOCKER_HOME,
        "PI_CODING_AGENT_DIR": f"{OMP_AGENT_DOCKER_HOME}/.omp/profiles/stateful/agent",
        "STATEFUL_HOME": OMP_AGENT_DOCKER_HOME,
        "XDG_CACHE_HOME": f"{OMP_AGENT_DOCKER_HOME}/.cache",
        "XDG_CONFIG_HOME": f"{OMP_AGENT_DOCKER_HOME}/.config",
    }
    for key, value in base_env.items():
        if key in OMP_AGENT_DOCKER_ENV_ALLOWLIST or key.startswith(OMP_AGENT_DOCKER_ENV_PREFIXES):
            env[key] = docker_host_url(value) if key == "STATEFUL_SERVER_URL" else value
    return env


def docker_omp_command_for_profile(
    workspace: Path,
    prompt_path: Path,
    home: Path,
    omp_bin: str,
    benchmark_model: str,
    docker_image: str,
    base_env: dict[str, str],
    benchmark_reasoning_effort: str = DEFAULT_OMP_REASONING_EFFORT,
    docker_bin: str = "docker",
    enable_native_subagent: bool = False,
    subagent_min_count: int = DEFAULT_SUBAGENT_MIN_COUNT,
    sandbox: str = "on",
) -> list[str]:
    docker_env = omp_agent_docker_env(base_env)
    if sandbox == "off":
        docker_env["STATEFUL_OMP_SANDBOX"] = "off"
    command = [
        docker_bin,
        "run",
        "--rm",
        "--network",
        "bridge",
        "--workdir",
        OMP_AGENT_DOCKER_WORKSPACE,
        "--mount",
        f"type=bind,source={workspace.resolve()},target={OMP_AGENT_DOCKER_WORKSPACE}",
        "--mount",
        f"type=bind,source={prompt_path.resolve()},target={OMP_AGENT_DOCKER_PROMPT},readonly",
        "--mount",
        f"type=bind,source={home.resolve()},target={OMP_AGENT_DOCKER_HOME}",
    ]
    for key in sorted(docker_env):
        if key in OMP_AGENT_DOCKER_ARG_VALUE_ENV:
            command.extend(["--env", f"{key}={docker_env[key]}"])
        else:
            command.extend(["--env", key])
    command.append(docker_image)
    command.extend(
        omp_command_for_profile(
            workspace=Path(OMP_AGENT_DOCKER_WORKSPACE),
            prompt_path=Path(OMP_AGENT_DOCKER_PROMPT),
            omp_bin=omp_bin,
            benchmark_model=benchmark_model,
            benchmark_reasoning_effort=benchmark_reasoning_effort,
            enable_native_subagent=enable_native_subagent,
            subagent_min_count=subagent_min_count,
        )
    )
    return command


def denovo_codex_environment(
    output: Path,
    instance_id: str,
    task_path: Path,
    workspace: Path,
    base_env: dict[str, str] | None = None,
) -> dict[str, str]:
    source_env = os.environ if base_env is None else base_env
    env = dict(source_env)
    nested_root = source_env.get(NESTED_CODEX_HOME_ROOT_ENV)
    if nested_root:
        output_scope_parts = output.parts[-4:] if len(output.parts) >= 4 else output.parts
        output_scope = path_fragment("--".join(output_scope_parts))
        scope_digest = path_scope_digest(output, workspace, task_path)
        home = Path(nested_root) / output_scope / path_fragment(instance_id) / scope_digest / "home"
    else:
        home = output / "codex-homes" / path_fragment(instance_id) / "home"
    env["HOME"] = str(home)
    env["CODEX_HOME"] = str(home / ".codex")
    env["XDG_CONFIG_HOME"] = str(home / ".config")
    env["XDG_CACHE_HOME"] = str(home / ".cache")

    system_cert = Path("/etc/ssl/cert.pem")
    if not env.get("SSL_CERT_FILE") and system_cert.is_file():
        env["SSL_CERT_FILE"] = str(system_cert)

    return env


def denovo_omp_environment(
    output: Path,
    instance_id: str,
    task_path: Path,
    workspace: Path,
    base_env: dict[str, str] | None = None,
) -> dict[str, str]:
    source_env = os.environ if base_env is None else base_env
    env = dict(source_env)
    env.pop("CODEX_HOME", None)
    home = output / "omp-homes" / path_fragment(instance_id) / "home"
    auth_source_agent = source_env.get("OMP_AUTH_SOURCE_AGENT_DIR")
    if not auth_source_agent:
        source_home = Path(source_env.get("HOME", "")).expanduser()
        for candidate in (
            source_home / ".omp" / "profiles" / "stateful" / "agent",
            source_home / ".omp" / "agent",
        ):
            if (candidate / "agent.db").exists():
                auth_source_agent = str(candidate)
                break
    if auth_source_agent:
        env["OMP_AUTH_SOURCE_AGENT_DIR"] = auth_source_agent
    env["HOME"] = str(home)
    env["STATEFUL_HOME"] = str(home)
    env["PI_CODING_AGENT_DIR"] = str(home / ".omp" / "profiles" / "stateful" / "agent")
    env["XDG_CONFIG_HOME"] = str(home / ".config")
    env["XDG_CACHE_HOME"] = str(home / ".cache")
    return env

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


def denovo_source_guard_extension_source() -> str:
    return """const BENCHMARK_SOURCE_BLOCK_ENV = "STATEFUL_BENCHMARK_SOURCE_BLOCK_PATTERNS";

function benchmarkSourceBlockPatterns() {
  const raw = process.env[BENCHMARK_SOURCE_BLOCK_ENV];
  if (!raw) return [];
  try {
    const parsed = JSON.parse(raw);
    if (Array.isArray(parsed)) {
      return parsed.map((item) => String(item || "").trim()).filter(Boolean);
    }
  } catch (_) {}
  return String(raw).split(/[\\r\\n,]+/).map((item) => item.trim()).filter(Boolean);
}

function benchmarkSourcePatternMatches(text, pattern) {
  const lowerPattern = pattern.toLowerCase();
  if (lowerPattern === "upstream" || lowerPattern === "upstream/") {
    return /(^|[^a-z0-9_-])upstream(?:\\/|[^a-z0-9_-]|$)/.test(text);
  }
  return text.includes(lowerPattern);
}

function benchmarkSourceBlockReason(event) {
  const patterns = benchmarkSourceBlockPatterns();
  if (patterns.length === 0) return "";
  const text = (String(event?.toolName || "") + "\\n" + JSON.stringify(event?.input || {})).toLowerCase();
  for (const pattern of patterns) {
    if (benchmarkSourcePatternMatches(text, pattern)) {
      return "DeNovo benchmark blocked target upstream source access before tool execution: " + pattern;
    }
  }
  return "";
}

export default function denovoBenchmarkSourceGuard(pi) {
  pi.on("tool_call", async (event) => {
    const benchmarkBlockReason = benchmarkSourceBlockReason(event);
    if (benchmarkBlockReason) return { block: true, reason: benchmarkBlockReason };
  });
}
"""


def install_non_stateful_omp_source_guard(env: dict[str, str]) -> None:
    agent_dir = Path(env["PI_CODING_AGENT_DIR"])
    extension_path = agent_dir / "extensions" / "denovo-benchmark-source-guard.js"
    extension_path.parent.mkdir(parents=True, exist_ok=True)
    extension_path.write_text(denovo_source_guard_extension_source(), encoding="utf-8")

    config_path = agent_dir / "config.yml"
    config_path.parent.mkdir(parents=True, exist_ok=True)
    entry = f"  - {extension_path}"
    contents = config_path.read_text(encoding="utf-8") if config_path.exists() else ""
    if any(line.strip() == entry.strip() for line in contents.splitlines()):
        return
    lines = contents.splitlines()
    for offset, line in enumerate(lines):
        if line.strip() == "extensions:":
            lines.insert(offset + 1, entry)
            config_path.write_text("\n".join(lines) + "\n", encoding="utf-8")
            return
    if contents and not contents.endswith("\n"):
        contents += "\n"
    contents += "extensions:\n"
    contents += entry
    contents += "\n"
    config_path.write_text(contents, encoding="utf-8")


def prepare_omp_environment(
    env: dict[str, str],
    enable_stateful: bool,
    stateful_binary: str,
    runner: Any = subprocess.run,
    runtime_stateful_binary: str | None = None,
    runtime_omp_home: str | None = None,
    omp_bin: str | None = None,
    enable_native_subagent: bool = False,
    agent_docker_image: str | None = None,
    docker_bin: str = "docker",
) -> None:
    Path(env["PI_CODING_AGENT_DIR"]).mkdir(parents=True, exist_ok=True)
    if enable_native_subagent:
        if omp_bin is None:
            raise StatefulRepoEnableError("OMP subagent:on requires an omp binary to unpack task agents")
        if agent_docker_image:
            completed = runner(
                [
                    docker_bin,
                    "run",
                    "--rm",
                    "--mount",
                    f"type=bind,source={Path(env['HOME']).resolve()},target={OMP_AGENT_DOCKER_HOME}",
                    "--env",
                    f"HOME={OMP_AGENT_DOCKER_HOME}",
                    "--env",
                    f"PI_CODING_AGENT_DIR={OMP_AGENT_DOCKER_HOME}/.omp/profiles/stateful/agent",
                    "--env",
                    f"STATEFUL_HOME={OMP_AGENT_DOCKER_HOME}",
                    "--env",
                    f"XDG_CACHE_HOME={OMP_AGENT_DOCKER_HOME}/.cache",
                    "--env",
                    f"XDG_CONFIG_HOME={OMP_AGENT_DOCKER_HOME}/.config",
                    agent_docker_image,
                    omp_bin,
                    "agents",
                    "unpack",
                    "--force",
                ],
                text=True,
                check=False,
                env=env,
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
            )
        else:
            completed = runner(
                [omp_bin, "agents", "unpack", "--force"],
                text=True,
                check=False,
                env=env,
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
            )
        if completed.returncode != 0:
            message = (completed.stderr or completed.stdout).strip()
            raise StatefulRepoEnableError(message or f"omp agents unpack exited {completed.returncode}")
    if not enable_stateful:
        install_non_stateful_omp_source_guard(env)
        if runtime_omp_home is not None:
            rewrite_omp_config_for_runtime_home(env, runtime_omp_home)
        return
    command = [stateful_binary, "install", "--agent", "omp", "--yes"]
    if runtime_stateful_binary and runtime_stateful_binary != stateful_binary:
        command.extend(["--binary", runtime_stateful_binary])
    completed = runner(
        command,
        text=True,
        check=False,
        env=env,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
    )
    if completed.returncode != 0:
        message = (completed.stderr or completed.stdout).strip()
        raise StatefulRepoEnableError(message or f"stateful omp install exited {completed.returncode}")
    if runtime_omp_home is not None:
        rewrite_omp_config_for_runtime_home(env, runtime_omp_home)


def rewrite_omp_config_for_runtime_home(env: dict[str, str], runtime_omp_home: str) -> None:
    host_home = Path(env["HOME"])
    config_path = Path(env["PI_CODING_AGENT_DIR"]) / "config.yml"
    if not config_path.exists():
        raise StatefulRepoEnableError(f"OMP stateful config missing: {config_path}")
    contents = config_path.read_text(encoding="utf-8")
    rewritten = contents.replace(str(host_home), runtime_omp_home.rstrip("/"))
    config_path.write_text(rewritten, encoding="utf-8")


def stateful_agent_id_fragment(value: str) -> str:
    fragment = "".join(
        character if character.isalnum() or character in "_-" else "-"
        for character in str(value)
    ).strip("-_")
    return fragment or "item"


def denovo_stateful_agent_id(
    output: Path,
    instance_id: str,
    task_path: Path,
    workspace: Path,
) -> str:
    return (
        f"denovo-{stateful_agent_id_fragment(instance_id)}-"
        f"{path_scope_digest(output, task_path, workspace)}"
    )


def native_subagent_usage(
    subagent: str,
    subagent_min_count: int,
    native_home: Path,
    cli_runtime: str = "codex",
) -> dict[str, Any]:
    native_usage = annotate_native_subagent_usage(
        detect_native_subagent_usage(native_home),
        subagent_min_count,
    )
    spawn_count = native_usage["subagent_spawn_count"]
    requirement_met = subagent != "on" or spawn_count >= subagent_min_count
    mode = f"native_{cli_runtime}_subagents" if subagent == "on" else "off"
    return {
        "mode": mode,
        "subagent_used": bool(native_usage["subagent_used"]),
        "subagent_requirement_met": requirement_met,
        "native_subagent": native_usage,
    }


def empty_native_subagent_usage(subagent_min_count: int) -> dict[str, Any]:
    native_usage = annotate_native_subagent_usage(
        {
            "subagent_used": False,
            "sources": [],
            "counts": empty_subagent_usage_counts(),
        },
        subagent_min_count,
    )
    return {
        "mode": "off",
        "subagent_min_count": subagent_min_count,
        "subagent_used": False,
        "subagent_requirement_met": True,
        "native_subagent": native_usage,
    }


def stateful_workspace_id_from_repo_metadata(
    env: dict[str, str],
    workspace_root: Path | str,
) -> str | None:
    repos_dir = Path(env.get("STATEFUL_HOME") or env["HOME"]) / "repos"
    if not repos_dir.exists():
        return None
    roots = {str(workspace_root).rstrip("/")}
    try:
        roots.add(str(Path(workspace_root).resolve()).rstrip("/"))
    except OSError:
        pass
    workspace_ids: list[str] = []
    for metadata_path in repos_dir.glob("*.json"):
        metadata = json.loads(metadata_path.read_text(encoding="utf-8"))
        repo_id = metadata.get("repo_id")
        if not isinstance(repo_id, str) or not repo_id.startswith("repo-"):
            continue
        workspace_id = "workspace-" + repo_id.removeprefix("repo-")
        workspace_ids.append(workspace_id)
        root = metadata.get("root")
        if isinstance(root, str) and root.rstrip("/") in roots:
            return workspace_id
    return workspace_ids[0] if len(workspace_ids) == 1 else None


def enable_stateful_repo(
    env: dict[str, str],
    workspace: Path,
    stateful_binary: str,
    runner: Any = subprocess.run,
    runtime_workspace: str | None = None,
) -> StatefulRepoEnableCleanup:
    stateful_dir = workspace / ".stateful"
    policy_config = stateful_dir / "config.yml"
    cleanup = StatefulRepoEnableCleanup(
        created_stateful_dir=not stateful_dir.exists(),
        created_policy_config=not policy_config.exists(),
    )
    completed = runner(
        [stateful_binary, "enable", "--repo", str(workspace)],
        cwd=workspace,
        env=env,
        text=True,
        check=False,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
    )
    if completed.returncode != 0:
        cleanup_stateful_repo_enable(workspace, cleanup)
        message = (completed.stderr or completed.stdout).strip()
        if not message:
            message = f"stateful enable exited {completed.returncode}"
        raise StatefulRepoEnableError(message)
    if runtime_workspace is not None:
        rewrite_stateful_repo_metadata_for_runtime_workspace(env, workspace, runtime_workspace)
    workspace_id = stateful_workspace_id_from_repo_metadata(
        env,
        runtime_workspace or workspace,
    )
    if workspace_id is not None:
        env["STATEFUL_WORKSPACE_ID"] = workspace_id
    return cleanup


def rewrite_stateful_repo_metadata_for_runtime_workspace(
    env: dict[str, str],
    workspace: Path,
    runtime_workspace: str,
) -> None:
    stateful_home = Path(env.get("STATEFUL_HOME") or env["HOME"])
    repos_dir = stateful_home / "repos"
    if not repos_dir.exists():
        raise StatefulRepoEnableError(f"stateful repo metadata missing: {repos_dir}")
    registry_path = stateful_home / "config.yml"
    if not registry_path.exists():
        raise StatefulRepoEnableError(f"stateful repo registry missing: {registry_path}")
    host_workspace_strings = sorted(
        {str(workspace), str(workspace.resolve())},
        key=len,
        reverse=True,
    )
    registry = registry_path.read_text(encoding="utf-8")
    for host_workspace in host_workspace_strings:
        registry = registry.replace(host_workspace, runtime_workspace.rstrip("/"))
    registry_path.write_text(registry, encoding="utf-8")
    host_workspaces = [workspace, workspace.resolve()]
    runtime_workspace_path = PurePosixPath(runtime_workspace.rstrip("/"))
    for metadata_path in repos_dir.glob("*.json"):
        metadata = json.loads(metadata_path.read_text(encoding="utf-8"))
        changed = False
        for key in ("root", "policy_config_path"):
            value = metadata.get(key)
            if not isinstance(value, str):
                continue
            host_value = Path(value)
            for host_workspace in host_workspaces:
                try:
                    relative = host_value.relative_to(host_workspace)
                except ValueError:
                    continue
                metadata[key] = str(runtime_workspace_path / PurePosixPath(relative.as_posix()))
                changed = True
                break
        if changed:
            metadata_path.write_text(
                json.dumps(metadata, indent=2, sort_keys=True) + "\n",
                encoding="utf-8",
            )


def cleanup_stateful_repo_enable(
    workspace: Path,
    cleanup: StatefulRepoEnableCleanup | None,
) -> None:
    if cleanup is None:
        return

    stateful_dir = workspace / ".stateful"
    policy_config = stateful_dir / "config.yml"
    if cleanup.created_policy_config:
        try:
            if policy_config.is_file() or policy_config.is_symlink():
                policy_config.unlink()
        except FileNotFoundError:
            pass

    if cleanup.created_stateful_dir:
        try:
            stateful_dir.rmdir()
        except FileNotFoundError:
            pass
        except OSError:
            pass


def profile_metadata(
    agent_mode: str,
    subagent: str,
    subagent_min_count: int = DEFAULT_SUBAGENT_MIN_COUNT,
    cli_runtime: str = "codex",
) -> dict[str, Any]:
    native_subagents = subagent == "on"
    return {
        "agent_kind": "omp-cli" if cli_runtime == "omp" else "codex-cli",
        "agent_mode": agent_mode,
        "subagent": subagent,
        "subagent_required": native_subagents,
        "subagent_min_count": subagent_min_count,
        "subagent_mode": f"native_{cli_runtime}_subagents" if native_subagents else "off",
        "official_benchmark_protocol": OFFICIAL_BENCHMARK_PROTOCOL,
        "agent_rollouts_per_instance": 1,
        "native_subagent_required": native_subagents,
        "eval_feedback_loop": False,
        "eval_feedback_attempts": 0,
        "resume_policy": RESUME_POLICY_CONTEXT_OR_TOKEN_ONLY,
        "ignore_user_config": agent_mode == "no-state",
        "ignore_rules": True,
        "bundled_skills_disabled": True,
        "stateful_mcp": agent_mode == "stateful",
        "stateful_hooks": agent_mode == "stateful",
        "stateful_skill": agent_mode == "stateful",
    }


def parse_args(argv: list[str] | None = None) -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument("--data-file", required=True)
    parser.add_argument("--config", required=True)
    parser.add_argument("--mode", choices=["batch", "single"], required=True)
    parser.add_argument("--output", required=True)
    parser.add_argument("--agent-mode", choices=["stateful", "no-state"], required=True)
    parser.add_argument("--subagent", choices=["on", "off"], required=True)
    parser.add_argument("--aweagent-root", required=True)
    parser.add_argument("--cli-runtime", choices=["codex", "omp"], default="codex")
    parser.add_argument("--codex-bin", required=True)
    parser.add_argument("--omp-bin", default="omp")
    parser.add_argument("--stateful-binary", required=True)
    parser.add_argument("--agent-docker-image")
    parser.add_argument(
        "--agent-docker-stateful-binary",
        default=DEFAULT_OMP_AGENT_DOCKER_STATEFUL_BINARY,
    )
    parser.add_argument("--agent-docker-sandbox", choices=["on", "off"], default="on")
    parser.add_argument("--benchmark-model", required=True)
    parser.add_argument("--benchmark-reasoning-effort", required=True)
    parser.add_argument("--benchmark-model-context-window", type=int, required=True)
    parser.add_argument("--benchmark-temperature", required=True)
    parser.add_argument("--benchmark-max-turns", type=int, required=True)
    parser.add_argument("--subagent-min-count", type=positive_int, default=DEFAULT_SUBAGENT_MIN_COUNT)
    parser.add_argument("--max-resumes", type=int, required=True)
    parser.add_argument("--codex-timeout-seconds", type=int, required=True)
    parser.add_argument("--max-steps", type=int)
    parser.add_argument("--max-concurrent", type=int)
    parser.add_argument("--instance-id", action="append", default=[])
    parser.add_argument("--skip-eval", action="store_true")
    parser.add_argument("--validate-run", action="store_true")
    parser.add_argument("--del-done-images", dest="del_done_images", action="store_true", default=True)
    parser.add_argument("--keep-done-images", dest="del_done_images", action="store_false")
    parser.add_argument("--dump-clean-snapshot")
    parser.add_argument("--eval-iters", type=int, required=True)
    parser.add_argument("--prompt-version", required=True)
    parser.add_argument("--min-free-disk-gb", type=float, default=DEFAULT_MIN_FREE_DISK_GB)
    parser.add_argument("--verbose", action="store_true")
    return parser.parse_args(argv)


def positive_int(value: str) -> int:
    parsed = int(value)
    if parsed < 1:
        raise argparse.ArgumentTypeError("value must be at least 1")
    return parsed


def read_jsonl(path: Path) -> list[dict[str, Any]]:
    rows: list[dict[str, Any]] = []
    with path.open("r", encoding="utf-8") as handle:
        for line in handle:
            stripped = line.strip()
            if stripped:
                rows.append(json.loads(stripped))
    return rows


def write_json(path: Path, value: Any) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(
        json.dumps(value, indent=2, sort_keys=True, default=str) + "\n",
        encoding="utf-8",
    )


def write_jsonl(path: Path, rows: Iterable[dict[str, Any]]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    with path.open("w", encoding="utf-8") as handle:
        for row in rows:
            handle.write(json.dumps(row, sort_keys=True, default=str) + "\n")


def append_jsonl(path: Path, row: dict[str, Any]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    with path.open("a", encoding="utf-8") as handle:
        handle.write(json.dumps(row, sort_keys=True, default=str) + "\n")


SUBAGENT_TABLES = (
    "agent_jobs",
    "agent_job_items",
    "thread_spawn_edges",
    "thread_dynamic_tools",
)
SUBAGENT_TOOL_NAMES = {
    "spawn_agent": "spawn_agent_calls",
    "multi_agent_v1spawn_agent": "spawn_agent_calls",
    "multi_agent_v1.spawn_agent": "spawn_agent_calls",
    "multi_agent_v1__spawn_agent": "spawn_agent_calls",
    "task": "spawn_agent_calls",
    "wait_agent": "wait_agent_calls",
    "multi_agent_v1wait_agent": "wait_agent_calls",
    "multi_agent_v1.wait_agent": "wait_agent_calls",
    "multi_agent_v1__wait_agent": "wait_agent_calls",
    "send_input": "send_input_calls",
    "multi_agent_v1send_input": "send_input_calls",
    "multi_agent_v1.send_input": "send_input_calls",
    "multi_agent_v1__send_input": "send_input_calls",
    "close_agent": "close_agent_calls",
    "multi_agent_v1close_agent": "close_agent_calls",
    "multi_agent_v1.close_agent": "close_agent_calls",
    "multi_agent_v1__close_agent": "close_agent_calls",
}


def empty_subagent_usage_counts() -> dict[str, int]:
    counts = {table: 0 for table in SUBAGENT_TABLES}
    counts.update(
        {
            "spawn_agent_calls": 0,
            "wait_agent_calls": 0,
            "send_input_calls": 0,
            "close_agent_calls": 0,
        }
    )
    return counts


def sqlite_table_count(db_path: Path, table: str) -> int:
    try:
        with sqlite3.connect(db_path) as connection:
            row = connection.execute(f'SELECT COUNT(*) FROM "{table}"').fetchone()
    except sqlite3.Error:
        return 0
    return int(row[0]) if row else 0


def response_item_function_name(event: dict[str, Any]) -> str | None:
    candidates = [event, event.get("payload"), event.get("item")]
    for candidate in candidates:
        if not isinstance(candidate, dict):
            continue
        call_type = candidate.get("type")
        if call_type in {"function_call", "custom_tool_call"}:
            name = candidate.get("name")
            if isinstance(name, str):
                return name
        function_call = candidate.get("function_call")
        if isinstance(function_call, dict):
            name = function_call.get("name")
            if isinstance(name, str):
                return name
    return None


def omp_session_tool_calls(event: dict[str, Any]) -> list[tuple[str, Any]]:
    message = event.get("message")
    if not isinstance(message, dict):
        return []
    content = message.get("content")
    if not isinstance(content, list):
        return []
    calls: list[tuple[str, Any]] = []
    for item in content:
        if not isinstance(item, dict) or item.get("type") != "toolCall":
            continue
        name = item.get("name")
        if isinstance(name, str):
            calls.append((name, item.get("arguments")))
    return calls


def subagent_tool_call_weight(name: str, arguments: Any) -> int:
    if name != "task" or not isinstance(arguments, dict):
        return 1
    tasks = arguments.get("tasks")
    if isinstance(tasks, list) and tasks:
        return len(tasks)
    return 1


def add_subagent_tool_call_count(
    counts: dict[str, int],
    name: str | None,
    arguments: Any = None,
) -> bool:
    count_key = SUBAGENT_TOOL_NAMES.get(name or "")
    if count_key is None:
        return False
    counts[count_key] += subagent_tool_call_weight(name or "", arguments)
    return True


def detect_native_subagent_usage(codex_home: Path) -> dict[str, Any]:
    counts = empty_subagent_usage_counts()
    sources: set[str] = set()

    for db_path in sorted(codex_home.glob("state*.sqlite")):
        for table in SUBAGENT_TABLES:
            table_count = sqlite_table_count(db_path, table)
            counts[table] += table_count
            if table_count and table != "thread_dynamic_tools":
                sources.add("codex_state_db")

    sessions_dir = codex_home / "sessions"
    if sessions_dir.is_dir():
        for session_log in sorted(sessions_dir.rglob("*.jsonl")):
            try:
                lines = session_log.read_text(encoding="utf-8").splitlines()
            except OSError:
                continue
            for line in lines:
                if not line.strip():
                    continue
                try:
                    event = json.loads(line)
                except json.JSONDecodeError:
                    continue
                if not isinstance(event, dict):
                    continue
                name = response_item_function_name(event)
                if add_subagent_tool_call_count(counts, name):
                    sources.add("codex_session_log")
                for omp_name, arguments in omp_session_tool_calls(event):
                    if add_subagent_tool_call_count(counts, omp_name, arguments):
                        sources.add("omp_session_log")

    used_keys = (
        "agent_jobs",
        "agent_job_items",
        "thread_spawn_edges",
        "spawn_agent_calls",
        "wait_agent_calls",
        "send_input_calls",
        "close_agent_calls",
    )
    return {
        "subagent_used": any(counts[key] > 0 for key in used_keys),
        "sources": sorted(sources),
        "counts": counts,
    }


def native_subagent_spawn_count(usage: dict[str, Any]) -> int:
    counts = usage.get("counts", {})
    if not isinstance(counts, dict):
        return 0
    return max(
        int(counts.get("spawn_agent_calls", 0) or 0),
        int(counts.get("thread_spawn_edges", 0) or 0),
        int(counts.get("agent_jobs", 0) or 0),
    )


def native_subagent_wait_count(usage: dict[str, Any]) -> int:
    counts = usage.get("counts", {})
    if not isinstance(counts, dict):
        return 0
    return int(counts.get("wait_agent_calls", 0) or 0)


def annotate_native_subagent_usage(
    usage: dict[str, Any],
    subagent_min_count: int,
) -> dict[str, Any]:
    annotated = dict(usage)
    annotated["subagent_spawn_count"] = native_subagent_spawn_count(usage)
    annotated["subagent_wait_count"] = native_subagent_wait_count(usage)
    annotated["subagent_min_count"] = subagent_min_count
    return annotated


def subagent_usage_metadata(
    results: list[InstanceResult],
    subagent_min_count: int = DEFAULT_SUBAGENT_MIN_COUNT,
) -> dict[str, Any]:
    observed = [result for result in results if result.subagent_used is not None]
    used_count = sum(1 for result in observed if result.subagent_used)
    requirement_met_count = sum(
        1
        for result in observed
        if result.subagent_usage
        and result.subagent_usage.get("subagent_requirement_met") is not False
    )
    return {
        "subagent_min_count": subagent_min_count,
        "subagent_observed_instances": len(observed),
        "subagent_used_count": used_count,
        "subagent_used_any": used_count > 0,
        "subagent_requirement_met_count": requirement_met_count,
        "subagent_requirement_met_any": requirement_met_count > 0,
    }

def instance_owner_repo(instance_id: str) -> tuple[str, str] | None:
    prefix, separator, _ = instance_id.rpartition("_pr")
    if not separator or "_" not in prefix:
        return None
    owner, repo = prefix.split("_", 1)
    if not owner or not repo:
        return None
    return owner, repo


def benchmark_source_leak_url_patterns(instance_id: str) -> tuple[str, ...]:
    owner_repo = instance_owner_repo(instance_id)
    if owner_repo is None:
        return ()
    owner, repo = owner_repo
    repo_path = f"{owner}/{repo}".lower()
    return (
        *(f"{host}/{repo_path}" for host in BENCHMARK_SOURCE_LEAK_HOST_PATTERNS),
        f"patch-diff.githubusercontent.com/raw/{repo_path}",
        f"git@github.com:{repo_path}",
        f"pr://{repo_path}",
        f"issue://{repo_path}",
    )


def benchmark_source_leak_local_path_pattern(text: str) -> str | None:
    if re.search(r"(?<![a-z0-9_-])upstream(?![a-z0-9_-])", text.lower()):
        return "upstream/"
    return None


def benchmark_source_leak_command_pattern(
    text: str,
    url_patterns: tuple[str, ...],
) -> str | None:
    lower_text = text.lower()
    command_pattern = None
    for pattern in BENCHMARK_SOURCE_LEAK_COMMAND_PATTERNS:
        words = r"\s+".join(re.escape(part) for part in pattern.split())
        if re.search(rf"(?<![a-z0-9_-]){words}(?![a-z0-9_-])", lower_text):
            command_pattern = pattern
            break
    if command_pattern is None:
        return None
    if benchmark_source_leak_local_path_pattern(text) is not None:
        return command_pattern
    if benchmark_source_leak_url_pattern(text, url_patterns) is not None:
        return command_pattern
    return None


def target_upstream_proxy_required(agent_docker_image: str | None) -> bool:
    return agent_docker_image is not None


def benchmark_source_block_patterns_for_env(instance_id: str) -> str:
    return json.dumps([*benchmark_source_leak_url_patterns(instance_id), "upstream", "upstream/"])


def benchmark_source_leak_url_pattern(text: str, patterns: tuple[str, ...]) -> str | None:
    candidate = text.strip()
    if candidate.lower().startswith("url:"):
        candidate = candidate[4:].strip()
    lower_candidate = candidate.lower()
    if "://" not in lower_candidate and not lower_candidate.startswith("git@github.com:"):
        return None
    for pattern in patterns:
        if pattern in lower_candidate:
            return pattern
    return None


@dataclass
class TargetUpstreamDenyProxy:
    server: socketserver.ThreadingTCPServer
    thread: threading.Thread
    url: str
    container_url: str

    def close(self) -> None:
        self.server.shutdown()
        self.server.server_close()
        self.thread.join(timeout=2)


class _TargetUpstreamProxyServer(socketserver.ThreadingMixIn, http.server.HTTPServer):
    daemon_threads = True
    allow_reuse_address = True

    def __init__(
        self,
        server_address: tuple[str, int],
        handler_class: type[http.server.BaseHTTPRequestHandler],
        url_patterns: tuple[str, ...],
    ) -> None:
        super().__init__(server_address, handler_class)
        self.url_patterns = url_patterns


class _TargetUpstreamProxyHandler(http.server.BaseHTTPRequestHandler):
    timeout = 60

    def log_message(self, format: str, *args: Any) -> None:
        return

    def _deny(self) -> None:
        body = b"target upstream URL blocked by DeNovo benchmark proxy\n"
        self.send_response(403)
        self.send_header("Content-Type", "text/plain")
        self.send_header("Content-Length", str(len(body)))
        self.send_header("Connection", "close")
        self.end_headers()
        self.wfile.write(body)

    def do_CONNECT(self) -> None:
        host, _, raw_port = self.path.rpartition(":")
        host = host.lower()
        if host in BENCHMARK_SOURCE_LEAK_CONNECT_HOSTS:
            self._deny()
            return
        upstream = socket.create_connection((host, int(raw_port or "443")), timeout=self.timeout)
        try:
            self.send_response(200, "Connection Established")
            self.end_headers()
            self._relay(upstream)
        finally:
            upstream.close()

    def do_GET(self) -> None:
        self._handle_http_request()

    def do_HEAD(self) -> None:
        self._handle_http_request()

    def _handle_http_request(self) -> None:
        if benchmark_source_leak_url_pattern(self.path, self.server.url_patterns) is not None:
            self._deny()
            return
        parsed = urllib.parse.urlparse(self.path)
        if not parsed.scheme or not parsed.hostname:
            self.send_error(502, "proxy requires absolute-form URL")
            return
        port = parsed.port or (443 if parsed.scheme == "https" else 80)
        if parsed.scheme == "https":
            self.send_error(502, "https proxying requires CONNECT")
            return
        target = urllib.parse.urlunparse(("", "", parsed.path or "/", parsed.params, parsed.query, ""))
        body = None
        content_length = self.headers.get("Content-Length")
        if content_length is not None:
            body = self.rfile.read(int(content_length))
        with socket.create_connection((parsed.hostname, port), timeout=self.timeout) as upstream:
            request = f"{self.command} {target} HTTP/1.1\r\n"
            upstream.sendall(request.encode("ascii"))
            for key, value in self.headers.items():
                if key.lower() in {"proxy-connection", "connection"}:
                    continue
                upstream.sendall(f"{key}: {value}\r\n".encode("latin-1"))
            upstream.sendall(b"Connection: close\r\n\r\n")
            if body is not None:
                upstream.sendall(body)
            while True:
                chunk = upstream.recv(65536)
                if not chunk:
                    break
                self.connection.sendall(chunk)

    def _relay(self, upstream: socket.socket) -> None:
        sockets = [self.connection, upstream]
        while True:
            readable, _, _ = select.select(sockets, [], [], self.timeout)
            if not readable:
                return
            for source in readable:
                data = source.recv(65536)
                if not data:
                    return
                target = upstream if source is self.connection else self.connection
                target.sendall(data)


def start_target_upstream_deny_proxy(instance_id: str) -> TargetUpstreamDenyProxy | None:
    url_patterns = benchmark_source_leak_url_patterns(instance_id)
    if not url_patterns:
        return None
    server = _TargetUpstreamProxyServer(("0.0.0.0", 0), _TargetUpstreamProxyHandler, url_patterns)
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    port = server.server_address[1]
    return TargetUpstreamDenyProxy(
        server=server,
        thread=thread,
        url=f"http://127.0.0.1:{port}",
        container_url=f"http://host.docker.internal:{port}",
    )


def install_target_upstream_proxy_env(env: dict[str, str], proxy: TargetUpstreamDenyProxy) -> None:
    env["HTTP_PROXY"] = proxy.container_url
    env["HTTPS_PROXY"] = proxy.container_url
    no_proxy = [part for part in env.get("NO_PROXY", "").split(",") if part]
    for host in ("127.0.0.1", "localhost", "host.docker.internal"):
        if host not in no_proxy:
            no_proxy.append(host)
    env["NO_PROXY"] = ",".join(no_proxy)


def parse_tool_arguments(arguments: Any) -> Any:
    if not isinstance(arguments, str):
        return arguments
    try:
        return json.loads(arguments)
    except json.JSONDecodeError:
        return arguments


def benchmark_json_tool_calls(event: dict[str, Any]) -> list[tuple[str, Any]]:
    calls = list(omp_session_tool_calls(event))
    candidates = [event, event.get("payload"), event.get("item")]
    for candidate in candidates:
        if not isinstance(candidate, dict):
            continue
        call_type = candidate.get("type")
        if call_type in {"function_call", "custom_tool_call"}:
            name = candidate.get("name")
            if isinstance(name, str):
                calls.append((name, candidate.get("arguments")))
        function_call = candidate.get("function_call")
        if isinstance(function_call, dict):
            name = function_call.get("name")
            if isinstance(name, str):
                calls.append((name, function_call.get("arguments")))
    return calls


def benchmark_tool_call_source_leak_pattern(
    name: str,
    arguments: Any,
    url_patterns: tuple[str, ...],
) -> str | None:
    parsed_arguments = parse_tool_arguments(arguments)
    if isinstance(parsed_arguments, dict):
        for key in ("path", "url"):
            value = parsed_arguments.get(key)
            if isinstance(value, str) and name in {"read", "browser"}:
                pattern = benchmark_source_leak_local_path_pattern(value)
                if pattern is not None:
                    return pattern
                pattern = benchmark_source_leak_url_pattern(value, url_patterns)
                if pattern is not None:
                    return pattern
        command = parsed_arguments.get("command")
        if isinstance(command, str):
            return benchmark_source_leak_command_pattern(command, url_patterns)
    elif isinstance(parsed_arguments, str):
        if name in {"read", "browser"}:
            pattern = benchmark_source_leak_local_path_pattern(parsed_arguments)
            if pattern is not None:
                return pattern
            pattern = benchmark_source_leak_url_pattern(parsed_arguments, url_patterns)
            if pattern is not None:
                return pattern
        return benchmark_source_leak_command_pattern(parsed_arguments, url_patterns)
    return None


def benchmark_artifact_source_leak_pattern(
    artifact_path: Path,
    line: str,
    url_patterns: tuple[str, ...],
) -> str | None:
    if artifact_path.name.endswith(".read.log"):
        if line.lstrip().lower().startswith("url:"):
            return benchmark_source_leak_url_pattern(line, url_patterns)
        return None
    if artifact_path.suffix != ".jsonl":
        return None
    try:
        event = json.loads(line)
    except json.JSONDecodeError:
        return None
    if not isinstance(event, dict):
        return None
    for name, arguments in benchmark_json_tool_calls(event):
        pattern = benchmark_tool_call_source_leak_pattern(name, arguments, url_patterns)
        if pattern is not None:
            return pattern
    return None


def benchmark_session_artifact_paths(codex_home: Path) -> list[Path]:
    if not codex_home.exists():
        return []
    candidates: list[Path] = []
    for pattern in ("**/*.jsonl", "**/*.log"):
        candidates.extend(path for path in codex_home.rglob(pattern) if path.is_file())
    return sorted(set(candidates))


def benchmark_contamination_record(
    instance_id: str,
    workspace: Path,
    codex_home: Path,
) -> dict[str, str] | None:
    upstream_dir = workspace / "upstream"
    if upstream_dir.exists():
        return {
            "kind": "upstream-worktree",
            "path": str(upstream_dir),
            "reason": "`upstream` directory exists in final workspace",
        }

    url_patterns = benchmark_source_leak_url_patterns(instance_id)
    for artifact_path in benchmark_session_artifact_paths(codex_home):
        try:
            with artifact_path.open("r", encoding="utf-8", errors="replace") as handle:
                for line_number, line in enumerate(handle, start=1):
                    pattern = benchmark_artifact_source_leak_pattern(
                        artifact_path,
                        line,
                        url_patterns,
                    )
                    if pattern is not None:
                        return {
                            "kind": "upstream-source-access",
                            "path": str(artifact_path),
                            "line": str(line_number),
                            "pattern": pattern,
                            "reason": "session transcript referenced upstream source control",
                        }
        except OSError:
            continue
    return None



def instance_result_row(result: InstanceResult) -> dict[str, Any]:
    row = {
        "instance_id": result.instance_id,
        "dataset_id": "denovo_swe",
        "success": result.success,
        "score": result.score,
        "finish_reason": result.finish_reason,
        "error": result.error,
    }
    if result.eval_result is not None:
        row["eval_result"] = result.eval_result
    if result.subagent_used is not None:
        row["subagent_used"] = result.subagent_used
    if result.subagent_usage is not None:
        row["subagent_usage"] = result.subagent_usage
    if result.token_usage is not None:
        row["token_usage"] = result.token_usage
    if result.orchestration_trace is not None:
        row["orchestration_trace"] = result.orchestration_trace
    return row


def append_result_jsonl(path: Path, result: InstanceResult) -> None:
    append_jsonl(path, instance_result_row(result))


def stateful_http_json(
    env: dict[str, str],
    path: str,
    payload: dict[str, Any] | None = None,
    timeout: float = 5.0,
    query: dict[str, Any] | None = None,
) -> dict[str, Any]:
    base_url = env.get("STATEFUL_SERVER_URL")
    token = env.get("STATEFUL_SERVER_TOKEN")
    if not base_url or not token:
        raise RuntimeError("STATEFUL_SERVER_URL and STATEFUL_SERVER_TOKEN are required")
    url = urllib.parse.urljoin(base_url.rstrip("/") + "/", path.lstrip("/"))
    if query is not None:
        url = f"{url}?{urllib.parse.urlencode(query, quote_via=urllib.parse.quote)}"
    data = None
    method = "GET"
    headers = {
        "Accept": "application/json",
        "Authorization": f"Bearer {token}",
    }
    if payload is not None:
        data = json.dumps(payload).encode("utf-8")
        method = "POST"
        headers["Content-Type"] = "application/json"
    request = urllib.request.Request(url, data=data, headers=headers, method=method)
    with urllib.request.urlopen(request, timeout=timeout) as response:
        return json.loads(response.read().decode("utf-8"))


def stateful_v2_trace_identity(
    stateful_agent_id: str | None,
    workspace_id: str | None,
    instance_id: str,
) -> dict[str, dict[str, Any]]:
    agent_id = stateful_agent_id or "denovo-trace"
    return {
        "agent": {
            "agent_id": agent_id,
            "actor_id": agent_id,
            "actor_type": "agent",
        },
        "workspace": {
            "root": "unknown",
            "workspace_id": workspace_id or "unknown",
            "repo_id": "unknown",
            "worktree_id": "unknown",
            "branch": "unknown",
        },
        "source": {
            "kind": "cli",
            "event": "orchestration_trace",
            "tool_name": "denovo_codex_agent",
            "source_ref": instance_id,
        },
    }


def stateful_v2_trace_query(
    identity: dict[str, dict[str, Any]],
    query: dict[str, Any] | None = None,
) -> dict[str, Any]:
    return {
        "protocol_version": "stateful.v2",
        "request_id": str(uuid.uuid4()),
        "observed_at": datetime.now(timezone.utc).isoformat().replace("+00:00", "Z"),
        **identity["agent"],
        **identity["workspace"],
        **identity["source"],
        **(query or {}),
    }


def stateful_v2_trace_request(
    identity: dict[str, dict[str, Any]],
    payload: dict[str, Any],
) -> dict[str, Any]:
    return {
        "protocol_version": "stateful.v2",
        "request_id": str(uuid.uuid4()),
        "observed_at": datetime.now(timezone.utc).isoformat().replace("+00:00", "Z"),
        **identity,
        "payload": payload,
    }


def event_payload(event: dict[str, Any]) -> dict[str, Any]:
    payload = event.get("payload")
    if not isinstance(payload, dict):
        return {}
    for field in ("event", "data", "data"):
        nested = payload.get(field)
        if not isinstance(nested, dict):
            break
        payload = nested
    return payload




def authorization_target_paths(event: dict[str, Any]) -> list[str]:
    targets = event_payload(event).get("targets")
    if not isinstance(targets, list):
        return []
    return [
        path
        for target in targets
        if isinstance(target, dict)
        for path in [target.get("path")]
        if isinstance(path, str) and path
    ]


def parse_event_time(event: dict[str, Any]) -> datetime | None:
    value = event.get("timestamp") or event.get("created_at")
    if not isinstance(value, str):
        return None
    try:
        return datetime.fromisoformat(value.replace("Z", "+00:00"))
    except ValueError:
        return None


def top_counts(counter: Counter[str], limit: int) -> dict[str, int]:
    return dict(sorted(counter.items(), key=lambda item: (-item[1], item[0]))[:limit])


def heartbeat_key(event: dict[str, Any]) -> tuple[Any, ...]:
    return (
        event.get("agent_id"),
        event.get("workspace_id"),
        event.get("repo_id"),
        event.get("worktree_id"),
    )


def heartbeat_summary(events: list[dict[str, Any]]) -> dict[str, int | None]:
    windows = 0
    count = 0
    max_gap_ms: int | None = None
    previous_key: tuple[Any, ...] | None = None
    previous_time: datetime | None = None
    in_window = False
    for event in sorted(
        events,
        key=lambda event: str(event.get("created_at") or event.get("timestamp") or ""),
    ):
        if event.get("event_type") != "presence.heartbeat":
            in_window = False
            continue
        count += 1
        current_key = heartbeat_key(event)
        current_time = parse_event_time(event)
        if not in_window or current_key != previous_key:
            windows += 1
        if current_key == previous_key and current_time is not None and previous_time is not None:
            gap_ms = int((current_time - previous_time).total_seconds() * 1000)
            max_gap_ms = gap_ms if max_gap_ms is None else max(max_gap_ms, gap_ms)
        in_window = True
        previous_key = current_key
        previous_time = current_time
    return {
        "heartbeat_events": count,
        "heartbeat_windows": windows,
        "heartbeat_max_gap_ms": max_gap_ms,
    }


def summarize_orchestration_events(
    events: list[dict[str, Any]],
    agent_id: str | None,
    workspace_id: str | None = None,
) -> dict[str, Any]:
    if workspace_id:
        matching = [
            event for event in events if event.get("workspace_id") == workspace_id
        ]
    else:
        matching = [
            event
            for event in events
            if not agent_id or event.get("agent_id") == agent_id
        ]
    event_types = Counter(str(event.get("event_type", "")) for event in matching)
    denial_paths: Counter[str] = Counter()
    denial_messages: Counter[str] = Counter()
    for event in matching:
        if event.get("event_type") != "authorization.denied":
            continue
        for path in authorization_target_paths(event):
            denial_paths[path] += 1
        payload = event_payload(event)
        message = payload.get("message") or payload.get("denial_reason")
        if message:
            denial_messages[str(message)] += 1
    heartbeat = heartbeat_summary(matching)
    return {
        "event_count": len(matching),
        "event_types": dict(sorted(event_types.items())),
        "reservation_events": sum(
            count
            for event_type, count in event_types.items()
            if event_type.startswith("reservation.")
        ),
        "claim_events": sum(
            count
            for event_type, count in event_types.items()
            if event_type.startswith("claim.")
        ),
        "conflict_events": sum(
            count
            for event_type, count in event_types.items()
            if event_type == "authorization.denied" or "conflict" in event_type
        ),
        "denial_events": event_types.get("authorization.denied", 0),
        "denial_paths": top_counts(denial_paths, 10),
        "denial_messages": top_counts(denial_messages, 5),
        **heartbeat,
    }


def write_orchestration_trace(
    instance_dir: Path,
    env: dict[str, str],
    instance_id: str,
    stateful_agent_id: str | None,
    subagent_usage: dict[str, Any],
    patch_path: Path | None = None,
) -> dict[str, Any]:
    trace_path = instance_dir / "orchestration-trace.json"
    relative_trace_path = trace_path.relative_to(instance_dir.parent).as_posix()
    trace: dict[str, Any] = {
        "instance_id": instance_id,
        "stateful_agent_id": stateful_agent_id,
        "trace_captured": False,
        "trace_path": relative_trace_path,
        "subagent_usage": subagent_usage,
    }
    if patch_path is not None:
        trace["patch_path"] = patch_path.relative_to(instance_dir.parent).as_posix()
    try:
        workspace_id = env.get("STATEFUL_WORKSPACE_ID")
        identity = stateful_v2_trace_identity(
            stateful_agent_id,
            workspace_id,
            instance_id,
        )
        current = stateful_http_json(
            env,
            "/v2/current",
            query=stateful_v2_trace_query(identity),
        )
        events_body = stateful_http_json(
            env,
            "/v2/events",
            query=stateful_v2_trace_query(
                identity,
                {"limit": ORCHESTRATION_TRACE_EVENT_LIMIT},
            ),
        )
        events = events_body.get("events", [])
        if not isinstance(events, list):
            events = []
        if len(events) >= ORCHESTRATION_TRACE_EVENT_LIMIT:
            raise RuntimeError(
                f"stateful event trace reached the {ORCHESTRATION_TRACE_EVENT_LIMIT}-event limit"
            )
        if workspace_id:
            trace["workspace_id"] = workspace_id
        trace.update(summarize_orchestration_events(events, stateful_agent_id, workspace_id))
        trace["trace_captured"] = True
        trace["current"] = current
        trace["events"] = events
        if workspace_id:
            trace["context"] = stateful_http_json(
                env,
                "/v2/context/render",
                stateful_v2_trace_request(identity, {"mode": "brief"}),
            )
    except Exception as error:  # noqa: BLE001 - trace capture must not fail the run.
        trace["trace_error"] = repr(error)
    write_json(trace_path, trace)
    result = {
        "trace_path": relative_trace_path,
        "trace_captured": trace["trace_captured"],
        "reservation_events": trace.get("reservation_events", 0),
        "claim_events": trace.get("claim_events", 0),
        "conflict_events": trace.get("conflict_events", 0),
        "event_count": trace.get("event_count", 0),
        "event_types": trace.get("event_types", {}),
        "heartbeat_events": trace.get("heartbeat_events", 0),
        "heartbeat_windows": trace.get("heartbeat_windows", 0),
        "heartbeat_max_gap_ms": trace.get("heartbeat_max_gap_ms"),
        "denial_events": trace.get("denial_events", 0),
        "denial_paths": trace.get("denial_paths", {}),
        "denial_messages": trace.get("denial_messages", {}),
    }
    if trace_error := trace.get("trace_error"):
        result["trace_error"] = trace_error
    return result


def missing_runtime_image_name(error: BaseException) -> str | None:
    text = repr(error)
    if "ImageNotFound" in text and "/images/" in text and "/json" in text:
        marker = "/images/"
        start = text.find(marker)
        end = text.find("/json", start)
        if start != -1 and end != -1:
            return urllib.parse.unquote(text[start + len(marker) : end])

    if "/images/create?" in text:
        start = text.find("/images/create?")
        end = text.find("'", start)
        query = text[start + len("/images/create?") : end if end != -1 else None]
        params = urllib.parse.parse_qs(query)
        from_image = params.get("fromImage", [""])[0]
        tag = params.get("tag", [""])[0]
        if from_image and tag:
            return f"{from_image}:{tag}"
    return None


def instance_setup_exception_result(
    instance_id: str,
    error: BaseException,
) -> InstanceResult:
    if isinstance(error, MissingRuntimeImageError):
        return InstanceResult(
            instance_id,
            False,
            None,
            "missing-runtime-image",
            str(error),
            None,
        )
    missing_image = missing_runtime_image_name(error)
    if missing_image is not None:
        return InstanceResult(
            instance_id,
            False,
            None,
            "missing-runtime-image",
            f"runtime image unavailable: {missing_image}; {repr(error)}",
            None,
        )
    return InstanceResult(instance_id, False, None, "adapter-error", repr(error), None)


def adapter_exit_code_after_results(results: list[InstanceResult]) -> int:
    return 0


def should_run_codex(args: argparse.Namespace) -> bool:
    return not args.validate_run


def stateful_runtime_env_error(env: dict[str, str]) -> str | None:
    if env.get("STATEFUL_SERVER_URL") and env.get("STATEFUL_SERVER_TOKEN"):
        return None
    return "stateful Codex benchmark requires STATEFUL_SERVER_URL and STATEFUL_SERVER_TOKEN"


def max_concurrent_limit(args: argparse.Namespace) -> int:
    if args.max_concurrent is None:
        return 1
    return max(1, args.max_concurrent)


def write_adapter_metadata(output: Path, metadata: dict[str, Any]) -> None:
    write_json(output / "adapter-metadata.json", metadata)


def run_fake_instances(args: argparse.Namespace) -> int:
    output = Path(args.output)
    rows = read_jsonl(Path(args.data_file))
    selected = set(args.instance_id)
    if selected:
        rows = [row for row in rows if row.get("instance_id") in selected]
    if args.mode == "single" and rows:
        rows = rows[:1]
    result_rows = [
        instance_result_row(
            InstanceResult(
                instance_id=str(row.get("instance_id", "unknown")),
                success=True,
                score=1.0,
                finish_reason="fake",
                error=None,
                eval_result={"details": {"pass_rate": 1.0}},
            )
        )
        for row in rows
    ]
    write_jsonl(output / "_" / "results.jsonl", result_rows)
    write_adapter_metadata(
        output,
        {
            **profile_metadata(args.agent_mode, args.subagent, args.subagent_min_count, args.cli_runtime),
            **subagent_usage_metadata([], args.subagent_min_count),
            "fake": True,
            "results": len(result_rows),
        },
    )
    return 0


async def run_one_instance_async(
    args: argparse.Namespace,
    config: Any,
    task: Any,
    inst: Any,
    output: Path,
) -> InstanceResult:
    from aweagent.core.eval.setup import PreAgentSetup
    from aweagent.core.task.runner import runtime_registry
    from aweagent.tasks.denovo_swe.evaluator import DeNovoSWEEvaluator

    min_free_bytes = int(args.min_free_disk_gb * BYTES_PER_GIB)
    disk_guard = low_disk_space_result(inst.id, output, min_free_bytes)
    if disk_guard is not None:
        return disk_guard

    instance_dir = output / "instances" / path_fragment(inst.id)
    workspace = instance_dir / "workspace"
    write_json(
        instance_dir / "instance.json",
        {
            "instance_id": inst.id,
            "repo": inst.repo,
            "image": inst.image,
            "workdir": inst.workdir,
            "base_commit": inst.base_commit,
        },
    )

    seeded_auth = None
    stateful_repo_cleanup = None
    codex_env = None
    image = None
    target_upstream_proxy = None

    try:
        source_env = dict(os.environ)
        image = task.get_image(inst)
        await preflight_runtime_image_available(config.runtime, image)
        await ensure_runtime_image_available(config.runtime, image)
        runtime_config = runtime_config_for_local_image(config.runtime, image, inst.workdir)
        runtime_cls = runtime_registry.get(config.runtime.backend)
        runtime = runtime_cls(runtime_config)

        async with runtime.session(image) as session:
            setup = PreAgentSetup(session, inst.workdir)
            await setup.prepare(inst)
            await task.prepare_session(inst, session)
            await export_session_workspace(session, inst.workdir, workspace)

        prompt = build_codex_prompt(
            instance_id=inst.id,
            document=inst.metadata.get("document", "") or "",
            benchmark_max_turns=args.benchmark_max_turns,
            max_steps=args.max_steps,
            prompt_version=args.prompt_version,
            subagent=args.subagent,
            subagent_min_count=args.subagent_min_count,
            stateful_binary=args.stateful_binary if args.agent_mode == "stateful" else None,
        )
        write_json(instance_dir / "prompt.json", {"prompt": prompt})
        prompt_path = instance_dir / "prompt.txt"
        prompt_path.write_text(prompt, encoding="utf-8")

        if not should_run_codex(args):
            patch = ""
            (instance_dir / "patch.diff").write_text(patch, encoding="utf-8")
            if args.skip_eval:
                return InstanceResult(
                    inst.id,
                    True,
                    1.0,
                    "skip-eval",
                    None,
                    {"details": {"pass_rate": 1.0}},
                )
            eval_runtime = runtime_cls(
                runtime_config_for_local_image(config.runtime, image, inst.workdir),
            )
            evaluator = build_denovo_evaluator(DeNovoSWEEvaluator, args, config)
            eval_result = await evaluator.evaluate(inst, patch, eval_runtime)
            eval_data = {
                "accepted": eval_result.accepted,
                "score": eval_result.score,
                "duration": eval_result.duration,
                "details": eval_result.details,
            }
            write_json(instance_dir / "eval-result.json", eval_data)
            return InstanceResult(
                inst.id,
                eval_result.accepted,
                eval_result.score,
                "validate-run",
                None,
                eval_data,
            )

        stateful_agent_id = (
            denovo_stateful_agent_id(
                output=output,
                instance_id=inst.id,
                task_path=Path(args.data_file),
                workspace=workspace,
            )
            if args.agent_mode == "stateful"
            else None
        )
        if args.cli_runtime == "omp":
            env = denovo_omp_environment(
                output=output,
                instance_id=inst.id,
                task_path=Path(args.data_file),
                workspace=workspace,
                base_env=source_env,
            )
            codex_home = Path(env["PI_CODING_AGENT_DIR"])
        else:
            command = codex_command_for_profile(
                workspace=workspace,
                agent_mode=args.agent_mode,
                subagent=args.subagent,
                codex_bin=args.codex_bin,
                stateful_binary=args.stateful_binary,
                benchmark_model=args.benchmark_model,
                benchmark_reasoning_effort=args.benchmark_reasoning_effort,
                benchmark_model_context_window=args.benchmark_model_context_window,
                benchmark_temperature=args.benchmark_temperature,
                base_env=source_env,
            )
            env = denovo_codex_environment(
                output=output,
                instance_id=inst.id,
                task_path=Path(args.data_file),
                workspace=workspace,
                base_env=source_env,
            )
            codex_env = env
            codex_home = Path(env["CODEX_HOME"])
        if args.agent_mode == "stateful":
            runtime_env_error = stateful_runtime_env_error(env)
            if runtime_env_error is not None:
                return InstanceResult(
                    inst.id,
                    False,
                    None,
                    "setup-error",
                    runtime_env_error,
                    None,
                )
        if args.cli_runtime == "omp":
            prepare_omp_environment(
                env,
                enable_stateful=args.agent_mode == "stateful",
                stateful_binary=args.stateful_binary,
                runtime_stateful_binary=(
                    args.agent_docker_stateful_binary if args.agent_docker_image else None
                ),
                runtime_omp_home=OMP_AGENT_DOCKER_HOME if args.agent_docker_image else None,
                omp_bin=args.omp_bin,
                enable_native_subagent=args.subagent == "on",
                agent_docker_image=args.agent_docker_image,
            )
            seed_omp_auth_credentials(env)
            env["STATEFUL_BENCHMARK_SOURCE_BLOCK_PATTERNS"] = (
                benchmark_source_block_patterns_for_env(inst.id)
            )
            if target_upstream_proxy_required(args.agent_docker_image):
                target_upstream_proxy = start_target_upstream_deny_proxy(inst.id)
                if target_upstream_proxy is not None:
                    install_target_upstream_proxy_env(env, target_upstream_proxy)
            if args.agent_docker_image:
                command = docker_omp_command_for_profile(
                    workspace=workspace,
                    prompt_path=prompt_path,
                    home=Path(env["HOME"]),
                    omp_bin=args.omp_bin,
                    benchmark_model=args.benchmark_model,
                    benchmark_reasoning_effort=args.benchmark_reasoning_effort,
                    docker_image=args.agent_docker_image,
                    base_env=env,
                    enable_native_subagent=args.subagent == "on",
                    subagent_min_count=args.subagent_min_count,
                    sandbox=args.agent_docker_sandbox,
                )
            else:
                command = omp_command_for_profile(
                    workspace=workspace,
                    prompt_path=prompt_path,
                    omp_bin=args.omp_bin,
                    benchmark_model=args.benchmark_model,
                    benchmark_reasoning_effort=args.benchmark_reasoning_effort,
                    enable_native_subagent=args.subagent == "on",
                    subagent_min_count=args.subagent_min_count,
                )
        else:
            seeded_auth = prepare_codex_environment(
                env,
                source_env=source_env,
                enable_stateful=args.agent_mode == "stateful",
                stateful_binary=args.stateful_binary,
                stateful_integration=(
                    STATEFUL_INTEGRATION_FULL
                    if args.agent_mode == "stateful"
                    else STATEFUL_INTEGRATION_NONE
                ),
            )
            if seeded_auth is None:
                return InstanceResult(
                    inst.id,
                    False,
                    None,
                    "setup-error",
                    "Codex auth could not be seeded into the isolated CODEX_HOME",
                    None,
                )

        if args.agent_mode == "stateful":
            stateful_repo_cleanup = enable_stateful_repo(
                env=env,
                workspace=workspace,
                stateful_binary=args.stateful_binary,
                runtime_workspace=OMP_AGENT_DOCKER_WORKSPACE if args.agent_docker_image else None,
            )

        started_at = time.monotonic()
        if args.cli_runtime == "omp":
            execution_summary = run_omp_with_timeout(
                command,
                workspace,
                env,
                timeout_seconds=args.codex_timeout_seconds,
            )
        else:
            execution_summary = run_codex_with_timeout(
                command,
                prompt,
                workspace,
                env,
                max_resumes=args.max_resumes,
                timeout_seconds=args.codex_timeout_seconds,
            )
        returncode = execution_summary.returncode
        token_usage = (
            execution_summary.token_usage
            if execution_summary.token_usage.get("turns", 0) > 0
            else None
        )
        duration = time.monotonic() - started_at
        subagent_usage = native_subagent_usage(
            args.subagent,
            args.subagent_min_count,
            codex_home,
            cli_runtime=args.cli_runtime,
        )
        command_record = {
            "command": command,
            "returncode": returncode,
            "duration": duration,
            "native_subagent": subagent_usage["native_subagent"],
            "subagent_usage": subagent_usage,
        }
        if token_usage is not None:
            command_record["token_usage"] = token_usage

        patch_path = instance_dir / "patch.diff"

        def capture_trace() -> dict[str, Any] | None:
            if args.agent_mode != "stateful":
                return None
            trace = write_orchestration_trace(
                instance_dir=instance_dir,
                env=env,
                instance_id=inst.id,
                stateful_agent_id=stateful_agent_id,
                subagent_usage=subagent_usage,
                patch_path=patch_path if patch_path.exists() else None,
            )
            command_record["orchestration_trace"] = trace
            return trace

        def finish_command_record(orchestration_trace: dict[str, Any] | None) -> None:
            if orchestration_trace is not None:
                command_record["orchestration_trace"] = orchestration_trace
            write_json(instance_dir / "codex-command.json", command_record)

        benchmark_contamination = benchmark_contamination_record(inst.id, workspace, codex_home)
        if benchmark_contamination is not None:
            patch = git_diff(workspace) if returncode == 0 else ""
            patch_path.write_text(patch, encoding="utf-8")
            command_record["benchmark_contamination"] = benchmark_contamination
            orchestration_trace = capture_trace()
            finish_command_record(orchestration_trace)
            cleanup_stateful_repo_enable(workspace, stateful_repo_cleanup)
            stateful_repo_cleanup = None
            return InstanceResult(
                inst.id,
                False,
                None,
                "benchmark-contamination",
                benchmark_contamination["reason"],
                None,
                subagent_used=subagent_usage["subagent_used"],
                subagent_usage=subagent_usage,
                token_usage=token_usage,
                orchestration_trace=orchestration_trace,
            )

        if returncode != 0:
            patch_path.write_text("", encoding="utf-8")
            orchestration_trace = capture_trace()
            finish_command_record(orchestration_trace)
            cleanup_stateful_repo_enable(workspace, stateful_repo_cleanup)
            stateful_repo_cleanup = None
            finish_reason, error = cli_runtime_failure(returncode, args.cli_runtime)
            return InstanceResult(
                inst.id,
                False,
                None,
                finish_reason,
                error,
                None,
                subagent_used=subagent_usage["subagent_used"],
                subagent_usage=subagent_usage,
                token_usage=token_usage,
                orchestration_trace=orchestration_trace,
            )



        patch = git_diff(workspace)
        patch_path.write_text(patch, encoding="utf-8")
        orchestration_trace = capture_trace()
        finish_command_record(orchestration_trace)
        cleanup_stateful_repo_enable(workspace, stateful_repo_cleanup)
        stateful_repo_cleanup = None

        if args.skip_eval:
            return InstanceResult(
                inst.id,
                True,
                1.0,
                "skip-eval",
                None,
                {"details": {"pass_rate": 1.0}},
                subagent_used=subagent_usage["subagent_used"],
                subagent_usage=subagent_usage,
                token_usage=token_usage,
                orchestration_trace=orchestration_trace,
            )

        eval_runtime = runtime_cls(
            runtime_config_for_local_image(config.runtime, image, inst.workdir),
        )
        evaluator = build_denovo_evaluator(DeNovoSWEEvaluator, args, config)
        eval_result = await evaluator.evaluate(inst, patch, eval_runtime)
        eval_data = {
            "accepted": eval_result.accepted,
            "score": eval_result.score,
            "duration": eval_result.duration,
            "details": eval_result.details,
        }
        write_json(instance_dir / "eval-result.json", eval_data)
        return InstanceResult(
            inst.id,
            eval_result.accepted,
            eval_result.score,
            "stop",
            None,
            eval_data,
            subagent_used=subagent_usage["subagent_used"],
            subagent_usage=subagent_usage,
            token_usage=token_usage,
            orchestration_trace=orchestration_trace,
        )
    except CodexTimeoutError:
        return InstanceResult(
            inst.id,
            False,
            None,
            f"{args.cli_runtime}-timeout",
            f"{args.cli_runtime} timed out after {args.codex_timeout_seconds}s",
            None,
        )
    except StatefulRepoEnableError as error:
        return InstanceResult(inst.id, False, None, "setup-error", str(error), None)
    except UnsafeNestedCodexHome as error:
        return InstanceResult(inst.id, False, None, "setup-error", str(error), None)
    except Exception as error:
        return instance_setup_exception_result(inst.id, error)
    finally:
        if target_upstream_proxy is not None:
            target_upstream_proxy.close()
        cleanup_stateful_repo_enable(workspace, stateful_repo_cleanup)
        cleanup_seeded_auth(seeded_auth)
        if codex_env is not None:
            cleanup_codex_home_caches(codex_env)
        await delete_runtime_image_after_instance(
            config.runtime,
            image,
            enabled=args.del_done_images,
        )


async def run_real_instances_async(args: argparse.Namespace) -> int:
    max_concurrent = max_concurrent_limit(args)
    output = Path(args.output)

    add_aweagent_to_path(Path(args.aweagent_root))
    from recipes.denovo_swe.run import _build_task, _load_config

    config_args = argparse.Namespace(
        config=args.config,
        llm_config=None,
        model=None,
        max_steps=args.max_steps,
        max_concurrent=args.max_concurrent,
        enable_search=None,
        output=str(output),
        data_file=args.data_file,
    )
    config = _load_config(config_args)
    task = _build_task(
        config,
        data_file=args.data_file,
        validate_run=args.validate_run,
        del_done_images=args.del_done_images,
        clean_snapshot_file=args.dump_clean_snapshot,
        prompt_version=args.prompt_version,
        eval_iters=args.eval_iters,
    )
    instances = task.get_instances(instance_ids=args.instance_id or None)
    if args.mode == "single":
        instances = instances[:1]

    results_path = output / "_" / "results.jsonl"
    write_jsonl(results_path, [])
    semaphore = asyncio.Semaphore(max_concurrent)
    results_by_index: list[InstanceResult | None] = [None] * len(instances)

    async def run_limited(index: int, inst: Any) -> tuple[int, InstanceResult]:
        async with semaphore:
            result = await asyncio.to_thread(
                lambda: asyncio.run(run_one_instance_async(args, config, task, inst, output))
            )
            return index, result

    tasks = [
        asyncio.create_task(run_limited(index, inst))
        for index, inst in enumerate(instances)
    ]
    for task_result in asyncio.as_completed(tasks):
        index, result = await task_result
        results_by_index[index] = result
        append_result_jsonl(results_path, result)

    results = [result for result in results_by_index if result is not None]

    write_jsonl(results_path, [instance_result_row(result) for result in results])
    write_adapter_metadata(
        output,
        {
            **profile_metadata(args.agent_mode, args.subagent, args.subagent_min_count, args.cli_runtime),
            **subagent_usage_metadata(results, args.subagent_min_count),
            "fake": False,
            "results": len(results),
            "benchmark_model": args.benchmark_model,
            "benchmark_reasoning_effort": args.benchmark_reasoning_effort,
            "benchmark_model_context_window": args.benchmark_model_context_window,
            "benchmark_temperature": args.benchmark_temperature,
            "benchmark_max_turns": args.benchmark_max_turns,
            "max_resumes": args.max_resumes,
            "eval_iters": args.eval_iters,
            "max_concurrent": max_concurrent,
            "agent_docker_image": args.agent_docker_image,
            "agent_docker_stateful_binary": (
                args.agent_docker_stateful_binary if args.agent_docker_image else None
            ),
            "agent_docker_sandbox": args.agent_docker_sandbox if args.agent_docker_image else None,
        },
    )
    return adapter_exit_code_after_results(results)


def run_real_instances(args: argparse.Namespace) -> int:
    return asyncio.run(run_real_instances_async(args))


def main(argv: list[str] | None = None) -> int:
    args = parse_args(argv)
    if os.environ.get("STATEFUL_BENCH_DENOVO_CODEX_FAKE") == "1":
        return run_fake_instances(args)
    return run_real_instances(args)


if __name__ == "__main__":
    sys.exit(main())
