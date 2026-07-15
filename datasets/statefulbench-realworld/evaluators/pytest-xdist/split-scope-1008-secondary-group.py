#!/usr/bin/env python3
"""Check benchmark-defined secondary group keys in loadscope."""
from __future__ import annotations

import os
from pathlib import Path
import subprocess
import sys


def main() -> None:
    checkout = Path(sys.argv[1]).resolve()
    environment = os.environ.copy()
    environment["PYTHONDONTWRITEBYTECODE"] = "1"
    check = '''
import importlib.util
import sys
import types
from pathlib import Path

pytest = types.ModuleType("pytest")
sys.modules["pytest"] = pytest
xdist = types.ModuleType("xdist")
xdist.__path__ = []
remote = types.ModuleType("xdist.remote")
remote.Producer = object
report = types.ModuleType("xdist.report")
report.report_collection_diff = lambda *args: ""
workermanage = types.ModuleType("xdist.workermanage")
workermanage.WorkerController = object
workermanage.parse_tx_spec_config = lambda config: []
sys.modules.update({
    "xdist": xdist,
    "xdist.remote": remote,
    "xdist.report": report,
    "xdist.workermanage": workermanage,
})
path = Path("src/xdist/scheduler/loadscope.py")
spec = importlib.util.spec_from_file_location("loadscope_under_test", path)
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)

split = module.LoadScopeScheduling._split_scope
assert split(None, "tests/test_api.py::test_create@serial") == "serial"
assert split(None, "tests/test_db.py::TestDatabase::test_write@database") == "database"
assert split(None, "tests/test_api.py::test_user[person@example.test]") == "tests/test_api.py"
assert split(None, "tests/test_api.py::test_user[person@example.test]@accounts") == "accounts"
assert split(None, "tests/test_plain.py::test_plain") == "tests/test_plain.py"
'''
    result = subprocess.run(
        [sys.executable, "-c", check],
        cwd=checkout,
        env=environment,
        text=True,
        capture_output=True,
    )
    assert result.returncode == 0, result.stdout + result.stderr


if __name__ == "__main__":
    main()
