#!/usr/bin/env python3
"""StatefulBench Lite: a small shared-checkout efficiency benchmark."""

from __future__ import annotations

import argparse
from contextlib import closing, contextmanager, nullcontext
import importlib.util
import json
import os
import shutil
import signal
import secrets
import socket
import subprocess
import sqlite3
import sys
import time
import urllib.request
from dataclasses import dataclass, replace
from pathlib import Path
from typing import Callable


_HELPER_PATH = Path(__file__).with_name("overlap_omp_agent.py")
_HELPER_SPEC = importlib.util.spec_from_file_location("statefulbench_overlap_omp_agent", _HELPER_PATH)
if _HELPER_SPEC is None or _HELPER_SPEC.loader is None:
    raise RuntimeError(f"cannot import OMP helpers from {_HELPER_PATH}")
_HELPERS = importlib.util.module_from_spec(_HELPER_SPEC)
_HELPER_SPEC.loader.exec_module(_HELPERS)
omp_environment = _HELPERS.omp_environment
omp_command = _HELPERS.omp_command
copy_openai_codex_auth = _HELPERS.copy_openai_codex_auth
prepare_environment = _HELPERS.prepare_environment

def copy_stateful_omp_agent_db(source_home: Path, agent_dir: Path) -> None:
    source = source_home / ".omp" / "profiles" / "stateful" / "agent" / "agent.db"
    if not source.exists():
        return
    with closing(sqlite3.connect(source)) as source_db:
        auth_schema = source_db.execute(
            "SELECT sql FROM sqlite_master WHERE type = 'table' AND name = 'auth_credentials'"
        ).fetchone()
        rows = source_db.execute(
            """
            SELECT provider, credential_type, data, disabled_cause, identity_key, created_at, updated_at
            FROM auth_credentials
            WHERE provider = 'openai-codex' AND credential_type = 'oauth'
            """
        ).fetchall()
    if not rows:
        return
    target_db = agent_dir / "agent.db"
    agent_dir.mkdir(parents=True, exist_ok=True)
    target_db.unlink(missing_ok=True)
    with closing(sqlite3.connect(target_db)) as target:
        if auth_schema is not None:
            target.execute(auth_schema[0])
        target.executemany(
            """
            INSERT INTO auth_credentials
                (provider, credential_type, data, disabled_cause, identity_key, created_at, updated_at)
            VALUES (?, ?, ?, ?, ?, ?, ?)
            """,
            rows,
        )
        target.commit()


def resolve_omp_binary(omp_bin: str) -> str:
    resolved = shutil.which(omp_bin)
    if resolved is None:
        raise ValueError(f"--omp-bin is not an executable on PATH: {omp_bin}")
    return str(Path(resolved).absolute())


def _sandbox_literal(path: Path) -> str:
    return str(path.absolute()).replace("\\", "\\\\").replace('"', '\\"')


def wrap_omp_with_denied_reads(command: list[str], denied_read_paths: tuple[Path, ...]) -> list[str]:
    if not denied_read_paths:
        return command
    sandbox_exec = shutil.which("sandbox-exec")
    if sandbox_exec is None:
        raise RuntimeError("sandbox-exec is required to deny real-world dataset reads")
    denied_rules = [
        f'(deny file-read* (literal "{_sandbox_literal(path)}"))'
        f'\n(deny file-read* (subpath "{_sandbox_literal(path)}"))'
        for path in denied_read_paths
    ]
    profile = "\n".join(["(version 1)", "(allow default)", "(allow network*)", *denied_rules])
    return [str(Path(sandbox_exec).absolute()), "-p", profile, *command]




