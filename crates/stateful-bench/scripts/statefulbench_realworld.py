from __future__ import annotations

import argparse
import ast
import contextlib
import hashlib
import importlib.util
import json
import os
import posixpath
import re
import shutil
import signal
import subprocess
import sys
import tarfile
import tempfile
import time
from contextlib import nullcontext
from dataclasses import replace
from pathlib import Path
from urllib import request
from urllib.parse import urlsplit


_LITE_PATH = Path(__file__).with_name("statefulbench_lite.py")
_LITE_SPEC = importlib.util.spec_from_file_location("statefulbench_lite_for_realworld", _LITE_PATH)
if _LITE_SPEC is None or _LITE_SPEC.loader is None:
    raise RuntimeError(f"cannot import lite runner from {_LITE_PATH}")
_LITE = importlib.util.module_from_spec(_LITE_SPEC)
sys.modules[_LITE_SPEC.name] = _LITE
_LITE_SPEC.loader.exec_module(_LITE)
AgentHandle = _LITE.AgentHandle
RunConfig = _LITE.RunConfig


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
_OPTIONAL_REPOSITORY_FIELDS = frozenset({"environment"})
_ENVIRONMENT_NAME = re.compile(r"[A-Z_][A-Z0-9_]*")
_PROTECTED_ENVIRONMENT_NAMES = frozenset(
    {"HOME", "PIP_CACHE_DIR", "TMPDIR", "CARGO_HOME", "VIRTUAL_ENV", "PATH", "RUSTUP_HOME"}
)
_HEX_40 = re.compile(r"[0-9a-f]{40}")
_HEX_64 = re.compile(r"[0-9a-f]{64}")

_CORPUS_FIELDS = frozenset(
    {
        "repository",
        "issue_snapshot",
        "tasks",
        "final_prompt",
        "evaluators",
        "integrated_reference_patch",
    }
)
_TASK_FIELDS = frozenset(
    {
        "key",
        "kind",
        "sources",
        "source_hash",
        "prompt",
        "acceptance",
        "overlap_anchors",
        "evaluator",
        "reference_patch",
    }
)
_ANCHOR_FIELDS = frozenset({"path", "symbol"})


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

def _link_stays_within_root(member: tarfile.TarInfo, root: str) -> bool:
    target = member.linkname
    if posixpath.isabs(target):
        return False
    if member.issym():
        target = posixpath.join(posixpath.dirname(member.name), target)
    target = posixpath.normpath(target)
    return target == root or target.startswith(f"{root}/")


def extract_workspace(archive: Path, expected_sha256: str, destination: Path) -> None:
    if destination.exists():
        raise ValueError("workspace destination must be absent")
    if _sha256(archive) != expected_sha256:
        raise ValueError("archive checksum mismatch")

    try:
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
            if any(
                (member.issym() or member.islnk()) and not _link_stays_within_root(member, root)
                for member in members
            ):
                raise ValueError("archive contains unsafe members")

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
    except tarfile.TarError as error:
        raise ValueError("archive contains unsafe members") from error


def _require_string(entry: dict, field: str) -> str:
    value = entry[field]
    if type(value) is not str or not value:
        raise ValueError(f"{field} must be a non-empty string")
    return value


def _require_key(entry: dict, field: str) -> str:
    key = _require_string(entry, field)
    if key in {".", ".."} or "/" in key or "\\" in key or Path(key).is_absolute():
        raise ValueError(f"{field} must be a safe single-component key")
    return key


def verified_python(required: str) -> Path:
    version = ".".join(str(part) for part in sys.version_info[:3])
    if version != required:
        raise ValueError(f"python version mismatch: manifest requires {required}, found {version}")
    return Path(sys.executable).resolve()


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

def _require_environment(entry: dict) -> None:
    if "environment" not in entry:
        return
    environment = entry["environment"]
    if type(environment) is not dict:
        raise ValueError("environment must be an object")
    for name, value in environment.items():
        if (
            type(name) is not str
            or not _ENVIRONMENT_NAME.fullmatch(name)
            or name in _PROTECTED_ENVIRONMENT_NAMES
            or name.startswith("PYTHON")
            or type(value) is not str
        ):
            raise ValueError("environment contains an unsafe setting")


def _validate_repository(entry: object, manifest_dir: Path, keys: set[str]) -> None:
    if type(entry) is not dict:
        raise ValueError("repository entry must be an object")
    if not _REPOSITORY_FIELDS.issubset(entry) or set(entry) - _REPOSITORY_FIELDS - _OPTIONAL_REPOSITORY_FIELDS:
        raise ValueError("repository entry fields are invalid")

    key = _require_key(entry, "key")
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
    _require_environment(entry)

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

def _require_dataset_path(entry: dict, field: str, dataset_root: Path) -> None:
    value = _require_string(entry, field)
    candidate = Path(value)
    resolved = (dataset_root / candidate).resolve()
    if candidate.is_absolute() or not resolved.is_relative_to(dataset_root):
        raise ValueError(f"{field} path must remain below the dataset root")



def _canonical_evaluator_path(entry: dict, dataset_root: Path) -> Path:
    value = _require_string(entry, "evaluator")
    candidate = Path(value)
    evaluators_root = (dataset_root / "evaluators").resolve()
    if candidate.is_absolute() or ".." in candidate.parts:
        raise ValueError("evaluator path must remain below the evaluators directory")
    try:
        relative = candidate.relative_to("evaluators")
    except ValueError as error:
        raise ValueError("evaluator path must remain below the evaluators directory") from error
    if relative == Path("."):
        raise ValueError("evaluator path must name a file below the evaluators directory")
    resolved = (dataset_root / candidate).resolve()
    if not resolved.is_relative_to(evaluators_root):
        raise ValueError("evaluator path must remain below the evaluators directory")
    return resolved

