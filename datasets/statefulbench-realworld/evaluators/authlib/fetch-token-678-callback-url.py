#!/usr/bin/env python3
"""Evaluator for Authlib issue #678 safe authorization-code callback URLs."""

import argparse
import sys
from pathlib import Path
from urllib.parse import parse_qs


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("repo", type=Path)
    args = parser.parse_args()
    sys.path.insert(0, str(args.repo))

    from authlib.oauth2.client import OAuth2Client
    from authlib.oauth2.rfc6749.errors import MismatchingStateException, OAuth2Error

    class RecordingClient(OAuth2Client):
        def __init__(self, **kwargs):
            super().__init__(session=object(), **kwargs)
            self.requests = []

        def _fetch_token(self, url, body="", auth=None, method="POST", headers=None, **kwargs):
            self.requests.append((url, parse_qs(body), auth))
            return {"access_token": "token", "token_type": "Bearer"}

    client = RecordingClient(
        client_id="configured-client",
        client_secret="configured-secret",
        state="csrf-state",
        token_endpoint="https://issuer.test/token",
    )

    # Query callbacks are parsed with normal form decoding, then posted to metadata endpoint.
    client.fetch_token("https://client.test/callback?code=a%2Bb%2Fc%3D&state=csrf-state")
    url, body, auth = client.requests[-1]
    assert url == "https://issuer.test/token"
    assert body == {"grant_type": ["authorization_code"], "code": ["a+b/c="]}
    assert auth.client_id == "configured-client"
    assert auth.client_secret == "configured-secret"

    # State remains checked before any token endpoint request.
    try:
        client.fetch_token("https://client.test/callback?code=unused&state=wrong-state")
    except MismatchingStateException:
        pass
    else:
        raise AssertionError("callback state mismatch must be rejected")
    assert len(client.requests) == 1

    # Authorization-server errors are callbacks, not token endpoints; credentials
    # must never be sent to a callback URL.
    try:
        client.fetch_token(
            "https://client.test/callback?error=access_denied"
            "&error_description=user+denied&state=csrf-state"
        )
    except OAuth2Error as error:
        assert error.error == "access_denied"
        assert error.description == "user denied"
    else:
        raise AssertionError("OAuth error callback must be rejected")
    assert len(client.requests) == 1

    # Ambiguous authorization codes are invalid before any credentialed request.
    try:
        client.fetch_token(
            "https://client.test/callback?code=first&code=second&state=csrf-state"
        )
    except ValueError as error:
        assert str(error) == "authorization response has multiple code parameters"
    else:
        raise AssertionError("duplicate callback codes must be rejected")
    assert len(client.requests) == 1

    # OAuth authorization-code responses use query, not fragment, parameters.
    try:
        client.fetch_token("https://client.test/callback#code=fragment-code")
    except ValueError as error:
        assert str(error) == "authorization code response must use query parameters"
    else:
        raise AssertionError("fragment code must not be exchanged")
    assert len(client.requests) == 1

    # A configured endpoint's own query remains an endpoint, not a callback.
    endpoint = "https://issuer.test/token?tenant=example"
    endpoint_client = RecordingClient(client_id="id", client_secret="secret", token_endpoint=endpoint)
    endpoint_client.fetch_token()
    url, body, _ = endpoint_client.requests[-1]
    assert url == endpoint
    assert body == {"grant_type": ["client_credentials"]}


if __name__ == "__main__":
    main()
