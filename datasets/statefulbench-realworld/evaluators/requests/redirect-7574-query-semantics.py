#!/usr/bin/env python3
"""Evaluator for requests issue #7574."""
import argparse
import sys
from pathlib import Path


class Raw:
    def close(self):
        pass

    def release_conn(self):
        pass

    def read(self, **kwargs):
        return b""


def redirect_response(Response, request, status_code):
    response = Response()
    response.status_code = status_code
    response.headers["Location"] = "https://example.test/next"
    response.url = request.url
    response.request = request
    response.raw = Raw()
    response._content = b""
    return response


def redirected_request(Session, Response, status_code):
    from requests.models import PreparedRequest

    request = PreparedRequest()
    request.prepare(
        "QUERY",
        "https://example.test/original",
        data=b"query=select",
        headers={"Content-Type": "application/query"},
    )
    return next(
        Session().resolve_redirects(
            redirect_response(Response, request, status_code), request, yield_requests=True
        )
    )


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("repo", type=Path)
    args = parser.parse_args()
    sys.path.insert(0, str(args.repo / "src"))
    from requests.models import Response
    from requests.sessions import Session

    for status_code in (301, 302, 307, 308):
        redirected = redirected_request(Session, Response, status_code)
        assert redirected.method == "QUERY"
        assert redirected.body == b"query=select"
        assert redirected.headers["Content-Length"] == str(len(b"query=select"))
        assert redirected.headers["Content-Type"] == "application/query"

    see_other = redirected_request(Session, Response, 303)
    assert see_other.method == "GET"
    assert see_other.body is None
    for header in ("Content-Length", "Content-Type", "Transfer-Encoding"):
        assert header not in see_other.headers


if __name__ == "__main__":
    main()
