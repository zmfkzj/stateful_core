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
