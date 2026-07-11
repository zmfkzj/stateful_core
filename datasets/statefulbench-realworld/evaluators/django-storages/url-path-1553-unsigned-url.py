#!/usr/bin/env python3
"""Evaluator for django-storages issue #1553."""
import argparse
import sys
from pathlib import Path


def dependencies(checkout: Path) -> Path:
    return next(
        parent / "django-storages-deps"
        for parent in checkout.resolve().parents
        if (parent / "django-storages-deps").is_dir()
    )


class Bucket:
    name = "assets"


class Client:
    def generate_presigned_url(self, *args, **kwargs):
        raise AssertionError("unsigned URL invoked presigning")


class Meta:
    client = Client()


class UnsignedConnection:
    meta = Meta()


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("repo", type=Path)
    args = parser.parse_args()
    sys.path[:0] = [str(args.repo), str(dependencies(args.repo))]

    from storages.backends.s3 import S3Storage

    class GuardedStorage(S3Storage):
        @property
        def unsigned_connection(self):
            self.unsigned_connection_accesses += 1
            return UnsignedConnection()

    storage = object.__new__(GuardedStorage)
    storage.location = "media"
    storage.custom_domain = None
    storage.url_protocol = "https:"
    storage.querystring_auth = False
    storage.cloudfront_signer = None
    storage.querystring_expire = 3600
    storage.endpoint_url = None
    storage.bucket_name = "assets"
    storage._bucket = Bucket()
    storage.unsigned_connection_accesses = 0

    storage.region_name = "us-west-2"
    storage.addressing_style = "virtual"

    assert storage.url(
        "report.pdf",
        parameters={"response-content-disposition": "attachment; filename=report.pdf"},
    ) == (
        "https://assets.s3.us-west-2.amazonaws.com/media/report.pdf?"
        "response-content-disposition=attachment%3B+filename%3Dreport.pdf"
    )

    storage.region_name = "cn-north-1"
    assert storage.url("report.pdf") == (
        "https://assets.s3.cn-north-1.amazonaws.com.cn/media/report.pdf"
    )

    storage.region_name = "us-west-2"
    storage.endpoint_url = "https://objects.example.test/api"
    assert storage.url("report.pdf") == (
        "https://assets.objects.example.test/api/media/report.pdf"
    )

    storage.addressing_style = "path"
    assert storage.url("report.pdf") == (
        "https://objects.example.test/api/assets/media/report.pdf"
    )

    storage.custom_domain = "cdn.example.test"
    assert storage.url("report.pdf") == "https://cdn.example.test/media/report.pdf"
    assert storage.unsigned_connection_accesses == 0


if __name__ == "__main__":
    main()
