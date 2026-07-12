#!/usr/bin/env python3
"""Focused evaluator for watchdog issue #1000."""

from __future__ import annotations

import sys
import threading
from pathlib import Path


class SignalingCondition(threading.Condition):
    def __init__(self) -> None:
        super().__init__()
        self.waiting = threading.Event()

    def wait(self, timeout: float | None = None) -> bool:
        self.waiting.set()
        return super().wait(timeout)


def main(checkout: str) -> None:
    sys.path.insert(0, str(Path(checkout) / "src"))
    from watchdog.events import FileModifiedEvent
    from watchdog.utils.event_debouncer import EventDebouncer

    received: list[list[FileModifiedEvent]] = []
    debouncer = EventDebouncer(60, received.append)
    condition = SignalingCondition()
    debouncer._cond = condition
    event = FileModifiedEvent("queued-event")

    debouncer.start()
    try:
        if not condition.waiting.wait(1):
            raise AssertionError("EventDebouncer did not enter its condition wait")
        debouncer.handle_event(event)
        debouncer.stop()
        debouncer.join(1)
        if debouncer.is_alive():
            raise AssertionError("EventDebouncer leaked after stop")
        if received != [[event]]:
            raise AssertionError(f"stop must flush the queued event, got {received!r}")
    finally:
        if debouncer.is_alive():
            debouncer.stop()
            debouncer.join(1)


if __name__ == "__main__":
    if len(sys.argv) != 2:
        raise SystemExit("usage: debouncer-1000-stop-flush.py <watchdog-checkout>")
    main(sys.argv[1])
