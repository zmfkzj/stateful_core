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
        endpoint = "https://issuer.test/token?fixed=1"

        async def handler(request):
            calls.append(
                (
                    request.method,
                    str(request.url),
                    request.headers.get("Authorization"),
                )
            )
            if request.url.path == "/failed":
                return httpx.Response(
                    400,
                    json={"error": "invalid_client"},
                    request=request,
                )
            if request.url.path == "/token":
                return httpx.Response(
                    200,
                    json={
                        "access_token": f"token-{len(calls)}",
                        "token_type": "Bearer",
                        "expires_in": 3600,
                    },
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
            await client.fetch_token(endpoint, method="GET")
            assert client.metadata["token_endpoint"] == endpoint
            client.token["expires_at"] = 0
            response = await client.get("https://service.test/resource")
            assert response.json() == {"ok": True}

        assert [method for method, _, _ in calls] == ["GET", "POST", "GET"]
        assert calls[0][1].startswith(f"{endpoint}&")
        assert calls[1][1] == endpoint
        assert calls[-1][1] == "https://service.test/resource"
        assert calls[-1][2] == "Bearer token-2"
        assert updates == [("token-2", "token-1")]

        configured_endpoint = "https://configured.test/token?fixed=1"
        explicit_endpoint = "https://different.test/token?fixed=2"
        configured_calls = []

        async def configured_handler(request):
            configured_calls.append((request.method, str(request.url)))
            if request.url.path == "/token":
                return httpx.Response(
                    200,
                    json={"access_token": "configured", "token_type": "Bearer", "expires_in": 3600},
                    request=request,
                )
            return httpx.Response(200, json={"ok": True}, request=request)

        async with AsyncOAuth2Client(
            "client-id",
            "client-secret",
            grant_type="client_credentials",
            token={"access_token": "old", "token_type": "Bearer", "expires_at": 0},
            token_endpoint=configured_endpoint,
            transport=httpx.MockTransport(configured_handler),
        ) as client:
            await client.fetch_token(explicit_endpoint, method="GET")
            assert client.metadata["token_endpoint"] == configured_endpoint
            client.token["expires_at"] = 0
            await client.get("https://service.test/resource")

        assert configured_calls[0][0] == "GET"
        assert configured_calls[1] == ("POST", configured_endpoint)

        async with AsyncOAuth2Client(
            "client-id",
            "client-secret",
            transport=httpx.MockTransport(handler),
        ) as client:
            try:
                await client.fetch_token("https://issuer.test/failed", method="GET")
            except Exception:
                pass
            else:
                raise AssertionError("OAuth error response must raise")
            assert "token_endpoint" not in client.metadata

    asyncio.run(scenario())


if __name__ == "__main__":
    main()
