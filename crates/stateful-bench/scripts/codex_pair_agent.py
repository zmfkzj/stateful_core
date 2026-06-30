#!/usr/bin/env python3
"""Launch one Codex agent for a SWE-bench pair task."""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import shlex
import shutil
import subprocess
import stat
import sys
from pathlib import Path


DEFAULT_BENCHMARK_MODEL = "gpt-5.4-mini"
DEFAULT_BENCHMARK_REASONING_EFFORT = "low"
DEFAULT_NATIVE_SUBAGENT_MIN_COUNT = 3
EMPTY_STOP_RETRY_CAP = 1
STATEFUL_INTEGRATION_FULL = "hooks-skill"
STATEFUL_INTEGRATION_HOOKS_ONLY = "hooks-only"
STATEFUL_INTEGRATION_NONE = "none"
NESTED_CODEX_HOME_ROOT_ENV = "STATEFUL_NESTED_CODEX_HOME_ROOT"
AUTH_FILE_NAME = "auth.json"
CODEX_CONFIG_FILE_NAME = "config.json"
CODEX_CONFIG_TOML_FILE_NAME = "config.toml"
COMMAND_POLICY_SKILL = "stateful-command-policy"
COMMAND_POLICY_SKILL_PATH = Path("skills") / COMMAND_POLICY_SKILL / "SKILL.md"
STATEFUL_BENCH_CONFIG_MARKER = "# stateful-bench nested Codex integration"
FALLBACK_COMMAND_POLICY_SKILL = """---
name: stateful-command-policy
description: Detailed procedure for using Stateful MCP coordination, claims, sandbox profiles, and hook-denial recovery after a Stateful rule or denial says this policy applies
---

# Stateful Command Policy

This skill is the procedural manual. Rules and hook denials decide when Stateful
policy applies. First inspect current state with the active Stateful MCP tool
names, then declare a task-level reservation covering the known file set and
acquire matching same-session claims before native edits. Use canonical names in
guidance (`state_current_read`,
`state_reservation_declare`, `state_claim_acquire`) and switch to runtime-specific
aliases only when those are the active tool names, such as Codex
`mcp__stateful__state_reservation_declare` or OMP
`mcp__stateful_state_reservation_declare`. Do not run stateful MCP calls through
Bash. Use the sandbox-run wrappers printed by hook denials for shell commands.
"""
RESUME_PROMPT = """\
The previous Codex exec turn stopped because of a token/context limit.
Continue the same benchmark task from the current workspace state. Re-read the
task file and relevant source files if needed, then finish the task while
preserving the benchmark constraints already given in this session.
"""
EMPTY_STOP_PROMPT = """\
Previous response was empty. Continue with the requested code change. Do not summarize.
"""



def native_subagent_prompt_instruction(
    enable_native_subagent: bool,
    subagent_min_count: int = DEFAULT_NATIVE_SUBAGENT_MIN_COUNT,
) -> str:
    if not enable_native_subagent:
        return ""
    return f"""

Native Codex subagent requirements:
- MUST use native Codex subagents for this task.
- Spawn at least {subagent_min_count} native subagents before finishing.
- MUST read and use the `dispatching-parallel-agents` skill before spawning native subagents when that skill is available.
- Use all {subagent_min_count} native subagents for repository editing.
- Do not leave any native subagent as analysis-only; each one must inspect, edit, and verify the workspace.
- Wait for each spawned subagent and incorporate its work or findings into the final workspace.
""".rstrip()


class SeededAuth:
    def __init__(
        self,
        path: Path,
        digest: str,
        extra_files: list[tuple[Path, str]] | None = None,
    ) -> None:
        self.path = path
        self.digest = digest
        self.files = [(path, digest)]
        if extra_files:
            self.files.extend(extra_files)


class UnsafeNestedCodexHome(RuntimeError):
    pass


class CodexRunResult:
    def __init__(
        self,
        returncode: int,
        stdout: str,
        stderr: str,
        session_id: str | None,
        resumeable_token_failure: bool,
        empty_stop: bool,
    ) -> None:
        self.returncode = returncode
        self.stdout = stdout
        self.stderr = stderr
        self.session_id = session_id
        self.resumeable_token_failure = resumeable_token_failure
        self.empty_stop = empty_stop


def toml_string(value: str) -> str:
    return json.dumps(value)


