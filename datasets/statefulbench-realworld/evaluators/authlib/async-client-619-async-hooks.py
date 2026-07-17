#!/usr/bin/env python3
"""Evaluator for Authlib issue #619's async access-token response-hook extension."""
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

    async def handler(request):
        return httpx.Response(
            200,
            json={"access_token": "raw", "token_type": "Bearer", "expires_in": 3600},
            request=request,
        )

    async def scenario() -> None:
        events = []
        expected_inputs = {}

        def synchronous(response):
            assert response.json()["access_token"] == expected_inputs[synchronous]
            events.append("sync")
            return httpx.Response(
                200,
                json={"access_token": "sync", "token_type": "Bearer"},
                request=response.request,
            )

        async def asynchronous(response):
            assert response.json()["access_token"] == expected_inputs[asynchronous]
            events.append("async:start")
            await asyncio.sleep(0)
            events.append("async:end")
            return httpx.Response(
                200,
                json={"access_token": "async", "token_type": "Bearer"},
                request=response.request,
            )

        async with AsyncOAuth2Client(
            "client-id", "client-secret", transport=httpx.MockTransport(handler)
        ) as client:
            client.register_compliance_hook("access_token_response", synchronous)
            client.register_compliance_hook("access_token_response", asynchronous)
            hooks = tuple(client.compliance_hook["access_token_response"])
            token_value = "raw"
            expected_events = []
            for hook in hooks:
                expected_inputs[hook] = token_value
                token_value = "sync" if hook is synchronous else "async"
                expected_events.extend(
                    ["sync"]
                    if hook is synchronous
                    else ["async:start", "async:end"]
                )
            token = await client.fetch_token(
                "https://issuer.test/token", grant_type="client_credentials"
            )
            assert token["access_token"] == token_value
        assert events == expected_events

        class HookFailure(Exception):
            pass

        async def failing(response):
            await asyncio.sleep(0)
            raise HookFailure("stop")

        async with AsyncOAuth2Client(
            "client-id", "client-secret", transport=httpx.MockTransport(handler)
        ) as client:
            client.register_compliance_hook("access_token_response", failing)
            try:
                await client.fetch_token(
                    "https://issuer.test/token", grant_type="client_credentials"
                )
            except HookFailure as error:
                assert str(error) == "stop"
            else:
                raise AssertionError("async hook errors must propagate")

    asyncio.run(scenario())


if __name__ == "__main__":
    main()
