#!/usr/bin/env python3
"""Evaluator for django-storages issue #1490; boto is fully faked."""

from __future__ import annotations

import sys
from pathlib import Path
from unittest.mock import patch


def main() -> None:
    checkout = Path(sys.argv[1]).resolve()
    deps = next(parent / "django-storages-deps" for parent in checkout.parents if (parent / "django-storages-deps").is_dir())
    sys.path[:0] = [str(checkout), str(deps)]

    from django.conf import settings

    if not settings.configured:
        settings.configure(SECRET_KEY="statefulbench", INSTALLED_APPS=[])
        import django

        django.setup()

    from botocore.config import Config
    from storages.backends import s3

    class Resource:
        def Bucket(self, name: str) -> object:
            return object()

    class Session:
        def __init__(self) -> None:
            self.calls: list[dict[str, object]] = []

        def resource(self, service: str, **kwargs: object) -> Resource:
            assert service == "s3"
            self.calls.append(kwargs)
            return Resource()

    selected_region = "sa-east-1"

    class RequestRegionStorage(s3.S3Storage):
        def get_region_name(self):
            return selected_region

    config = Config(signature_version="s3v4", proxies={"https": "proxy.test"})
    session = Session()
    session_calls: list[dict[str, object]] = []

    def create_session(**kwargs: object) -> Session:
        session_calls.append(kwargs)
        return session

    with patch.object(s3.boto3, "Session", side_effect=create_session):
        storage = RequestRegionStorage(
            session_profile="request-profile",
            endpoint_url="https://s3.test",
            use_ssl=True,
            verify="/srv/certs/custom-ca.pem",
            client_config=config,
        )
        brazil_bucket = storage.bucket
        brazil_connection = storage.connection
        selected_region = "ap-southeast-2"
        australia_bucket = storage.bucket
        australia_connection = storage.connection
        selected_region = "sa-east-1"
        brazil_unsigned = storage.unsigned_connection
        selected_region = "ap-southeast-2"
        australia_unsigned = storage.unsigned_connection

    assert brazil_bucket is not australia_bucket
    assert brazil_connection is not australia_connection
    assert brazil_unsigned is not australia_unsigned
    assert [call["region_name"] for call in session.calls] == [
        "sa-east-1",
        "ap-southeast-2",
        "sa-east-1",
        "ap-southeast-2",
    ]
    assert session_calls == [{"profile_name": "request-profile"}] * 4
    for call in session.calls:
        assert call["endpoint_url"] == "https://s3.test", call
        assert call["verify"] == "/srv/certs/custom-ca.pem", call
    for call in session.calls[:2]:
        assert call["config"] is config, call
    for call in session.calls[2:]:
        assert call["config"].signature_version is s3.botocore.UNSIGNED, call
        assert call["config"].proxies == {"https": "proxy.test"}, call


if __name__ == "__main__":
    main()
