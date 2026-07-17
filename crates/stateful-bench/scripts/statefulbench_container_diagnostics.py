#!/usr/bin/env python3
"""Emit deterministic, value-free diagnostics for a shared agent HOME."""
from __future__ import annotations

import argparse
from datetime import datetime, timezone
import hashlib
import json
import os
import sqlite3
import shutil
import tempfile
from math import isfinite
import stat
from pathlib import Path

_SENSITIVE = ("auth", "credential", "token", "secret", "cookie", "header")
_LOCK_SUFFIXES = ("-wal", "-shm", "-journal", ".lock", ".tmp", ".temp")
_CONTEXT_RENDER_SUCCESS_MARKER = b"[stateful-metric] context_render_success"
_V2_TABLES = {
    "journal_events",
    "presence_current",
    "handoff_current",
    "read_observation_current",
    "wait_current",
    "context_delivery_current",
    "workspace_version",
    "notification_current",
}
_V2_JOURNAL_EVENT_TYPES = frozenset(
    {
        "migration.started", "migration.legacy_audit_imported",
        "migration.presence_snapshot_seeded", "migration.reservation_snapshot_seeded",
        "migration.claim_snapshot_seeded", "migration.wait_snapshot_seeded",
        "migration.write_fence_snapshot_seeded", "migration.human_observation_snapshot_seeded",
        "migration.legacy_handoff_snapshot_seeded", "migration.delivery_snapshot_seeded",
        "migration.validated", "migration.completed",
        "presence.registered", "presence.heartbeat", "presence.goal_updated",
        "presence.phase_updated", "presence.plan_updated", "presence.resources_updated",
        "presence.tool_started", "presence.tool_completed", "presence.finalized",
        "presence.expired",
        "reservation.declared", "reservation.refreshed", "reservation.released",
        "reservation.expired",
        "claim.acquired", "claim.observation_refreshed", "claim.released", "claim.expired",
        "wait.requested", "wait.became_claimable", "wait.claimed", "wait.cancelled",
        "wait.expired",
        "write_fence.acquired", "write_fence.conflict_observed", "write_fence.released",
        "write_fence.expired",
        "read_observation.started", "read_observation.stabilized",
        "read_observation.unstable", "read_observation.aborted",
        "read_observation.invalidated", "read_observation.expired",
        "write_intent.started", "write_intent.committed", "write_intent.failed",
        "write_intent.outcome_unknown", "write_intent.reconciled",
        "human_observation.observed", "human_observation.reconciled",
        "human_observation.expired", "human_acknowledgement.recorded",
        "handoff.finalized", "handoff.expired",
        "authorization.allowed", "authorization.warned", "authorization.denied",
        "authorization.override_granted",
        "context.rendered", "context.delivery_created",
        "context.delivery_acknowledged", "context.delivery_superseded",
        "notification.created", "notification.delivered", "notification.expired",
        "notification.coalesced",
        "recovery.queued", "recovery.attempted", "recovery.delivered", "recovery.failed",
    }
)
_V2_NOTIFICATION_KINDS = frozenset(
    {"context_invalidated", "reservation_granted", "scope_overlap"}
)
_V2_HANDOFF_STATUSES = frozenset({"done", "failed", "blocked", "cancelled", "unknown"})
_V2_WARNED_REASON_CODES = frozenset(
    {
        "missing_read_provenance", "missing_reservation", "inactive_session_phase",
        "scope_mismatch", "invalid_write_action", "missing_claim",
        "coordination_conflict",
    }
)
_V2_DENIED_REASON_CODES = frozenset(
    {
        "invalid_target", "unknown_write_outcome", "stale_observation",
        "write_fence_conflict", "unreconciled_human_write", "missing_read_provenance",
        "missing_reservation", "inactive_session_phase", "scope_mismatch",
        "invalid_write_action", "missing_claim",
    }
)
_V2_WAIT_STATUSES = frozenset({"queued", "claimable", "claimed", "canceled", "expired"})


def _group_counts(rows) -> dict[str, int]:
    counts: dict[str, int] = {}
    for value, count in rows:
        if isinstance(value, str) and isinstance(count, int) and count >= 0:
            counts[value] = count
    return dict(sorted(counts.items()))


