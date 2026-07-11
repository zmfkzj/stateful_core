#!/usr/bin/env python3
"""Evaluator for django-storages issue #1553."""
import argparse
import sys
from pathlib import Path


DEPENDENCIES = Path("/private/tmp/statefulbench-realworld-curation/django-storages-deps")


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
    sys.path[:0] = [str(args.repo), str(DEPENDENCIES)]

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

    assert storage.url(
        "report.pdf",
        parameters={"response-content-disposition": "attachment; filename=report.pdf"},
    ) == (
        "https://assets.s3.amazonaws.com/media/report.pdf?"
        "response-content-disposition=attachment%3B+filename%3Dreport.pdf"
    )
    assert storage.unsigned_connection_accesses == 0

    storage.custom_domain = "cdn.example.test"
    assert storage.url("report.pdf") == "https://cdn.example.test/media/report.pdf"
    assert storage.unsigned_connection_accesses == 0


if __name__ == "__main__":
    main()
