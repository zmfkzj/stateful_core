from __future__ import annotations

from contextlib import closing, contextmanager
import json
import os
import sqlite3
import subprocess
import sys
import tempfile
import time
import unittest
from types import SimpleNamespace
from pathlib import Path
from unittest.mock import Mock, patch

from .conftest import load_script


class _FakePopen:
    def __init__(self, agent_id, events, exit_code=0, timed_out=False):
        self.agent_id = agent_id
        self.events = events
        self.exit_code = exit_code
        self.timed_out = timed_out
        self.returncode = None
        self.pid = 999999
        self._timed_out_once = False

    def wait(self, timeout=None):
        self.events.append(("wait", self.agent_id))
        if self.timed_out and not self._timed_out_once:
            self._timed_out_once = True
            raise subprocess.TimeoutExpired(self.agent_id, timeout)
        self.returncode = self.exit_code
        return self.returncode


def _fake_launch(events, outcomes=None):
    outcomes = outcomes or {}

    def launch(arm_dir, workspace, agent_id, prompt_path, mode, cfg):
        events.append(("launch", agent_id, mode))
        exit_code, timed_out = outcomes.get(agent_id, (0, False))
        return SimpleNamespace(
            popen=_FakePopen(agent_id, events, exit_code, timed_out),
            agent_id=agent_id,
            started_monotonic=time.monotonic(),
        )

    return launch


def _suite_result(returncode):
    def run(*args, **kwargs):
        return subprocess.CompletedProcess(args[0] if args else [], returncode)

    return run


class StatefulBenchLiteTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.mod = load_script("statefulbench_lite.py")

    def test_generate_workspace_is_deterministic_and_red(self):
        with tempfile.TemporaryDirectory() as temp_dir:
            root = Path(temp_dir)
            first = root / "first"
            second = root / "second"
            self.mod.generate_workspace(first, 5)
            self.mod.generate_workspace(second, 5)

            def files(workspace):
                return {
                    path.relative_to(workspace): path.read_bytes()
                    for path in workspace.rglob("*")
                    if path.is_file() and ".git" not in path.relative_to(workspace).parts
                }

            self.assertEqual(files(first), files(second))
            red = subprocess.run(
                [sys.executable, "-m", "unittest", "discover", "-s", "tests", "-t", "."],
                cwd=first,
                stdout=subprocess.DEVNULL,
                stderr=subprocess.PIPE,
                text=True,
            )
            self.assertNotEqual(red.returncode, 0)
            self.assertIn("KeyError", red.stderr)

    def test_task_prompts_bind_module_key_and_shared_files(self):
        contracts = {
            "slug": "`slug(text: str) -> str`: lowercase; every run of non-alphanumeric chars becomes one `-`; strip leading/trailing `-`",
            "stats": "`stats(nums: list) -> tuple`: `(mean, median)`; mean is float; median averages the two middle values for even length",
            "rle": "`encode(text: str) -> str` run-length (`\"aaabcc\" -> \"a3b1c2\"`); also `decode(code: str) -> str`; registry value is `encode`",
            "roman": "`roman(n: int) -> str` for 1..3999",
            "intervals": "`intervals(pairs: list[tuple[int,int]]) -> list[tuple[int,int]]`: merge overlapping/touching, sorted",
        }
        for spec in self.mod.TASK_SPECS:
            with self.subTest(key=spec["key"]):
                prompt = self.mod.render_task_prompt(spec)
                self.assertIn(f"taskset/{spec['module']}.py", prompt)
                self.assertIn(f"Contract: {contracts[spec['key']]}", prompt)
                self.assertIn(f'REGISTRY["{spec["key"]}"]', prompt)
                self.assertIn("CHANGELOG.md", prompt)
        self.assertIn("python3 -m unittest discover -s tests -t .", self.mod.render_final_prompt())

    def test_duplicate_arm_selection_is_rejected(self):
        with self.assertRaisesRegex(self.mod.argparse.ArgumentTypeError, "duplicate"):
            self.mod._parse_arms("sequential,sequential")

        self.assertEqual(
            self.mod._parse_arms("sequential,parallel-off"),
            ["sequential", "parallel-off"],
        )

    def test_json_writer_is_atomic_and_preserves_prior_valid_json(self):
        with tempfile.TemporaryDirectory() as temp_dir:
            target = Path(temp_dir) / "results.json"
            original_value = {"total_tokens": 1}
            self.mod._write_json_atomically(target, original_value)
            self.assertEqual(json.loads(target.read_text(encoding="utf-8")), original_value)
            original = target.read_text(encoding="utf-8")

            with patch.object(self.mod.os, "replace", side_effect=OSError("replace failed")):
                with self.assertRaisesRegex(OSError, "replace failed"):
                    self.mod._write_json_atomically(target, {"total_tokens": 2})

            self.assertEqual(target.read_text(encoding="utf-8"), original)
            self.assertFalse(any(target.parent.glob(f".{target.name}.*.tmp")))

    def test_usage_parser_sums_tokens_and_tool_calls(self):
        with tempfile.TemporaryDirectory() as temp_dir:
            log = Path(temp_dir) / "agent.stdout.log"
            log.write_text(
                "\n".join(
                    [
                        json.dumps({"message": {"usage": {"totalTokens": 11, "toolCalls": 3}}}),
                        "not json",
                        json.dumps({"usage": {"total_tokens": 7, "tool_calls": 2}}),
                        json.dumps({"type": "tool_execution_start"}),
                    ]
                )
                + "\n",
                encoding="utf-8",
            )
            self.assertEqual(self.mod.usage_from_log(log), {"total_tokens": 18, "tool_calls": 5})

            event_log = Path(temp_dir) / "event-agent.stdout.log"
            event_log.write_text(
                "\n".join(
                    [
                        json.dumps({"type": "toolcall_start"}),
                        json.dumps({"type": "tool_execution_start"}),
                        json.dumps({"type": "tool_execution_end"}),
                        json.dumps({"type": "tool_execution_start"}),
                    ]
                )
                + "\n",
                encoding="utf-8",
            )
            self.assertEqual(self.mod.usage_from_log(event_log), {"total_tokens": 0, "tool_calls": 2})
    def test_copy_stateful_omp_agent_db_seeds_only_codex_oauth_credentials(self):
        with tempfile.TemporaryDirectory() as temp_dir:
            root = Path(temp_dir)
            host_home = root / "host-home"
            source = host_home / ".omp" / "profiles" / "stateful" / "agent" / "agent.db"
            source.parent.mkdir(parents=True)
            with closing(sqlite3.connect(source)) as db:
                db.execute(
                    "CREATE TABLE auth_credentials (provider TEXT, credential_type TEXT, data TEXT, disabled_cause TEXT, identity_key TEXT, created_at INTEGER, updated_at INTEGER)"
                )
                db.execute(
                    "CREATE TABLE unrelated_state (secret TEXT)"
                )
                db.executemany(
                    "INSERT INTO auth_credentials VALUES (?, ?, ?, ?, ?, ?, ?)",
                    [
                        ("openai-codex", "oauth", "oauth-token", None, "user", 1, 2),
                        ("openai-codex", "api-key", "api-key", None, "user", 3, 4),
                    ],
                )
                db.execute("INSERT INTO unrelated_state VALUES ('must not copy')")
                db.commit()
            agent_dir = root / "arm" / "omp-homes" / "agent-a" / "home" / ".omp" / "profiles" / "stateful" / "agent"

            self.mod.copy_stateful_omp_agent_db(host_home, agent_dir)

            with closing(sqlite3.connect(agent_dir / "agent.db")) as db:
                tables = {
                    row[0]
                    for row in db.execute(
                        "SELECT name FROM sqlite_master WHERE type = 'table'"
                    )
                }
                rows = db.execute(
                    "SELECT provider, credential_type, data FROM auth_credentials"
                ).fetchall()
            self.assertEqual(tables, {"auth_credentials"})
            self.assertEqual(rows, [("openai-codex", "oauth", "oauth-token")])
            missing_agent_dir = root / "missing" / "agent"
            self.mod.copy_stateful_omp_agent_db(root / "missing-home", missing_agent_dir)
            self.assertFalse((missing_agent_dir / "agent.db").exists())

    def test_resolve_omp_binary_is_absolute_or_errors(self):
        with patch.object(self.mod.shutil, "which", return_value="/tmp/bin/omp"):
            self.assertEqual(
                self.mod.resolve_omp_binary("custom-omp"),
                str(Path("/tmp/bin/omp").absolute()),
            )
        with patch.object(self.mod.shutil, "which", return_value=None):
            with self.assertRaisesRegex(ValueError, "--omp-bin"):
                self.mod.resolve_omp_binary("missing-omp")

    def test_denied_read_wrapper_blocks_dataset_reads_and_writes(self):
        denied = Path("/datasets/statefulbench-realworld").resolve()
        with patch.object(self.mod.shutil, "which", return_value="/usr/bin/sandbox-exec"):
            command = self.mod.wrap_omp_with_denied_reads(
                ["/opt/omp/bin/omp", "--mode", "json"],
                (denied,),
            )

        self.assertEqual(command[:2], [str(Path("/usr/bin/sandbox-exec").resolve()), "-p"])
        profile = command[2]
        self.assertIn("(allow default)", profile)
        self.assertIn("(allow network*)", profile)
        for operation in ("file-read*", "file-write*"):
            self.assertIn(f'(deny {operation} (literal "{denied}"))', profile)
            self.assertIn(f'(deny {operation} (subpath "{denied}"))', profile)
            self.assertLess(profile.index("(allow default)"), profile.index(f"(deny {operation}"))
        self.assertEqual(command[3:], ["/opt/omp/bin/omp", "--mode", "json"])

    def test_wait_agent_reaps_group_after_normal_parent_exit(self):
        events = []
        process = _FakePopen("normal", events)
        handle = SimpleNamespace(
            popen=process,
            agent_id="normal",
            started_monotonic=time.monotonic(),
        )
        with tempfile.TemporaryDirectory() as temp_dir:
            arm_dir = Path(temp_dir)
            (arm_dir / "logs").mkdir()
            (arm_dir / "logs" / "normal.stdout.log").write_text("", encoding="utf-8")

            def killpg(pid, sig):
                events.append(("killpg", pid, sig))

            with patch.object(self.mod.os, "killpg", side_effect=killpg):
                record, _ = self.mod._wait_agent(
                    handle, arm_dir, "task", self.mod.RunConfig()
                )

        self.assertEqual(record["exit_code"], 0)
        self.assertIsNone(record["cleanup_error"])
        self.assertEqual(
            events,
            [
                ("wait", "normal"),
                ("killpg", 999999, self.mod.signal.SIGTERM),
                ("killpg", 999999, self.mod.signal.SIGKILL),
            ],
        )

    def test_wait_agent_timeout_signal_denial_is_bounded_and_recorded(self):
        wait_timeouts = []

        class Process:
            pid = 999999
            returncode = None

            def wait(self, timeout=None):
                wait_timeouts.append(timeout)
                if len(wait_timeouts) == 1:
                    raise subprocess.TimeoutExpired("timed-out", timeout)
                self.returncode = 0
                return self.returncode

        handle = SimpleNamespace(
            popen=Process(),
            agent_id="timed-out",
            started_monotonic=time.monotonic(),
        )
        with tempfile.TemporaryDirectory() as temp_dir:
            arm_dir = Path(temp_dir)
            (arm_dir / "logs").mkdir()
            (arm_dir / "logs" / "timed-out.stdout.log").write_text("", encoding="utf-8")
            with patch.object(self.mod.os, "killpg", side_effect=PermissionError("denied")):
                record, _ = self.mod._wait_agent(
                    handle, arm_dir, "task", self.mod.RunConfig(timeout_s=1)
                )

        self.assertTrue(record["timed_out"])
        self.assertIn("Permission denied", record["cleanup_error"])
        self.assertEqual(wait_timeouts, [1, self.mod._CLEANUP_WAIT_S])

    def test_cleanup_denial_after_normal_exit_aborts_before_grading(self):
        suite_run = Mock(return_value=subprocess.CompletedProcess([], 0))
        with tempfile.TemporaryDirectory() as temp_dir:
            with patch.object(self.mod.os, "killpg", side_effect=PermissionError("denied")):
                result = self.mod.run_arm(
                    "sequential",
                    Path(temp_dir),
                    self.mod.RunConfig(tasks=2),
                    launch=_fake_launch([]),
                    suite_run=suite_run,
                )

        self.assertIn("cleanup failed", result["error"])
        self.assertFalse(result["cleared"])
        self.assertFalse(result["post_suite_ok"])
        suite_run.assert_not_called()

    def test_sequential_serializes_and_parallel_overlaps(self):
        task_ids = [f"task-{spec['key']}" for spec in self.mod.TASK_SPECS]
        with tempfile.TemporaryDirectory() as temp_dir:
            root = Path(temp_dir)
            sequential_events = []
            self.mod.run_arm(
                "sequential",
                root / "sequential",
                self.mod.RunConfig(tasks=5),
                launch=_fake_launch(sequential_events),
            )
            parallel_events = []
            self.mod.run_arm(
                "parallel-off",
                root / "parallel",
                self.mod.RunConfig(tasks=5),
                launch=_fake_launch(parallel_events),
            )

        def index(events, kind, agent_id):
            return next(position for position, event in enumerate(events) if event[:2] == (kind, agent_id))

        for current, following in zip(task_ids, task_ids[1:]):
            self.assertLess(index(sequential_events, "wait", current), index(sequential_events, "launch", following))
        self.assertLess(
            max(index(sequential_events, "wait", task_id) for task_id in task_ids),
            index(sequential_events, "launch", "final"),
        )
        self.assertLess(
            max(index(parallel_events, "launch", task_id) for task_id in task_ids),
            min(index(parallel_events, "wait", task_id) for task_id in task_ids),
        )
        self.assertLess(
            max(index(parallel_events, "wait", task_id) for task_id in task_ids),
            index(parallel_events, "launch", "final"),
        )

    def test_cleared_requires_zero_exits_no_timeouts_and_passing_post_suite(self):
        with tempfile.TemporaryDirectory() as temp_dir:
            root = Path(temp_dir)
            all_zero = self.mod.run_arm(
                "parallel-off",
                root / "all-zero",
                self.mod.RunConfig(tasks=5),
                launch=_fake_launch([]),
                suite_run=_suite_result(0),
            )
            failed_suite = self.mod.run_arm(
                "parallel-off",
                root / "failed-suite",
                self.mod.RunConfig(tasks=5),
                launch=_fake_launch([]),
                suite_run=_suite_result(1),
            )
            nonzero = self.mod.run_arm(
                "parallel-off",
                root / "nonzero",
                self.mod.RunConfig(tasks=5),
                launch=_fake_launch([], {"task-slug": (1, False)}),
                suite_run=_suite_result(0),
            )
            with patch.object(self.mod.os, "killpg"):
                timed_out = self.mod.run_arm(
                    "parallel-off",
                    root / "timed-out",
                    self.mod.RunConfig(tasks=5),
                    launch=_fake_launch([], {"final": (0, True)}),
                    suite_run=_suite_result(0),
                )
        self.assertTrue(all_zero["cleared"])
        self.assertTrue(all_zero["post_suite_ok"])
        self.assertFalse(failed_suite["cleared"])
        self.assertFalse(failed_suite["post_suite_ok"])
        self.assertFalse(nonzero["cleared"])
        self.assertFalse(timed_out["cleared"])

    def test_timeout_race_retains_timed_out_task_and_runs_final(self):
        with tempfile.TemporaryDirectory() as temp_dir:
            result = None
            with patch.object(self.mod.os, "killpg", side_effect=ProcessLookupError):
                result = self.mod.run_arm(
                    "parallel-off",
                    Path(temp_dir),
                    self.mod.RunConfig(tasks=2),
                    launch=_fake_launch([], {"task-slug": (0, True)}),
                )

        self.assertIsNone(result["error"])
        self.assertFalse(result["cleared"])
        self.assertEqual(
            {record["agent_id"] for record in result["agents"]},
            {"task-slug", "task-stats", "final"},
        )
        timed_out = next(record for record in result["agents"] if record["agent_id"] == "task-slug")
        self.assertTrue(timed_out["timed_out"])
    def test_arm_server_tolerates_signal_denial_from_outer_sandbox(self):
        process = Mock(pid=42)
        process.poll.return_value = None
        response = Mock(status=200)
        response.__enter__ = Mock(return_value=response)
        response.__exit__ = Mock(return_value=False)

        with tempfile.TemporaryDirectory() as temp_dir:
            with (
                patch.object(self.mod, "_available_port", return_value=45678),
                patch.object(self.mod.secrets, "token_urlsafe", return_value="token"),
                patch.object(self.mod.subprocess, "Popen", return_value=process),
                patch.object(self.mod.urllib.request, "urlopen", return_value=response),
                patch.object(self.mod.os, "killpg", side_effect=PermissionError),
            ):
                with self.mod.arm_stateful_server(
                    Path(temp_dir),
                    self.mod.RunConfig(stateful_binary="/tmp/stateful"),
                ) as env:
                    self.assertEqual(env["STATEFUL_SERVER_URL"], "http://127.0.0.1:45678")
                    self.assertEqual(env["STATEFUL_SERVER_TOKEN"], "token")

        process.terminate.assert_called_once_with()
        process.wait.assert_called_once_with(timeout=5)

    def test_launch_agent_isolates_host_secrets_but_preserves_runtime_environment(self):
        process = Mock()
        with tempfile.TemporaryDirectory() as temp_dir:
            root = Path(temp_dir)
            workspace = root / "workspace"
            workspace.mkdir()
            launch_env = {
                "PATH": "/workspace/bin:/usr/bin",
                "VIRTUAL_ENV": str(workspace / ".venv"),
                "OPENAI_API_KEY": "launch-secret",
                "PYTHONPATH": "/host/python",
            }

            def environment(_arm_dir, _agent_id, base_env):
                return {
                    **base_env,
                    "HOME": str(root / "home"),
                    "PI_CODING_AGENT_DIR": str(root / "agent"),
                }

            with (
                patch.dict(
                    self.mod.os.environ,
                    {
                        "OPENAI_API_KEY": "host-secret",
                        "AWS_SECRET_ACCESS_KEY": "aws-secret",
                        "GITHUB_TOKEN": "github-secret",
                        "PYTHONPATH": "/host/pythonpath",
                    },
                    clear=False,
                ),
                patch.object(self.mod, "omp_environment", side_effect=environment),
                patch.object(self.mod, "copy_openai_codex_auth"),
                patch.object(self.mod, "copy_stateful_omp_agent_db"),
                patch.object(self.mod, "prepare_environment"),
                patch.object(self.mod, "omp_command", return_value=["omp"]),
                patch.object(self.mod.subprocess, "Popen", return_value=process) as popen,
            ):
                self.mod.launch_agent(
                    root,
                    workspace,
                    "task",
                    root / "task.prompt.txt",
                    "stateful",
                    self.mod.RunConfig(
                        launch_env=launch_env,
                        stateful_runtime_env={"STATEFUL_SERVER_TOKEN": "runtime-token"},
                    ),
                )

        launched_env = popen.call_args.kwargs["env"]
        self.assertEqual(launched_env["PATH"], "/workspace/bin:/usr/bin")
        self.assertEqual(launched_env["VIRTUAL_ENV"], str(workspace / ".venv"))
        self.assertEqual(launched_env["STATEFUL_SERVER_TOKEN"], "runtime-token")
        for name in ("OPENAI_API_KEY", "AWS_SECRET_ACCESS_KEY", "GITHUB_TOKEN", "PYTHONPATH"):
            self.assertNotIn(name, launched_env)

    def test_launch_agent_uses_safe_path_for_explicit_empty_environment(self):
        process = Mock()
        with tempfile.TemporaryDirectory() as temp_dir:
            root = Path(temp_dir)
            workspace = root / "workspace"
            workspace.mkdir()

            def environment(_arm_dir, _agent_id, base_env):
                self.assertEqual(base_env, {"PATH": os.defpath})
                return {
                    **base_env,
                    "HOME": str(root / "home"),
                    "PI_CODING_AGENT_DIR": str(root / "agent"),
                }

            with (
                patch.object(self.mod, "omp_environment", side_effect=environment),
                patch.object(self.mod, "copy_openai_codex_auth"),
                patch.object(self.mod, "copy_stateful_omp_agent_db"),
                patch.object(self.mod, "prepare_environment"),
                patch.object(self.mod, "omp_command", return_value=["omp"]),
                patch.object(self.mod.subprocess, "Popen", return_value=process),
            ):
                self.mod.launch_agent(
                    root,
                    workspace,
                    "task",
                    root / "task.prompt.txt",
                    "no-state",
                    self.mod.RunConfig(launch_env={}),
                )

    def test_launch_agent_merges_workspace_virtualenv_environment_before_popen(self):
        process = Mock()
        with tempfile.TemporaryDirectory() as temp_dir:
            root = Path(temp_dir)
            workspace = root / "workspace"
            workspace.mkdir()
            venv = workspace / ".statefulbench-venv"
            launch_env = {
                "VIRTUAL_ENV": str(venv),
                "PATH": f"{venv / 'bin'}:/usr/bin",
            }
            with (
                patch.object(
                    self.mod,
                    "omp_environment",
                    side_effect=lambda _arm_dir, _agent_id, base_env: {
                        **base_env,
                        "HOME": str(root / "home"),
                        "PI_CODING_AGENT_DIR": str(root / "agent"),
                    },
                ),
                patch.object(self.mod, "copy_openai_codex_auth"),
                patch.object(self.mod, "copy_stateful_omp_agent_db"),
                patch.object(self.mod, "prepare_environment"),
                patch.object(self.mod, "omp_command", return_value=["omp"]),
                patch.object(self.mod.subprocess, "Popen", return_value=process) as popen,
            ):
                self.mod.launch_agent(
                    root,
                    workspace,
                    "task",
                    root / "task.prompt.txt",
                    "no-state",
                    self.mod.RunConfig(launch_env=launch_env),
                )

        launched_env = popen.call_args.kwargs["env"]
        self.assertEqual(launched_env["VIRTUAL_ENV"], str(venv))
        self.assertEqual(launched_env["PATH"], f"{venv / 'bin'}:/usr/bin")

    def test_parallel_on_shares_one_arm_server_across_agents(self):
        events = []

        @contextmanager
        def server(arm_dir, cfg):
            events.append(("server", "start"))
            yield {
                "STATEFUL_SERVER_URL": "http://127.0.0.1:45678",
                "STATEFUL_SERVER_TOKEN": "token",
            }
            events.append(("server", "stop"))

        def launch(arm_dir, workspace, agent_id, prompt_path, mode, cfg):
            self.assertEqual(
                cfg.stateful_runtime_env,
                {
                    "STATEFUL_SERVER_URL": "http://127.0.0.1:45678",
                    "STATEFUL_SERVER_TOKEN": "token",
                },
            )
            return _fake_launch(events)(arm_dir, workspace, agent_id, prompt_path, mode, cfg)

        with tempfile.TemporaryDirectory() as temp_dir:
            result = self.mod.run_arm(
                "parallel-on",
                Path(temp_dir),
                self.mod.RunConfig(tasks=2, stateful_binary="/tmp/stateful"),
                launch=launch,
                server=server,
                suite_run=_suite_result(0),
            )

        self.assertTrue(result["cleared"])
        self.assertEqual(events.count(("server", "start")), 1)
        self.assertEqual(events.count(("server", "stop")), 1)
        self.assertLess(events.index(("server", "start")), events.index(("launch", "task-slug", "stateful")))
        self.assertLess(events.index(("wait", "final")), events.index(("server", "stop")))
    def test_parallel_on_without_stateful_binary_errors(self):
        with tempfile.TemporaryDirectory() as temp_dir:
            root = Path(temp_dir)
            events = []
            cfg = self.mod.RunConfig(tasks=5, stateful_binary=None)
            results = [
                self.mod.run_arm("sequential", root, cfg, launch=_fake_launch(events)),
                self.mod.run_arm("parallel-off", root, cfg, launch=_fake_launch(events)),
                self.mod.run_arm("parallel-on", root, cfg, launch=_fake_launch(events)),
            ]
        self.assertIsNone(results[0]["error"])
        self.assertIsNone(results[1]["error"])
        self.assertIn("stateful binary", results[2]["error"])
        self.assertEqual(len([event for event in events if event[0] == "launch"]), 12)


if __name__ == "__main__":
    unittest.main()
