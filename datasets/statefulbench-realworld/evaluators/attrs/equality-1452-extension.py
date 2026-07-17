#!/usr/bin/env python3
"""Evaluate generated equality extension for attrs issue #1452."""

from __future__ import annotations

import sys
from pathlib import Path


def main(checkout: str) -> None:
    sys.path.insert(0, str(Path(checkout) / "src"))

    from attrs import define, field

    magic = object()

    @define
    class Label:
        value: str = field(eq=str.casefold)
        ignored: int = field(eq=False)

        def __eq__(self, other):
            if other is magic:
                return True
            return self.__attrs_eq__(other)

    assert Label("One", 1) == Label("one", 2)
    assert Label("One", 1) != Label("two", 1)
    assert Label("One", 1) == magic
    assert Label("One", 1).__attrs_eq__(object()) is NotImplemented
    assert Label.__hash__ is None

    @define(order=True)
    class Ordered:
        value: int

    assert Ordered(1) == Ordered(1)
    assert Ordered(1) < Ordered(2)
    assert Ordered(1).__attrs_eq__(Ordered(1))


if __name__ == "__main__":
    main(sys.argv[1])
