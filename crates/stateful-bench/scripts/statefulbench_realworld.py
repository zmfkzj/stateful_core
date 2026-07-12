from __future__ import annotations

import argparse
import ast
import contextlib
import hashlib
import json
import os
import re
import tarfile
import shutil
import subprocess
import sys
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


def _validate_repository(entry: object, manifest_dir: Path, keys: set[str]) -> None:
    if type(entry) is not dict:
        raise ValueError("repository entry must be an object")
    if set(entry) != _REPOSITORY_FIELDS:
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
    _require_dataset_path(entry, "evaluator", dataset_root)
    _require_dataset_path(entry, "reference_patch", dataset_root)

    anchors = entry["overlap_anchors"]
    if type(anchors) is not list or not anchors:
        raise ValueError("overlap_anchors must be a non-empty array")
    pairs: set[tuple[str, str]] = set()
    for anchor in anchors:
        if type(anchor) is not dict or set(anchor) != _ANCHOR_FIELDS:
            raise ValueError("overlap_anchors entries must contain path and symbol")
        pairs.add((_require_string(anchor, "path"), _require_string(anchor, "symbol")))
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
        _require_dataset_path({"evaluator": evaluator}, "evaluator", dataset_root)

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


def _sanitized_environment(venv: Path | None = None) -> dict[str, str]:
    env = dict(os.environ)
    for name in ("PYTHONHOME", "PYTHONPATH", "PYTHONSTARTUP", "PYTHONUSERBASE"):
        env.pop(name, None)
    if venv is None:
        env.pop("VIRTUAL_ENV", None)
        env["PATH"] = os.defpath
    else:
        env["VIRTUAL_ENV"] = str(venv)
        env["PATH"] = f"{venv / 'bin'}{os.pathsep}{os.defpath}"
    return env


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
            env=_sanitized_environment(),
        )
        python = venv / "bin" / "python"
        yield (
            (workspace, python, _sanitized_environment(venv))
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
    source: Path, anchors: list[tuple[Path, str]], hunks: list[tuple[int, int]]
) -> set[str]:
    try:
        tree = ast.parse(source.read_text(encoding="utf-8"))
    except (OSError, SyntaxError):
        return set()
    module = source.with_suffix("").name
    ranges: dict[str, tuple[int, int]] = {}

    class Symbols(ast.NodeVisitor):
        def __init__(self) -> None:
            self.scope: list[str] = []

        def _record(self, name: str, node: ast.AST) -> None:
            ranges[".".join((module, *self.scope, name))] = (
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
    for anchor_source, symbol in anchors:
        if anchor_source != source or symbol not in ranges:
            continue
        first, last = ranges[symbol]
        if any(count and start <= last and start + count - 1 >= first for start, count in hunks):
            changed.add(f"{source.name}:{symbol}")
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
        ["git", "diff", "--cached", "--unified=0"],
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
                            (workspace / candidate["path"], candidate["symbol"])
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


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser()
    commands = parser.add_subparsers(dest="command", required=True)
    qualify = commands.add_parser(
        "qualify", help="qualify archived real-world reference corpora"
    )
    qualify.add_argument("--manifest", type=Path, required=True)
    qualify.add_argument("--cache", type=Path, required=True)
    qualify.add_argument("--repo", action="append")
    arguments = parser.parse_args(argv)

    manifest = load_manifest(arguments.manifest)
    repositories = repo_entries(manifest)
    if arguments.repo:
        selected = tuple(
            repo for repo in repositories if repo["key"] in set(arguments.repo)
        )
        missing = sorted(set(arguments.repo) - {repo["key"] for repo in selected})
        if missing:
            parser.error(f"unknown repository key: {', '.join(missing)}")
    else:
        selected = repositories

    results = []
    for repo in selected:
        try:
            corpus = load_corpus(arguments.manifest.parent / repo["corpus"])
            if corpus["repository"] != repo["key"]:
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
