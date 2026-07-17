from __future__ import annotations

import io
import json
import subprocess
import sys
from pathlib import Path

import pytest

from conftest import arg_after, load_script


def test_codex_pair_agent_help_omits_stateful_session_arguments():
    script = Path(__file__).resolve().parents[1] / "codex_pair_agent.py"
    output = subprocess.run([sys.executable, str(script), "--help"], text=True, stdout=subprocess.PIPE, stderr=subprocess.PIPE, check=True)
    stdout = output.stdout
    assert "--session-id" not in stdout
    for flag in [
        "--workspace",
        "--benchmark-model",
        "--benchmark-reasoning-effort",
        "--benchmark-model-context-window",
        "--benchmark-max-turns",
        "--subagent-min-count",
        "--enable-native-subagent",
        "--disable-bundled-skills",
        "--stateful-integration",
        "--max-resumes",
    ]:
        assert flag in stdout


def test_codex_pair_agent_command_prompt_and_environment():
    mod = load_script("codex_pair_agent.py")
    default = mod.codex_command(Path("/tmp/workspace"), "no-state", stateful_binary="/tmp/stateful", base_env={})
    nested = mod.codex_command(Path("/tmp/workspace"), "no-state", stateful_binary="/tmp/stateful", base_env={"STATEFUL_NESTED_CODEX_HOME_ROOT": "/repo/target/nested-codex-homes"})
    assert arg_after(default, "--sandbox") == "workspace-write"
    assert "sandbox_workspace_write.network_access=true" in default
    assert arg_after(nested, "--sandbox") == "danger-full-access"
    assert "sandbox_workspace_write.network_access=true" not in nested

    command = mod.codex_command(
        Path("/tmp/workspace"),
        "stateful",
        stateful_binary="/tmp/stateful",
        benchmark_model="gpt-5.5",
        benchmark_reasoning_effort="xhigh",
        benchmark_model_context_window=256000,
        enable_native_subagent=True,
        disable_bundled_skills=True,
        stateful_integration="hooks-only",
        base_env={"STATEFUL_NESTED_CODEX_HOME_ROOT": "/repo/target/nested-codex-homes"},
    )
    assert arg_after(command, "--model") == "gpt-5.5"
    assert "model_reasoning_effort=\"xhigh\"" in command
    assert "model_context_window=256000" in command
    assert "features.multi_agent=true" in command
    assert "skills.bundled.enabled=false" in command

    assert mod.native_subagent_prompt_instruction(False) == ""
    prompt = mod.native_subagent_prompt_instruction(True, 3)
    assert "Native Codex subagent requirements" in prompt
    assert "MUST use native Codex subagents" in prompt
    assert "Spawn at least 3 native subagents" in prompt
    assert "Use all 3 native subagents for repository editing" in prompt
    assert "Wait for each spawned subagent" in prompt

    env = mod.codex_environment(
        task_path=Path("/repo/runs/pair one/workspace/.stateful_bench/task-a.json"),
        workspace=Path("/repo/runs/pair one/workspace"),
        base_env={"PATH": "/bin", "STATEFUL_NESTED_CODEX_HOME_ROOT": "/repo/target/nested-codex-homes"},
    )
    assert env["HOME"].startswith("/repo/target/nested-codex-homes/pair-one/task-a/")
    assert env["HOME"].endswith("/home")
    assert env["CODEX_HOME"] == f"{env['HOME']}/.codex"
    assert env["XDG_CONFIG_HOME"] == f"{env['HOME']}/.config"
    assert env["XDG_CACHE_HOME"] == f"{env['HOME']}/.cache"
    assert env["PATH"] == "/bin"


