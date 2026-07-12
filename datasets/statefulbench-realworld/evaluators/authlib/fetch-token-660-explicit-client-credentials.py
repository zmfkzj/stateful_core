#!/usr/bin/env python3
"""Evaluator for Authlib issue #660 explicit client-credentials renewal."""

import argparse
import base64
import sys
from pathlib import Path
from urllib.parse import parse_qs


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("repo", type=Path)
    args = parser.parse_args()
    sys.path.insert(0, str(args.repo))

    from authlib.oauth2.auth import ClientAuth
    from authlib.oauth2.client import OAuth2Client

    class RecordingClient(OAuth2Client):
        def __init__(self, **kwargs):
            super().__init__(session=object(), **kwargs)
            self.requests = []

        def _fetch_token(self, url, body="", auth=None, method="POST", headers=None, **kwargs):
            request_headers = dict(headers)
            _, request_headers, body = auth.prepare(method, url, request_headers, body)
            self.requests.append((url, parse_qs(body), request_headers))
            return {"access_token": "replacement", "token_type": "Bearer", "expires_at": 0}

    client = RecordingClient(
        client_id="configured-client",
        client_secret="configured-secret",
        token_endpoint="https://issuer.test/token",
        token={"access_token": "expired", "token_type": "Bearer", "expires_at": 0},
    )

    # Explicit client_credentials must define the later automatic-renewal policy.
    client.fetch_token(grant_type="client_credentials")
    assert client.metadata["grant_type"] == "client_credentials"
    client.ensure_active_token()
    assert len(client.requests) == 2
    url, body, headers = client.requests[-1]
    assert url == "https://issuer.test/token"
    assert body == {"grant_type": ["client_credentials"]}
    expected = base64.b64encode(b"configured-client:configured-secret").decode("ascii")
    assert headers["Authorization"] == f"Basic {expected}"

    # A supplied auth object remains authoritative and does not leak configured creds.
    override = ClientAuth("override-client", "override-secret", "client_secret_post")
    client.fetch_token("https://issuer.test/override", auth=override, grant_type="client_credentials")
    _, body, headers = client.requests[-1]
    assert body["client_id"] == ["override-client"]
    assert body["client_secret"] == ["override-secret"]
    assert "Authorization" not in headers


if __name__ == "__main__":
    main()
