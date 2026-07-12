#!/usr/bin/env python3
"""Evaluator for watchdog issue #1110 in PollingEmitter."""
import argparse
import os
import queue
import sys
from pathlib import Path


class Snapshot:
    def __init__(self, entries):
        self._entries = entries

    @property
    def paths(self):
        return set(self._entries)

    def path(self, inode):
        return next((path for path, value in self._entries.items() if value[0] == inode), None)

    def inode(self, path):
        return self._entries[path][0]

    def isdir(self, path):
        return self._entries[path][1]

    def mtime(self, path):
        return 0

    def size(self, path):
        return 0


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("repo", type=Path)
    repo = parser.parse_args().repo.resolve()
    sys.path.insert(0, str(repo / "src"))

    import watchdog.observers.polling as polling
    from watchdog.events import FileCreatedEvent
    from watchdog.observers.api import ObservedWatch

    snapshots = iter(
        (
            Snapshot({"/watch/target": ((1, 1), False)}),
            Snapshot({"/watch/target": ((1, 1), False), "/watch/target.link": ((2, 1), False)}),
        )
    )
    captured_stats = []

    def fake_directory_snapshot(path, *, recursive, stat, listdir):
        captured_stats.append(stat)
        return next(snapshots)

    original_snapshot = polling.DirectorySnapshot
    polling.DirectorySnapshot = fake_directory_snapshot
    try:
        event_queue = queue.Queue()
        emitter = polling.PollingEmitter(event_queue, ObservedWatch("/watch", recursive=True))
        emitter.on_thread_start()
        emitter.queue_events(0)
    finally:
        polling.DirectorySnapshot = original_snapshot

    assert captured_stats == [os.lstat, os.lstat], captured_stats
    queued_event, queued_watch, *_ = event_queue.get_nowait()
    assert type(queued_event) is FileCreatedEvent
    assert queued_event.src_path == "/watch/target.link"
    assert queued_watch.path == "/watch"
    assert event_queue.empty()


if __name__ == "__main__":
    main()
