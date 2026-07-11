#!/usr/bin/env python3
"""Evaluator for jsonschema issue #1496."""
import argparse
import importlib.util
import sys
import types
from pathlib import Path


def load_format_checker(repo: Path):
    package = types.ModuleType("jsonschema")
    package.__path__ = [str(repo / "jsonschema")]
    exceptions = types.ModuleType("jsonschema.exceptions")

    class FormatError(Exception):
        def __init__(self, message, cause=None):
            super().__init__(message)
            self.cause = cause

    exceptions.FormatError = FormatError
    sys.modules.update(
        {"jsonschema": package, "jsonschema.exceptions": exceptions},
    )
    spec = importlib.util.spec_from_file_location(
        "jsonschema._format", repo / "jsonschema" / "_format.py",
    )
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    assert spec.loader is not None
    spec.loader.exec_module(module)
    return module.FormatChecker, FormatError


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("repo", type=Path)
    args = parser.parse_args()
    FormatChecker, FormatError = load_format_checker(args.repo)
    checker = FormatChecker(formats=())
    calls = []

    @checker.checks("composed")
    def first(instance):
        calls.append(("first", instance))
        return instance.startswith("ok")

    @checker.checks("composed")
    def second(instance):
        calls.append(("second", instance))
        return instance.endswith("!")

    checker.check("ok!", "composed")
    assert calls == [("first", "ok!"), ("second", "ok!")]

    calls.clear()
    try:
        checker.check("bad!", "composed")
    except FormatError:
        pass
    else:
        raise AssertionError("the first registered checker must reject bad!")
    assert calls == [("first", "bad!")]

    calls.clear()
    try:
        checker.check("ok?", "composed")
    except FormatError:
        pass
    else:
        raise AssertionError("the second registered checker must reject ok?")
    assert calls == [("first", "ok?"), ("second", "ok?")]

    checker.check("anything", "unknown")


if __name__ == "__main__":
    main()
