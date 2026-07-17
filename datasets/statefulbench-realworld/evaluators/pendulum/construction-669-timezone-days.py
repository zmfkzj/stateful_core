#!/usr/bin/env python3
"""Evaluator for Pendulum issue #669."""
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

    start = pendulum.datetime(2022, 11, 1, 9, tz="America/Los_Angeles")
    result = start + timedelta(days=6)
    assert (result.year, result.month, result.day, result.hour) == (2022, 11, 7, 9)
    assert result.timezone_name == "America/Los_Angeles"
    assert result.utcoffset().total_seconds() == -8 * 60 * 60


if __name__ == "__main__":
    main()
