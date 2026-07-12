#!/usr/bin/env python3
"""Evaluator for watchdog issue #1039."""
import argparse
import sys
from pathlib import Path


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("repo", type=Path)
    args = parser.parse_args()
    sys.path.insert(0, str(args.repo / "src"))

    from watchdog.events import FileSystemEventHandler
    from watchdog.observers.api import BaseObserver, EventEmitter

    observer = BaseObserver(EventEmitter)
    empty_watch = observer.schedule(None, "handlerless")
    assert empty_watch in observer._watches
    assert empty_watch not in observer._handlers, "a handlerless watch registered a None handler"
    observer.unschedule(empty_watch)
    assert empty_watch not in observer._watches
    assert empty_watch not in observer._handlers
    assert not observer.emitters

    watch = observer.schedule(None, "later-handler")
    handler = FileSystemEventHandler()
    observer.add_handler_for_watch(handler, watch)
    assert observer._handlers[watch] == {handler}
    observer.unschedule(watch)
    assert watch not in observer._watches
    assert watch not in observer._handlers
    assert not observer.emitters


if __name__ == "__main__":
    main()
