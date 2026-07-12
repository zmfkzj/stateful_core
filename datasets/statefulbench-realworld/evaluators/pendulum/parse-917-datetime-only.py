#!/usr/bin/env python3
"""Evaluator for Pendulum issue #917."""

from __future__ import annotations

import datetime
import importlib.util
import inspect
import sys
import types
from pathlib import Path


def main() -> None:
    checkout = Path(sys.argv[1]).resolve()
    sys.path.insert(0, str(checkout / "src"))
    if importlib.util.find_spec("dateutil") is None:
        dateutil = types.ModuleType("dateutil")
        dateutil.parser = types.ModuleType("dateutil.parser")
        sys.modules["dateutil"] = dateutil
        sys.modules["dateutil.parser"] = dateutil.parser

    import pendulum
    import pendulum.parser as parser
    from pendulum.parsing.exceptions import ParserError

    parse_dt = pendulum.parse_dt
    source = inspect.getsource(parser.parse_dt)
    assert "parse(text, **options)" in source and "DateTime" in source, source

    utc = parse_dt("2001-01-01T12:34:56.123456Z")
    assert isinstance(utc, pendulum.DateTime)
    assert utc.microsecond == 123456
    assert utc.utcoffset() == datetime.timedelta(0)

    offset = parse_dt("2001-01-01T12:34:56.654321+05:30")
    assert isinstance(offset, pendulum.DateTime)
    assert offset.microsecond == 654321
    assert offset.utcoffset() == datetime.timedelta(hours=5, minutes=30)

    try:
        parse_dt("2001-01-01T00:00:00Z/2001-01-02T00:00:00Z")
    except ParserError:
        pass
    else:
        raise AssertionError("parse_dt accepted an interval")


if __name__ == "__main__":
    main()
