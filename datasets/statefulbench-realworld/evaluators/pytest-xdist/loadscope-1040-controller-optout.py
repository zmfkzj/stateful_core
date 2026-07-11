#!/usr/bin/env python3
"""Check benchmark-defined controller execution for loadscope opt-outs."""
from __future__ import annotations

import importlib.util
from collections.abc import Callable
from pathlib import Path
import sys
import types


class Option:
    def __init__(self, runner: Callable[[str], bool] | None) -> None:
        self.loadscopereorder = False
        if runner is not None:
            self.loadscopecontroller = runner


class Config:
    def __init__(self, runner: Callable[[str], bool] | None) -> None:
        self.option = Option(runner)

    def getvalue(self, name: str) -> list[str]:
        assert name == "tx"
        return ["1*popen"]


class Node:
    def __init__(self, collection: list[str]) -> None:
        self.collection = collection
        self.shutting_down = False
        self.sent: list[list[str]] = []

    def send_runtest_some(self, indexes: list[int]) -> None:
        self.sent.append([self.collection[index] for index in indexes])

    def shutdown(self) -> None:
        self.shutting_down = True


def load_scheduler(checkout: Path) -> Callable[..., object]:
    pytest_module = types.ModuleType("pytest")
    pytest_module.Config = object
    remote_module = types.ModuleType("xdist.remote")
    remote_module.Producer = lambda _name: lambda *_args: None
    report_module = types.ModuleType("xdist.report")
    report_module.report_collection_diff = lambda *_args: ""
    workermanage_module = types.ModuleType("xdist.workermanage")
    workermanage_module.parse_tx_spec_config = lambda config: config.getvalue("tx")
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


def schedule(
    checkout: Path, collection: list[str], runner: Callable[[str], bool] | None
) -> Node:
    scheduler = load_scheduler(checkout)(Config(runner))
    worker = Node(collection)
    scheduler.add_node(worker)
    scheduler.add_node_collection(worker, collection)
    scheduler.schedule()
    return worker


def main() -> None:
    checkout = Path(sys.argv[1]).resolve()
    collection = [
        "test_serial.py::test_exclusive",
        "test_parallel.py::test_worker_one",
        "test_parallel.py::test_worker_two",
    ]
    controller_runs: list[str] = []

    def controller_runner(nodeid: str) -> bool:
        if nodeid == "test_serial.py::test_exclusive":
            controller_runs.append(nodeid)
            return True
        return False

    worker = schedule(checkout, collection, controller_runner)
    assert controller_runs == ["test_serial.py::test_exclusive"], controller_runs
    assert worker.sent == [[collection[1], collection[2]]], worker.sent

    without_extension = schedule(checkout, collection, None)
    assert without_extension.sent == [[collection[0]], collection[1:]], without_extension.sent


if __name__ == "__main__":
    main()