def test_codex_pair_agent_stateful_config_modes(tmp_path):
    mod = load_script("codex_pair_agent.py")
    source_home = tmp_path / "source-home"
    source_config = source_home / ".codex/config.toml"
    source_config.parent.mkdir(parents=True)
    source_config.write_text('''model_provider = "codex-lb"

[model_providers.codex-lb]
base_url = "http://127.0.0.1:2455/backend-api/codex"
wire_api = "responses"
websocket = true
websocker = true

[features]
goals = true
websocket = true
websocker = true

[mcp_servers.stateful]
command = "stale-stateful"
''')
    workspace = tmp_path / "runs/pair-one/workspace"
    task_path = workspace / ".stateful_bench/task-a.json"
    source_env = {"PATH": "/bin", "HOME": str(source_home), "STATEFUL_NESTED_CODEX_HOME_ROOT": str(tmp_path / "nested-codex-homes")}

    env = mod.codex_environment(task_path, workspace, source_env)
    mod.prepare_codex_environment(env, source_env=source_env, enable_stateful=True, stateful_binary="/tmp/stateful")
    codex_home = Path(env["CODEX_HOME"])
    config = (codex_home / "config.toml").read_text()
    skill = (codex_home / "skills/stateful-command-policy/SKILL.md").read_text()
    assert "model_provider = \"codex-lb\"" in config
    assert "base_url = \"http://127.0.0.1:2455/backend-api/codex\"" in config
    assert config.count("[features]") == 1
    assert "goals = true" not in config
    assert "websocket = true" not in config
    assert "websocker = true" not in config
    assert "stale-stateful" not in config
    assert "[mcp_servers.stateful]" not in config
    assert "STATEFUL_SESSION_ID" not in config
    assert "[[hooks.SessionStart]]" in config
    assert "[[hooks.PreToolUse]]" in config
    assert "[[hooks.Stop]]" in config
    assert "name: stateful-command-policy" in skill
    assert "state_reservation_declare" in skill
    assert "state_claim_acquire" in skill
    assert "benchmark-contamination" not in skill
    assert not (codex_home / "auth.json").exists()

    hooks_env = mod.codex_environment(task_path=tmp_path / "hooks/workspace/.stateful_bench/task-a.json", workspace=tmp_path / "hooks/workspace", base_env={"PATH": "/bin", "STATEFUL_NESTED_CODEX_HOME_ROOT": str(tmp_path / "nested-codex-homes")})
    mod.prepare_codex_environment(hooks_env, source_env={"PATH": "/bin", "STATEFUL_NESTED_CODEX_HOME_ROOT": str(tmp_path / "nested-codex-homes")}, enable_stateful=True, stateful_binary="/tmp/stateful", stateful_integration="hooks-only")
    hooks_home = Path(hooks_env["CODEX_HOME"])
    hooks_config = (hooks_home / "config.toml").read_text()
    assert "[mcp_servers.stateful]" not in hooks_config
    assert "hooks = true" in hooks_config
    assert "/tmp/stateful hook codex session-start" in hooks_config
    assert not (hooks_home / "skills/stateful-command-policy/SKILL.md").exists()

    no_state_env = mod.codex_environment(task_path=tmp_path / "no-state/workspace/.stateful_bench/task-a.json", workspace=tmp_path / "no-state/workspace", base_env={"PATH": "/bin", "STATEFUL_NESTED_CODEX_HOME_ROOT": str(tmp_path / "nested-codex-homes")})
    no_state_home = Path(no_state_env["CODEX_HOME"])
    (no_state_home / "skills/stateful-command-policy").mkdir(parents=True)
    (no_state_home / "config.toml").write_text("# stateful-bench nested Codex integration\nstale = true\n")
    (no_state_home / "skills/stateful-command-policy/SKILL.md").write_text("stale skill")
    mod.prepare_codex_environment(no_state_env, source_env={"PATH": "/bin", "STATEFUL_NESTED_CODEX_HOME_ROOT": str(tmp_path / "nested-codex-homes")})
    assert not (no_state_home / "config.toml").exists()
    assert not (no_state_home / "skills/stateful-command-policy/SKILL.md").exists()

