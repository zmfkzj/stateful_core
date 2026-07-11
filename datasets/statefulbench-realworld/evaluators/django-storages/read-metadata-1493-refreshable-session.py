#!/usr/bin/env python3
"""Evaluator for the #1493 refreshable default-credential session behavior."""

from __future__ import annotations

import sys
from pathlib import Path
from unittest.mock import patch


checkout = Path(sys.argv[1]).resolve()
sys.path.insert(0, str(checkout))

from django.conf import settings

if not settings.configured:
    settings.configure(
        AWS_STORAGE_BUCKET_NAME="bucket",
        DEFAULT_FILE_STORAGE="storages.backends.s3.S3Storage",
        SECRET_KEY="evaluator",
    )

from storages.backends.s3 import S3Storage


class Session:
    def resource(self, service: str, **kwargs: object) -> object:
        assert service == "s3"
        return object()


def main() -> None:
    calls: list[dict[str, object]] = []

    def create_session(**kwargs: object) -> Session:
        calls.append(kwargs)
        return Session()

    with patch("storages.backends.s3.boto3.Session", side_effect=create_session):
        S3Storage(
            access_key=None,
            secret_key=None,
            security_token=None,
        ).connection
        S3Storage(session_profile="named").connection
        S3Storage(
            access_key="access",
            secret_key="secret",
            security_token="token",
        ).connection

    assert calls == [
        {},
        {"profile_name": "named"},
        {
            "aws_access_key_id": "access",
            "aws_secret_access_key": "secret",
            "aws_session_token": "token",
        },
    ], calls


if __name__ == "__main__":
    main()
