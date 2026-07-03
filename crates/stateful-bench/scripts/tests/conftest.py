from __future__ import annotations

import importlib.util
import subprocess
import sys
from pathlib import Path
from types import ModuleType


SCRIPT_DIR = Path(__file__).resolve().parents[1]


def load_script(name: str) -> ModuleType:
    path = SCRIPT_DIR / name
    sys.path.insert(0, str(SCRIPT_DIR))
    spec = importlib.util.spec_from_file_location(f"{path.stem}_test", path)
    assert spec and spec.loader
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


def arg_after(command: list[str], flag: str) -> str | None:
    try:
        return command[command.index(flag) + 1]
    except (ValueError, IndexError):
        return None


def write_jsonl(path: Path, rows: list[str]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text("\n".join(rows) + "\n", encoding="utf-8")


def init_git_repo(workspace: Path) -> None:
    workspace.mkdir(parents=True, exist_ok=True)
    subprocess.run(["git", "init"], cwd=workspace, check=True, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
    subprocess.run(["git", "config", "user.email", "bench@example.test"], cwd=workspace, check=True)
    subprocess.run(["git", "config", "user.name", "Bench Test"], cwd=workspace, check=True)