def _require_github_sources(entry: dict) -> None:
    sources = entry["sources"]
    if type(sources) is not list or not sources:
        raise ValueError("sources must be a non-empty array")
    for source in sources:
        if type(source) is not str or not source:
            raise ValueError("sources must be a non-empty array")
        try:
            parsed = urlsplit(source)
            valid = (
                parsed.scheme == "https"
                and parsed.hostname == "github.com"
                and parsed.port in (None, 443)
                and parsed.path
                and not parsed.username
                and not parsed.password
                and not parsed.query
                and not parsed.fragment
            )
        except ValueError:
            valid = False
        if not valid:
            raise ValueError("sources must be GitHub HTTPS URLs")


def _require_acceptance(entry: dict) -> None:
    acceptance = entry["acceptance"]
    if (
        type(acceptance) is not list
        or len(acceptance) < 3
        or any(type(item) is not str or not item for item in acceptance)
    ):
        raise ValueError("acceptance must contain at least three non-empty strings")

def _require_production_source_path(anchor: dict) -> str:
    path = _require_string(anchor, "path")
    candidate = Path(path)
    parts = path.replace("\\", "/").split("/")
    if (
        candidate.is_absolute()
        or ".." in candidate.parts
        or candidate.suffix != ".py"
        or {"docs", "tests", "generated"} & set(parts)
        or candidate.name.startswith("test_")
        or candidate.name == "conftest.py"
    ):
        raise ValueError("overlap anchor path must identify production Python source")
    return path




def _validate_task(
    entry: object, dataset_root: Path, keys: set[str]
) -> tuple[str, set[tuple[str, str]]]:
    if type(entry) is not dict:
        raise ValueError("task entry must be an object")
    if set(entry) != _TASK_FIELDS:
        raise ValueError("task entry fields are invalid")

    key = _require_key(entry, "key")
    if key in keys:
        raise ValueError(f"duplicate task key: {key}")
    keys.add(key)
    if _require_string(entry, "kind") not in {"bug", "feature"}:
        raise ValueError("kind must be bug or feature")
    _require_string(entry, "prompt")
    _require_github_sources(entry)
    if not _HEX_64.fullmatch(_require_string(entry, "source_hash")):
        raise ValueError("source_hash has invalid SHA format")
    _require_acceptance(entry)
    _canonical_evaluator_path(entry, dataset_root)
    _require_dataset_path(entry, "reference_patch", dataset_root)

    anchors = entry["overlap_anchors"]
    if type(anchors) is not list or not anchors:
        raise ValueError("overlap_anchors must be a non-empty array")
    pairs: set[tuple[str, str]] = set()
    for anchor in anchors:
        if type(anchor) is not dict or set(anchor) != _ANCHOR_FIELDS:
            raise ValueError("overlap_anchors entries must contain path and symbol")
        pairs.add((_require_production_source_path(anchor), _require_string(anchor, "symbol")))
    return key, pairs


def load_corpus(path: Path) -> dict:
    try:
        corpus = json.loads(path.read_text(encoding="utf-8"))
    except json.JSONDecodeError as error:
        raise ValueError("corpus is not valid JSON") from error
    if type(corpus) is not dict:
        raise ValueError("corpus must be an object")
    if set(corpus) != _CORPUS_FIELDS:
        raise ValueError("corpus fields are invalid")

    _require_string(corpus, "repository")
    _require_string(corpus, "final_prompt")
    dataset_root = path.parent.parent.resolve()
    _require_dataset_path(corpus, "issue_snapshot", dataset_root)
    _require_dataset_path(corpus, "integrated_reference_patch", dataset_root)
    evaluators = corpus["evaluators"]
    if type(evaluators) is not list or not evaluators:
        raise ValueError("evaluators must be a non-empty array")
    for evaluator in evaluators:
        if type(evaluator) is not str or not evaluator:
            raise ValueError("evaluators must be a non-empty array")
        _canonical_evaluator_path({"evaluator": evaluator}, dataset_root)

    tasks = corpus["tasks"]
    if type(tasks) is not list or len(tasks) != 10:
        raise ValueError("corpus must contain exactly ten tasks")
    keys: set[str] = set()
    task_anchors = [_validate_task(task, dataset_root, keys) for task in tasks]
    if evaluators != [task["evaluator"] for task in tasks]:
        raise ValueError("evaluators must exactly match task evaluators")
    kinds = [task["kind"] for task in tasks]
    if kinds.count("bug") != 5 or kinds.count("feature") != 5:
        raise ValueError("corpus must contain five bug and five feature tasks")

    anchor_counts: dict[tuple[str, str], int] = {}
    for _, anchors in task_anchors:
        for anchor in anchors:
            anchor_counts[anchor] = anchor_counts.get(anchor, 0) + 1
    for key, anchors in task_anchors:
        if not any(anchor_counts[anchor] > 1 for anchor in anchors):
            raise ValueError(f"task has isolated overlap anchors: {key}")
    return corpus




def repo_entries(manifest: dict) -> tuple[dict, ...]:
    if type(manifest) is not dict or type(manifest.get("repositories")) is not list:
        raise ValueError("manifest repositories must be an array")
    return tuple(manifest["repositories"])


def _corpus_matches_repository(repo: dict, corpus: dict) -> bool:
    return corpus["repository"] in {
        repo["key"],
        urlsplit(repo["canonical_url"]).path.strip("/"),
    }


