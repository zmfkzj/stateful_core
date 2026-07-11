#!/usr/bin/env python3
"""Verify TaskGroup failures from async-generator fixtures finish teardown."""

from __future__ import annotations

import os
from pathlib import Path
import subprocess
import sys
import tempfile
import textwrap
import time


def main() -> None:
    if len(sys.argv) != 2:
        raise SystemExit(f"usage: {Path(sys.argv[0]).name} <pytest-asyncio-repo>")

    repo = Path(sys.argv[1]).resolve()
    test = '''\
        import asyncio
        from asyncio import TaskGroup


        import pytest
        import pytest_asyncio


        @pytest_asyncio.fixture
        async def guard():
            async def companion():
                try:
                    await asyncio.Event().wait()
                finally:
                    print("companion cancelled")

            async def fail():
                await asyncio.sleep(0)
                raise RuntimeError("fixture boom")

            async with TaskGroup() as tasks:
                tasks.create_task(companion())
                tasks.create_task(fail())
                yield

        @pytest.mark.asyncio
        async def test_taskgroup_failure_is_reported(guard):
            await asyncio.sleep(0.05)
            print("test body completed")
        '''
    with tempfile.TemporaryDirectory() as temporary_directory:
        test_path = Path(temporary_directory) / "test_taskgroup_fixture.py"
        test_path.write_text(textwrap.dedent(test), encoding="utf-8")
        metadata = Path(temporary_directory) / "pytest_asyncio-0.dist-info"
        metadata.mkdir()
        (metadata / "METADATA").write_text(
            "Metadata-Version: 2.1\nName: pytest-asyncio\nVersion: 0\n",
            encoding="utf-8",
        )
        dependencies = next(
            (
                candidate
                for parent in repo.parents
                if (candidate := parent / "pytest-asyncio-deps").is_dir()
            ),
            None,
        )
        environment = os.environ.copy()
        environment["PYTHONPATH"] = os.pathsep.join(
            filter(
                None,
                (
                    temporary_directory,
                    str(repo),
                    str(dependencies) if dependencies is not None else "",
                    environment.get("PYTHONPATH"),
                ),
            )
        )
        environment["PYTEST_PLUGINS"] = "pytest_asyncio.plugin"
        started = time.monotonic()
        process = subprocess.Popen(
            [
                sys.executable,
                "-m",
                "pytest",
                "-q",
                "-s",
                "--asyncio-mode=strict",
                str(test_path),
            ],
            cwd=temporary_directory,
            env=environment,
            text=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
        )
        try:
            stdout, stderr = process.communicate(timeout=3)
        except subprocess.TimeoutExpired as error:
            process.kill()
            process.communicate()
            raise AssertionError("TaskGroup fixture teardown hung") from error

        result = subprocess.CompletedProcess(
            process.args, process.returncode, stdout, stderr
        )

    output = result.stdout + result.stderr
    if result.returncode == 0:
        raise AssertionError("TaskGroup fixture failure was not reported")
    if time.monotonic() - started >= 3:
        raise AssertionError("TaskGroup fixture teardown was not prompt")
    for expected in ("test body completed", "companion cancelled", "fixture boom"):
        if expected not in output:
            raise AssertionError(f"missing expected output: {expected!r}\n{output}")


if __name__ == "__main__":
    main()