TASK_SPECS = (
    {
        "key": "slug",
        "module": "slug",
        "fn": "slug",
        "contract": "`slug(text: str) -> str`: lowercase; every run of non-alphanumeric chars becomes one `-`; strip leading/trailing `-`",
    },
    {
        "key": "stats",
        "module": "stats",
        "fn": "stats",
        "contract": "`stats(nums: list) -> tuple`: `(mean, median)`; mean is float; median averages the two middle values for even length",
    },
    {
        "key": "rle",
        "module": "rle",
        "fn": "encode",
        "contract": "`encode(text: str) -> str` run-length (`\"aaabcc\" -> \"a3b1c2\"`); also `decode(code: str) -> str`; registry value is `encode`",
    },
    {
        "key": "roman",
        "module": "roman",
        "fn": "roman",
        "contract": "`roman(n: int) -> str` for 1..3999",
    },
    {
        "key": "intervals",
        "module": "intervals",
        "fn": "intervals",
        "contract": "`intervals(pairs: list[tuple[int,int]]) -> list[tuple[int,int]]`: merge overlapping/touching, sorted",
    },
)

FINAL_PROMPT = """You are the integration reviewer for this repository.
Run: python3 -m unittest discover -s tests -t .
Fix every failure: missing registry entries, broken imports, damaged shared files (taskset/__init__.py, CHANGELOG.md), or incorrect task modules.
Finish when the full suite passes.
"""


@dataclass
class AgentHandle:
    popen: subprocess.Popen
    agent_id: str
    started_monotonic: float


@dataclass
class RunConfig:
    tasks: int = 5
    timeout_s: int = 900
    model: str = "openai-codex/gpt-5.6-terra"
    thinking: str = "high"
    omp_bin: str = "omp"
    stateful_binary: str | None = None
    stateful_runtime_env: dict[str, str] | None = None
    launch_env: dict[str, str] | None = None
    denied_read_paths: tuple[Path, ...] = ()
    trial: int = 1


def selected_specs(task_count: int) -> tuple[dict[str, str], ...]:
    if not 2 <= task_count <= len(TASK_SPECS):
        raise ValueError(f"task_count must be between 2 and {len(TASK_SPECS)}")
    return TASK_SPECS[:task_count]


def render_task_prompt(spec: dict[str, str]) -> str:
    return (
        "You are working in a shared repository checkout. Other agents may be editing other files concurrently.\n"
        f"Task: implement `taskset/{spec['module']}.py`.\n"
        f"Contract: {spec['contract']}\n"
        f"Register it: in `taskset/__init__.py`, import your function and add `REGISTRY[\"{spec['key']}\"] = {spec['fn']}`.\n"
        f"Append exactly one line `- add {spec['key']}` to `CHANGELOG.md`.\n"
        f"Verify with: python3 -m unittest discover -s tests -t . -p 'test_{spec['key']}.py'\n"
        "Do not modify other task modules or their tests. Keep unrelated lines in shared files intact.\n"
    )


def render_final_prompt() -> str:
    return FINAL_PROMPT


def _task_test(spec: dict[str, str]) -> str:
    key = spec["key"]
    if key == "slug":
        body = '''    def test_slug(self):
        self.assertEqual(REGISTRY["slug"]("Hello, World!"), "hello-world")
        self.assertEqual(REGISTRY["slug"]("  A  B  "), "a-b")
        self.assertEqual(REGISTRY["slug"]("---"), "")
'''
    elif key == "stats":
        body = '''    def test_stats(self):
        self.assertEqual(REGISTRY["stats"]([1, 2, 3]), (2.0, 2))
        self.assertEqual(REGISTRY["stats"]([4, 1, 3, 2]), (2.5, 2.5))
'''
    elif key == "rle":
        body = '''    def test_encode(self):
        self.assertEqual(REGISTRY["rle"]("aaabcc"), "a3b1c2")
        self.assertEqual(REGISTRY["rle"](""), "")

    def test_decode(self):
        from taskset.rle import decode

        self.assertEqual(decode("a3b1c2"), "aaabcc")
'''
    elif key == "roman":
        body = '''    def test_roman(self):
        self.assertEqual(REGISTRY["roman"](4), "IV")
        self.assertEqual(REGISTRY["roman"](1994), "MCMXCIV")
        self.assertEqual(REGISTRY["roman"](3999), "MMMCMXCIX")
'''
    else:
        body = '''    def test_intervals(self):
        self.assertEqual(REGISTRY["intervals"]([(1, 3), (2, 6), (8, 10)]), [(1, 6), (8, 10)])
        self.assertEqual(REGISTRY["intervals"]([(1, 2), (2, 3)]), [(1, 3)])
        self.assertEqual(REGISTRY["intervals"]([]), [])
'''
    return "from __future__ import annotations\n\nimport unittest\n\nfrom taskset import REGISTRY\n\n\nclass TaskTests(unittest.TestCase):\n" + body


