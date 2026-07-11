#!/usr/bin/env python3
"""Evaluator for jsonschema issue #1142."""
import argparse
import importlib.util
import sys
import types
from pathlib import Path


class RegexSyntaxError(Exception):
    pass


class UnicodeRegex:
    error = RegexSyntaxError
    compiled: list[str] = []

    @classmethod
    def compile(cls, pattern: str) -> object:
        cls.compiled.append(pattern)
        if pattern in {r"^[a-z]+$", r"\p{L}+"}:
            return object()
        raise RegexSyntaxError(pattern)


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

    default = FormatChecker(formats=("regex",))
    assert default.conforms(r"^[a-z]+$", "regex")
    assert not default.conforms(r"\p{L}+", "regex")

    UnicodeRegex.compiled.clear()
    checker = FormatChecker(formats=("regex",), regex=UnicodeRegex)
    checker.check(r"^[a-z]+$", "regex")
    checker.check(r"\p{L}+", "regex")
    assert UnicodeRegex.compiled == [r"^[a-z]+$", r"\p{L}+"]

    with_exception = False
    try:
        checker.check("[", "regex")
    except FormatError as error:
        with_exception = isinstance(error.cause, RegexSyntaxError)
    assert with_exception


if __name__ == "__main__":
    main()
