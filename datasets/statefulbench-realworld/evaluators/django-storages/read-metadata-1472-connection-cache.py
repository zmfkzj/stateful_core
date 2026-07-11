#!/usr/bin/env python3
"""Evaluator for #1472's semantically isolated per-thread S3 resources."""

from __future__ import annotations

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

    from botocore.config import Config
    from storages.backends import s3

    class Session:
        def __init__(self, number: int) -> None:
            self.number = number
            self.resource_calls: list[dict[str, object]] = []

        def resource(self, service: str, **kwargs: object) -> object:
            assert service == "s3"
            self.resource_calls.append(kwargs)
            return object()

    calls: list[dict[str, object]] = []
    sessions: list[Session] = []

    def create_session(**kwargs: object) -> Session:
        calls.append(kwargs)
        session = Session(len(calls))
        sessions.append(session)
        return session

    equivalent_a = Config(
        proxies={"https": "https://proxy.test", "http": "http://proxy.test"},
        retries={"mode": "standard", "max_attempts": 3},
    )
    equivalent_b = Config(
        retries={"max_attempts": 3, "mode": "standard"},
        proxies={"http": "http://proxy.test", "https": "https://proxy.test"},
    )
    different_config = Config(retries={"mode": "adaptive", "max_attempts": 3})

    with patch.object(s3.boto3, "Session", side_effect=create_session):
        first = s3.S3Storage(client_config=equivalent_a)
        equivalent = s3.S3Storage(client_config=equivalent_b)
        profile = s3.S3Storage(session_profile="other", client_config=equivalent_a)
        tenant_a = s3.S3Storage(
            access_key="tenant-a",
            secret_key="tenant-a-secret",
            security_token="tenant-a-token",
            client_config=equivalent_a,
        )
        tenant_b = s3.S3Storage(
            access_key="tenant-b",
            secret_key="tenant-b-secret",
            security_token="tenant-b-token",
            client_config=equivalent_a,
        )
        endpoint = s3.S3Storage(
            endpoint_url="https://s3.other.test", client_config=equivalent_a
        )
        region = s3.S3Storage(region_name="eu-west-1", client_config=equivalent_a)
        insecure = s3.S3Storage(use_ssl=False, client_config=equivalent_a)
        unverified = s3.S3Storage(verify=False, client_config=equivalent_a)
        config = s3.S3Storage(client_config=different_config)

        first_connection = first.connection
        equivalent_connection = equivalent.connection
        isolated_connections = [
            storage.connection
            for storage in (
                profile,
                tenant_a,
                tenant_b,
                endpoint,
                region,
                insecure,
                unverified,
                config,
            )
        ]

    assert first_connection is equivalent_connection
    assert all(connection is not first_connection for connection in isolated_connections)
    assert len(set(map(id, isolated_connections))) == len(isolated_connections)
    assert len(calls) == 9, calls
    assert calls[1] == {"profile_name": "other"}, calls
    assert calls[2:4] == [
        {
            "aws_access_key_id": "tenant-a",
            "aws_secret_access_key": "tenant-a-secret",
            "aws_session_token": "tenant-a-token",
        },
        {
            "aws_access_key_id": "tenant-b",
            "aws_secret_access_key": "tenant-b-secret",
            "aws_session_token": "tenant-b-token",
        },
    ], calls
    assert sessions[0].resource_calls == [
        {
            "region_name": None,
            "use_ssl": True,
            "endpoint_url": None,
            "config": equivalent_a,
            "verify": None,
        }
    ]
    assert sessions[4].resource_calls[0]["endpoint_url"] == "https://s3.other.test"
    assert sessions[5].resource_calls[0]["region_name"] == "eu-west-1"
    assert sessions[6].resource_calls[0]["use_ssl"] is False
    assert sessions[7].resource_calls[0]["verify"] is False
    assert sessions[8].resource_calls[0]["config"] is different_config


if __name__ == "__main__":
    main()
