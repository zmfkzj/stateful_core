from __future__ import annotations

import json
import os
import runpy
import subprocess
import tempfile
import unittest
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

if __name__ == "__main__":
    unittest.main()
