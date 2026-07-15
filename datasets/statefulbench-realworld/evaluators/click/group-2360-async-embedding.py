#!/usr/bin/env python3
"""Evaluator for Click issue #2360."""

from __future__ import annotations

import asyncio
import contextvars
import gc
import sys
import warnings
from pathlib import Path


def main() -> None:
    checkout = Path(sys.argv[1]).resolve()
    sys.path.insert(0, str(checkout / "src"))

    import click

    marker = contextvars.ContextVar[str]("marker")

    @click.group(invoke_without_command=True)
    async def cli() -> str:
        await asyncio.sleep(0)
        assert marker.get() == "embedded"
        assert click.get_current_context().command is cli
        return "callback value"

    @cli.result_callback()
    async def process_result(value: str) -> str:
        await asyncio.sleep(0)
        assert marker.get() == "embedded"
        assert value == "callback value"
        return f"processed {value}"

    @click.group()
    def normal() -> None:
        pass

    @normal.command()
    def command() -> str:
        return "command value"

    @normal.result_callback()
    async def process_command(value: str) -> str:
        await asyncio.sleep(0)
        assert marker.get() == "embedded"
        assert value == "command value"
        return f"processed {value}"

    @click.group(invoke_without_command=True)
    def failing() -> None:
        pass

    @failing.result_callback()
    async def fail_result(_: object) -> None:
        await asyncio.sleep(0)
        assert marker.get() == "embedded"
        raise ValueError("result failure")

    async def invoke_inside_running_loop() -> None:
        token = marker.set("embedded")
        try:
            with warnings.catch_warnings(record=True) as recorded:
                warnings.simplefilter("always", RuntimeWarning)
                result = cli.main([], standalone_mode=False)
                assert result == "processed callback value", result
                result = normal.main(["command"], standalone_mode=False)
                assert result == "processed command value", result
                try:
                    failing.main([], standalone_mode=False)
                except ValueError as error:
                    assert str(error) == "result failure", error
                else:
                    raise AssertionError("async result callback error did not propagate")
                gc.collect()
            assert not recorded, recorded
        finally:
            marker.reset(token)

    asyncio.run(invoke_inside_running_loop())


if __name__ == "__main__":
    main()
