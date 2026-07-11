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

    # Keyword errors provide a concise reason that does not reveal the
    # rejected instance, while retaining the legacy instance-containing message.
    try:
        Draft202012Validator({"type": "integer"}).validate("top-secret")
    except ValidationError as generated:
        assert generated.reason == "Expected type 'integer'."
        assert repr("top-secret") not in generated.reason
        assert "top-secret" in generated.message
        # Regression: copying a generated keyword error preserves its safe reason.
        copied = ValidationError.create_from(generated)
        assert copied.reason == generated.reason
        assert copied.message == generated.message
        assert copied.args[0] == generated.message
    else:
        raise AssertionError("a non-integer instance must fail validation")



if __name__ == "__main__":
    main()
