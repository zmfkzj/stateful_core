#!/usr/bin/env python3
"""Evaluator for collectstatic metadata reuse from django-storages issue #1255."""

import argparse
import datetime
import threading
import sys
from pathlib import Path
from types import SimpleNamespace


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("repo", type=Path)
    repo = parser.parse_args().repo.resolve()
    sys.path.insert(0, str(repo))
    from django.conf import settings

    if not settings.configured:
        settings.configure(USE_TZ=True)

    from django.core.files.base import ContentFile
    from storages.backends.s3 import S3Storage

    modified = datetime.datetime(2026, 1, 2, tzinfo=datetime.timezone.utc)

    class Client:
        def __init__(self, *, block=False):
            self.calls = []
            self.block = block
            self.head_started = threading.Event()
            self.release_head = threading.Event()

        def head_object(self, **kwargs):
            self.calls.append(kwargs)
            if self.block:
                self.head_started.set()
                assert self.release_head.wait(5), "HEAD was not released"
            return {"ETag": '"cached"', "LastModified": modified}

    class Object:
        def __init__(self, bucket, key):
            self.bucket = bucket
            self.key = key
            self.last_modified = modified

        def delete(self):
            if self.bucket.block_delete:
                self.bucket.delete_started.set()
                assert self.bucket.release_delete.wait(5), "delete was not released"
            self.bucket.deleted.append(self.key)
            self.bucket.deleted_event.set()

        def upload_fileobj(self, content, **kwargs):
            self.bucket.uploaded.append((self.key, content.read(), kwargs))

    class Bucket:
        def __init__(self):
            self.object_calls = []
            self.deleted = []
            self.deleted_event = threading.Event()
            self.block_delete = False
            self.delete_started = threading.Event()
            self.release_delete = threading.Event()
            self.uploaded = []

        def Object(self, key):
            self.object_calls.append(key)
            return Object(self, key)

    def storage_for(client):
        bucket = Bucket()
        storage = S3Storage(bucket_name="unit-bucket", location="media")
        storage._connections.connection = SimpleNamespace(
            meta=SimpleNamespace(client=client)
        )
        storage._connections.bucket = bucket
        storage._bucket = bucket
        return storage, bucket

    client = Client()
    storage, bucket = storage_for(client)

    # Repeated collectstatic existence checks reuse metadata from the first HEAD.
    assert storage.exists("assets/app.css") is True
    assert storage.exists("assets/app.css") is True
    assert client.calls == [
        {"Bucket": "unit-bucket", "Key": "media/assets/app.css"}
    ], client.calls

    # The adjacent modified-time lookup must use that same HEAD response.
    assert storage.get_modified_time("assets/app.css") == modified
    assert bucket.object_calls == [], bucket.object_calls

    # Both mutations drop cached metadata before touching the object.
    storage._save("assets/app.css", ContentFile(b"replacement"))
    assert bucket.uploaded[0][1] == b"replacement", bucket.uploaded
    assert storage.exists("assets/app.css") is True
    assert len(client.calls) == 2, client.calls
    storage.delete("assets/app.css")
    assert bucket.deleted == ["media/assets/app.css"], bucket.deleted
    assert storage.exists("assets/app.css") is True
    assert len(client.calls) == 3, client.calls

    # Request-local regions with the same normalized key cannot share metadata.
    regional_client = Client()
    regional, _ = storage_for(regional_client)
    region = ["us-east-1"]
    regional.get_region_name = lambda: region[0]
    regional._connections.region_name = region[0]
    assert regional.exists("assets/app.css") is True
    region[0] = "eu-west-1"
    regional._connections.connection = SimpleNamespace(
        meta=SimpleNamespace(client=regional_client)
    )
    regional._connections.region_name = region[0]
    assert regional.exists("assets/app.css") is True
    assert len(regional_client.calls) == 2, regional_client.calls

    # Cache and synchronization primitives never survive serialized storage state.
    state = storage.__getstate__()
    assert "_object_metadata" not in state, state
    assert "_object_metadata_generations" not in state, state
    assert "_object_metadata_lock" not in state, state
    restored = S3Storage.__new__(S3Storage)
    restored.__setstate__(state)
    assert restored._object_metadata == {}, restored._object_metadata

    # A HEAD started before an invalidation must not publish stale metadata later.
    racing_client = Client(block=True)
    racing, racing_bucket = storage_for(racing_client)
    exists_result = []
    def read_racing_metadata():
        racing._connections.connection = SimpleNamespace(
            meta=SimpleNamespace(client=racing_client)
        )
        racing._connections.bucket = racing_bucket
        exists_result.append(racing.exists("assets/app.css"))

    def delete_racing_metadata():
        racing._connections.connection = SimpleNamespace(
            meta=SimpleNamespace(client=racing_client)
        )
        racing._connections.bucket = racing_bucket
        racing.delete("assets/app.css")

    reader = threading.Thread(target=read_racing_metadata)
    reader.start()
    assert racing_client.head_started.wait(5), "HEAD did not start"
    deleter = threading.Thread(target=delete_racing_metadata)
    deleter.start()
    assert racing_bucket.deleted_event.wait(5), "delete did not finish"
    racing_client.release_head.set()
    reader.join(5)
    deleter.join(5)
    assert not reader.is_alive(), "HEAD thread did not finish"
    assert not deleter.is_alive(), "delete thread did not finish"
    assert exists_result == [True], exists_result
    assert racing.exists("assets/app.css") is True
    assert len(racing_client.calls) == 2, racing_client.calls
    # A HEAD during an in-flight delete must be invalidated again on completion.
    late_client = Client()
    late, late_bucket = storage_for(late_client)
    late_bucket.block_delete = True
    def delete_late_metadata():
        late._connections.connection = SimpleNamespace(
            meta=SimpleNamespace(client=late_client)
        )
        late._connections.bucket = late_bucket
        late.delete("assets/app.css")

    late_deleter = threading.Thread(target=delete_late_metadata)
    late_deleter.start()
    assert late_bucket.delete_started.wait(5), "delete did not start"
    assert late.exists("assets/app.css") is True
    assert len(late_client.calls) == 1, late_client.calls
    late_bucket.release_delete.set()
    assert late_bucket.deleted_event.wait(5), "delete did not finish"
    late_deleter.join(5)
    assert not late_deleter.is_alive(), "delete thread did not finish"
    assert late.exists("assets/app.css") is True
    assert len(late_client.calls) == 2, late_client.calls



if __name__ == "__main__":
    main()
