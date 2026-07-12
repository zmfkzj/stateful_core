from __future__ import annotations

import json
import os
from pathlib import Path


def main(argv: list[str]) -> int:
    if len(argv) < 3:
        raise SystemExit("usage: statefulbench-container-entry PID_FILE COMMAND [ARG ...]")
    pid_file = Path(argv[1])
    os.setsid()
    record = {"pid": os.getpid(), "pgid": os.getpgrp()}
    temporary = pid_file.with_suffix(".tmp")
    temporary.write_text(json.dumps(record) + "\n", encoding="utf-8")
    os.replace(temporary, pid_file)
    os.execvpe(argv[2], argv[2:], os.environ)
    return 127


if __name__ == "__main__":
    main(os.sys.argv)
