#!/usr/bin/env python3
"""Evaluator for Click issue #1411's custom confirm conversion extension."""
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
    errors = io.StringIO()
    answers = iter(("later", "SHIP", "HOLD", "", "yes"))
    seen = []

    class ConfirmationWords(click.ParamType):
        name = "confirmation word"

        def convert(self, value, param, ctx):
            seen.append(value)
            if value == "SHIP":
                return True
            if value == "HOLD":
                return False
            self.fail("choose ship or hold", param, ctx)

    def read_answer(prompt: str) -> str:
        print(prompt, end="")
        return next(answers)

    click.termui.visible_prompt_func = read_answer
    words = ConfirmationWords()
    with contextlib.redirect_stdout(output), contextlib.redirect_stderr(errors):
        assert click.confirm("Deploy", type=words) is True
        try:
            click.confirm("Abort", type=words, abort=True)
        except click.Abort:
            pass
        else:
            raise AssertionError("a custom false answer did not abort")
        assert click.confirm("Default", default=True, type=words) is True
        assert click.confirm("Legacy", False, False, ": ", False, True) is True

    assert seen == ["later", "SHIP", "HOLD"]
    text = output.getvalue()
    assert text.count("Deploy [y/N]: ") == 2
    assert "Error: choose ship or hold" in text
    assert "Legacy" not in text
    assert errors.getvalue() == "Legacy: "


if __name__ == "__main__":
    main()
