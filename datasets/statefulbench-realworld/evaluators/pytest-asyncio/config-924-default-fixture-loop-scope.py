#!/usr/bin/env python3
"""Check the finalized default fixture-loop scope."""
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
        project = Path(temporary)
        metadata = project / "pytest_asyncio-0.dist-info"
        metadata.mkdir()
        (metadata / "METADATA").write_text(
            "Metadata-Version: 2.1\nName: pytest-asyncio\nVersion: 0\n"
        )
        (project / "pytest.ini").write_text(
            "[pytest]\nfilterwarnings =\n    error::pytest.PytestDeprecationWarning\n"
        )
        (project / "conftest.py").write_text(
            "import warnings\n\nimport pytest\n\n\n"
            "@pytest.hookimpl(tryfirst=True)\n"
            "def pytest_configure():\n"
            "    warnings.simplefilter(\"error\", pytest.PytestDeprecationWarning)\n"
        )
        (project / "test_default.py").write_text(
            """import asyncio
import pytest
import pytest_asyncio


@pytest_asyncio.fixture
async def fixture_loop():
    return asyncio.get_running_loop()


@pytest.mark.asyncio
async def test_uses_function_default(fixture_loop, pytestconfig):
    assert pytestconfig.getini("asyncio_default_fixture_loop_scope") == "function"
    assert asyncio.get_running_loop() is fixture_loop
"""
        )
        result = run_pytest(project, checkout)

    output = result.stdout + result.stderr
    assert result.returncode == 0, output
    assert "1 passed" in output, output
    assert '"asyncio_default_fixture_loop_scope" is unset' not in output, output


if __name__ == "__main__":
    main()
