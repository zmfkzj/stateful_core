#!/usr/bin/env python3
"""Evaluator for Authlib issue #822's reusable remote-app OAuth session."""
import argparse
import sys
import types
from pathlib import Path


TOKEN = {"access_token": "resource-token", "expires_at": 4102444800}


def load_sync_app(repo: Path):
    sys.path.insert(0, str(repo))
    for name in tuple(sys.modules):
        if name == "authlib" or name.startswith("authlib."):
            del sys.modules[name]
    import authlib.integrations

    base_client = types.ModuleType("authlib.integrations.base_client")
    base_client.__path__ = [str(repo / "authlib/integrations/base_client")]
    sys.modules[base_client.__name__] = base_client
    from authlib.integrations.base_client.errors import MissingTokenError
    from authlib.integrations.base_client.async_app import AsyncOAuth2Mixin
    from authlib.integrations.base_client.sync_app import OAuth2Mixin

    return MissingTokenError, OAuth2Mixin, AsyncOAuth2Mixin


class Framework:
    def update_token(self, token, **kwargs):
        raise AssertionError("resource requests must not update the supplied token")


class ReusableSession:
    created = []

    def __init__(self, **metadata):
        self.metadata = metadata
        self.headers = {}
        self.token = None
        self.calls = []
        self.closed = False
        type(self).created.append(self)

    def __enter__(self):
        return self

    def __exit__(self, *args):
        self.closed = True

    def get(self, url, **kwargs):
        self.calls.append(("GET", url, kwargs, self.token))
        return {"url": url, "token": self.token}


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("repo", type=Path)
    args = parser.parse_args()
    MissingTokenError, OAuth2Mixin, AsyncOAuth2Mixin = load_sync_app(args.repo)
    assert not hasattr(AsyncOAuth2Mixin, "client")

    requested_token_calls = []
    fetched_requests = []
    request = object()

    def fetch_token(value):
        fetched_requests.append(value)
        return TOKEN

    class App(OAuth2Mixin):
        client_cls = ReusableSession

        def _get_requested_token(self, value):
            requested_token_calls.append(value)
            return super()._get_requested_token(value)

    app = App(
        Framework(),
        client_id="client-id",
        client_secret="client-secret",
        access_token_url="https://issuer.example/token",
        fetch_token=fetch_token,
    )
    try:
        with app.client():
            raise AssertionError("an unauthenticated reusable client was yielded")
    except MissingTokenError:
        pass

    with app.client(token=TOKEN) as client:
        first = client.get("https://resource.example/one")
        second = client.get("https://resource.example/two")
        assert first["token"] == TOKEN
        assert second["token"] == TOKEN
        assert client.calls == [
            ("GET", "https://resource.example/one", {}, TOKEN),
            ("GET", "https://resource.example/two", {}, TOKEN),
        ]
        assert client.closed is False

    with app.client(request=request) as client:
        first = client.get("https://resource.example/three")
        second = client.get("https://resource.example/four")
        assert first["token"] == TOKEN
        assert second["token"] == TOKEN
        assert client.calls == [
            ("GET", "https://resource.example/three", {}, TOKEN),
            ("GET", "https://resource.example/four", {}, TOKEN),
        ]
        assert client.closed is False

    assert requested_token_calls == [None, request]
    assert fetched_requests == [request]
    assert len(ReusableSession.created) == 2
    assert all(session.closed for session in ReusableSession.created)


if __name__ == "__main__":
    main()
