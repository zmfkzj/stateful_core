#!/usr/bin/env python3
"""Evaluate Click issue #3362 usage wrapping without hyphen splits."""
from __future__ import annotations

import sys
from pathlib import Path


def main() -> None:
    if len(sys.argv) != 2:
        raise SystemExit(f"usage: {Path(sys.argv[0]).name} REPOSITORY")

    repository = Path(sys.argv[1]).resolve()
    sys.path.insert(0, str(repository / "src"))

    import click
    from click.formatting import wrap_text


    options = [
        "--enable-verbose-logging",
        "--output-file-path",
        "--max-retry-count",
        "--disable-cache-mode",
        "--config-file-location",
        "--user-auth-token",
        "--auto-update-interval",
        "--force-overwrite-existing",
        "--network-timeout-seconds",
        "--debug-trace-enabled",
    ]
    formatter = click.HelpFormatter(width=65)
    formatter.write_usage("program", " ".join(options))
    output = formatter.getvalue()
    expected = (
        "Usage: program --enable-verbose-logging --output-file-path\n"
        "               --max-retry-count --disable-cache-mode\n"
        "               --config-file-location --user-auth-token\n"
        "               --auto-update-interval --force-overwrite-existing\n"
        "               --network-timeout-seconds --debug-trace-enabled\n"
    )
    assert output == expected, output
    assert wrap_text("prefix-suffix", width=8) == "prefix-\nsuffix"


    styled = click.style("--long-option-name", fg="red")
    formatter = click.HelpFormatter(width=40)
    formatter.write_usage("prog", f"{styled} -x")
    styled_output = formatter.getvalue()
    assert click.unstyle(styled_output) == "Usage: prog --long-option-name -x\n", styled_output
    assert "\x1b[31m" in styled_output and "\x1b[0m" in styled_output


if __name__ == "__main__":
    main()
