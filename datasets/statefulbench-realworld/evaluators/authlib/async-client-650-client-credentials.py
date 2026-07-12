#!/usr/bin/env python3
"""Evaluator for Authlib issue #650."""
import argparse
import asyncio
import importlib
import sys
import types
from pathlib import Path


def load_async_client(repo: Path):
    try:
        from authlib.integrations.httpx_client import AsyncOAuth2Client
    except ModuleNotFoundError as error:
        if error.name != "joserfc":
            raise
        base_client = types.ModuleType("authlib.integrations.base_client")
        for name in (
            "InvalidTokenError",
            "MissingTokenError",
            "OAuthError",
            "UnsupportedTokenTypeError",
        ):
            setattr(base_client, name, type(name, (Exception,), {}))
        sys.modules[base_client.__name__] = base_client
        package = types.ModuleType("authlib.integrations.httpx_client")
        package.__path__ = [str(repo / "authlib" / "integrations" / "httpx_client")]
        sys.modules[package.__name__] = package
        AsyncOAuth2Client = importlib.import_module(
            "authlib.integrations.httpx_client.oauth2_client"
        ).AsyncOAuth2Client
    return AsyncOAuth2Client


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("repo", type=Path)
    args = parser.parse_args()
    sys.path.insert(0, str(args.repo))

    import httpx
    AsyncOAuth2Client = load_async_client(args.repo)

    async def scenario() -> None:
        calls = []
        updates = []

        async def handler(request):
            calls.append((request.method, str(request.url), request.headers.get("Authorization")))
            if request.url.path == "/token":
                return httpx.Response(
                    200,
                    json={"access_token": f"token-{len(calls)}", "token_type": "Bearer", "expires_in": 3600},
                    request=request,
                )
            assert request.url.path == "/resource"
            return httpx.Response(200, json={"ok": True}, request=request)

        async def update_token(token, access_token):
            updates.append((token["access_token"], access_token))

        async with AsyncOAuth2Client(
            "client-id",
            "client-secret",
            scope="scope",
            grant_type="client_credentials",
            token={"access_token": "old", "token_type": "Bearer", "expires_at": 0},
            update_token=update_token,
            transport=httpx.MockTransport(handler),
        ) as client:
            await client.fetch_token("https://issuer.test/token")
            client.token["expires_at"] = 0
            response = await client.get("https://service.test/resource")
            assert response.json() == {"ok": True}

        assert [url for _, url, _ in calls] == [
            "https://issuer.test/token",
            "https://issuer.test/token",
            "https://service.test/resource",
        ]
        assert calls[-1][2] == "Bearer token-2"
        assert updates == [("token-2", "token-1")]

    asyncio.run(scenario())


if __name__ == "__main__":
    main()
