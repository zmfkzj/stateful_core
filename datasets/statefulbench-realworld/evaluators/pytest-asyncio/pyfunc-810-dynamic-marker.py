#!/usr/bin/env python3
"""Evaluator for pytest-asyncio issue #810."""

from __future__ import annotations

import os
import subprocess
import sys
import tempfile
from pathlib import Path


def main() -> None:
    checkout = Path(sys.argv[1]).resolve()
    dependencies = next(
        parent / "pytest-asyncio-deps"
        for parent in checkout.parents
        if (parent / "pytest-asyncio-deps").is_dir()
    )
    with tempfile.TemporaryDirectory() as temporary:
        root = Path(temporary)
        metadata = root / "pytest_asyncio-0.dist-info"
        metadata.mkdir()
        (metadata / "METADATA").write_text(
            "Metadata-Version: 2.1\nName: pytest-asyncio\nVersion: 0\n",
        )
        (root / "conftest.py").write_text(
            "import inspect\n\n"
            "def pytest_collection_modifyitems(items):\n"
            "    for item in items:\n"
            "        if inspect.iscoroutinefunction(item.obj):\n"
            "            item.add_marker('asyncio')\n",
        )
        (root / "test_dynamic.py").write_text(
            "import asyncio\n\n"
            "async def test_dynamic_marker_runs():\n"
            "    assert asyncio.get_running_loop().is_running()\n",
        )
        environment = os.environ.copy()
        environment["PYTEST_DISABLE_PLUGIN_AUTOLOAD"] = "1"
        environment["PYTHONPATH"] = os.pathsep.join(
            (str(root), str(checkout), str(dependencies)),
        )
        result = subprocess.run(
            [
                sys.executable,
                "-m",
                "pytest",
                "-p",
                "pytest_asyncio.plugin",
                "--asyncio-mode=strict",
                "-q",
            ],
            cwd=root,
            env=environment,
            text=True,
            capture_output=True,
            check=False,
            timeout=10,
        )
        assert result.returncode == 0, result.stdout + result.stderr
        assert "1 passed" in result.stdout, result.stdout


if __name__ == "__main__":
    main()
