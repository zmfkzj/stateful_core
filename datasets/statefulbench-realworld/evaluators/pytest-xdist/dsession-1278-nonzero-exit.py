#!/usr/bin/env python3
"""Evaluator for pytest-xdist issue #1278."""
import argparse
import sys
from pathlib import Path
from types import ModuleType
from types import SimpleNamespace


def load_dsession(repo: Path):
    sys.path.insert(0, str(repo / "src"))
    for name in tuple(sys.modules):
        if name == "xdist" or name.startswith("xdist."):
            del sys.modules[name]
    version = ModuleType("xdist._version")
    version.version = "0"  # type: ignore[attr-defined]
    sys.modules["xdist._version"] = version
    from xdist.dsession import DSession

    return DSession


class Hooks:
    def __init__(self) -> None:
        self.down: list[tuple[object, object | None]] = []

    def pytest_testnodedown(self, *, node: object, error: object | None) -> None:
        self.down.append((node, error))


class Config:
    def __init__(self, maxworkerrestart: str = "0") -> None:
        self.option = SimpleNamespace(
            debug=False,
            verbose=-1,
            maxworkerrestart=maxworkerrestart,
            numprocesses=1,
        )
        self.hook = Hooks()
        self.pluginmanager = SimpleNamespace(getplugin=lambda name: None)

    def getvalue(self, name: str) -> int:
        assert name == "maxfail"
        return 0


class Node:
    def __init__(self, exitstatus: int) -> None:
        self.gateway = SimpleNamespace(id="gw0", spec=None)
        self.workeroutput = {
            "exitstatus": exitstatus,
            "shouldfail": False,
            "shouldstop": False,
        }


class Scheduler:
    def __init__(self, node: Node) -> None:
        self.nodes = {node}
        self.removed: list[Node] = []

    def remove_node(self, node: Node) -> None:
        self.removed.append(node)
        self.nodes.remove(node)
        return None


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("repo", type=Path)
    repo = parser.parse_args().repo.resolve()
    DSession = load_dsession(repo)

    normal_config = Config()
    normal_session = DSession(normal_config)  # type: ignore[arg-type]
    normal_node = Node(1)
    normal_scheduler = Scheduler(normal_node)
    normal_session.sched = normal_scheduler  # type: ignore[assignment]
    normal_session._active_nodes.add(normal_node)  # type: ignore[arg-type]

    normal_session.worker_workerfinished(normal_node)  # type: ignore[arg-type]

    assert not normal_session.shuttingdown
    assert normal_scheduler.removed == [normal_node]
    assert normal_config.hook.down == [(normal_node, None)]

    config = Config()
    session = DSession(config)  # type: ignore[arg-type]
    node = Node(0)
    scheduler = Scheduler(node)
    session.sched = scheduler  # type: ignore[assignment]
    session._active_nodes.add(node)  # type: ignore[arg-type]

    # WorkerController emits this error for a direct process exit that closes
    # its channel before it can send workerfinished.
    session.worker_errordown(node, "Not properly terminated")  # type: ignore[arg-type]

    message = "worker gw0 crashed and worker restarting disabled"
    assert session.shouldstop == message
    assert session.shuttingdown
    assert not session._active_nodes
    assert scheduler.removed == [node]
    assert config.hook.down == [(node, "Not properly terminated")]

    restart_config = Config(maxworkerrestart="1")
    restart_session = DSession(restart_config)  # type: ignore[arg-type]
    restart_node = Node(0)
    restart_scheduler = Scheduler(restart_node)
    restart_session.sched = restart_scheduler  # type: ignore[assignment]
    restart_session._active_nodes.add(restart_node)  # type: ignore[arg-type]
    replacements: list[Node] = []
    restart_session._clone_node = replacements.append  # type: ignore[method-assign]

    restart_session.worker_errordown(  # type: ignore[arg-type]
        restart_node, "Not properly terminated"
    )

    assert not restart_session.shuttingdown
    assert not getattr(restart_session, "shouldstop", False)
    assert replacements == [restart_node]


if __name__ == "__main__":
    main()
