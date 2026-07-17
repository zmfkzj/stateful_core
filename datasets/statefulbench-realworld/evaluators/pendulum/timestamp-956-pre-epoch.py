#!/usr/bin/env python3
"""Evaluator for Pendulum issue #956."""
import argparse
import sys
from pathlib import Path


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("repo", type=Path)
    args = parser.parse_args()
    sys.path.insert(0, str(args.repo / "src"))

    import pendulum

    real_datetime = pendulum._datetime.datetime

    class WindowsDatetime(real_datetime):
        @classmethod
        def fromtimestamp(cls, timestamp, tz=None):
            if timestamp < -43200:
                raise OSError("[Errno 22] Invalid argument")
            return super().fromtimestamp(timestamp, tz=tz)

    pendulum._datetime.datetime = WindowsDatetime
    try:
        boundary = pendulum.from_timestamp(-43200, tz="UTC")
        assert (boundary.year, boundary.month, boundary.day) == (1969, 12, 31)
        assert (boundary.hour, boundary.minute, boundary.second) == (12, 0, 0)

        utc = pendulum.from_timestamp(-43201.25, tz="UTC")
        assert (utc.year, utc.month, utc.day) == (1969, 12, 31)
        assert (utc.hour, utc.minute, utc.second, utc.microsecond) == (11, 59, 58, 750000)

        new_york = pendulum.from_timestamp(-43201, tz="America/New_York")
        assert (new_york.year, new_york.month, new_york.day) == (1969, 12, 31)
        assert (new_york.hour, new_york.minute, new_york.second) == (6, 59, 59)

        tokyo = pendulum.from_timestamp(-43201.5, tz="Asia/Tokyo")
        assert (tokyo.year, tokyo.month, tokyo.day) == (1969, 12, 31)
        assert (tokyo.hour, tokyo.minute, tokyo.second, tokyo.microsecond) == (20, 59, 58, 500000)
    finally:
        pendulum._datetime.datetime = real_datetime


if __name__ == "__main__":
    main()
