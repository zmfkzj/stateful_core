#!/usr/bin/env python3
"""Evaluator for Pendulum issue #935."""

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

    source = inspect.getsource(parser.parse)
    assert "re.sub(" in source and "{9}" in source, source

    utc = pendulum.parse("2001-01-01T12:34:56.123456789Z")
    assert isinstance(utc, pendulum.DateTime)
    assert utc.microsecond == 123456
    assert utc.utcoffset() == datetime.timedelta(0)

    long_utc = pendulum.parse("2001-01-01T12:34:56.1234567890Z")
    assert isinstance(long_utc, pendulum.DateTime)
    assert long_utc.microsecond == 123456
    assert long_utc.utcoffset() == datetime.timedelta(0)

    offset = pendulum.parse("2001-01-01T12:34:56.987654321012345+05:30")
    assert isinstance(offset, pendulum.DateTime)
    assert offset.microsecond == 987654
    assert offset.utcoffset() == datetime.timedelta(hours=5, minutes=30)


if __name__ == "__main__":
    main()
