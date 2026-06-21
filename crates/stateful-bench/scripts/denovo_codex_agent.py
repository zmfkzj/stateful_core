#!/usr/bin/env python3
"""Run DeNovoSWE instances with host Codex CLI."""

from __future__ import annotations

import argparse
import asyncio
import json
import os
import shutil
import sqlite3
import subprocess
import sys
import tarfile
import tempfile
import time
import urllib.parse
import urllib.request
from dataclasses import dataclass
from pathlib import Path
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
DEFAULT_MIN_FREE_DISK_GB = 20.0
BYTES_PER_GIB = 1024**3
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
    return f"""

Native Codex subagent requirements:
- MUST use native Codex subagents for this benchmark condition.
- Spawn at least {subagent_min_count} native subagents before finishing.
- Use all {subagent_min_count} native subagents for repository editing.
- Do not leave any native subagent as analysis-only; each one must inspect, edit, and verify the workspace.
- Wait for each spawned subagent and incorporate its work or findings into the final workspace.
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
    _ = stateful_binary
    step_line = f"- Maximum task steps: {max_steps}.\n" if max_steps is not None else ""
    subagent_instruction = native_subagent_prompt_instruction(subagent, subagent_min_count)
    return f"""
You are solving one DeNovoSWE benchmark instance.

Instance id:
{instance_id}

Repository specification:
{document}

Constraints:
- Solve only this DeNovoSWE instance.
- Edit only files in the provided workspace.
- Do not edit benchmark artifacts, result files, Codex logs, auth files, or generated metadata.
- Leave the workspace containing the final code changes.
- Benchmark max turns: {benchmark_max_turns}.
{step_line}- Prompt version: {prompt_version}.
{subagent_instruction}
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
    for event in iter_json_events(output):
        if not isinstance(event, dict) or event.get("type") != "turn.completed":
            continue
        usage = event.get("usage")
        if not isinstance(usage, dict):
            continue
        input_tokens = int(usage.get("input_tokens", 0) or 0)
        cached_input_tokens = int(usage.get("cached_input_tokens", 0) or 0)
        output_tokens = int(usage.get("output_tokens", 0) or 0)
        reasoning_output_tokens = int(usage.get("reasoning_output_tokens", 0) or 0)
        uncached_input_tokens = max(0, input_tokens - cached_input_tokens)
        total["turns"] += 1
        total["input_tokens"] += input_tokens
        total["cached_input_tokens"] += cached_input_tokens
        total["output_tokens"] += output_tokens
        total["reasoning_output_tokens"] += reasoning_output_tokens
        total["input_plus_output_tokens"] += input_tokens + output_tokens
        total["uncached_input_tokens"] += uncached_input_tokens
        total["uncached_input_plus_output_tokens"] += uncached_input_tokens + output_tokens
    return total


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
    shutil.copytree(source, workspace, symlinks=True)


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


def denovo_codex_environment(
    output: Path,
    instance_id: str,
    task_path: Path,
    workspace: Path,
    base_env: dict[str, str] | None = None,
    preserve_stateful_session: bool = False,
    stateful_session_id: str | None = None,
) -> dict[str, str]:
    source_env = os.environ if base_env is None else base_env
    env = dict(source_env)
    _ = preserve_stateful_session
    env.pop("CODEX_THREAD_ID", None)
    env.pop("STATEFUL_CODEX_RUN_ID", None)
    env.pop("STATEFUL_SESSION_ID", None)
    if stateful_session_id:
        env["STATEFUL_CODEX_RUN_ID"] = stateful_session_id
        env["STATEFUL_SESSION_ID"] = stateful_session_id
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


def stateful_session_fragment(value: str) -> str:
    fragment = "".join(
        character if character.isalnum() or character in "_-" else "-"
        for character in str(value)
    ).strip("-_")
    return fragment or "item"


def denovo_stateful_session_id(
    output: Path,
    instance_id: str,
    task_path: Path,
    workspace: Path,
) -> str:
    return (
        f"denovo-{stateful_session_fragment(instance_id)}-"
        f"{path_scope_digest(output, task_path, workspace)}"
    )