def toml_table_name(line: str) -> str | None:
    stripped = line.strip()
    if stripped.startswith("[[") and stripped.endswith("]]"):
        return stripped[2:-2].strip()
    if stripped.startswith("[") and stripped.endswith("]"):
        return stripped[1:-1].strip()
    return None


def toml_key_name(line: str) -> str | None:
    stripped = line.strip()
    if not stripped or stripped.startswith("#") or "=" not in stripped:
        return None
    return stripped.split("=", 1)[0].strip()


def codex_provider_config_fragment(config: str | None) -> str:
    if not config:
        return ""

    lines = config.splitlines()
    output: list[str] = []
    include_table = False
    in_table = False

    for line in lines:
        table = toml_table_name(line)
        if table is not None:
            in_table = True
            include_table = table == "model_providers" or table.startswith("model_providers.")
            if include_table:
                if output and output[-1] != "":
                    output.append("")
                output.append(line)
            continue

        if include_table:
            if toml_key_name(line) in {
                "websocket",
                "websocker",
                "features.websocket",
                "features.websocker",
            }:
                continue
            output.append(line)
            continue

        if in_table:
            continue

        key = toml_key_name(line)
        if key == "model_provider":
            output.append(line)

    while output and output[-1] == "":
        output.pop()
    return "\n".join(output)


def hook_override(event_name: str, command: str, status_message: str, matcher: str | None = None) -> str:
    fields = []
    if matcher is not None:
        fields.append(f"matcher = {toml_string(matcher)}")
    fields.append(
        "hooks = [{ "
        f"type = {toml_string('command')}, "
        f"command = {toml_string(command)}, "
        f"statusMessage = {toml_string(status_message)} "
        "}]"
    )
    return f"hooks.{event_name}=[{{ {', '.join(fields)} }}]"


def stateful_hook_overrides(stateful_binary: str) -> list[str]:
    hook_prefix = f'"{stateful_binary}" hook codex'
    return [
        "features.hooks=true",
        hook_override(
            "PreToolUse",
            f"{hook_prefix} pre-tool-use",
            "Authorizing stateful tool use",
            "Bash|apply_patch|Edit|Write|mcp__filesystem__.*",
        ),
        hook_override(
            "PostToolUse",
            f"{hook_prefix} post-tool-use",
            "Recording stateful activity",
            "Bash|apply_patch|Edit|Write|mcp__filesystem__.*",
        ),
    ]


def stateful_codex_config(
    stateful_binary: str,
    base_config: str | None = None,
) -> str:
    hook_prefix = f"{shlex.quote(stateful_binary)} hook codex"
    base_config_text = ""
    provider_config = codex_provider_config_fragment(base_config)
    if provider_config:
        stripped = provider_config.rstrip()
        if stripped:
            base_config_text = f"{stripped}\n\n"
    return f"""{STATEFUL_BENCH_CONFIG_MARKER}
{base_config_text}
[features]
hooks = true

[[hooks.SessionStart]]
matcher = "startup|resume|clear|compact"

[[hooks.SessionStart.hooks]]
type = "command"
command = {toml_string(f"{hook_prefix} session-start")}
statusMessage = "Loading stateful current state"

[[hooks.UserPromptSubmit]]

[[hooks.UserPromptSubmit.hooks]]
type = "command"
command = {toml_string(f"{hook_prefix} user-prompt-submit")}
statusMessage = "Checking stateful reservation context"

[[hooks.PreToolUse]]
matcher = ".*"

[[hooks.PreToolUse.hooks]]
type = "command"
command = {toml_string(f"{hook_prefix} pre-tool-use")}
statusMessage = "Authorizing stateful tool use"

[[hooks.PostToolUse]]
matcher = "Bash|apply_patch|Edit|Write|file_change|mcp__filesystem__.*"

[[hooks.PostToolUse.hooks]]
type = "command"
command = {toml_string(f"{hook_prefix} post-tool-use")}
statusMessage = "Recording stateful activity"

[[hooks.Stop]]

[[hooks.Stop.hooks]]
type = "command"
command = {toml_string(f"{hook_prefix} stop")}
statusMessage = "Recording stateful activity"
"""


