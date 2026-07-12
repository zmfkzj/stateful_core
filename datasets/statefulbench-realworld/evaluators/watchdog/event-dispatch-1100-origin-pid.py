#!/usr/bin/env python3
"""Evaluator for Watchdog #1100 optional event-origin process IDs."""

from __future__ import annotations

import sys
from pathlib import Path


def main(checkout: str) -> None:
    sys.path.insert(0, str(Path(checkout) / "src"))

    from watchdog.events import FileCreatedEvent, FileSystemEventHandler
    from watchdog.observers.api import BaseObserver, EventEmitter, ObservedWatch

    observer = BaseObserver(object)
    watch = ObservedWatch("/watched", recursive=False)
    emitter = EventEmitter(observer.event_queue, watch)
    received = []

    class RecordingHandler(FileSystemEventHandler):
        def on_any_event(self, event) -> None:
            received.append(event)

    observer.add_handler_for_watch(RecordingHandler(), watch)
    event = FileCreatedEvent("/watched/from-process")
    emitter.queue_event(event, origin_pid=4242)
    observer.dispatch_events(observer.event_queue)

    assert received == [event], "dispatch must preserve event delivery when origin metadata is supplied"
    assert event.origin_pid == 4242, "handlers must receive the emitter-supplied origin PID"

    event_without_origin = FileCreatedEvent("/watched/unknown-origin")
    emitter.queue_event(event_without_origin)
    observer.dispatch_events(observer.event_queue)
    assert event_without_origin.origin_pid is None, "events without an origin PID must expose None"

    legacy_event = FileCreatedEvent("/watched/legacy-entry")
    observer.event_queue.put((legacy_event, watch))
    observer.dispatch_events(observer.event_queue)
    assert received[-1] is legacy_event, "dispatch must keep accepting existing two-item queue entries"
    assert legacy_event.origin_pid is None, "legacy queue entries must expose an unknown origin"
    assert observer.event_queue.unfinished_tasks == 0, "dispatch must acknowledge every queued event"


if __name__ == "__main__":
    main(sys.argv[1])
