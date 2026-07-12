#!/usr/bin/env python3
"""Evaluator for the zero-byte framing extension motivated by Requests issue #6294."""

import argparse
import io
import sys
from pathlib import Path


def prepare(data, *, files=None):
    from requests.models import PreparedRequest

    request = PreparedRequest()
    request.prepare(
        method="PUT", url="https://example.test/object", data=data, files=files
    )
    return request


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("repo", type=Path)
    args = parser.parse_args()
    sys.path.insert(0, str(args.repo / "src"))
    from requests.models import PreparedRequest

    # Normal: an empty seekable upload is explicitly framed for servers without chunking.
    empty = prepare(io.BytesIO())
    assert empty.body is not None
    assert empty.headers.get("Content-Length") == "0", empty.headers
    assert "Transfer-Encoding" not in empty.headers, empty.headers

    # Boundary: non-empty seekable streams retain exact length framing.
    payload = b"abc"
    known = prepare(io.BytesIO(payload))
    assert known.headers.get("Content-Length") == str(len(payload)), known.headers
    assert "Transfer-Encoding" not in known.headers, known.headers

    # Error path: streamed bodies and multipart files remain mutually exclusive.
    try:
        prepare(io.BytesIO(), files={"file": ("empty.txt", b"")})
    except NotImplementedError as error:
        assert str(error) == "Streamed bodies and files are mutually exclusive."
    else:
        raise AssertionError("streamed multipart input must be rejected")

    # Explicit zero framing is method-independent for body-bearing methods.
    post = PreparedRequest()
    post.prepare(method="POST", url="https://example.test/object", data=io.BytesIO())
    assert post.headers.get("Content-Length") == "0", post.headers
    assert "Transfer-Encoding" not in post.headers, post.headers
    # Authentication must not reclassify an explicitly zero-length seekable
    # stream as chunked.
    authenticated = PreparedRequest()
    authenticated.prepare(
        method="POST",
        url="https://example.test/object",
        data=io.BytesIO(),
        auth=("user", "pass"),
    )
    assert authenticated.headers.get("Content-Length") == "0", authenticated.headers
    assert "Transfer-Encoding" not in authenticated.headers, authenticated.headers


if __name__ == "__main__":
    main()