def command_policy_skill_text() -> str:
    script_path = Path(__file__).resolve()
    candidates = [
        script_path.parents[2]
        / "stateful-cli"
        / "assets"
        / "stateful-command-policy"
        / "SKILL.md",
        script_path.parents[1]
        / "crates"
        / "stateful-cli"
        / "assets"
        / "stateful-command-policy"
        / "SKILL.md",
    ]
    for candidate in candidates:
        try:
            if candidate.is_file():
                return candidate.read_text(encoding="utf-8")
        except OSError:
            pass
    return FALLBACK_COMMAND_POLICY_SKILL


def write_text_file(path: Path, contents: str) -> None:
    if path.is_symlink():
        raise UnsafeNestedCodexHome(f"unsafe symlinked Codex file: {path}")
    if not ensure_safe_directory(path.parent):
        raise UnsafeNestedCodexHome(f"unsafe Codex directory: {path.parent}")
    path.write_text(contents, encoding="utf-8")


def write_stateful_codex_integration(
    env: dict[str, str],
    stateful_binary: str,
    include_skill: bool = True,
    base_config: str | None = None,
) -> None:
    codex_home = Path(env["CODEX_HOME"])
    write_text_file(
        codex_home / "config.toml",
        stateful_codex_config(
            stateful_binary,
            base_config=base_config,
        ),
    )
    if include_skill:
        write_text_file(codex_home / COMMAND_POLICY_SKILL_PATH, command_policy_skill_text())
    else:
        remove_file(codex_home / COMMAND_POLICY_SKILL_PATH)


def remove_file(path: Path) -> None:
    try:
        if path.is_file() or path.is_symlink():
            path.unlink()
    except FileNotFoundError:
        pass


def remove_stateful_codex_integration(env: dict[str, str]) -> None:
    codex_home = Path(env["CODEX_HOME"])
    config_path = codex_home / "config.toml"
    try:
        if config_path.is_file() and config_path.read_text(encoding="utf-8").startswith(
            STATEFUL_BENCH_CONFIG_MARKER
        ):
            config_path.unlink()
    except FileNotFoundError:
        pass
    remove_file(codex_home / COMMAND_POLICY_SKILL_PATH)


def codex_command(
    workspace: Path,
    mode: str,
    stateful_binary: str,
    benchmark_model: str = DEFAULT_BENCHMARK_MODEL,
    benchmark_reasoning_effort: str = DEFAULT_BENCHMARK_REASONING_EFFORT,
    benchmark_model_context_window: int | None = None,
    enable_native_subagent: bool = False,
    disable_bundled_skills: bool = False,
    stateful_integration: str = STATEFUL_INTEGRATION_FULL,
    base_env: dict[str, str] | None = None,
) -> list[str]:
    source_env = os.environ if base_env is None else base_env
    nested_benchmark = bool(source_env.get(NESTED_CODEX_HOME_ROOT_ENV))
    sandbox = "danger-full-access" if nested_benchmark else "workspace-write"
    command = [
        "codex",
        "--model",
        benchmark_model,
        "-c",
        f"model_reasoning_effort={toml_string(benchmark_reasoning_effort)}",
        "--ask-for-approval",
        "never",
        "exec",
        "--json",
        "--dangerously-bypass-hook-trust",
        "--cd",
        str(workspace),
        "--sandbox",
        sandbox,
    ]
    if benchmark_model_context_window is not None:
        command.extend(["-c", f"model_context_window={benchmark_model_context_window}"])
    if enable_native_subagent:
        command.extend(["-c", "features.multi_agent=true"])
    if disable_bundled_skills:
        command.extend(["-c", "skills.bundled.enabled=false"])
    if not nested_benchmark:
        command.extend(["-c", "sandbox_workspace_write.network_access=true"])
    if (
        mode == "stateful"
        and not nested_benchmark
        and stateful_integration != STATEFUL_INTEGRATION_NONE
    ):
        for override in stateful_hook_overrides(stateful_binary):
            command.extend(["-c", override])
    command.append("-")
    return command


