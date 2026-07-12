#!/usr/bin/env python3
"""Evaluator for Authlib issue #632 automatic expired-token refresh opt-out."""

import argparse
import sys
from pathlib import Path


class Response:
    status_code = 200

    def __init__(self, payload):
        self.payload = payload

    def json(self):
        return self.payload


class FakeSession:
    def __init__(self, responses=()):
        self.responses = iter(responses)
        self.posts = []

    def post(self, url, data=None, headers=None, auth=None, **kwargs):
        self.posts.append((url, data, headers, auth, kwargs))
        return Response(next(self.responses))


def expired_token(refresh_token=None):
    token = {"access_token": "old-access", "token_type": "Bearer", "expires_at": 0}
    if refresh_token:
        token["refresh_token"] = refresh_token
    return token


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("repo", type=Path)
    args = parser.parse_args()
    sys.path.insert(0, str(args.repo))

    from authlib.oauth2.client import OAuth2Client

    # Opt-out: no endpoint request is made, even when both a refresh token and
    # endpoint exist. The expired token is retained for caller-controlled flow.
    disabled_session = FakeSession()
    disabled = OAuth2Client(
        disabled_session,
        token=expired_token("refresh-secret"),
        token_endpoint="https://issuer.test/token",
        grant_type="client_credentials",
        token_auto_update=False,
    )
    assert disabled.ensure_active_token() is False
    assert disabled_session.posts == []
    assert disabled.token["access_token"] == "old-access"

    # The normal default remains automatic client-credentials renewal.
    automatic_session = FakeSession(
        [{"access_token": "new-access", "token_type": "Bearer"}]
    )
    automatic = OAuth2Client(
        automatic_session,
        token=expired_token(),
        token_endpoint="https://issuer.test/token",
        grant_type="client_credentials",
    )
    assert automatic.ensure_active_token() is True
    assert automatic_session.posts[0][1]["grant_type"] == "client_credentials"
    assert automatic.token["access_token"] == "new-access"

    # Boundary: disabling automation does not disable an explicit, caller-made
    # refresh request with caller-selected endpoint details.
    manual_session = FakeSession(
        [{"access_token": "manual-access", "token_type": "Bearer"}]
    )
    manual = OAuth2Client(
        manual_session,
        token=expired_token("refresh-secret"),
        token_auto_update=False,
    )
    token = manual.refresh_token("https://issuer.test/manual-token")
    assert token["access_token"] == "manual-access"
    assert manual_session.posts[0][0] == "https://issuer.test/manual-token"
    assert manual_session.posts[0][1] == {
        "grant_type": "refresh_token",
        "refresh_token": "refresh-secret",
    }

    # Error: the default policy still propagates a token-endpoint failure.
    from authlib.oauth2.client import OAuth2Error

    rejected_session = FakeSession(
        [{"error": "invalid_client", "error_description": "client rejected"}]
    )
    rejected = OAuth2Client(
        rejected_session,
        token=expired_token(),
        token_endpoint="https://issuer.test/token",
        grant_type="client_credentials",
    )
    try:
        rejected.ensure_active_token()
    except OAuth2Error as error:
        assert error.error == "invalid_client"
    else:
        raise AssertionError("default automatic refresh must propagate OAuth errors")


if __name__ == "__main__":
    main()
