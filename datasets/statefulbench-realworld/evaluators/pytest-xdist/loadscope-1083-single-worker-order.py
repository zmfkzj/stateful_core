#!/usr/bin/env python3
"""Check that loadscope preserves collection order for a single worker."""
from __future__ import annotations

import importlib.util
from pathlib import Path
import sys
import types
from collections.abc import Callable


class Option:
    def __init__(self, reorder: bool) -> None:
        self.loadscopereorder = reorder


class Config:
    def __init__(self, nodes: int, reorder: bool) -> None:
        self.nodes = nodes
        self.option = Option(reorder)

    def getvalue(self, name: str) -> list[str]:
        assert name == "tx"
        return [f"{self.nodes}*popen"]


class Node:
    def __init__(self, collection: list[str]) -> None:
        self.collection = collection
        self.gateway = types.SimpleNamespace(id="worker")
        self.shutting_down = False
        self.sent: list[list[str]] = []
        self.shutdown_calls = 0

    def send_runtest_some(self, indexes: list[int]) -> None:
        self.sent.append([self.collection[index] for index in indexes])

    def shutdown(self) -> None:
        self.shutdown_calls += 1
        self.shutting_down = True


def load_scheduler(checkout: Path) -> Callable[..., object]:
    pytest_module = types.ModuleType("pytest")
    pytest_module.Config = object
    remote_module = types.ModuleType("xdist.remote")
    remote_module.Producer = lambda _name: lambda *_args: None
    report_module = types.ModuleType("xdist.report")
    report_module.report_collection_diff = lambda *_args: ""
    workermanage_module = types.ModuleType("xdist.workermanage")
    workermanage_module.parse_tx_spec_config = lambda config: ["popen"] * int(
        config.getvalue("tx")[0].split("*")[0]
    )
    workermanage_module.WorkerController = object
    sys.modules.update(
        {
            "pytest": pytest_module,
            "xdist": types.ModuleType("xdist"),
            "xdist.remote": remote_module,
            "xdist.report": report_module,
            "xdist.workermanage": workermanage_module,
        }
    )
    spec = importlib.util.spec_from_file_location(
        "loadscope_under_test", checkout / "src/xdist/scheduler/loadscope.py"
    )
    assert spec and spec.loader
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module.LoadScopeScheduling


def schedule(checkout: Path, nodes: int, reorder: bool, collection: list[str]) -> list[Node]:
    scheduler = load_scheduler(checkout)(Config(nodes, reorder))
    workers = [Node(collection) for _ in range(nodes)]
    for worker in workers:
        scheduler.add_node(worker)
        scheduler.add_node_collection(worker, collection)
    scheduler.schedule()
    return workers


def assigned(workers: list[Node]) -> list[str]:
    return [item for worker in workers for batch in worker.sent for item in batch]


def main() -> None:
    checkout = Path(sys.argv[1]).resolve()
    collection = [
        "test_django.py::TestCase::test_first",
        "test_transaction.py::TransactionalTestCase::test_one",
        "test_transaction.py::TransactionalTestCase::test_two",
    ]

    single = schedule(checkout, nodes=1, reorder=True, collection=collection)
    assert single[0].sent == [
        ["test_django.py::TestCase::test_first"],
        [
            "test_transaction.py::TransactionalTestCase::test_one",
            "test_transaction.py::TransactionalTestCase::test_two",
        ],
    ], single[0].sent

    parallel = schedule(checkout, nodes=2, reorder=True, collection=collection)
    assert parallel[0].sent[0] == collection[1:], parallel[0].sent
    assert parallel[1].sent[0] == collection[:1], parallel[1].sent

    opt_out = schedule(checkout, nodes=2, reorder=False, collection=collection)
    assert opt_out[0].sent[0] == collection[:1], opt_out[0].sent
    assert opt_out[1].sent[0] == collection[1:], opt_out[1].sent
    assert assigned(opt_out) == collection, assigned(opt_out)


if __name__ == "__main__":
    main()
