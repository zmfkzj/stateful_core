#!/usr/bin/env python3
"""Evaluator for Pendulum issue #817."""
import argparse
from datetime import timedelta
import sys
import types
from pathlib import Path


def load_pendulum(repo: Path):
    dateutil = types.ModuleType("dateutil")
    dateutil.parser = types.ModuleType("dateutil.parser")
    sys.modules.update({"dateutil": dateutil, "dateutil.parser": dateutil.parser})
    sys.path.insert(0, str(repo / "src"))
    import pendulum

    return pendulum


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("repo", type=Path)
    args = parser.parse_args()
    pendulum = load_pendulum(args.repo)

    start = pendulum.datetime(
        2022, 11, 1, 9, 0, 0, 123456, tz="America/Los_Angeles"
    )
    native = start + timedelta(days=6, seconds=5, microseconds=7)
    duration = start + pendulum.duration(days=6, seconds=5, microseconds=7)
    assert native == duration
    assert (native.hour, native.second, native.microsecond) == (9, 5, 123463)
    assert native.timezone_name == "America/Los_Angeles"
    fall_back = pendulum.datetime(2022, 11, 6, 1, tz="America/Los_Angeles")
    negative = fall_back + timedelta(seconds=-1)
    negative_duration = fall_back + pendulum.duration(seconds=-1)
    assert negative == negative_duration
    assert (negative.hour, negative.minute, negative.second) == (1, 59, 59)
    assert negative.utcoffset().total_seconds() == -7 * 60 * 60


if __name__ == "__main__":
    main()
