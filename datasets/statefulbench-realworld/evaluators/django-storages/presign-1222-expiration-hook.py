#!/usr/bin/env python3
"""Evaluator for django-storages issue #1222."""

from __future__ import annotations

import sys
from pathlib import Path


def configure_django() -> None:
    from django.conf import settings

    if not settings.configured:
        settings.configure(
            AWS_STORAGE_BUCKET_NAME="benchmark-bucket",
            AWS_ACCESS_KEY_ID="benchmark-key",
            AWS_SECRET_ACCESS_KEY="benchmark-secret",
            INSTALLED_APPS=[],
            SECRET_KEY="benchmark",
        )
        import django

        django.setup()


class Client:
    def __init__(self) -> None:
        self.calls: list[tuple[str, dict[str, object], int, str | None]] = []

    def generate_presigned_url(
        self,
        operation: str,
        *,
        Params: dict[str, object],
        ExpiresIn: int,
        HttpMethod: str | None,
    ) -> str:
        self.calls.append((operation, Params, ExpiresIn, HttpMethod))
        return "https://presigned.example/get"


class Resource:
    def __init__(self, client: Client) -> None:
        self.meta = type("Meta", (), {"client": client})()

    def Bucket(self, name: str) -> object:
        return type("Bucket", (), {"name": name})()


class Signer:
    def __init__(self) -> None:
        self.calls: list[tuple[str, object]] = []

    def generate_presigned_url(self, url: str, *, date_less_than: object) -> str:
        self.calls.append((url, date_less_than))
        return "https://signed.example/get"


def main() -> None:
    sys.path[:0] = [
        "/private/tmp/statefulbench-realworld-curation/django-storages-deps",
        str(Path(sys.argv[1]).resolve()),
    ]
    configure_django()

    from storages.backends.s3 import S3Storage

    marker = object()

    class BucketedExpirationStorage(S3Storage):
        """Uses a benchmark-defined hook to select discrete expiry instants."""

        seen: list[int] = []

        def get_presigned_url_expiration(self, expire):
            self.seen.append(expire)
            return marker

    client = Client()
    signer = Signer()
    storage = BucketedExpirationStorage(cloudfront_signer=signer)
    storage.custom_domain = "media.example"
    storage._connections.connection = Resource(client)
    storage._unsigned_connections.connection = Resource(client)

    assert storage.url("report.pdf", expire=91) == "https://signed.example/get"
    assert storage.seen == [91], storage.seen
    assert signer.calls == [
        ("https://media.example/report.pdf", marker)
    ], signer.calls
    assert client.calls == [], client.calls

    storage.querystring_auth = False
    signer.calls.clear()
    assert storage.url("report.pdf", expire=17) == "https://media.example/report.pdf"
    assert storage.seen == [91], storage.seen
    assert signer.calls == [], signer.calls

    storage.custom_domain = None
    storage.querystring_auth = True
    assert storage.url("report.pdf", expire=23) == "https://presigned.example/get"
    assert client.calls == [
        (
            "get_object",
            {"Bucket": "benchmark-bucket", "Key": "report.pdf"},
            23,
            None,
        )
    ], client.calls


if __name__ == "__main__":
    main()
