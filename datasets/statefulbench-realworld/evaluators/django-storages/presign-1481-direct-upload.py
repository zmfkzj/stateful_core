#!/usr/bin/env python3
"""Evaluator for django-storages issue #1481."""

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
        return f"https://presigned.example/{operation}"


class UnsignedClient:
    def generate_presigned_url(self, *args: object, **kwargs: object) -> str:
        raise AssertionError("direct uploads must not use the unsigned connection")


class Resource:
    def __init__(self, client: object) -> None:
        self.meta = type("Meta", (), {"client": client})()

    def Bucket(self, name: str) -> object:
        return type("Bucket", (), {"name": name})()


def storage_with(client: Client):
    from storages.backends.s3 import S3Storage

    storage = S3Storage()
    storage._connections.connection = Resource(client)
    storage._unsigned_connections.connection = Resource(UnsignedClient())
    return storage


def main() -> None:
    checkout = Path(sys.argv[1]).resolve()
    deps = next(
        parent / "django-storages-deps"
        for parent in checkout.parents
        if (parent / "django-storages-deps").is_dir()
    )
    sys.path[:0] = [str(checkout), str(deps)]
    configure_django()



    client = Client()
    storage = storage_with(client)
    assert storage.generate_presigned_upload_url(
        "uploads/report.pdf", parameters={"ContentType": "application/pdf"}, expire=61
    ) == "https://presigned.example/put_object"
    assert client.calls == [
        (
            "put_object",
            {
                "Bucket": "benchmark-bucket",
                "Key": "uploads/report.pdf",
                "ContentType": "application/pdf",
            },
            61,
            "PUT",
        )
    ], client.calls

    client.calls.clear()
    assert storage.url("downloads/report.pdf") == "https://presigned.example/get_object"
    assert client.calls == [
        (
            "get_object",
            {"Bucket": "benchmark-bucket", "Key": "downloads/report.pdf"},
            storage.querystring_expire,
            None,
        )
    ], client.calls

    storage.custom_domain = "media.example"
    client.calls.clear()
    assert storage.url("downloads/report.pdf") == "https://media.example/downloads/report.pdf"
    assert client.calls == [], client.calls

    storage.querystring_auth = False
    assert storage.generate_presigned_upload_url("uploads/private.pdf") == (
        "https://presigned.example/put_object"
    )
    assert client.calls == [
        (
            "put_object",
            {"Bucket": "benchmark-bucket", "Key": "uploads/private.pdf"},
            storage.querystring_expire,
            "PUT",
        )
    ], client.calls


if __name__ == "__main__":
    main()
