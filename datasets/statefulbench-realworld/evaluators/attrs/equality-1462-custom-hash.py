#!/usr/bin/env python3
"""Evaluate safe custom-equality hashing for attrs issue #1462."""

from __future__ import annotations

import sys
from pathlib import Path


def main(checkout: str) -> None:
    sys.path.insert(0, str(Path(checkout) / "src"))

    from attrs import define, field

    ne_marker = object()

    @define(frozen=True)
    class SafeValue:
        value: str = field(eq=str.casefold)

        def __eq__(self, other):
            return self.__attrs_eq__(other)

        def __ne__(self, other):
            return other is ne_marker

    same = SafeValue("One")
    assert same == SafeValue("one")
    assert same.__attrs_eq__(object()) is NotImplemented
    assert same != ne_marker
    assert SafeValue.__hash__ is None
    try:
        hash(same)
    except TypeError:
        pass
    else:
        raise AssertionError("custom equality must remain unhashable by default")

    @define(frozen=True, unsafe_hash=True)
    class ExplicitlyHashable:
        value: str = field(eq=str.casefold)

        def __eq__(self, other):
            return self.__attrs_eq__(other)

    left = ExplicitlyHashable("One")
    right = ExplicitlyHashable("one")
    assert left == right
    assert hash(left) == hash(right)


if __name__ == "__main__":
    main(sys.argv[1])
