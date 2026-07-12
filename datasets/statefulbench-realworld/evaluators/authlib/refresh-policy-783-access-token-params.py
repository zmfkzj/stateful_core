#!/usr/bin/env python3
"""Evaluator for Authlib issue #783 automatic client-credentials refresh."""

import argparse
import sys
from pathlib import Path


class Response:
    def __init__(self, payload):
        self.payload = payload
        self.status_code = 200

    def json(self):
        return self.payload


class FakeSession:
    def __init__(self, responses):
        self.responses = iter(responses)
        self.posts = []

    def post(self, url, data=None, headers=None, auth=None, **kwargs):
        self.posts.append((url, data, headers, auth, kwargs))
        return Response(next(self.responses))


def expired_token():
    return {"access_token": "old-access", "token_type": "Bearer", "expires_at": 0}


def client(session, **metadata):
    from authlib.oauth2.client import OAuth2Client

    return OAuth2Client(
        session,
        client_id="client-id",
        token=expired_token(),
        token_endpoint="https://issuer.test/token",
        grant_type="client_credentials",
        **metadata,
    )


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("repo", type=Path)
    args = parser.parse_args()
    sys.path.insert(0, str(args.repo))

    params = {"audience": "inventory-api", "grant_type": "authorization_code"}
    session = FakeSession([{"access_token": "new-access", "token_type": "Bearer"}])
    updates = []
    oauth = client(session, access_token_params=params)
    oauth.update_token = lambda token, **kwargs: updates.append((dict(token), kwargs))

    # Normal + precedence: registered parameters are forwarded, but metadata
    # must not change the automatic client-credentials grant.
    assert oauth.ensure_active_token() is True
    assert session.posts[0][0] == "https://issuer.test/token"
    assert session.posts[0][1] == {
        "grant_type": "client_credentials",
        "audience": "inventory-api",
    }
    assert params == {"audience": "inventory-api", "grant_type": "authorization_code"}
    assert updates == [
        (
            {"access_token": "new-access", "token_type": "Bearer"},
            {"access_token": "old-access"},
        )
    ]

    # Error: token-endpoint OAuth errors remain visible and do not call a
    # token updater with untrusted error content.
    from authlib.oauth2.client import OAuth2Error

    rejected = FakeSession(
        [{"error": "access_denied", "error_description": "audience denied"}]
    )
    oauth = client(rejected, access_token_params={"audience": "inventory-api"})
    oauth.update_token = lambda *args, **kwargs: (_ for _ in ()).throw(
        AssertionError("update_token must not run after an OAuth error")
    )
    try:
        oauth.ensure_active_token()
    except OAuth2Error as error:
        assert error.error == "access_denied"
        assert error.description == "audience denied"
    else:
        raise AssertionError("token endpoint error must be propagated")
    assert rejected.posts[0][1]["audience"] == "inventory-api"


if __name__ == "__main__":
    main()