def _run_logged(
    argv: list[str],
    cwd: Path,
    artifacts: dict[str, dict[str, str]],
    artifact_dir: Path,
    label: str,
    *,
    env: dict[str, str] | None = None,
) -> subprocess.CompletedProcess[str]:
    completed = subprocess.run(
        argv,
        cwd=cwd,
        env=env,
        capture_output=True,
        check=False,
        encoding="utf-8",
        errors="replace",
    )
    number = len(artifacts)
    stdout = artifact_dir / f"{number:03d}.stdout.log"
    stderr = artifact_dir / f"{number:03d}.stderr.log"
    stdout.write_text(completed.stdout, encoding="utf-8")
    stderr.write_text(completed.stderr, encoding="utf-8")
    artifacts[label] = {"stdout": str(stdout), "stderr": str(stderr)}
    return completed


def _sanitized_environment(
    venv: Path | None = None, workspace: Path | None = None
) -> dict[str, str]:
    path_parts = [str(venv / "bin")] if venv is not None else []
    for executable in ("rustc", "cargo"):
        if executable_path := shutil.which(executable):
            path_parts.append(str(Path(executable_path).resolve().parent))
    path_parts.extend(os.defpath.split(os.pathsep))
    env = {"PATH": os.pathsep.join(dict.fromkeys(path_parts))}
    if venv is not None:
        env["VIRTUAL_ENV"] = str(venv)
    rustup_home = os.environ.get("RUSTUP_HOME")
    if rustup_home is None:
        candidate = Path.home() / ".rustup"
        rustup_home = str(candidate) if candidate.is_dir() else None
    if rustup_home:
        env["RUSTUP_HOME"] = rustup_home
    if workspace is not None:
        locations = {
            "HOME": workspace / ".statefulbench-home",
            "PIP_CACHE_DIR": workspace / ".statefulbench-pip-cache",
            "TMPDIR": workspace / ".statefulbench-tmp",
            "CARGO_HOME": workspace / ".statefulbench-cargo-home",
        }
        for name, location in locations.items():
            location.mkdir(parents=True, exist_ok=True)
            env[name] = str(location)
    return env


def _repository_environment(repo: dict, environment: dict[str, str]) -> dict[str, str]:
    return {**environment, **repo.get("environment", {})}


def _venv_argv(argv: list[str], python: Path) -> list[str]:
    if Path(argv[0]).name.startswith("python"):
        return [str(python), *argv[1:]]
    return argv


@contextlib.contextmanager
def _fresh_workspace(
    repo: dict,
    archive: Path,
    cache_dir: Path,
    artifacts: dict[str, dict[str, str]],
    artifact_dir: Path,
    label: str,
):
    interpreter = verified_python(repo["python"])
    with tempfile.TemporaryDirectory(prefix="statefulbench-qualify-", dir=cache_dir) as temporary:
        workspace = Path(temporary) / "workspace"
        try:
            extract_workspace(archive, repo["archive_sha256"], workspace)
        except ValueError as error:
            error_path = artifact_dir / f"{len(artifacts):03d}.extract.stderr.log"
            error_path.write_text(f"{error}\n", encoding="utf-8")
            artifacts[f"{label}:extract"] = {"stdout": "", "stderr": str(error_path)}
            yield None
            return
        initialized = _run_logged(
            ["git", "init"], workspace, artifacts, artifact_dir, f"{label}:git-init"
        )
        indexed = _run_logged(
            ["git", "add", "-A"], workspace, artifacts, artifact_dir, f"{label}:git-add"
        )
        committed = _run_logged(
            [
                "git",
                "-c",
                "user.email=statefulbench@local",
                "-c",
                "user.name=StatefulBench",
                "commit",
                "-m",
                "seed workspace",
            ],
            workspace,
            artifacts,
            artifact_dir,
            f"{label}:git-commit",
        )
        venv = workspace / ".statefulbench-venv"
        created = _run_logged(
            [str(interpreter), "-m", "venv", str(venv)],
            workspace,
            artifacts,
            artifact_dir,
            f"{label}:venv",
            env=_sanitized_environment(workspace=workspace),
        )
        python = venv / "bin" / "python"
        yield (
            (workspace, python, _repository_environment(repo, _sanitized_environment(venv, workspace)))
            if initialized.returncode == 0
            and indexed.returncode == 0
            and committed.returncode == 0
            and created.returncode == 0
            and python.is_file()
            else None
        )


def _patch_hunks(diff: str) -> dict[str, list[tuple[int, int]]]:
    hunks: dict[str, list[tuple[int, int]]] = {}
    path: str | None = None
    header = re.compile(r"^@@ -\d+(?:,\d+)? \+(\d+)(?:,(\d+))? @@")
    for line in diff.splitlines():
        if line.startswith("+++ b/"):
            path = line[6:]
        elif path is not None and (match := header.match(line)):
            hunks.setdefault(path, []).append(
                (int(match.group(1)), int(match.group(2) or 1))
            )
    return hunks


