#!/usr/bin/env python3
"""Evaluate command-line normalization for pytest-xdist issue #1218."""
from __future__ import annotations

import argparse
import configparser
import importlib
import os
import shlex
import sys
import tempfile
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


class _OptionGroup:
    def __init__(self, parser: argparse.ArgumentParser) -> None:
        self.parser = parser

    def addoption(self, *options: str, **kwargs: object) -> None:
        if "--dist" in options or "-n" in options:
            self.parser.add_argument(*options, **kwargs)

    _addoption = addoption


class _Parser:
    def __init__(self) -> None:
        self.options = argparse.ArgumentParser()
        self.group = _OptionGroup(self.options)

    def getgroup(self, _name: str, _description: str) -> _OptionGroup:
        return self.group


    def addini(self, *_args: object, **_kwargs: object) -> None:
        pass

def parsed_xdist_options(plugin: object, args: tuple[str, ...]) -> argparse.Namespace:
    parser = _Parser()
    plugin.pytest_addoption(parser)
    return parser.options.parse_args(args)


def env_addopts_args() -> tuple[str, ...]:
    return tuple(shlex.split(os.environ["PYTEST_ADDOPTS"]))


def ini_addopts_args() -> tuple[str, ...]:
    with tempfile.TemporaryDirectory() as directory:
        ini_path = Path(directory) / "pytest.ini"
        ini_path.write_text("[pytest]\naddopts = -n 2 --dist=no\n")
        ini = configparser.ConfigParser()
        ini.read(ini_path)
        return tuple(shlex.split(ini["pytest"]["addopts"]))


def config(
    *, dist: str, numprocesses: int, explicit_dist: bool = False
) -> SimpleNamespace:
    option = SimpleNamespace(
        dist=dist,
        distload=False,
        maxprocesses=None,
        numprocesses=numprocesses,
        tx=[],
        _xdist_explicit_dist=explicit_dist,
    )
    return SimpleNamespace(
        option=option,
        invocation_params=SimpleNamespace(args=()),
        getoption=lambda name, default=None: False if name == "usepdb" else default,
        getvalue=lambda name: False,
    )


def normalize(
    plugin: object, *, dist: str, numprocesses: int, explicit_dist: bool = False
) -> SimpleNamespace:
    parsed = config(
        dist=dist, numprocesses=numprocesses, explicit_dist=explicit_dist
    )
    plugin.pytest_cmdline_main(parsed)
    return parsed

def normalize_source(plugin: object, args: tuple[str, ...]) -> SimpleNamespace:
    parsed = parsed_xdist_options(plugin, args)
    return normalize(
        plugin,
        dist=parsed.dist,
        numprocesses=parsed.numprocesses,
        explicit_dist=getattr(parsed, "_xdist_explicit_dist", False),
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

    default = normalize(plugin, dist="no", numprocesses=2)
    assert default.option.dist == "load", default.option.dist
    assert default.option.tx == ["popen", "popen"], default.option.tx

    worksteal = normalize_source(plugin, ("-n", "2", "--dist=worksteal"))
    assert worksteal.option.dist == "worksteal", worksteal.option.dist
    assert worksteal.option.tx == ["popen", "popen"], worksteal.option.tx

    disabled = normalize(
        plugin, dist="no", numprocesses=2, explicit_dist=True
    )
    assert disabled.option.dist == "no", disabled.option.dist
    assert disabled.option.numprocesses == 0, disabled.option.numprocesses
    assert disabled.option.tx == [], disabled.option.tx

    separate_disabled = normalize(
        plugin, dist="no", numprocesses=2, explicit_dist=True
    )
    assert separate_disabled.option.dist == "no", separate_disabled.option.dist
    assert separate_disabled.option.numprocesses == 0, separate_disabled.option.numprocesses
    assert separate_disabled.option.tx == [], separate_disabled.option.tx

    zero = normalize(
        plugin, dist="worksteal", numprocesses=0, explicit_dist=True
    )
    assert zero.option.dist == "no", zero.option.dist
    assert zero.option.tx == [], zero.option.tx
    cli_disabled = normalize_source(plugin, ("-n", "2", "--dist", "no"))
    previous_addopts = os.environ.get("PYTEST_ADDOPTS")
    os.environ["PYTEST_ADDOPTS"] = "-n 2 --dist=no"
    try:
        env_disabled = normalize_source(plugin, env_addopts_args())
    finally:
        if previous_addopts is None:
            del os.environ["PYTEST_ADDOPTS"]
        else:
            os.environ["PYTEST_ADDOPTS"] = previous_addopts
    ini_disabled = normalize_source(plugin, ini_addopts_args())
    for source in (cli_disabled, env_disabled, ini_disabled):
        assert source.option.dist == "no", source.option.dist
        assert source.option.numprocesses == 0, source.option.numprocesses
        assert source.option.tx == [], source.option.tx


if __name__ == "__main__":
    main()
