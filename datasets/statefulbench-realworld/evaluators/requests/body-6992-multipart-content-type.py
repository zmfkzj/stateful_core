#!/usr/bin/env python3
"""Evaluator for Requests issue #6992 multipart Content-Type handling."""

import argparse
import sys
from pathlib import Path


def prepare(headers, *, data=None, files=None):
    from requests.models import PreparedRequest

    request = PreparedRequest()
    request.prepare(
        method="POST",
        url="https://example.test/upload",
        headers=headers,
        data=data,
        files=files,
    )
    return request


def multipart_boundary(request):
    content_type = request.headers["Content-Type"]
    assert content_type.startswith("multipart/form-data; boundary="), content_type
    boundary = content_type.partition("boundary=")[2].encode("ascii")
    assert boundary and boundary in request.body


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("repo", type=Path)
    args = parser.parse_args()
    sys.path.insert(0, str(args.repo / "src"))

    # Normal: a caller's JSON default cannot describe a multipart body.
    direct = prepare(
        {"Content-Type": "application/json"},
        data={"label": "report"},
        files={"file": ("report.txt", b"contents", "text/plain")},
    )
    multipart_boundary(direct)
    assert b'name="label"' in direct.body
    assert b'filename="report.txt"' in direct.body

    # Boundary: Session-style inherited values, including parameters, must be replaced.
    inherited = prepare(
        {"Content-Type": "application/json; charset=utf-8"},
        files={"file": ("empty.txt", b"")},
    )
    multipart_boundary(inherited)
    assert b'filename="empty.txt"' in inherited.body

    # Error path: malformed multipart inputs retain their existing validation error.
    try:
        prepare(
            {"Content-Type": "application/json"},
            data="not-a-field-mapping",
            files={"file": ("report.txt", b"contents")},
        )
    except ValueError as error:
        assert str(error) == "Data must not be a string."
    else:
        raise AssertionError("string data with files must be rejected")

    # Non-multipart data must continue to honor a caller-selected media type.
    plain = prepare({"Content-Type": "application/json"}, data=b"{}")
    assert plain.headers["Content-Type"] == "application/json"


if __name__ == "__main__":
    main()
