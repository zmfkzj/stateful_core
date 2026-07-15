#!/usr/bin/env python3
"""Evaluator for watchdog issue #1044."""
import argparse
import sys
import threading
from pathlib import Path


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("repo", type=Path)
    args = parser.parse_args()
    sys.path.insert(0, str(args.repo / "src"))

    from watchdog.events import FileSystemEventHandler
    from watchdog.observers.api import BaseObserver, EventEmitter

    class BlockingEmitter(EventEmitter):
        first_start_entered = threading.Event()
        release_first_start = threading.Event()

        def start(self) -> None:
            if self.watch.path == "first":
                type(self).first_start_entered.set()
                assert type(self).release_first_start.wait(2), "test setup timed out waiting to release first emitter"
            super().start()

    observer = BaseObserver(BlockingEmitter)
    observer.schedule(FileSystemEventHandler(), "first")
    start_error: list[BaseException] = []
    schedule_error: list[BaseException] = []
    schedule_finished = threading.Event()

    def start_observer() -> None:
        try:
            observer.start()
        except BaseException as error:
            start_error.append(error)

    def schedule_second_watch() -> None:
        try:
            observer.schedule(None, "second")
        except BaseException as error:
            schedule_error.append(error)
        finally:
            schedule_finished.set()

    start_thread = threading.Thread(target=start_observer)
    schedule_thread = threading.Thread(target=schedule_second_watch)
    try:
        start_thread.start()
        assert BlockingEmitter.first_start_entered.wait(2), "observer startup did not enter the controlled emitter"
        schedule_thread.start()
        assert not schedule_finished.wait(0.2), "schedule raced ahead of an in-progress observer startup"

        BlockingEmitter.release_first_start.set()
        start_thread.join(2)
        schedule_thread.join(2)
        assert not start_thread.is_alive(), "observer start thread leaked"
        assert not schedule_thread.is_alive(), "schedule thread leaked"
        assert not start_error, start_error
        assert not schedule_error, schedule_error

        second_emitter = observer._emitter_for_watch[next(watch for watch in observer._watches if watch.path == "second")]
        assert second_emitter.is_alive(), "watch scheduled during startup was not started"
    finally:
        BlockingEmitter.release_first_start.set()
        start_thread.join(2)
        schedule_thread.join(2)
        observer.stop()
        observer.join(2)
        assert not start_thread.is_alive(), "observer start thread leaked during cleanup"
        assert not schedule_thread.is_alive(), "schedule thread leaked during cleanup"
        assert not observer.is_alive(), "observer thread leaked during cleanup"
        assert not observer.emitters, "emitter leaked during cleanup"


if __name__ == "__main__":
    main()
