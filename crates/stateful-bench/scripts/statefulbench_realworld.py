from __future__ import annotations

import hashlib
import json
import os
import re
import tarfile
import tempfile
from pathlib import Path
from urllib import request
from urllib.parse import urlsplit


_REPOSITORY_FIELDS = frozenset(
    {
        "key",
        "requested_url",
        "canonical_url",
        "commit",
        "archive_url",
        "archive_sha256",
        "python",
        "setup",
        "suite",
        "corpus",
    }
)
_HEX_40 = re.compile(r"[0-9a-f]{40}")
_HEX_64 = re.compile(r"[0-9a-f]{64}")


def _sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as source:
        while chunk := source.read(1024 * 1024):
            digest.update(chunk)
    return digest.hexdigest()


def ensure_archive(repo: dict, cache_dir: Path, opener=request.urlopen) -> Path:
    expected_sha256 = repo["archive_sha256"]
    archive = cache_dir / f"{expected_sha256}.tar.gz"
    if archive.exists():
        if _sha256(archive) == expected_sha256:
            return archive
        raise ValueError("cached archive checksum mismatch")

    cache_dir.mkdir(parents=True, exist_ok=True)
    temporary: Path | None = None
    digest = hashlib.sha256()
    try:
        with tempfile.NamedTemporaryFile(
            mode="wb",
            dir=cache_dir,
            prefix=f"{expected_sha256}.",
            suffix=".tmp",
            delete=False,
        ) as output:
            temporary = Path(output.name)
            with opener(repo["archive_url"]) as response:
                while chunk := response.read(1024 * 1024):
                    output.write(chunk)
                    digest.update(chunk)
        if digest.hexdigest() != expected_sha256:
            raise ValueError("downloaded archive checksum mismatch")
        os.replace(temporary, archive)
    finally:
        if temporary is not None:
            temporary.unlink(missing_ok=True)
    return archive


def extract_workspace(archive: Path, expected_sha256: str, destination: Path) -> None:
    if destination.exists():
        raise ValueError("workspace destination must be absent")
    if _sha256(archive) != expected_sha256:
        raise ValueError("archive checksum mismatch")

    with tarfile.open(archive, "r:gz") as source:
        members = source.getmembers()
        if any(member.name == "." or member.name.startswith("./") for member in members):
            raise ValueError("archive contains unsafe members")
        roots = {member.name.split("/", 1)[0] for member in members if member.name}
        if len(roots) != 1:
            raise ValueError("archive must contain exactly one root directory")
        root = roots.pop()
        if any(".." in member.name.split("/") for member in members):
            raise ValueError("archive contains unsafe members")
        if not any(member.name.rstrip("/") == root and member.isdir() for member in members):
            raise ValueError("archive root must be a directory")
        if any(member.issym() or member.islnk() for member in members):
            raise ValueError("archive contains link members")

        with tempfile.TemporaryDirectory(dir=destination.parent) as temporary:
            extracted = Path(temporary)
            try:
                source.extractall(extracted, filter="data")
            except tarfile.TarError as error:
                raise ValueError("archive contains unsafe members") from error
            root_directory = extracted / root
            destination.mkdir()
            for child in root_directory.iterdir():
                child.replace(destination / child.name)


def _require_string(entry: dict, field: str) -> str:
    value = entry[field]
    if type(value) is not str or not value:
        raise ValueError(f"{field} must be a non-empty string")
    return value


def _require_https_url(entry: dict, field: str) -> None:
    value = _require_string(entry, field)
    try:
        parsed = urlsplit(value)
        valid = (
            parsed.scheme == "https"
            and parsed.hostname is not None
            and parsed.path
            and not parsed.username
            and not parsed.password
            and not parsed.query
            and not parsed.fragment
        )
    except ValueError:
        valid = False
    if not valid:
        raise ValueError(f"{field} must be an HTTPS URL")


def _require_argv(entry: dict, field: str) -> None:
    value = entry[field]
    if type(value) is not list or not value or any(type(part) is not str or not part for part in value):
        raise ValueError(f"{field} must be a non-empty argv array")


def _validate_repository(entry: object, manifest_dir: Path, keys: set[str]) -> None:
    if type(entry) is not dict:
        raise ValueError("repository entry must be an object")
    if set(entry) != _REPOSITORY_FIELDS:
        raise ValueError("repository entry fields are invalid")

    key = _require_string(entry, "key")
    if key in keys:
        raise ValueError(f"duplicate repository key: {key}")
    keys.add(key)

    for field in ("requested_url", "canonical_url", "archive_url"):
        _require_https_url(entry, field)
    for field, pattern in (("commit", _HEX_40), ("archive_sha256", _HEX_64)):
        value = _require_string(entry, field)
        if not pattern.fullmatch(value):
            raise ValueError(f"{field} has invalid SHA format")
    if _require_string(entry, "python") != "3.14.6":
        raise ValueError("python must be 3.14.6")
    for field in ("setup", "suite"):
        _require_argv(entry, field)

    corpus = Path(_require_string(entry, "corpus"))
    resolved_corpus = (manifest_dir / corpus).resolve()
    if corpus.is_absolute() or not resolved_corpus.is_relative_to(manifest_dir):
        raise ValueError("corpus path must remain below the manifest directory")


def load_manifest(path: Path) -> dict:
    try:
        manifest = json.loads(path.read_text(encoding="utf-8"))
    except json.JSONDecodeError as error:
        raise ValueError("manifest is not valid JSON") from error
    if type(manifest) is not dict:
        raise ValueError("manifest must be an object")
    if set(manifest) != {"schema_version", "generated_at", "repositories"}:
        raise ValueError("manifest fields are invalid")
    if type(manifest["schema_version"]) is not int or manifest["schema_version"] != 1:
        raise ValueError("schema_version must be 1")
    if type(manifest["generated_at"]) is not str or not manifest["generated_at"]:
        raise ValueError("generated_at must be a non-empty string")
    repositories = manifest["repositories"]
    if type(repositories) is not list or len(repositories) != 10:
        raise ValueError("manifest must contain exactly ten repositories")

    manifest_dir = path.parent.resolve()
    keys: set[str] = set()
    for entry in repositories:
        _validate_repository(entry, manifest_dir, keys)
    return manifest


def repo_entries(manifest: dict) -> tuple[dict, ...]:
    if type(manifest) is not dict or type(manifest.get("repositories")) is not list:
        raise ValueError("manifest repositories must be an array")
    return tuple(manifest["repositories"])
