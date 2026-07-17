#!/usr/bin/env python3
"""Evaluator for pytest-xdist issue #1219."""
import argparse
import sys
from pathlib import Path
from types import ModuleType
from types import SimpleNamespace


def load_module(repo: Path):
    sys.path.insert(0, str(repo / "src"))
    for name in tuple(sys.modules):
        if name == "xdist" or name.startswith("xdist."):
            del sys.modules[name]
    version = ModuleType("xdist._version")
    version.version = "0"  # type: ignore[attr-defined]
    sys.modules["xdist._version"] = version
    import xdist.dsession as dsession

    return dsession


class Hooks:
    def __init__(self) -> None:
        self.setup_calls: list[tuple[object, tuple[str, ...]]] = []

    def pytest_xdist_setupnodes(self, *, config: object, specs: list[str]) -> None:
        self.setup_calls.append((config, tuple(specs)))


class Config:
    def __init__(self, ramp: float) -> None:
        self.ramp = ramp
        self.option = SimpleNamespace(
            debug=False,
            verbose=-1,
            maxworkerrestart=None,
            numprocesses=3,
        )
        self.hook = Hooks()
        self.pluginmanager = SimpleNamespace(getplugin=lambda name: None)

    def getvalue(self, name: str) -> int:
        assert name == "maxfail"
        return 0

    def getoption(self, name: str) -> float:
        assert name == "ramp"
        return self.ramp


class Manager:
    instances: list["Manager"] = []

    def __init__(self, config: Config) -> None:
        self.specs = ["one", "two", "three"]
        self.batch_calls = 0
        self.started: list[tuple[str, int]] = []
        Manager.instances.append(self)

    def setup_nodes(self, putevent: object) -> list[object]:
        self.batch_calls += 1
        return [object() for _ in self.specs]

    def setup_node(self, spec: str, putevent: object, worker_index: int) -> object:
        self.started.append((spec, worker_index))
        return object()


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("repo", type=Path)
    repo = parser.parse_args().repo.resolve()
    dsession = load_module(repo)
    sleeps: list[float] = []
    dsession.NodeManager = Manager
    dsession.time = SimpleNamespace(sleep=sleeps.append)

    config = Config(6.0)
    session = dsession.DSession(config)  # type: ignore[arg-type]
    session.pytest_sessionstart(object())  # type: ignore[arg-type]

    manager = Manager.instances[-1]
    assert manager.batch_calls == 0
    assert manager.started == [("one", 0), ("two", 1), ("three", 2)]
    assert config.hook.setup_calls == [(config, ("one", "two", "three"))]
    assert sleeps == [3.0, 3.0]
    assert len(session._active_nodes) == 3

    no_ramp_config = Config(0.0)
    no_ramp_session = dsession.DSession(no_ramp_config)  # type: ignore[arg-type]
    no_ramp_session.pytest_sessionstart(object())  # type: ignore[arg-type]

    no_ramp_manager = Manager.instances[-1]
    assert no_ramp_manager.batch_calls == 1
    assert no_ramp_manager.started == []
    assert no_ramp_config.hook.setup_calls == []
    assert sleeps == [3.0, 3.0]


if __name__ == "__main__":
    main()
