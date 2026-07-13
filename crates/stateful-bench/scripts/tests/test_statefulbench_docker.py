from __future__ import annotations

import json
import os
import runpy
import subprocess
import tempfile
import unittest
import time
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
        runtime = self.mod.inspect_runtime(
            "docker", "statefulbench-realworld:local", runner=Mock(return_value=completed)
        )
        self.assertEqual(runtime.image_id, "sha256:abc")
        self.assertEqual(runtime.repo_digests, ("statefulbench@sha256:def",))
        self.assertEqual(runtime.platform, "linux/arm64")
        self.assertTrue(Path(runtime.binary).is_absolute())

    def test_inspect_runtime_fails_closed_on_missing_daemon_or_non_linux_image(self) -> None:
        with self.assertRaisesRegex(RuntimeError, "Docker image inspection failed"):
            self.mod.inspect_runtime(
                "docker",
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
                patch.object(self.entry.os, "setsid") as setsid,
                patch.object(self.entry.os, "getpid", return_value=42),
                patch.object(self.entry.os, "getpgrp", side_effect=[41, 42]),
                patch.object(self.entry.os, "replace") as replace,
                patch.object(self.entry.os, "execvpe", side_effect=RuntimeError("exec")) as execvpe,
            ):
                with self.assertRaisesRegex(RuntimeError, "exec"):
                    self.entry.main(["entry", str(pid_file), "/usr/bin/env", "true"])

            assert json.loads(temporary.read_text(encoding="utf-8")) == {"pid": 42, "pgid": 42}
            setsid.assert_called_once_with()
            replace.assert_called_once_with(temporary, pid_file)
            execvpe.assert_called_once_with("/usr/bin/env", ["/usr/bin/env", "true"], os.environ)

    def test_entrypoint_forks_group_leader_and_records_exec_child_identity(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            pid_file = Path(directory) / "agent.pid.json"
            temporary = pid_file.with_suffix(".tmp")
            with (
                patch.object(self.entry.os, "fork", return_value=0) as fork,
                patch.object(self.entry.os, "setsid") as setsid,
                patch.object(self.entry.os, "getpid", side_effect=[42, 99]),
                patch.object(self.entry.os, "getpgrp", side_effect=[42, 99]),
                patch.object(self.entry.os, "replace") as replace,
                patch.object(self.entry.os, "execvpe", side_effect=RuntimeError("exec")) as execvpe,
            ):
                with self.assertRaisesRegex(RuntimeError, "exec"):
                    self.entry.main(["entry", str(pid_file), "/usr/bin/env", "true"])

            assert json.loads(temporary.read_text(encoding="utf-8")) == {"pid": 99, "pgid": 99}
            fork.assert_called_once_with()
            setsid.assert_called_once_with()
            replace.assert_called_once_with(temporary, pid_file)
            execvpe.assert_called_once_with("/usr/bin/env", ["/usr/bin/env", "true"], os.environ)

    def test_entrypoint_group_leader_parent_waits_for_child_exit(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            pid_file = Path(directory) / "agent.pid.json"
            with (
                patch.object(self.entry.os, "fork", return_value=99) as fork,
                patch.object(self.entry.os, "setsid") as setsid,
                patch.object(self.entry.os, "getpid", return_value=42),
                patch.object(self.entry.os, "getpgrp", return_value=42),
                patch.object(self.entry.os, "waitpid", return_value=(99, 0)) as waitpid,
                patch.object(self.entry.os, "execvpe", side_effect=RuntimeError("exec")),
            ):
                self.assertEqual(
                    self.entry.main(["entry", str(pid_file), "/usr/bin/env", "true"]),
                    0,
                )

        fork.assert_called_once_with()
        waitpid.assert_called_once_with(99, 0)
        setsid.assert_not_called()

    def test_entrypoint_main_exits_with_group_leader_child_failure(self) -> None:
        with (
            patch.object(os, "fork", return_value=99),
            patch.object(os, "getpid", return_value=42),
            patch.object(os, "getpgrp", return_value=42),
            patch.object(os, "waitpid", return_value=(99, 3 << 8)),
            patch.object(sys, "argv", ["entry", "/tmp/agent.pid.json", "/usr/bin/env", "true"]),
        ):
            with self.assertRaises(SystemExit) as exited:
                runpy.run_path(self.entry.__file__, run_name="__main__")

        self.assertEqual(exited.exception.code, 3)

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

        with patch.object(self.mod, "copy_to_container") as copy:
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
        copy.assert_called_once_with(
            container,
            credential,
            "/home/stateful/.omp/profiles/stateful/agent/agent.db",
            runner=runner,
        )
        commands = [call.args[0] for call in runner.call_args_list]
        self.assertEqual(sum(command[-4:] == ["install", "--agent", "omp", "--yes"] for command in commands), 1)
        self.assertEqual(sum(command[-3:] == ["enable", "--repo", "/workspace"] for command in commands), 1)
        self.assertEqual(
            sum(command[-3:] == ["server", "start", "--coordination-mode"] or command[-4:] == ["server", "start", "--coordination-mode", "enforcement"] for command in commands),
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
            self.assertIn("-d", command)
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
    def test_wait_requires_the_inner_process_group_to_be_gone(self) -> None:
        pid_record = self.container.runtime_dir / "pids" / "task-a.json"
        pid_record.parent.mkdir()
        pid_record.write_text('{"pid": 42, "pgid": 42}\n', encoding="utf-8")
        exit_record = pid_record.with_suffix(".exit")
        exit_record.write_text("0\n", encoding="utf-8")
        handle = self.mod.DockerAgentHandle(
            Mock(wait=Mock(return_value=0), returncode=0),
            "task-a",
            self.container.container_id,
            pid_record,
            0.0,
            exit_record,
        )
        runner = Mock(return_value=subprocess.CompletedProcess([], 1, "", ""))

        record, _ = self.mod.wait_agent(
            self.container, handle, self.root, "task", self.cfg, runner=runner
        )

        self.assertEqual(record["exit_code"], 0)
        self.assertIsNone(record["cleanup_error"])
        self.assertIn(
            ["/usr/local/bin/docker", "exec", "--workdir", "/workspace", "container-1", "kill", "-0", "-42"],
            [call.args[0] for call in runner.call_args_list],
        )

    def test_wait_reaps_a_timed_out_detached_docker_client(self) -> None:
        pid_record = self.container.runtime_dir / "pids" / "task-a.json"
        pid_record.parent.mkdir()
        pid_record.write_text('{"pid": 42, "pgid": 42}\n', encoding="utf-8")
        process = Mock(returncode=None)
        process.wait.side_effect = [subprocess.TimeoutExpired("docker", 5), 0]
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

    def test_wait_polls_for_a_delayed_exit_record(self) -> None:
        pid_record = self.container.runtime_dir / "pids" / "task-a.json"
        pid_record.parent.mkdir()
        pid_record.write_text('{"pid": 42, "pgid": 42}\n', encoding="utf-8")
        exit_record = pid_record.with_suffix(".exit")
        handle = self.mod.DockerAgentHandle(
            Mock(wait=Mock(return_value=0), returncode=0),
            "task-a",
            self.container.container_id,
            pid_record,
            time.monotonic(),
            exit_record,
        )
        runner = Mock(
            side_effect=[
                subprocess.CompletedProcess([], 1, "", ""),
                subprocess.CompletedProcess([], 1, "", ""),
            ]
        )

        with patch.object(self.mod.time, "sleep", side_effect=lambda _: exit_record.write_text("0\n")):
            record, _ = self.mod.wait_agent(
                self.container, handle, self.root, "task", self.cfg, runner=runner
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

    def setUp(self) -> None:
        self.tempdir = tempfile.TemporaryDirectory()
        self.home = Path(self.tempdir.name) / "home"
        self.home.mkdir()

    def tearDown(self) -> None:
        self.tempdir.cleanup()

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
            self.diagnostics.classify_runtime_failure("not a database"),
            "sqlite_malformed",
        )
        self.assertEqual(
            self.diagnostics.classify_runtime_failure("unexpected failure"),
            "unclassified_runtime_failure",
        )

    def test_snapshot_skips_symlinked_databases_and_fails_closed_on_unavailable_sqlite(self) -> None:
        import sqlite3

        outside = Path(self.tempdir.name) / "outside.db"
        with sqlite3.connect(outside) as connection:
            connection.execute("create table external_data (id integer)")
        connection.close()
        (self.home / "outside.db").symlink_to(outside)

        snapshot = self.diagnostics.snapshot_home(self.home)

        self.assertNotIn("outside.db", snapshot["databases"])
        self.assertEqual(
            self.diagnostics.classify_runtime_failure(
                None, {"databases": {"agent.db": {"integrity": "unavailable"}}}
            ),
            "sqlite_unavailable",
        )

if __name__ == "__main__":
    unittest.main()