def _integration_test(specs: tuple[dict[str, str], ...]) -> str:
    keys = ", ".join(repr(spec["key"]) for spec in specs)
    representatives = {
        "slug": 'self.assertEqual(REGISTRY["slug"]("Hello"), "hello")',
        "stats": 'self.assertEqual(REGISTRY["stats"]([1, 2, 3]), (2.0, 2))',
        "rle": 'self.assertEqual(REGISTRY["rle"]("aa"), "a2")',
        "roman": 'self.assertEqual(REGISTRY["roman"](4), "IV")',
        "intervals": 'self.assertEqual(REGISTRY["intervals"]([(1, 2), (2, 3)]), [(1, 3)])',
    }
    calls = "\n".join(f"        {representatives[spec['key']]}" for spec in specs)
    changelog_checks = "\n".join(
        f'        self.assertEqual(lines.count("- add {spec["key"]}"), 1)' for spec in specs
    )
    return f'''from __future__ import annotations

import unittest
from pathlib import Path

from taskset import REGISTRY


class IntegrationTests(unittest.TestCase):
    def test_registry_has_selected_keys(self):
        self.assertEqual(set(REGISTRY), {{{keys}}})

    def test_representative_calls(self):
{calls}

    def test_changelog_has_one_line_per_task(self):
        lines = (Path(__file__).parents[1] / "CHANGELOG.md").read_text(encoding="utf-8").splitlines()
{changelog_checks}
'''


def generate_workspace(dest: Path, task_count: int) -> None:
    """Create a deterministic, intentionally incomplete git workspace."""
    specs = selected_specs(task_count)
    if dest.exists() and any(dest.iterdir()):
        raise ValueError(f"workspace destination is not empty: {dest}")
    dest.mkdir(parents=True, exist_ok=True)
    taskset = dest / "taskset"
    tests = dest / "tests"
    taskset.mkdir()
    tests.mkdir()
    (tests / "__init__.py").write_text("", encoding="utf-8")
    (taskset / "__init__.py").write_text(
        '"""Task registry. Each task registers exactly one callable."""\nREGISTRY: dict[str, object] = {}\n',
        encoding="utf-8",
    )
    (dest / "CHANGELOG.md").write_text("# Changelog\n", encoding="utf-8")
    (dest / "README.md").write_text(
        "taskset is a small task registry. Every task registers itself in taskset/__init__.py and appends one CHANGELOG line.\n",
        encoding="utf-8",
    )
    for spec in specs:
        (tests / f"test_{spec['key']}.py").write_text(_task_test(spec), encoding="utf-8")
    (tests / "test_integration.py").write_text(_integration_test(specs), encoding="utf-8")
    subprocess.run(["git", "init"], cwd=dest, check=True, stdout=subprocess.DEVNULL)
    subprocess.run(["git", "add", "-A"], cwd=dest, check=True)
    subprocess.run(
        ["git", "-c", "user.email=bench@local", "-c", "user.name=bench", "commit", "-m", "seed taskset workspace"],
        cwd=dest,
        check=True,
        stdout=subprocess.DEVNULL,
    )


