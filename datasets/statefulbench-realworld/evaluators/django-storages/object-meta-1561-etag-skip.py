#!/usr/bin/env python3
"""Evaluator for ETag-based incremental collectstatic from issue #1561."""

import argparse
import hashlib
import sys
from pathlib import Path
from types import SimpleNamespace


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("repo", type=Path)
    repo = parser.parse_args().repo
    sys.path.insert(0, str(repo))

    from django.conf import settings

    if not settings.configured:
        settings.configure()

    from django.core.files.base import ContentFile
    from storages.backends.s3 import S3Storage

    class Client:
        def __init__(self, etag):
            self.etag = etag
            self.calls = []

        def head_object(self, **kwargs):
            self.calls.append(kwargs)
            return {"ETag": self.etag}

    def storage_for(client, *, skip_unchanged):
        uploads = []

        class Object:
            def upload_fileobj(self, content, **kwargs):
                uploads.append((content.read(), kwargs))

        storage = S3Storage(bucket_name="unit-bucket", location="media")
        storage.skip_unchanged = skip_unchanged
        storage._connections.connection = SimpleNamespace(
            meta=SimpleNamespace(client=client)
        )
        storage._bucket = SimpleNamespace(Object=lambda key: Object())
        return storage, uploads

    payload = b"stable asset"
    digest = hashlib.md5(payload).hexdigest()

    # exists() saves the ETag-bearing metadata that _save() uses without a second HEAD.
    matching_client = Client(f'"{digest}"')
    matching, matching_uploads = storage_for(matching_client, skip_unchanged=True)
    assert matching.exists("assets/app.js") is True
    matching_content = ContentFile(payload)
    assert matching._save("assets/app.js", matching_content) == "assets/app.js"
    assert matching_client.calls == [
        {"Bucket": "unit-bucket", "Key": "media/assets/app.js"}
    ], matching_client.calls
    assert matching_uploads == [], matching_uploads
    assert matching_content.tell() == 0, matching_content.tell()

    # A different cached ETag transfers bytes and invalidates stale metadata.
    changed_client = Client('"different"')
    changed, changed_uploads = storage_for(changed_client, skip_unchanged=True)
    assert changed.exists("assets/app.js") is True
    assert changed._save("assets/app.js", ContentFile(payload)) == "assets/app.js"
    assert changed_uploads[0][0] == payload, changed_uploads
    assert changed.exists("assets/app.js") is True
    assert len(changed_client.calls) == 2, changed_client.calls

    # The option remains opt-in after exists() has populated metadata.
    disabled_client = Client(f'"{digest}"')
    disabled, disabled_uploads = storage_for(disabled_client, skip_unchanged=False)
    assert disabled.exists("assets/app.js") is True
    disabled._save("assets/app.js", ContentFile(payload))
    assert disabled_uploads[0][0] == payload, disabled_uploads
    assert len(disabled_client.calls) == 1, disabled_client.calls


if __name__ == "__main__":
    main()