def test_codex_pair_agent_policy_fallback_keeps_v2_awareness_guidance(monkeypatch):
    mod = load_script("codex_pair_agent.py")
    monkeypatch.setattr(mod.Path, "is_file", lambda _: False)

    skill = mod.command_policy_skill_text()

    assert "Awareness is the default coordination mode." in skill
    assert "Enforcement is opt-in only." in skill
    assert "active presence, complete exact-read freshness, and handoff context" in skill
    assert "then declare a task-level reservation" not in skill
    assert "acquire same-agent claims before native edits" not in skill
    assert "Enforcement is the default" not in skill
    assert "/v1/" not in skill


def test_codex_pair_agent_resume_and_empty_stop():
    mod = load_script("codex_pair_agent.py")

    class Completed:
        def __init__(self, returncode, stdout, stderr=""):
            self.returncode = returncode
            self.stdout = stdout
            self.stderr = stderr

    calls = []
    observed = []

    def fake_run(command, input, text, cwd, check, env, stdout, stderr):
        calls.append({"command": command, "input": input, "cwd": str(cwd), "stdout_pipe": stdout is mod.subprocess.PIPE, "stderr_pipe": stderr is mod.subprocess.PIPE})
        if len(calls) == 1:
            return Completed(1, '{"type":"session_meta","payload":{"id":"session-123"}}\n{"type":"turn.failed","error":{"message":"context_length_exceeded: input tokens exceed the model context window"}}\n')
        return Completed(0, '{"type":"token_count","info":{"total_token_usage":{"total_tokens":42}}}\n')

    captured_stdout = io.StringIO()
    original_stdout = sys.stdout
    sys.stdout = captured_stdout
    try:
        code = mod.run_codex_with_resume(["codex", "--model", "gpt-5.5", "exec", "--json", "--dangerously-bypass-hook-trust", "--cd", "/repo/work", "--sandbox", "workspace-write", "-"], "initial prompt", Path("/repo/work"), {"PATH": "/bin"}, 1, runner=fake_run, result_observer=observed.append)
    finally:
        sys.stdout = original_stdout
    assert code == 0
    assert len(calls) == 2
    assert observed[0].codex_session_id == "session-123"
    assert observed[0].resumeable_token_failure is True
    assert observed[1].returncode == 0
    assert calls[0]["input"] == "initial prompt"
    assert "Continue the same benchmark task" in calls[1]["input"]
    assert "resume" in calls[1]["command"]
    assert "session-123" in calls[1]["command"]
    assert "--cd" not in calls[1]["command"]
    assert "--sandbox" not in calls[1]["command"]
    assert "turn.failed" not in captured_stdout.getvalue()
    assert "stateful_bench.resume" in captured_stdout.getvalue()
    assert "token_count" in captured_stdout.getvalue()

    assert mod.codex_output_is_empty_stop('{"type":"message","role":"assistant","content":[]}\n{"type":"turn.completed","usage":{}}\n', "") is True
    assert mod.codex_output_is_empty_stop('{"type":"response.completed","payload":{"role":"assistant","content":[]}}\n', "") is True
    assert mod.codex_output_is_empty_stop('{"type":"message","role":"assistant","content":[{"type":"text","text":"done"}]}\n', "") is False

    responses = [
        subprocess.CompletedProcess(["codex"], 0, '{"type":"session_meta","payload":{"id":"s1"}}\n{"type":"message","role":"assistant","content":[]}\n{"type":"turn.completed","usage":{}}\n', ''),
        subprocess.CompletedProcess(["codex"], 0, '{"type":"message","role":"assistant","content":[{"type":"text","text":"done"}]}\n', ''),
    ]
    prompts = []
    def retry_runner(command, input, text, cwd, check, env, stdout, stderr):
        prompts.append(input); return responses.pop(0)
    captured_stdout = io.StringIO(); original_stdout = sys.stdout; sys.stdout = captured_stdout
    try:
        retry_code = mod.run_codex_with_resume(["codex", "exec", "-"], "original", Path("."), {}, 1, runner=retry_runner)
    finally:
        sys.stdout = original_stdout
    assert retry_code == 0
    assert len(prompts) == 2
    assert "Previous response was empty" in prompts[1]
    assert '"content":[]' not in captured_stdout.getvalue()

    responses = [
        subprocess.CompletedProcess(["codex"], 0, '{"type":"session_meta","payload":{"id":"s1"}}\n{"type":"message","role":"assistant","content":[]}\n{"type":"turn.completed","usage":{}}\n', ''),
        subprocess.CompletedProcess(["codex"], 0, '{"type":"message","role":"assistant","content":[]}\n{"type":"turn.completed","usage":{}}\n', ''),
    ]
    prompts = []
    def cap_runner(command, input, text, cwd, check, env, stdout, stderr):
        prompts.append(input); return responses.pop(0)
    assert mod.run_codex_with_resume(["codex", "exec", "-"], "original", Path("."), {}, 2, runner=cap_runner) == 2
    assert len(prompts) == 2


