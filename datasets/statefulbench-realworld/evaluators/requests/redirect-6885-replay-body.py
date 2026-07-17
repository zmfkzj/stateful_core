#!/usr/bin/env python3
"""Evaluator for requests issue #6885."""
import argparse
import io
import sys
from pathlib import Path


class EarlyCloseRaw:
    def close(self):
        pass

    def release_conn(self):
        pass

    def read(self, **kwargs):
        return b""

    def stream(self, *args, **kwargs):
        from urllib3.exceptions import SSLError

        raise SSLError("EOF occurred in violation of protocol")


def redirect_response(Response, request, status_code):
    response = Response()
    response.status_code = status_code
    response.headers["Location"] = "https://example.test/next"
    response.url = request.url
    response.request = request
    response.raw = EarlyCloseRaw()
    return response


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("repo", type=Path)
    args = parser.parse_args()
    sys.path.insert(0, str(args.repo / "src"))
    from requests.models import PreparedRequest, Response
    from requests.sessions import Session

    for status_code in (307, 308):
        request = PreparedRequest()
        request.prepare("PUT", "https://example.test/original", data=io.BytesIO(b"payload"))
        request.body.read()
        redirected = next(
            Session().resolve_redirects(
                redirect_response(Response, request, status_code),
                request,
                yield_requests=True,
            )
        )
        assert redirected.method == "PUT"
        assert redirected.headers["Content-Length"] == str(len(b"payload"))
        assert redirected.body.read() == b"payload"


if __name__ == "__main__":
    main()
