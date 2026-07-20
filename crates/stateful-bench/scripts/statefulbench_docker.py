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
    server_platform: str = ""


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
    cleanup_error: str | None = None
    container_removed: bool = False
    pid: int | None = None
    pgid: int | None = None
    client_timed_out: bool = False
    client_term_attempted: bool = False
    client_term_result: str | None = None
    client_kill_attempted: bool = False
    client_kill_result: str | None = None
    inner_term_attempted: bool = False
    inner_term_returncode: int | None = None
    inner_term_error: str | None = None
    inner_kill_attempted: bool = False
    inner_kill_returncode: int | None = None
    inner_kill_error: str | None = None
    container_removal_attempted: bool = False
    container_removal_succeeded: bool = False

def resolve_binary(binary: str) -> str:
    resolved = shutil.which(binary)
    if resolved is None:
        raise RuntimeError(f"Docker binary is not executable: {binary}")
    return str(Path(resolved).resolve())


def inspect_runtime(
    docker_bin: str, image: str, *, runner=subprocess.run
) -> DockerRuntime:
    binary = resolve_binary(docker_bin)
    server = runner(
        [binary, "version", "--format", "{{.Server.Os}}/{{.Server.Arch}}"],
        capture_output=True,
        text=True,
        check=False,
    )
    server_platform = server.stdout.strip().lower()
    if server.returncode != 0 or server_platform.count("/") != 1:
        raise RuntimeError(f"Docker server platform inspection failed: {server.stderr.strip()}")
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
    platform = f"{row['Os']}/{row['Architecture']}"
    if platform != "linux/arm64":
        raise RuntimeError(f"Docker image platform must be linux/arm64, got {platform}")
    return DockerRuntime(
        binary=binary,
        image=image,
        image_id=row["Id"],
        repo_digests=tuple(sorted(row.get("RepoDigests") or ())),
        platform=platform,
        server_platform=server_platform,
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
        "--ulimit",
        "nofile=4096:4096",
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
def _agent_runtime_paths(container: ArmContainer, agent_id: str) -> tuple[Path, Path, Path]:
    if not agent_id or Path(agent_id).name != agent_id:
        raise ValueError("agent id must be a single path component")
    return (
        container.runtime_dir / "pids" / f"{agent_id}.json",
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
    pid_record, stdout, stderr = _agent_runtime_paths(container, agent_id)
    stdout.parent.mkdir(parents=True, exist_ok=True)
    prompt_destination = f"/runtime/prompts/{agent_id}.prompt.txt"
    copy(container, prompt_path, prompt_destination)
    inner = [
        "statefulbench-container-entry",
        f"/runtime/pids/{agent_id}.json",
        f"/runtime/logs/{stdout.name}",
        f"/runtime/logs/{stderr.name}",
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
    command = [container.runtime.binary, "exec", "-w", "/workspace"]
    for name, value in sorted(env.items()):
        command.extend(("--env", f"{name}={value}"))
    process = popen(
        [*command, container.container_id, *inner],
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
    )
    return DockerAgentHandle(
        popen=process,
        agent_id=agent_id,
        container_id=container.container_id,
        pid_record=pid_record,
        started_monotonic=time.monotonic(),
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

def _completion_from_channel(value: object) -> tuple[int, int, int] | None:
    if not isinstance(value, bytes):
        return None
    try:
        lines = value.decode("ascii").splitlines()
        completion = json.loads(lines[0]) if len(lines) == 1 else None
    except (UnicodeDecodeError, json.JSONDecodeError):
        return None
    if (
        type(completion) is not dict
        or set(completion) != {"pid", "pgid", "exit_code"}
    ):
        return None
    pid, pgid, exit_code = (
        completion["pid"],
        completion["pgid"],
        completion["exit_code"],
    )
    if (
        type(pid) is not int
        or type(pgid) is not int
        or type(exit_code) is not int
        or pid < 1
        or pgid < 1
    ):
        return None
    return pid, pgid, exit_code


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
    force_container_removal: bool = False,
) -> str | None:
    identity = _inner_identity(handle)
    errors: list[str] = []
    if identity is None:
        errors.append(f"missing or invalid inner pid record for {handle.agent_id}")
    else:
        pid, pgid = identity
        handle.pid, handle.pgid = pid, pgid
        for signal_name in ("TERM", "KILL"):
            if signal_name == "TERM":
                handle.inner_term_attempted = True
            else:
                handle.inner_kill_attempted = True
            try:
                completed = exec_in_container(
                    container,
                    "kill",
                    f"-{signal_name}",
                    f"-{pgid}",
                    runner=runner,
                    check=False,
                )
                if signal_name == "TERM":
                    handle.inner_term_returncode = completed.returncode
                else:
                    handle.inner_kill_returncode = completed.returncode
                if completed.returncode != 0 and _inner_process_exists(container, pid, runner=runner):
                    errors.append(f"inner process group {pgid} rejected {signal_name}")
                if _wait_for_inner_exit(container, pid, pgid, wait_s, runner=runner):
                    if not force_container_removal:
                        return None
            except (OSError, RuntimeError, subprocess.SubprocessError) as error:
                if signal_name == "TERM":
                    handle.inner_term_error = str(error)
                else:
                    handle.inner_kill_error = str(error)
                errors.append(f"inner process group {pgid} {signal_name} failed: {error}")
        if not force_container_removal:
            errors.append(f"inner process group {pgid} survived TERM/KILL escalation")
    if force_container_removal:
        errors.append("entry subreaper did not reap all descendants before deadline")
    if remove is None:
        remove = remove_arm_container
    handle.container_removal_attempted = True
    try:
        remove(container, runner=runner)
        handle.container_removed = True
        handle.container_removal_succeeded = True
    except (OSError, RuntimeError, subprocess.SubprocessError) as error:
        errors.append(f"arm container removal failed: {error}")
    handle.cleanup_error = "; ".join(errors)
    return handle.cleanup_error


def _reap_docker_client(handle: DockerAgentHandle) -> str | None:
    handle.client_term_attempted = True
    try:
        handle.popen.terminate()
        handle.client_term_result = "sent"
    except ProcessLookupError:
        handle.client_term_result = "not-running"
    except (OSError, subprocess.SubprocessError) as error:
        handle.client_term_result = f"error: {error}"
    try:
        handle.popen.wait(timeout=5)
        return None
    except subprocess.TimeoutExpired:
        handle.client_kill_attempted = True
        try:
            handle.popen.kill()
            handle.client_kill_result = "sent"
        except ProcessLookupError:
            handle.client_kill_result = "not-running"
        except (OSError, subprocess.SubprocessError) as error:
            handle.client_kill_result = f"error: {error}"
        try:
            handle.popen.wait(timeout=5)
            return None
        except subprocess.TimeoutExpired:
            handle.client_kill_result = "timed-out"
            return "docker exec client did not exit after TERM/KILL"
        except (OSError, subprocess.SubprocessError) as error:
            handle.client_kill_result = f"error: {error}"
            return f"docker exec client wait after KILL failed: {error}"
    except (OSError, subprocess.SubprocessError) as error:
        return f"docker exec client wait after TERM failed: {error}"

def _communicate_client(popen: subprocess.Popen, timeout: float) -> tuple[bytes, bytes]:
    stdout, stderr = popen.communicate(timeout=timeout)
    if not isinstance(stdout, bytes) or not isinstance(stderr, bytes):
        raise OSError("docker exec client did not return byte streams")
    return stdout, stderr


def _close_client_pipes(popen: subprocess.Popen) -> None:
    for stream in (popen.stdout, popen.stderr):
        if stream is not None:
            try:
                stream.close()
            except OSError:
                pass


def wait_agent(
    container: ArmContainer,
    handle: DockerAgentHandle,
    arm_dir: Path,
    kind: str,
    cfg: object,
    *,
    runner=subprocess.run,
) -> tuple[dict, float]:
    del arm_dir
    timed_out = False
    cleanup_error: str | None = None
    client_cleanup_error: str | None = None
    stdout = b""
    try:
        stdout, _ = _communicate_client(handle.popen, float(cfg.timeout_s))
        client_exit_code = handle.popen.returncode
    except subprocess.TimeoutExpired:
        timed_out = True
        client_exit_code = -9
        handle.client_timed_out = True
        client_cleanup_error = _reap_docker_client(handle)
        try:
            _communicate_client(handle.popen, 5)
        except (OSError, subprocess.SubprocessError) as error:
            client_cleanup_error = "; ".join(
                detail for detail in (client_cleanup_error, str(error)) if detail
            )
        finally:
            _close_client_pipes(handle.popen)
        cleanup_error = terminate_agent_group(
            container, handle, runner=runner, force_container_removal=True
        )
        exit_code = -9
    except (OSError, subprocess.SubprocessError) as error:
        client_exit_code = -1
        client_cleanup_error = str(error)
        _close_client_pipes(handle.popen)
        cleanup_error = terminate_agent_group(
            container, handle, runner=runner, force_container_removal=True
        )
        exit_code = -1
    else:
        _close_client_pipes(handle.popen)
        completion = _completion_from_channel(stdout) if client_exit_code == 0 else None
        if completion is None:
            reason = (
                f"docker exec exited {client_exit_code} without trusted completion"
                if client_exit_code != 0
                else "missing or invalid trusted completion record"
            )
            cleanup_error = terminate_agent_group(
                container, handle, runner=runner, force_container_removal=True
            )
            cleanup_error = "; ".join(
                error for error in (reason, cleanup_error) if error
            )
            exit_code = -1
        else:
            handle.pid, handle.pgid, exit_code = completion
    cleanup_error = "; ".join(
        dict.fromkeys(
            error
            for error in (cleanup_error, client_cleanup_error, handle.cleanup_error)
            if error
        )
    ) or None
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
            "exit_code": exit_code,
            "timed_out": timed_out,
            "pid": handle.pid,
            "pgid": handle.pgid,
            "client_timed_out": handle.client_timed_out,
            "container_removed": handle.container_removed,
            "cleanup": {
                "client": {
                    "term_attempted": handle.client_term_attempted,
                    "term_result": handle.client_term_result,
                    "kill_attempted": handle.client_kill_attempted,
                    "kill_result": handle.client_kill_result,
                },
                "inner": {
                    "term_attempted": handle.inner_term_attempted,
                    "term_returncode": handle.inner_term_returncode,
                    "term_error": handle.inner_term_error,
                    "kill_attempted": handle.inner_kill_attempted,
                    "kill_returncode": handle.inner_kill_returncode,
                    "kill_error": handle.inner_kill_error,
                },
                "container_removal": {
                    "attempted": handle.container_removal_attempted,
                    "succeeded": handle.container_removal_succeeded,
                },
            },
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
    exec_in_container(container, "rm", "-rf", env["STATEFUL_HOME"], env=env, runner=runner)
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
            "awareness",
            env=env,
            runner=runner,
        )
        runtime_record = json.loads(
            exec_in_container(
                container,
                "cat",
                f"{env['STATEFUL_HOME']}/runtime/server.json",
                env=env,
                runner=runner,
            ).stdout
        )
        if not isinstance(runtime_record, dict) or not all(
            isinstance(runtime_record.get(key), str) and runtime_record[key]
            for key in ("base_url", "token")
        ):
            raise RuntimeError("stateful server runtime identity is invalid")
        env["STATEFUL_SERVER_URL"] = runtime_record["base_url"]
        env["STATEFUL_SERVER_TOKEN"] = runtime_record["token"]
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
        "--ulimit",
        "nofile=4096:4096",
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
        f"STATEFULBENCH_SERVER_PLATFORM={runtime.server_platform}",
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
