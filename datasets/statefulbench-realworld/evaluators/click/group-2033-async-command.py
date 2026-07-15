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

    parent_contexts: list[str] = []
    result_values: list[object] = []

    @click.group()
    async def cli() -> None:
        await asyncio.sleep(0)
        parent_contexts.append(click.get_current_context().info_name)

    @cli.result_callback()
    def process_result(value: object) -> object:
        result_values.append(value)
        return value

    @cli.command("async-command")
    async def async_command() -> str:
        await asyncio.sleep(0)
        click.echo(click.get_current_context().info_name)
        return "async result"

    @cli.command("sync-command")
    def sync_command() -> str:
        click.echo("sync")
        return "sync result"

    @cli.command("failing-command")
    async def failing_command() -> None:
        await asyncio.sleep(0)
        raise ValueError("async failure")

    chain_parent_contexts: list[str] = []
    chain_results: list[list[str]] = []

    @click.group(chain=True)
    async def chained() -> None:
        await asyncio.sleep(0)
        chain_parent_contexts.append(click.get_current_context().info_name)

    @chained.result_callback()
    def process_chain(values: list[str]) -> list[str]:
        chain_results.append(values)
        return values

    @chained.command()
    async def first() -> str:
        await asyncio.sleep(0)
        return "first"

    @chained.command()
    async def second() -> str:
        await asyncio.sleep(0)
        return "second"

    runner = CliRunner()
    with warnings.catch_warnings(record=True) as recorded:
        warnings.simplefilter("always", RuntimeWarning)

        result = runner.invoke(cli, ["async-command"])
        assert result.exit_code == 0, result.exception
        assert result.output == "async-command\n", result.output
        assert parent_contexts == ["cli"], parent_contexts
        assert result_values == ["async result"], result_values

        result = runner.invoke(cli, ["sync-command"])
        assert result.exit_code == 0, result.exception
        assert result.output == "sync\n", result.output
        assert result_values == ["async result", "sync result"], result_values

        result = runner.invoke(chained, ["first", "second"])
        assert result.exit_code == 0, result.exception
        assert chain_parent_contexts == ["chained"], chain_parent_contexts
        assert chain_results == [["first", "second"]], chain_results

        result = runner.invoke(cli, ["failing-command"])
        assert isinstance(result.exception, ValueError), result.exception
        assert str(result.exception) == "async failure", result.exception

        async def nested_invocation() -> None:
            try:
                cli.main(["async-command"], standalone_mode=False)
            except click.UsageError as error:
                assert "event loop is running" in error.message
            else:
                raise AssertionError("nested async command did not raise UsageError")

        asyncio.run(nested_invocation())
        gc.collect()

    assert not recorded, recorded


if __name__ == "__main__":
    main()
