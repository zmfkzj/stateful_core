#!/usr/bin/env python3
"""Evaluator for Jsonschema issue #1218 instance-free error reasons."""

import argparse
import sys
from pathlib import Path


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("repo", type=Path)
    args = parser.parse_args()
    sys.path.insert(0, str(args.repo))

    from jsonschema import Draft202012Validator
    from jsonschema.exceptions import ValidationError

    # Normal: callers can provide a concise safe message without changing the
    # legacy message or exception args that may include the rejected instance.
    error = ValidationError(
        "'top-secret' is not of type 'integer'",
        validator="type",
        validator_value="integer",
        instance="top-secret",
        schema={"type": "integer"},
        reason="Expected an integer.",
    )
    assert error.reason == "Expected an integer."
    assert "top-secret" in error.message
    assert "top-secret" not in error.reason
    assert error.args[0] == error.message

    # Boundary: an empty, explicitly supplied reason remains distinct from an
    # omitted reason and is not replaced by a potentially unsafe message.
    blank = ValidationError("credential=top-secret", reason="")
    omitted = ValidationError("credential=top-secret")
    assert blank.reason == ""
    assert omitted.reason is None

    # Error path: ordinary validator failures remain ValidationError instances
    # and expose no derived value that could leak the failing instance.
    try:
        Draft202012Validator({"type": "integer"}).validate("top-secret")
    except ValidationError as generated:
        assert generated.reason is None
        assert "top-secret" not in (generated.reason or "")
    else:
        raise AssertionError("a non-integer instance must fail validation")

    # Regression: copying an error preserves the optional safe reason.
    copied = ValidationError.create_from(error)
    assert copied.reason == "Expected an integer."
    assert copied.message == error.message
    assert copied.args[0] == error.message


if __name__ == "__main__":
    main()