def test_codex_pair_agent_auth_seeding(tmp_path):
    mod = load_script("codex_pair_agent.py")
    source_home = tmp_path / "source-home"
    source_auth = source_home / ".codex/auth.json"
    source_auth.parent.mkdir(parents=True)
    source_auth.write_text('{"token":"source"}')
    source_config = source_home / ".codex/config.json"
    source_config.write_text('{"provider":"codex_lb"}')
    source_config_toml = source_home / ".codex/config.toml"
    source_config_toml.write_text('''model_provider = "codex-lb"

[model_providers.codex-lb]
base_url = "http://127.0.0.1:2455/backend-api/codex"

[mcp_servers.stateful]
command = "stale-stateful"

[features]
hooks = true

[[hooks.PreToolUse]]
matcher = ".*"
''')
    workspace = tmp_path / "runs/pair-one/workspace"
    task_path = workspace / ".stateful_bench/task-a.json"
    source_env = {"PATH": "/bin", "HOME": str(source_home), "STATEFUL_NESTED_CODEX_HOME_ROOT": str(tmp_path / "nested-codex-homes")}
    env = mod.codex_environment(task_path, workspace, source_env)
    seeded = mod.prepare_codex_environment(env, source_env=source_env)
    target_home = Path(env["CODEX_HOME"])
    assert (target_home / "auth.json").read_text() == '{"token":"source"}'
    assert (target_home / "config.json").read_text() == '{"provider":"codex_lb"}'
    assert (target_home / "config.toml").read_text() == 'model_provider = "codex-lb"\n\n[model_providers.codex-lb]\nbase_url = "http://127.0.0.1:2455/backend-api/codex"\n'
    mod.cleanup_seeded_auth(seeded)
    assert not (target_home / "auth.json").exists()
    assert not (target_home / "config.json").exists()
    assert not (target_home / "config.toml").exists()
    assert source_auth.exists() and source_config.exists() and source_config_toml.exists()

    env = mod.codex_environment(tmp_path / "second/workspace/.stateful_bench/task-a.json", tmp_path / "second/workspace", source_env)
    target_auth = Path(env["CODEX_HOME"]) / "auth.json"
    target_auth.parent.mkdir(parents=True)
    target_auth.write_text('{"token":"stale"}')
    seeded = mod.prepare_codex_environment(env, source_env=source_env)
    assert target_auth.read_text() == '{"token":"source"}'
    mod.cleanup_seeded_auth(seeded)
    assert not target_auth.exists()

    env = mod.codex_environment(tmp_path / "third/workspace/.stateful_bench/task-a.json", tmp_path / "third/workspace", source_env)
    seeded = mod.prepare_codex_environment(env, source_env=source_env)
    target_auth = Path(env["CODEX_HOME"]) / "auth.json"
    target_auth.write_text('{"token":"child"}')
    mod.cleanup_seeded_auth(seeded)
    assert target_auth.exists()
    assert target_auth.read_text() == '{"token":"child"}'