def native_subagent_usage(
    subagent: str,
    subagent_min_count: int,
    codex_home: Path,
) -> dict[str, Any]:
    native_usage = annotate_native_subagent_usage(
        detect_native_subagent_usage(codex_home),
        subagent_min_count,
    )
    spawn_count = native_usage["subagent_spawn_count"]
    requirement_met = subagent != "on" or spawn_count >= subagent_min_count
    return {
        "mode": "native_codex_subagents" if subagent == "on" else "off",
        "subagent_min_count": subagent_min_count,
        "subagent_used": bool(native_usage["subagent_used"]),
        "subagent_requirement_met": requirement_met,
        "native_subagent": native_usage,
    }


def enable_stateful_repo(
    env: dict[str, str],
    workspace: Path,
    stateful_binary: str,
    runner: Any = subprocess.run,
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
    return cleanup


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
) -> dict[str, Any]:
    return {
        "agent_kind": "codex-cli",
        "agent_mode": agent_mode,
        "subagent": subagent,
        "subagent_required": subagent == "on",
        "subagent_min_count": subagent_min_count,
        "subagent_mode": "native_codex_subagents" if subagent == "on" else "off",
        "official_benchmark_protocol": OFFICIAL_BENCHMARK_PROTOCOL,
        "agent_rollouts_per_instance": 1,
        "native_subagent_required": subagent == "on",
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
    parser.add_argument("--codex-bin", required=True)
    parser.add_argument("--stateful-binary", required=True)
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
                count_key = SUBAGENT_TOOL_NAMES.get(name or "")
                if count_key is not None:
                    counts[count_key] += 1
                    sources.add("codex_session_log")

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
) -> dict[str, Any]:
    base_url = env.get("STATEFUL_SERVER_URL")
    token = env.get("STATEFUL_SERVER_TOKEN")
    if not base_url or not token:
        raise RuntimeError("STATEFUL_SERVER_URL and STATEFUL_SERVER_TOKEN are required")
    url = urllib.parse.urljoin(base_url.rstrip("/") + "/", path.lstrip("/"))
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


def summarize_orchestration_events(
    events: list[dict[str, Any]],
    session_id: str | None,
) -> dict[str, Any]:
    matching = [
        event
        for event in events
        if not session_id or event.get("session_id") == session_id
    ]
    event_types = [str(event.get("event_type", "")) for event in matching]
    return {
        "event_count": len(matching),
        "intent_events": sum(1 for event_type in event_types if event_type.startswith("Intent")),
        "lease_events": sum(1 for event_type in event_types if event_type.startswith("Lease")),
        "conflict_events": sum(
            1
            for event_type in event_types
            if event_type == "AuthorizationDenied" or "Conflict" in event_type
        ),
    }


def write_orchestration_trace(
    instance_dir: Path,
    env: dict[str, str],
    instance_id: str,
    session_id: str | None,
    subagent_usage: dict[str, Any],
    patch_path: Path | None = None,
) -> dict[str, Any]:
    trace_path = instance_dir / "orchestration-trace.json"
    relative_trace_path = trace_path.relative_to(instance_dir.parent).as_posix()
    trace: dict[str, Any] = {
        "instance_id": instance_id,
        "session_id": session_id,
        "trace_captured": False,
        "trace_path": relative_trace_path,
        "subagent_usage": subagent_usage,
    }
    if patch_path is not None:
        trace["patch_path"] = patch_path.relative_to(instance_dir.parent).as_posix()
    try:
        current = stateful_http_json(env, "/v1/current")
        events_body = stateful_http_json(env, "/v1/events")
        events = events_body.get("events", [])
        if not isinstance(events, list):
            events = []
        trace.update(summarize_orchestration_events(events, session_id))
        trace["trace_captured"] = True
        trace["current"] = current.get("current", current)
        trace["events"] = events
        workspace_id = env.get("STATEFUL_WORKSPACE_ID")
        if workspace_id:
            trace["context"] = stateful_http_json(
                env,
                "/v1/context/render",
                {
                    "session_id": session_id,
                    "workspace_id": workspace_id,
                    "mode": "brief",
                },
            )
    except Exception as error:  # noqa: BLE001 - trace capture must not fail the run.
        trace["trace_error"] = repr(error)
    write_json(trace_path, trace)
    return {
        "trace_path": relative_trace_path,
        "trace_captured": trace["trace_captured"],
        "intent_events": trace.get("intent_events", 0),
        "lease_events": trace.get("lease_events", 0),
        "conflict_events": trace.get("conflict_events", 0),
    }


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