def codex_resume_command(command: list[str], session_id: str) -> list[str]:
    exec_index = command.index("exec")
    resume = command[: exec_index + 1] + ["resume"]
    tail = command[exec_index + 1 :]
    index = 0
    flags_with_values = {
        "-c",
        "--config",
        "-m",
        "--model",
        "-i",
        "--image",
        "-o",
        "--output-last-message",
    }
    flags_without_values = {
        "--json",
        "--dangerously-bypass-hook-trust",
        "--skip-git-repo-check",
        "--ephemeral",
        "--ignore-user-config",
        "--ignore-rules",
        "--strict-config",
    }
    unsupported_resume_flags = {"--cd", "-C", "--sandbox", "-s", "--add-dir"}
    while index < len(tail):
        arg = tail[index]
        if arg in flags_without_values:
            resume.append(arg)
        elif arg in flags_with_values:
            if index + 1 < len(tail):
                resume.extend([arg, tail[index + 1]])
                index += 1
        elif arg in unsupported_resume_flags:
            if index + 1 < len(tail):
                index += 1
        elif arg == "-":
            pass
        index += 1
    resume.extend([session_id, "-"])
    return resume


def run_codex_once(
    command: list[str],
    prompt: str,
    workspace: Path,
    env: dict[str, str] | None,
    runner=subprocess.run,
) -> CodexRunResult:
    completed = runner(
        command,
        input=prompt,
        text=True,
        cwd=workspace,
        check=False,
        env=env,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
    )
    stdout = completed.stdout or ""
    stderr = completed.stderr or ""
    return CodexRunResult(
        returncode=completed.returncode,
        stdout=stdout,
        stderr=stderr,
        session_id=codex_session_id_from_output(stdout),
        resumeable_token_failure=codex_output_has_resumeable_token_failure(stdout, stderr),
        empty_stop=completed.returncode == 0 and codex_output_is_empty_stop(stdout, stderr),
    )


def run_codex_with_resume(
    command: list[str],
    prompt: str,
    workspace: Path,
    env: dict[str, str] | None,
    max_resumes: int,
    runner=subprocess.run,
    result_observer=None,
) -> int:
    pending_resume_failures: list[CodexRunResult] = []
    session_id: str | None = None
    resume_attempts = 0
    empty_stop_attempts = 0
    current_command = command
    current_prompt = prompt

    while True:
        result = run_codex_once(
            current_command,
            current_prompt,
            workspace,
            env,
            runner=runner,
        )
        if result.session_id:
            session_id = result.session_id
        if result_observer is not None:
            result_observer(result)

        if result.resumeable_token_failure:
            if session_id and resume_attempts < max_resumes:
                resume_attempts += 1
                pending_resume_failures.append(result)
                current_command = codex_resume_command(command, session_id)
                current_prompt = RESUME_PROMPT
                continue
            emit_codex_results(pending_resume_failures, suppress_resumeable_failures=False)
            emit_codex_result(result, suppress_resumeable_failures=False)
            return result.returncode if result.returncode != 0 else 1

        if result.empty_stop:
            if session_id and empty_stop_attempts < EMPTY_STOP_RETRY_CAP:
                empty_stop_attempts += 1
                current_command = codex_resume_command(command, session_id)
                current_prompt = EMPTY_STOP_PROMPT
                continue
            emit_codex_results(pending_resume_failures, suppress_resumeable_failures=True)
            return 2

        if result.returncode == 0:
            emit_codex_results(pending_resume_failures, suppress_resumeable_failures=True)
            for attempt in range(1, resume_attempts + 1):
                print_resume_event(session_id=session_id, attempt=attempt)
            emit_codex_result(result, suppress_resumeable_failures=False)
            return 0

        emit_codex_results(pending_resume_failures, suppress_resumeable_failures=False)
        emit_codex_result(result, suppress_resumeable_failures=False)
        return result.returncode


def emit_codex_results(
    results: list[CodexRunResult],
    suppress_resumeable_failures: bool,
) -> None:
    for result in results:
        emit_codex_result(result, suppress_resumeable_failures=suppress_resumeable_failures)


def emit_codex_result(result: CodexRunResult, suppress_resumeable_failures: bool) -> None:
    for line in result.stdout.splitlines(keepends=True):
        if suppress_resumeable_failures and codex_line_is_resumeable_token_failure(line):
            continue
        sys.stdout.write(line)
    if result.stdout:
        sys.stdout.flush()
    if result.stderr:
        sys.stderr.write(result.stderr)
        sys.stderr.flush()


def print_resume_event(session_id: str | None, attempt: int) -> None:
    sys.stdout.write(
        json.dumps(
            {
                "type": "stateful_bench.resume",
                "attempt": attempt,
                "session_id": session_id,
                "reason": "token_context_limit",
            },
            sort_keys=True,
        )
        + "\n"
    )
    sys.stdout.flush()


