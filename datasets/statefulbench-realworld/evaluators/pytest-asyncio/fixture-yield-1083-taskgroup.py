#!/usr/bin/env python3
"""Verify async-generator fixture failures preserve their owning task."""

from __future__ import annotations

import ast
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import textwrap
import time


def _assert_python_310_create_task(repo: Path) -> None:
    tree = ast.parse((repo / "pytest_asyncio" / "plugin.py").read_text(encoding="utf-8"))
    create_task_aliases = {"create_task"}
    for node in ast.walk(tree):
        if isinstance(node, ast.ImportFrom):
            create_task_aliases.update(
                alias.asname or alias.name
                for alias in node.names
                if alias.name == "create_task"
            )

    def is_create_task(callable: ast.expr) -> bool:
        return (
            isinstance(callable, ast.Attribute) and callable.attr == "create_task"
        ) or (
            isinstance(callable, ast.Name) and callable.id in create_task_aliases
        )

    class CreateTaskVisitor(ast.NodeVisitor):
        def visit_Assign(self, node: ast.Assign) -> None:
            self.visit(node.value)
            if is_create_task(node.value):
                create_task_aliases.update(
                    target.id for target in node.targets if isinstance(target, ast.Name)
                )

        def visit_AnnAssign(self, node: ast.AnnAssign) -> None:
            if node.value is not None:
                self.visit(node.value)
                if is_create_task(node.value) and isinstance(node.target, ast.Name):
                    create_task_aliases.add(node.target.id)

        def visit_Call(self, node: ast.Call) -> None:
            if is_create_task(node.func) and any(
                keyword.arg == "context" for keyword in node.keywords
            ):
                raise AssertionError(
                    "create_task(..., context=...) is not Python 3.10 compatible"
                )
            self.generic_visit(node)

    CreateTaskVisitor().visit(tree)


def _run_pytest(repo: Path, test: str, *, timeout: float = 3) -> subprocess.CompletedProcess[str]:
    with tempfile.TemporaryDirectory() as temporary_directory:
        temporary_path = Path(temporary_directory)
        test_path = temporary_path / "test_asyncgen_fixture.py"
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
            stdout, stderr = process.communicate(timeout=timeout)
        except subprocess.TimeoutExpired as error:
            process.kill()
            process.communicate()
            raise AssertionError("async-generator fixture teardown hung") from error
        return subprocess.CompletedProcess(process.args, process.returncode, stdout, stderr)


def _require(output: str, *expected: str) -> None:
    for value in expected:
        if value not in output:
            raise AssertionError(f"missing expected output: {value!r}\n{output}")


def main() -> None:
    if len(sys.argv) != 2:
        raise SystemExit(f"usage: {Path(sys.argv[0]).name} <pytest-asyncio-repo>")

    repo = Path(sys.argv[1]).resolve()
    _assert_python_310_create_task(repo)
    taskgroup = '''\
        import asyncio
        import contextvars
        from asyncio import TaskGroup

        import pytest
        import pytest_asyncio

        fixture_context = contextvars.ContextVar("fixture_context", default="missing")

        @pytest.fixture(autouse=True)
        def python_310_create_task(monkeypatch):
            original = asyncio.create_task
            def create_task(coro, *, name=None):
                return original(coro, name=name)
            monkeypatch.setattr(asyncio, "create_task", create_task)

        @pytest_asyncio.fixture
        async def context_fixture():
            token = fixture_context.set("fixture")
            owner = asyncio.current_task()
            try:
                yield owner
            finally:
                assert fixture_context.get() == "fixture"
                assert asyncio.current_task() is owner
                fixture_context.reset(token)
                print("fixture context preserved")

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
        async def test_contextvars_and_taskgroup(context_fixture, guard):
            assert fixture_context.get() == "fixture"
            assert context_fixture is not None
            await asyncio.sleep(0.05)
            print("test body completed")
        '''
    started = time.monotonic()
    result = _run_pytest(repo, taskgroup)
    output = result.stdout + result.stderr
    if result.returncode == 0:
        raise AssertionError("TaskGroup fixture failure was not reported")
    if time.monotonic() - started >= 3:
        raise AssertionError("TaskGroup fixture teardown was not prompt")
    _require(
        output,
        "test body completed",
        "fixture context preserved",
        "companion cancelled",
        "fixture boom",
    )

    setup_error = '''\
        import asyncio

        import pytest
        import pytest_asyncio

        @pytest.fixture(autouse=True)
        def python_310_create_task(monkeypatch):
            original = asyncio.create_task
            def create_task(coro, *, name=None):
                return original(coro, name=name)
            monkeypatch.setattr(asyncio, "create_task", create_task)

        @pytest_asyncio.fixture
        async def broken_setup():
            raise RuntimeError("setup boom")
            yield

        @pytest.mark.asyncio
        async def test_setup_error_is_retrieved(broken_setup):
            pass
        '''
    result = _run_pytest(repo, setup_error)
    output = result.stdout + result.stderr
    if result.returncode == 0:
        raise AssertionError("fixture setup error was not reported")
    _require(output, "setup boom")
    for forbidden in ("Task exception was never retrieved", "was never awaited"):
        if forbidden in output:
            raise AssertionError(f"setup exception leaked from background task\n{output}")


if __name__ == "__main__":
    main()
