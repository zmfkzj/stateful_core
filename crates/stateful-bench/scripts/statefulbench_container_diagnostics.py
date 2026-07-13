#!/usr/bin/env python3
"""Emit deterministic, value-free diagnostics for a shared agent HOME."""
from __future__ import annotations

import argparse
import hashlib
import json
import os
import sqlite3
import shutil
import tempfile
import stat
from pathlib import Path

_SENSITIVE = ("auth", "credential", "token", "secret", "cookie", "header")
_LOCK_SUFFIXES = ("-wal", "-shm", "-journal", ".lock", ".tmp", ".temp")


def _sensitive(name: str) -> bool:
    return any(pattern in name.casefold() for pattern in _SENSITIVE)


def _regular_descriptor(
    path: Path, expected: os.stat_result | None = None
) -> tuple[int, os.stat_result]:
    nofollow = getattr(os, "O_NOFOLLOW", None)
    if nofollow is None:
        raise OSError("O_NOFOLLOW is unavailable")
    descriptor = os.open(path, os.O_RDONLY | os.O_NONBLOCK | nofollow)
    try:
        opened = os.fstat(descriptor)
        if not stat.S_ISREG(opened.st_mode) or (
            expected is not None
            and (
                (opened.st_dev, opened.st_ino, opened.st_size, opened.st_mtime_ns)
                != (
                    expected.st_dev,
                    expected.st_ino,
                    expected.st_size,
                    expected.st_mtime_ns,
                )
            )
        ):
            raise OSError("diagnostic file changed after lstat")
        return descriptor, opened
    except BaseException:
        os.close(descriptor)
        raise


def _digest(path: Path, expected: os.stat_result) -> str:
    descriptor, opened = _regular_descriptor(path, expected)
    try:
        digest = hashlib.sha256()
        while block := os.read(descriptor, 1024 * 1024):
            digest.update(block)
        current = os.fstat(descriptor)
        if (
            (current.st_dev, current.st_ino, current.st_size, current.st_mtime_ns)
            != (opened.st_dev, opened.st_ino, opened.st_size, opened.st_mtime_ns)
        ):
            raise OSError("diagnostic file changed while hashing")
        return digest.hexdigest()
    finally:
        os.close(descriptor)


def _file_record(home: Path, path: Path) -> dict:
    metadata = path.lstat()
    relative = path.relative_to(home).as_posix()
    record = {"path": relative, "size": metadata.st_size, "mtime_ns": metadata.st_mtime_ns}
    if stat.S_ISLNK(metadata.st_mode):
        return {**record, "type": "symlink"}
    if stat.S_ISDIR(metadata.st_mode):
        return {**record, "type": "directory"}
    if stat.S_ISREG(metadata.st_mode):
        return {**record, "type": "file", "sha256": _digest(path, metadata)}
    if stat.S_ISFIFO(metadata.st_mode):
        return {**record, "type": "fifo"}
    if stat.S_ISSOCK(metadata.st_mode):
        return {**record, "type": "socket"}
    return {**record, "type": "device"}


def _quote_identifier(identifier: str) -> str:
    return '"' + identifier.replace('"', '""') + '"'


def _copy_descriptor(descriptor: int, destination: Path) -> None:
    output = os.open(destination, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    try:
        while block := os.read(descriptor, 1024 * 1024):
            remaining = memoryview(block)
            while remaining:
                written = os.write(output, remaining)
                if written <= 0:
                    raise OSError("diagnostic copy write failed")
                remaining = remaining[written:]
    finally:
        os.close(output)


def _unchanged(current: os.stat_result, expected: os.stat_result) -> bool:
    return (
        current.st_dev,
        current.st_ino,
        current.st_size,
        current.st_mtime_ns,
    ) == (
        expected.st_dev,
        expected.st_ino,
        expected.st_size,
        expected.st_mtime_ns,
    )

def _sqlite_record(path: Path, expected: os.stat_result) -> dict:
    record: dict = {"integrity": "unknown", "schemas": [], "table_counts": {}}
    connection: sqlite3.Connection | None = None
    temporary_dir: Path | None = None
    descriptors: list[tuple[int, os.stat_result]] = []
    sidecars = tuple(path.with_name(f"{path.name}{suffix}") for suffix in ("-wal", "-shm", "-journal"))
    absent_sidecars: list[Path] = []
    try:
        sources: list[tuple[Path, os.stat_result]] = [(path, expected)]
        for sidecar in sidecars:
            try:
                sources.append((sidecar, sidecar.lstat()))
            except FileNotFoundError:
                absent_sidecars.append(sidecar)
        for source, metadata in sources:
            descriptors.append(_regular_descriptor(source, metadata))
        temporary_dir = Path(tempfile.mkdtemp(prefix="statefulbench-sqlite-"))
        temporary_dir.chmod(0o700)
        for (source, _), (descriptor, _) in zip(sources, descriptors, strict=True):
            _copy_descriptor(descriptor, temporary_dir / source.name)
        for sidecar in absent_sidecars:
            try:
                sidecar.lstat()
            except FileNotFoundError:
                continue
            raise OSError("SQLite sidecar appeared during diagnostic capture")
        if any(
            not _unchanged(os.fstat(descriptor), metadata)
            for descriptor, metadata in descriptors
        ):
            raise OSError("SQLite source changed during diagnostic capture")
        for descriptor, _ in descriptors:
            os.close(descriptor)
        descriptors.clear()
        copied_database = temporary_dir / path.name
        connection = sqlite3.connect(f"{copied_database.as_uri()}?mode=ro", uri=True)
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
    except sqlite3.DatabaseError:
        record["integrity"] = "malformed"
    except OSError:
        record["integrity"] = "unavailable"
    finally:
        for descriptor, _ in descriptors:
            os.close(descriptor)
        if connection is not None:
            connection.close()
        if temporary_dir is not None:
            shutil.rmtree(temporary_dir)
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
        metadata = path.lstat()
        is_regular = stat.S_ISREG(metadata.st_mode)
        files.append(_file_record(home, path))
        if is_regular and path.suffix in {".db", ".sqlite", ".sqlite3"}:
            databases[relative] = _sqlite_record(path, metadata)
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
        return {
            item["path"]: item
            for item in snapshot.get("files", [])
            if type(item) is dict and type(item.get("path")) is str
        }

    first, second = index(before), index(after)
    changes = []
    for path in sorted(first.keys() | second.keys()):
        if path not in first:
            changes.append({"path": path, "change": "created"})
        elif path not in second:
            changes.append({"path": path, "change": "deleted"})
        elif first[path] != second[path]:
            changes.append({"path": path, "change": "changed"})
    return changes


def classify_runtime_failure(error: str | None, snapshot: dict | None = None) -> str | None:
    text = (error or "").casefold()
    if "sqlite_unavailable" in text:
        return "sqlite_unavailable"
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
