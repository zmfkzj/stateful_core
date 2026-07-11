#!/usr/bin/env python3
"""Evaluator for Jsonschema issue #191 schema/path consistency."""

import argparse
import sys
from collections import deque
from pathlib import Path


def first_error(schema, instance):
    from jsonschema import Draft202012Validator

    return next(Draft202012Validator(schema).iter_errors(instance))


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("repo", type=Path)
    args = parser.parse_args()
    sys.path.insert(0, str(args.repo))

    # Normal: a nested error names a path relative to its own subschema.
    schema = {
        "properties": {
            "debug": {
                "properties": {
                    "foo": {
                        "properties": {
                            "bar": {
                                "additionalProperties": False,
                                "type": "object",
                            },
                        },
                    },
                },
            },
        },
    }
    error = first_error(
        schema,
        {"debug": {"foo": {"bar": {"unexpected": "value"}}}},
    )
    assert error.schema is schema["properties"]["debug"]["properties"]["foo"]["properties"]["bar"]
    assert error.relative_schema_path == deque(["additionalProperties"]), error.relative_schema_path
    assert error.schema_path == deque([
        "properties", "debug", "properties", "foo", "properties", "bar", "additionalProperties",
    ]), error.schema_path
    assert error.absolute_schema_path == error.schema_path
    assert "Failed validating 'additionalProperties' in schema:" in str(error)

    # Boundary: root-schema errors retain identical relative and absolute paths.
    root = {"additionalProperties": False}
    root_error = first_error(root, {"unexpected": 1})
    assert root_error.schema is root
    assert root_error.relative_schema_path == deque(["additionalProperties"])
    assert root_error.schema_path == deque(["additionalProperties"])
    assert root_error.absolute_schema_path == deque(["additionalProperties"])

    # Error context: a nested anyOf failure keeps its local path while retaining
    # the full public schema_path and absolute_schema_path.
    any_of = {"anyOf": [{"type": "integer"}]}
    parent = first_error(any_of, "secret")
    child = parent.context[0]
    assert child.schema is any_of["anyOf"][0]
    assert child.relative_schema_path == deque(["type"]), child.relative_schema_path
    assert child.schema_path == deque([0, "type"]), child.schema_path
    assert child.absolute_schema_path == deque(["anyOf", 0, "type"])


if __name__ == "__main__":
    main()
