#!/usr/bin/env python3
"""Evaluator for the cookie-policy extension motivated by Requests issue #7122."""
import argparse
import http.cookiejar
import sys
from pathlib import Path


class Policy(http.cookiejar.DefaultCookiePolicy):
    def __init__(self, allowed: bool) -> None:
        super().__init__()
        self.allowed = allowed

    def return_ok(self, cookie, request) -> bool:
        return self.allowed


def secure_cookie() -> http.cookiejar.Cookie:
    return http.cookiejar.Cookie(
        version=0,
        name="secure",
        value="yes",
        port=None,
        port_specified=False,
        domain="example.test",
        domain_specified=True,
        domain_initial_dot=False,
        path="/",
        path_specified=True,
        secure=True,
        expires=None,
        discard=True,
        comment=None,
        comment_url=None,
        rest={},
        rfc2109=False,
    )


def prepared(Session, Request, policy: Policy):
    jar = http.cookiejar.CookieJar(policy=policy)
    jar.set_cookie(secure_cookie())
    session = Session()
    session.cookies = jar
    return session.prepare_request(Request("GET", "http://example.test/"))


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

    allowed = Policy(True)
    accepted = prepared(Session, Request, allowed)
    assert accepted.headers.get("Cookie") == "secure=yes"
    assert accepted._cookies._policy is allowed

    denied = prepared(Session, Request, Policy(False))
    assert "Cookie" not in denied.headers


if __name__ == "__main__":
    main()
