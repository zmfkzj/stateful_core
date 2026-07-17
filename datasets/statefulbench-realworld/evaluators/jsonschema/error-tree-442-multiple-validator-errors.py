#!/usr/bin/env python3
"""Evaluator for jsonschema issue #442's ErrorTree error preservation."""
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

    from jsonschema import exceptions

    first = exceptions.ValidationError("first", validator="anyOf", instance="x")
    second = exceptions.ValidationError("second", validator="anyOf", instance="x")
    tree = exceptions.ErrorTree([first, second])
    assert tree.errors["anyOf"] is second
    assert tree.all_errors["anyOf"] == (first, second)
    assert tree.total_errors == 2

    minimum = exceptions.ValidationError("minimum", validator="minimum", instance=3)
    mixed = exceptions.ErrorTree([first, minimum])
    assert mixed.errors == {"anyOf": first, "minimum": minimum}
    assert mixed.all_errors == {
        "anyOf": (first,),
        "minimum": (minimum,),
    }

    nested_first = exceptions.ValidationError(
        "nested first", validator="type", path=["items", 0], instance="x",
    )
    nested_second = exceptions.ValidationError(
        "nested second", validator="type", path=["items", 0], instance="x",
    )
    nested = exceptions.ErrorTree([nested_first, nested_second])
    assert nested["items"][0].all_errors["type"] == (
        nested_first,
        nested_second,
    )
    assert nested.total_errors == 2


if __name__ == "__main__":
    main()
