from __future__ import annotations

import json
import shutil
import subprocess
from dataclasses import dataclass
from pathlib import Path


@dataclass(frozen=True)
class DockerRuntime:
    binary: str
    image: str
    image_id: str
    repo_digests: tuple[str, ...]
    platform: str


def resolve_binary(binary: str) -> str:
    resolved = shutil.which(binary)
    if resolved is None:
        raise RuntimeError(f"Docker binary is not executable: {binary}")
    return str(Path(resolved).resolve())


def inspect_runtime(
    docker_bin: str, image: str, *, runner=subprocess.run
) -> DockerRuntime:
    binary = resolve_binary(docker_bin)
    completed = runner(
        [binary, "image", "inspect", image],
        capture_output=True,
        text=True,
        check=False,
    )
    if completed.returncode != 0:
        raise RuntimeError(f"Docker image inspection failed: {completed.stderr.strip()}")
    rows = json.loads(completed.stdout)
    if len(rows) != 1 or rows[0].get("Os") != "linux":
        raise RuntimeError("Docker image must resolve to exactly one Linux image")
    row = rows[0]
    return DockerRuntime(
        binary=binary,
        image=image,
        image_id=row["Id"],
        repo_digests=tuple(sorted(row.get("RepoDigests") or ())),
        platform=f"{row['Os']}/{row['Architecture']}",
    )


def docker_command(runtime: DockerRuntime, subcommand: str, *args: str) -> list[str]:
    command = [runtime.binary, subcommand]
    if subcommand in {"build", "create", "run"}:
        command.append(f"--platform={runtime.platform}")
    return [*command, *args]


def qualification_command(
    runtime: DockerRuntime,
    repo_root: Path,
    manifest: Path,
    cache: Path,
    repositories: tuple[str, ...],
) -> list[str]:
    root = repo_root.resolve()
    manifest_path = manifest.resolve()
    cache_path = cache.resolve()
    if not manifest_path.is_relative_to(root):
        raise ValueError("manifest must be inside the repository root")
    if cache_path.is_relative_to(root):
        raise ValueError("cache must be outside the read-only repository root")
    container_manifest = Path("/benchmark") / manifest_path.relative_to(root)
    command = docker_command(
        runtime,
        "run",
        "--rm",
        "--mount",
        f"type=bind,source={root},target=/benchmark,readonly",
        "--mount",
        f"type=bind,source={cache_path},target=/cache",
        "--env",
        "STATEFULBENCH_DOCKER_INNER=qualification",
        "--env",
        f"STATEFULBENCH_IMAGE_ID={runtime.image_id}",
        "--env",
        f"STATEFULBENCH_IMAGE_PLATFORM={runtime.platform}",
        "--env",
        f"STATEFULBENCH_IMAGE_REPO_DIGESTS={json.dumps(runtime.repo_digests)}",
        "--env",
        "PYTHONDONTWRITEBYTECODE=1",
        runtime.image_id,
        "python3",
        "/benchmark/crates/stateful-bench/scripts/statefulbench_realworld.py",
        "qualify",
        "--manifest",
        str(container_manifest),
        "--cache",
        "/cache",
        "--docker-image",
        runtime.image,
        "--docker-bin",
        "docker",
    )
    for repository in repositories:
        command.extend(("--repo", repository))
    return command


def run_qualification_container(
    runtime: DockerRuntime,
    repo_root: Path,
    manifest: Path,
    cache: Path,
    repositories: tuple[str, ...],
    *,
    runner=subprocess.run,
) -> int:
    completed = runner(
        qualification_command(runtime, repo_root, manifest, cache, repositories),
        check=False,
    )
    return completed.returncode
