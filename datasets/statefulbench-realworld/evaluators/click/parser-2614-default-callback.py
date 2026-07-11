#!/usr/bin/env python3
"""Evaluator for Click issue #2614."""

from __future__ import annotations

import sys
from pathlib import Path


def main() -> None:
    checkout = Path(sys.argv[1]).resolve()
    sys.path.insert(0, str(checkout / "src"))

    import click
    from click.testing import CliRunner
    from click.shell_completion import BashComplete

    calls: list[str] = []
    seen: list[dict[str, object]] = []


    def expensive_default() -> str:
        calls.append("called")
        return "computed"

    def complete(ctx: click.Context, param: click.Parameter, incomplete: str) -> list[str]:
        seen.append(dict(ctx.params))
        return []

    @click.command()
    @click.option("--config", default=expensive_default)
    @click.option("--choice", shell_complete=complete)
    def cli(config: str, choice: str | None) -> None:
        pass

    completions = BashComplete(cli, {}, "cli", "_CLI_COMPLETE").get_completions(
        ["--choice"], ""
    )
    assert completions == []
    assert calls == [], calls
    assert seen == [{"config": None, "choice": None}], seen
    result = CliRunner().invoke(cli, [])
    assert result.exit_code == 0, result.output
    assert calls == ["called"], calls
    calls.clear()
    seen.clear()
    BashComplete(
        cli, {"default_map": {"config": "provided"}}, "cli", "_CLI_COMPLETE"
    ).get_completions(["--choice"], "")
    assert calls == [], calls
    assert seen == [{"config": "provided", "choice": None}], seen


if __name__ == "__main__":
    main()
