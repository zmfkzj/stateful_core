#!/usr/bin/env python3
"""Evaluator for attrs issue #1333."""
from __future__ import annotations

from functools import cached_property
from pathlib import Path
import sys


checkout = Path(sys.argv[1]).resolve()
sys.path.insert(0, str(checkout / "src"))

import attr


parent_calls = 0
child_calls = 0


@attr.define(slots=True)
class Parent:
    @cached_property
    def name(self) -> str:
        global parent_calls
        parent_calls += 1
        return "Alice"


@attr.define(slots=True)
class Child(Parent):
    @cached_property
    def name(self) -> str:
        global child_calls
        child_calls += 1
        return f"Bob (son of {super().name})"


parent = Parent()
child = Child()

assert parent.name == "Alice"
assert parent.name == "Alice"
assert child.name == "Bob (son of Alice)"
assert child.name == "Bob (son of Alice)"
assert parent_calls == 2, parent_calls
assert child_calls == 1, child_calls