def _parse_timestamp(value: object) -> datetime | None:
    if not isinstance(value, str):
        return None
    try:
        parsed = datetime.fromisoformat(value.replace("Z", "+00:00"))
    except ValueError:
        return None
    return parsed if parsed.tzinfo is not None else parsed.replace(tzinfo=timezone.utc)


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

def _rooted_regular_descriptor(
    root: Path, path: Path, expected: os.stat_result
) -> tuple[int, os.stat_result]:
    nofollow = getattr(os, "O_NOFOLLOW", None)
    directory_flag = getattr(os, "O_DIRECTORY", None)
    if nofollow is None or directory_flag is None:
        raise OSError("safe directory descriptor flags are unavailable")
    relative = path.relative_to(root)
    if not relative.parts:
        raise OSError("diagnostic log must be a file below its HOME")
    directory = os.open(root, os.O_RDONLY | directory_flag | nofollow)
    try:
        for component in relative.parts[:-1]:
            child = os.open(
                component, os.O_RDONLY | directory_flag | nofollow, dir_fd=directory
            )
            os.close(directory)
            directory = child
        descriptor = os.open(
            relative.parts[-1], os.O_RDONLY | os.O_NONBLOCK | nofollow, dir_fd=directory
        )
    finally:
        os.close(directory)
    try:
        opened = os.fstat(descriptor)
        if not stat.S_ISREG(opened.st_mode) or not _unchanged(opened, expected):
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

def _count_context_render_markers(
    path: Path, expected: os.stat_result, root: Path | None = None
) -> int:
    descriptor, opened = (
        _regular_descriptor(path, expected)
        if root is None
        else _rooted_regular_descriptor(root, path, expected)
    )
    try:
        count = 0
        partial = b""
        while block := os.read(descriptor, 1024 * 1024):
            lines = (partial + block).split(b"\n")
            partial = lines.pop()
            count += sum(
                line == _CONTEXT_RENDER_SUCCESS_MARKER for line in lines
            )
        if partial == _CONTEXT_RENDER_SUCCESS_MARKER:
            count += 1
        if not _unchanged(os.fstat(descriptor), opened):
            raise OSError("diagnostic file changed while counting markers")
        return count
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

def _event_data(value: object) -> dict:
    try:
        payload = json.loads(value)
    except (TypeError, UnicodeDecodeError, json.JSONDecodeError):
        return {}
    event = payload.get("event") if isinstance(payload, dict) else None
    event_data = event.get("data") if isinstance(event, dict) else None
    data = event_data.get("data") if isinstance(event_data, dict) else None
    return data if isinstance(data, dict) else {}




def _projection_statuses(connection: sqlite3.Connection) -> dict[str, int]:
    statuses: dict[str, int] = {}
    for (payload_json,) in connection.execute("SELECT payload_json FROM wait_current"):
        try:
            payload = json.loads(payload_json)
        except (TypeError, UnicodeDecodeError, json.JSONDecodeError):
            continue
        status = payload.get("status") if isinstance(payload, dict) else None
        if isinstance(status, str) and status in _V2_WAIT_STATUSES:
            statuses[status] = statuses.get(status, 0) + 1
    return dict(sorted(statuses.items()))


