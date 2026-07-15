#!/usr/bin/env python3
"""Evaluator for pytest-asyncio issue #215."""

from __future__ import annotations

import os
import subprocess
import sys
import tempfile
from pathlib import Path


def main() -> None:
    checkout = Path(sys.argv[1]).resolve()
    with tempfile.TemporaryDirectory() as temporary:
        root = Path(temporary)
        metadata = root / "pytest_asyncio-0.dist-info"
        metadata.mkdir()
        (metadata / "METADATA").write_text(
            "Metadata-Version: 2.1\nName: pytest-asyncio\nVersion: 0\n",
        )
        (root / "test_timeout.py").write_text(
            "import asyncio\n"
            "from pathlib import Path\n"
            "import pytest\n\n"
            "@pytest.mark.asyncio(timeout=0.01)\n"
            "async def test_timeout_cleans_up():\n"
            "    try:\n"
            "        await asyncio.sleep(0.1)\n"
            "    finally:\n"
            "        Path('cleanup.txt').write_text('done')\n\n"
            "def test_cancellation_finally_ran():\n"
            "    assert Path('cleanup.txt').read_text() == 'done'\n",
        )
        environment = os.environ.copy()
        environment["PYTEST_DISABLE_PLUGIN_AUTOLOAD"] = "1"
        environment["PYTHONWARNINGS"] = "default"
        environment["PYTHONPATH"] = os.pathsep.join((str(root), str(checkout)))
        result = subprocess.run(
            [
                sys.executable,
                "-m",
                "pytest",
                "-p",
                "pytest_asyncio.plugin",
                "-W",
                "default",
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
        output = result.stdout + result.stderr
        assert result.returncode != 0, output
        assert "TimeoutError" in output, output
        assert "1 failed, 1 passed" in output, output
        assert (root / "cleanup.txt").read_text() == "done"


if __name__ == "__main__":
    main()
