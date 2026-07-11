#!/usr/bin/env python3
"""Check nested pytest configuration does not revive the unset warning."""
from __future__ import annotations

import os
from pathlib import Path
import subprocess
import sys
import tempfile


DEPS = Path("/private/tmp/statefulbench-realworld-curation/pytest-asyncio-deps")


def run_pytest(project: Path, checkout: Path) -> subprocess.CompletedProcess[str]:
    environment = os.environ.copy()
    environment["PYTHONPATH"] = os.pathsep.join(
        (str(checkout), str(DEPS), environment.get("PYTHONPATH", ""))
    )
    return subprocess.run(
        [
            sys.executable,
            "-B",
            "-m",
            "pytest",
            "-p",
            "pytest_asyncio.plugin",
            "-q",
        ],
        cwd=project,
        env=environment,
        text=True,
        capture_output=True,
    )


def main() -> None:
    checkout = Path(sys.argv[1]).resolve()
    with tempfile.TemporaryDirectory() as temporary:
        root = Path(temporary)
        metadata = root / "pytest_asyncio-0.dist-info"
        metadata.mkdir()
        (metadata / "METADATA").write_text(
            "Metadata-Version: 2.1\nName: pytest-asyncio\nVersion: 0\n"
        )
        (root / "pyproject.toml").write_text(
            "[tool.pytest.ini_options]\n"
            'asyncio_default_fixture_loop_scope = "module"\n'
        )
        (root / "test_outer.py").write_text(
            """def test_outer_config_keeps_precedence(pytestconfig):
    assert pytestconfig.getini("asyncio_default_fixture_loop_scope") == "module"
"""
        )
        outer = run_pytest(root, checkout)

        inner = root / "embedded"
        inner.mkdir()
        metadata.rename(inner / metadata.name)
        (inner / "pytest.ini").write_text(
            "[pytest]\nfilterwarnings =\n    error::pytest.PytestDeprecationWarning\n"
        )
        (inner / "conftest.py").write_text(
            "import warnings\n\nimport pytest\n\n\n"
            "@pytest.hookimpl(tryfirst=True)\n"
            "def pytest_configure():\n"
            "    warnings.simplefilter(\"error\", pytest.PytestDeprecationWarning)\n"
        )
        (inner / "test_inner.py").write_text(
            """import asyncio
import pytest
import pytest_asyncio


@pytest_asyncio.fixture
async def fixture_loop():
    return asyncio.get_running_loop()


@pytest.mark.asyncio
async def test_inner_uses_default(fixture_loop):
    assert asyncio.get_running_loop() is fixture_loop
"""
        )
        nested = run_pytest(inner, checkout)

    outer_output = outer.stdout + outer.stderr
    assert outer.returncode == 0, outer_output
    assert "1 passed" in outer_output, outer_output
    assert '"asyncio_default_fixture_loop_scope" is unset' not in outer_output, outer_output

    nested_output = nested.stdout + nested.stderr
    assert nested.returncode == 0, nested_output
    assert "1 passed" in nested_output, nested_output
    assert '"asyncio_default_fixture_loop_scope" is unset' not in nested_output, nested_output


if __name__ == "__main__":
    main()
