#!/usr/bin/env python3
"""Evaluate command-line normalization for pytest-xdist issue #1218."""
from __future__ import annotations

import importlib
import sys
from pathlib import Path
from types import ModuleType, SimpleNamespace


def _decorator(function=None, **_kwargs):
    if function is None:
        return lambda decorated: decorated
    return function


def install_pytest_stub() -> None:
    pytest = ModuleType("pytest")
    pytest.hookimpl = _decorator
    pytest.fixture = _decorator
    pytest.UsageError = type("UsageError", (Exception,), {})
    sys.modules["pytest"] = pytest


def config(*args: str, dist: str, numprocesses: int) -> SimpleNamespace:
    option = SimpleNamespace(
        dist=dist,
        distload=False,
        maxprocesses=None,
        numprocesses=numprocesses,
        tx=[],
    )
    return SimpleNamespace(
        option=option,
        invocation_params=SimpleNamespace(args=args),
        getoption=lambda name, default=None: False if name == "usepdb" else default,
        getvalue=lambda name: False,
    )


def normalize(plugin: object, *args: str, dist: str, numprocesses: int) -> SimpleNamespace:
    parsed = config(*args, dist=dist, numprocesses=numprocesses)
    plugin.pytest_cmdline_main(parsed)
    return parsed


def main() -> None:
    if len(sys.argv) != 2:
        raise SystemExit(f"usage: {Path(sys.argv[0]).name} REPOSITORY")
    repository = Path(sys.argv[1]).resolve()
    install_pytest_stub()
    sys.path.insert(0, str(repository / "src"))
    version = ModuleType("xdist._version")
    version.version = "0"
    sys.modules["xdist._version"] = version
    plugin = importlib.import_module("xdist.plugin")

    default = normalize(plugin, "-n", "2", dist="no", numprocesses=2)
    assert default.option.dist == "load", default.option.dist
    assert default.option.tx == ["popen", "popen"], default.option.tx

    worksteal = normalize(
        plugin, "--dist=worksteal", "-n", "2", dist="worksteal", numprocesses=2
    )
    assert worksteal.option.dist == "worksteal", worksteal.option.dist
    assert worksteal.option.tx == ["popen", "popen"], worksteal.option.tx

    disabled = normalize(plugin, "--dist=no", "-n", "2", dist="no", numprocesses=2)
    assert disabled.option.dist == "no", disabled.option.dist
    assert disabled.option.numprocesses == 0, disabled.option.numprocesses
    assert disabled.option.tx == [], disabled.option.tx

    separate_disabled = normalize(
        plugin, "--dist", "no", "-n", "2", dist="no", numprocesses=2
    )
    assert separate_disabled.option.dist == "no", separate_disabled.option.dist
    assert separate_disabled.option.numprocesses == 0, separate_disabled.option.numprocesses
    assert separate_disabled.option.tx == [], separate_disabled.option.tx

    zero = normalize(plugin, "--dist=worksteal", "-n", "0", dist="worksteal", numprocesses=0)
    assert zero.option.dist == "no", zero.option.dist
    assert zero.option.tx == [], zero.option.tx


if __name__ == "__main__":
    main()
