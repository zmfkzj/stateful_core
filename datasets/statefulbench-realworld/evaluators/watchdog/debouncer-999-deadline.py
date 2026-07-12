#!/usr/bin/env python3
"""Focused evaluator for watchdog issue #999's repeated debounce windows."""

from __future__ import annotations

import sys
from pathlib import Path
from types import SimpleNamespace


class Clock:
    def __init__(self) -> None:
        self.value = 0.0

    def monotonic(self) -> float:
        return self.value

    def advance(self, seconds: float) -> None:
        self.value += seconds


class ScheduledCondition:
    def __init__(self, debouncer: object, clock: Clock, later_events: list[object]) -> None:
        self.debouncer = debouncer
        self.clock = clock
        self.later_events = later_events
        self.timeouts: list[float | None] = []
        self.calls = 0

    def __enter__(self) -> ScheduledCondition:
        return self

    def __exit__(self, *_: object) -> None:
        return None

    def notify(self) -> None:
        return None

    def wait(self, timeout: float | None = None) -> bool:
        self.timeouts.append(timeout)
        call = self.calls
        self.calls += 1
        if call == 0:  # First queued event is already present at t=0.
            return True
        if call == 1:  # First debounce window completes at t=10.
            assert timeout == 10
            self.clock.advance(timeout)
            return False
        if call == 2:  # A new batch begins at t=10.
            self.debouncer._events.append(self.later_events[0])
            return True
        if call == 3:  # Another event arrives four seconds into that window.
            assert timeout == 10
            self.debouncer._events.append(self.later_events[1])
            self.clock.advance(4)
            return True
        if call == 4:  # The remaining time must be six seconds, not a new ten seconds.
            self.clock.advance(timeout)
            return False
        if call == 5:
            self.debouncer.stop()
            return True
        raise AssertionError(f"unexpected condition wait {call}")


def main(checkout: str) -> None:
    sys.path.insert(0, str(Path(checkout) / "src"))
    from watchdog.utils import event_debouncer as module

    clock = Clock()
    module.time = SimpleNamespace(monotonic=clock.monotonic)
    batches: list[tuple[float, list[object]]] = []
    debouncer = module.EventDebouncer(10, lambda events: batches.append((clock.monotonic(), events)))
    first, second, third = object(), object(), object()
    debouncer._events.append(first)
    condition = ScheduledCondition(debouncer, clock, [second, third])
    debouncer._cond = condition

    debouncer.run()

    if debouncer.is_alive():
        raise AssertionError("synchronous debounce run leaked a thread")
    if batches != [(10.0, [first]), (20.0, [second, third])]:
        raise AssertionError(f"debounce windows must retain their original deadlines, got {batches!r}")
    if condition.timeouts != [None, 10, None, 10, 6, None]:
        raise AssertionError(f"expected shrinking repeated-window delays, got {condition.timeouts!r}")


if __name__ == "__main__":
    if len(sys.argv) != 2:
        raise SystemExit("usage: debouncer-999-deadline.py <watchdog-checkout>")
    main(sys.argv[1])
