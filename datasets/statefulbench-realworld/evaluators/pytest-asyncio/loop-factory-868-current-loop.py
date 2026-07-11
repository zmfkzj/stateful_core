#!/usr/bin/env python3
"""Evaluator for pytest-asyncio issue #868."""
import argparse
import os
import subprocess
import sys
import tempfile
from pathlib import Path


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("repo", type=Path)
    repo = parser.parse_args().repo.resolve()
    with tempfile.TemporaryDirectory() as directory:
        work = Path(directory)
        metadata = work / "pytest_asyncio-0.dist-info"
        metadata.mkdir()
        (metadata / "METADATA").write_text(
            "Metadata-Version: 2.1\nName: pytest_asyncio\nVersion: 0\n"
        )
        (metadata / "entry_points.txt").write_text(
            "[pytest11]\nasyncio = pytest_asyncio.plugin\n"
        )
        (work / "conftest.py").write_text(
            "import asyncio\n"
            "import pytest\n"
            "original_loop = asyncio.new_event_loop()\n"
            "asyncio.set_event_loop(original_loop)\n"
            "@pytest.hookimpl(trylast=True)\n"
            "def pytest_sessionfinish(session, exitstatus):\n"
            "    try:\n"
            "        assert asyncio.get_event_loop() is original_loop\n"
            "    finally:\n"
            "        asyncio.set_event_loop(None)\n"
            "        original_loop.close()\n"
        )
        (work / "test_current_loop.py").write_text(
            "import pytest\n"
            "pytest_plugins = 'pytest_asyncio'\n"
            "@pytest.mark.asyncio\n"
            "async def test_async():\n"
            "    pass\n"
        )
        env = os.environ | {
            "PYTHONPATH": os.pathsep.join((str(repo), str(work), os.environ.get("PYTHONPATH", ""))),
        }
        result = subprocess.run(
            [sys.executable, "-m", "pytest", "-q", "-p", "no:cacheprovider"],
            cwd=work,
            env=env,
            text=True,
            capture_output=True,
        )
        assert result.returncode == 0, result.stdout + result.stderr


if __name__ == "__main__":
    main()
