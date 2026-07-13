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
