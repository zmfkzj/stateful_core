#!/usr/bin/env python3
"""Evaluator for Click issue #2614."""

from __future__ import annotations

import inspect
import sys
from pathlib import Path


def main() -> None:
    checkout = Path(sys.argv[1]).resolve()
    sys.path.insert(0, str(checkout / "src"))

    import click
    import click.shell_completion as shell_completion
    from click.testing import CliRunner
    from click.shell_completion import BashComplete

    resolve_source = inspect.getsource(shell_completion._resolve_context)
    assert (
        "callable(param.default)" in resolve_source
        and "command.params" in resolve_source
    ), resolve_source

    calls: list[str] = []
    seen: list[dict[str, object]] = []

    def expensive_default() -> str:
        calls.append("called")
        return "computed"

    def complete(ctx: click.Context, param: click.Parameter, incomplete: str) -> list[str]:
        seen.append(dict(ctx.params))
        return []

    @click.group()
    def cli() -> None:
        pass

    @cli.command()
    @click.option("--config", default=expensive_default)
    @click.option("--choice", shell_complete=complete)
    def sub(config: str, choice: str | None) -> None:
        pass

    completions = BashComplete(cli, {}, "cli", "_CLI_COMPLETE").get_completions(
        ["sub", "--choice"], ""
    )
    assert completions == []
    assert calls == [], calls
    assert seen == [{"config": None, "choice": None}], seen

    result = CliRunner().invoke(cli, ["sub"])
    assert result.exit_code == 0, result.output
    assert calls == ["called"], calls

    calls.clear()
    seen.clear()
    BashComplete(
        cli, {"default_map": {"sub": {"config": "provided"}}}, "cli", "_CLI_COMPLETE"
    ).get_completions(["sub", "--choice"], "")
    assert calls == [], calls
    assert seen == [{"config": "provided", "choice": None}], seen

    seen.clear()
    BashComplete(cli, {}, "cli", "_CLI_COMPLETE").get_completions(
        ["sub", "--config", "explicit", "--choice"], ""
    )
    assert calls == [], calls
    assert seen == [{"config": "explicit", "choice": None}], seen

if __name__ == "__main__":
    main()