def codex_session_id_from_output(stdout: str) -> str | None:
    session_id = None
    for event in iter_json_events(stdout):
        session_id = codex_session_id_from_event(event) or session_id
    return session_id


def codex_session_id_from_event(event: object) -> str | None:
    if not isinstance(event, dict):
        return None
    event_type = str(event.get("type", ""))
    if isinstance(event.get("session_id"), str):
        return event["session_id"]
    if "session" in event_type and isinstance(event.get("id"), str):
        return event["id"]

    payload = event.get("payload")
    if isinstance(payload, dict):
        if isinstance(payload.get("session_id"), str):
            return payload["session_id"]
        if "session" in event_type and isinstance(payload.get("id"), str):
            return payload["id"]
        session = payload.get("session")
        if isinstance(session, dict) and isinstance(session.get("id"), str):
            return session["id"]
    return None


def codex_output_is_empty_stop(stdout: str, stderr: str) -> bool:
    if stderr.strip():
        return False
    saw_terminal = False
    saw_assistant = False
    for event in iter_json_events(stdout):
        if codex_event_has_meaningful_assistant_content(event):
            return False
        event_type = str(event.get("type", "")).lower() if isinstance(event, dict) else ""
        if event_type in {"turn.completed", "turn.done", "response.completed"} or event_type.endswith(
            ".completed"
        ):
            saw_terminal = True
        if isinstance(event, dict):
            payload = event.get("payload")
            if str(event.get("role", "")).lower() == "assistant" or (
                isinstance(payload, dict) and str(payload.get("role", "")).lower() == "assistant"
            ):
                saw_assistant = True
    return saw_terminal and saw_assistant


def codex_event_has_meaningful_assistant_content(event: object) -> bool:
    if not isinstance(event, dict):
        return False
    candidates = []
    if str(event.get("role", "")).lower() == "assistant":
        candidates.append(event.get("content"))
    payload = event.get("payload")
    if isinstance(payload, dict) and str(payload.get("role", "")).lower() == "assistant":
        candidates.append(payload.get("content"))
    for content in candidates:
        if isinstance(content, str) and content.strip():
            return True
        if isinstance(content, list):
            for item in content:
                if isinstance(item, str) and item.strip():
                    return True
                if isinstance(item, dict) and str(item.get("text", "")).strip():
                    return True
    return False


def codex_output_has_resumeable_token_failure(stdout: str, stderr: str) -> bool:
    if contains_resumeable_token_failure_text(stderr):
        return True
    return any(codex_event_is_resumeable_token_failure(event) for event in iter_json_events(stdout))


def codex_line_is_resumeable_token_failure(line: str) -> bool:
    try:
        event = json.loads(line)
    except json.JSONDecodeError:
        return False
    return codex_event_is_resumeable_token_failure(event)


def codex_event_is_resumeable_token_failure(event: object) -> bool:
    if not isinstance(event, dict):
        return False
    event_type = str(event.get("type", "")).lower()
    is_failure_event = event_type in {"turn.failed", "error"} or event_type.endswith(".failed")
    if not is_failure_event and "error" not in event:
        return False
    return contains_resumeable_token_failure_text(json.dumps(event, sort_keys=True))


def contains_resumeable_token_failure_text(text: str) -> bool:
    normalized = text.lower()
    non_resumeable = [
        "usage limit",
        "rate limit",
        "rate_limit",
        "insufficient_quota",
        "purchase more credits",
        "billing",
    ]
    if any(pattern in normalized for pattern in non_resumeable):
        return False
    resumeable = [
        "context_length_exceeded",
        "context length",
        "context window",
        "model context",
        "too many tokens",
        "token limit",
        "maximum tokens",
        "max tokens",
        "input tokens",
        "exceeds the model",
        "exceed the model",
    ]
    return any(pattern in normalized for pattern in resumeable)


def iter_json_events(text: str):
    for line in text.splitlines():
        try:
            yield json.loads(line)
        except json.JSONDecodeError:
            continue


def path_fragment(value: str) -> str:
    fragment = "".join(
        character if character.isalnum() or character in "._-" else "-"
        for character in str(value)
    ).strip(".-")
    return fragment or "item"


