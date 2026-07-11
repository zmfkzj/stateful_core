#!/usr/bin/env python3
"""Evaluator for the #1472 equivalent-S3Storage connection reuse behavior."""

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
    def __init__(self, number: int) -> None:
        self.resource_calls: list[dict[str, object]] = []
        self.resource_value = object()
        self.number = number

    def resource(self, service: str, **kwargs: object) -> object:
        assert service == "s3"
        self.resource_calls.append(kwargs)
        return self.resource_value


def main() -> None:
    calls: list[dict[str, object]] = []
    sessions: list[Session] = []

    def create_session(**kwargs: object) -> Session:
        calls.append(kwargs)
        session = Session(len(calls))
        sessions.append(session)
        return session

    with patch("storages.backends.s3.boto3.Session", side_effect=create_session):
        common = {"client_config": {"cache": "shared"}}
        first = S3Storage(**common)
        second = S3Storage(**common)
        profiled = S3Storage(session_profile="other", **common)

        first_connection = first.connection
        second_connection = second.connection
        profiled_connection = profiled.connection

    assert first_connection is second_connection
    assert profiled_connection is not first_connection
    assert len(calls) == 2, calls
    assert calls[1] == {"profile_name": "other"}, calls
    assert sessions[0].resource_calls == [
        {
            "region_name": None,
            "use_ssl": True,
            "endpoint_url": None,
            "config": {"cache": "shared"},
            "verify": None,
        }
    ]


if __name__ == "__main__":
    main()