def test_codex_pair_agent_auth_failures_and_symlink_guard(tmp_path):
    mod = load_script("codex_pair_agent.py")
    source_home = tmp_path / "source-home"
    source_auth = source_home / ".codex/auth.json"
    source_auth.parent.mkdir(parents=True)
    source_auth.write_text('{"token":"source"}')
    source_env = {"PATH": "/bin", "HOME": str(source_home), "STATEFUL_NESTED_CODEX_HOME_ROOT": str(tmp_path / "nested-codex-homes")}
    env = mod.codex_environment(tmp_path / "runs/pair-one/workspace/.stateful_bench/task-a.json", tmp_path / "runs/pair-one/workspace", source_env)

    def fail_copy(*_args, **_kwargs):
        raise OSError("simulated copy failure")
    original = mod.shutil.copy2
    mod.shutil.copy2 = fail_copy
    try:
        seeded = mod.prepare_codex_environment(env, source_env=source_env)
    finally:
        mod.shutil.copy2 = original
    assert seeded is None
    assert not (Path(env["CODEX_HOME"]) / "auth.json").exists()
    assert source_auth.exists()

    symlink_root = tmp_path / "symlink"
    home_parent = symlink_root / "nested-codex-homes/pair-one"
    home_parent.parent.mkdir(parents=True)
    (symlink_root / "outside-home").mkdir()
    home_parent.symlink_to(symlink_root / "outside-home", target_is_directory=True)
    env = mod.codex_environment(symlink_root / "runs/pair-one/workspace/.stateful_bench/task-a.json", symlink_root / "runs/pair-one/workspace", {"PATH": "/bin", "STATEFUL_NESTED_CODEX_HOME_ROOT": str(symlink_root / "nested-codex-homes")})
    with pytest.raises(mod.UnsafeNestedCodexHome):
        mod.prepare_codex_environment(env, source_env={"PATH": "/bin", "STATEFUL_NESTED_CODEX_HOME_ROOT": str(symlink_root / "nested-codex-homes")})
    assert not (symlink_root / "outside-home/task-a/home/.codex").exists()


def test_codex_synthetic_agent_command_env_and_auth(tmp_path):
    mod = load_script("codex_synthetic_agent.py")
    default = mod.codex_command(Path("/tmp/workspace"), "no-state", stateful_binary="/tmp/stateful", base_env={})
    nested = mod.codex_command(Path("/tmp/workspace"), "no-state", stateful_binary="/tmp/stateful", base_env={"STATEFUL_NESTED_CODEX_HOME_ROOT": "/repo/target/nested-codex-homes"})
    assert arg_after(default, "--sandbox") == "workspace-write"
    assert arg_after(nested, "--sandbox") == "danger-full-access"

    env = mod.codex_environment("pair/one", "agent-a", {"PATH": "/bin", "STATEFUL_NESTED_CODEX_HOME_ROOT": "/repo/target/nested-codex-homes"})
    assert env["HOME"] == "/repo/target/nested-codex-homes/pair-one/agent-a/home"
    assert env["CODEX_HOME"] == "/repo/target/nested-codex-homes/pair-one/agent-a/home/.codex"
    if Path("/etc/ssl/cert.pem").is_file():
        assert env["SSL_CERT_FILE"] == "/etc/ssl/cert.pem"

    source_home = tmp_path / "source-home"
    source_auth = source_home / ".codex/auth.json"
    source_auth.parent.mkdir(parents=True)
    source_auth.write_text('{"token":"source"}')
    source_env = {"PATH": "/bin", "HOME": str(source_home), "STATEFUL_NESTED_CODEX_HOME_ROOT": str(tmp_path / "nested-codex-homes")}
    env = mod.codex_environment("pair-one", "agent-a", source_env)
    seeded = mod.prepare_codex_environment(env, source_env=source_env)
    target_auth = Path(env["CODEX_HOME"]) / "auth.json"
    assert target_auth.read_text() == '{"token":"source"}'
    mod.cleanup_seeded_auth(seeded)
    assert not target_auth.exists()
    assert source_auth.exists()
