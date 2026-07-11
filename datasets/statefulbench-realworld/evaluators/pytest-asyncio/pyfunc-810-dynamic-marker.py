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
            "import asyncio\n"
            "import inspect\n"
            "import pytest\n"
            "import warnings\n\n"
            "warnings.filterwarnings('ignore', message=\".*DefaultEventLoopPolicy.*\", category=DeprecationWarning)\n\n"
            "class TaggedPolicy(asyncio.DefaultEventLoopPolicy):\n"
            "    def new_event_loop(self):\n"
            "        loop = super().new_event_loop()\n"
            "        loop.created_by_dynamic_policy = True\n"
            "        return loop\n\n"
            "@pytest.fixture\n"
            "def event_loop_policy():\n"
            "    return TaggedPolicy()\n\n"
            "def pytest_collection_modifyitems(items):\n"
            "    for item in items:\n"
            "        if inspect.iscoroutinefunction(item.obj):\n"
            "            item.add_marker(pytest.mark.asyncio(timeout=0.01))\n"
            "        elif item.name == 'test_dynamic_marked_sync':\n"
            "            item.add_marker(pytest.mark.asyncio)\n",
        )
        (root / "test_dynamic.py").write_text(
            "import asyncio\n"
            "from pathlib import Path\n\n"
            "async def test_dynamic_marker_uses_configured_runner_and_timeout():\n"
            "    assert asyncio.get_running_loop().created_by_dynamic_policy\n"
            "    try:\n"
            "        await asyncio.sleep(0.1)\n"
            "    finally:\n"
            "        Path('cleanup.txt').write_text('done')\n\n"
            "def test_following_test_runs_after_timeout():\n"
            "    assert Path('cleanup.txt').read_text() == 'done'\n\n"
            "def test_dynamic_marked_sync():\n"
            "    pass\n",
        )
        environment = os.environ.copy()
        environment["PYTEST_DISABLE_PLUGIN_AUTOLOAD"] = "1"
        environment["PYTHONWARNINGS"] = "default"
        environment["PYTHONPATH"] = os.pathsep.join(
            (str(root), str(checkout), str(dependencies)),
        )
        result = subprocess.run(
            [
                sys.executable,
                "-m",
                "pytest",
                "-W",
                "default",
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
        output = result.stdout + result.stderr
        assert result.returncode != 0, output
        assert "TimeoutError" in output, output
        assert "1 failed, 2 passed" in output, output
        assert (root / "cleanup.txt").read_text() == "done"
        assert "but it is not an async function." in output, output


if __name__ == "__main__":
    main()
