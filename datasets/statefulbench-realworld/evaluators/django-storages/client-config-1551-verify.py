#!/usr/bin/env python3
"""Evaluator for django-storages issue #1551; boto is fully faked."""

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

    class Session:
        def __init__(self) -> None:
            self.calls: list[dict[str, object]] = []

        def resource(self, service: str, **kwargs: object) -> object:
            assert service == "s3"
            self.calls.append(kwargs)
            return object()

    config = Config(signature_version="s3v4", proxies={"https": "proxy.test"})
    false_session = Session()
    with patch.object(s3.boto3, "Session", return_value=false_session):
        storage = s3.S3Storage(
            region_name="eu-west-1",
            endpoint_url="https://s3.test",
            use_ssl=True,
            verify="False",
            client_config=config,
        )
        storage.connection
        storage.unsigned_connection

    false_call = false_session.calls[0]
    assert false_call["verify"] is False, false_call
    assert false_call["region_name"] == "eu-west-1", false_call
    assert false_call["endpoint_url"] == "https://s3.test", false_call
    assert false_call["config"] is config, false_call
    assert [call["verify"] for call in false_session.calls] == [False, False]

    bundle_session = Session()
    with patch.object(s3.boto3, "Session", return_value=bundle_session):
        s3.S3Storage(verify="/srv/certs/custom-ca.pem").connection
    assert bundle_session.calls[0]["verify"] == "/srv/certs/custom-ca.pem"


if __name__ == "__main__":
    main()