def changed_anchor_symbols(
    source: Path,
    anchors: list[tuple[Path, str, str]],
    hunks: list[tuple[int, int]],
) -> set[str]:
    try:
        tree = ast.parse(source.read_text(encoding="utf-8"))
    except (OSError, SyntaxError):
        return set()
    ranges: dict[str, tuple[int, int]] = {}

    class Symbols(ast.NodeVisitor):
        def __init__(self) -> None:
            self.scope: list[str] = []

        def _record(self, name: str, node: ast.AST) -> None:
            ranges[".".join((*self.scope, name))] = (
                node.lineno,
                node.end_lineno or node.lineno,
            )

        def visit_ClassDef(self, node: ast.ClassDef) -> None:
            self._record(node.name, node)
            self.scope.append(node.name)
            self.generic_visit(node)
            self.scope.pop()

        def visit_FunctionDef(self, node: ast.FunctionDef) -> None:
            self._record(node.name, node)
            self.scope.append(node.name)
            self.generic_visit(node)
            self.scope.pop()

        visit_AsyncFunctionDef = visit_FunctionDef

        def visit_Assign(self, node: ast.Assign) -> None:
            for target in node.targets:
                if isinstance(target, ast.Name):
                    self._record(target.id, node)
            self.generic_visit(node)

        def visit_AnnAssign(self, node: ast.AnnAssign) -> None:
            if isinstance(node.target, ast.Name):
                self._record(node.target.id, node)
            self.generic_visit(node)

    Symbols().visit(tree)
    changed: set[str] = set()
    for anchor_source, path, symbol in anchors:
        if anchor_source != source:
            continue
        target = symbol.split(maxsplit=1)[0]
        matching_ranges = [
            source_range
            for local_name, source_range in ranges.items()
            if target == local_name or target.endswith(f".{local_name}")
        ]
        if any(
            count and start <= last and start + count - 1 >= first
            for first, last in matching_ranges
            for start, count in hunks
        ):
            changed.add(f"{path}:{symbol}")
    return changed


def _apply_patch(
    workspace: Path,
    patch: Path,
    artifacts: dict[str, dict[str, str]],
    artifact_dir: Path,
    label: str,
) -> tuple[bool, dict[str, list[tuple[int, int]]]]:
    applied = _run_logged(
        ["git", "apply", "--index", str(patch)],
        workspace,
        artifacts,
        artifact_dir,
        f"{label}:git-apply",
    )
    if applied.returncode != 0:
        return False, {}
    changed = _run_logged(
        ["git", "diff", "--no-ext-diff", "--cached", "--unified=0"],
        workspace,
        artifacts,
        artifact_dir,
        f"{label}:git-diff",
    )
    return changed.returncode == 0, _patch_hunks(changed.stdout)


def _run_setup(
    repo: dict,
    workspace: Path,
    python: Path,
    env: dict[str, str],
    artifacts: dict[str, dict[str, str]],
    artifact_dir: Path,
    label: str,
) -> bool:
    return (
        _run_logged(
            _venv_argv(repo["setup"], python),
            workspace,
            artifacts,
            artifact_dir,
            f"{label}:setup",
            env=env,
        ).returncode
        == 0
    )


def _run_suite(
    repo: dict,
    workspace: Path,
    python: Path,
    env: dict[str, str],
    artifacts: dict[str, dict[str, str]],
    artifact_dir: Path,
    label: str,
) -> bool:
    return (
        _run_logged(
            _venv_argv(repo["suite"], python),
            workspace,
            artifacts,
            artifact_dir,
            f"{label}:upstream-suite",
            env=env,
        ).returncode
        == 0
    )


def _run_evaluator(
    evaluator: Path,
    workspace: Path,
    python: Path,
    env: dict[str, str],
    artifacts: dict[str, dict[str, str]],
    artifact_dir: Path,
    label: str,
) -> bool:
    return (
        _run_logged(
            [str(python), str(evaluator), str(workspace)],
            workspace,
            artifacts,
            artifact_dir,
            f"{label}:evaluator",
            env=env,
        ).returncode
        == 0
    )


def _qualify_base_suite(
    repo: dict,
    archive: Path,
    cache_dir: Path,
    artifacts: dict[str, dict[str, str]],
    artifact_dir: Path,
) -> bool:
    with _fresh_workspace(
        repo, archive, cache_dir, artifacts, artifact_dir, "base-suite"
    ) as fresh:
        if fresh is None:
            return False
        workspace, python, env = fresh
        return _run_setup(
            repo, workspace, python, env, artifacts, artifact_dir, "base-suite"
        ) and _run_suite(
            repo, workspace, python, env, artifacts, artifact_dir, "base-suite"
        )


def _qualify_task(
    repo: dict,
    task: dict,
    dataset_root: Path,
    archive: Path,
    cache_dir: Path,
    artifacts: dict[str, dict[str, str]],
    artifact_dir: Path,
) -> dict:
    evaluator = dataset_root / task["evaluator"]
    base_red = False
    with _fresh_workspace(
        repo, archive, cache_dir, artifacts, artifact_dir, f"{task['key']}:base"
    ) as fresh:
        if fresh is not None:
            workspace, python, env = fresh
            base_red = _run_setup(
                repo, workspace, python, env, artifacts, artifact_dir, f"{task['key']}:base"
            ) and not _run_evaluator(
                evaluator, workspace, python, env, artifacts, artifact_dir, f"{task['key']}:base"
            )

    reference_green = False
    changed_hunks: dict[str, list[tuple[int, int]]] = {}
    with _fresh_workspace(
        repo, archive, cache_dir, artifacts, artifact_dir, f"{task['key']}:reference"
    ) as fresh:
        if fresh is not None:
            workspace, python, env = fresh
            applied, changed_hunks = _apply_patch(
                workspace,
                dataset_root / task["reference_patch"],
                artifacts,
                artifact_dir,
                f"{task['key']}:reference",
            )
            reference_green = applied and _run_setup(
                repo, workspace, python, env, artifacts, artifact_dir, f"{task['key']}:reference"
            )
            if reference_green:
                reference_green = _run_evaluator(
                    evaluator, workspace, python, env, artifacts, artifact_dir, f"{task['key']}:reference"
                )
            changed_anchors = set().union(
                *(
                    changed_anchor_symbols(
                        workspace / anchor["path"],
                        [
                            (
                                workspace / candidate["path"],
                                candidate["path"],
                                candidate["symbol"],
                            )
                            for candidate in task["overlap_anchors"]
                        ],
                        changed_hunks.get(anchor["path"], []),
                    )
                    for anchor in task["overlap_anchors"]
                )
            )
        else:
            changed_anchors = set()
    return {
        "key": task["key"],
        "base_red": base_red,
        "reference_green": reference_green,
        "changed_anchors": sorted(changed_anchors),
    }


