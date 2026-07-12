#!/usr/bin/env python3
"""Exercise queued idle-worker shutdown with deterministic scheduler nodes."""
from __future__ import annotations

import argparse
import importlib.util
import sys
from pathlib import Path
from types import ModuleType
from types import SimpleNamespace


class Config:
    option = SimpleNamespace(loadscopereorder=False)

    def getvalue(self, name: str) -> list[str]:
        assert name == "tx"
        return ["popen", "popen"]


class Node:
    """Model WorkerController's FIFO commands, not process termination."""

    def __init__(self, name: str) -> None:
        self.gateway = SimpleNamespace(id=name)
        self._down = False
        self._shutdown_sent = False
        self.sent: list[list[int]] = []
        self.commands: list[tuple[str, list[int]]] = []
        self.shutdown_calls = 0

    @property
    def shutting_down(self) -> bool:
        return self._down or self._shutdown_sent

    def send_runtest_some(self, indices: list[int]) -> None:
        self.sent.append(indices)
        self.commands.append(("runtests", indices))

    def shutdown(self) -> None:
        if not self._down:
            self.commands.append(("shutdown", []))
            self.shutdown_calls += 1
            self._shutdown_sent = True

    def queued_indices(self) -> list[int]:
        """Return runnable commands before the graceful shutdown marker."""
        indices: list[int] = []
        shutdown_seen = False
        for name, payload in self.commands:
            if name == "runtests":
                assert not shutdown_seen, "shutdown was queued before assigned work"
                indices.extend(payload)
            else:
                assert name == "shutdown" and payload == []
                shutdown_seen = True
        assert shutdown_seen, "worker has no graceful shutdown marker"
        return indices


def load_scheduler(repo: Path) -> type:
    pytest = ModuleType("pytest")
    pytest.Config = object
    sys.modules["pytest"] = pytest

    xdist = ModuleType("xdist")
    xdist.__path__ = [str(repo / "src" / "xdist")]
    sys.modules["xdist"] = xdist
    scheduler = ModuleType("xdist.scheduler")
    scheduler.__path__ = [str(repo / "src" / "xdist" / "scheduler")]
    sys.modules["xdist.scheduler"] = scheduler

    remote = ModuleType("xdist.remote")
    remote.WorkerController = object

    class Producer:
        def __init__(self, _name: str) -> None:
            pass

        def __getattr__(self, _name: str) -> Producer:
            return self

        def __call__(self, *_args: object) -> None:
            pass

    remote.Producer = Producer
    sys.modules["xdist.remote"] = remote
    report = ModuleType("xdist.report")
    report.report_collection_diff = lambda *_args: ""
    sys.modules["xdist.report"] = report
    workermanage = ModuleType("xdist.workermanage")
    workermanage.WorkerController = object
    workermanage.parse_tx_spec_config = lambda config: config.getvalue("tx")
    sys.modules["xdist.workermanage"] = workermanage

    for name in ("loadscope", "loadgroup"):
        spec = importlib.util.spec_from_file_location(
            f"xdist.scheduler.{name}", repo / "src" / "xdist" / "scheduler" / f"{name}.py"
        )
        assert spec and spec.loader
        module = importlib.util.module_from_spec(spec)
        sys.modules[spec.name] = module
        spec.loader.exec_module(module)
    return sys.modules["xdist.scheduler.loadgroup"].LoadGroupScheduling


def main(repo: Path) -> None:
    LoadGroupScheduling = load_scheduler(repo)

    scheduler = LoadGroupScheduling(Config())
    fast = Node("gw0")
    slow = Node("gw1")
    collection = ["test_demo.py::test_fast@fast", "test_demo.py::test_slow@slow"]
    for node in (fast, slow):
        scheduler.add_node(node)
        scheduler.add_node_collection(node, collection)
    scheduler.schedule()

    # WorkerController.shutdown queues a FIFO marker. It may be sent more than
    # once by the scheduler, but every marker follows the assigned runtests
    # command and therefore cannot terminate assigned work prematurely.
    assert fast.sent == [[0]] and slow.sent == [[1]]
    assert fast.shutdown_calls > 0 and slow.shutdown_calls > 0
    assert fast.queued_indices() == [0]
    assert slow.queued_indices() == [1]
    assert scheduler.tests_finished is False
    assert scheduler.has_pending

    # A queued marker lets the worker report its final test without asking the
    # scheduler for a post-completion shutdown command.
    fast_shutdown_calls = fast.shutdown_calls
    slow_shutdown_calls = slow.shutdown_calls
    scheduler.mark_test_complete(fast, 0)
    assert fast.shutdown_calls == fast_shutdown_calls
    assert slow.shutdown_calls == slow_shutdown_calls
    assert slow.queued_indices() == [1]
    assert scheduler.tests_finished is False
    assert scheduler.has_pending

    scheduler.mark_test_complete(slow, 1)
    assert slow.shutdown_calls == slow_shutdown_calls
    assert scheduler.tests_finished
    assert scheduler.has_pending is False


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("repo", type=Path)
    main(parser.parse_args().repo)
