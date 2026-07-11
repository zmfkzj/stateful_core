#!/usr/bin/env python3
"""Evaluator for jsonschema issue #1511."""
import argparse
import decimal
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
    isoduration = types.ModuleType("isoduration")

    class DurationParsingException(Exception):
        pass

    def parse_duration(instance):
        if instance == "P1E1000000D":
            raise decimal.Overflow
        if instance != "P1D":
            raise DurationParsingException(instance)

    isoduration.DurationParsingException = DurationParsingException
    isoduration.parse_duration = parse_duration
    sys.modules.update(
        {
            "jsonschema": package,
            "jsonschema.exceptions": exceptions,
            "isoduration": isoduration,
        },
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
    checker = FormatChecker()

    checker.check("P1D", "duration")

    try:
        checker.check("not-a-duration", "duration")
    except FormatError as error:
        assert type(error.cause).__name__ == "DurationParsingException"
    else:
        raise AssertionError("malformed durations must raise FormatError")

    try:
        checker.check("P1E1000000D", "duration")
    except FormatError as error:
        assert isinstance(error.cause, decimal.Overflow)
    else:
        raise AssertionError("overflowing durations must raise FormatError")


if __name__ == "__main__":
    main()
