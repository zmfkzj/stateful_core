#!/usr/bin/env python3
"""Check Context dynamic parameters can be parsed in a nested context."""
from __future__ import annotations

from pathlib import Path
import sys


checkout = Path(sys.argv[1]).resolve()
sys.path.insert(0, str(checkout / "src"))

import click
from click.testing import CliRunner


def main() -> None:
    observed: list[tuple[object, click.ParameterSource | None]] = []
    overridden: list[tuple[object, click.ParameterSource | None]] = []
    group_observed: list[tuple[object, click.ParameterSource | None]] = []
    closed: list[bool] = []

    def add_dynamic(ctx: click.Context, param: click.Parameter, value: str) -> str:
        ctx.dynamic_params.append(click.Option([f"--{value}"]))
        ctx.dynamic_params.append(click.Argument([f"{value}_path"], required=False))
        return value

    @click.command(
        context_settings={"allow_extra_args": True, "ignore_unknown_options": True}
    )
    @click.option("--dynamic", default="extra", is_eager=True, callback=add_dynamic)
    @click.pass_context
    def command(ctx: click.Context, dynamic: str) -> None:
        assert [param.name for param in command.get_params(ctx)].count(dynamic) == 1
        child = click.Context(click.Command("child"), info_name="child", parent=ctx)
        assert child.dynamic_params == []
        assert child.command_path == f"command [{dynamic.upper()}_PATH] child", child.command_path
        parsed = command.make_dynamic_context(ctx)
        assert parsed.parent is ctx
        parsed.call_on_close(lambda: closed.append(True))
        cleanup_count = len(closed)
        with parsed:
            observed.append(
                (parsed.params[dynamic], parsed.get_parameter_source(dynamic))
            )
        assert len(closed) == cleanup_count + 1
        if not ctx.args:
            override = command.make_dynamic_context(
                ctx, default_map={dynamic: "overridden"}
            )
            with override:
                overridden.append(
                    (
                        override.params[dynamic],
                        override.get_parameter_source(dynamic),
                    )
                )

    @click.group(invoke_without_command=True)
    @click.option("--dynamic", default="extra", is_eager=True, callback=add_dynamic)
    @click.pass_context
    def group(ctx: click.Context, dynamic: str) -> None:
        parsed = group.make_dynamic_context(ctx)
        assert parsed.parent is ctx
        with parsed:
            group_observed.append(
                (parsed.params[dynamic], parsed.get_parameter_source(dynamic))
            )

    runner = CliRunner()
    result = runner.invoke(command, ["--dynamic", "extra", "--extra", "value"])
    assert result.exit_code == 0, (result.output, result.exception)
    result = runner.invoke(command, ["--dynamic", "extra"], default_map={"extra": "mapped"})
    assert result.exit_code == 0, (result.output, result.exception)
    result = runner.invoke(group, ["--dynamic", "extra"], default_map={"extra": "group"})
    assert result.exit_code == 0, (result.output, result.exception)
    assert observed == [
        ("value", click.ParameterSource.COMMANDLINE),
        ("mapped", click.ParameterSource.DEFAULT_MAP),
    ], observed
    assert overridden == [
        ("overridden", click.ParameterSource.DEFAULT_MAP),
        ("overridden", click.ParameterSource.DEFAULT_MAP),
    ], overridden
    assert group_observed == [("group", click.ParameterSource.DEFAULT_MAP)]



if __name__ == "__main__":
    main()
