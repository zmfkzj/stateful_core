#!/usr/bin/env python3
"""Evaluator for the explicit #1335 init-subclass keyword extension."""
import argparse
import sys
from pathlib import Path


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("repo", type=Path)
    args = parser.parse_args()
    sys.path.insert(0, str(args.repo / "src"))

    import attr

    class ConfiguredBase:
        calls = []

        @classmethod
        def __attrs_init_subclass__(cls, *, role: str, active: bool = True) -> None:
            cls.role = role
            cls.active = active
            ConfiguredBase.calls.append((cls, role, active))

    @attr.define
    class ConfiguredChild(ConfiguredBase):
        __attrs_init_subclass_kwargs__ = {"role": "worker", "active": False}

    assert ConfiguredChild.role == "worker"
    assert ConfiguredChild.active is False
    assert ConfiguredBase.calls == [(ConfiguredChild, "worker", False)]

    class LegacyBase:
        calls = []

        @classmethod
        def __attrs_init_subclass__(cls) -> None:
            LegacyBase.calls.append(cls)

    @attr.define
    class LegacyChild(LegacyBase):
        pass

    assert LegacyBase.calls == [LegacyChild]


if __name__ == "__main__":
    main()
