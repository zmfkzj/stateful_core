#!/usr/bin/env python3
"""Check parameter sources survive Context.invoke and Context.forward."""
from __future__ import annotations

from pathlib import Path
import sys


checkout = Path(sys.argv[1]).resolve()
sys.path.insert(0, str(checkout / "src"))

import click
from click.testing import CliRunner


def run(
    args: list[str], **extra: object
) -> list[tuple[int, click.ParameterSource | None]]:
    seen: list[tuple[int, click.ParameterSource | None]] = []

    @click.command()
    @click.option("--count", default=1)
    @click.pass_context
    def target(ctx: click.Context, count: int) -> None:
        seen.append((count, ctx.get_parameter_source("count")))

    @click.command()
    @click.option("--count", default=1)
    @click.pass_context
    def root(ctx: click.Context, count: int) -> None:
        ctx.invoke(target)
        ctx.forward(target)

    result = CliRunner().invoke(root, args, **extra)
    assert result.exit_code == 0, result.output
    return seen


def run_direct() -> list[tuple[int, click.ParameterSource | None]]:
    seen: list[tuple[int, click.ParameterSource | None]] = []

    @click.command()
    @click.option("--count", default=1)
    @click.pass_context
    def target(ctx: click.Context, count: int) -> None:
        seen.append((count, ctx.get_parameter_source("count")))

    @click.command()
    @click.pass_context
    def root(ctx: click.Context) -> None:
        ctx.invoke(target, count=7)

    result = CliRunner().invoke(root, [])
    assert result.exit_code == 0, result.output
    return seen


def main() -> None:
    observed = run(["--count", "7"])
    assert observed == [
        (1, click.ParameterSource.DEFAULT),
        (7, click.ParameterSource.COMMANDLINE),
    ], observed
    observed = run([], default_map={"count": 9})
    assert observed == [
        (1, click.ParameterSource.DEFAULT),
        (9, click.ParameterSource.DEFAULT_MAP),
    ], observed
    assert run_direct() == [(7, click.ParameterSource.COMMANDLINE)]


if __name__ == "__main__":
    main()
