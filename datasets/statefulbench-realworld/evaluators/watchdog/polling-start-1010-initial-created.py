#!/usr/bin/env python3
"""Evaluator for the #1010 PollingEmitter startup extension."""
import argparse
import queue
import sys
from pathlib import Path


class Snapshot:
    pass


class InitialDiff:
    def __init__(self, before, after):
        from watchdog.utils.dirsnapshot import EmptyDirectorySnapshot

        assert isinstance(before, EmptyDirectorySnapshot)
        assert isinstance(after, Snapshot)
        self.files_deleted = []
        self.files_modified = []
        self.files_created = ["/watch/ready.txt"]
        self.files_moved = []
        self.dirs_deleted = []
        self.dirs_modified = []
        self.dirs_created = ["/watch/incoming"]
        self.dirs_moved = []


def queued_types(event_queue):
    queued = []
    while not event_queue.empty():
        event, _watch, *_ = event_queue.get_nowait()
        queued.append((type(event), event.src_path))
    return queued


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("repo", type=Path)
    repo = parser.parse_args().repo.resolve()
    sys.path.insert(0, str(repo / "src"))

    import watchdog.observers.polling as polling
    from watchdog.events import DirCreatedEvent, FileCreatedEvent
    from watchdog.observers.api import ObservedWatch

    original_snapshot = polling.DirectorySnapshot
    original_diff = polling.DirectorySnapshotDiff
    polling.DirectorySnapshot = lambda *args, **kwargs: Snapshot()
    polling.DirectorySnapshotDiff = InitialDiff
    try:
        watch = ObservedWatch("/watch", recursive=True)
        quiet_queue = queue.Queue()
        quiet_emitter = polling.PollingEmitter(quiet_queue, watch)
        quiet_emitter.on_thread_start()
        assert queued_types(quiet_queue) == []

        event_queue = queue.Queue()
        emitter = polling.PollingEmitter(event_queue, watch, emit_on_start=True)
        emitter.on_thread_start()

        filtered_queue = queue.Queue()
        filtered_emitter = polling.PollingEmitter(
            filtered_queue,
            watch,
            emit_on_start=True,
            event_filter=[FileCreatedEvent],
        )
        filtered_emitter.on_thread_start()
    finally:
        polling.DirectorySnapshot = original_snapshot
        polling.DirectorySnapshotDiff = original_diff

    assert queued_types(event_queue) == [
        (FileCreatedEvent, "/watch/ready.txt"),
        (DirCreatedEvent, "/watch/incoming"),
    ]
    assert queued_types(filtered_queue) == [(FileCreatedEvent, "/watch/ready.txt")]


if __name__ == "__main__":
    main()
