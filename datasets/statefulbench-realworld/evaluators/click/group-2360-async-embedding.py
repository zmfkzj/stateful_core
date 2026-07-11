#!/usr/bin/env python3
"""Evaluator for Click issue #2360."""

from __future__ import annotations

import asyncio
import gc
import sys
import warnings
from pathlib import Path


def main() -> None:
    checkout = Path(sys.argv[1]).resolve()
    sys.path.insert(0, str(checkout / "src"))

    import click

    @click.group(invoke_without_command=True)
    async def cli() -> str:
        await asyncio.sleep(0)
        click.echo(click.get_current_context().info_name)
        return "embedded"

    async def invoke_inside_running_loop() -> None:
        with warnings.catch_warnings(record=True) as recorded:
            warnings.simplefilter("always", RuntimeWarning)
            result = cli.main([], standalone_mode=False)
            gc.collect()
        assert result == "embedded", result
        assert not recorded, recorded

    asyncio.run(invoke_inside_running_loop())


if __name__ == "__main__":
    main()
