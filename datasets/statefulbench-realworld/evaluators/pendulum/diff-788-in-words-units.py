#!/usr/bin/env python3
"""Check DateTime.diff configures the returned interval's word units."""
from __future__ import annotations

from pathlib import Path
import sys


checkout = Path(sys.argv[1]).resolve()
sys.path.insert(0, str(checkout / "src"))

import pendulum


def main() -> None:
    start = pendulum.datetime(2024, 1, 1, tz="UTC")
    end = pendulum.datetime(2024, 1, 3, 3, 4, tz="UTC")

    assert start.diff(end).in_words() == "2 days 3 hours 4 minutes"
    assert start.diff(end, units=("day", "hour")).in_words() == "2 days 3 hours"
    assert start.diff(end, units=("hour",)).in_words() == "3 hours"
    assert start.diff(end, units=("hour",)).in_words(units=("day",)) == "2 days"
    day_end = start.add(days=1)
    assert start.diff(day_end, units=("hour",)).in_words() == "0 hours"
    assert start.diff(day_end, units=("hour",)).in_words(locale="fr") == "0 heure"
    assert start.diff(day_end, units=()).in_words() == ""

    subsecond_end = start.add(microseconds=500_000)
    assert start.diff(subsecond_end, units=("hour",)).in_words() == "0 hours"

    dst_start = pendulum.datetime(2024, 3, 10, 1, 30, tz="America/New_York")
    dst_end = pendulum.datetime(2024, 3, 10, 3, 30, tz="America/New_York")
    assert dst_start.diff(dst_end, units=("hour",)).in_words() == "1 hour"


if __name__ == "__main__":
    main()