def path_scope_digest(*values: Path | str) -> str:
    normalized = [str(Path(value).expanduser().resolve(strict=False)) for value in values]
    return hashlib.sha256("\0".join(normalized).encode("utf-8")).hexdigest()[:16]


def codex_environment(
    task_path: Path,
    workspace: Path,
    base_env: dict[str, str] | None = None,
) -> dict[str, str] | None:
    source_env = os.environ if base_env is None else base_env
    root = source_env.get(NESTED_CODEX_HOME_ROOT_ENV)
    if not root:
        return None

    env = dict(source_env)
    env.pop("STATEFUL_SESSION_ID", None)
    pair_fragment = path_fragment(workspace.parent.name or workspace.name)
    agent_fragment = path_fragment(task_path.stem)
    scope_digest = path_scope_digest(task_path, workspace)
    home = Path(root) / pair_fragment / agent_fragment / scope_digest / "home"
    env["HOME"] = str(home)
    env["CODEX_HOME"] = str(home / ".codex")
    env["XDG_CONFIG_HOME"] = str(home / ".config")
    env["XDG_CACHE_HOME"] = str(home / ".cache")

    system_cert = Path("/etc/ssl/cert.pem")
    if not env.get("SSL_CERT_FILE") and system_cert.is_file():
        env["SSL_CERT_FILE"] = str(system_cert)

    return env


def benchmark_source_env(
    mode: str,
    session_id: str | None,
    base_env: dict[str, str] | None = None,
    preserve_stateful_session: bool = True,
) -> dict[str, str]:
    env = dict(os.environ if base_env is None else base_env)
    env.pop("STATEFUL_SESSION_ID", None)
    if mode == "stateful" and preserve_stateful_session:
        if not session_id:
            raise ValueError("session_id is required in stateful mode")
        env["STATEFUL_SESSION_ID"] = session_id
    return env


def source_codex_file_path(source_env: dict[str, str], file_name: str) -> Path | None:
    codex_home = source_env.get("CODEX_HOME")
    if codex_home:
        path = Path(codex_home) / file_name
        if path.is_file():
            return path

    home = source_env.get("HOME")
    if home:
        path = Path(home) / ".codex" / file_name
        if path.is_file():
            return path

    return None


def source_codex_auth_path(source_env: dict[str, str]) -> Path | None:
    return source_codex_file_path(source_env, AUTH_FILE_NAME)


def source_codex_config_path(source_env: dict[str, str]) -> Path | None:
    return source_codex_file_path(source_env, CODEX_CONFIG_FILE_NAME)


def source_codex_config_toml_path(source_env: dict[str, str]) -> Path | None:
    return source_codex_file_path(source_env, CODEX_CONFIG_TOML_FILE_NAME)