def _qualify_integration(
    repo: dict,
    corpus: dict,
    dataset_root: Path,
    archive: Path,
    cache_dir: Path,
    artifacts: dict[str, dict[str, str]],
    artifact_dir: Path,
) -> tuple[bool, bool]:
    integrated_green = False
    upstream_green = False
    with _fresh_workspace(
        repo, archive, cache_dir, artifacts, artifact_dir, "integrated"
    ) as fresh:
        if fresh is not None:
            workspace, python, env = fresh
            applied, _ = _apply_patch(
                workspace,
                dataset_root / corpus["integrated_reference_patch"],
                artifacts,
                artifact_dir,
                "integrated",
            )
            setup_green = applied and _run_setup(
                repo, workspace, python, env, artifacts, artifact_dir, "integrated"
            )
            evaluator_results = [
                _run_evaluator(
                    dataset_root / evaluator,
                    workspace,
                    python,
                    env,
                    artifacts,
                    artifact_dir,
                    f"integrated:{index}",
                )
                for index, evaluator in enumerate(corpus["evaluators"])
            ] if setup_green else []
            integrated_green = setup_green and all(evaluator_results)
            upstream_green = setup_green and _run_suite(
                repo, workspace, python, env, artifacts, artifact_dir, "integrated"
            )
    return integrated_green, upstream_green


def qualify_repository(repo: dict, corpus: dict, manifest_dir: Path, cache_dir: Path) -> dict:
    qualification_root = (cache_dir / "qualification").resolve()
    output_dir = (qualification_root / repo["key"]).resolve()
    if not output_dir.is_relative_to(qualification_root):
        raise ValueError("repository key escapes qualification output")
    shutil.rmtree(output_dir, ignore_errors=True)
    artifact_dir = output_dir / "artifacts"
    artifact_dir.mkdir(parents=True)
    artifacts: dict[str, dict[str, str]] = {}
    dataset_root = manifest_dir.resolve()
    archive = ensure_archive(repo, cache_dir)
    try:
        with tarfile.open(archive, "r:gz") as source:
            source.getmembers()
    except tarfile.TarError as error:
        raise ValueError("archive is unreadable") from error
    base_suite_green = _qualify_base_suite(
        repo, archive, cache_dir, artifacts, artifact_dir
    )
    tasks = [
        _qualify_task(
            repo, task, dataset_root, archive, cache_dir, artifacts, artifact_dir
        )
        for task in corpus["tasks"]
    ]
    integrated_green, upstream_green = _qualify_integration(
        repo, corpus, dataset_root, archive, cache_dir, artifacts, artifact_dir
    )
    changed_sets = [set(task["changed_anchors"]) for task in tasks]
    isolated_tasks = [
        task["key"]
        for index, task in enumerate(tasks)
        if not any(
            changed_sets[index] & other
            for other_index, other in enumerate(changed_sets)
            if other_index != index
        )
    ]
    return {
        "key": repo["key"],
        "base_suite_green": base_suite_green,
        "tasks": tasks,
        "integrated_green": integrated_green,
        "upstream_green": upstream_green,
        "isolated_tasks": isolated_tasks,
        "artifacts": artifacts,
    }


def _qualified(result: dict) -> bool:
    return (
        not result.get("error")
        and result["base_suite_green"]
        and all(task["base_red"] and task["reference_green"] for task in result["tasks"])
        and result["integrated_green"]
        and result["upstream_green"]
        and not result["isolated_tasks"]
    )


def _runner_prompts(corpus: dict, arm_dir: Path) -> tuple[list[tuple[dict, Path]], Path]:
    prompt_dir = arm_dir / "prompts"
    prompt_dir.mkdir(parents=True, exist_ok=True)
    tasks = []
    for task in corpus["tasks"]:
        prompt = prompt_dir / f"task-{task['key']}.prompt.txt"
        prompt.write_text(
            "You are working in a shared repository checkout. Other agents may edit concurrently.\n"
            f"{task['prompt']}\n"
            "Do not modify evaluator files.\n",
            encoding="utf-8",
        )
        tasks.append((task, prompt))
    final_prompt = prompt_dir / "final.prompt.txt"
    specifications = "\n\n".join(
        f"{task['key']}:\n{task['prompt']}" for task in corpus["tasks"]
    )
    final_prompt.write_text(
        "You are the integration reviewer for this repository.\n"
        f"{corpus['final_prompt']}\n\n"
        "Evaluator scripts have been injected into .statefulbench-evaluators. "
        "Run every evaluator and the upstream suite, then fix all failures.\n\n"
        f"Task specifications:\n{specifications}\n",
        encoding="utf-8",
    )
    return tasks, final_prompt


def _inject_evaluators(corpus: dict, dataset_root: Path, workspace: Path) -> list[Path]:
    injected = workspace / ".statefulbench-evaluators"
    paths = []
    for task in corpus["tasks"]:
        source = _canonical_evaluator_path(task, dataset_root)
        destination = injected / source.relative_to((dataset_root / "evaluators").resolve())
        destination.parent.mkdir(parents=True, exist_ok=True)
        destination.unlink(missing_ok=True)
        shutil.copyfile(source, destination)
        destination.chmod(0o444)
        paths.append(source)
    return paths


