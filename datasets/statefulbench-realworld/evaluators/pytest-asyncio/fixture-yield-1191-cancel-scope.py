#!/usr/bin/env python3
"""Verify async-generator fixtures exit task-bound scopes in their setup task."""

from __future__ import annotations

import os
from pathlib import Path
import subprocess
import sys
import tempfile
import textwrap


def main() -> None:
    if len(sys.argv) != 2:
        raise SystemExit(f"usage: {Path(sys.argv[0]).name} <pytest-asyncio-repo>")

    repo = Path(sys.argv[1]).resolve()
    test = '''\
        import asyncio

        import pytest
        import pytest_asyncio

        class TaskBoundScope:
            def __enter__(self):
                self.entered_by = asyncio.current_task()
                return self

            def __exit__(self, exc_type, exc, traceback):
                if asyncio.current_task() is not self.entered_by:
                    raise RuntimeError("scope exited in a different task")

        @pytest_asyncio.fixture
        async def scope():
            with TaskBoundScope() as value:
                yield value

        @pytest.mark.asyncio
        async def test_scope_teardown_uses_setup_task(scope):
            assert scope.entered_by is not None
            print("test body completed")
        '''
    with tempfile.TemporaryDirectory() as temporary_directory:
        temporary_path = Path(temporary_directory)
        test_path = temporary_path / "test_task_bound_scope.py"
        test_path.write_text(textwrap.dedent(test), encoding="utf-8")
        metadata = temporary_path / "pytest_asyncio-0.dist-info"
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
        result = subprocess.run(
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
            capture_output=True,
            check=False,
        )

    output = result.stdout + result.stderr
    if result.returncode != 0:
        raise AssertionError(f"task-bound fixture scope did not teardown cleanly\n{output}")
    if "test body completed" not in output:
        raise AssertionError(f"fixture test did not run\n{output}")


if __name__ == "__main__":
    main()