def _coordination_metrics(
    connection: sqlite3.Connection, table_names: set[str], database_bytes: int
) -> dict | None:
    if not _V2_TABLES.issubset(table_names) or database_bytes < 0:
        return None
    rows = list(
        connection.execute(
            """
            SELECT aggregate_id, event_type, occurred_at, payload_json, agent_id
            FROM journal_events ORDER BY event_seq
            """
        )
    )
    by_event_type: dict[str, int] = {}
    active_presence: set[str] = set()
    peak_active = 0
    handoff_statuses: dict[str, int] = {}
    notification_kinds: dict[str, int] = {}
    warned: dict[str, int] = {}
    denied: dict[str, int] = {}
    requested_at: dict[str, datetime] = {}
    grants: list[tuple[object, object]] = []
    same_path_operations: set[str] = set()
    previous_writers: dict[str, str] = {}
    cross_agent_overwrites = 0

    for aggregate_id, event_type, occurred_at, payload_json, agent_id in rows:
        category = (
            event_type
            if isinstance(event_type, str) and event_type in _V2_JOURNAL_EVENT_TYPES
            else None
        )
        if category is None:
            continue
        by_event_type[category] = by_event_type.get(category, 0) + 1
        data = _event_data(payload_json)
        if category == "presence.registered" and isinstance(aggregate_id, str):
            active_presence.add(aggregate_id)
            peak_active = max(peak_active, len(active_presence))
        elif category in {"presence.finalized", "presence.expired"} and isinstance(aggregate_id, str):
            active_presence.discard(aggregate_id)

        if category == "handoff.finalized":
            handoff = data.get("handoff")
            if isinstance(handoff, dict):
                status = handoff.get("status")
                if isinstance(status, str) and status in _V2_HANDOFF_STATUSES:
                    handoff_statuses[status] = handoff_statuses.get(status, 0) + 1
                if handoff.get("explicit") is False:
                    fallback_cause = data.get("fallback_cause")
                    if fallback_cause not in {"stop", "ttl"}:
                        return None
                    key = f"_fallback_{fallback_cause}"
                    handoff_statuses[key] = handoff_statuses.get(key, 0) + 1

        if category == "notification.created":
            notification = data.get("notification")
            if isinstance(notification, dict):
                kind = notification.get("kind")
                if isinstance(kind, str) and kind in _V2_NOTIFICATION_KINDS:
                    notification_kinds[kind] = notification_kinds.get(kind, 0) + 1
                    if kind == "reservation_granted":
                        grants.append((notification.get("payload"), occurred_at))

        if category == "wait.requested" and isinstance(aggregate_id, str):
            timestamp = _parse_timestamp(occurred_at)
            if timestamp is not None:
                requested_at[aggregate_id] = timestamp

        if category in {"authorization.warned", "authorization.denied"}:
            reason = data.get("reason_code")
            allowed_reasons = (
                _V2_WARNED_REASON_CODES
                if category == "authorization.warned"
                else _V2_DENIED_REASON_CODES
            )
            if isinstance(reason, str) and reason in allowed_reasons:
                target = warned if category == "authorization.warned" else denied
                target[reason] = target.get(reason, 0) + 1

        if category == "write_fence.conflict_observed":
            operation = data.get("operation_id")
            if isinstance(operation, str) and operation:
                same_path_operations.add(operation)

        if category == "write_intent.committed":
            intent = data.get("write_intent")
            writer = agent_id if isinstance(agent_id, str) else None
            targets = intent.get("targets") if isinstance(intent, dict) else None
            if writer is not None and isinstance(targets, list):
                for target in targets:
                    path = target.get("path") if isinstance(target, dict) else None
                    if isinstance(path, str):
                        if path in previous_writers and previous_writers[path] != writer:
                            cross_agent_overwrites += 1
                        previous_writers[path] = writer

    fallback_stop = handoff_statuses.pop("_fallback_stop", 0)
    fallback_ttl = handoff_statuses.pop("_fallback_ttl", 0)
    durations: list[float] = []
    unmeasured_grants = 0
    for payload, created_at in grants:
        wait_id = payload.get("wait_id") if isinstance(payload, dict) else None
        requested = requested_at.get(wait_id) if isinstance(wait_id, str) else None
        created = _parse_timestamp(created_at)
        if requested is None or created is None:
            unmeasured_grants += 1
            continue
        seconds = (created - requested).total_seconds()
        if not isfinite(seconds) or seconds < 0:
            unmeasured_grants += 1
            continue
        durations.append(round(seconds, 6))
    total = round(sum(durations), 6) if durations else 0.0
    count = len(durations)
    version_row = connection.execute("SELECT COALESCE(MAX(version), 0) FROM workspace_version").fetchone()
    versions = version_row[0] if version_row and type(version_row[0]) is int and version_row[0] >= 0 else 0

    def event_count(name: str) -> int:
        return by_event_type.get(name, 0)

    prompt_utf8_bytes = 0
    prompt_unicode_scalars = 0
    prompt_items = 0
    for _aggregate_id, event_type, _occurred_at, payload_json, _agent_id in rows:
        if event_type != "context.delivery_created":
            continue
        delivery = _event_data(payload_json).get("context_delivery")
        if not isinstance(delivery, dict):
            continue
        prompt = delivery.get("prompt_text")
        if isinstance(prompt, str):
            prompt_utf8_bytes += len(prompt.encode("utf-8"))
            prompt_unicode_scalars += len(prompt)
        items = delivery.get("items")
        if isinstance(items, list):
            prompt_items += len(items)

    return {
        "protocol_version": "stateful.v2",
        "journal": {
            "events": len(rows),
            "bytes_start": database_bytes,
            "bytes_end": database_bytes,
            "bytes_growth": 0,
            "by_event_type": dict(sorted(by_event_type.items())),
        },
        "presence": {
            "registered": event_count("presence.registered"),
            "expired": event_count("presence.expired"),
            "finalized": event_count("presence.finalized"),
            "peak_active": peak_active,
        },
        "handoffs": {
            "explicit": sum(
                isinstance(handoff, dict) and handoff.get("explicit") is True
                for _aggregate_id, event_type, _occurred_at, payload_json, _agent_id in rows
                if event_type == "handoff.finalized"
                for handoff in (_event_data(payload_json).get("handoff"),)
            ),
            "fallback_stop": fallback_stop,
            "fallback_ttl": fallback_ttl,
            "by_status": dict(sorted(handoff_statuses.items())),
        },
        "read_observations": {
            "started": event_count("read_observation.started"),
            "stable": event_count("read_observation.stabilized"),
            "unstable": event_count("read_observation.unstable"),
            "aborted": event_count("read_observation.aborted"),
            "invalidated": event_count("read_observation.invalidated"),
        },
        "context": {
            "versions": versions,
            "renders": event_count("context.rendered"),
            "deliveries": event_count("context.delivery_created"),
            "acks": event_count("context.delivery_acknowledged"),
            "redeliveries": event_count("context.delivery_superseded"),
            "coalesced": event_count("notification.coalesced"),
            "prompt_utf8_bytes": prompt_utf8_bytes,
            "prompt_unicode_scalars": prompt_unicode_scalars,
            "prompt_items": prompt_items,
        },
        "authorization": {
            "warned_by_reason": dict(sorted(warned.items())),
            "denied_by_reason": dict(sorted(denied.items())),
        },
        "write_safety": {
            "fence_conflicts": event_count("write_fence.conflict_observed"),
            "unknown_outcomes": event_count("write_intent.outcome_unknown"),
            "same_path_overlaps": len(same_path_operations),
            "cross_agent_overwrites": cross_agent_overwrites,
        },
        "notifications": {"by_kind": dict(sorted(notification_kinds.items()))},
        "waits": {
            "by_final_status": _projection_statuses(connection),
            "grant_wait_time_s": {
                "count": count,
                "total": total,
                "mean": None if count == 0 else round(total / count, 6),
                "max": None if count == 0 else max(durations),
            },
            "unmeasured_grants": unmeasured_grants,
        },
    }


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
        coordination_metrics = _coordination_metrics(
            connection, set(names), sum(metadata.st_size for _, metadata in sources)
        )
        if coordination_metrics is not None:
            record["coordination_metrics"] = coordination_metrics
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
    server_log = home / ".stateful" / "runtime" / "server.log"
    try:
        server_log_metadata = server_log.lstat()
    except FileNotFoundError:
        context_render_count = None
    else:
        context_render_count = _count_context_render_markers(
            server_log, server_log_metadata, root=home
        )
    return {
        "schema_version": 1,
        "home": home.as_posix() if home.as_posix().startswith("/home/") else "<home>",
        "files": files,
        "databases": databases,
        "lock_files": locks,
        "per_agent_home_tree": (home.parent / "agents").exists(),
        "processes": _process_snapshot(),
        "runtime_metrics": {
            "context_render_success_count": context_render_count,
        },
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