def file_digest(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def canonical_system_temp_prefix(path: Path) -> Path:
    if sys.platform != "darwin" or not path.is_absolute():
        return path
    raw = str(path)
    for public, canonical in (("/tmp", "/private/tmp"), ("/var", "/private/var")):
        if raw == public:
            return Path(canonical)
        if raw.startswith(f"{public}/"):
            return Path(canonical) / raw[len(public) + 1 :]
    return path


def ensure_safe_directory(path: Path) -> bool:
    path = canonical_system_temp_prefix(path)
    cursor = Path(path.anchor) if path.is_absolute() else Path()
    parts = path.parts[1:] if path.is_absolute() else path.parts
    for part in parts:
        cursor = cursor / part
        try:
            metadata = cursor.lstat()
        except FileNotFoundError:
            try:
                cursor.mkdir()
            except FileExistsError:
                pass
            except OSError:
                return False
            try:
                metadata = cursor.lstat()
            except OSError:
                return False

        if stat.S_ISLNK(metadata.st_mode) or not stat.S_ISDIR(metadata.st_mode):
            return False
    return True


def prepare_codex_environment(
    env: dict[str, str] | None,
    source_env: dict[str, str] | None = None,
    enable_stateful: bool = False,
    stateful_binary: str | None = None,
    stateful_integration: str | None = None,
) -> SeededAuth | None:
    if env is None:
        return None

    if stateful_integration is None:
        stateful_integration = (
            STATEFUL_INTEGRATION_FULL if enable_stateful else STATEFUL_INTEGRATION_NONE
        )

    for key in ["HOME", "CODEX_HOME", "XDG_CONFIG_HOME", "XDG_CACHE_HOME"]:
        if not ensure_safe_directory(Path(env[key])):
            raise UnsafeNestedCodexHome(f"unsafe nested Codex directory for {key}: {env[key]}")

    target_auth = Path(env["CODEX_HOME"]) / AUTH_FILE_NAME
    target_config = Path(env["CODEX_HOME"]) / CODEX_CONFIG_FILE_NAME
    target_config_toml = Path(env["CODEX_HOME"]) / CODEX_CONFIG_TOML_FILE_NAME
    source = os.environ if source_env is None else source_env
    source_config_toml = source_codex_config_toml_path(source)
    base_config_toml = None
    if (
        source_config_toml is not None
        and source_config_toml.resolve() != target_config_toml.resolve()
    ):
        try:
            base_config_toml = source_config_toml.read_text(encoding="utf-8")
        except OSError:
            base_config_toml = None

    if stateful_integration != STATEFUL_INTEGRATION_NONE:
        if not stateful_binary:
            raise UnsafeNestedCodexHome("stateful_binary is required for stateful Codex setup")
        write_stateful_codex_integration(
            env,
            stateful_binary,
            include_skill=stateful_integration == STATEFUL_INTEGRATION_FULL,
            base_config=base_config_toml,
        )
    else:
        remove_stateful_codex_integration(env)

    source_auth = source_codex_auth_path(source)
    source_config = source_codex_config_path(source)
    if source_auth is None:
        remove_stale_nested_auth(target_auth)
        remove_stale_nested_auth(target_config)
        if stateful_integration == STATEFUL_INTEGRATION_NONE:
            remove_stale_nested_auth(target_config_toml)
        return None

    if source_auth.resolve() == target_auth.resolve():
        return None

    try:
        source_digest = file_digest(source_auth)
        remove_stale_nested_auth(target_auth)
        shutil.copy2(source_auth, target_auth)
        extra_files = []
        if source_config is None:
            remove_stale_nested_auth(target_config)
        elif source_config.resolve() != target_config.resolve():
            config_digest = file_digest(source_config)
            remove_stale_nested_auth(target_config)
            shutil.copy2(source_config, target_config)
            extra_files.append((target_config, config_digest))
        if stateful_integration == STATEFUL_INTEGRATION_NONE:
            if source_config_toml is None:
                remove_stale_nested_auth(target_config_toml)
            elif source_config_toml.resolve() != target_config_toml.resolve():
                provider_config_toml = codex_provider_config_fragment(base_config_toml)
                remove_stale_nested_auth(target_config_toml)
                if provider_config_toml:
                    write_text_file(target_config_toml, f"{provider_config_toml.rstrip()}\n")
                    extra_files.append((target_config_toml, file_digest(target_config_toml)))
        copied_auth = SeededAuth(
            path=target_auth,
            digest=source_digest,
            extra_files=extra_files,
        )
    except OSError:
        remove_stale_nested_auth(target_auth)
        remove_stale_nested_auth(target_config)
        if stateful_integration == STATEFUL_INTEGRATION_NONE:
            remove_stale_nested_auth(target_config_toml)
        return None
    return copied_auth


def remove_stale_nested_auth(path: Path) -> None:
    try:
        if path.exists() or path.is_symlink():
            path.unlink()
    except OSError:
        pass


def cleanup_seeded_auth(seeded_auth: SeededAuth | None) -> None:
    if seeded_auth is None:
        return
    for path, digest in seeded_auth.files:
        if path.is_symlink():
            continue
        try:
            if file_digest(path) == digest:
                path.unlink()
        except (FileNotFoundError, OSError):
            pass


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--task-json", required=True)
    parser.add_argument("--workspace", required=True)
    parser.add_argument("--mode", choices=["stateful", "no-state"], required=True)
    parser.add_argument("--stateful-binary", required=True)
    parser.add_argument("--session-id")
    parser.add_argument("--workspace-id")
    parser.add_argument("--benchmark-model", default=DEFAULT_BENCHMARK_MODEL)
    parser.add_argument("--benchmark-reasoning-effort", default=DEFAULT_BENCHMARK_REASONING_EFFORT)
    parser.add_argument("--benchmark-model-context-window", type=int)
    parser.add_argument("--benchmark-max-turns", type=int)
    parser.add_argument("--enable-native-subagent", action="store_true")
    parser.add_argument(
        "--subagent-min-count",
        type=int,
        default=DEFAULT_NATIVE_SUBAGENT_MIN_COUNT,
    )
    parser.add_argument("--disable-bundled-skills", action="store_true")
    parser.add_argument(
        "--stateful-integration",
        choices=[
            STATEFUL_INTEGRATION_FULL,
            STATEFUL_INTEGRATION_HOOKS_ONLY,
            STATEFUL_INTEGRATION_NONE,
        ],
        default=STATEFUL_INTEGRATION_FULL,
    )
    parser.add_argument("--max-resumes", type=int, default=1)
    args = parser.parse_args()

    if args.mode == "stateful" and (not args.session_id or not args.workspace_id):
        parser.error("--session-id and --workspace-id are required in stateful mode")
    if args.max_resumes < 0:
        parser.error("--max-resumes must be non-negative")
    if args.subagent_min_count < 1:
        parser.error("--subagent-min-count must be at least 1")

    task_path = Path(args.task_json).resolve()
    workspace = Path(args.workspace).resolve()
    task = json.loads(task_path.read_text())

    stateful_instruction = ""
    if args.mode == "stateful":
        stateful_instruction = f"""
Before any file modification, inspect the code enough to identify the production
file or files you plan to edit, then use the stateful MCP tools to declare a
task-level reservation covering the known file set and acquire same-session file
claims for the planned files. Do not pass a manual session id; use the current
Codex thread session provided
by the stateful hooks. If reservation declaration or claim acquisition fails, stop
without editing.
"""
    max_turns_instruction = ""
    if args.benchmark_max_turns is not None:
        max_turns_instruction = f"\n- Benchmark max turns: {args.benchmark_max_turns}.\n"
    subagent_instruction = native_subagent_prompt_instruction(
        args.enable_native_subagent,
        args.subagent_min_count,
    )

    prompt = f"""
You are one of two concurrent agents in a shared SWE-bench workspace.

Task JSON path:
{task_path}

Task:
{task["problem_statement"]}

Constraints:
- Solve only this task. Do not inspect pair.json, the other task JSON, run
  artifacts, gold patches, or benchmark metadata outside the task JSON above.
- Edit only production source files needed for the fix.
- Do not edit tests, documentation, generated files, package metadata, or
  benchmark artifacts.
- Use apply_patch for code edits. Do not use Bash, Python, Perl, sed, tee, or
  shell redirection to modify code.
- Bash is allowed for read-only inspection and test commands.
{max_turns_instruction}
{subagent_instruction}
{stateful_instruction}
When finished, leave the working tree with only the production code fix for this
task.
""".strip()

    source_env = benchmark_source_env(
        args.mode,
        args.session_id,
        preserve_stateful_session=not args.enable_native_subagent,
    )
    command = codex_command(
        workspace=workspace,
        mode=args.mode,
        stateful_binary=args.stateful_binary,
        benchmark_model=args.benchmark_model,
        benchmark_reasoning_effort=args.benchmark_reasoning_effort,
        benchmark_model_context_window=args.benchmark_model_context_window,
        enable_native_subagent=args.enable_native_subagent,
        disable_bundled_skills=args.disable_bundled_skills,
        stateful_integration=(
            args.stateful_integration if args.mode == "stateful" else STATEFUL_INTEGRATION_NONE
        ),
        base_env=source_env,
    )
    env = codex_environment(task_path=task_path, workspace=workspace, base_env=source_env)
    try:
        seeded_auth = prepare_codex_environment(
            env,
            source_env=source_env,
            enable_stateful=args.mode == "stateful",
            stateful_binary=args.stateful_binary,
            stateful_integration=(
                args.stateful_integration
                if args.mode == "stateful"
                else STATEFUL_INTEGRATION_NONE
            ),
        )
    except UnsafeNestedCodexHome as error:
        print(f"codex pair agent setup failed: {error}", file=sys.stderr)
        return 1
    try:
        return run_codex_with_resume(
            command,
            prompt,
            workspace,
            env,
            max_resumes=args.max_resumes,
        )
    finally:
        cleanup_seeded_auth(seeded_auth)


if __name__ == "__main__":
    sys.exit(main())
