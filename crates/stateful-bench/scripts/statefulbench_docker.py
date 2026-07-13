from __future__ import annotations

import json
import importlib.util

import secrets
import shlex
import shutil
import subprocess
import time
from dataclasses import dataclass
from pathlib import Path, PurePosixPath

DIAGNOSTIC_PHASES = (
    "initialized",
    "before-tasks",
    "after-tasks",
    "after-final",
    "after-grading",
    "before-remove",
)



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



@dataclass
class DockerAgentHandle:
    popen: subprocess.Popen
    agent_id: str
    container_id: str
    pid_record: Path
    started_monotonic: float
    exit_record: Path | None = None
    cleanup_error: str | None = None
    container_removed: bool = False

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


def diagnostic_artifact_path(phase: str) -> str:
    if phase not in DIAGNOSTIC_PHASES:
        raise ValueError(f"unknown diagnostic phase: {phase}")
    return f"runtime/diagnostics/{phase}.json"


def _relative_diagnostic_path(value: object) -> bool:
    if type(value) is not str:
        return False
    path = PurePosixPath(value)
    return not path.is_absolute() and value not in {"", "."} and ".." not in path.parts


def _sanitized_home_snapshot(
    snapshot: object, phase: str, container: ArmContainer
) -> bool:
    if (
        type(snapshot) is not dict
        or snapshot.get("phase") != phase
        or snapshot.get("schema_version") != 1
        or snapshot.get("home") != container.home
        or snapshot.get("per_agent_home_tree") is not False
        or not isinstance(snapshot.get("files"), list)
        or not isinstance(snapshot.get("databases"), dict)
        or not isinstance(snapshot.get("lock_files"), list)
        or not isinstance(snapshot.get("processes"), list)
    ):
        return False
    if not all(
        type(record) is dict
        and _relative_diagnostic_path(record.get("path"))
        and type(record.get("type")) is str
        and type(record.get("size")) is int
        and type(record.get("mtime_ns")) is int
        for record in snapshot["files"]
    ):
        return False
    if not all(_relative_diagnostic_path(path) for path in snapshot["databases"]):
        return False
    if not all(_relative_diagnostic_path(path) for path in snapshot["lock_files"]):
        return False

    def no_absolute_value(value: object) -> bool:
        if type(value) is str:
            return not value.startswith("/")
        if isinstance(value, list):
            return all(no_absolute_value(item) for item in value)
        if isinstance(value, dict):
            return all(no_absolute_value(item) for item in value.values())
        return True

    return all(
        no_absolute_value(value)
        for key, value in snapshot.items()
        if key != "home"
    )


def capture_home_snapshot(
    container: ArmContainer,
    phase: str,
    *,
    runner=subprocess.run,
) -> dict:
    relative = diagnostic_artifact_path(phase)
    output = container.runtime_dir.parent / relative
    exec_in_container(
        container,
        "/usr/local/bin/statefulbench-container-diagnostics",
        "--home",
        container.home,
        "--phase",
        phase,
        "--output",
        f"/{relative}",
        runner=runner,
    )
    try:
        if output.is_symlink():
            raise ValueError("diagnostic artifact must not be a symlink")
        encoded = output.read_text(encoding="utf-8")
        snapshot = json.loads(encoded)
    except (OSError, json.JSONDecodeError, ValueError) as error:
        raise RuntimeError(f"diagnostic capture failed for {phase}") from error
    decoded = json.dumps(snapshot, sort_keys=True, separators=(",", ":"))
    if any(
        str(path.resolve()) in decoded
        for path in (container.runtime_dir, container.workspace)
    ):
        raise RuntimeError(f"diagnostic capture leaked host path for {phase}")
    if not _sanitized_home_snapshot(snapshot, phase, container):
        raise RuntimeError(f"diagnostic capture malformed for {phase}")
    return snapshot

def inspect_arm_container(
    container: ArmContainer, *, runner=subprocess.run, timeout_s: float = 60
) -> dict:
    completed = runner(
        [
            container.runtime.binary,
            "inspect",
            "--format",
            "{{json .State}}",
            container.container_id,
        ],
        capture_output=True,
        text=True,
        check=False,
        timeout=timeout_s,
    )
    if completed.returncode != 0:
        raise RuntimeError(f"arm container inspection failed: {completed.stderr.strip()}")
    try:
        state = json.loads(completed.stdout)
    except json.JSONDecodeError as error:
        raise RuntimeError("arm container inspection returned malformed state") from error
    if (
        type(state) is not dict
        or type(state.get("Status")) is not str
        or type(state.get("Pid")) is not int
        or type(state.get("StartedAt")) is not str
        or type(state.get("FinishedAt")) is not str
    ):
        raise RuntimeError("arm container inspection returned unsafe state")
    return {
        "id": container.container_id,
        "image_id": container.runtime.image_id,
        "state": {
            "status": state["Status"],
            "pid": state["Pid"],
            "started_at": state["StartedAt"],
            "finished_at": state["FinishedAt"],
        },
    }


