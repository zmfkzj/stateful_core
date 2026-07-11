#!/usr/bin/env python3
"""Evaluator for Click issue #2184."""

from __future__ import annotations

import sys
from pathlib import Path


def main() -> None:
    checkout = Path(sys.argv[1]).resolve()
    sys.path.insert(0, str(checkout / "src"))

    import click
    from click.shell_completion import BashComplete

    seen: list[dict[str, object]] = []

    def complete(ctx: click.Context, param: click.Parameter, incomplete: str) -> list[str]:
        seen.append(dict(ctx.params))
        return []

    @click.command()
    @click.argument("item")
    @click.option("--choice", shell_complete=complete)
    def cli(item: str, choice: str | None) -> None:
        pass

    completions = BashComplete(cli, {}, "cli", "_CLI_COMPLETE").get_completions(
        ["known", "--choice"], ""
    )
    assert completions == []
    assert seen == [{"item": "known", "choice": None}], seen


if __name__ == "__main__":
    main()