def _empty_run_result(repo: dict, arm: str, trial: int, error: str | None = None) -> dict:
    return {
        "repository": repo["key"],
        "arm": arm,
        "trial": trial,
        "cleared": False,
        "error": error,
        "arm_wall_time_s": 0.0,
        "tasks_wall_time_s": 0.0,
        "final_wall_time_s": 0.0,
        "total_tokens": 0,
        "total_tool_calls": 0,
        "post_suite_ok": False,
        "evaluators_ok": False,
        "upstream_suite_ok": False,
        "evaluator_results": [],
        "agents": [],
        "artifacts": {},
    }


def _write_json_atomically(path: Path, value: dict) -> None:
    temporary: Path | None = None
    try:
        with tempfile.NamedTemporaryFile(
            mode="w",
            encoding="utf-8",
            dir=path.parent,
            prefix=f".{path.name}.",
            suffix=".tmp",
            delete=False,
        ) as output:
            temporary = Path(output.name)
            json.dump(value, output, indent=2)
            output.write("\n")
            output.flush()
            os.fsync(output.fileno())
        os.replace(temporary, path)
    finally:
        if temporary is not None:
            temporary.unlink(missing_ok=True)


def _write_run_result(out_dir: Path, result: dict) -> None:
    result_dir = (
        out_dir
        / result["repository"]
        / result["arm"]
        / f"trial-{result['trial']}"
    )
    result_dir.mkdir(parents=True, exist_ok=True)
    _write_json_atomically(result_dir / "results.json", result)


def run_repo_arm(
    repo: dict,
    corpus: dict,
    manifest_dir: Path,
    cache_dir: Path,
    out_dir: Path,
    arm: str,
    cfg: RunConfig,
    *,
    launch=_LITE.launch_agent,
    server=_LITE.arm_stateful_server,
    workspace_factory=_fresh_workspace,
    archive_loader=ensure_archive,
    setup=_run_setup,
    evaluator=_run_evaluator,
    suite=_run_suite,
) -> dict:
    if arm not in {"sequential", "parallel-off", "parallel-on"}:
        raise ValueError(f"unknown arm: {arm}")
    trial = cfg.trial
    if arm == "parallel-on" and not cfg.stateful_binary:
        result = _empty_run_result(
            repo, arm, trial, "parallel-on requires a resolvable stateful binary"
        )
        _write_run_result(out_dir, result)
        return result

    arm_dir = out_dir / repo["key"] / arm / f"trial-{trial}"
    artifact_dir = arm_dir / "artifacts"
    artifact_dir.mkdir(parents=True, exist_ok=True)
    artifacts: dict[str, dict[str, str]] = {}
    tasks, final_prompt = _runner_prompts(corpus, arm_dir)
    agents: list[dict] = []
    task_started: float | None = None
    task_ended: float | None = None
    arm_started: float | None = None
    final_started: float | None = None
    final_ended: float | None = None
    error: str | None = None
    evaluators_ok = False
    upstream_suite_ok = False
    evaluator_results: list[dict[str, bool | str]] = []
    pending: list[tuple[AgentHandle, str]] = []


    def wait(handle: AgentHandle, kind: str) -> tuple[dict, float]:
        record, ended = _LITE._wait_agent(handle, arm_dir, kind, cfg)
        pending.remove((handle, kind))
        return record, ended

    try:
        archive = archive_loader(repo, cache_dir)
        with workspace_factory(
            repo, archive, cache_dir, artifacts, artifact_dir, f"run:{arm}:trial-{trial}"
        ) as materialized:
            if materialized is None:
                raise RuntimeError("unable to create benchmark workspace")
            workspace, python, env = materialized
            env = _repository_environment(repo, env)
            if not setup(repo, workspace, python, env, artifacts, artifact_dir, "run"):
                raise RuntimeError("repository setup failed")
            mode = "stateful" if arm == "parallel-on" else "no-state"
            server_context = server(arm_dir, cfg) if arm == "parallel-on" else nullcontext({})
            with server_context as runtime_env:
                runtime_cfg = replace(
                    cfg,
                    launch_env={name: value for name, value in env.items() if name != "HOME"},
                    stateful_runtime_env=runtime_env or None,
                )

                def start(task: dict, prompt: Path) -> AgentHandle:
                    nonlocal task_started, arm_started
                    handle = launch(arm_dir, workspace, task["key"], prompt, mode, runtime_cfg)
                    pending.append((handle, "task"))
                    started = getattr(handle, "started_monotonic", time.monotonic())
                    task_started = started if task_started is None else min(task_started, started)
                    arm_started = started if arm_started is None else min(arm_started, started)
                    return handle

                if arm == "sequential":
                    for task, prompt in tasks:
                        record, ended = wait(start(task, prompt), "task")
                        agents.append(record)
                        task_ended = ended
                else:
                    handles = []
                    for task, prompt in tasks:
                        handles.append(start(task, prompt))
                    for handle in handles:
                        record, ended = wait(handle, "task")
                        agents.append(record)
                        task_ended = ended if task_ended is None else max(task_ended, ended)

                evaluator_paths = _inject_evaluators(corpus, manifest_dir, workspace)
                evaluator_hashes = {path: _sha256(path) for path in evaluator_paths}
                final_handle = launch(arm_dir, workspace, "final", final_prompt, mode, runtime_cfg)
                pending.append((final_handle, "final"))
                final_started = getattr(final_handle, "started_monotonic", time.monotonic())
                arm_started = final_started if arm_started is None else min(arm_started, final_started)
                final_record, final_ended = wait(final_handle, "final")
                agents.append(final_record)

                if any(_sha256(path) != digest for path, digest in evaluator_hashes.items()):
                    raise RuntimeError("canonical evaluator changed during agent execution")
                for path, (task, _) in zip(evaluator_paths, tasks, strict=True):
                    evaluator_results.append(
                        {
                            "key": task["key"],
                            "ok": evaluator(
                                path,
                                workspace,
                                python,
                                env,
                                artifacts,
                                artifact_dir,
                                task["key"],
                            ),
                        }
                    )
                evaluators_ok = all(result["ok"] for result in evaluator_results)
                upstream_suite_ok = suite(
                    repo, workspace, python, env, artifacts, artifact_dir, "post-final"
                )
    except (OSError, RuntimeError, subprocess.SubprocessError, ValueError) as exc:
        error = str(exc)
    finally:
        for handle, kind in pending[:]:
            try:
                record, ended = wait(handle, kind)
            except (OSError, subprocess.SubprocessError):
                continue
            agents.append(record)
            if kind == "task":
                task_ended = ended if task_ended is None else max(task_ended, ended)
            else:
                final_ended = ended

    post_suite_ok = evaluators_ok and upstream_suite_ok
    tasks_wall_time_s = (
        0.0
        if task_started is None or task_ended is None
        else max(0.0, task_ended - task_started)
    )
    arm_end = final_ended if final_ended is not None else task_ended
    arm_wall_time_s = (
        0.0
        if arm_started is None or arm_end is None
        else max(0.0, arm_end - arm_started)
    )
    final_wall_time_s = (
        0.0
        if final_started is None or final_ended is None
        else max(0.0, final_ended - final_started)
    )
    result = {
        "repository": repo["key"],
        "arm": arm,
        "trial": trial,
        "cleared": (
            error is None
            and post_suite_ok
            and len(agents) == len(tasks) + 1
            and all(record["exit_code"] == 0 and not record["timed_out"] for record in agents)
        ),
        "error": error,
        "arm_wall_time_s": arm_wall_time_s,
        "tasks_wall_time_s": tasks_wall_time_s,
        "final_wall_time_s": final_wall_time_s,
        "total_tokens": sum(record["total_tokens"] for record in agents),
        "total_tool_calls": sum(record["tool_calls"] for record in agents),
        "post_suite_ok": post_suite_ok,
        "evaluators_ok": evaluators_ok,
        "upstream_suite_ok": upstream_suite_ok,
        "evaluator_results": evaluator_results,
        "agents": agents,
        "artifacts": artifacts,
    }
    _write_run_result(out_dir, result)
    return result



