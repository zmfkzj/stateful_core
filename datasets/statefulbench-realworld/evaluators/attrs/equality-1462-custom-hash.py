#!/usr/bin/env python3
"""Evaluate opt-in identity hashing for custom equality in #1462."""

from __future__ import annotations

import sys
from pathlib import Path


def main(checkout: str) -> None:
    sys.path.insert(0, str(Path(checkout) / "src"))

    from attrs import define

    @define(frozen=True)
    class C:
        x: int

        def __eq__(self, value):
            return self.x in (0, value)

    zero = C(0)
    assert zero == C(1)
    assert C.__hash__ is None
    try:
        hash(zero)
    except TypeError:
        pass
    else:
        raise AssertionError("custom value equality must remain unhashable by default")

    @define(frozen=True, unsafe_identity_hash=True)
    class Identity:
        x: int

        def __eq__(self, other):
            return self is other

    first = Identity(1)
    second = Identity(1)
    assert first == first
    assert first != second
    assert Identity.__hash__ is object.__hash__
    hash(first)

    try:

        @define(frozen=True, unsafe_identity_hash=True)
        class MissingCustomEquality:
            x: int

        raise AssertionError("identity hashing requires custom equality")
    except TypeError:
        pass

    try:

        @define(
            frozen=True, unsafe_hash=True, unsafe_identity_hash=True
        )
        class ConflictingHash:
            x: int

        raise AssertionError("conflicting hash strategies must be rejected")
    except TypeError:
        pass


if __name__ == "__main__":
    main(sys.argv[1])
