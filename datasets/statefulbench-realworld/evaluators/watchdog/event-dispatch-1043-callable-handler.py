#!/usr/bin/env python3
"""Evaluator for Watchdog #1043 callable event handlers."""

from __future__ import annotations

import sys
from pathlib import Path


def main(checkout: str) -> None:
    sys.path.insert(0, str(Path(checkout) / "src"))

    from watchdog.events import FileCreatedEvent, FileSystemEventHandler
    from watchdog.observers.api import BaseObserver, ObservedWatch

    observer = BaseObserver(object)
    watch = ObservedWatch("/watched", recursive=False)
    received = []
    event = FileCreatedEvent("/watched/new-file")

    observer.add_handler_for_watch(received.append, watch)
    observer.event_queue.put((event, watch))
    observer.dispatch_events(observer.event_queue)

    assert received == [event], "a scheduled callable must receive the dispatched event exactly once"

    class RecordingHandler(FileSystemEventHandler):
        def __init__(self) -> None:
            self.received = []

        def on_any_event(self, dispatched_event) -> None:
            self.received.append(dispatched_event)

    object_handler = RecordingHandler()
    observer.add_handler_for_watch(object_handler, watch)
    second_event = FileCreatedEvent("/watched/second-file")
    observer.event_queue.put((second_event, watch))
    observer.dispatch_events(observer.event_queue)
    assert object_handler.received == [second_event], "FileSystemEventHandler instances must keep dispatch() semantics"

    class CallableHandler(RecordingHandler):
        def __init__(self) -> None:
            super().__init__()
            self.called = False

        def __call__(self, dispatched_event) -> None:
            self.called = True

    callable_object_observer = BaseObserver(object)
    callable_object_watch = ObservedWatch("/callable-object", recursive=False)
    callable_object_handler = CallableHandler()
    callable_object_event = FileCreatedEvent("/callable-object/new-file")
    callable_object_observer.add_handler_for_watch(callable_object_handler, callable_object_watch)
    callable_object_observer.event_queue.put((callable_object_event, callable_object_watch))
    callable_object_observer.dispatch_events(callable_object_observer.event_queue)
    assert callable_object_handler.received == [callable_object_event], "callable handler objects must retain dispatch() semantics"
    assert not callable_object_handler.called, "only function handlers may bypass dispatch()"
    assert callable_object_observer.event_queue.unfinished_tasks == 0, "dispatch must acknowledge every queued event"
    assert observer.event_queue.unfinished_tasks == 0, "dispatch must always acknowledge the queued event"


if __name__ == "__main__":
    main(sys.argv[1])
