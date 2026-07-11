#!/usr/bin/env python3
"""Evaluator for django-storages issue #1558's S3Storage.url path entry."""
import argparse
import sys
from pathlib import Path


DEPENDENCIES = Path("/private/tmp/statefulbench-realworld-curation/django-storages-deps")


def storage(S3Storage, location):
    instance = object.__new__(S3Storage)
    instance.location = location
    instance.custom_domain = "cdn.example.test"
    instance.url_protocol = "https:"
    instance.querystring_auth = False
    instance.cloudfront_signer = None
    instance.querystring_expire = 3600
    return instance


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("repo", type=Path)
    args = parser.parse_args()
    sys.path[:0] = [str(args.repo), str(DEPENDENCIES)]

    from django.core.exceptions import SuspiciousOperation
    from storages.backends.s3 import S3Storage

    root = storage(S3Storage, "")
    assert root.url("album/photo.jpg") == "https://cdn.example.test/album/photo.jpg"
    try:
        root.url("../../secret.txt")
    except SuspiciousOperation:
        pass
    else:
        raise AssertionError("empty location allowed a URL path to escape its root")

    prefixed = storage(S3Storage, "media")
    assert prefixed.url("album/photo.jpg") == "https://cdn.example.test/media/album/photo.jpg"


if __name__ == "__main__":
    main()
