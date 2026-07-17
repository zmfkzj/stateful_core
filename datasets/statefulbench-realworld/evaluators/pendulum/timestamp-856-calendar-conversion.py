#!/usr/bin/env python3
"""Evaluator for Pendulum issue #856."""
import argparse
import sys
from pathlib import Path


def gregorian_to_jalali(dt):
    month_days = (0, 31, 59, 90, 120, 151, 181, 212, 243, 273, 304, 334)
    gregorian_year = dt.year - (1600 if dt.year >= 1600 else 621)
    jalali_year = 979 if dt.year >= 1600 else 0
    leap_year = gregorian_year + 1 if dt.month > 2 else gregorian_year
    days = (
        365 * gregorian_year
        + (leap_year + 3) // 4
        - (leap_year + 99) // 100
        + (leap_year + 399) // 400
        - 80
        + dt.day
        + month_days[dt.month - 1]
    )
    jalali_year += 33 * (days // 12053)
    days %= 12053
    jalali_year += 4 * (days // 1461)
    days %= 1461
    if days > 365:
        jalali_year += (days - 1) // 365
        days = (days - 1) % 365
    if days < 186:
        jalali_month, jalali_day = 1 + days // 31, 1 + days % 31
    else:
        jalali_month, jalali_day = 7 + (days - 186) // 30, 1 + (days - 186) % 30
    return jalali_year, jalali_month, jalali_day, dt.hour, dt.minute, dt.second


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("repo", type=Path)
    args = parser.parse_args()
    sys.path.insert(0, str(args.repo / "src"))

    import pendulum

    plain = pendulum.from_timestamp(0, tz="Asia/Tehran")
    assert isinstance(plain, pendulum.DateTime)

    epoch = pendulum.from_timestamp(0, tz="Asia/Tehran", calendar=gregorian_to_jalali)
    assert epoch == (1348, 10, 11, 3, 30, 0)

    nowruz = pendulum.from_timestamp(
        1710892800, tz="Asia/Tehran", calendar=gregorian_to_jalali
    )
    assert nowruz == (1403, 1, 1, 3, 30, 0)


if __name__ == "__main__":
    main()
