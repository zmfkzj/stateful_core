#!/usr/bin/env python3
"""Run DeNovoSWE instances with host Codex CLI."""

from __future__ import annotations

import argparse
import asyncio
import json
import os
import shutil
import subprocess
import sys
import tarfile
import tempfile
import time
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
    codex_environment,
    path_fragment,
    prepare_codex_environment,
    run_codex_with_resume,
    toml_string,
)


OFFICIAL_BENCHMARK_PROTOCOL = "denovo_swe_single_rollout"
RESUME_POLICY_CONTEXT_OR_TOKEN_ONLY = "context_or_token_failure_only"


@dataclass
class InstanceResult:
    instance_id: str
    success: bool | None
    score: float | None
    finish_reason: str | None
    error: str | None
    eval_result: dict[str, Any] | None


class CodexTimeoutError(TimeoutError):
    pass


class StatefulRepoEnableError(RuntimeError):
    pass


@dataclass
class StatefulRepoEnableCleanup:
    created_stateful_dir: bool
    created_policy_config: bool


def build_codex_prompt(
    instance_id: str,
    document: str,
    benchmark_max_turns: int,
    max_steps: int | None,
    prompt_version: str,
) -> str:
    step_line = f"- Maximum task steps: {max_steps}.\n" if max_steps is not None else ""
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
""".strip()


def git_diff(workspace: Path) -> str:
    add_completed = subprocess.run(
        ["git", "add", "-A"],
        cwd=workspace,
        text=True,
        check=False,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
    )
    if add_completed.returncode != 0:
        raise RuntimeError(add_completed.stderr.strip() or "git add -A failed")

    diff_completed = subprocess.run(
        ["git", "diff", "--cached", "--binary"],
        cwd=workspace,
        text=True,
        check=False,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
    )
    if diff_completed.returncode != 0:
        raise RuntimeError(diff_completed.stderr.strip() or "git diff --cached --binary failed")
    return diff_completed.stdout


def run_codex_with_timeout(
    command: list[str],
    prompt: str,
    workspace: Path,
    env: dict[str, str] | None,
    max_resumes: int,
    timeout_seconds: float,
    runner: Any = subprocess.run,
) -> int:
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

    return run_codex_with_resume(
        command,
        prompt,
        workspace,
        env,
        max_resumes=max_resumes,
        runner=bounded_runner,
    )


def add_aweagent_to_path(aweagent_root: Path) -> None:
    root = str(aweagent_root.resolve())
    if root not in sys.path:
        sys.path.insert(0, root)


def _safe_extract_tar(tar: tarfile.TarFile, destination: Path) -> None:
    destination_root = destination.resolve()
    for member in tar.getmembers():
        if member.issym() or member.islnk():
            raise RuntimeError(f"unsafe archive link: {member.name} -> {member.linkname}")
        target = (destination / member.name).resolve()
        if target != destination_root and destination_root not in target.parents:
            raise RuntimeError(f"unsafe archive member: {member.name}")
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
            shutil.copytree(source, workspace)

    await asyncio.to_thread(_export)


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
) -> dict[str, str]:
    nested_env = codex_environment(
        task_path=task_path,
        workspace=workspace,
        base_env=base_env,
    )
    if nested_env is not None:
        return nested_env

    source_env = os.environ if base_env is None else base_env
    env = dict(source_env)
    home = output / "codex-homes" / path_fragment(instance_id) / "home"
    env["HOME"] = str(home)
    env["CODEX_HOME"] = str(home / ".codex")
    env["XDG_CONFIG_HOME"] = str(home / ".config")
    env["XDG_CACHE_HOME"] = str(home / ".cache")

    system_cert = Path("/etc/ssl/cert.pem")
    if not env.get("SSL_CERT_FILE") and system_cert.is_file():
        env["SSL_CERT_FILE"] = str(system_cert)

    return env


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


def profile_metadata(agent_mode: str, subagent: str) -> dict[str, Any]:
    return {
        "agent_kind": "codex-cli",
        "agent_mode": agent_mode,
        "subagent": subagent,
        "official_benchmark_protocol": OFFICIAL_BENCHMARK_PROTOCOL,
        "agent_rollouts_per_instance": 1,
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
    parser.add_argument("--max-resumes", type=int, required=True)
    parser.add_argument("--codex-timeout-seconds", type=int, required=True)
    parser.add_argument("--max-steps", type=int)
    parser.add_argument("--max-concurrent", type=int)
    parser.add_argument("--instance-id", action="append", default=[])
    parser.add_argument("--skip-eval", action="store_true")
    parser.add_argument("--validate-run", action="store_true")
    parser.add_argument("--del-done-images", action="store_true")
    parser.add_argument("--dump-clean-snapshot")
    parser.add_argument("--eval-iters", type=int, required=True)
    parser.add_argument("--prompt-version", required=True)
    parser.add_argument("--verbose", action="store_true")
    return parser.parse_args(argv)


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
    return row


def adapter_exit_code_after_results(results: list[InstanceResult]) -> int:
    return 0


def should_run_codex(args: argparse.Namespace) -> bool:
    return not args.validate_run


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
            **profile_metadata(args.agent_mode, args.subagent),
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
    try:
        source_env = dict(os.environ)
        image = task.get_image(inst)
        runtime_config = config.runtime.model_copy(
            update={"image": image, "workdir": inst.workdir},
        )
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
                config.runtime.model_copy(
                    update={"image": image, "workdir": inst.workdir},
                ),
            )
            evaluator = DeNovoSWEEvaluator(
                timeout=config.eval.timeout,
                validate_run=args.validate_run,
                del_done_images=args.del_done_images,
                eval_iters=args.eval_iters,
            )
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
        returncode = run_codex_with_timeout(
            command,
            prompt,
            workspace,
            env,
            max_resumes=args.max_resumes,
            timeout_seconds=args.codex_timeout_seconds,
        )
        duration = time.monotonic() - started_at
        write_json(
            instance_dir / "codex-command.json",
            {
                "command": command,
                "returncode": returncode,
                "duration": duration,
            },
        )

        cleanup_stateful_repo_enable(workspace, stateful_repo_cleanup)
        stateful_repo_cleanup = None
        patch = git_diff(workspace)
        (instance_dir / "patch.diff").write_text(patch, encoding="utf-8")

        if returncode != 0:
            return InstanceResult(
                inst.id,
                False,
                None,
                "codex-error",
                f"codex exited {returncode}",
                None,
            )

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
            config.runtime.model_copy(update={"image": image, "workdir": inst.workdir}),
        )
        evaluator = DeNovoSWEEvaluator(
            timeout=config.eval.timeout,
            validate_run=args.validate_run,
            del_done_images=args.del_done_images,
            eval_iters=args.eval_iters,
        )
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
        return InstanceResult(inst.id, False, None, "adapter-error", repr(error), None)
    finally:
        cleanup_stateful_repo_enable(workspace, stateful_repo_cleanup)
        cleanup_seeded_auth(seeded_auth)


async def run_real_instances_async(args: argparse.Namespace) -> int:
    output = Path(args.output)
    max_concurrent_error = max_concurrent_error_result(args)
    if max_concurrent_error is not None:
        results = [max_concurrent_error]
        write_jsonl(output / "_" / "results.jsonl", [instance_result_row(result) for result in results])
        write_adapter_metadata(
            output,
            {
                **profile_metadata(args.agent_mode, args.subagent),
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

    results = []
    for inst in instances:
        results.append(await run_one_instance_async(args, config, task, inst, output))

    write_jsonl(output / "_" / "results.jsonl", [instance_result_row(result) for result in results])
    write_adapter_metadata(
        output,
        {
            **profile_metadata(args.agent_mode, args.subagent),
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
