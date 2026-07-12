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
    # Every draft's context children are local to their own schemas. Their
    # legacy schema paths and root-relative absolute paths are unchanged.
    from jsonschema import (
        Draft201909Validator,
        Draft202012Validator,
        Draft3Validator,
        Draft4Validator,
        Draft6Validator,
        Draft7Validator,
    )

    for Validator, child_schema, absolute_path in (
        (
            Draft202012Validator,
            {"anyOf": [{"type": "integer"}]},
            deque(["anyOf", 0, "type"]),
        ),
        (
            Draft201909Validator,
            {"anyOf": [{"type": "integer"}]},
            deque(["anyOf", 0, "type"]),
        ),
        (
            Draft7Validator,
            {"anyOf": [{"type": "integer"}]},
            deque(["anyOf", 0, "type"]),
        ),
        (
            Draft6Validator,
            {"anyOf": [{"type": "integer"}]},
            deque(["anyOf", 0, "type"]),
        ),
        (
            Draft4Validator,
            {"anyOf": [{"type": "integer"}]},
            deque(["anyOf", 0, "type"]),
        ),
        (
            Draft3Validator,
            {"type": [{"type": "integer"}]},
            deque(["type", 0, "type"]),
        ),
    ):
        parent = next(Validator(child_schema).iter_errors("secret"))
        child = parent.context[0]
        assert child.relative_schema_path == deque(["type"])
        assert child.schema_path == deque([0, "type"])
        assert child.absolute_schema_path == absolute_path

    # Every draft also keeps errors yielded through a $ref local to their
    # referenced schemas. The public paths continue to name the reference
    # site, which is the root-relative location callers use.
    for Validator, definitions_key in (
        (Draft202012Validator, "$defs"),
        (Draft201909Validator, "$defs"),
        (Draft7Validator, "definitions"),
        (Draft6Validator, "definitions"),
        (Draft4Validator, "definitions"),
        (Draft3Validator, "definitions"),
    ):
        ref_schema = {
            definitions_key: {"foo": {"type": "integer"}},
            "properties": {
                "aprop": {
                    "$ref": f"#/{definitions_key}/foo",
                    "type": "string",
                },
            },
        }
        errors = list(Validator(ref_schema).iter_errors({"aprop": None}))
        target_error = errors[0]
        assert target_error.schema is ref_schema[definitions_key]["foo"]
        assert target_error.relative_schema_path == deque(["type"])
        assert target_error.schema_path == deque(["properties", "aprop", "type"])
        assert target_error.absolute_schema_path == target_error.schema_path
        if Validator is Draft202012Validator:
            assert len(errors) == 2
            inline_error = errors[1]
            assert inline_error.schema is ref_schema["properties"]["aprop"]
            assert inline_error.relative_schema_path == deque(["type"])
            assert inline_error.schema_path == deque(
                ["properties", "aprop", "type"],
            )
            assert inline_error.absolute_schema_path == inline_error.schema_path


if __name__ == "__main__":
    main()
