#!/usr/bin/env python3
"""Evaluator for Click issue #1370's deterministic confirm input stream."""
import argparse
import contextlib
import io
import sys
from pathlib import Path


def load_click(repo: Path):
    sys.path.insert(0, str(repo / "src"))
    for name in tuple(sys.modules):
        if name == "click" or name.startswith("click."):
            del sys.modules[name]
    import click

    return click


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("repo", type=Path)
    args = parser.parse_args()
    click = load_click(args.repo)
    output = io.StringIO()

    def no_tty(_: str) -> str:
        raise AssertionError("confirm consulted the interactive prompt function")

    click.termui.visible_prompt_func = no_tty
    with contextlib.redirect_stdout(output):
        assert click.confirm(
            "Continue", input_stream=io.StringIO("maybe\nYES\n")
        ) is True
        assert click.confirm(
            "Default", default=True, input_stream=io.StringIO("\n")
        ) is True
        try:
            click.confirm("Abort", abort=True, input_stream=io.StringIO("no\n"))
        except click.Abort:
            pass
        else:
            raise AssertionError("a negative streamed answer did not abort")
        try:
            click.confirm("EOF", input_stream=io.StringIO())
        except click.Abort:
            pass
        else:
            raise AssertionError("an exhausted input stream did not abort")

    text = output.getvalue()
    assert text.count("Continue [y/N]: ") == 2
    assert "Error: invalid input" in text
    assert "Default [Y/n]: " in text


if __name__ == "__main__":
    main()
