#!/usr/bin/env python3
"""Verify that field extensions may subclass attrs' counting descriptor."""

import sys
from pathlib import Path


def main(repo: Path) -> None:
    sys.path.insert(0, str(repo / "src"))

    import attrs
    from attr._make import _CountingAttr

    class ExtendedCountingAttr(_CountingAttr):
        def __init__(self, counting_attr):
            for name in counting_attr.__slots__:
                setattr(self, name, getattr(counting_attr, name))
            _CountingAttr.cls_counter -= 1
            self.counter -= 1

    def extended_field(**kwargs):
        return ExtendedCountingAttr(attrs.field(**kwargs))

    @attrs.define
    class Example:
        value: int = extended_field(default=42, metadata={"extension": "ok"})

    attribute = attrs.fields(Example).value
    assert attribute.default == 42, attribute.default
    assert attribute.metadata["extension"] == "ok"
    assert Example().value == 42
    @attrs.define(auto_attribs=False)
    class Legacy:
        value = extended_field(default=7)

    assert attrs.fields(Legacy).value.default == 7
    assert Legacy().value == 7


if __name__ == "__main__":
    main(Path(sys.argv[1]).resolve())
