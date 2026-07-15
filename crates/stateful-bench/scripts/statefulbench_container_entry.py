#!/usr/bin/env python3
from __future__ import annotations

import ctypes
import json
import os
import sys
from pathlib import Path


def _set_child_subreaper() -> None:
    prctl = ctypes.CDLL(None, use_errno=True).prctl
    if prctl(36, 1, 0, 0, 0) != 0:  # PR_SET_CHILD_SUBREAPER
        raise OSError(ctypes.get_errno(), "PR_SET_CHILD_SUBREAPER failed")


def _reap_descendants() -> None:
    while True:
        try:
            os.waitpid(-1, 0)
        except ChildProcessError:
            return


def _redirect_child_output(stdout: str, stderr: str) -> None:
    for path, descriptor in ((stdout, 1), (stderr, 2)):
        output = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)
        try:
            os.dup2(output, descriptor)
        finally:
            os.close(output)


def _emit_completion(pid: int, exit_code: int) -> None:
    sys.stdout.write(
        json.dumps({"pid": pid, "pgid": pid, "exit_code": exit_code}) + "\n"
    )
    sys.stdout.flush()

def main(argv: list[str]) -> int:
    if len(argv) < 5:
        raise SystemExit(
            "usage: statefulbench-container-entry PID_FILE STDOUT_LOG STDERR_LOG COMMAND [ARG ...]"
        )
    _set_child_subreaper()
    child = os.fork()
    if child:
        _, status = os.waitpid(child, 0)
        _reap_descendants()
        _emit_completion(child, os.waitstatus_to_exitcode(status))
        return 0
    pid_file = Path(argv[1])
    _redirect_child_output(argv[2], argv[3])
    os.setsid()
    record = {"pid": os.getpid(), "pgid": os.getpgrp()}
    temporary = pid_file.with_suffix(".tmp")
    temporary.write_text(json.dumps(record) + "\n", encoding="utf-8")
    os.replace(temporary, pid_file)
    os.execvpe(argv[4], argv[4:], os.environ)
    return 127


if __name__ == "__main__":
    raise SystemExit(main(os.sys.argv))
