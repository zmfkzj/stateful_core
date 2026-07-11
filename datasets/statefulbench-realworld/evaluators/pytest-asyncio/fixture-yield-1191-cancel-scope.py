#!/usr/bin/env python3
"""Verify async-generator fixtures exit task-bound scopes in their setup task."""

from __future__ import annotations

import ast
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import textwrap


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


def _run_pytest(repo: Path, test: str) -> subprocess.CompletedProcess[str]:
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
        return subprocess.run(
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


def _require(output: str, *expected: str) -> None:
    for value in expected:
        if value not in output:
            raise AssertionError(f"missing expected output: {value!r}\n{output}")


def main() -> None:
    if len(sys.argv) != 2:
        raise SystemExit(f"usage: {Path(sys.argv[0]).name} <pytest-asyncio-repo>")

    repo = Path(sys.argv[1]).resolve()
    _assert_python_310_create_task(repo)
    scope_test = '''\
        import asyncio
        import contextvars

        import pytest
        import pytest_asyncio

        fixture_context = contextvars.ContextVar("fixture_context", default="missing")

        class TaskBoundScope:
            def __enter__(self):
                self.entered_by = asyncio.current_task()
                return self

            def __exit__(self, exc_type, exc, traceback):
                if asyncio.current_task() is not self.entered_by:
                    raise RuntimeError("scope exited in a different task")

        @pytest.fixture(autouse=True)
        def python_310_create_task(monkeypatch):
            original = asyncio.create_task
            def create_task(coro, *, name=None):
                return original(coro, name=name)
            monkeypatch.setattr(asyncio, "create_task", create_task)

        @pytest_asyncio.fixture
        async def scope():
            token = fixture_context.set("fixture")
            with TaskBoundScope() as value:
                try:
                    yield value
                finally:
                    assert fixture_context.get() == "fixture"
                    fixture_context.reset(token)
                    print("fixture context preserved")

        @pytest.mark.asyncio
        async def test_scope_teardown_uses_setup_task(scope):
            assert scope.entered_by is not None
            assert fixture_context.get() == "fixture"
            print("test body completed")
        '''
    result = _run_pytest(repo, scope_test)
    output = result.stdout + result.stderr
    if result.returncode != 0:
        raise AssertionError(f"task-bound fixture scope did not teardown cleanly\n{output}")
    _require(output, "test body completed", "fixture context preserved")

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