def _agent_runtime_paths(container: ArmContainer, agent_id: str) -> tuple[Path, Path, Path, Path]:
    if not agent_id or Path(agent_id).name != agent_id:
        raise ValueError("agent id must be a single path component")
    pid = container.runtime_dir / "pids" / f"{agent_id}.json"
    return (
        pid,
        pid.with_suffix(".exit"),
        container.runtime_dir / "logs" / f"{agent_id}.stdout.log",
        container.runtime_dir / "logs" / f"{agent_id}.stderr.log",
    )


def launch_agent(
    container: ArmContainer,
    arm_dir: Path,
    agent_id: str,
    prompt_path: Path,
    cfg: object,
    env: dict[str, str],
    *,
    popen=subprocess.Popen,
    copy=copy_to_container,
) -> DockerAgentHandle:
    del arm_dir
    pid_record, exit_record, stdout, stderr = _agent_runtime_paths(container, agent_id)
    stdout.parent.mkdir(parents=True, exist_ok=True)
    prompt_destination = f"/runtime/prompts/{agent_id}.prompt.txt"
    copy(container, prompt_path, prompt_destination)
    inner = [
        "statefulbench-container-entry",
        f"/runtime/pids/{agent_id}.json",
        str(cfg.omp_bin),
        "--cwd",
        "/workspace",
        "--mode",
        "json",
        "--model",
        str(cfg.model),
        "--thinking",
        str(cfg.thinking),
        "--approval-mode",
        "yolo",
        "--no-title",
        f"@{prompt_destination}",
    ]
    shell = (
        f"{shlex.join(inner)} >{shlex.quote('/runtime/logs/' + stdout.name)} "
        f"2>{shlex.quote('/runtime/logs/' + stderr.name)}; status=$?; "
        f"printf '%s\\n' \"$status\" >{shlex.quote('/runtime/pids/' + exit_record.name)}; exit \"$status\""
    )
    command = [container.runtime.binary, "exec", "-d", "-w", "/workspace"]
    for name, value in sorted(env.items()):
        command.extend(("--env", f"{name}={value}"))
    process = popen(
        [*command, container.container_id, "sh", "-c", shell],
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    )
    return DockerAgentHandle(
        popen=process,
        agent_id=agent_id,
        container_id=container.container_id,
        pid_record=pid_record,
        started_monotonic=time.monotonic(),
        exit_record=exit_record,
    )


def _inner_identity(handle: DockerAgentHandle) -> tuple[int, int] | None:
    try:
        value = json.loads(handle.pid_record.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError):
        return None
    if type(value) is not dict:
        return None
    pid, pgid = value.get("pid"), value.get("pgid")
    if type(pid) is not int or type(pgid) is not int or pid < 1 or pgid < 1:
        return None
    return pid, pgid


def _inner_process_exists(
    container: ArmContainer, pid: int, *, runner=subprocess.run
) -> bool:
    return (
        exec_in_container(
            container, "test", "-e", f"/proc/{pid}", runner=runner, check=False
        ).returncode
        == 0
    )


def _inner_group_exists(
    container: ArmContainer, pgid: int, *, runner=subprocess.run
) -> bool:
    return (
        exec_in_container(
            container, "kill", "-0", f"-{pgid}", runner=runner, check=False
        ).returncode
        == 0
    )


def _wait_for_inner_exit(
    container: ArmContainer, pid: int, pgid: int, wait_s: float, *, runner=subprocess.run
) -> bool:
    deadline = time.monotonic() + wait_s
    while True:
        if not _inner_process_exists(container, pid, runner=runner) and not _inner_group_exists(
            container, pgid, runner=runner
        ):
            return True
        if time.monotonic() >= deadline:
            return False
        time.sleep(min(0.1, max(0.0, deadline - time.monotonic())))