def _failure_reason(result: dict) -> str | None:
    if result["error"] is not None:
        return result["error"]
    failures = []
    for record in result.get("agents", []):
        agent = f"{record['kind']} agent {record['agent_id']}"
        if record["timed_out"]:
            failures.append(f"{agent} timed out")
        elif record["exit_code"] != 0:
            failures.append(f"{agent} exited with code {record['exit_code']}")
    if not result.get("evaluators_ok", True):
        failed = [
            status["key"]
            for status in result.get("evaluator_results", [])
            if not status["ok"]
        ]
        failures.append(
            f"evaluator failed: {', '.join(failed)}" if failed else "evaluator failed"
        )
    if not result.get("upstream_suite_ok", True):
        failures.append("upstream suite failed")
    if failures:
        return "; ".join(failures)
    return "run did not clear" if not result["cleared"] else None


def _report_row(result: dict) -> dict:
    return {
        "repo": result["repository"],
        "arm": result["arm"],
        "trial": result["trial"],
        "cleared": result["cleared"],
        "wall_time_s": result["arm_wall_time_s"],
        "tokens": result["total_tokens"],
        "tool_calls": result["total_tool_calls"],
        "error": _failure_reason(result),
    }


def build_run_summary(
    repositories: tuple[dict, ...] | list[dict],
    arms: list[str],
    trials: int,
    model: str,
    thinking: str,
    results: list[dict],
    generated_at: str,
) -> dict:
    rows = [_report_row(result) for result in results]
    aggregates = []
    for repository in repositories:
        key = repository["key"]
        for arm in arms:
            matching = [
                row for row in rows if row["repo"] == key and row["arm"] == arm
            ]
            failures = [
                {
                    "repo": row["repo"],
                    "arm": row["arm"],
                    "trial": row["trial"],
                    "error": row["error"],
                }
                for row in matching
                if not row["cleared"]
            ]
            present_trials = {row["trial"] for row in matching}
            failures.extend(
                {
                    "repo": key,
                    "arm": arm,
                    "trial": trial,
                    "error": "missing result",
                }
                for trial in range(1, trials + 1)
                if trial not in present_trials
            )
            original_rows = [
                result
                for result in results
                if result["repository"] == key and result["arm"] == arm
            ]
            aggregates.append(
                {
                    "repo": key,
                    "arm": arm,
                    "row_count": len(matching),
                    "cleared_count": sum(row["cleared"] for row in matching),
                    "wall_time_s": sum(row["wall_time_s"] for row in matching),
                    "tasks_wall_time_s": sum(
                        result["tasks_wall_time_s"] for result in original_rows
                    ),
                    "final_wall_time_s": sum(
                        result["final_wall_time_s"] for result in original_rows
                    ),
                    "tokens": sum(row["tokens"] for row in matching),
                    "tool_calls": sum(row["tool_calls"] for row in matching),
                    "failures": failures,
                }
            )
    return {
        "model": model,
        "thinking": thinking,
        "trials": trials,
        "repositories": [
            {
                "key": repository["key"],
                "source_sha": repository["commit"],
                "archive_sha256": repository["archive_sha256"],
            }
            for repository in repositories
        ],
        "generated_at": generated_at,
        "arms": rows,
        "aggregates": aggregates,
    }


