from __future__ import annotations

import subprocess
import sys
from pathlib import Path
from types import SimpleNamespace

from conftest import load_script


class FakeClock:
    def __init__(self, start_ms: int = 1_000):
        self.current_ms = start_ms

    def now_ms(self) -> int:
        return self.current_ms

    def advance(self, delta_ms: int) -> None:
        self.current_ms += delta_ms


def base_args(tmp_path: Path, *, instance_id: str = "case-a") -> SimpleNamespace:
    return SimpleNamespace(
        benchmark_max_turns=3,
        condition_dir=str(tmp_path / "condition"),
        condition_id="stateful-on_subagent-off",
        instance_id=instance_id,
        model=None,
        subagent=False,
        timeout_seconds=60,
    )


def read_metadata(tmp_path: Path, instance_id: str = "case-a") -> dict:
    metadata_path = tmp_path / "condition" / instance_id / "instance.json"
    import json

    return json.loads(metadata_path.read_text(encoding="utf-8"))


def test_codex_metadata_records_agent_time_separately_from_elapsed_wrapper_time(tmp_path, monkeypatch):
    mod = load_script("programbench_codex_agent.py")
    clock = FakeClock()
    args = base_args(tmp_path)
    monkeypatch.setattr(mod, "now_ms", clock.now_ms)

    def fake_archive_workspace(_args, instance_dir):
        clock.advance(400)
        return instance_dir / "submission.tar.gz"

    def fake_run_agent(_args, _prompt):
        clock.advance(500)
        return subprocess.CompletedProcess(["codex"], 0, stdout="", stderr="")

    monkeypatch.setattr(mod, "archive_workspace", fake_archive_workspace)

    exit_code = mod.run_main(
        args,
        agent_name="codex-cli",
        exited_error_prefix="codex",
        token_usage_from_output=lambda _output: {},
        run_agent_func=fake_run_agent,
    )

    metadata = read_metadata(tmp_path)
    assert exit_code == 0
    assert metadata["agent_running_time_ms"] == 500
    assert metadata["running_time_ms"] == 900


def test_omp_metadata_records_only_omp_command_time_excluding_adapter_overhead(tmp_path, monkeypatch):
    mod = load_script("programbench_omp_agent.py")
    codex_support = sys.modules["programbench_codex_agent"]
    clock = FakeClock()
    args = base_args(tmp_path)
    args.container_id = "target-container"
    args.omp_bin = "/bin/omp"
    args.stateful = True
    args.stateful_binary = "/bin/stateful"
    args.thinking = None
    monkeypatch.setattr(codex_support, "now_ms", clock.now_ms)
    monkeypatch.setattr(mod, "now_ms", clock.now_ms, raising=False)

    def advance_by(delta_ms: int):
        def inner(*_args, **_kwargs):
            clock.advance(delta_ms)
        return inner

    def fake_run_omp_command(command, *, cwd, env, timeout_seconds):
        assert command[:2] == ["/bin/omp", "--cwd"]
        assert command[-2] == "-p"
        clock.advance(500)
        return subprocess.CompletedProcess(command, 0, stdout="", stderr="")

    def fake_archive_workspace(_args, instance_dir):
        clock.advance(400)
        return instance_dir / "submission.tar.gz"

    monkeypatch.setattr(mod, "copy_workspace_from_container", advance_by(50))
    monkeypatch.setattr(mod, "install_stateful_for_agent", advance_by(70))
    monkeypatch.setattr(mod, "enable_stateful_repo", advance_by(80))
    monkeypatch.setattr(mod, "seed_omp_auth_credentials", advance_by(30))
    monkeypatch.setattr(mod, "run_omp_command", fake_run_omp_command)
    monkeypatch.setattr(mod, "smoke_compile_airlock", advance_by(300))
    monkeypatch.setattr(mod, "stop_stateful_server", advance_by(60))
    monkeypatch.setattr(codex_support, "archive_workspace", fake_archive_workspace)

    exit_code = mod.run_main(
        args,
        agent_name="omp-cli",
        exited_error_prefix="omp",
        token_usage_from_output=lambda _output: {},
        run_agent_func=mod.run_agent,
    )

    metadata = read_metadata(tmp_path)
    assert exit_code == 0
    assert metadata["agent_running_time_ms"] == 500
    assert metadata["running_time_ms"] == 1_490


def test_metadata_omits_agent_time_when_error_happens_before_agent_starts(tmp_path, monkeypatch):
    mod = load_script("programbench_codex_agent.py")
    args = base_args(tmp_path, instance_id="case-before-agent")

    def fail_before_agent(_args):
        raise RuntimeError("prompt failed before agent")

    def fail_if_agent_runs(_args, _prompt):  # pragma: no cover - assertion protects the contract.
        raise AssertionError("agent should not start")

    monkeypatch.setattr(mod, "prompt_for_args", fail_before_agent)
    monkeypatch.setattr(mod, "archive_workspace", lambda _args, instance_dir: instance_dir / "submission.tar.gz")

    exit_code = mod.run_main(
        args,
        agent_name="codex-cli",
        exited_error_prefix="codex",
        token_usage_from_output=lambda _output: {},
        run_agent_func=fail_if_agent_runs,
    )

    metadata = read_metadata(tmp_path, "case-before-agent")
    assert exit_code == 1
    assert metadata["error"] == "prompt failed before agent"
    assert "agent_running_time_ms" not in metadata


def test_omp_docker_records_only_agent_command_time(tmp_path, monkeypatch):
    mod = load_script("programbench_omp_agent.py")
    clock = FakeClock()
    args = base_args(tmp_path)
    args.agent_docker_image = "omp-image"
    args.agent_docker_omp_bin = "omp"
    args.docker_bin = "docker"
    args.stateful = False
    args.stateful_binary = "/bin/stateful"
    args.thinking = None
    monkeypatch.setattr(mod, "now_ms", clock.now_ms)

    def advance_by(delta_ms: int):
        def inner(*_args, **_kwargs):
            clock.advance(delta_ms)
        return inner

    def fake_docker_exec(*_args, **_kwargs):
        return ["docker", "exec", "agent"]

    def fake_run(command, **_kwargs):
        clock.advance(500)
        return subprocess.CompletedProcess(command, 0, stdout="", stderr="")

    monkeypatch.setattr(mod, "start_agent_docker_container", lambda _args: "agent")
    monkeypatch.setattr(mod, "agent_docker_env", lambda *_args: {})
    monkeypatch.setattr(mod, "copy_airlock_to_agent_container", advance_by(100))
    monkeypatch.setattr(mod, "seed_omp_auth_credentials_into_container", advance_by(100))
    monkeypatch.setattr(mod, "docker_agent_exec_command", fake_docker_exec)
    monkeypatch.setattr(mod.subprocess, "run", fake_run)
    monkeypatch.setattr(mod, "copy_agent_workspace_to_airlock", advance_by(200))
    monkeypatch.setattr(mod, "smoke_compile_airlock", advance_by(300))
    monkeypatch.setattr(mod, "remove_agent_docker_container", advance_by(100))

    result = mod.run_agent(args, "prompt")

    assert result.returncode == 0
    assert args.agent_running_time_ms == 500