def terminate_agent_group(
    container: ArmContainer,
    handle: DockerAgentHandle,
    *,
    runner=subprocess.run,
    wait_s: float = 5,
    remove=None,
) -> str | None:
    identity = _inner_identity(handle)
    errors: list[str] = []
    if identity is None:
        errors.append(f"missing or invalid inner pid record for {handle.agent_id}")
    else:
        pid, pgid = identity
        for signal_name in ("TERM", "KILL"):
            completed = exec_in_container(
                container,
                "kill",
                f"-{signal_name}",
                f"-{pgid}",
                runner=runner,
                check=False,
            )
            if completed.returncode != 0 and _inner_process_exists(container, pid, runner=runner):
                errors.append(f"inner process group {pgid} rejected {signal_name}")
            if _wait_for_inner_exit(container, pid, pgid, wait_s, runner=runner):
                return None
        errors.append(f"inner process group {pgid} survived TERM/KILL escalation")
    if remove is None:
        remove = remove_arm_container
    try:
        remove(container, runner=runner)
        handle.container_removed = True
    except (OSError, RuntimeError, subprocess.SubprocessError) as error:
        errors.append(f"arm container removal failed: {error}")
    handle.cleanup_error = "; ".join(errors)
    return handle.cleanup_error


def _exit_code(handle: DockerAgentHandle) -> int | None:
    if handle.exit_record is None:
        return None
    try:
        return int(handle.exit_record.read_text(encoding="utf-8").strip())
    except (OSError, ValueError):
        return None

def _reap_docker_client(popen: subprocess.Popen) -> str | None:
    try:
        popen.terminate()
    except ProcessLookupError:
        pass
    try:
        popen.wait(timeout=5)
        return None
    except subprocess.TimeoutExpired:
        try:
            popen.kill()
        except ProcessLookupError:
            pass
        try:
            popen.wait(timeout=5)
            return None
        except subprocess.TimeoutExpired:
            return "detached docker exec client did not exit after TERM/KILL"


def wait_agent(
    container: ArmContainer,
    handle: DockerAgentHandle,
    arm_dir: Path,
    kind: str,
    cfg: object,
    *,
    runner=subprocess.run,
) -> tuple[dict, float]:
    timed_out = False
    cleanup_error: str | None = None
    launch_code: int | None = None
    try:
        launch_code = handle.popen.wait(timeout=min(5, float(cfg.timeout_s)))
    except subprocess.TimeoutExpired:
        timed_out = True
        client_cleanup_error = _reap_docker_client(handle.popen)
        cleanup_error = terminate_agent_group(container, handle, runner=runner)
        if client_cleanup_error is not None:
            cleanup_error = "; ".join(
                error for error in (cleanup_error, client_cleanup_error) if error
            )
    identity = _inner_identity(handle)
    deadline = handle.started_monotonic + float(cfg.timeout_s)
    exit_code = launch_code if launch_code not in (None, 0) else None
    while not timed_out and exit_code is None and identity is None:
        if time.monotonic() >= deadline:
            timed_out = True
            cleanup_error = terminate_agent_group(container, handle, runner=runner)
            break
        time.sleep(min(0.1, max(0.0, deadline - time.monotonic())))
        identity = _inner_identity(handle)
    if not timed_out and exit_code is None and identity is not None:
        pid, pgid = identity
        while _inner_process_exists(container, pid, runner=runner) or _inner_group_exists(
            container, pgid, runner=runner
        ):
            if time.monotonic() >= deadline:
                timed_out = True
                cleanup_error = terminate_agent_group(container, handle, runner=runner)
                break
            time.sleep(min(0.1, max(0.0, deadline - time.monotonic())))
        if not timed_out:
            while (exit_code := _exit_code(handle)) is None and time.monotonic() < deadline:
                time.sleep(min(0.1, max(0.0, deadline - time.monotonic())))
            if exit_code is None:
                cleanup_error = f"missing exit record for {handle.agent_id}"
    if timed_out:
        exit_code = -9
    if handle.cleanup_error:
        cleanup_error = handle.cleanup_error
    ended = time.monotonic()
    usage = (
        cfg.usage_from_log(container.runtime_dir / "logs" / f"{handle.agent_id}.stdout.log")
        if hasattr(cfg, "usage_from_log")
        else {"total_tokens": 0, "tool_calls": 0}
    )
    return (
        {
            "agent_id": handle.agent_id,
            "kind": kind,
            "exit_code": -1 if exit_code is None else exit_code,
            "timed_out": timed_out,
            "cleanup_error": cleanup_error,
            "wall_time_s": max(0.0, ended - handle.started_monotonic),
            **usage,
        },
        ended,
    )


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
    omp_binary: str = "/usr/local/bin/omp",
    stateful_binary: str = "/usr/local/bin/stateful",
    activate_stateful: bool = True,
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
        "/runtime/prompts",
        "/runtime/diagnostics",
        env=env,
        runner=runner,
    )
    for binary in (omp_binary, stateful_binary):
        exec_in_container(container, "test", "-x", binary, env=env, runner=runner)
    if arm == "parallel-on" and activate_stateful:
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
    if arm == "parallel-on" and activate_stateful:
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
