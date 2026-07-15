#!/usr/bin/env python3
"""Evaluator for attrs issue #1532."""
from __future__ import annotations

from functools import cached_property
from pathlib import Path
import sys


checkout = Path(sys.argv[1]).resolve()
sys.path.insert(0, str(checkout / "src"))

import attr


@attr.define(slots=True)
class Parent:
    regular: int = 3

    @cached_property
    def parent_value(self) -> int:
        return self.regular + 1


@attr.define(slots=True)
class Child(Parent):
    @cached_property
    def child_value(self) -> int:
        return self.regular + 2


parent_properties = Parent.__attrs_cached_properties__
child_properties = Child.__attrs_cached_properties__

assert set(parent_properties) == {"parent_value"}
assert set(child_properties) == {"child_value"}
assert callable(parent_properties["parent_value"])
assert callable(child_properties["child_value"])
assert parent_properties["parent_value"](Parent()) == 4
assert child_properties["child_value"](Child()) == 5

child = Child()
assert child.parent_value == 4
assert child.child_value == 5
assert child.parent_value == 4
assert child.child_value == 5