def usage_from_log(path: Path) -> dict[str, int]:
    total_tokens = 0
    usage_tool_calls = 0
    execution_tool_calls = 0
    has_usage_tool_calls = False
    if not path.exists():
        return {"total_tokens": total_tokens, "tool_calls": usage_tool_calls}
    for line in path.read_text(encoding="utf-8", errors="ignore").splitlines():
        try:
            value = json.loads(line)
        except json.JSONDecodeError:
            continue
        if not isinstance(value, dict):
            continue
        if value.get("type") == "tool_execution_start":
            execution_tool_calls += 1
        message = value.get("message")
        usage = message.get("usage") if isinstance(message, dict) else None
        if not isinstance(usage, dict):
            usage = value.get("usage")
        if not isinstance(usage, dict):
            continue
        total_tokens += int(usage.get("totalTokens") or usage.get("total_tokens") or 0)
        if "toolCalls" in usage or "tool_calls" in usage:
            has_usage_tool_calls = True
            usage_tool_calls += int(usage.get("toolCalls") or usage.get("tool_calls") or 0)
    return {
        "total_tokens": total_tokens,
        "tool_calls": usage_tool_calls if has_usage_tool_calls else execution_tool_calls,
    }


def _available_port() -> int:
    with socket.socket() as sock:
        sock.bind(("127.0.0.1", 0))
        return int(sock.getsockname()[1])


@contextmanager
def arm_stateful_server(arm_dir: Path, cfg: RunConfig):
    port = _available_port()
    token = secrets.token_urlsafe(32)
    workspace_id = f"statefulbench-lite-{cfg.trial}"
    server_home = arm_dir / "stateful-server-home"
    server_home.mkdir(parents=True, exist_ok=True)
    logs = arm_dir / "logs"
    logs.mkdir(parents=True, exist_ok=True)
    stdout_path = logs / "stateful-server.stdout.log"
    stderr_path = logs / "stateful-server.stderr.log"
    env = dict(os.environ)
    env["HOME"] = str(server_home)
    env["STATEFUL_HOME"] = str(server_home)
    command = [
        cfg.stateful_binary,
        "server",
        "start",
        "--foreground",
        "--host",
        "127.0.0.1",
        "--port",
        str(port),
        "--token",
        token,
        "--workspace-id",
        workspace_id,
        "--coordination-mode",
        "enforcement",
    ]
    with stdout_path.open("w", encoding="utf-8") as stdout, stderr_path.open("w", encoding="utf-8") as stderr:
        process = subprocess.Popen(command, env=env, stdout=stdout, stderr=stderr, start_new_session=True)

    server_url = f"http://127.0.0.1:{port}"
    deadline = time.monotonic() + 10
    while True:
        if process.poll() is not None:
            detail = stderr_path.read_text(encoding="utf-8", errors="ignore").strip()
            raise RuntimeError(f"stateful server exited before becoming healthy: {detail}")
        try:
            with urllib.request.urlopen(f"{server_url}/health", timeout=0.2) as response:
                if response.status == 200:
                    break
        except OSError:
            pass
        if time.monotonic() >= deadline:
            os.killpg(process.pid, signal.SIGKILL)
            process.wait()
            raise RuntimeError("stateful server did not become healthy within 10 seconds")
        time.sleep(0.05)

    try:
        yield {
            "STATEFUL_SERVER_URL": server_url,
            "STATEFUL_SERVER_TOKEN": token,
            "STATEFUL_WORKSPACE_ID": workspace_id,
        }
    finally:
        signal_denied = False
        try:
            os.killpg(process.pid, signal.SIGTERM)
        except PermissionError:
            try:
                process.terminate()
            except PermissionError:
                signal_denied = True
        except ProcessLookupError:
            pass
        if not signal_denied:
            try:
                process.wait(timeout=5)
            except subprocess.TimeoutExpired:
                try:
                    os.killpg(process.pid, signal.SIGKILL)
                except PermissionError:
                    try:
                        process.kill()
                    except PermissionError:
                        signal_denied = True
                except ProcessLookupError:
                    pass
                if not signal_denied:
                    process.wait()


