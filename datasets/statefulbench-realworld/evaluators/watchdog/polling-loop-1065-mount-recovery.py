#!/usr/bin/env python3
"""Evaluator for Watchdog #1065 PollingEmitter mount recovery."""

from __future__ import annotations

import sys
import tempfile
from pathlib import Path


def main(checkout: str) -> None:
    sys.path.insert(0, str(Path(checkout) / "src"))

    from watchdog.events import FileCreatedEvent
    from watchdog.observers.api import EventQueue, ObservedWatch
    from watchdog.observers.polling import PollingEmitter

    with tempfile.TemporaryDirectory() as temporary_directory:
        root = Path(temporary_directory)
        (root / "before.txt").write_text("before")
        event_queue = EventQueue()
        emitter = PollingEmitter(event_queue, ObservedWatch(root, recursive=True), timeout=0)
        emitter.on_thread_start()
        take_snapshot = emitter._take_snapshot
        unavailable = True

        def intermittently_unavailable():
            nonlocal unavailable
            if unavailable:
                unavailable = False
                raise OSError("SMB mount temporarily unavailable")
            return take_snapshot()

        emitter._take_snapshot = intermittently_unavailable
        emitter.queue_events(0)
        assert emitter.should_keep_running(), "temporary snapshot failure stopped the polling emitter"
        assert event_queue.empty(), "temporary snapshot failure emitted a deletion event"

        recovered = root / "recovered.txt"
        recovered.write_text("recovered")
        emitter.queue_events(0)
        queued_events = []
        while not event_queue.empty():
            event, _watch, *_ = event_queue.get_nowait()
            queued_events.append(event)
        assert any(
            isinstance(event, FileCreatedEvent) and event.src_path == str(recovered) for event in queued_events
        ), "the emitter did not resume polling after the mount recovered"
        def fatal_snapshot():
            raise RuntimeError("unexpected snapshot failure")

        emitter._take_snapshot = fatal_snapshot
        try:
            emitter.queue_events(0)
        except RuntimeError:
            pass
        else:
            raise AssertionError("non-OSError snapshot failures must still propagate")

        emitter.stop()
        assert not emitter.should_keep_running(), "explicit stop must still stop the polling emitter"
        emitter.queue_events(0)


if __name__ == "__main__":
    main(sys.argv[1])