def max_concurrent_error_result(args: argparse.Namespace) -> InstanceResult | None:
    if args.max_concurrent is None or args.max_concurrent <= 1:
        return None
    return InstanceResult(
        "adapter-setup",
        False,
        None,
        "setup-error",
        "Codex DeNovo adapter currently supports --max-concurrent 1 only",
        None,
    )


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
            **profile_metadata(args.agent_mode, args.subagent, args.subagent_min_count),
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
        )
        write_json(instance_dir / "prompt.json", {"prompt": prompt})

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
            stateful_session_id=(
                denovo_stateful_session_id(
                    output=output,
                    instance_id=inst.id,
                    task_path=Path(args.data_file),
                    workspace=workspace,
                )
                if args.agent_mode == "stateful"
                else None
            ),
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
            )

        started_at = time.monotonic()
        codex_summary = run_codex_with_timeout(
            command,
            prompt,
            workspace,
            env,
            max_resumes=args.max_resumes,
            timeout_seconds=args.codex_timeout_seconds,
        )
        returncode = codex_summary.returncode
        token_usage = (
            codex_summary.token_usage
            if codex_summary.token_usage.get("turns", 0) > 0
            else None
        )
        duration = time.monotonic() - started_at
        subagent_usage = native_subagent_usage(
            args.subagent,
            args.subagent_min_count,
            codex_home,
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
                session_id=env.get("STATEFUL_SESSION_ID"),
                subagent_usage=subagent_usage,
                patch_path=patch_path if patch_path.exists() else None,
            )
            command_record["orchestration_trace"] = trace
            return trace

        def finish_command_record(orchestration_trace: dict[str, Any] | None) -> None:
            if orchestration_trace is not None:
                command_record["orchestration_trace"] = orchestration_trace
            write_json(instance_dir / "codex-command.json", command_record)

        if returncode != 0:
            patch_path.write_text("", encoding="utf-8")
            orchestration_trace = capture_trace()
            finish_command_record(orchestration_trace)
            cleanup_stateful_repo_enable(workspace, stateful_repo_cleanup)
            stateful_repo_cleanup = None
            return InstanceResult(
                inst.id,
                False,
                None,
                "codex-error",
                f"codex exited {returncode}",
                None,
                subagent_used=subagent_usage["subagent_used"],
                subagent_usage=subagent_usage,
                token_usage=token_usage,
                orchestration_trace=orchestration_trace,
            )

        if args.subagent == "on" and not subagent_usage["subagent_requirement_met"]:
            patch_path.write_text("", encoding="utf-8")
            orchestration_trace = capture_trace()
            finish_command_record(orchestration_trace)
            cleanup_stateful_repo_enable(workspace, stateful_repo_cleanup)
            stateful_repo_cleanup = None
            spawn_count = subagent_usage["native_subagent"]["subagent_spawn_count"]
            return InstanceResult(
                inst.id,
                False,
                None,
                "subagent-requirement-failed",
                (
                    f"subagent:on requires at least {args.subagent_min_count} native Codex "
                    f"subagent spawns; observed {spawn_count}"
                ),
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
            "codex-timeout",
            f"codex timed out after {args.codex_timeout_seconds}s",
            None,
        )
    except StatefulRepoEnableError as error:
        return InstanceResult(inst.id, False, None, "setup-error", str(error), None)
    except UnsafeNestedCodexHome as error:
        return InstanceResult(inst.id, False, None, "setup-error", str(error), None)
    except Exception as error:
        return instance_setup_exception_result(inst.id, error)
    finally:
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
    output = Path(args.output)
    max_concurrent_error = max_concurrent_error_result(args)
    if max_concurrent_error is not None:
        results = [max_concurrent_error]
        write_jsonl(output / "_" / "results.jsonl", [instance_result_row(result) for result in results])
        write_adapter_metadata(
            output,
            {
                **profile_metadata(args.agent_mode, args.subagent, args.subagent_min_count),
                **subagent_usage_metadata(results, args.subagent_min_count),
                "fake": False,
                "results": len(results),
                "max_concurrent": args.max_concurrent,
                "error": max_concurrent_error.error,
            },
        )
        return adapter_exit_code_after_results(results)

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
    results = []
    for inst in instances:
        result = await run_one_instance_async(args, config, task, inst, output)
        results.append(result)
        append_result_jsonl(results_path, result)

    write_jsonl(results_path, [instance_result_row(result) for result in results])
    write_adapter_metadata(
        output,
        {
            **profile_metadata(args.agent_mode, args.subagent, args.subagent_min_count),
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
