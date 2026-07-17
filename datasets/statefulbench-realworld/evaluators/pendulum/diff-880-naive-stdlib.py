#!/usr/bin/env python3
"""Check DateTime.diff preserves naïve stdlib datetime endpoints."""
from __future__ import annotations

from datetime import datetime
from datetime import timezone
from pathlib import Path
import sys


checkout = Path(sys.argv[1]).resolve()
sys.path.insert(0, str(checkout / "src"))

import pendulum


def main() -> None:
    naive = pendulum.datetime(2024, 1, 1, 12, 0, 0, tz=None)
    interval = naive.diff(datetime(2024, 1, 2, 15, 30, 0))
    assert interval.start.tzinfo is None
    assert interval.end.tzinfo is None
    assert interval.in_seconds() == 99_000

    aware = pendulum.datetime(2024, 3, 10, 1, 30, tz="America/New_York")
    dst_interval = aware.diff(datetime(2024, 3, 10, 7, 30, tzinfo=timezone.utc))
    assert dst_interval.in_seconds() == 3_600


if __name__ == "__main__":
    main()
