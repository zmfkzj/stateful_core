#!/usr/bin/env python3
"""Evaluator for the narrow polling implementation of Watchdog #1071."""

from __future__ import annotations

import sys
import tempfile
from pathlib import Path


def main(checkout: str) -> None:
    sys.path.insert(0, str(Path(checkout) / "src"))

    from watchdog.events import FileClosedEvent, FileModifiedEvent
    from watchdog.observers.api import EventQueue, ObservedWatch
    from watchdog.observers.polling import PollingEmitter

    with tempfile.TemporaryDirectory() as temporary_directory:
        root = Path(temporary_directory)
        watched_file = root / "flushed.txt"
        watched_file.write_text("before")
        event_queue = EventQueue()
        watch = ObservedWatch(root, recursive=True, event_filter=[FileClosedEvent])
        emitter = PollingEmitter(event_queue, watch, timeout=0, event_filter=[FileClosedEvent])
        emitter.on_thread_start()

        with watched_file.open("w") as file:
            file.write("after flush")
            file.flush()

        emitter.queue_events(0)
        queued_events = []
        while not event_queue.empty():
            event, _watch, *_ = event_queue.get_nowait()
            queued_events.append(event)
        assert queued_events == [FileClosedEvent(str(watched_file))], (
            "a FileClosedEvent filter must opt into one deterministic flush event for a changed file"
        )
        emitter.stop()
        default_queue = EventQueue()
        default_emitter = PollingEmitter(
            default_queue, ObservedWatch(root, recursive=True), timeout=0
        )
        default_emitter.on_thread_start()
        watched_file.write_text("default polling event")
        default_emitter.queue_events(0)
        default_events = []
        while not default_queue.empty():
            event, _watch, *_ = default_queue.get_nowait()
            default_events.append(event)
        assert any(
            isinstance(event, FileModifiedEvent) and event.src_path == str(watched_file)
            for event in default_events
        ), "unfiltered polling must retain its ordinary modification event"
        assert not any(isinstance(event, FileClosedEvent) for event in default_events), (
            "unfiltered polling must not add an optional flush event"
        )
        default_emitter.stop()


if __name__ == "__main__":
    main(sys.argv[1])
