#!/usr/bin/env python3
"""Evaluator for Requests issue #6890's session-preparation path."""
import argparse
import http.cookiejar
import sys
from pathlib import Path


def cookie(name: str, value: str, *, secure: bool = False) -> http.cookiejar.Cookie:
    return http.cookiejar.Cookie(
        version=0,
        name=name,
        value=value,
        port=None,
        port_specified=False,
        domain="example.test",
        domain_specified=True,
        domain_initial_dot=False,
        path="/",
        path_specified=True,
        secure=secure,
        expires=None,
        discard=True,
        comment=None,
        comment_url=None,
        rest={},
        rfc2109=False,
    )


def prepared_header(Session, Request, jar: http.cookiejar.CookieJar) -> str | None:
    session = Session()
    session.cookies = jar
    return session.prepare_request(Request("GET", "https://example.test/")).headers.get("Cookie")


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("repo", type=Path)
    args = parser.parse_args()
    for root in (args.repo.parent, args.repo.parent.parent):
        deps = root / "requests-deps"
        if deps.is_dir():
            sys.path.insert(0, str(deps))
    sys.path.insert(0, str(args.repo / "src"))
    from requests import Request, Session
    from requests.cookies import RequestsCookieJar

    direct = RequestsCookieJar()
    direct.set_cookie(cookie("direct", '"159\\"687"'))
    assert direct["direct"] == '"159\\"687"'

    plain = http.cookiejar.CookieJar()
    plain.set_cookie(cookie("plain", "value"))
    assert prepared_header(Session, Request, plain) == "plain=value"

    escaped = http.cookiejar.CookieJar()
    escaped.set_cookie(cookie("token", '"159\\"687"'))
    assert prepared_header(Session, Request, escaped) == 'token="159\\"687"'

    secure = http.cookiejar.CookieJar()
    secure.set_cookie(cookie("secure", "yes", secure=True))
    assert prepared_header(Session, Request, secure) == "secure=yes"


if __name__ == "__main__":
    main()
