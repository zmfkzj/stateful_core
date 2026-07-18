from __future__ import annotations

import ctypes
import json
import hashlib
import os
import runpy
import subprocess
import tempfile
import unittest
import time
import tarfile
import textwrap
import sys
from pathlib import Path
from unittest.mock import Mock, patch

from .conftest import load_script


class DockerRuntimeTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls) -> None:
        cls.mod = load_script("statefulbench_docker.py")
        cls.entry = load_script("statefulbench_container_entry.py")

    def test_inspect_runtime_records_immutable_native_identity(self) -> None:
        completed = subprocess.CompletedProcess(
            ["docker"],
            0,
            stdout=json.dumps(
                [
                    {
                        "Id": "sha256:abc",
                        "RepoDigests": ["statefulbench@sha256:def"],
                        "Os": "linux",
                        "Architecture": "arm64",
                    }
                ]
            ),
            stderr="",
        )
        def runner(command, **_kwargs):
            if command[1:3] == ["version", "--format"]:
                return subprocess.CompletedProcess(command, 0, "linux/arm64\n", "")
            return completed

        runtime = self.mod.inspect_runtime(
            sys.executable, "statefulbench-realworld:local", runner=runner
        )
        self.assertEqual(runtime.image_id, "sha256:abc")
        self.assertEqual(runtime.repo_digests, ("statefulbench@sha256:def",))
        self.assertEqual(runtime.platform, "linux/arm64")
        self.assertTrue(Path(runtime.binary).is_absolute())

    def test_inspect_runtime_accepts_arm64_image_on_non_arm_daemon(self) -> None:
        image = subprocess.CompletedProcess(
            ["docker"],
            0,
            stdout=json.dumps(
                [
                    {
                        "Id": "sha256:abc",
                        "RepoDigests": [],
                        "Os": "linux",
                        "Architecture": "arm64",
                    }
                ]
            ),
            stderr="",
        )

        def runner(command, **_kwargs):
            if command[1:3] == ["version", "--format"]:
                return subprocess.CompletedProcess(command, 0, "linux/amd64\n", "")
            self.assertEqual(command[1:3], ["image", "inspect"])
            return image

        runtime = self.mod.inspect_runtime(
            sys.executable, "statefulbench-realworld:local", runner=runner
        )
        self.assertEqual(runtime.platform, "linux/arm64")
        self.assertEqual(runtime.server_platform, "linux/amd64")

    def test_inspect_runtime_rejects_amd64_image_even_on_amd64_daemon(self) -> None:
        image = subprocess.CompletedProcess(
            ["docker"],
            0,
            stdout=json.dumps(
                [
                    {
                        "Id": "sha256:amd64",
                        "RepoDigests": ["statefulbench@sha256:amd64"],
                        "Os": "linux",
                        "Architecture": "amd64",
                    }
                ]
            ),
            stderr="",
        )

        def runner(command, **_kwargs):
            if command[1:3] == ["version", "--format"]:
                return subprocess.CompletedProcess(command, 0, "linux/amd64\n", "")
            self.assertEqual(command[1:3], ["image", "inspect"])
            return image

        with self.assertRaisesRegex(RuntimeError, "linux/arm64"):
            self.mod.inspect_runtime(sys.executable, "statefulbench-realworld:local", runner=runner)

    def test_inspect_runtime_fails_closed_on_missing_daemon_or_non_linux_image(self) -> None:
        with self.assertRaisesRegex(RuntimeError, r"Docker .*inspection failed"):
            self.mod.inspect_runtime(
                sys.executable,
                "missing",
                runner=Mock(
                    return_value=subprocess.CompletedProcess([], 1, "", "daemon unavailable")
                ),
            )

    def test_docker_command_uses_resolved_binary_and_native_platform(self) -> None:
        runtime = self.mod.DockerRuntime(
            binary="/usr/local/bin/docker",
            image="statefulbench-realworld:local",
            image_id="sha256:abc",
            repo_digests=("statefulbench@sha256:def",),
            platform="linux/arm64",
        )
        command = self.mod.docker_command(runtime, "run", "--rm", runtime.image, "omp", "--version")
        self.assertEqual(command[:3], [runtime.binary, "run", "--platform=linux/arm64"])
        self.assertEqual(command[3:], ["--rm", runtime.image, "omp", "--version"])

    def test_entrypoint_creates_session_records_pid_and_execs_without_shell(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            pid_file = Path(directory) / "agent.pid.json"
            temporary = pid_file.with_suffix(".tmp")
            with (
                patch.object(self.entry, "_set_child_subreaper") as subreaper,
                patch.object(self.entry, "_redirect_child_output") as redirect,
                patch.object(self.entry.os, "fork", return_value=0) as fork,
                patch.object(self.entry.os, "setsid") as setsid,
                patch.object(self.entry.os, "getpid", return_value=42),
                patch.object(self.entry.os, "getpgrp", return_value=42),
                patch.object(self.entry.os, "replace") as replace,
                patch.object(self.entry.os, "execvpe", side_effect=RuntimeError("exec")) as execvpe,
            ):
                with self.assertRaisesRegex(RuntimeError, "exec"):
                    self.entry.main(["entry", str(pid_file), "/tmp/out", "/tmp/err", "/usr/bin/env", "true"])

            assert json.loads(temporary.read_text(encoding="utf-8")) == {"pid": 42, "pgid": 42}
            subreaper.assert_called_once_with()
            fork.assert_called_once_with()
            setsid.assert_called_once_with()
            replace.assert_called_once_with(temporary, pid_file)
            redirect.assert_called_once_with("/tmp/out", "/tmp/err")
            execvpe.assert_called_once_with("/usr/bin/env", ["/usr/bin/env", "true"], os.environ)

    def test_entrypoint_forks_group_leader_and_records_exec_child_identity(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            pid_file = Path(directory) / "agent.pid.json"
            temporary = pid_file.with_suffix(".tmp")
            with (
                patch.object(self.entry, "_set_child_subreaper") as subreaper,
                patch.object(self.entry, "_redirect_child_output") as redirect,
                patch.object(self.entry.os, "fork", return_value=0) as fork,
                patch.object(self.entry.os, "setsid") as setsid,
                patch.object(self.entry.os, "getpid", return_value=99),
                patch.object(self.entry.os, "getpgrp", return_value=99),
                patch.object(self.entry.os, "replace") as replace,
                patch.object(self.entry.os, "execvpe", side_effect=RuntimeError("exec")) as execvpe,
            ):
                with self.assertRaisesRegex(RuntimeError, "exec"):
                    self.entry.main(["entry", str(pid_file), "/tmp/out", "/tmp/err", "/usr/bin/env", "true"])

            assert json.loads(temporary.read_text(encoding="utf-8")) == {"pid": 99, "pgid": 99}
            fork.assert_called_once_with()
            setsid.assert_called_once_with()
            replace.assert_called_once_with(temporary, pid_file)
            execvpe.assert_called_once_with("/usr/bin/env", ["/usr/bin/env", "true"], os.environ)
            redirect.assert_called_once_with("/tmp/out", "/tmp/err")
            subreaper.assert_called_once_with()

    def test_entrypoint_group_leader_parent_waits_for_child_exit(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            pid_file = Path(directory) / "agent.pid.json"
            with (
                patch.object(self.entry, "_set_child_subreaper") as subreaper,
                patch.object(self.entry, "_emit_completion") as emit,
                patch.object(self.entry.os, "fork", return_value=99) as fork,
                patch.object(self.entry.os, "setsid") as setsid,
                patch.object(self.entry.os, "getpid", return_value=42),
                patch.object(self.entry.os, "getpgrp", return_value=42),
                patch.object(
                    self.entry.os,
                    "waitpid",
                    side_effect=[(99, 0), ChildProcessError],
                ) as waitpid,
                patch.object(self.entry.os, "execvpe", side_effect=RuntimeError("exec")),
            ):
                self.assertEqual(
                    self.entry.main(["entry", str(pid_file), "/tmp/out", "/tmp/err", "/usr/bin/env", "true"]),
                    0,
                )
                emit.assert_called_once_with(99, 0)

        subreaper.assert_called_once_with()
        fork.assert_called_once_with()
        self.assertEqual(
            waitpid.call_args_list,
            [unittest.mock.call(99, 0), unittest.mock.call(-1, 0)],
        )
        setsid.assert_not_called()
    def test_entrypoint_subreaper_waits_for_escaped_descendants(self) -> None:
        with (
            patch.object(self.entry, "_set_child_subreaper", create=True) as subreaper,
            patch.object(self.entry.os, "fork", return_value=99),
            patch.object(self.entry.os, "getpid", return_value=42),
            patch.object(self.entry.os, "getpgrp", return_value=42),
            patch.object(
                self.entry.os,
                "waitpid",
                side_effect=[(99, 0), (100, 0), ChildProcessError],
            ) as waitpid,
            patch.object(Path, "write_text"),
            patch.object(self.entry.os, "replace"),
        ):
            self.assertEqual(
                self.entry.main(["entry", "/tmp/agent.pid.json", "/tmp/out", "/tmp/err", "/usr/bin/env", "true"]),
                0,
            )

        subreaper.assert_called_once_with()
        self.assertEqual(
            waitpid.call_args_list,
            [
                unittest.mock.call(99, 0),
                unittest.mock.call(-1, 0),
                unittest.mock.call(-1, 0),
            ],
        )


    def test_entrypoint_main_exits_with_group_leader_child_failure(self) -> None:
        with (
            patch.object(Path, "write_text"),
            patch.object(os, "replace"),
            patch.object(os, "fork", return_value=99),
            patch.object(
                os, "waitpid", side_effect=[(99, 3 << 8), ChildProcessError]
            ),
            patch.object(ctypes, "CDLL", return_value=Mock(prctl=Mock(return_value=0))),
            patch.object(sys, "argv", ["entry", "/tmp/agent.pid.json", "/tmp/out", "/tmp/err", "/usr/bin/env", "true"]),
        ):
            with self.assertRaises(SystemExit) as exited:
                runpy.run_path(self.entry.__file__, run_name="__main__")

        self.assertEqual(exited.exception.code, 0)

    def test_entrypoint_is_directly_executable_after_image_chmod(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            installed = Path(directory) / "statefulbench-container-entry"
            installed.write_bytes(Path(self.entry.__file__).read_bytes())
            installed.chmod(0o755)
            completed = subprocess.run(
                [str(installed)],
                capture_output=True,
                text=True,
                check=False,
            )

        self.assertEqual(completed.returncode, 1)
        self.assertIn("usage: statefulbench-container-entry", completed.stderr)




class DockerArmContainerTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls) -> None:
        cls.mod = load_script("statefulbench_docker.py")

    def setUp(self) -> None:
        self.runtime = self.mod.DockerRuntime(
            binary="/usr/local/bin/docker",
            image="statefulbench-realworld:local",
            image_id="sha256:abc",
            repo_digests=("statefulbench@sha256:def",),
            platform="linux/arm64",
        )

    def test_arm_container_has_one_shared_workspace_and_home_without_hidden_mounts(self) -> None:
        command = self.mod.arm_container_command(
            self.runtime,
            name="statefulbench-requests-parallel-on-1",
            workspace=Path("/runs/requests/parallel-on/trial-1/workspace"),
            runtime_dir=Path("/runs/requests/parallel-on/trial-1/runtime"),
        )

        text = " ".join(command)
        self.assertIn("target=/workspace", text)
        self.assertIn("target=/runtime", text)
        self.assertNotIn("datasets/statefulbench-realworld", text)
        self.assertNotIn("docker.sock", text)
        self.assertNotIn("/home/agents", text)
        self.assertIn(self.runtime.image_id, command)
        self.assertEqual(command[-2:], ["sleep", "infinity"])
        self.assertEqual(command[command.index("--ulimit") + 1], "nofile=4096:4096")

    def test_prepare_arm_runtime_initializes_one_shared_home_and_stateful_once(self) -> None:
        container = self.mod.ArmContainer(
            self.runtime,
            "container-id",
            "statefulbench-requests-parallel-on-1",
            Path("/runs/workspace"),
            Path("/runs/runtime"),
        )
        runner = Mock(return_value=subprocess.CompletedProcess([], 0, "", ""))
        credential = Path("/runs/seed/agent.db")

        with (
            patch.object(self.mod, "copy_to_container") as copy,
            patch.object(self.mod.secrets, "token_hex", return_value="runtime-token"),
        ):
            env = self.mod.prepare_arm_runtime(
                container,
                "parallel-on",
                credential_db=credential,
                runner=runner,
            )

        self.assertEqual(env["HOME"], "/home/stateful")
        self.assertEqual(
            env["PI_CODING_AGENT_DIR"], "/home/stateful/.omp/profiles/stateful/agent"
        )
        self.assertEqual(env["STATEFUL_OMP_SANDBOX"], "off")
        self.assertEqual(env["STATEFUL_SERVER_URL"], "http://127.0.0.1:43873")
        self.assertEqual(env["STATEFUL_SERVER_TOKEN"], "runtime-token")
        copy.assert_called_once_with(
            container,
            credential,
            "/home/stateful/.omp/profiles/stateful/agent/agent.db",
            runner=runner,
        )
        commands = [call.args[0] for call in runner.call_args_list]
        self.assertIn(["rm", "-rf", "/home/stateful/.stateful"], [command[-3:] for command in commands])
        self.assertEqual(sum(command[-4:] == ["install", "--agent", "omp", "--yes"] for command in commands), 1)
        self.assertEqual(sum(command[-3:] == ["enable", "--repo", "/workspace"] for command in commands), 1)
        self.assertEqual(
            sum(
                command[-6:]
                == [
                    "server",
                    "start",
                    "--coordination-mode",
                    "awareness",
                    "--token",
                    "runtime-token",
                ]
                for command in commands
            ),
            1,
        )

    def test_start_rejects_empty_container_id_and_remove_fails_closed(self) -> None:
        with self.assertRaisesRegex(RuntimeError, "container id"):
            self.mod.start_arm_container(
                self.runtime,
                "statefulbench-requests-parallel-off-1",
                Path("/runs/workspace"),
                Path("/runs/runtime"),
                runner=Mock(return_value=subprocess.CompletedProcess([], 0, "\n", "")),
            )

        container = self.mod.ArmContainer(
            self.runtime,
            "container-id",
            "statefulbench-requests-parallel-off-1",
            Path("/runs/workspace"),
            Path("/runs/runtime"),
        )
        with self.assertRaisesRegex(RuntimeError, "removal failed"):
            self.mod.remove_arm_container(
                container,
                runner=Mock(return_value=subprocess.CompletedProcess([], 1, "", "still running")),
            )

    def test_start_failure_removes_only_a_token_owned_indeterminate_container(self) -> None:
        runner = Mock(
            side_effect=[
                subprocess.CompletedProcess([], 1, "", "daemon disconnected"),
                subprocess.CompletedProcess([], 0, "orphan-container-id owned-token\n", ""),
                subprocess.CompletedProcess([], 0, "", ""),
            ]
        )

        with (
            patch.object(self.mod.secrets, "token_hex", return_value="owned-token"),
            self.assertRaisesRegex(RuntimeError, "daemon disconnected"),
        ):
            self.mod.start_arm_container(
                self.runtime,
                "statefulbench-requests-parallel-off-1",
                Path("/runs/workspace"),
                Path("/runs/runtime"),
                runner=runner,
            )

        self.assertEqual(
            runner.call_args_list[2].args[0],
            [
                "/usr/local/bin/docker",
                "rm",
                "-f",
                "orphan-container-id",
            ],
        )


class DockerAgentLifecycleTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls) -> None:
        cls.mod = load_script("statefulbench_docker.py")

    def setUp(self) -> None:
        self.tempdir = tempfile.TemporaryDirectory()
        self.root = Path(self.tempdir.name)
        runtime = self.mod.DockerRuntime(
            binary="/usr/local/bin/docker",
            image="statefulbench-realworld:local",
            image_id="sha256:abc",
            repo_digests=(),
            platform="linux/arm64",
        )
        self.container = self.mod.ArmContainer(
            runtime,
            "container-1",
            "statefulbench-fixture-parallel-off-1",
            self.root / "workspace",
            self.root / "runtime",
        )
        self.container.runtime_dir.mkdir()
        self.prompt = self.root / "task.prompt.txt"
        self.prompt.write_text("implement task\n", encoding="utf-8")
        self.cfg = type(
            "Config",
            (),
            {
                "model": "model",
                "thinking": "high",
                "omp_bin": "/usr/local/bin/omp",
                "timeout_s": 1,
            },
        )()
        self.env = {"HOME": "/home/stateful", "STATEFUL_HOME": "/home/stateful/.stateful"}

    def tearDown(self) -> None:
        self.tempdir.cleanup()

    def test_all_agents_use_same_home_and_workspace(self) -> None:
        commands = []

        def popen(command, **_kwargs):
            commands.append(command)
            return Mock(wait=Mock(return_value=0), returncode=0)
        copy = Mock()
        handles = [
            self.mod.launch_agent(
                self.container,
                self.root,
                agent_id,
                self.prompt,
                self.cfg,
                self.env,
                popen=popen,
                copy=copy,
            )
            for agent_id in ("task-a", "task-b", "final")
        ]

        for command in commands:
            self.assertIn("HOME=/home/stateful", command)
            self.assertIn("-w", command)
            self.assertIn("/workspace", command)
            self.assertNotIn("-d", command)
            self.assertIn("statefulbench-container-entry", " ".join(command))
        self.assertEqual({handle.container_id for handle in handles}, {"container-1"})

    def test_terminate_escalates_term_to_kill_and_requires_inner_death(self) -> None:
        pid_record = self.container.runtime_dir / "pids" / "task-a.json"
        pid_record.parent.mkdir()
        pid_record.write_text('{"pid": 42, "pgid": 42}\n', encoding="utf-8")
        handle = self.mod.DockerAgentHandle(
            Mock(wait=Mock(return_value=0), returncode=0),
            "task-a",
            self.container.container_id,
            pid_record,
            0.0,
        )
        runner = Mock(
            side_effect=[
                subprocess.CompletedProcess([], 0, "", ""),
                subprocess.CompletedProcess([], 0, "", ""),
                subprocess.CompletedProcess([], 0, "", ""),
                subprocess.CompletedProcess([], 1, "", ""),
                subprocess.CompletedProcess([], 1, "", ""),
            ]
        )

        cleanup_error = self.mod.terminate_agent_group(
            self.container, handle, runner=runner, wait_s=0
        )

        self.assertIsNone(cleanup_error)
        commands = [call.args[0] for call in runner.call_args_list]
        self.assertIn(
            ["/usr/local/bin/docker", "exec", "--workdir", "/workspace", "container-1", "kill", "-TERM", "-42"],
            commands,
        )
        self.assertIn(
            ["/usr/local/bin/docker", "exec", "--workdir", "/workspace", "container-1", "kill", "-KILL", "-42"],
            commands,
        )
    def test_terminate_records_signal_error_and_still_removes_arm(self) -> None:
        pid_record = self.container.runtime_dir / "pids" / "task-a.json"
        pid_record.parent.mkdir()
        pid_record.write_text('{"pid": 42, "pgid": 42}\n', encoding="utf-8")
        handle = self.mod.DockerAgentHandle(
            Mock(wait=Mock(return_value=0), returncode=0),
            "task-a",
            self.container.container_id,
            pid_record,
            0.0,
        )
        remove = Mock()

        cleanup_error = self.mod.terminate_agent_group(
            self.container,
            handle,
            runner=Mock(side_effect=OSError("docker exec failed")),
            remove=remove,
            force_container_removal=True,
        )

        self.assertIn("TERM failed", cleanup_error)
        self.assertTrue(handle.inner_term_attempted)
        self.assertEqual(handle.inner_term_error, "docker exec failed")
        self.assertTrue(handle.container_removal_succeeded)
        remove.assert_called_once()
    def test_wait_records_client_termination_error_then_removes_arm(self) -> None:
        pid_record = self.container.runtime_dir / "pids" / "task-a.json"
        pid_record.parent.mkdir()
        pid_record.write_text('{"pid": 42, "pgid": 42}\n', encoding="utf-8")
        process = Mock(returncode=None)
        process.wait.side_effect = [subprocess.TimeoutExpired("docker", 1), 0]
        process.terminate.side_effect = OSError("client termination failed")
        process.communicate.side_effect = [subprocess.TimeoutExpired("docker", 1), (b"", b"")]
        handle = self.mod.DockerAgentHandle(
            process, "task-a", self.container.container_id, pid_record, 0.0
        )

        with patch.object(self.mod, "remove_arm_container") as remove:
            record, _ = self.mod.wait_agent(
                self.container,
                handle,
                self.root,
                "task",
                self.cfg,
                runner=Mock(return_value=subprocess.CompletedProcess([], 1, "", "")),
            )

        self.assertIn("client termination failed", record["cleanup"]["client"]["term_result"])
        self.assertTrue(record["cleanup"]["container_removal"]["succeeded"])
        remove.assert_called_once()


    def test_wait_uses_entry_client_completion_not_agent_writable_exit_record(self) -> None:
        pid_record = self.container.runtime_dir / "pids" / "task-a.json"
        pid_record.parent.mkdir()
        pid_record.write_text('{"pid": 42, "pgid": 42}\n', encoding="utf-8")
        exit_record = pid_record.with_suffix(".exit")
        exit_record.write_text("0\n", encoding="utf-8")
        pid_record.with_suffix(".completion.json").write_text(
            '{"pid": 42, "pgid": 42, "exit_code": 0}\n', encoding="utf-8"
        )
        handle = self.mod.DockerAgentHandle(
            Mock(wait=Mock(return_value=0), returncode=0),
            "task-a",
            self.container.container_id,
            pid_record,
            0.0,
        )
        handle.popen.communicate.return_value = (b'{"pid":42,"pgid":42,"exit_code":0}\n', b"")
        runner = Mock()

        record, _ = self.mod.wait_agent(
            self.container, handle, self.root, "task", self.cfg, runner=runner
        )

        self.assertEqual(record["exit_code"], 0)
        self.assertIsNone(record["cleanup_error"])
        self.assertEqual((record["pid"], record["pgid"]), (42, 42))
        runner.assert_not_called()
        handle.popen.stdout.close.assert_called_once_with()
        handle.popen.stderr.close.assert_called_once_with()
    def test_wait_removes_arm_when_docker_exec_exits_nonzero(self) -> None:
        pid_record = self.container.runtime_dir / "pids" / "task-a.json"
        pid_record.parent.mkdir()
        pid_record.write_text('{"pid": 42, "pgid": 42}\n', encoding="utf-8")
        handle = self.mod.DockerAgentHandle(
            Mock(wait=Mock(return_value=125), returncode=125),
            "task-a",
            self.container.container_id,
            pid_record,
            0.0,
        )
        handle.popen.communicate.return_value = (b"", b"")

        with patch.object(self.mod, "remove_arm_container") as remove:
            record, _ = self.mod.wait_agent(
                self.container,
                handle,
                self.root,
                "task",
                self.cfg,
                runner=Mock(return_value=subprocess.CompletedProcess([], 1, "", "")),
            )

        self.assertEqual(record["exit_code"], -1)
        self.assertIn("docker exec exited 125", record["cleanup_error"])
        self.assertTrue(record["container_removed"])
        remove.assert_called_once()

    def test_wait_removes_arm_on_malformed_wrapper_completion_bytes(self) -> None:
        pid_record = self.container.runtime_dir / "pids" / "task-a.json"
        pid_record.parent.mkdir()
        pid_record.write_text('{"pid": 42, "pgid": 42}\n', encoding="utf-8")
        handle = self.mod.DockerAgentHandle(
            Mock(wait=Mock(return_value=0), returncode=0),
            "task-a",
            self.container.container_id,
            pid_record,
            0.0,
        )
        handle.popen.communicate.return_value = (b"\xff", b"")

        with patch.object(self.mod, "remove_arm_container") as remove:
            record, _ = self.mod.wait_agent(
                self.container,
                handle,
                self.root,
                "task",
                self.cfg,
                runner=Mock(return_value=subprocess.CompletedProcess([], 1, "", "")),
            )

        self.assertEqual(record["exit_code"], -1)
        self.assertIn("missing or invalid trusted completion", record["cleanup_error"])
        remove.assert_called_once()

    def test_wait_preserves_nonzero_agent_exit_from_trusted_completion(self) -> None:
        pid_record = self.container.runtime_dir / "pids" / "task-a.json"
        pid_record.parent.mkdir()
        pid_record.write_text('{"pid": 42, "pgid": 42}\n', encoding="utf-8")
        pid_record.with_suffix(".completion.json").write_text(
            '{"pid": 42, "pgid": 42, "exit_code": 3}\n', encoding="utf-8"
        )
        handle = self.mod.DockerAgentHandle(
            Mock(wait=Mock(return_value=0), returncode=0),
            "task-a",
            self.container.container_id,
            pid_record,
            0.0,
        )
        handle.popen.communicate.return_value = (b'{"pid":42,"pgid":42,"exit_code":3}\n', b"")
        runner = Mock()

        record, _ = self.mod.wait_agent(
            self.container, handle, self.root, "task", self.cfg, runner=runner
        )

        self.assertEqual(record["exit_code"], 3)
        self.assertIsNone(record["cleanup_error"])
        runner.assert_not_called()

    def test_wait_removes_arm_when_subreaper_never_reaps_escaped_descendant(self) -> None:
        pid_record = self.container.runtime_dir / "pids" / "task-a.json"
        pid_record.parent.mkdir()
        pid_record.write_text('{"pid": 42, "pgid": 42}\n', encoding="utf-8")
        process = Mock(returncode=None)
        process.wait.side_effect = [subprocess.TimeoutExpired("docker", 1), 0]
        process.communicate.side_effect = [subprocess.TimeoutExpired("docker", 1), (b"", b"")]
        handle = self.mod.DockerAgentHandle(
            process,
            "task-a",
            self.container.container_id,
            pid_record,
            0.0,
        )

        with patch.object(self.mod, "remove_arm_container") as remove:
            record, _ = self.mod.wait_agent(
                self.container,
                handle,
                self.root,
                "task",
                self.cfg,
                runner=Mock(return_value=subprocess.CompletedProcess([], 1, "", "")),
            )

        remove.assert_called_once_with(self.container, runner=unittest.mock.ANY)
        self.assertTrue(record["timed_out"])
        self.assertTrue(record["container_removed"])
        self.assertEqual((record["pid"], record["pgid"]), (42, 42))
        self.assertTrue(record["client_timed_out"])
        self.assertTrue(record["cleanup"]["client"]["term_attempted"])
        self.assertTrue(record["cleanup"]["inner"]["term_attempted"])
        self.assertTrue(record["cleanup"]["inner"]["kill_attempted"])
        self.assertEqual(
            record["cleanup"]["container_removal"],
            {"attempted": True, "succeeded": True},
        )


    def test_wait_reaps_a_timed_out_detached_docker_client(self) -> None:
        pid_record = self.container.runtime_dir / "pids" / "task-a.json"
        pid_record.parent.mkdir()
        pid_record.write_text('{"pid": 42, "pgid": 42}\n', encoding="utf-8")
        process = Mock(returncode=None)
        process.wait.side_effect = [subprocess.TimeoutExpired("docker", 5), 0]
        process.communicate.side_effect = [subprocess.TimeoutExpired("docker", 5), (b"", b"")]
        handle = self.mod.DockerAgentHandle(
            process, "task-a", self.container.container_id, pid_record, time.monotonic()
        )

        record, _ = self.mod.wait_agent(
            self.container,
            handle,
            self.root,
            "task",
            self.cfg,
            runner=Mock(return_value=subprocess.CompletedProcess([], 1, "", "")),
        )

        self.assertTrue(record["timed_out"])
        process.terminate.assert_called_once_with()
        self.assertEqual(process.wait.call_count, 2)
        self.assertTrue(record["client_timed_out"])
        self.assertEqual(record["cleanup"]["client"]["term_result"], "sent")
        self.assertTrue(record["cleanup"]["container_removal"]["attempted"])

    def test_wait_does_not_poll_agent_writable_exit_record(self) -> None:
        pid_record = self.container.runtime_dir / "pids" / "task-a.json"
        pid_record.parent.mkdir()
        pid_record.write_text('{"pid": 42, "pgid": 42}\n', encoding="utf-8")
        exit_record = pid_record.with_suffix(".exit")
        pid_record.with_suffix(".completion.json").write_text(
            '{"pid": 42, "pgid": 42, "exit_code": 0}\n', encoding="utf-8"
        )
        handle = self.mod.DockerAgentHandle(
            Mock(wait=Mock(return_value=0), returncode=0),
            "task-a",
            self.container.container_id,
            pid_record,
            time.monotonic(),
        )
        handle.popen.communicate.return_value = (b'{"pid":42,"pgid":42,"exit_code":0}\n', b"")

        record, _ = self.mod.wait_agent(
            self.container,
            handle,
            self.root,
            "task",
            self.cfg,
            runner=Mock(),
        )

        self.assertEqual(record["exit_code"], 0)
        self.assertIsNone(record["cleanup_error"])

class DockerQualificationTests(unittest.TestCase):
    @classmethod

    def setUpClass(cls) -> None:
        cls.mod = load_script("statefulbench_docker.py")

    def setUp(self) -> None:
        self.runtime = self.mod.DockerRuntime(
            binary="/usr/local/bin/docker",
            image="statefulbench-realworld:local",
            image_id="sha256:abc",
            repo_digests=("statefulbench@sha256:def",),
            platform="linux/arm64",
        )

    def test_qualification_command_mounts_repo_read_only_and_cache_artifacts_rw(self) -> None:
        command = self.mod.qualification_command(
            runtime=self.runtime,
            repo_root=Path("/repo"),
            manifest=Path("/repo/datasets/statefulbench-realworld/manifest.json"),
            cache=Path("/runs/cache"),
            repositories=("requests",),
        )

        self.assertIn("type=bind,source=/repo,target=/benchmark,readonly", command)
        self.assertIn("type=bind,source=/runs/cache,target=/cache", command)
        self.assertNotIn("/Users/arthur", " ".join(command))
        self.assertIn("STATEFULBENCH_DOCKER_INNER=qualification", command)
        self.assertEqual(command.count("--repo"), 1)
        self.assertEqual(command[command.index("python3") - 1], self.runtime.image_id)
        self.assertEqual(command[command.index("--ulimit") + 1], "nofile=4096:4096")

    def test_qualification_command_rejects_unsafe_mount_boundaries(self) -> None:
        with self.assertRaisesRegex(ValueError, "manifest"):
            self.mod.qualification_command(
                self.runtime,
                Path("/repo"),
                Path("/outside/manifest.json"),
                Path("/runs/cache"),
                (),
            )
        with self.assertRaisesRegex(ValueError, "cache"):
            self.mod.qualification_command(
                self.runtime,
                Path("/repo"),
                Path("/repo/cache"),
                Path("/repo/cache"),
                (),
            )

    def test_run_qualification_container_propagates_docker_status(self) -> None:
        completed = subprocess.CompletedProcess([], 23, "", "failed")
        status = self.mod.run_qualification_container(
            self.runtime,
            Path("/repo"),
            Path("/repo/manifest.json"),
            Path("/runs/cache"),
            ("requests",),
            runner=Mock(return_value=completed),
        )

        self.assertEqual(status, 23)


class DockerDiagnosticTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls) -> None:
        cls.diagnostics = load_script("statefulbench_container_diagnostics.py")
        cls.mod = load_script("statefulbench_docker.py")

    def setUp(self) -> None:
        self.tempdir = tempfile.TemporaryDirectory()
        self.home = Path(self.tempdir.name) / "home"
        self.home.mkdir()

    def tearDown(self) -> None:
        self.tempdir.cleanup()
    def test_snapshot_counts_exact_context_render_markers(self) -> None:
        log = self.home / ".stateful" / "runtime" / "server.log"
        log.parent.mkdir(parents=True)
        log.write_text(
            "startup\n"
            "[stateful-metric] context_render_success\n"
            "not [stateful-metric] context_render_success extra\n"
            "[stateful-metric] context_render_success\n",
            encoding="utf-8",
        )

        snapshot = self.diagnostics.snapshot_home(self.home)

        self.assertEqual(
            snapshot["runtime_metrics"]["context_render_success_count"], 2
        )

    def test_context_render_markers_reject_changing_log(self) -> None:
        log = self.home / ".stateful" / "runtime" / "server.log"
        log.parent.mkdir(parents=True)
        log.write_text("[stateful-metric] context_render_success\n", encoding="utf-8")
        expected = log.lstat()
        original = self.diagnostics._regular_descriptor

        def change_before_open(path, expected_metadata):
            log.write_text("changed\n", encoding="utf-8")
            return original(path, expected_metadata)

        with patch.object(
            self.diagnostics, "_regular_descriptor", side_effect=change_before_open
        ):
            with self.assertRaisesRegex(OSError, "changed after lstat"):
                self.diagnostics._count_context_render_markers(log, expected)

    def test_context_render_markers_reject_symlinked_parent(self) -> None:
        external_stateful = Path(self.tempdir.name) / "external-stateful"
        external_log = external_stateful / "runtime" / "server.log"
        external_log.parent.mkdir(parents=True)
        external_log.write_text(
            "[stateful-metric] context_render_success\n", encoding="utf-8"
        )
        (self.home / ".stateful").symlink_to(
            external_stateful, target_is_directory=True
        )

        with self.assertRaises(OSError):
            self.diagnostics.snapshot_home(self.home)

    def test_snapshot_redacts_contents_and_reports_safe_sqlite_metadata(self) -> None:
        database = self.home / "agent.db"
        import sqlite3

        with sqlite3.connect(database) as connection:
            connection.execute("create table safe_items (id integer)")
            connection.execute("insert into safe_items values (1)")
        connection.close()
        (self.home / "agent.db-wal").write_text("secret-token-value", encoding="utf-8")
        (self.home / "broken.db").write_text("not sqlite", encoding="utf-8")
        (self.home / "token.txt").write_text("secret-token-value", encoding="utf-8")

        snapshot = self.diagnostics.snapshot_home(self.home)
        encoded = json.dumps(snapshot)

        self.assertNotIn("secret-token-value", encoded)
        self.assertIn("agent.db", encoded)
        self.assertEqual(snapshot["databases"]["agent.db"]["integrity"], "ok")
        self.assertNotIn("rows", snapshot["databases"]["agent.db"])
        self.assertEqual(snapshot["databases"]["broken.db"]["integrity"], "malformed")
        self.assertEqual(snapshot["lock_files"], ["agent.db-wal"])

    def test_snapshot_opens_percent_encoded_database_by_uri(self) -> None:
        import sqlite3

        database = self.home / "agent%2Fstate.db"
        with sqlite3.connect(database) as connection:
            connection.execute("create table safe_items (id integer)")
        connection.close()

        snapshot = self.diagnostics.snapshot_home(self.home)

        self.assertEqual(snapshot["databases"]["agent%2Fstate.db"]["integrity"], "ok")

    def test_snapshot_includes_committed_wal_database_contents(self) -> None:
        import sqlite3

        database = self.home / "agent.db"
        connection = sqlite3.connect(database)
        try:
            self.assertEqual(
                connection.execute("pragma journal_mode = wal").fetchone(),
                ("wal",),
            )
            connection.execute("create table safe_items (id integer)")
            connection.execute("insert into safe_items values (1)")
            connection.commit()
            self.assertTrue((self.home / "agent.db-wal").is_file())

            snapshot = self.diagnostics.snapshot_home(self.home)
        finally:
            connection.close()

        self.assertEqual(snapshot["databases"]["agent.db"]["integrity"], "ok")
        self.assertEqual(snapshot["databases"]["agent.db"]["schemas"], ["safe_items"])
        self.assertEqual(snapshot["databases"]["agent.db"]["table_counts"], {"safe_items": 1})

    def test_snapshot_fails_when_private_sqlite_copy_cannot_be_removed(self) -> None:
        import sqlite3

        database = self.home / "agent.db"
        with sqlite3.connect(database) as connection:
            connection.execute("create table safe_items (id integer)")
        connection.close()

        def cleanup(_path, *, ignore_errors=False):
            if not ignore_errors:
                raise OSError("private SQLite copy cleanup failed")

        with patch.object(self.diagnostics.shutil, "rmtree", side_effect=cleanup):
            with self.assertRaisesRegex(OSError, "private SQLite copy cleanup failed"):
                self.diagnostics.snapshot_home(self.home)

    def test_snapshot_rejects_unsafe_sqlite_sidecar(self) -> None:
        import sqlite3

        database = self.home / "agent.db"
        outside = Path(self.tempdir.name) / "outside"
        with sqlite3.connect(database) as connection:
            connection.execute("create table safe_items (id integer)")
        connection.close()
        outside.write_text("untrusted", encoding="utf-8")
        (self.home / "agent.db-wal").symlink_to(outside)

        snapshot = self.diagnostics.snapshot_home(self.home)

        self.assertEqual(snapshot["databases"]["agent.db"]["integrity"], "unavailable")

    def test_snapshot_does_not_reopen_database_after_symlink_swap(self) -> None:
        import sqlite3

        database = self.home / "agent.db"
        outside = Path(self.tempdir.name) / "outside.db"
        with sqlite3.connect(database) as connection:
            connection.execute("create table local_data (id integer)")
        connection.close()
        with sqlite3.connect(outside) as connection:
            connection.execute("create table external_data (id integer)")
        connection.close()
        original = self.diagnostics._file_record

        def swap(home, path):
            record = original(home, path)
            if path.name == "agent.db":
                path.unlink()
                path.symlink_to(outside)
            return record

        with patch.object(self.diagnostics, "_file_record", side_effect=swap):
            snapshot = self.diagnostics.snapshot_home(self.home)

        self.assertEqual(snapshot["databases"]["agent.db"]["integrity"], "unavailable")

    def test_snapshot_does_not_open_database_fifo_after_swap(self) -> None:
        import sqlite3

        database = self.home / "agent.db"
        with sqlite3.connect(database) as connection:
            connection.execute("create table local_data (id integer)")
        connection.close()
        original = self.diagnostics._file_record

        def swap(home, path):
            record = original(home, path)
            if path.name == "agent.db":
                path.unlink()
                os.mkfifo(path)
            return record

        with (
            patch.object(self.diagnostics, "_file_record", side_effect=swap),
            patch.object(
                self.diagnostics.sqlite3,
                "connect",
                side_effect=AssertionError("must not open swapped FIFO"),
            ),
        ):
            snapshot = self.diagnostics.snapshot_home(self.home)

        self.assertEqual(snapshot["databases"]["agent.db"]["integrity"], "unavailable")

    def test_diff_and_runtime_classification_fail_closed(self) -> None:
        (self.home / "before.txt").write_text("one", encoding="utf-8")
        before = self.diagnostics.snapshot_home(self.home)
        (self.home / "before.txt").write_text("two", encoding="utf-8")
        (self.home / "created.txt").write_text("three", encoding="utf-8")
        after = self.diagnostics.snapshot_home(self.home)

        self.assertEqual(
            self.diagnostics.snapshot_changes(before, after),
            [{"path": "before.txt", "change": "changed"}, {"path": "created.txt", "change": "created"}],
        )
        self.assertEqual(
            self.diagnostics.classify_runtime_failure("database is locked"),
            "sqlite_locked",
        )
        self.assertEqual(
            self.diagnostics.classify_runtime_failure("database is busy"),
            "sqlite_locked",
        )
        self.assertEqual(
            self.diagnostics.classify_runtime_failure("not a database"),
            "sqlite_malformed",
        )
        self.assertEqual(
            self.diagnostics.classify_runtime_failure("unexpected failure"),
            "unclassified_runtime_failure",
        )
        self.assertEqual(
            self.diagnostics.classify_runtime_failure("sqlite_unavailable"),
            "sqlite_unavailable",
        )

    def test_snapshot_changes_include_nonregular_and_type_replacements(self) -> None:
        (self.home / "entry").mkdir()
        (self.home / "removed").mkdir()
        before = self.diagnostics.snapshot_home(self.home)
        (self.home / "entry").rmdir()
        (self.home / "entry").write_text("now regular", encoding="utf-8")
        (self.home / "removed").rmdir()
        os.mkfifo(self.home / "stream")
        after = self.diagnostics.snapshot_home(self.home)

        self.assertEqual(
            self.diagnostics.snapshot_changes(before, after),
            [
                {"path": "entry", "change": "changed"},
                {"path": "removed", "change": "deleted"},
                {"path": "stream", "change": "created"},
            ],
        )

    def test_snapshot_never_dereferences_symlinks_or_hashes_fifos(self) -> None:
        import sqlite3

        outside = Path(self.tempdir.name) / "outside.db"
        with sqlite3.connect(outside) as connection:
            connection.execute("create table external_data (id integer)")
        connection.close()
        (self.home / "outside.db").symlink_to(outside)
        os.mkfifo(self.home / "stream.db")

        snapshot = self.diagnostics.snapshot_home(self.home)
        files = {item["path"]: item for item in snapshot["files"]}

        self.assertNotIn("outside.db", snapshot["databases"])
        self.assertNotIn("stream.db", snapshot["databases"])
        self.assertEqual(files["outside.db"]["type"], "symlink")
        self.assertNotIn("sha256", files["outside.db"])
        self.assertEqual(files["stream.db"]["type"], "fifo")
        self.assertNotIn("sha256", files["stream.db"])
        self.assertEqual(
            self.diagnostics.classify_runtime_failure(
                None, {"databases": {"agent.db": {"integrity": "unavailable"}}}
            ),
            "sqlite_unavailable",
        )

    def test_capture_rejects_escaped_host_workspace_path_leaks(self) -> None:
        runtime = self.mod.DockerRuntime(
            binary="/docker",
            image="fixture",
            image_id="sha256:fixture",
            repo_digests=(),
            platform="linux/arm64",
        )
        container = self.mod.ArmContainer(
            runtime,
            "container-1",
            "arm",
            self.home / "workspace",
            self.home / "runtime",
        )
        output = self.home / "runtime" / "diagnostics" / "initialized.json"
        output.parent.mkdir(parents=True)
        snapshot = {
            "schema_version": 1,
            "phase": "initialized",
            "home": "/home/stateful",
            "files": [],
            "databases": {},
            "lock_files": [],
            "processes": [],
            "per_agent_home_tree": False,
            "workspace_hint": str(container.workspace.resolve()),
        }

        def emit(*_args, **_kwargs):
            output.write_text(
                json.dumps(snapshot).replace("/", "\\u002f"),
                encoding="utf-8",
            )
            return subprocess.CompletedProcess([], 0, "", "")
        with patch.object(self.mod, "exec_in_container", side_effect=emit):
            with self.assertRaisesRegex(RuntimeError, "leaked host path"):
                self.mod.capture_home_snapshot(container, "initialized")


    def test_inspect_summary_redacts_container_state(self) -> None:
        runtime = self.mod.DockerRuntime(
            binary="/docker",
            image="fixture",
            image_id="sha256:fixture",
            repo_digests=(),
            platform="linux/arm64",
        )
        container = self.mod.ArmContainer(
            runtime,
            "container-1",
            "arm",
            self.home / "workspace",
            self.home / "runtime",
        )
        completed = subprocess.CompletedProcess(
            [],
            0,
            json.dumps(
                {
                    "Status": "running",
                    "Pid": 42,
                    "StartedAt": "2026-07-13T00:00:00Z",
                    "FinishedAt": "",
                    "Error": "secret-token-value",
                }
            ),
            "",
        )

        summary = self.mod.inspect_arm_container(
            container,
            runner=Mock(return_value=completed),
        )

        self.assertEqual(
            summary,
            {
                "id": "container-1",
                "image_id": "sha256:fixture",
                "state": {
                    "status": "running",
                    "pid": 42,
                    "started_at": "2026-07-13T00:00:00Z",
                    "finished_at": "",
                },
            },
        )



@unittest.skipUnless(
    os.environ.get("STATEFULBENCH_DOCKER_TEST_IMAGE"),
    "set STATEFULBENCH_DOCKER_TEST_IMAGE to run Docker end-to-end tests",
)
class DockerEndToEndTests(unittest.TestCase):
    """Credit-free proof that the live Docker arm path shares one HOME/workspace."""

    def setUp(self) -> None:
        self.tempdir = tempfile.TemporaryDirectory()
        self.root = Path(self.tempdir.name)
        self.docker = load_script("statefulbench_docker.py")
        self.realworld = load_script("statefulbench_realworld.py")
        self.image = os.environ["STATEFULBENCH_DOCKER_TEST_IMAGE"]
        self.dataset = self.root / "dataset"
        self.archive = self._write_fixture()
        self.runtime = self.docker.inspect_runtime("docker", self.image)
        self.repo = {
            "key": "docker-e2e",
            "archive_sha256": hashlib.sha256(self.archive.read_bytes()).hexdigest(),
            "setup": ["python", "-c", "pass"],
            "suite": ["python", "suite.py"],
            "environment": {"STATEFULBENCH_FAKE_EXIT": "0"},
        }
        self.corpus = {
            "repository": "docker-e2e",
            "final_prompt": "verify the two fixture edits",
            "tasks": [
                {
                    "key": key,
                    "prompt": f"write {key}.txt",
                    "evaluator": f"evaluators/{key}.py",
                }
                for key in ("alpha", "beta")
            ],
        }
        for key in ("alpha", "beta"):
            evaluator = self.dataset / "evaluators" / f"{key}.py"
            evaluator.parent.mkdir(parents=True, exist_ok=True)
            evaluator.write_text(
                "from pathlib import Path\n"
                "import sys\n"
                f"assert (Path(sys.argv[1]) / '{key}.txt').read_text() == '{key}\\n'\n",
                encoding="utf-8",
            )

    def tearDown(self) -> None:
        self.tempdir.cleanup()

    def _write_fixture(self) -> Path:
        source = self.root / "source"
        source.mkdir()
        (source / "fake-omp").write_text(
            textwrap.dedent(
                """\
                #!/usr/bin/env python3
                import json
                import os
                import sys
                import time
                from pathlib import Path

                if "--version" in sys.argv:
                    print("fake-omp 1")
                    raise SystemExit(0)
                prompt = next(argument[1:] for argument in sys.argv if argument.startswith("@"))
                agent_id = Path(prompt).name.removesuffix(".prompt.txt")
                home = Path(os.environ["HOME"])
                workspace = Path.cwd()

                def append(path, value):
                    descriptor = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_APPEND, 0o644)
                    try:
                        os.write(descriptor, (value + "\\n").encode())
                    finally:
                        os.close(descriptor)

                home.mkdir(parents=True, exist_ok=True)
                shared = home / "shared-agent-log.txt"
                (home / "secret-token-value.txt").write_text("secret-token-value")
                append(shared, agent_id)
                append(
                    workspace / "agent-observations.jsonl",
                    json.dumps({"agent_id": agent_id, "home": str(home), "pwd": str(workspace)}),
                )
                started = time.monotonic_ns()
                if agent_id != "final":
                    append(home / "ready-agents.txt", agent_id)
                    if os.environ.get("STATEFULBENCH_FAKE_PARALLEL") == "1":
                        while len(shared.read_text().splitlines()) < 2:
                            time.sleep(0.01)
                    (workspace / f"{agent_id}.txt").write_text(f"{agent_id}\\n")
                else:
                    (workspace / "final.txt").write_text("final\\n")
                time.sleep(0.15)
                append(
                    workspace / "agent-events.jsonl",
                    json.dumps(
                        {
                            "agent_id": agent_id,
                            "start": started,
                            "end": time.monotonic_ns(),
                            "shared_ids": shared.read_text().splitlines(),
                        }
                    ),
                )
                print(json.dumps({"message": {"usage": {"totalTokens": 7, "toolCalls": 1}}}))
                raise SystemExit(int(os.environ.get("STATEFULBENCH_FAKE_EXIT", "0")))
                """
            ),
            encoding="utf-8",
        )
        (source / "fake-stateful").write_text(
            "#!/bin/sh\nprintf '%s\\n' \"$*\" >> /workspace/stateful-invocations.log\n"
            "exec /usr/local/bin/stateful \"$@\"\n",
            encoding="utf-8",
        )
        (source / "suite.py").write_text(
            "from pathlib import Path\n"
            "assert Path('alpha.txt').read_text() == 'alpha\\n'\n"
            "assert Path('beta.txt').read_text() == 'beta\\n'\n",
            encoding="utf-8",
        )
        for executable in ("fake-omp", "fake-stateful"):
            (source / executable).chmod(0o755)
        archive = self.root / "fixture.tar.gz"
        with tarfile.open(archive, "w:gz") as output:
            output.add(source, arcname="fixture")
        return archive

    def _run_arm(self, arm: str) -> dict:
        repo = {
            **self.repo,
            "environment": {
                **self.repo["environment"],
                "STATEFULBENCH_FAKE_PARALLEL": "0" if arm == "sequential" else "1",
            },
        }
        return self.realworld.run_repo_arm(
            repo,
            self.corpus,
            self.dataset,
            self.root / "cache",
            self.root / "out",
            arm,
            self.realworld.RunConfig(
                tasks=2,
                timeout_s=30,
                omp_bin="/workspace/fake-omp",
                stateful_binary="/workspace/fake-stateful",
            ),
            runtime=self.runtime,
            archive_loader=lambda *_: self.archive,
        )

    def test_all_arms_share_home_grade_and_cleanup(self) -> None:
        results = {arm: self._run_arm(arm) for arm in ("sequential", "parallel-off", "parallel-on")}

        self.assertTrue(all(result["cleared"] for result in results.values()), results)
        for arm, result in results.items():
            with self.subTest(arm=arm):
                self.assertTrue(result["cleared"], result)
                self.assertTrue(result["post_suite_ok"], result)
                self.assertTrue(result["evaluators_ok"], result)
                self.assertTrue(result["upstream_suite_ok"], result)
                self.assertEqual(result["total_tokens"], 21)
                self.assertEqual(result["total_tool_calls"], 3)
                self.assertTrue(result["container"]["removed"])
                self.assertTrue(all(record["exit_code"] == 0 for record in result["agents"]))
                workspace = self.root / "out" / "docker-e2e" / arm / "trial-1" / "workspace"
                observations = [
                    json.loads(line)
                    for line in (workspace / "agent-observations.jsonl").read_text().splitlines()
                ]
                self.assertEqual({record["agent_id"] for record in observations}, {"alpha", "beta", "final"})
                self.assertEqual({record["home"] for record in observations}, {"/home/stateful"})
                self.assertEqual({record["pwd"] for record in observations}, {"/workspace"})
                self.assertEqual((workspace / "alpha.txt").read_text(), "alpha\n")
                self.assertEqual((workspace / "beta.txt").read_text(), "beta\n")
                snapshot_path = self.root / "out" / "docker-e2e" / arm / "trial-1" / "runtime" / "diagnostics" / "after-tasks.json"
                snapshot = json.loads(snapshot_path.read_text(encoding="utf-8"))
                self.assertIn("shared-agent-log.txt", {record["path"] for record in snapshot["files"]})
                encoded = json.dumps(snapshot)
                self.assertNotIn("secret-token-value", encoded)
                self.assertNotIn(str(self.root), encoded)
                self.assertTrue(all(not record["path"].startswith("/") for record in snapshot["files"]))

        intervals = {
            arm: {
                item["agent_id"]: item
                for item in (
                    json.loads(line)
                    for line in (
                        self.root / "out" / "docker-e2e" / arm / "trial-1" / "workspace" / "agent-events.jsonl"
                    ).read_text().splitlines()
                )
                if item["agent_id"] != "final"
            }
            for arm in results
        }
        sequential = intervals["sequential"]
        self.assertTrue(
            sequential["alpha"]["end"] <= sequential["beta"]["start"]
            or sequential["beta"]["end"] <= sequential["alpha"]["start"]
        )
        parallel = intervals["parallel-off"]
        self.assertLess(parallel["alpha"]["start"], parallel["beta"]["end"])
        self.assertLess(parallel["beta"]["start"], parallel["alpha"]["end"])
        parallel_on = intervals["parallel-on"]
        self.assertLess(parallel_on["alpha"]["start"], parallel_on["beta"]["end"])
        self.assertLess(parallel_on["beta"]["start"], parallel_on["alpha"]["end"])
        events = {
            arm: {
                item["agent_id"]: item
                for item in (
                    json.loads(line)
                    for line in (
                        self.root / "out" / "docker-e2e" / arm / "trial-1" / "workspace" / "agent-events.jsonl"
                    ).read_text().splitlines()
                )
            }
            for arm in results
        }
        self.assertEqual(events["sequential"]["alpha"]["shared_ids"], ["alpha"])
        self.assertEqual(events["sequential"]["beta"]["shared_ids"], ["alpha", "beta"])
        self.assertEqual(events["sequential"]["final"]["shared_ids"], ["alpha", "beta", "final"])
        for arm in ("parallel-off", "parallel-on"):
            self.assertEqual(set(events[arm]["alpha"]["shared_ids"]), {"alpha", "beta"})
            self.assertEqual(set(events[arm]["beta"]["shared_ids"]), {"alpha", "beta"})
            self.assertEqual(
                set(events[arm]["final"]["shared_ids"]), {"alpha", "beta", "final"}
            )
        invocations = (
            self.root / "out" / "docker-e2e" / "parallel-on" / "trial-1" / "workspace" / "stateful-invocations.log"
        ).read_text().splitlines()
        self.assertEqual(invocations.count("server start --coordination-mode awareness"), 1)
        names = ",".join(f"statefulbench-docker-e2e-{arm}-1" for arm in results)
        completed = subprocess.run(
            ["docker", "ps", "-a", "--format", "{{.Names}}"],
            capture_output=True,
            text=True,
            check=True,
        )
        self.assertFalse(set(completed.stdout.splitlines()) & set(names.split(",")))


class V2DiagnosticContractTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls) -> None:
        cls.diagnostics = load_script("statefulbench_container_diagnostics.py")

    def setUp(self) -> None:
        self.tempdir = tempfile.TemporaryDirectory()
        self.home = Path(self.tempdir.name) / "home"
        self.home.mkdir()

    def tearDown(self) -> None:
        self.tempdir.cleanup()

    def test_v2_snapshot_emits_only_locked_value_free_metrics(self) -> None:
        import sqlite3

        database = self.home / ".stateful" / "state.db"
        database.parent.mkdir()
        connection = sqlite3.connect(database)
        connection.executescript(
            """
            CREATE TABLE journal_events (
                event_seq INTEGER PRIMARY KEY,
                aggregate_id TEXT NOT NULL,
                event_type TEXT NOT NULL,
                occurred_at TEXT NOT NULL,
                payload_json TEXT NOT NULL,
                agent_id TEXT NOT NULL,
                source_ref TEXT NOT NULL
            );
            CREATE TABLE presence_current (payload_json TEXT);
            CREATE TABLE handoff_current (payload_json TEXT);
            CREATE TABLE read_observation_current (payload_json TEXT);
            CREATE TABLE wait_current (payload_json TEXT);
            CREATE TABLE context_delivery_current (payload_json TEXT);
            CREATE TABLE workspace_version (version INTEGER);
            CREATE TABLE notification_current (payload_json TEXT);
            """
        )

        def payload(data: dict) -> str:
            return json.dumps({"event": {"data": {"data": data}}})

        rows = [
            ("agent-one", "presence.registered", payload({})),
            ("agent-one", "presence.finalized", payload({})),
            ("agent-two", "presence.expired", payload({})),
            ("handoff-one", "handoff.finalized", payload({"handoff": {"explicit": True, "status": "customer_secret"}})),
            (
                "handoff-two",
                "handoff.finalized",
                payload(
                    {
                        "handoff": {"explicit": False, "status": "unknown"},
                        "fallback_cause": "stop",
                    }
                ),
            ),
            (
                "handoff-three",
                "handoff.finalized",
                payload(
                    {
                        "handoff": {"explicit": False, "status": "unknown"},
                        "fallback_cause": "stop",
                    }
                ),
            ),
            (
                "handoff-four",
                "handoff.finalized",
                payload(
                    {
                        "handoff": {"explicit": False, "status": "unknown"},
                        "fallback_cause": "ttl",
                    }
                ),
            ),
            ("read-one", "read_observation.started", payload({})),
            ("read-one", "read_observation.stabilized", payload({})),
            ("read-two", "read_observation.unstable", payload({})),
            ("read-three", "read_observation.aborted", payload({})),
            ("read-four", "read_observation.invalidated", payload({})),
            ("context-one", "context.rendered", payload({})),
            (
                "delivery-one",
                "context.delivery_created",
                payload(
                    {
                        "context_delivery": {
                            "prompt_text": "private context message",
                            "items": [{"resource": "private/path.txt"}],
                        }
                    }
                ),
            ),
            ("delivery-one", "context.delivery_acknowledged", payload({})),
            ("delivery-two", "context.delivery_superseded", payload({})),
            (
                "notification-one",
                "notification.created",
                payload({"notification": {"kind": "scope_overlap", "payload": {"path": "private/path.txt"}}}),
            ),
            ("wait-private", "wait.requested", payload({})),
            (
                "notification-two",
                "notification.created",
                payload(
                    {
                        "notification": {
                            "kind": "reservation_granted",
                            "payload": {"wait_id": "wait-private", "message": "private grant"},
                        }
                    }
                ),
            ),
            ("wait-private", "wait.claimed", payload({})),
            ("audit-one", "authorization.warned", payload({"reason_code": "missing_claim", "message": "private warning"})),
            ("audit-conflict", "authorization.warned", payload({"reason_code": "coordination_conflict"})),
            ("audit-two", "authorization.denied", payload({"reason_code": "active_claim_conflict", "path": "private/path.txt"})),
            ("fence-one", "write_fence.conflict_observed", payload({"operation_id": "operation-private"})),
            ("intent-one", "write_intent.outcome_unknown", payload({})),
            ("unknown-event", "customer_secret", payload({})),
            ("audit-three", "authorization.warned", payload({"reason_code": "customer_secret"})),
            ("audit-four", "authorization.denied", payload({"reason_code": "customer_secret"})),
        ]
        connection.executemany(
            "INSERT INTO journal_events VALUES (?, ?, ?, ?, ?, ?, ?)",
            [
                (
                    index,
                    aggregate_id,
                    event_type,
                    f"2026-07-16T00:00:{index:02d}Z",
                    body,
                    "agent-private",
                    "presence.stop"
                    if aggregate_id == "handoff-four"
                    else "presence.expire",
                )
                for index, (aggregate_id, event_type, body) in enumerate(rows, start=1)
            ],
        )
        connection.executemany(
            "INSERT INTO wait_current VALUES (?)",
            [(json.dumps({"status": "claimed"}),), (json.dumps({"status": "customer_secret"}),)],
        )
        connection.execute("INSERT INTO workspace_version VALUES (3)")
        connection.commit()
        connection.close()

        snapshot = self.diagnostics.snapshot_home(self.home)
        metrics = snapshot["databases"][".stateful/state.db"]["coordination_metrics"]

        self.assertEqual(
            set(metrics),
            {
                "protocol_version",
                "journal",
                "presence",
                "handoffs",
                "read_observations",
                "context",
                "authorization",
                "write_safety",
                "notifications",
                "waits",
            },
        )
        self.assertEqual(metrics["protocol_version"], "stateful.v2")
        self.assertEqual(metrics["journal"]["events"], len(rows))
        self.assertEqual(metrics["journal"]["by_event_type"], dict(sorted(metrics["journal"]["by_event_type"].items())))
        self.assertEqual(metrics["presence"], {"registered": 1, "expired": 1, "finalized": 1, "peak_active": 1})
        self.assertEqual(metrics["handoffs"]["explicit"], 1)
        self.assertEqual(metrics["handoffs"]["fallback_stop"], 2)
        self.assertEqual(metrics["handoffs"]["fallback_ttl"], 1)
        self.assertEqual(metrics["read_observations"], {"started": 1, "stable": 1, "unstable": 1, "aborted": 1, "invalidated": 1})
        self.assertEqual(metrics["context"]["versions"], 3)
        self.assertEqual(metrics["context"]["renders"], 1)
        self.assertEqual(metrics["context"]["deliveries"], 1)
        self.assertEqual(metrics["context"]["acks"], 1)
        self.assertEqual(metrics["context"]["redeliveries"], 1)
        self.assertEqual(
            metrics["authorization"],
            {"warned_by_reason": {"coordination_conflict": 1, "missing_claim": 1}, "denied_by_reason": {}},
        )
        self.assertEqual(metrics["write_safety"]["fence_conflicts"], 1)
        self.assertEqual(metrics["write_safety"]["unknown_outcomes"], 1)
        self.assertEqual(metrics["notifications"]["by_kind"], {"reservation_granted": 1, "scope_overlap": 1})
        self.assertEqual(metrics["waits"]["by_final_status"], {"claimed": 1})
        self.assertEqual(metrics["waits"]["grant_wait_time_s"], {"count": 1, "total": 1.0, "mean": 1.0, "max": 1.0})
        encoded = json.dumps(snapshot)
        for private_value in (
            "agent-private",
            "agent-one",
            "private/path.txt",
            "private context message",
            "private grant",
            "private warning",
            "operation-private",
            "2026-07-16T00:00:01Z",
        ):
            self.assertNotIn(private_value, encoded)
        self.assertNotIn("customer_secret", encoded)
    def test_v2_snapshot_excludes_unknown_notification_kind(self) -> None:
        import sqlite3

        database = self.home / ".stateful" / "state.db"
        database.parent.mkdir()
        connection = sqlite3.connect(database)
        connection.executescript(
            """
            CREATE TABLE journal_events (
                event_seq INTEGER PRIMARY KEY,
                aggregate_id TEXT NOT NULL,
                event_type TEXT NOT NULL,
                occurred_at TEXT NOT NULL,
                payload_json TEXT NOT NULL,
                agent_id TEXT NOT NULL,
                source_ref TEXT NOT NULL
            );
            CREATE TABLE presence_current (payload_json TEXT);
            CREATE TABLE handoff_current (payload_json TEXT);
            CREATE TABLE read_observation_current (payload_json TEXT);
            CREATE TABLE wait_current (payload_json TEXT);
            CREATE TABLE context_delivery_current (payload_json TEXT);
            CREATE TABLE workspace_version (version INTEGER);
            CREATE TABLE notification_current (payload_json TEXT);
            """
        )
        connection.executemany(
            "INSERT INTO journal_events VALUES (?, ?, ?, ?, ?, ?, ?)",
            [
                (
                    1,
                    "notification-known",
                    "notification.created",
                    "2026-07-16T00:00:01Z",
                    json.dumps({"event": {"data": {"data": {"notification": {"kind": "scope_overlap"}}}}}),
                    "agent",
                    "source",
                ),
                (
                    2,
                    "notification-secret",
                    "notification.created",
                    "2026-07-16T00:00:02Z",
                    json.dumps({"event": {"data": {"data": {"notification": {"kind": "customer_secret"}}}}}),
                    "agent",
                    "source",
                ),
            ],
        )
        connection.commit()
        connection.close()

        snapshot = self.diagnostics.snapshot_home(self.home)
        metrics = snapshot["databases"][".stateful/state.db"]["coordination_metrics"]

        self.assertEqual(metrics["notifications"]["by_kind"], {"scope_overlap": 1})

if __name__ == "__main__":
    unittest.main()
