#!/usr/bin/env python3
"""Evaluate command-line normalization for pytest-xdist issue #1221."""
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


def config(numprocesses: str, *, usepdb: bool = False, workers: int = 4) -> SimpleNamespace:
    option = SimpleNamespace(
        dist="no",
        distload=False,
        maxprocesses=None,
        numprocesses=numprocesses,
        tx=[],
    )
    auto_worker_calls: list[object] = []

    def auto_num_workers(config: object) -> int:
        auto_worker_calls.append(config)
        return workers

    hook = SimpleNamespace(pytest_xdist_auto_num_workers=auto_num_workers)
    return SimpleNamespace(
        option=option,
        hook=hook,
        invocation_params=SimpleNamespace(args=("-n", numprocesses)),
        getoption=lambda name, default=None: usepdb if name == "usepdb" else default,
        getvalue=lambda name: False,
        auto_worker_calls=auto_worker_calls,
    )


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

    assert plugin.parse_numprocesses("auto-1") == "auto-1"
    for invalid in ("auto-2", "auto--1", "auto-one"):
        try:
            plugin.parse_numprocesses(invalid)
        except ValueError:
            pass
        else:
            raise AssertionError(f"{invalid!r} must be rejected")

    four_cpus = config("auto-1", workers=4)
    plugin.pytest_cmdline_main(four_cpus)
    assert four_cpus.option.numprocesses == 3, four_cpus.option.numprocesses
    assert four_cpus.option.dist == "load", four_cpus.option.dist
    assert four_cpus.option.tx == ["popen"] * 3, four_cpus.option.tx
    assert four_cpus.auto_worker_calls == [four_cpus], four_cpus.auto_worker_calls

    one_cpu = config("auto-1", workers=1)
    plugin.pytest_cmdline_main(one_cpu)
    assert one_cpu.option.numprocesses == 0, one_cpu.option.numprocesses
    assert one_cpu.option.dist == "no", one_cpu.option.dist
    assert one_cpu.option.tx == [], one_cpu.option.tx
    assert one_cpu.auto_worker_calls == [one_cpu], one_cpu.auto_worker_calls

    pdb = config("auto-1", usepdb=True)
    pdb.hook.pytest_xdist_auto_num_workers = lambda config: (_ for _ in ()).throw(
        AssertionError("auto detection must not run with --pdb")
    )
    plugin.pytest_cmdline_main(pdb)
    assert pdb.option.numprocesses == 0, pdb.option.numprocesses
    assert pdb.option.dist == "no", pdb.option.dist


if __name__ == "__main__":
    main()
