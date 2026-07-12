#!/usr/bin/env python3
"""Evaluator for attrs #1549 decorated defaults on inherited classes."""
import argparse
import sys
from pathlib import Path


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("repo", type=Path)
    args = parser.parse_args()
    sys.path.insert(0, str(args.repo / "src"))

    import attr

    @attr.define
    class Parent:
        value: str = attr.field()

        @value.default
        def _value_default(self) -> str:
            return "parent"

    @attr.define
    class Child(Parent):
        def _value_default(self) -> str:
            return "child"

    assert Parent().value == "parent"
    assert Child().value == "child", "an inherited decorated default ignored Child's override"
    assert Child("explicit").value == "explicit"


if __name__ == "__main__":
    main()