def launch_agent(
    arm_dir: Path,
    workspace: Path,
    agent_id: str,
    prompt_path: Path,
    mode: str,
    cfg: RunConfig,
) -> AgentHandle:
    env = omp_environment(arm_dir, agent_id)
    if cfg.launch_env:
        env.update(cfg.launch_env)
    if cfg.stateful_runtime_env:
        env.update(cfg.stateful_runtime_env)
    copy_openai_codex_auth(Path.home(), Path(env["HOME"]))
    copy_stateful_omp_agent_db(Path.home(), Path(env["PI_CODING_AGENT_DIR"]))
    prepare_environment(env, workspace, mode, cfg.stateful_binary)
    omp_binary = resolve_omp_binary(cfg.omp_bin)
    logs = arm_dir / "logs"
    logs.mkdir(parents=True, exist_ok=True)
    stdout = (logs / f"{agent_id}.stdout.log").open("w", encoding="utf-8")
    stderr = (logs / f"{agent_id}.stderr.log").open("w", encoding="utf-8")
    try:
        command = wrap_omp_with_denied_reads(
            omp_command(workspace, prompt_path, omp_binary, cfg.model, cfg.thinking),
            cfg.denied_read_paths,
        )
        popen = subprocess.Popen(
            command,
            env=env,
            cwd=workspace,
            stdout=stdout,
            stderr=stderr,
            start_new_session=True,
        )
    finally:
        stdout.close()
        stderr.close()
    return AgentHandle(popen=popen, agent_id=agent_id, started_monotonic=time.monotonic())


def _cleanup_agent_group(popen: subprocess.Popen) -> None:
    for group_signal in (signal.SIGTERM, signal.SIGKILL):
        try:
            os.killpg(popen.pid, group_signal)
        except (PermissionError, ProcessLookupError):
            pass


def _wait_agent(handle: AgentHandle, arm_dir: Path, kind: str, cfg: RunConfig) -> tuple[dict, float]:
    timed_out = False
    try:
        try:
            completed = handle.popen.wait(timeout=cfg.timeout_s)
        except subprocess.TimeoutExpired:
            timed_out = True
            _cleanup_agent_group(handle.popen)
            completed = handle.popen.wait()
    finally:
        _cleanup_agent_group(handle.popen)
    ended = time.monotonic()
    exit_code = getattr(handle.popen, "returncode", None)
    if exit_code is None:
        exit_code = completed
    usage = usage_from_log(arm_dir / "logs" / f"{handle.agent_id}.stdout.log")
    return (
        {
            "agent_id": handle.agent_id,
            "kind": kind,
            "exit_code": exit_code,
            "timed_out": timed_out,
            "wall_time_s": max(0.0, ended - getattr(handle, "started_monotonic", ended)),
            **usage,
        },
        ended,
    )


def _empty_arm_result(arm: str, trial: int, error: str | None = None) -> dict:
    return {
        "arm": arm,
        "trial": trial,
        "cleared": False,
        "error": error,
        "arm_wall_time_s": 0.0,
        "tasks_wall_time_s": 0.0,
        "final_wall_time_s": 0.0,
        "total_tokens": 0,
        "total_tool_calls": 0,
        "post_suite_ok": False,
        "agents": [],
    }


