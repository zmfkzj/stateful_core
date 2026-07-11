#!/usr/bin/env python3
"""Evaluate Click issue #3652 repeated-option metavar help."""
from __future__ import annotations

import sys
from pathlib import Path


def main() -> None:
    if len(sys.argv) != 2:
        raise SystemExit(f"usage: {Path(sys.argv[0]).name} REPOSITORY")

    repository = Path(sys.argv[1]).resolve()
    sys.path.insert(0, str(repository / "src"))

    import click
    from click.testing import CliRunner

    @click.command()
    @click.option("-t", "--tag", multiple=True, help="Repeatable tag.")
    @click.option("--pair", type=(str, int), multiple=True, help="Repeatable pair.")
    @click.option("--single", help="Single value.")
    @click.option("-v", "--verbose", is_flag=True, multiple=True, help="Repeatable flag.")
    def cli(tag: tuple[str, ...], pair: tuple[tuple[str, int], ...], single: str | None, verbose: tuple[bool, ...]) -> None:
        pass

    result = CliRunner().invoke(cli, ["--help"])
    assert result.exit_code == 0, result.output
    option_lines = result.output.splitlines()
    assert any(line.startswith("  -t, --tag TEXT...") for line in option_lines), result.output
    assert "--tag TEXT......" not in result.output, result.output
    assert any(
        line.startswith("  --pair <TEXT INTEGER>...") for line in option_lines
    ), result.output
    assert "--pair <TEXT INTEGER>......" not in result.output, result.output
    assert "--single TEXT" in result.output, result.output
    assert "--single TEXT..." not in result.output, result.output
    assert "-v, --verbose" in result.output, result.output
    assert "--verbose ..." not in result.output, result.output


if __name__ == "__main__":
    main()
