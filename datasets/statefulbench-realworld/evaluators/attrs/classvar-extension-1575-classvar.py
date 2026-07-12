#!/usr/bin/env python3
"""Verify that unresolved ClassVar annotations stay out of attrs classes."""

import sys
from pathlib import Path


def main(repo: Path) -> None:
    sys.path.insert(0, str(repo / "src"))

    import attrs

    namespace = {"attrs": attrs}
    exec(
        "from typing import TYPE_CHECKING\n"
        "if TYPE_CHECKING:\n"
        "    from typing import ClassVar\n"
        "@attrs.define\n"
        "class Example:\n"
        "    label: ClassVar[str]\n",
        namespace,
    )
    example = namespace["Example"]

    assert attrs.fields(example) == (), attrs.fields(example)
    assert example() is not None
    @attrs.define
    class Normal:
        value: int

    assert Normal(3).value == 3


if __name__ == "__main__":
    main(Path(sys.argv[1]).resolve())