def run_arm(
    arm: str,
    out_dir: Path,
    cfg: RunConfig,
    launch: Callable[[Path, Path, str, Path, str, RunConfig], AgentHandle] = launch_agent,
    server=arm_stateful_server,
    suite_run: Callable = subprocess.run,
) -> dict:
    if arm not in {"sequential", "parallel-off", "parallel-on"}:
        raise ValueError(f"unknown arm: {arm}")
    trial = getattr(cfg, "trial", 1)
    if arm == "parallel-on" and not getattr(cfg, "stateful_binary", None):
        return _empty_arm_result(arm, trial, "parallel-on requires a resolvable stateful binary")

    arm_dir = out_dir / arm / f"trial-{trial}"
    workspace = arm_dir / "workspace"
    generate_workspace(workspace, cfg.tasks)
    prompts = arm_dir / "prompts"
    prompts.mkdir(parents=True, exist_ok=True)
    specs = selected_specs(cfg.tasks)
    for spec in specs:
        (prompts / f"task-{spec['key']}.prompt.txt").write_text(render_task_prompt(spec), encoding="utf-8")
    final_prompt = prompts / "final.prompt.txt"
    final_prompt.write_text(render_final_prompt(), encoding="utf-8")

    mode = "stateful" if arm == "parallel-on" else "no-state"
    server_context = server(arm_dir, cfg) if arm == "parallel-on" else nullcontext({})
    try:
        runtime_env = server_context.__enter__()
        cfg = replace(cfg, stateful_runtime_env=runtime_env or None)
    except Exception as exc:
        result = _empty_arm_result(arm, trial, str(exc))
        arm_dir.mkdir(parents=True, exist_ok=True)
        (arm_dir / "results.json").write_text(json.dumps(result, indent=2) + "\n", encoding="utf-8")
        return result
    agents: list[dict] = []
    task_handles: list[AgentHandle] = []
    task_started: float | None = None
    task_ended: float | None = None
    arm_started: float | None = None
    final_started: float | None = None
    final_ended: float | None = None
    error: str | None = None

    def start_task(spec: dict[str, str]) -> AgentHandle:
        nonlocal task_started, arm_started
        handle = launch(arm_dir, workspace, f"task-{spec['key']}", prompts / f"task-{spec['key']}.prompt.txt", mode, cfg)
        started = getattr(handle, "started_monotonic", time.monotonic())
        task_started = started if task_started is None else min(task_started, started)
        arm_started = started if arm_started is None else min(arm_started, started)
        return handle

    try:
        if arm == "sequential":
            for spec in specs:
                handle = start_task(spec)
                record, ended = _wait_agent(handle, arm_dir, "task", cfg)
                agents.append(record)
                task_ended = ended
        else:
            for spec in specs:
                task_handles.append(start_task(spec))
            for handle in task_handles:
                record, ended = _wait_agent(handle, arm_dir, "task", cfg)
                agents.append(record)
                task_ended = ended if task_ended is None else max(task_ended, ended)

        final_handle = launch(arm_dir, workspace, "final", final_prompt, mode, cfg)
        final_started = getattr(final_handle, "started_monotonic", time.monotonic())
        arm_started = final_started if arm_started is None else min(arm_started, final_started)
        final_record, final_ended = _wait_agent(final_handle, arm_dir, "final", cfg)
        agents.append(final_record)
    except Exception as exc:
        error = str(exc)
        for handle in task_handles:
            if not any(record["agent_id"] == handle.agent_id for record in agents):
                try:
                    record, ended = _wait_agent(handle, arm_dir, "task", cfg)
                    agents.append(record)
                    task_ended = ended if task_ended is None else max(task_ended, ended)
                except Exception:
                    pass

    try:
        server_context.__exit__(None, None, None)
    except Exception as exc:
        error = str(exc)

    post_suite_ok = False
    if final_ended is not None:
        post_suite_ok = suite_run(
            [sys.executable, "-m", "unittest", "discover", "-s", "tests", "-t", "."],
            cwd=workspace,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
        ).returncode == 0

    tasks_wall_time_s = 0.0 if task_started is None or task_ended is None else max(0.0, task_ended - task_started)
    arm_end = final_ended if final_ended is not None else task_ended
    arm_wall_time_s = 0.0 if arm_started is None or arm_end is None else max(0.0, arm_end - arm_started)
    final_wall_time_s = 0.0 if final_started is None or final_ended is None else max(0.0, final_ended - final_started)
    expected_agents = len(specs) + 1
    result = {
        "arm": arm,
        "trial": trial,
        "cleared": post_suite_ok and error is None and len(agents) == expected_agents and all(
            record["exit_code"] == 0 and not record["timed_out"] for record in agents
        ),
        "error": error,
        "arm_wall_time_s": arm_wall_time_s,
        "tasks_wall_time_s": tasks_wall_time_s,
        "final_wall_time_s": final_wall_time_s,
        "total_tokens": sum(record["total_tokens"] for record in agents),
        "total_tool_calls": sum(record["tool_calls"] for record in agents),
        "post_suite_ok": post_suite_ok,
        "agents": agents,
    }
    arm_dir.mkdir(parents=True, exist_ok=True)
    (arm_dir / "results.json").write_text(json.dumps(result, indent=2) + "\n", encoding="utf-8")
    return result


