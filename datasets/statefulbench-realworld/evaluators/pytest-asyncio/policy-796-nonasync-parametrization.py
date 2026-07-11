#!/usr/bin/env python3
"""Check that event_loop_policy variants do not multiply synchronous tests."""
from __future__ import annotations

import os
from pathlib import Path
import subprocess
import sys
import tempfile


def main() -> None:
    checkout = Path(sys.argv[1]).resolve()
    curation_root = checkout.parent.parent if checkout.name == "policy" else checkout.parent
    dependencies = curation_root / "pytest-asyncio-deps"
    if not dependencies.exists():
        raise RuntimeError(f"missing pytest dependency bundle: {dependencies}")
    with tempfile.TemporaryDirectory(
        dir=curation_root / "pytest-asyncio-pairs" / "policy"
    ) as temporary:
        project = Path(temporary)
        metadata = project / "pytest_asyncio-0.0.0.dist-info"
        metadata.mkdir()
        (metadata / "METADATA").write_text("Name: pytest-asyncio\nVersion: 0.0.0\n")
        (project / "pytest.ini").write_text(
            "[pytest]\nfilterwarnings = ignore::pytest.PytestDeprecationWarning\n"
        )
        (project / "conftest.py").write_text(
            """import asyncio
import pytest


class FirstLoop(asyncio.SelectorEventLoop):
    pass


class SecondLoop(asyncio.SelectorEventLoop):
    pass


class FirstPolicy(asyncio.DefaultEventLoopPolicy):
    loop_type = FirstLoop

    def new_event_loop(self):
        return self.loop_type()


class SecondPolicy(asyncio.DefaultEventLoopPolicy):
    loop_type = SecondLoop

    def new_event_loop(self):
        return self.loop_type()


@pytest.fixture(scope="session", params=[FirstPolicy(), SecondPolicy()], ids=["first", "second"])
def event_loop_policy(request):
    return request.param
"""
        )
        (project / "test_policy.py").write_text(
            """import asyncio
import pytest


def test_plain():
    pass


@pytest.mark.asyncio
async def test_async(event_loop_policy):
    assert type(asyncio.get_running_loop()) is event_loop_policy.loop_type
"""
        )
        environment = os.environ.copy()
        environment["PYTHONPATH"] = os.pathsep.join(
            [str(checkout), str(dependencies), environment.get("PYTHONPATH", "")]
        )
        result = subprocess.run(
            [
                sys.executable,
                "-m",
                "pytest",
                "-p",
                "pytest_asyncio.plugin",
                "-q",
                "--asyncio-mode=strict",
            ],
            cwd=project,
            env=environment,
            text=True,
            capture_output=True,
        )
    output = result.stdout + result.stderr
    assert result.returncode == 0, output
    assert "3 passed" in output, output


if __name__ == "__main__":
    main()
