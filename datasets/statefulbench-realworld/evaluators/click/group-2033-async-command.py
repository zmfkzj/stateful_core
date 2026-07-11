#!/usr/bin/env python3
"""Evaluator for Click issue #2033."""

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
    from click.testing import CliRunner

    @click.group()
    def cli() -> None:
        pass

    @cli.command("async-command")
    async def async_command() -> None:
        await asyncio.sleep(0)
        click.echo(click.get_current_context().info_name)

    @cli.command("sync-command")
    def sync_command() -> None:
        click.echo("sync")

    @cli.command("failing-command")
    async def failing_command() -> None:
        await asyncio.sleep(0)
        raise ValueError("async failure")

    runner = CliRunner()
    result = runner.invoke(cli, ["async-command"])
    assert result.exit_code == 0, result.exception
    assert result.output == "async-command\n", result.output

    result = runner.invoke(cli, ["sync-command"])
    assert result.exit_code == 0, result.exception
    assert result.output == "sync\n", result.output

    result = runner.invoke(cli, ["failing-command"])
    assert isinstance(result.exception, ValueError), result.exception
    assert str(result.exception) == "async failure", result.exception

    async def nested_invocation() -> None:
        with warnings.catch_warnings(record=True) as recorded:
            warnings.simplefilter("always", RuntimeWarning)
            try:
                cli.main(["async-command"], standalone_mode=False)
            except click.UsageError as error:
                assert "event loop is running" in error.message
            else:
                raise AssertionError("nested async command did not raise UsageError")
            gc.collect()
        assert not recorded, recorded

    asyncio.run(nested_invocation())


if __name__ == "__main__":
    main()
