#!/usr/bin/env python3
"""Evaluator for #1493's provider-backed credentials across resource use."""

from __future__ import annotations

import os
import sys
from pathlib import Path
from unittest.mock import patch


def main() -> None:
    checkout = Path(sys.argv[1]).resolve()
    deps = next(
        parent / "django-storages-deps"
        for parent in checkout.parents
        if (parent / "django-storages-deps").is_dir()
    )
    sys.path[:0] = [str(checkout), str(deps)]

    from django.conf import settings

    if not settings.configured:
        settings.configure(AWS_STORAGE_BUCKET_NAME="bucket", SECRET_KEY="evaluator")
        import django

        django.setup()

    from storages.backends import s3

    calls: list[dict[str, object]] = []

    class Resource:
        def __init__(self, access_key: str | object) -> None:
            self._access_key = access_key

        def use(self) -> str:
            if callable(self._access_key):
                return self._access_key()
            return self._access_key

    class Session:
        def __init__(self, **kwargs: object) -> None:
            calls.append(kwargs)
            explicit = any(
                kwargs.get(name)
                for name in (
                    "aws_access_key_id",
                    "aws_secret_access_key",
                    "aws_session_token",
                )
            )
            self._access_key: str | object = (
                kwargs["aws_access_key_id"]
                if explicit
                else lambda: os.environ["AWS_ACCESS_KEY_ID"]
            )

        def resource(self, service: str, **kwargs: object) -> Resource:
            assert service == "s3"
            return Resource(self._access_key)

    environment = {
        "AWS_ACCESS_KEY_ID": "machine-role-first",
        "AWS_SECRET_ACCESS_KEY": "machine-role-secret",
        "AWS_SESSION_TOKEN": "machine-role-token",
    }
    with patch.dict(os.environ, environment, clear=False):
        with patch.object(s3.boto3, "Session", side_effect=Session):
            default = s3.S3Storage()
            default_connection = default.connection
            assert default_connection.use() == "machine-role-first"

            os.environ["AWS_ACCESS_KEY_ID"] = "machine-role-refreshed"
            assert default_connection.use() == "machine-role-refreshed"

            s3.S3Storage(session_profile="named").connection
            explicit = s3.S3Storage(
                access_key="access",
                secret_key="secret",
                security_token="token",
            ).connection
            assert explicit.use() == "access"

    assert calls[0] in (
        {},
        {
            "aws_access_key_id": None,
            "aws_secret_access_key": None,
            "aws_session_token": None,
        },
    ), calls
    assert calls[1:] == [
        {"profile_name": "named"},
        {
            "aws_access_key_id": "access",
            "aws_secret_access_key": "secret",
            "aws_session_token": "token",
        },
    ], calls


if __name__ == "__main__":
    main()
