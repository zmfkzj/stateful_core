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
            "import contextvars\n"
            "import inspect\n"
            "import pytest\n"
            "import warnings\n\n"
            "warnings.filterwarnings('ignore', message=\".*DefaultEventLoopPolicy.*\", category=DeprecationWarning)\n\n"
            "late_context = contextvars.ContextVar('late_context', default=None)\n\n"
            "class TaggedPolicy(asyncio.DefaultEventLoopPolicy):\n"
            "    def new_event_loop(self):\n"
            "        loop = super().new_event_loop()\n"
            "        loop.created_by_dynamic_policy = True\n"
            "        return loop\n\n"
            "class DynamicFactoryLoop(asyncio.SelectorEventLoop):\n"
            "    pass\n\n"
            "def pytest_configure(config):\n"
            "    original_run = asyncio.Runner.run\n"
            "    def run(self, coro, *, context=None):\n"
            "        assert context is not None, 'late coroutine omitted runner context'\n"
            "        return original_run(self, coro, context=context)\n"
            "    asyncio.Runner.run = run\n\n"
            "@pytest.fixture(scope='module')\n"
            "def event_loop_policy():\n"
            "    return TaggedPolicy()\n\n"
            "def pytest_asyncio_loop_factories(config, item):\n"
            "    return {'dynamic': DynamicFactoryLoop}\n\n"
            "def pytest_collection_modifyitems(items):\n"
            "    late_context.set('collection-context')\n"
            "    for item in items:\n"
            "        if item.name == 'test_dynamic_factory_timeout':\n"
            "            item.add_marker(pytest.mark.asyncio(timeout=0.01, loop_scope='module', loop_factories=['dynamic']))\n"
            "        elif item.name == 'test_dynamic_policy_runner':\n"
            "            item.add_marker(pytest.mark.asyncio)\n"
            "        elif item.name == 'test_dynamic_marked_sync':\n"
            "            item.add_marker(pytest.mark.asyncio)\n"
            "        elif item.name == 'test_dynamic_unknown_factory':\n"
            "            item.add_marker(pytest.mark.asyncio(loop_factories=['missing']))\n"
        )
        (root / "test_dynamic.py").write_text(
            "import asyncio\n"
            "from pathlib import Path\n"
            "from conftest import DynamicFactoryLoop, late_context\n\n"
            "async def test_dynamic_factory_timeout():\n"
            "    assert late_context.get() == 'collection-context'\n"
            "    assert type(asyncio.get_running_loop()) is DynamicFactoryLoop\n"
            "    try:\n"
            "        await asyncio.sleep(0.1)\n"
            "    finally:\n"
            "        Path('cleanup.txt').write_text('done')\n\n"
            "async def test_dynamic_policy_runner():\n"
            "    assert asyncio.get_running_loop().created_by_dynamic_policy\n\n"
            "def test_following_test_runs_after_timeout():\n"
            "    assert Path('cleanup.txt').read_text() == 'done'\n\n"
            "def test_dynamic_marked_sync():\n"
            "    pass\n\n"
            "async def test_dynamic_unknown_factory():\n"
            "    assert False, 'an unavailable late factory must skip this test'\n",
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
                "-rs",
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
        assert "1 failed, 3 passed, 1 skipped" in output, output
        assert (root / "cleanup.txt").read_text() == "done"
        assert "but it is not an async function." in output, output
        assert "Loop factory 'missing' is not available. Available factories: dynamic." in output, output


if __name__ == "__main__":
    main()
