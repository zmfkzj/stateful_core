#!/usr/bin/env python3
"""Check the configured event-loop factory API."""
from __future__ import annotations

import os
from pathlib import Path
import subprocess
import sys
import tempfile


def run_pytest(project: Path, checkout: Path) -> subprocess.CompletedProcess[str]:
    curation_root = checkout.parent.parent if checkout.name == "policy" else checkout.parent
    test_dependencies = curation_root / "pytest-asyncio-deps"
    if not test_dependencies.exists():
        raise RuntimeError(f"missing pytest dependency bundle: {test_dependencies}")
    environment = os.environ.copy()
    environment["PYTHONPATH"] = os.pathsep.join(
        [str(checkout), str(test_dependencies), environment.get("PYTHONPATH", "")]
    )
    return subprocess.run(
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


def main() -> None:
    checkout = Path(sys.argv[1]).resolve()
    curation_root = checkout.parent.parent if checkout.name == "policy" else checkout.parent
    with tempfile.TemporaryDirectory(
        dir=curation_root / "pytest-asyncio-pairs" / "policy"
    ) as temporary:
        project = Path(temporary)
        metadata = project / "pytest_asyncio-0.0.0.dist-info"
        metadata.mkdir()
        (metadata / "METADATA").write_text("Name: pytest-asyncio\nVersion: 0.0.0\n")
        (project / "loop_config.py").write_text(
            """import asyncio


class ConfiguredLoop(asyncio.SelectorEventLoop):
    pass


def make_loop():
    return ConfiguredLoop()


not_a_factory = object()
"""
        )
        (project / "pytest.ini").write_text(
            """[pytest]
asyncio_loop_factory = loop_config:make_loop
"""
        )
        (project / "test_loop_config.py").write_text(
            """import asyncio
import pytest


@pytest.mark.asyncio
async def test_uses_configured_loop_factory():
    assert type(asyncio.get_running_loop()).__name__ == \"ConfiguredLoop\"
"""
        )
        configured = run_pytest(project, checkout)
        assert configured.returncode == 0, configured.stdout + configured.stderr
        configured_output = configured.stdout + configured.stderr
        assert "1 passed" in configured_output, configured_output
        assert "Unknown config option" not in configured_output, configured_output
        (project / "conftest.py").write_text(
            """import asyncio
import pytest


class OverrideLoop(asyncio.SelectorEventLoop):
    pass


class OverridePolicy(asyncio.DefaultEventLoopPolicy):
    def new_event_loop(self):
        return OverrideLoop()


@pytest.fixture(scope="session")
def event_loop_policy():
    return OverridePolicy()
"""
        )
        (project / "test_loop_config.py").write_text(
            """import asyncio
import pytest


@pytest.mark.asyncio
async def test_user_policy_override_is_preserved():
    assert type(asyncio.get_running_loop()).__name__ == "OverrideLoop"
"""
        )
        overridden = run_pytest(project, checkout)
        assert overridden.returncode == 0, overridden.stdout + overridden.stderr
        (project / "conftest.py").unlink()

        (project / "pytest.ini").write_text(
            """[pytest]
asyncio_loop_factory = loop_config:missing_factory
"""
        )
        invalid = run_pytest(project, checkout)
        invalid_output = invalid.stdout + invalid.stderr
        assert invalid.returncode != 0, invalid_output
        assert "asyncio_loop_factory" in invalid_output, invalid_output
        assert "missing_factory" in invalid_output, invalid_output

        (project / "pytest.ini").write_text(
            """[pytest]
asyncio_loop_factory = loop_config:not_a_factory
"""
        )
        noncallable = run_pytest(project, checkout)
        noncallable_output = noncallable.stdout + noncallable.stderr
        assert noncallable.returncode != 0, noncallable_output
        assert "asyncio_loop_factory" in noncallable_output, noncallable_output
        assert "not callable" in noncallable_output, noncallable_output

        (project / "pytest.ini").write_text(
            """[pytest]
asyncio_loop_factory = loop_config
"""
        )
        malformed = run_pytest(project, checkout)
        malformed_output = malformed.stdout + malformed.stderr
        assert malformed.returncode != 0, malformed_output
        assert "asyncio_loop_factory" in malformed_output, malformed_output
        assert "module:callable" in malformed_output, malformed_output


if __name__ == "__main__":
    main()
