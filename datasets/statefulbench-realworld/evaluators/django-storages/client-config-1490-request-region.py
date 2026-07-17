#!/usr/bin/env python3
"""Evaluator for django-storages issue #1490; boto is fully faked."""

from __future__ import annotations

import threading
import sys
from pathlib import Path
from unittest.mock import patch


def main() -> None:
    checkout = Path(sys.argv[1]).resolve()
    sys.path.insert(0, str(checkout))

    from django.conf import settings

    if not settings.configured:
        settings.configure(SECRET_KEY="statefulbench", INSTALLED_APPS=[])
        import django

        django.setup()

    from botocore.config import Config
    from storages.backends import s3

    class Bucket:
        def __init__(self, region_name: str, name: str) -> None:
            self.region_name = region_name
            self.name = name

    class Resource:
        def __init__(self, region_name: str) -> None:
            self.region_name = region_name

        def Bucket(self, name: str) -> Bucket:
            return Bucket(self.region_name, name)

    class Session:
        def __init__(self) -> None:
            self.calls: list[dict[str, object]] = []
            self.lock = threading.Lock()

        def resource(self, service: str, **kwargs: object) -> Resource:
            assert service == "s3"
            with self.lock:
                self.calls.append(kwargs)
            return Resource(kwargs["region_name"])

    selected_region = threading.local()

    class RequestRegionStorage(s3.S3Storage):
        def get_region_name(self):
            return selected_region.name

    config = Config(signature_version="s3v4", proxies={"https": "proxy.test"})
    session = Session()
    session_calls: list[dict[str, object]] = []
    session_lock = threading.Lock()

    def create_session(**kwargs: object) -> Session:
        with session_lock:
            session_calls.append(kwargs)
        return session

    created_a = threading.Event()
    created_b = threading.Event()
    result: dict[str, object] = {}

    def use_bucket(region_name: str, own_created: threading.Event, other_created: threading.Event) -> None:
        selected_region.name = region_name
        result[f"{region_name}-connection"] = storage.connection
        own_created.set()
        assert other_created.wait(5), "other thread did not create its connection"
        result[f"{region_name}-bucket"] = storage.bucket
        result[f"{region_name}-unsigned"] = storage.unsigned_connection

    with patch.object(s3.boto3, "Session", side_effect=create_session):
        storage = RequestRegionStorage(
            bucket_name="request-bucket",
            session_profile="request-profile",
            endpoint_url="https://s3.test",
            use_ssl=True,
            verify="/srv/certs/custom-ca.pem",
            client_config=config,
        )
        brazil = threading.Thread(
            target=use_bucket,
            args=("sa-east-1", created_a, created_b),
        )
        australia = threading.Thread(
            target=use_bucket,
            args=("ap-southeast-2", created_b, created_a),
        )
        brazil.start()
        australia.start()
        brazil.join(5)
        australia.join(5)

    assert not brazil.is_alive()
    assert not australia.is_alive()
    for region_name in ("sa-east-1", "ap-southeast-2"):
        connection = result[f"{region_name}-connection"]
        bucket = result[f"{region_name}-bucket"]
        unsigned = result[f"{region_name}-unsigned"]
        assert connection is not unsigned
        assert bucket.region_name == region_name
        assert bucket.name == "request-bucket"
    assert {call["region_name"] for call in session.calls} == {
        "sa-east-1",
        "ap-southeast-2",
    }
    assert session_calls == [{"profile_name": "request-profile"}] * 4
    for call in session.calls:
        assert call["endpoint_url"] == "https://s3.test", call
        assert call["verify"] == "/srv/certs/custom-ca.pem", call
    signed_calls = [
        call for call in session.calls if call["config"].signature_version == "s3v4"
    ]
    unsigned_calls = [
        call
        for call in session.calls
        if call["config"].signature_version is s3.botocore.UNSIGNED
    ]
    assert len(signed_calls) == len(unsigned_calls) == 2
    for call in signed_calls:
        assert call["config"] is config, call
    for call in unsigned_calls:
        assert call["config"].proxies == {"https": "proxy.test"}, call


if __name__ == "__main__":
    main()
