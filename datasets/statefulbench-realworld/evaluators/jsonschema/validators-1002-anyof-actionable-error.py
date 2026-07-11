#!/usr/bin/env python3
"""Evaluator for jsonschema issue #1002."""

from __future__ import annotations

import sys
from pathlib import Path


def main() -> None:
    checkout = Path(sys.argv[1]).resolve()
    for parent in (checkout.parent, checkout.parent.parent):
        dependencies = parent / "jsonschema-deps"
        if dependencies.exists():
            sys.path.insert(0, str(dependencies))
    sys.path.insert(0, str(checkout))

    from jsonschema import Draft202012Validator, exceptions

    schema = {
        "anyOf": [
            {
                "properties": {
                    "version": {"const": 1},
                    "description": {"type": "string"},
                },
                "required": ["version", "description"],
            },
            {
                "properties": {
                    "version": {"const": 2},
                    "details": {
                        "properties": {
                            "settings": {"minProperties": 2},
                        },
                    },
                },
                "required": ["version", "details"],
            },
        ],
    }
    error = exceptions.best_match(
        Draft202012Validator(schema).iter_errors(
            {"version": 1, "description": 0, "details": {"settings": {}}},
        ),
    )

    assert error.validator == "type", (
        "anyOf should surface the v1 description type error rather than "
        f"the generic anyOf summary or an unrelated v2 error, got "
        f"{error.validator!r} at {list(error.path)!r}"
    )
    assert list(error.path) == ["description"]
    assert error.instance == 0


if __name__ == "__main__":
    main()
