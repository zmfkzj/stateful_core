#!/usr/bin/env python3
"""Evaluator for jsonschema issue #1257."""

from __future__ import annotations

import sys
from pathlib import Path


def best_match_for(checkout: Path, applicator: str):
    for parent in (checkout.parent, checkout.parent.parent):
        dependencies = parent / "jsonschema-deps"
        if dependencies.exists():
            sys.path.insert(0, str(dependencies))
    sys.path.insert(0, str(checkout))
    from jsonschema import Draft202012Validator, exceptions

    schema = {
        applicator: [
            {
                "properties": {
                    "run": {"type": "string"},
                },
                "required": ["run"],
            },
            {
                "properties": {
                    "uses": {"type": "string"},
                },
                "required": ["uses"],
            },
        ],
    }
    return exceptions.best_match(
        Draft202012Validator(schema).iter_errors({"run": 1, "uses": 1}),
    )


def main() -> None:
    checkout = Path(sys.argv[1]).resolve()
    for applicator in ("anyOf", "oneOf"):
        error = best_match_for(checkout, applicator)
        assert error.validator == applicator, (
            f"{applicator} alternatives tie; expected its summary error, "
            f"got {error.validator!r} at {list(error.path)!r}"
        )


if __name__ == "__main__":
    main()
