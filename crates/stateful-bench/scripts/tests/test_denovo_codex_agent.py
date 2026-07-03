from __future__ import annotations

import asyncio
import io
import json
import sqlite3
import subprocess
import sys
import tarfile
import urllib.error
import urllib.request
from argparse import Namespace
from pathlib import Path
from types import SimpleNamespace

import pytest

from conftest import arg_after, init_git_repo, load_script


@pytest.fixture
def mod():
    return load_script("denovo_codex_agent.py")


def test_prompt_records_benchmark_constraints(mod):
    prompt = mod.build_codex_prompt(
        instance_id="fake-a",
        document="Build a parser package.",
        benchmark_max_turns=500,
        max_steps=500,
        prompt_version="v1",
        stateful_binary="/opt/stateful/bin/stateful",
    )
    assert "fake-a" in prompt
    assert "Build a parser package." in prompt
    assert "Benchmark max turns: 500" in prompt
    assert "Maximum task steps: 500" in prompt
    assert "Do not edit benchmark artifacts" in prompt
    assert "Stateful command policy" not in prompt
    assert "state_current_read" not in prompt
    assert "search_tool_bm25" not in prompt
    assert "/opt/stateful/bin/stateful" not in prompt


def test_git_diff_includes_new_and_modified_files(mod, tmp_path):
    workspace = tmp_path / "workspace"
    init_git_repo(workspace)
    (workspace / "tracked.txt").write_text("old line\n", encoding="utf-8")
    subprocess.run(["git", "add", "tracked.txt"], cwd=workspace, check=True)
    subprocess.run(["git", "commit", "-m", "initial"], cwd=workspace, check=True, stdout=subprocess.PIPE, stderr=subprocess.PIPE)

    (workspace / "tracked.txt").write_text("modified line\n", encoding="utf-8")
    (workspace / "new_file.txt").write_text("created from untracked\n", encoding="utf-8")
    patch = mod.git_diff(workspace)
    assert "diff --git a/new_file.txt b/new_file.txt" in patch
    assert "new file mode" in patch
    assert "+created from untracked" in patch
    assert "-old line" in patch
    assert "+modified line" in patch


