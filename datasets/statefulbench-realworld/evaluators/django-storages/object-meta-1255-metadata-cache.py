#!/usr/bin/env python3
"""Evaluator for collectstatic metadata reuse from django-storages issue #1255."""

import argparse
import datetime
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
        settings.configure(USE_TZ=True)

    from storages.backends.s3 import S3Storage

    modified = datetime.datetime(2026, 1, 2, tzinfo=datetime.timezone.utc)

    class Client:
        def __init__(self):
            self.calls = []

        def head_object(self, **kwargs):
            self.calls.append(kwargs)
            return {"ETag": '"cached"', "LastModified": modified}

    class Bucket:
        def __init__(self):
            self.object_calls = []
            self.deleted = []

        def Object(self, key):
            self.object_calls.append(key)
            return SimpleNamespace(
                last_modified=modified, delete=lambda: self.deleted.append(key)
            )

    client = Client()
    bucket = Bucket()
    storage = S3Storage(bucket_name="unit-bucket", location="media")
    storage._connections.connection = SimpleNamespace(meta=SimpleNamespace(client=client))
    storage._bucket = bucket

    # Repeated collectstatic existence checks reuse metadata from the first HEAD.
    assert storage.exists("assets/app.css") is True
    assert storage.exists("assets/app.css") is True
    assert client.calls == [
        {"Bucket": "unit-bucket", "Key": "media/assets/app.css"}
    ], client.calls

    # The adjacent modified-time lookup must use that same HEAD response.
    assert storage.get_modified_time("assets/app.css") == modified
    assert bucket.object_calls == [], bucket.object_calls
    # Deletion drops cached metadata so a recreated key is checked again.
    storage.delete("assets/app.css")
    assert bucket.deleted == ["media/assets/app.css"], bucket.deleted
    assert storage.exists("assets/app.css") is True
    assert len(client.calls) == 2, client.calls



if __name__ == "__main__":
    main()
