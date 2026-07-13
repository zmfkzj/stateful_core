from __future__ import annotations

import json
import secrets
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


@dataclass(frozen=True)
class ArmContainer:
    runtime: DockerRuntime
    container_id: str
    name: str
    workspace: Path
    runtime_dir: Path

    @property
    def home(self) -> str:
        return "/home/stateful"


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


def arm_container_command(
    runtime: DockerRuntime,
    *,
    name: str,
    workspace: Path,
    runtime_dir: Path,
    ownership_token: str | None = None,
) -> list[str]:
    workspace = workspace.resolve()
    runtime_dir = runtime_dir.resolve()
    return docker_command(
        runtime,
        "run",
        "-d",
        "--init",
        "--cap-add",
        "SYS_ADMIN",
        "--security-opt",
        "seccomp=unconfined",
        "--security-opt",
        "apparmor=unconfined",
        "--security-opt",
        "systempaths=unconfined",
        "--network",
        "bridge",
        "--workdir",
        "/workspace",
        "--name",
        name,
        "--label",
        f"statefulbench.arm-token={ownership_token or name}",
        "--mount",
        f"type=bind,source={workspace},target=/workspace",
        "--mount",
        f"type=bind,source={runtime_dir},target=/runtime",
        runtime.image_id,
        "sleep",
        "infinity",
    )


def start_arm_container(
    runtime: DockerRuntime,
    name: str,
    workspace: Path,
    runtime_dir: Path,
    *,
    runner=subprocess.run,
    timeout_s: float = 60,
) -> ArmContainer:
    token = secrets.token_hex(16)

    def fail(detail: str, cause: BaseException | None = None) -> None:
        inspected = runner(
            [
                runtime.binary,
                "inspect",
                "--format",
                "{{.Id}}\t{{ index .Config.Labels \"statefulbench.arm-token\" }}",
                name,
            ],
            capture_output=True,
            text=True,
            check=False,
            timeout=timeout_s,
        )
        identity = inspected.stdout.strip().split(maxsplit=1)
        if inspected.returncode == 0 and len(identity) == 2 and identity[1] == token:
            provisional = ArmContainer(
                runtime, identity[0], name, workspace.resolve(), runtime_dir.resolve()
            )
            try:
                remove_arm_container(provisional, runner=runner, timeout_s=timeout_s)
            except (OSError, RuntimeError, subprocess.SubprocessError) as cleanup_error:
                detail = f"{detail}; indeterminate container removal failed: {cleanup_error}"
        else:
            detail = f"{detail}; indeterminate container ownership could not be verified"
        error = RuntimeError(f"arm container start failed: {detail}")
        if cause is None:
            raise error
        raise error from cause

    try:
        completed = runner(
            arm_container_command(
                runtime,
                name=name,
                workspace=workspace,
                runtime_dir=runtime_dir,
                ownership_token=token,
            ),
            capture_output=True,
            text=True,
            check=False,
            timeout=timeout_s,
        )
    except (OSError, subprocess.SubprocessError) as error:
        fail(str(error), error)
    container_id = completed.stdout.strip()
    if completed.returncode != 0 or not container_id:
        fail(completed.stderr.strip() or "Docker did not return a container id")
    return ArmContainer(runtime, container_id, name, workspace.resolve(), runtime_dir.resolve())


def exec_in_container(
    container: ArmContainer,
    *inner: str,
    env: dict[str, str] | None = None,
    runner=subprocess.run,
    timeout_s: float = 60,
    check: bool = True,
) -> subprocess.CompletedProcess[str]:
    command = [container.runtime.binary, "exec", "--workdir", "/workspace"]
    for name, value in sorted((env or {}).items()):
        command.extend(("--env", f"{name}={value}"))
    completed = runner(
        [*command, container.container_id, *inner],
        capture_output=True,
        text=True,
        check=False,
        timeout=timeout_s,
    )
    if check and completed.returncode != 0:
        raise RuntimeError(f"arm container command failed: {completed.stderr.strip()}")
    return completed


def copy_to_container(
    container: ArmContainer,
    source: Path,
    destination: str,
    *,
    runner=subprocess.run,
    timeout_s: float = 60,
) -> None:
    completed = runner(
        [container.runtime.binary, "cp", str(source), f"{container.container_id}:{destination}"],
        capture_output=True,
        text=True,
        check=False,
        timeout=timeout_s,
    )
    if completed.returncode != 0:
        raise RuntimeError(f"arm container copy failed: {completed.stderr.strip()}")


def remove_arm_container(
    container: ArmContainer, *, runner=subprocess.run, timeout_s: float = 60
) -> None:
    completed = runner(
        [container.runtime.binary, "rm", "-f", container.container_id],
        capture_output=True,
        text=True,
        check=False,
        timeout=timeout_s,
    )
    if completed.returncode != 0:
        raise RuntimeError(f"arm container removal failed: {completed.stderr.strip()}")


def prepare_arm_runtime(
    container: ArmContainer,
    arm: str,
    *,
    credential_db: Path | None = None,
    stateful_binary: str = "stateful",
    runner=subprocess.run,
) -> dict[str, str]:
    if arm not in {"sequential", "parallel-off", "parallel-on"}:
        raise ValueError(f"unknown arm: {arm}")
    home = container.home
    env = {
        "HOME": home,
        "PI_CODING_AGENT_DIR": f"{home}/.omp/profiles/stateful/agent",
        "STATEFUL_HOME": f"{home}/.stateful",
        "XDG_CACHE_HOME": f"{home}/.cache",
        "XDG_CONFIG_HOME": f"{home}/.config",
        "PIP_CACHE_DIR": "/runtime/pip-cache",
        "TMPDIR": "/runtime/tmp",
        "STATEFUL_OMP_SANDBOX": "off",
    }
    exec_in_container(
        container,
        "mkdir",
        "-p",
        home,
        env["PI_CODING_AGENT_DIR"],
        env["STATEFUL_HOME"],
        env["XDG_CACHE_HOME"],
        env["XDG_CONFIG_HOME"],
        env["PIP_CACHE_DIR"],
        env["TMPDIR"],
        "/runtime/logs",
        "/runtime/pids",
        "/runtime/diagnostics",
        env=env,
        runner=runner,
    )
    if arm == "parallel-on":
        exec_in_container(
            container, stateful_binary, "install", "--agent", "omp", "--yes", env=env, runner=runner
        )
    if credential_db is not None:
        copy_to_container(
            container,
            credential_db,
            f"{env['PI_CODING_AGENT_DIR']}/agent.db",
            runner=runner,
        )
    if arm == "parallel-on":
        exec_in_container(
            container, stateful_binary, "enable", "--repo", "/workspace", env=env, runner=runner
        )
        exec_in_container(
            container,
            stateful_binary,
            "server",
            "start",
            "--coordination-mode",
            "enforcement",
            env=env,
            runner=runner,
        )
    return env


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
