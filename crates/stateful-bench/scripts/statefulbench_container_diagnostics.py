#!/usr/bin/env python3
"""Emit deterministic, value-free diagnostics for a shared agent HOME."""
from __future__ import annotations

import argparse
import hashlib
import json
import os
import sqlite3
import stat
from pathlib import Path

_SENSITIVE = ("auth", "credential", "token", "secret", "cookie", "header")
_LOCK_SUFFIXES = ("-wal", "-shm", "-journal", ".lock", ".tmp", ".temp")


def _sensitive(name: str) -> bool:
    return any(pattern in name.casefold() for pattern in _SENSITIVE)


def _digest(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as source:
        for block in iter(lambda: source.read(1024 * 1024), b""):
            digest.update(block)
    return digest.hexdigest()


def _file_record(home: Path, path: Path) -> dict:
    metadata = path.lstat()
    relative = path.relative_to(home).as_posix()
    record = {"path": relative, "size": metadata.st_size, "mtime_ns": metadata.st_mtime_ns}
    if stat.S_ISLNK(metadata.st_mode):
        return {**record, "type": "symlink"}
    if stat.S_ISDIR(metadata.st_mode):
        return {**record, "type": "directory"}
    if stat.S_ISREG(metadata.st_mode):
        return {**record, "type": "file", "sha256": _digest(path)}
    if stat.S_ISFIFO(metadata.st_mode):
        return {**record, "type": "fifo"}
    if stat.S_ISSOCK(metadata.st_mode):
        return {**record, "type": "socket"}
    return {**record, "type": "device"}


def _quote_identifier(identifier: str) -> str:
    return '"' + identifier.replace('"', '""') + '"'


def _sqlite_record(path: Path) -> dict:
    record: dict = {"integrity": "unknown", "schemas": [], "table_counts": {}}
    connection: sqlite3.Connection | None = None
    try:
        connection = sqlite3.connect(f"file:{path}?mode=ro", uri=True)
        integrity = connection.execute("pragma integrity_check").fetchone()
        if integrity != ("ok",):
            record["integrity"] = "malformed"
            return record
        record["integrity"] = "ok"
        names = [
            row[0]
            for row in connection.execute(
                "select name from sqlite_master where type = 'table' order by name"
            )
            if isinstance(row[0], str) and not _sensitive(row[0])
        ]
        record["schemas"] = names
        record["table_counts"] = {
            name: connection.execute(f"select count(*) from {_quote_identifier(name)}").fetchone()[0]
            for name in names
        }
    except sqlite3.OperationalError as error:
        record["integrity"] = (
            "locked"
            if "locked" in str(error).casefold() or "busy" in str(error).casefold()
            else "unavailable"
        )
    except (sqlite3.DatabaseError, OSError):
        record["integrity"] = "malformed"
    finally:
        if connection is not None:
            connection.close()
    return record


def _process_snapshot() -> list[dict]:
    processes = []
    proc = Path("/proc")
    if not proc.is_dir():
        return processes
    for child in sorted(proc.iterdir(), key=lambda entry: entry.name):
        if not child.name.isdecimal():
            continue
        try:
            fields = (child / "stat").read_text(encoding="utf-8").rsplit(") ", 1)[1].split()
            command = (child / "comm").read_text(encoding="utf-8").strip()
            processes.append(
                {
                    "pid": int(child.name),
                    "ppid": int(fields[1]),
                    "pgid": int(fields[2]),
                    "command": Path(command).name,
                }
            )
        except (OSError, IndexError, ValueError):
            continue
    return processes


def snapshot_home(home: Path) -> dict:
    home = home.resolve()
    if not home.is_dir():
        raise ValueError("HOME must be an existing directory")
    files = []
    databases = {}
    locks = []
    for path in sorted(home.rglob("*"), key=lambda entry: entry.relative_to(home).as_posix()):
        relative = path.relative_to(home).as_posix()
        if _sensitive(relative):
            continue
        is_regular = stat.S_ISREG(path.lstat().st_mode)
        files.append(_file_record(home, path))
        if is_regular and path.suffix in {".db", ".sqlite", ".sqlite3"}:
            databases[relative] = _sqlite_record(path)
        if is_regular and path.name.casefold().endswith(_LOCK_SUFFIXES):
            locks.append(relative)
    return {
        "schema_version": 1,
        "home": home.as_posix() if home.as_posix().startswith("/home/") else "<home>",
        "files": files,
        "databases": databases,
        "lock_files": locks,
        "per_agent_home_tree": (home.parent / "agents").exists(),
        "processes": _process_snapshot(),
    }


def snapshot_changes(before: dict, after: dict) -> list[dict]:
    def index(snapshot: dict) -> dict[str, dict]:
        return {item["path"]: item for item in snapshot.get("files", []) if item.get("type") == "file"}

    first, second = index(before), index(after)
    changes = []
    for path in sorted(first.keys() | second.keys()):
        if path not in first:
            changes.append({"path": path, "change": "created"})
        elif path not in second:
            changes.append({"path": path, "change": "deleted"})
        elif any(first[path].get(key) != second[path].get(key) for key in ("size", "mtime_ns", "sha256")):
            changes.append({"path": path, "change": "changed"})
    return changes


def classify_runtime_failure(error: str | None, snapshot: dict | None = None) -> str | None:
    text = (error or "").casefold()
    if "locked" in text or "busy" in text:
        return "sqlite_locked"
    if "malformed" in text or "not a database" in text:
        return "sqlite_malformed"
    for database in (snapshot or {}).get("databases", {}).values():
        if database.get("integrity") == "locked":
            return "sqlite_locked"
        if database.get("integrity") == "malformed":
            return "sqlite_malformed"
        if database.get("integrity") != "ok":
            return "sqlite_unavailable"
    return "unclassified_runtime_failure" if error else None


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--home", required=True)
    parser.add_argument("--phase", required=True, choices=("initialized", "before-tasks", "after-tasks", "after-final", "after-grading", "before-remove"))
    parser.add_argument("--output", required=True)
    args = parser.parse_args(argv)
    snapshot = snapshot_home(Path(args.home))
    snapshot["phase"] = args.phase
    output = Path(args.output)
    output.parent.mkdir(parents=True, exist_ok=True)
    temporary = output.with_suffix(".tmp")
    temporary.write_text(json.dumps(snapshot, sort_keys=True, separators=(",", ":")) + "\n", encoding="utf-8")
    os.replace(temporary, output)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
