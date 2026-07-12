#!/usr/bin/env python3
"""Evaluator for Authlib issue #780's remote-app client-credentials refresh."""
import argparse
import sys
import types
from pathlib import Path


EXPIRED = {"access_token": "expired", "expires_at": 0}
FRESH = {"access_token": "fresh", "expires_at": 4102444800}


def load_sync_app(repo: Path):
    sys.path.insert(0, str(repo))
    for name in tuple(sys.modules):
        if name == "authlib" or name.startswith("authlib."):
            del sys.modules[name]
    import authlib.integrations

    base_client = types.ModuleType("authlib.integrations.base_client")
    base_client.__path__ = [str(repo / "authlib/integrations/base_client")]
    sys.modules[base_client.__name__] = base_client
    from authlib.integrations.base_client.sync_app import OAuth2Mixin

    return OAuth2Mixin


class Framework:
    def __init__(self):
        self.updated = []

    def update_token(self, token, **kwargs):
        self.updated.append((token, kwargs))


class RefreshingSession:
    def __init__(self, **metadata):
        self.metadata = metadata
        self.headers = {}
        self.token = None
        self.requests = []
        self.closed = False

    def __enter__(self):
        return self

    def __exit__(self, *args):
        self.closed = True

    def request(self, method, url, **kwargs):
        self.requests.append((method, url, kwargs))
        assert self.token == EXPIRED
        assert self.metadata["grant_type"] == "client_credentials"
        assert self.metadata["token_endpoint"] == "https://issuer.example/token"
        assert url != self.metadata["token_endpoint"]
        self.token = FRESH
        return {"access_token": self.token["access_token"]}


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("repo", type=Path)
    args = parser.parse_args()
    OAuth2Mixin = load_sync_app(args.repo)

    class App(OAuth2Mixin):
        client_cls = RefreshingSession

    app = App(
        Framework(),
        client_id="client-id",
        client_secret="client-secret",
        access_token_url="https://issuer.example/token",
    )
    with app._get_oauth_client() as session:
        session.token = EXPIRED
        response = session.request("GET", "https://resource.example/profile")

    assert response == {"access_token": "fresh"}
    assert session.closed is True
    assert session.requests == [("GET", "https://resource.example/profile", {})]
    with App(Framework(), authorize_url="https://issuer.example/authorize",
             access_token_url="https://issuer.example/token")._get_oauth_client() as auth_code:
        assert "grant_type" not in auth_code.metadata
    with App(Framework(), access_token_url="https://issuer.example/token",
             client_kwargs={"grant_type": "password"})._get_oauth_client() as explicit:
        assert explicit.metadata["grant_type"] == "password"
    with App(Framework(), access_token_url="https://issuer.example/token",
             access_token_params={"username": "alice"})._get_oauth_client() as parameterized:
        assert "grant_type" not in parameterized.metadata


if __name__ == "__main__":
    main()