def _parse_arms(value: str) -> list[str]:
    arms = [arm.strip() for arm in value.split(",") if arm.strip()]
    valid = {"sequential", "parallel-off", "parallel-on"}
    invalid = [arm for arm in arms if arm not in valid]
    if not arms or invalid:
        raise argparse.ArgumentTypeError(f"arms must be comma-separated values from {', '.join(sorted(valid))}")
    return arms


def _table(results: list[dict]) -> str:
    lines = [
        "| arm | trial | cleared | arm_wall_time_s | total_tokens | total_tool_calls | post_suite_ok |",
        "| --- | ---: | --- | ---: | ---: | ---: | --- |",
    ]
    for result in results:
        lines.append(
            "| {arm} | {trial} | {cleared} | {wall:.3f} | {tokens} | {tools} | {suite} |".format(
                arm=result["arm"],
                trial=result["trial"],
                cleared=result["cleared"],
                wall=result["arm_wall_time_s"],
                tokens=result["total_tokens"],
                tools=result["total_tool_calls"],
                suite=result["post_suite_ok"],
            )
        )
    return "\n".join(lines)


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser()
    subparsers = parser.add_subparsers(dest="command", required=True)
    generate = subparsers.add_parser("generate")
    generate.add_argument("--dest", type=Path, required=True)
    generate.add_argument("--tasks", type=int, default=5)

    run = subparsers.add_parser("run")
    run.add_argument("--arms", type=_parse_arms, default=_parse_arms("sequential,parallel-off,parallel-on"))
    run.add_argument("--tasks", type=int, default=5)
    run.add_argument("--trials", type=int, default=1)
    run.add_argument("--model", default="openai-codex/gpt-5.6-terra")
    run.add_argument("--thinking", default="high")
    run.add_argument("--omp-bin", default="omp")
    run.add_argument("--stateful-binary", default=shutil.which("stateful"))
    run.add_argument("--timeout-s", type=int, default=900)
    run.add_argument("--out", type=Path, default=Path("tmp/statefulbench-lite") / time.strftime("%Y%m%d-%H%M%S", time.gmtime()))

    args = parser.parse_args(argv)
    if not 2 <= args.tasks <= len(TASK_SPECS):
        parser.error(f"--tasks must be between 2 and {len(TASK_SPECS)}")
    if args.command == "generate":
        generate_workspace(args.dest, args.tasks)
        return 0
    if args.trials < 1:
        parser.error("--trials must be at least 1")
    if args.timeout_s < 1:
        parser.error("--timeout-s must be at least 1")
    if "parallel-on" in args.arms and not args.stateful_binary:
        parser.error("parallel-on requires a resolvable stateful binary; pass --stateful-binary")

    try:
        omp_binary = resolve_omp_binary(args.omp_bin)
    except ValueError as error:
        parser.error(str(error))

    cfg = RunConfig(
        tasks=args.tasks,
        timeout_s=args.timeout_s,
        model=args.model,
        thinking=args.thinking,
        omp_bin=omp_binary,
        stateful_binary=args.stateful_binary,
    )
    results = []
    for arm in args.arms:
        for trial in range(1, args.trials + 1):
            results.append(run_arm(arm, args.out, replace(cfg, trial=trial)))
    summary = {
        "model": cfg.model,
        "thinking": cfg.thinking,
        "tasks": cfg.tasks,
        "trials": args.trials,
        "generated_at": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()),
        "arms": results,
    }
    args.out.mkdir(parents=True, exist_ok=True)
    (args.out / "summary.json").write_text(json.dumps(summary, indent=2) + "\n", encoding="utf-8")
    print(_table(results))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
