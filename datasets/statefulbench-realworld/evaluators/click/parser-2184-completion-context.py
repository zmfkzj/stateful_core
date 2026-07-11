#!/usr/bin/env python3
"""Evaluator for Click issue #2184."""

from __future__ import annotations

import inspect
import sys
from pathlib import Path


def main() -> None:
    checkout = Path(sys.argv[1]).resolve()
    sys.path.insert(0, str(checkout / "src"))

    import click
    import click.shell_completion as shell_completion
    from click.shell_completion import BashComplete

    resolve_source = inspect.getsource(shell_completion._resolve_context)
    assert (
        "Option" in resolve_source
        and "command.params" in resolve_source
        and "args[:-1]" in resolve_source
    ), resolve_source

    seen: list[dict[str, object]] = []

    def complete(ctx: click.Context, param: click.Parameter, incomplete: str) -> list[str]:
        seen.append(dict(ctx.params))
        return []

    @click.command()
    @click.argument("item")
    @click.option("--choice", shell_complete=complete)
    def flat(item: str, choice: str | None) -> None:
        pass

    completions = BashComplete(flat, {}, "flat", "_FLAT_COMPLETE").get_completions(
        ["known", "--choice"], ""
    )
    assert completions == []
    assert seen == [{"item": "known", "choice": None}], seen

    seen.clear()

    @click.group()
    def cli() -> None:
        pass

    @cli.command()
    @click.argument("item")
    @click.option("--choice", shell_complete=complete)
    def sub(item: str, choice: str | None) -> None:
        pass

    completions = BashComplete(cli, {}, "cli", "_CLI_COMPLETE").get_completions(
        ["sub", "known", "--choice"], ""
    )
    assert completions == []
    assert seen == [{"item": "known", "choice": None}], seen
