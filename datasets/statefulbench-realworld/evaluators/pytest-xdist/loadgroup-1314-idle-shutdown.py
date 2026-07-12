#!/usr/bin/env python3
"""Exercise early idle-worker shutdown with deterministic scheduler nodes."""
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
    def __init__(self, name: str) -> None:
        self.gateway = SimpleNamespace(id=name)
        self.shutting_down = False
        self.sent: list[list[int]] = []
        self.shutdown_calls = 0

    def send_runtest_some(self, indices: list[int]) -> None:
        self.sent.append(indices)

    def shutdown(self) -> None:
        self.shutting_down = True
        self.shutdown_calls += 1


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

    assert fast.sent == [[0]] and slow.sent == [[1]]
    # WorkerController.shutdown queues a graceful remote shutdown marker; it
    # does not abort the work that was already sent to each worker.
    assert fast.shutdown_calls == slow.shutdown_calls == 1

    scheduler.mark_test_complete(fast, 0)
    assert fast.shutdown_calls == slow.shutdown_calls == 1
    assert slow.sent == [[1]]
    assert scheduler.has_pending

    scheduler.mark_test_complete(slow, 1)
    assert slow.shutdown_calls == 1
    assert scheduler.tests_finished
    assert scheduler.has_pending is False


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("repo", type=Path)
    main(parser.parse_args().repo)