def _table(results: list[dict]) -> str:
    lines = [
        "| repository | arm | trial | cleared | wall_time_s | tokens | tool_calls | error |",
        "| --- | --- | ---: | --- | ---: | ---: | ---: | --- |",
    ]
    for row in map(_report_row, results):
        error = "" if row["error"] is None else re.sub(r"\r\n?|\n", r"\\n", str(row["error"])).replace("|", "\\|")
        lines.append(
            "| {repo} | {arm} | {trial} | {cleared} | {wall:.3f} | {tokens} | {tools} | {error} |".format(
                repo=row["repo"],
                arm=row["arm"],
                trial=row["trial"],
                cleared=row["cleared"],
                wall=row["wall_time_s"],
                tokens=row["tokens"],
                tools=row["tool_calls"],
                error=error,
            )
        )
    return "\n".join(lines)


def _parse_repositories(value: str) -> list[str]:
    values = [item.strip() for item in value.split(",") if item.strip()]
    if not values:
        raise argparse.ArgumentTypeError("--repos must contain one or more repository keys")
    return values


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser()
    commands = parser.add_subparsers(dest="command", required=True)
    qualify = commands.add_parser(
        "qualify", help="qualify archived real-world reference corpora"
    )
    qualify.add_argument("--manifest", type=Path, required=True)
    qualify.add_argument("--cache", type=Path, required=True)
    qualify.add_argument("--repo", action="append")
    run = commands.add_parser("run", help="run real-world three-arm corpus")
    run.add_argument("--manifest", type=Path, required=True)
    run.add_argument("--cache", type=Path, required=True)
    run.add_argument("--out", type=Path, required=True)
    run.add_argument("--repos", type=_parse_repositories)
    run.add_argument("--arms", type=_LITE._parse_arms, default=_LITE._parse_arms("sequential,parallel-off,parallel-on"))
    run.add_argument("--trials", type=int, default=1)
    run.add_argument("--model", default="openai-codex/gpt-5.6-terra")
    run.add_argument("--thinking", default="high")
    run.add_argument("--omp-bin", default="omp")
    run.add_argument("--stateful-binary", default=shutil.which("stateful"))
    run.add_argument("--timeout-s", type=int, default=900)
    arguments = parser.parse_args(argv)

    if arguments.command == "run":
        if arguments.trials < 1:
            parser.error("--trials must be at least 1")
        if arguments.timeout_s < 1:
            parser.error("--timeout-s must be at least 1")
        if "parallel-on" in arguments.arms and not arguments.stateful_binary:
            parser.error("parallel-on requires a resolvable stateful binary; pass --stateful-binary")
    manifest = load_manifest(arguments.manifest)
    repositories = repo_entries(manifest)
    wanted = arguments.repos if arguments.command == "run" else arguments.repo
    if wanted:
        selected = tuple(repo for repo in repositories if repo["key"] in set(wanted))
        missing = sorted(set(wanted) - {repo["key"] for repo in selected})
        if missing:
            parser.error(f"unknown repository key: {', '.join(missing)}")
    else:
        selected = repositories

    if arguments.command == "run":
        cfg = RunConfig(
            tasks=10,
            timeout_s=arguments.timeout_s,
            model=arguments.model,
            thinking=arguments.thinking,
            omp_bin=arguments.omp_bin,
            stateful_binary=arguments.stateful_binary,
        )
        results = []
        for repo in selected:
            try:
                corpus = load_corpus(arguments.manifest.parent / repo["corpus"])
                if not _corpus_matches_repository(repo, corpus):
                    raise ValueError("corpus repository does not match manifest key")
            except (OSError, RuntimeError, ValueError, subprocess.SubprocessError) as error:
                for trial in range(1, arguments.trials + 1):
                    for arm in arguments.arms:
                        result = _empty_run_result(repo, arm, trial, str(error))
                        _write_run_result(arguments.out, result)
                        results.append(result)
                continue
            for trial in range(1, arguments.trials + 1):
                for arm in arguments.arms:
                    try:
                        result = run_repo_arm(
                            repo,
                            corpus,
                            arguments.manifest.parent,
                            arguments.cache,
                            arguments.out,
                            arm,
                            replace(cfg, trial=trial),
                        )
                    except (OSError, RuntimeError, ValueError, subprocess.SubprocessError) as error:
                        result = _empty_run_result(repo, arm, trial, str(error))
                    _write_run_result(arguments.out, result)
                    results.append(result)
        summary = build_run_summary(
            selected,
            arguments.arms,
            arguments.trials,
            cfg.model,
            cfg.thinking,
            results,
            time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()),
        )
        arguments.out.mkdir(parents=True, exist_ok=True)
        _write_json_atomically(arguments.out / "summary.json", summary)
        print(_table(results))
        return 0 if results and all(result["cleared"] for result in results) else 1

    results = []
    for repo in selected:
        try:
            corpus = load_corpus(arguments.manifest.parent / repo["corpus"])
            if not _corpus_matches_repository(repo, corpus):
                raise ValueError("corpus repository does not match manifest key")
            result = qualify_repository(repo, corpus, arguments.manifest.parent, arguments.cache)
        except (OSError, ValueError, subprocess.SubprocessError) as error:
            result = {
                "key": repo["key"],
                "error": str(error),
                "tasks": [],
                "base_suite_green": False,
                "integrated_green": False,
                "upstream_green": False,
                "isolated_tasks": [],
                "artifacts": {},
            }
        results.append(result)
    print(json.dumps({"repositories": results}, sort_keys=True))
    return 0 if all(_qualified(result) for result in results) else 1


if __name__ == "__main__":
    raise SystemExit(main())
