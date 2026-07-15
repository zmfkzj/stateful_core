#!/usr/bin/env python3
"""Evaluator for attrs issue #1294: per-slot documentation."""

from __future__ import annotations

import inspect
import sys
from pathlib import Path


def main() -> None:
    checkout = Path(sys.argv[1]).resolve()
    sys.path.insert(0, str(checkout / "src"))

    import attr

    @attr.s(slots=True)
    class Documented:
        documented = attr.ib(metadata={"slot_doc": "A documented slot."})
        ordinary = attr.ib()

    documented_slot = Documented.__dict__["documented"]
    ordinary_slot = Documented.__dict__["ordinary"]
    assert inspect.getdoc(documented_slot) == "A documented slot."
    assert inspect.getdoc(ordinary_slot) is None

    instance = Documented(1, 2)
    assert (instance.documented, instance.ordinary) == (1, 2)
    instance.documented = 3
    assert instance.documented == 3


if __name__ == "__main__":
    main()
