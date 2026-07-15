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
    searched: list[tuple[str, str]] = []

    @classmethod
    def compile(cls, pattern: str) -> object:
        cls.compiled.append(pattern)
        if pattern in {r"^[a-z]+$", r"\p{L}+"}:
            return object()
        raise RegexSyntaxError(pattern)

    @classmethod
    def search(cls, pattern: str, instance: str) -> object | None:
        cls.searched.append((pattern, instance))
        if pattern == r"\p{L}+" and instance == "café":
            return object()
        return None


class KeywordValidator:
    def __init__(self, format_checker, validation_error):
        self.format_checker = format_checker
        self.validation_error = validation_error

    def is_type(self, instance, type):
        return (type == "string" and isinstance(instance, str)) or (
            type == "object" and isinstance(instance, dict)
        )

    def descend(self, instance, schema, path, schema_path):
        if schema.get("type") == "integer" and not isinstance(instance, int):
            yield self.validation_error(f"{instance!r} is not an integer")


def load_modules(repo: Path):
    package = types.ModuleType("jsonschema")
    package.__path__ = [str(repo / "jsonschema")]
    exceptions = types.ModuleType("jsonschema.exceptions")

    class FormatError(Exception):
        def __init__(self, message, cause=None):
            super().__init__(message)
            self.cause = cause

    class ValidationError(Exception):
        pass

    exceptions.FormatError = FormatError
    exceptions.ValidationError = ValidationError
    sys.modules.update(
        {"jsonschema": package, "jsonschema.exceptions": exceptions},
    )

    def load(name):
        spec = importlib.util.spec_from_file_location(
            f"jsonschema.{name}", repo / "jsonschema" / f"{name}.py",
        )
        module = importlib.util.module_from_spec(spec)
        sys.modules[spec.name] = module
        assert spec.loader is not None
        spec.loader.exec_module(module)
        return module

    utils = load("_utils")
    format = load("_format")
    keywords = load("_keywords")
    return format.FormatChecker, FormatError, ValidationError, keywords, utils


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("repo", type=Path)
    args = parser.parse_args()
    FormatChecker, FormatError, ValidationError, keywords, _ = load_modules(args.repo)

    default = FormatChecker(formats=("regex",))
    assert default.conforms(r"^[a-z]+$", "regex")

    UnicodeRegex.compiled.clear()
    UnicodeRegex.searched.clear()
    checker = FormatChecker(formats=("regex",), regex=UnicodeRegex)
    checker.check(r"\p{L}+", "regex")
    assert UnicodeRegex.compiled == [r"\p{L}+"]

    validator = KeywordValidator(checker, ValidationError)
    assert list(keywords.pattern(validator, r"\p{L}+", "café", {})) == []
    assert UnicodeRegex.searched == [(r"\p{L}+", "café")]

    UnicodeRegex.searched.clear()
    schema = {
        "patternProperties": {r"\p{L}+": {"type": "integer"}},
        "additionalProperties": False,
    }
    assert list(keywords.patternProperties(validator, schema["patternProperties"], {"café": 1}, schema)) == []
    assert list(keywords.additionalProperties(validator, False, {"café": 1}, schema)) == []
    assert len(list(keywords.patternProperties(validator, schema["patternProperties"], {"café": "not an integer"}, schema))) == 1
    assert list(keywords.additionalProperties(validator, False, {"café": "not an integer"}, schema)) == []
    assert UnicodeRegex.searched == [(r"\p{L}+", "café")] * 4

    try:
        checker.check("[", "regex")
    except FormatError as error:
        assert isinstance(error.cause, RegexSyntaxError)
    else:
        raise AssertionError("the injected backend's syntax error was not retained")


if __name__ == "__main__":
    main()
