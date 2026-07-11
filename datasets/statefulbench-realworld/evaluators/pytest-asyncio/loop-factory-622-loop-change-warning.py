#!/usr/bin/env python3
"""Evaluator for pytest-asyncio issue #622."""
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
        (work / "conftest.py").write_text(
            "import asyncio\n"
            "from pathlib import Path\n"
            "import pytest\n"
            "import pytest_asyncio.plugin as plugin\n"
            "state = Path('runner-state.txt')\n"
            "original_loop = asyncio.new_event_loop()\n"
            "asyncio.set_event_loop(original_loop)\n"
            "runners = []\n"
            "Runner = plugin.Runner\n"
            "class TrackingRunner(Runner):\n"
            "    def __enter__(self):\n"
            "        runner = super().__enter__()\n"
            "        self.created_loop = runner.get_loop()\n"
            "        runners.append(self)\n"
            "        return runner\n"
            "plugin.Runner = TrackingRunner\n"
            "@pytest.hookimpl(trylast=True)\n"
            "def pytest_sessionfinish(session, exitstatus):\n"
            "    try:\n"
            "        assert len(runners) == 1\n"
            "        assert runners[0].created_loop.is_closed()\n"
            "        assert asyncio.get_event_loop() is original_loop\n"
            "        state.write_text('runner closed; old loop restored\\n')\n"
            "    finally:\n"
            "        asyncio.set_event_loop(None)\n"
            "        original_loop.close()\n"
        )
        env = os.environ | {
            "PYTHONPATH": os.pathsep.join((str(repo), str(work), os.environ.get("PYTHONPATH", ""))),
        }

        def run(*args: str) -> subprocess.CompletedProcess[str]:
            return subprocess.run(
                [sys.executable, "-m", "pytest", "-q", "-p", "no:cacheprovider", "-p", "pytest_asyncio.plugin", *args],
                cwd=work,
                env=env,
                text=True,
                capture_output=True,
            )

        unchanged = work / "test_unchanged.py"
        unchanged.write_text(
            "import pytest\n"
            "@pytest.mark.asyncio\n"
            "async def test_keeps_runner_loop_current():\n"
            "    pass\n"
        )
        result = run("-W", "default")
        output = result.stdout + result.stderr
        assert result.returncode == 0, output
        assert "pytest-asyncio detected that a test changed the current event loop" not in output
        assert (work / "runner-state.txt").read_text() == "runner closed; old loop restored\n"
        unchanged.unlink()

        changed = work / "test_changed.py"
        changed.write_text(
            "import asyncio\n"
            "import pytest\n"
            "@pytest.mark.asyncio\n"
            "async def test_replaces_current_loop():\n"
            "    replacement_loop = asyncio.new_event_loop()\n"
            "    asyncio.set_event_loop(replacement_loop)\n"
            "    replacement_loop.close()\n"
        )
        (work / "runner-state.txt").unlink()
        result = run("-W", "default")
        output = result.stdout + result.stderr
        warning = "pytest-asyncio detected that a test changed the current event loop"
        assert result.returncode == 0, output
        assert output.count(warning) == 1, output
        assert (work / "runner-state.txt").read_text() == "runner closed; old loop restored\n"

        (work / "runner-state.txt").unlink()
        result = run("-W", "error::RuntimeWarning")
        output = result.stdout + result.stderr
        assert result.returncode != 0, output
        assert output.count(warning) == 1, output
        assert (work / "runner-state.txt").read_text() == "runner closed; old loop restored\n"


if __name__ == "__main__":
    main()
