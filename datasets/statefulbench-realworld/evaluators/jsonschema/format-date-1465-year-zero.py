#!/usr/bin/env python3
"""Evaluator for jsonschema issue #1465."""
import argparse
from datetime import datetime
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

    rfc3339_validator = types.ModuleType("rfc3339_validator")

    def validate_rfc3339(instance):
        if instance.startswith("0000-"):
            return False
        if not instance.endswith("Z"):
            return False
        try:
            datetime.fromisoformat(instance.replace("Z", "+00:00"))
        except ValueError:
            return False
        return True

    exceptions.FormatError = FormatError
    rfc3339_validator.validate_rfc3339 = validate_rfc3339
    sys.modules.update(
        {
            "jsonschema": package,
            "jsonschema.exceptions": exceptions,
            "rfc3339_validator": rfc3339_validator,
        },
    )
    spec = importlib.util.spec_from_file_location(
        "jsonschema._format", repo / "jsonschema" / "_format.py",
    )
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    assert spec.loader is not None
    spec.loader.exec_module(module)
    return module.FormatChecker


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("repo", type=Path)
    args = parser.parse_args()
    FormatChecker = load_format_checker(args.repo)

    checker = FormatChecker(formats=("date-time",))
    assert checker.conforms("2024-02-29T12:34:56Z", "date-time")
    assert checker.conforms("0000-02-29T00:00:00Z", "date-time")
    assert not checker.conforms("0000-02-30T00:00:00Z", "date-time")
    assert not checker.conforms("0000-01-01T00:00:00", "date-time")
    seen = []
    custom = FormatChecker(formats=())

    @custom.checks("date-time")
    def custom_date_time(instance):
        seen.append(instance)
        return True

    custom.check("0000-02-29T00:00:00Z", "date-time")
    assert seen == ["0000-02-29T00:00:00Z"]


if __name__ == "__main__":
    main()
