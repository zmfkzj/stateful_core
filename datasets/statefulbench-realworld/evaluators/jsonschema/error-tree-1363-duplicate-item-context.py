#!/usr/bin/env python3
"""Evaluator for jsonschema feature #1363's ErrorTree item context API."""
import argparse
import sys
from pathlib import Path


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("repo", type=Path)
    args = parser.parse_args()
    for root in (args.repo.parent, args.repo.parent.parent):
        deps = root / "jsonschema-deps"
        if deps.is_dir():
            sys.path.insert(0, str(deps))
    sys.path.insert(0, str(args.repo))

    from jsonschema import Draft202012Validator, exceptions

    validator = Draft202012Validator({"uniqueItems": True})
    tree = exceptions.ErrorTree(validator.iter_errors(["red", "blue", "red"]))
    assert tree.duplicate_item_contexts == {
        "uniqueItems": [(0, 2, "red")],
    }

    repeated = exceptions.ErrorTree(validator.iter_errors([1, 1, 1]))
    assert repeated.duplicate_item_contexts["uniqueItems"] == [
        (0, 1, 1),
        (0, 2, 1),
    ]

    nested_validator = Draft202012Validator(
        {"properties": {"items": {"uniqueItems": True}}},
    )
    nested = exceptions.ErrorTree(
        nested_validator.iter_errors({"items": [True, 1, True]}),
    )
    assert nested["items"].duplicate_item_contexts == {
        "uniqueItems": [(0, 2, True)],
    }

    valid = exceptions.ErrorTree(validator.iter_errors([1, 2, 3]))
    assert valid.duplicate_item_contexts == {}


if __name__ == "__main__":
    main()
