#!/usr/bin/env python3
"""Evaluator for attrs issue #1288: cached properties and child __getattr__."""

from __future__ import annotations

import sys
from functools import cached_property
from pathlib import Path


def main() -> None:
    checkout = Path(sys.argv[1]).resolve()
    sys.path.insert(0, str(checkout / "src"))

    from attr import define

    calls = []

    @define
    class Parent:
        @cached_property
        def value(self):
            calls.append("computed")
            return 3

    class Child(Parent):
        def __getattr__(self, name):
            raise AttributeError(f"child fallback for {name}")

    child = Child()
    assert child.value == 3
    assert child.value == 3
    assert calls == ["computed"]
    try:
        child.unknown
    except AttributeError as error:
        assert str(error) == "child fallback for unknown"
    else:
        raise AssertionError("child __getattr__ was not preserved for unknown names")

    @define
    class Grandparent:
        @cached_property
        def label(self):
            return "grandparent"

    @define
    class Descendant(Grandparent):
        @cached_property
        def label(self):
            return super().label + ":descendant"

    assert Descendant().label == "grandparent:descendant"


if __name__ == "__main__":
    main()