def test_git_diff_excludes_stateful_runtime_artifacts(mod, tmp_path):
    workspace = tmp_path / "workspace"
    init_git_repo(workspace)
    (workspace / "tracked.txt").write_text("old line\n", encoding="utf-8")
    (workspace / "clean.sh").write_text("benchmark cleaner\n", encoding="utf-8")
    subprocess.run(["git", "add", "tracked.txt", "clean.sh"], cwd=workspace, check=True)
    subprocess.run(["git", "commit", "-m", "initial"], cwd=workspace, check=True, stdout=subprocess.PIPE, stderr=subprocess.PIPE)

    (workspace / "tracked.txt").write_text("new line\n", encoding="utf-8")
    (workspace / "new_file.txt").write_text("legitimate change\n", encoding="utf-8")
    for directory in [
        ".stateful_core/runtime/sessions",
        ".stateful",
        ".codex",
        "tmp/verify-supervisor/.stateful-tmp",
        ".stateful-tmp",
        ".pytest_cache/v/cache",
        ".ruff_cache/content",
        ".mypy_cache/3.11",
        "package/__pycache__",
        "target/debug",
        "upstream",
    ]:
        (workspace / directory).mkdir(parents=True, exist_ok=True)
    (workspace / ".stateful_core/runtime/session.json").write_text("{}\n", encoding="utf-8")
    (workspace / ".stateful/config.yml").write_text("policy\n", encoding="utf-8")
    (workspace / ".codex/trace.json").write_text("{}\n", encoding="utf-8")
    (workspace / "tmp/verify-supervisor/.stateful-tmp/xcrun_db").write_text("cache\n", encoding="utf-8")
    (workspace / ".stateful-tmp/xcrun_db").write_text("cache\n", encoding="utf-8")
    (workspace / ".pytest_cache/v/cache/nodeids").write_text("[]\n", encoding="utf-8")
    (workspace / ".ruff_cache/content/cache").write_text("cache\n", encoding="utf-8")
    (workspace / ".mypy_cache/3.11/module.json").write_text("{}\n", encoding="utf-8")
    (workspace / "package/__pycache__/module.cpython-311.pyc").write_text("bytecode\n", encoding="utf-8")
    (workspace / ".coverage").write_text("coverage\n", encoding="utf-8")
    (workspace / "target/debug/artifact").write_text("build artifact\n", encoding="utf-8")
    init_git_repo(workspace / "upstream")
    (workspace / "upstream/README.md").write_text("source clone scratch\n", encoding="utf-8")
    subprocess.run(["git", "add", "README.md"], cwd=workspace / "upstream", check=True)
    subprocess.run(["git", "commit", "-m", "upstream scratch"], cwd=workspace / "upstream", check=True, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
    (workspace / "clean.sh").unlink()

    patch = mod.git_diff(workspace)
    assert "diff --git a/new_file.txt b/new_file.txt" in patch
    assert "diff --git a/tracked.txt b/tracked.txt" in patch
    for hidden in [
        ".stateful_core", ".stateful/", ".codex/", "tmp/verify-supervisor", ".stateful-tmp",
        ".pytest_cache", ".ruff_cache", ".mypy_cache", "__pycache__", ".coverage",
        "target/debug/artifact", "diff --git a/clean.sh b/clean.sh", "diff --git a/upstream b/upstream", "Subproject commit",
    ]:
        assert hidden not in patch


def test_token_usage_parsers(mod):
    usage = mod.codex_token_usage_from_output(
        '{"type":"token_count","info":{"total_token_usage":{"input_tokens":10,"cached_input_tokens":3,"output_tokens":5,"reasoning_output_tokens":2,"total_tokens":15}}}\n'
    )
    assert usage == {
        "turns": 1,
        "input_tokens": 10,
        "cached_input_tokens": 3,
        "output_tokens": 5,
        "reasoning_output_tokens": 2,
        "input_plus_output_tokens": 15,
        "uncached_input_tokens": 7,
        "uncached_input_plus_output_tokens": 12,
    }

    omp_usage = mod.omp_token_usage_from_output(
        '{"type":"message","message":{"role":"assistant","usage":{"input":100,"output":12,"cacheRead":40,"reasoningTokens":5,"totalTokens":152}}}\n'
        '{"type":"message","message":{"role":"assistant","usage":{"input":110,"output":15,"cacheRead":44,"reasoningTokens":7,"totalTokens":169}}}\n'
    )
    assert omp_usage["turns"] == 1
    assert omp_usage["input_tokens"] == 154
    assert omp_usage["cached_input_tokens"] == 44
    assert omp_usage["input_plus_output_tokens"] == 169
    assert omp_usage["uncached_input_plus_output_tokens"] == 125


def test_timeout_wrappers(mod):
    def fast_runner(command, **kwargs):
        return subprocess.CompletedProcess(
            command,
            0,
            '{"type":"turn.completed","usage":{"input_tokens":100,"cached_input_tokens":40,"output_tokens":12,"reasoning_output_tokens":5}}\n'
            '{"type":"turn.completed","usage":{"input_tokens":10,"cached_input_tokens":4,"output_tokens":3,"reasoning_output_tokens":2}}\n',
            "",
        )

    def timeout_runner(command, **kwargs):
        raise subprocess.TimeoutExpired(command, kwargs.get("timeout"))

    captured_stdout = io.StringIO()
    original_stdout = sys.stdout
    sys.stdout = captured_stdout
    try:
        fast = mod.run_codex_with_timeout(["codex", "exec", "-"], "prompt", Path("/tmp"), None, 0, 1, runner=fast_runner)
    finally:
        sys.stdout = original_stdout
    with pytest.raises(mod.CodexTimeoutError, match="0.25s"):
        mod.run_codex_with_timeout(["codex", "exec", "-"], "prompt", Path("/tmp"), None, 0, 0.25, runner=timeout_runner)
    assert fast.returncode == 0
    assert fast.token_usage["input_tokens"] == 110
    assert fast.token_usage["input_plus_output_tokens"] == 125
    assert fast.token_usage["uncached_input_plus_output_tokens"] == 81
    assert captured_stdout.getvalue().count('"type":"turn.completed"') == 2

    calls = []

    def omp_runner(command, cwd, text, check, env, stdin, stdout, stderr, timeout):
        calls.append({"command": command, "cwd": str(cwd), "stdin_is_devnull": stdin == mod.subprocess.DEVNULL})
        return SimpleNamespace(
            returncode=0,
            stdout='{"type":"message","message":{"role":"assistant","usage":{"input":10,"output":5,"cacheRead":3,"reasoningTokens":2,"totalTokens":18}}}\n',
            stderr="",
        )

    summary = mod.run_omp_with_timeout(["omp", "-p", "@/tmp/prompt.txt"], Path("target/workspace"), {"HOME": "target/home"}, 5, runner=omp_runner)
    assert summary.returncode == 0
    assert summary.token_usage["input_plus_output_tokens"] == 18
    assert calls == [{"command": ["omp", "-p", "@/tmp/prompt.txt"], "cwd": "target/workspace", "stdin_is_devnull": True}]


def test_prompt_and_command_builders(mod, tmp_path):
    prompt = mod.native_subagent_prompt_instruction("on", 3)
    assert "spawn exactly 3 native subagents" in prompt
    assert "at least 3 native subagents" not in prompt

    kwargs = {
        "workspace": Path("/tmp/workspace"),
        "subagent": "on",
        "codex_bin": "/opt/homebrew/bin/codex",
        "stateful_binary": "/opt/stateful/bin/stateful",
        "benchmark_model": "gpt-5.4-mini",
        "benchmark_reasoning_effort": "low",
        "benchmark_model_context_window": 256000,
        "benchmark_temperature": "1",
    }
    no_state = mod.codex_command_for_profile(agent_mode="no-state", **kwargs)
    stateful = mod.codex_command_for_profile(agent_mode="stateful", **kwargs)
    nested = mod.codex_command_for_profile(agent_mode="no-state", base_env={"STATEFUL_NESTED_CODEX_HOME_ROOT": "/repo/target/nested-codex-homes"}, **kwargs)
    assert no_state[0] == stateful[0] == nested[0] == "/opt/homebrew/bin/codex"
    assert "--ignore-user-config" in no_state
    assert "--ignore-user-config" not in stateful
    assert "--ignore-user-config" not in nested
    for command in [no_state, stateful, nested]:
        assert "--ignore-rules" in command
        assert "skills.bundled.enabled=false" in command
        assert "features.multi_agent=true" in command

    omp = mod.omp_command_for_profile(Path("/tmp/workspace"), Path("/tmp/instance/prompt.txt"), "/opt/homebrew/bin/omp", "deepseek-v4-flash", "high")
    native_omp = mod.omp_command_for_profile(Path("/tmp/workspace"), Path("/tmp/instance/prompt.txt"), "/opt/homebrew/bin/omp", "deepseek-v4-flash", "high", enable_native_subagent=True)
    assert omp[0] == "/opt/homebrew/bin/omp"
    assert all(part in omp for part in ["-p", "--mode", "json", "--model", "deepseek-v4-flash", "--cwd", "/tmp/workspace", "--approval-mode", "yolo", "--no-title"])
    assert arg_after(omp, "--thinking") == "high"
    assert not any(part in omp for part in ["exec", "--json", "--ignore-rules", "--ignore-user-config", "--dangerously-bypass-hook-trust", "features.multi_agent=true"])
    assert "features.multi_agent=true" not in native_omp
    assert "--append-system-prompt" not in native_omp

    docker = mod.docker_omp_command_for_profile(
        workspace=tmp_path / "workspace",
        prompt_path=tmp_path / "instance" / "prompt.txt",
        home=tmp_path / "home",
        omp_bin="omp",
        benchmark_model="deepseek-v4-flash",
        benchmark_reasoning_effort="high",
        docker_image="ghcr.io/stateful/omp-agent:latest",
        base_env={"HOME": "host-home", "OPENAI_API_KEY": "sk-test", "STATEFUL_SERVER_TOKEN": "token-123", "STATEFUL_SERVER_URL": "http://127.0.0.1:43873"},
        enable_native_subagent=True,
    )
    assert docker[0] == "docker"
    assert "run" in docker and "--rm" in docker and "ghcr.io/stateful/omp-agent:latest" in docker
    assert arg_after(docker, "--network") == "bridge"
    assert arg_after(docker, "--workdir") == "/workspace"
    assert "@/prompt.txt" in docker
    assert arg_after(docker, "--thinking") == "high"
    assert "--append-system-prompt" not in docker
    env_values = [docker[index + 1] for index, value in enumerate(docker) if value == "--env"]
    assert "HOME=/home/stateful" in env_values
    assert "OPENAI_API_KEY" in env_values
    assert "STATEFUL_SERVER_URL=http://host.docker.internal:43873" in env_values
    assert "STATEFUL_SERVER_TOKEN" in env_values
    assert "OPENAI_API_KEY=sk-test" not in env_values
    assert "STATEFUL_SERVER_TOKEN=token-123" not in env_values


def test_omp_docker_command_can_disable_inner_sandbox(mod):
    command = mod.docker_omp_command_for_profile(
        workspace=Path("/tmp/workspace"),
        prompt_path=Path("/tmp/prompt.txt"),
        home=Path("/tmp/home"),
        omp_bin="omp",
        benchmark_model="deepseek-v4-flash",
        docker_image="stateful-denovo-omp-agent:local",
        base_env={"STATEFUL_SERVER_URL": "http://127.0.0.1:1234", "DEEPSEEK_API_KEY": "secret"},
        sandbox="off",
    )
    assert ["--env", "STATEFUL_OMP_SANDBOX=off"] in [command[index:index + 2] for index in range(len(command) - 1)]


def test_omp_auth_seed(mod, tmp_path):
    source_agent = tmp_path / "source/.omp/profiles/stateful/agent"
    target_agent = tmp_path / "target/.omp/profiles/stateful/agent"
    source_agent.mkdir(parents=True)
    target_agent.mkdir(parents=True)
    with sqlite3.connect(source_agent / "agent.db") as db:
        db.execute("CREATE TABLE auth_credentials (id INTEGER PRIMARY KEY, provider TEXT, credential_type TEXT, data TEXT, disabled_cause TEXT, identity_key TEXT, created_at INTEGER, updated_at INTEGER)")
        db.execute("INSERT INTO auth_credentials (provider, credential_type, data, identity_key, created_at, updated_at) VALUES ('openai-codex', 'oauth', '{\"access_token\":\"token\"}', 'email:test@example.com', 1, 2)")
        db.execute("CREATE TABLE auth_schema_version (version INTEGER)")
        db.execute("INSERT INTO auth_schema_version VALUES (1)")

    mod.seed_omp_auth_credentials({
        "HOME": str(tmp_path / "target"),
        "PI_CODING_AGENT_DIR": str(target_agent),
        "STATEFUL_HOME": str(tmp_path / "target"),
        "OMP_AUTH_SOURCE_AGENT_DIR": str(source_agent),
    })
    with sqlite3.connect(target_agent / "agent.db") as db:
        rows = db.execute("SELECT provider, credential_type, identity_key FROM auth_credentials").fetchall()
    assert rows == [("openai-codex", "oauth", "email:test@example.com")]


def test_safe_extract_symlinks(mod, tmp_path):
    buffer = io.BytesIO()
    with tarfile.open(fileobj=buffer, mode="w") as tar:
        readme = b"changes"
        info = tarfile.TarInfo("repo/CHANGES.rst")
        info.size = len(readme)
        tar.addfile(info, io.BytesIO(readme))
        link = tarfile.TarInfo("repo/docs/source/changelog.rst")
        link.type = tarfile.SYMTYPE
        link.linkname = "../../CHANGES.rst"
        tar.addfile(link)
    buffer.seek(0)
    with tarfile.open(fileobj=buffer, mode="r") as tar:
        mod._safe_extract_tar(tar, tmp_path)
    link_path = tmp_path / "repo/docs/source/changelog.rst"
    assert link_path.is_symlink()
    assert link_path.readlink().as_posix() == "../../CHANGES.rst"
    assert link_path.read_text(encoding="utf-8") == "changes"

    buffer = io.BytesIO()
    with tarfile.open(fileobj=buffer, mode="w") as tar:
        link = tarfile.TarInfo("link-out")
        link.type = tarfile.SYMTYPE
        link.linkname = "/etc/passwd"
        tar.addfile(link)
    buffer.seek(0)
    with pytest.raises(RuntimeError, match="unsafe archive link.*link-out"):
        with tarfile.open(fileobj=buffer, mode="r") as tar:
            mod._safe_extract_tar(tar, tmp_path / "reject")


def test_copy_exported_workspace_filters_benchmark_artifacts(mod, tmp_path):
    source = tmp_path / "source"
    workspace = tmp_path / "workspace"
    (source / "docs").mkdir(parents=True)
    (source / "docs/contributing.md").symlink_to("../missing/contributing.md")
    (source / ".stateful_bench/agent_synthetic").mkdir(parents=True)
    (source / ".stateful_bench/agent_synthetic/codex_synthetic_agent.py").write_text("copied")
    (source / "upstream/package").mkdir(parents=True)
    (source / "upstream/package/answer.py").write_text("leaked")
    (source / "README.md").write_text("kept")

    mod.copy_exported_workspace(source, workspace)
    link = workspace / "docs/contributing.md"
    assert link.is_symlink()
    assert link.readlink().as_posix() == "../missing/contributing.md"
    assert not link.exists()
    assert (workspace / "README.md").read_text() == "kept"
    assert not (workspace / ".stateful_bench").exists()
    assert not (workspace / "upstream").exists()


def test_environment_preparation(mod, tmp_path):
    source_home = tmp_path / "source-home"
    source_auth = source_home / ".codex/auth.json"
    source_auth.parent.mkdir(parents=True)
    source_auth.write_text('{"token":"source"}')
    source_env = {"HOME": str(source_home), "PATH": "/bin", "STATEFUL_SERVER_URL": "http://127.0.0.1:43873", "STATEFUL_SERVER_TOKEN": "token-123"}
    output = tmp_path / "adapter-output"
    workspace = tmp_path / "workspace"
    task_path = tmp_path / "extracts/results.jsonl"
    workspace.mkdir()

    no_state_env = mod.denovo_codex_environment(output, "issue/no-state", task_path, workspace, source_env)
    mod.prepare_codex_environment(no_state_env, source_env=source_env, enable_stateful=False, stateful_integration=mod.STATEFUL_INTEGRATION_NONE)
    no_state_home = Path(no_state_env["CODEX_HOME"])
    assert no_state_env["HOME"].endswith("adapter-output/codex-homes/issue-no-state/home")
    assert not (no_state_home / "config.toml").exists()
    assert not (no_state_home / "skills/stateful-command-policy/SKILL.md").exists()
    assert (no_state_home / "auth.json").exists()

    stateful_env = mod.denovo_codex_environment(output, "issue/stateful", task_path, workspace, source_env)
    mod.prepare_codex_environment(stateful_env, source_env=source_env, enable_stateful=True, stateful_binary="/tmp/stateful", stateful_integration=mod.STATEFUL_INTEGRATION_FULL)
    stateful_home = Path(stateful_env["CODEX_HOME"])
    config = (stateful_home / "config.toml").read_text()
    assert stateful_env["HOME"].endswith("adapter-output/codex-homes/issue-stateful/home")
    assert "[mcp_servers.stateful]" not in config
    assert "args = [\"mcp\", \"serve\"]" not in config
    assert "[[hooks.SessionStart]]" in config
    assert (stateful_home / "skills/stateful-command-policy/SKILL.md").exists()
    assert mod.denovo_stateful_agent_id(output, "owner/repo#1", task_path, workspace).startswith("denovo-owner-repo-1-")

    err = mod.stateful_runtime_env_error({"PATH": "/bin"})
    assert err == "stateful Codex benchmark requires STATEFUL_SERVER_URL and STATEFUL_SERVER_TOKEN"
    assert mod.stateful_runtime_env_error({"PATH": "/bin", "STATEFUL_SERVER_URL": "x", "STATEFUL_SERVER_TOKEN": "y"}) is None


def test_omp_environment_preparation(mod, tmp_path):
    source_home = tmp_path / "source-home"
    (source_home / ".codex").mkdir(parents=True)
    (source_home / ".codex/config.toml").write_text("[mcp_servers.stateful]\ncommand = 'leak'\n")
    source_env = {"HOME": str(source_home), "PATH": "/bin", "CODEX_HOME": str(source_home / ".codex"), "STATEFUL_SERVER_URL": "http://127.0.0.1:43873", "STATEFUL_SERVER_TOKEN": "token-123"}
    output = tmp_path / "adapter-output"
    workspace = tmp_path / "workspace"
    task_path = tmp_path / "extracts/results.jsonl"
    workspace.mkdir()
    commands = []

    def runner(command, text, check, env, stdout, stderr):
        commands.append(command)
        agent = Path(env["PI_CODING_AGENT_DIR"])
        extension = agent / "extensions/stateful-omp-extension.js"
        extension.parent.mkdir(parents=True, exist_ok=True)
        extension.write_text("extension")
        (agent / "config.yml").write_text(f"extensions:\n  - {extension}\ntools:\n  approvalMode: yolo\n")
        return SimpleNamespace(returncode=0, stdout="", stderr="")

    no_state_env = mod.denovo_omp_environment(output, "issue/no-state", task_path, workspace, source_env)
    mod.prepare_omp_environment(no_state_env, enable_stateful=False, stateful_binary="/tmp/stateful", runner=runner)
    stateful_env = mod.denovo_omp_environment(output, "issue/stateful", task_path, workspace, source_env)
    mod.prepare_omp_environment(
        stateful_env,
        enable_stateful=True,
        stateful_binary="/tmp/stateful",
        runner=runner,
        runtime_stateful_binary="/container/stateful",
        runtime_omp_home="/home/stateful",
        omp_bin="/tmp/omp",
        enable_native_subagent=True,
        agent_docker_image="ghcr.io/stateful/omp-agent:latest",
    )
    assert no_state_env["HOME"] == str(output / "omp-homes/issue-no-state/home")
    assert stateful_env["HOME"] == str(output / "omp-homes/issue-stateful/home")
    assert "CODEX_HOME" not in no_state_env and "CODEX_HOME" not in stateful_env
    no_state_config = Path(no_state_env["PI_CODING_AGENT_DIR"]).joinpath("config.yml").read_text()
    stateful_config = Path(stateful_env["PI_CODING_AGENT_DIR"]).joinpath("config.yml").read_text()
    assert "denovo-benchmark-source-guard.js" in no_state_config
    assert "/home/stateful/.omp/profiles/stateful/agent/extensions/stateful-omp-extension.js" in stateful_config
    assert str(output / "omp-homes/issue-stateful/home") not in stateful_config
    assert all(part in commands[0] for part in ["docker", "run", "ghcr.io/stateful/omp-agent:latest", "/tmp/omp", "agents", "unpack", "--force"])
    assert all(part in commands[1] for part in ["install", "--agent", "omp", "--yes", "--binary", "/container/stateful"])


def test_nested_codex_home_scoped_by_condition(mod, tmp_path):
    source_env = {"PATH": "/bin", "STATEFUL_NESTED_CODEX_HOME_ROOT": str(tmp_path / "nested-codex-homes")}
    task_path = tmp_path / "extracts/results.jsonl"
    off = mod.denovo_codex_environment(tmp_path / "runs/run-a/conditions/stateful-off_subagent-on/codex-cli", "owner/repo#1", task_path, tmp_path / "off/workspace", source_env)
    on = mod.denovo_codex_environment(tmp_path / "runs/run-a/conditions/stateful-on_subagent-on/codex-cli", "owner/repo#1", task_path, tmp_path / "on/workspace", source_env)
    assert off["CODEX_HOME"] != on["CODEX_HOME"]
    assert "stateful-off_subagent-on" in off["CODEX_HOME"]
    assert "stateful-on_subagent-on" in on["CODEX_HOME"]


def test_stateful_repo_enable_rewrites_container_paths_and_cleans_created_files(mod, tmp_path):
    workspace = tmp_path / "workspace"
    home = tmp_path / "home"
    workspace.mkdir()
    env = {"HOME": str(home), "STATEFUL_HOME": str(home), "CODEX_HOME": str(home / ".codex"), "PATH": "/bin"}
    calls = []

    def runner(command, cwd, env, text, check, stdout, stderr):
        calls.append({"command": [str(part) for part in command], "cwd": str(cwd), "home": env.get("HOME"), "codex_home": env.get("CODEX_HOME"), "text": text, "check": check, "stdout_pipe": stdout is subprocess.PIPE, "stderr_pipe": stderr is subprocess.PIPE})
        repos = Path(env["STATEFUL_HOME"]) / "repos"
        repos.mkdir(parents=True)
        (repos / "repo-test.json").write_text(json.dumps({"repo_id": "repo-test", "root": str(workspace), "enabled": True, "policy_config_path": str(workspace / ".stateful/config.yml")}))
        (Path(env["STATEFUL_HOME"]) / "config.yml").write_text(f"repos:\n- repo_id: repo-test\n  root: {workspace}\n  enabled: true\n  policy_config_path: {workspace / '.stateful/config.yml'}\n")
        return SimpleNamespace(returncode=0, stdout="enabled\n", stderr="")

    mod.enable_stateful_repo(env, workspace, "/tmp/stateful", runner=runner, runtime_workspace="/workspace")
    assert calls[0]["command"] == ["/tmp/stateful", "enable", "--repo", str(workspace)]
    assert calls[0]["cwd"] == str(workspace)
    assert calls[0]["home"] == str(home)
    assert calls[0]["codex_home"] == str(home / ".codex")
    metadata = json.loads((home / "repos/repo-test.json").read_text())
    registry = (home / "config.yml").read_text()
    assert metadata["root"] == "/workspace"
    assert metadata["policy_config_path"] == "/workspace/.stateful/config.yml"
    assert "root: /workspace" in registry
    assert str(workspace) not in registry

    stateful_dir = workspace / ".stateful"
    stateful_dir.mkdir(parents=True)
    (stateful_dir / "config.yml").write_text("created by enable\n")
    mod.cleanup_stateful_repo_enable(workspace, mod.StatefulRepoEnableCleanup(True, True))
    assert not stateful_dir.exists()
    stateful_dir.mkdir(parents=True)
    (stateful_dir / "config.yml").write_text("existing config\n")
    mod.cleanup_stateful_repo_enable(workspace, mod.StatefulRepoEnableCleanup(False, False))
    assert (stateful_dir / "config.yml").read_text() == "existing config\n"


def test_result_rows_runtime_errors_and_metadata(mod, tmp_path):
    result = mod.InstanceResult("case-a", False, None, "codex-error", "codex exited 1", None)
    row = mod.instance_result_row(result)
    assert mod.adapter_exit_code_after_results([result]) == 0
    assert row["error"] == "codex exited 1"
    assert row["finish_reason"] == "codex-error"

    class ReprError(Exception):
        def __init__(self, repr_text):
            self.repr_text = repr_text
        def __repr__(self):
            return self.repr_text

    missing = mod.instance_result_row(mod.instance_setup_exception_result("case-a", ReprError("ImageNotFound(HTTPError('404 Client Error: Not Found for url: http+docker://localhost/v1.53/images/aweaiteam/denovoswe:case-a/json'))")))
    assert missing["finish_reason"] == "missing-runtime-image"
    assert "aweaiteam/denovoswe:case-a" in missing["error"]
    assert mod.instance_result_row(mod.instance_setup_exception_result("case-c", RuntimeError("unsafe archive link")))["finish_reason"] == "adapter-error"

    trace_row = mod.instance_result_row(mod.InstanceResult("fake-a", True, 1.0, "skip-eval", None, {"details": {"pass_rate": 1.0}}, orchestration_trace={"trace_path": "fake-a/orchestration-trace.json", "trace_captured": True, "reservation_events": 2, "claim_events": 1, "conflict_events": 0}))
    assert trace_row["orchestration_trace"]["trace_path"] == "fake-a/orchestration-trace.json"
    assert trace_row["orchestration_trace"]["reservation_events"] == 2

    metadata = mod.profile_metadata("stateful", "on")
    assert metadata["official_benchmark_protocol"] == "denovo_swe_single_rollout"
    assert metadata["agent_rollouts_per_instance"] == 1
    assert "host_worker_count" not in metadata
    assert metadata["subagent_mode"] == "native_codex_subagents"
    assert metadata["resume_policy"] == "context_or_token_failure_only"
    omp_metadata = mod.profile_metadata("stateful", "on", cli_runtime="omp")
    assert omp_metadata["agent_kind"] == "omp-cli"
    assert omp_metadata["subagent_mode"] == "native_omp_subagents"

    results_path = tmp_path / "codex-cli/_/results.jsonl"
    mod.append_result_jsonl(results_path, result)
    mod.append_result_jsonl(results_path, mod.InstanceResult("case-b", True, 1.0, "stop", None, {"details": {"pass_rate": 1.0}}))
    rows = [json.loads(line) for line in results_path.read_text(encoding="utf-8").splitlines() if line.strip()]
    assert [row["instance_id"] for row in rows] == ["case-a", "case-b"]


def test_image_lifecycle_and_preflight(mod):
    class FakeDockerConfig:
        def __init__(self, pull_policy): self.pull_policy = pull_policy
        def model_copy(self, update):
            copied = FakeDockerConfig(self.pull_policy)
            for key, value in update.items(): setattr(copied, key, value)
            return copied
    class FakeRuntimeConfig:
        def __init__(self, backend="docker", pull_policy="if_not_present"):
            self.backend = backend; self.docker = FakeDockerConfig(pull_policy); self.image = ""; self.workdir = ""
        def model_copy(self, update):
            copied = FakeRuntimeConfig(self.backend, self.docker.pull_policy); copied.image = self.image; copied.workdir = self.workdir; copied.docker = self.docker
            for key, value in update.items(): setattr(copied, key, value)
            return copied
    class FakeImages:
        def __init__(self): self.calls = []
        def get(self, image): self.calls.append(["get", image]); raise RuntimeError("missing")
        def pull(self, image): self.calls.append(["pull", image])
        def remove(self, image, force=False): self.calls.append(["remove", image, force])
    class FakeClient:
        def __init__(self): self.images = FakeImages()
    class FakeEvaluator:
        def __init__(self, **kwargs): self.kwargs = kwargs

    async def lifecycle():
        client = FakeClient(); base = FakeRuntimeConfig()
        await mod.ensure_runtime_image_available(base, "aweaiteam/denovoswe:case-a", client_factory=lambda: client)
        local = mod.runtime_config_for_local_image(base, "aweaiteam/denovoswe:case-a", "/workspace/case-a")
        evaluator = mod.build_denovo_evaluator(FakeEvaluator, Namespace(validate_run=False, del_done_images=True, eval_iters=3), Namespace(eval=Namespace(timeout=123)))
        await mod.delete_runtime_image_after_instance(base, "aweaiteam/denovoswe:case-a", enabled=True, client_factory=lambda: client)
        return client.images.calls, base, local, evaluator

    calls, base, local, evaluator = asyncio.run(lifecycle())
    assert calls == [["get", "aweaiteam/denovoswe:case-a"], ["pull", "aweaiteam/denovoswe:case-a"], ["remove", "aweaiteam/denovoswe:case-a", True]]
    assert base.docker.pull_policy == "if_not_present"
    assert local.docker.pull_policy == "never"
    assert local.image == "aweaiteam/denovoswe:case-a"
    assert local.workdir == "/workspace/case-a"
    assert evaluator.kwargs["del_done_images"] is False
    assert evaluator.kwargs["eval_iters"] == 3

    class NotFoundError(Exception):
        def __repr__(self): return "NotFound(HTTPError('404 Client Error: Not Found for url: https://registry-1.docker.io/v2/aweaiteam/denovoswe/manifests/case-missing'))"
    class PreflightImages:
        def __init__(self): self.calls = []
        def get(self, image): self.calls.append(["get", image]); raise NotFoundError()
        def get_registry_data(self, image): self.calls.append(["get_registry_data", image]); raise NotFoundError()
        def pull(self, image): self.calls.append(["pull", image])
    class PreflightClient:
        def __init__(self): self.images = PreflightImages()

    async def preflight():
        client = PreflightClient()
        with pytest.raises(Exception) as error:
            await mod.preflight_runtime_image_available(FakeRuntimeConfig(), "aweaiteam/denovoswe:case-missing", client_factory=lambda: client)
        row = mod.instance_result_row(mod.instance_setup_exception_result("case-missing", error.value))
        return client.images.calls, row

    preflight_calls, row = asyncio.run(preflight())
    assert preflight_calls == [["get", "aweaiteam/denovoswe:case-missing"], ["get_registry_data", "aweaiteam/denovoswe:case-missing"]]
    assert row["finish_reason"] == "missing-runtime-image"
    assert "aweaiteam/denovoswe:case-missing" in row["error"]


def test_prompt_requires_native_subagents_and_blocks_upstream_source(mod):
    off = mod.build_codex_prompt("i1", "doc", 500, None, "v1", subagent="off")
    on = mod.build_codex_prompt("i1", "doc", 500, None, "v1", subagent="on", subagent_min_count=3)
    assert "Native Codex/OMP subagent requirements" not in off
    assert "Native Codex/OMP subagent requirements" in on
    assert "MUST use native subagents" in on
    assert "tasks` array containing exactly 3 implementation subagents" in on
    assert "Use exactly 3 native subagents for repository editing" in on
    assert "Benchmark isolation requirements" in off and "Benchmark isolation requirements" in on
    assert "Do not fetch, clone, open, or inspect the upstream repository" in on
    assert "ABSOLUTE RULE: DO NOT DOWNLOAD THE TARGET PACKAGE'S SOURCE CODE FROM THE INTERNET" in on
    assert "Do not create or use an `upstream` checkout" in on
    assert on.index("Native Codex/OMP subagent requirements") < on.index("Repository specification:")
    assert off != on


def test_contamination_and_subagent_usage_detection(mod, tmp_path):
    root = tmp_path
    workspace = root / "workspace"
    workspace.mkdir()

    def session_dir(home_name):
        session = root / home_name / ".omp/profiles/stateful/agent/sessions/--workspace--"
        session.mkdir(parents=True)
        return session

    false_positive = session_dir("false-positive-home")
    (false_positive / "rollout.jsonl").write_text("\n".join([
        json.dumps({"type": "message", "message": {"role": "toolResult", "content": [{"type": "text", "text": "[setup.py#ABCD]\n1:url='https://github.com/thebjorn/pydeps'\n"}]}}),
        json.dumps({"type": "message", "message": {"role": "toolResult", "content": [{"type": "text", "text": "Example: stateful sandbox run --fs git --command 'git fetch --all'"}]}}),
    ]) + "\n", encoding="utf-8")
    (false_positive / "0.read.log").write_text("URL: https://pypi.org/pypi/pydeps/json\nContent-Type: application/json\n\nhttps://github.com/thebjorn/pydeps/actions/workflows/ci.yml\n", encoding="utf-8")
    raw = session_dir("raw-read-home")
    (raw / "0.read.log").write_text("URL: https://raw.githubusercontent.com/thebjorn/pydeps/master/pydeps.py\nContent-Type: text/plain\n", encoding="utf-8")
    command = session_dir("command-home")
    (command / "rollout.jsonl").write_text(json.dumps({"type": "message", "message": {"role": "assistant", "content": [{"type": "toolCall", "name": "sandbox_bash", "arguments": {"command": "git fetch upstream main"}}]}}) + "\n", encoding="utf-8")
    other = session_dir("other-repo-command-home")
    (other / "rollout.jsonl").write_text(json.dumps({"type": "message", "message": {"role": "assistant", "content": [{"type": "toolCall", "name": "sandbox_bash", "arguments": {"command": "git clone https://github.com/other/project"}}]}}) + "\n", encoding="utf-8")
    clean_home = root / "clean-home"
    clean_home.mkdir()
    (workspace / "upstream").mkdir()
    upstream = mod.benchmark_contamination_record("thebjorn_pydeps_pr233", workspace, clean_home)
    (workspace / "upstream").rmdir()
    assert upstream["kind"] == "upstream-worktree"
    assert mod.benchmark_contamination_record("thebjorn_pydeps_pr233", workspace, root / "false-positive-home/.omp/profiles/stateful/agent") is None
    assert mod.benchmark_contamination_record("thebjorn_pydeps_pr233", workspace, root / "raw-read-home/.omp/profiles/stateful/agent")["pattern"] == "raw.githubusercontent.com/thebjorn/pydeps"
    assert mod.benchmark_contamination_record("thebjorn_pydeps_pr233", workspace, root / "command-home/.omp/profiles/stateful/agent")["pattern"] == "git fetch"
    assert mod.benchmark_contamination_record("thebjorn_pydeps_pr233", workspace, root / "other-repo-command-home/.omp/profiles/stateful/agent") is None

    codex_home = root / ".codex"
    session = codex_home / "sessions/2026/06/14"
    session.mkdir(parents=True)
    (session / "rollout.jsonl").write_text(json.dumps({"type": "response_item", "payload": {"type": "function_call", "name": "multi_agent_v1spawn_agent"}}) + "\n" + json.dumps({"type": "response_item", "payload": {"type": "function_call", "name": "wait_agent"}}) + "\n", encoding="utf-8")
    db = sqlite3.connect(codex_home / "state_5.sqlite")
    for table in ["agent_jobs", "agent_job_items", "thread_spawn_edges", "thread_dynamic_tools"]:
        db.execute(f"create table {table}(id integer primary key)")
        db.execute(f"insert into {table}(id) values (1)")
    db.commit(); db.close()
    used = mod.detect_native_subagent_usage(codex_home)
    assert used["subagent_used"] is True
    assert used["counts"]["spawn_agent_calls"] == 1
    assert used["counts"]["wait_agent_calls"] == 1
    assert used["counts"]["agent_jobs"] == 1
    omp_home = root / "omp-agent"
    omp_session = omp_home / "sessions/--workspace--"
    omp_session.mkdir(parents=True)
    (omp_session / "session.jsonl").write_text(json.dumps({"type": "message", "message": {"role": "assistant", "content": [{"type": "toolCall", "name": "task", "arguments": {"tasks": [{"assignment": "one"}, {"assignment": "two"}, {"assignment": "three"}]}}]}}) + "\n", encoding="utf-8")
    omp_usage = mod.native_subagent_usage("on", 3, omp_home, cli_runtime="omp")
    assert omp_usage["mode"] == "native_omp_subagents"
    assert omp_usage["native_subagent"]["subagent_spawn_count"] == 3
    assert omp_usage["subagent_requirement_met"] is True


def test_orchestration_summaries(mod):
    summary = mod.summarize_orchestration_events([
        {"event_type": "ReservationDeclared", "agent_id": "omp-agent", "workspace_id": "workspace-a"},
        {"event_type": "ClaimAcquired", "agent_id": "omp-agent", "workspace_id": "workspace-a"},
        {"event_type": "AuthorizationDenied", "agent_id": "omp-agent", "workspace_id": "workspace-a"},
        {"event_type": "AuthorizationDenied", "agent_id": "omp-agent", "workspace_id": "workspace-other"},
    ], agent_id="denovo-instance", workspace_id="workspace-a")
    assert summary["event_count"] == 3
    assert summary["reservation_events"] == 1
    assert summary["claim_events"] == 1
    assert summary["conflict_events"] == 1

    heartbeat = mod.summarize_orchestration_events([
        {"event_type": "AgentHeartbeat", "timestamp": "2026-06-28T14:16:39Z", "agent_id": "omp-agent", "workspace_id": "workspace-a"},
        {"event_type": "AgentHeartbeat", "timestamp": "2026-06-28T14:16:44Z", "agent_id": "omp-agent", "workspace_id": "workspace-a"},
        {"event_type": "AuthorizationDenied", "timestamp": "2026-06-28T14:16:45Z", "agent_id": "omp-agent", "workspace_id": "workspace-a", "payload": {"path": "src/pkg.py", "message": "Target existence changed since the supplied base observation."}},
        {"event_type": "AgentHeartbeat", "timestamp": "2026-06-28T14:17:30Z", "agent_id": "omp-agent", "workspace_id": "workspace-a"},
        {"event_type": "AgentHeartbeat", "timestamp": "2026-06-28T14:17:35Z", "agent_id": "omp-agent", "workspace_id": "workspace-a"},
        {"event_type": "ReservationDeclared", "timestamp": "2026-06-28T14:17:31Z", "agent_id": "omp-agent", "workspace_id": "workspace-a"},
    ], agent_id="denovo-instance", workspace_id="workspace-a")
    assert heartbeat["event_count"] == 6
    assert heartbeat["event_types"]["AgentHeartbeat"] == 4
    assert heartbeat["heartbeat_windows"] == 2
    assert heartbeat["heartbeat_max_gap_ms"] == 46000
    assert heartbeat["denial_paths"]["src/pkg.py"] == 1


def test_parse_defaults_validate_run_and_empty_stop(mod):
    base = [
        "--data-file", "data.jsonl", "--config", "configs/tasks/denovoswe.yaml", "--mode", "batch", "--output", "out",
        "--agent-mode", "stateful", "--subagent", "on", "--aweagent-root", "AweAgent", "--codex-bin", "codex",
        "--stateful-binary", "stateful", "--benchmark-model", "gpt-5.4-mini", "--benchmark-reasoning-effort", "low",
        "--benchmark-model-context-window", "256000", "--benchmark-temperature", "1", "--benchmark-max-turns", "500",
        "--max-resumes", "1", "--codex-timeout-seconds", "7200", "--eval-iters", "1", "--prompt-version", "v1",
    ]
    assert mod.parse_args(base).del_done_images is True
    assert mod.parse_args(base + ["--keep-done-images"]).del_done_images is False
    assert mod.max_concurrent_limit(Namespace(max_concurrent=None)) == 1
    assert mod.max_concurrent_limit(Namespace(max_concurrent=0)) == 1
    assert mod.max_concurrent_limit(Namespace(max_concurrent=6)) == 6
    assert mod.should_run_codex(Namespace(validate_run=True)) is False
    assert mod.should_run_codex(Namespace(validate_run=False)) is True
    assert mod.cli_runtime_failure(mod.CODEX_EMPTY_STOP_EXIT_CODE, "codex") == ("codex-empty-stop", "codex returned an empty stop after retry cap")
    assert mod.cli_runtime_failure(mod.CODEX_EMPTY_STOP_EXIT_CODE, "omp") == ("omp-empty-stop", "omp returned an empty stop after retry cap")
    assert mod.cli_runtime_failure(99, "omp") == ("omp-error", "omp exited 99")


def test_low_disk_cache_cleanup_and_tmp_harvest(mod, tmp_path):
    result = mod.low_disk_space_result("case-a", Path("/tmp/output"), min_free_bytes=100, disk_usage=lambda path: SimpleNamespace(free=40))
    row = mod.instance_result_row(result)
    assert row["finish_reason"] == "disk-space-low"
    assert "free disk space 40 bytes is below required 100 bytes" in row["error"]

    home = tmp_path / "home"
    cache = home / ".cache"
    library_cache = home / "Library/Caches"
    codex_home = home / ".codex"
    cache.mkdir(parents=True); library_cache.mkdir(parents=True); codex_home.mkdir(parents=True)
    (cache / "blob").write_text("cache")
    (library_cache / "blob").write_text("cache")
    (codex_home / "session.jsonl").write_text("log")
    removed = mod.cleanup_codex_home_caches({"HOME": str(home), "XDG_CACHE_HOME": str(cache), "CODEX_HOME": str(codex_home)})
    assert sorted(Path(path).name for path in removed) == [".cache", "Caches"]
    assert not cache.exists() and not library_cache.exists()
    assert (codex_home / "session.jsonl").exists()

    workspace = tmp_path / "workspace"
    init_git_repo(workspace)
    (workspace / "tmp").mkdir()
    (workspace / "tmp/kept.txt").write_text("kept\n")
    (workspace / "README.md").write_text("base\n")
    subprocess.run(["git", "add", "README.md"], cwd=workspace, check=True)
    subprocess.run(["git", "commit", "-m", "base"], cwd=workspace, check=True, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
    assert "tmp/kept.txt" in mod.git_diff(workspace)


def test_target_upstream_proxy_blocks_raw_source_url(mod):
    proxy = mod.start_target_upstream_deny_proxy("cloudtools_troposphere_pr2343")
    try:
        opener = urllib.request.build_opener(urllib.request.ProxyHandler({"http": proxy.url}))
        with pytest.raises(urllib.error.HTTPError) as error:
            opener.open("http://raw.githubusercontent.com/cloudtools/troposphere/master/troposphere/apigateway.py", timeout=2)
        assert error.value.code == 403
    finally:
        proxy.close()


def test_denovo_omp_agent_dockerfile_installs_bubblewrap():
    dockerfile = Path(__file__).resolve().parents[2] / "docker" / "denovo-omp-agent.Dockerfile"
    text = dockerfile.read_text(encoding="utf-8")
    assert "bubblewrap" in text
    assert "command -v bwrap" in text
