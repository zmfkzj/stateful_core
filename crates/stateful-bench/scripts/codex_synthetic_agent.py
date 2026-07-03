#!/usr/bin/env python3
"""Launch one Codex agent for a synthetic concurrent document edit task."""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import shutil
import subprocess
import stat
import sys
from pathlib import Path


DEFAULT_BENCHMARK_MODEL = "gpt-5.4-mini"
DEFAULT_BENCHMARK_REASONING_EFFORT = "low"
NESTED_CODEX_HOME_ROOT_ENV = "STATEFUL_NESTED_CODEX_HOME_ROOT"
AUTH_FILE_NAME = "auth.json"


class SeededAuth:
    def __init__(self, path: Path, digest: str) -> None:
        self.path = path
        self.digest = digest


class UnsafeNestedCodexHome(RuntimeError):
    pass


def toml_string(value: str) -> str:
    return json.dumps(value)


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
    hook_prefix = f'"{stateful_binary}" hook'
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


def codex_command(
    workspace: Path,
    mode: str,
    stateful_binary: str,
    benchmark_model: str = DEFAULT_BENCHMARK_MODEL,
    benchmark_reasoning_effort: str = DEFAULT_BENCHMARK_REASONING_EFFORT,
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
    if not nested_benchmark:
        command.extend(["-c", "sandbox_workspace_write.network_access=true"])
    if mode == "stateful":
        for override in stateful_hook_overrides(stateful_binary):
            command.extend(["-c", override])
    command.append("-")
    return command


def path_fragment(value: str) -> str:
    fragment = "".join(
        character if character.isalnum() or character in "._-" else "-"
        for character in str(value)
    ).strip(".-")
    return fragment or "item"


def codex_environment(
    pair_id: str,
    agent_id: str,
    base_env: dict[str, str] | None = None,
) -> dict[str, str] | None:
    source_env = os.environ if base_env is None else base_env
    root = source_env.get(NESTED_CODEX_HOME_ROOT_ENV)
    if not root:
        return None

    env = dict(source_env)
    home = Path(root) / path_fragment(pair_id) / path_fragment(agent_id) / "home"
    env["HOME"] = str(home)
    env["CODEX_HOME"] = str(home / ".codex")
    env["XDG_CONFIG_HOME"] = str(home / ".config")
    env["XDG_CACHE_HOME"] = str(home / ".cache")

    system_cert = Path("/etc/ssl/cert.pem")
    if not env.get("SSL_CERT_FILE") and system_cert.is_file():
        env["SSL_CERT_FILE"] = str(system_cert)

    return env


def source_codex_auth_path(source_env: dict[str, str]) -> Path | None:
    codex_home = source_env.get("CODEX_HOME")
    if codex_home:
        auth_path = Path(codex_home) / AUTH_FILE_NAME
        if auth_path.is_file():
            return auth_path

    home = source_env.get("HOME")
    if home:
        auth_path = Path(home) / ".codex" / AUTH_FILE_NAME
        if auth_path.is_file():
            return auth_path

    return None


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
) -> SeededAuth | None:
    if env is None:
        return None
    for key in ["HOME", "CODEX_HOME", "XDG_CONFIG_HOME", "XDG_CACHE_HOME"]:
        if not ensure_safe_directory(Path(env[key])):
            raise UnsafeNestedCodexHome(f"unsafe nested Codex directory for {key}: {env[key]}")

    target_auth = Path(env["CODEX_HOME"]) / AUTH_FILE_NAME
    source = os.environ if source_env is None else source_env
    source_auth = source_codex_auth_path(source)
    if source_auth is None:
        remove_stale_nested_auth(target_auth)
        return None

    if source_auth.resolve() == target_auth.resolve():
        return None

    try:
        source_digest = file_digest(source_auth)
        remove_stale_nested_auth(target_auth)
        shutil.copy2(source_auth, target_auth)
        copied_auth = SeededAuth(path=target_auth, digest=source_digest)
    except OSError:
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
    path = seeded_auth.path
    if path.is_symlink():
        return
    try:
        if file_digest(path) == seeded_auth.digest:
            path.unlink()
    except (FileNotFoundError, OSError):
        pass


def scenario_metadata(pair: dict) -> dict:
    raw = pair.get("task_a", {}).get("test_patch") or "{}"
    return json.loads(raw)


def agent_metadata_key(agent_id: str) -> str:
    return agent_id.replace("-", "_")


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--task-json", required=True)
    parser.add_argument("--workspace", required=True)
    parser.add_argument("--mode", choices=["stateful", "no-state"], required=True)
    parser.add_argument("--stateful-binary", required=True)
    parser.add_argument("--agent-id")
    parser.add_argument("--workspace-id")
    parser.add_argument("--benchmark-model", default=DEFAULT_BENCHMARK_MODEL)
    parser.add_argument("--benchmark-reasoning-effort", default=DEFAULT_BENCHMARK_REASONING_EFFORT)
    parser.add_argument("--chaos", action="store_true")
    parser.add_argument("--pair-json")
    parser.add_argument(
        "--coordination-profile",
        choices=[
            "no_state",
            "stateful_without_claim",
            "stateful_without_replay",
            "stateful_without_resume",
            "stateful_without_commit_tracking",
            "stateful_full",
        ],
    )
    args = parser.parse_args()

    if args.mode == "stateful" and (not args.agent_id or not args.workspace_id):
        parser.error("--agent-id and --workspace-id are required in stateful mode")
    if args.chaos and (not args.pair_json or not args.agent_id):
        parser.error("--pair-json and --agent-id are required with --chaos")

    task_path = Path(args.task_json).resolve()
    workspace = Path(args.workspace).resolve()
    task = json.loads(task_path.read_text())
    pair_id = str(task.get("instance_id", "single-task"))
    coordination_profile = args.coordination_profile
    if coordination_profile is None:
        coordination_profile = "stateful_full" if args.mode == "stateful" else "no_state"

    if args.mode == "stateful":
        if coordination_profile == "stateful_without_claim":
            stateful_instruction = f"""
Stateful coordination ablation profile: stateful_without_claim.
- Do not declare file reservation before modifying doc.txt.
- Still use the stateful hooks and observe whether the write is blocked or loses a peer edit.
- If a write is denied, stop without repairing the document.
"""
        else:
            stateful_instruction = f"""
Stateful coordination:
- Before your first modification to doc.txt, run exactly:

    {args.stateful_binary} reservation declare --agent-id {args.agent_id} --workspace-id {args.workspace_id} --purpose "coordinate chaos benchmark edit" doc.txt

- Use this exact agent id and workspace id.
- If a write is denied, read doc.txt again, run the declaration again, and retry once while preserving already visible content.
"""
        if coordination_profile == "stateful_without_replay":
            stateful_instruction += """
Ablation: do not deduplicate repeated op_id deliveries. Apply replayed operations as if every delivery is unique.
"""
        elif coordination_profile == "stateful_without_resume":
            stateful_instruction += """
Ablation: do not perform reconnect/resume repair. If the document changes while you are offline or denied, do not re-read before retrying.
"""
        elif coordination_profile == "stateful_without_commit_tracking":
            stateful_instruction += """
Ablation: do not update persisted_doc.txt and do not use commit/persistence context to repair stale full-document writes.
"""
    else:
        stateful_instruction = """
No stateful coordination is available in this run. Still preserve any content already visible in doc.txt.
"""

    if args.chaos:
        pair_path = Path(args.pair_json).resolve()
        pair = json.loads(pair_path.read_text())
        pair_id = str(pair.get("pair_id", pair_id))
        metadata = scenario_metadata(pair)
        operation = metadata.get(agent_metadata_key(args.agent_id), {})
        stale_snapshot = workspace / f"stale_snapshot_{args.agent_id}.txt"
        persisted_doc = workspace / "persisted_doc.txt"
        chaos_instruction = stateful_instruction
        if args.mode == "no-state":
            chaos_instruction = """
No stateful coordination is available in this run.
For this chaos run, stale/replay semantics override ordinary preservation:
- Do not repair or merge peer edits that are absent from your stale snapshot.
- If delivery_order contains your agent id multiple times, replay your operation that many times.
- If your operation has an op_id replay, do not make it idempotent in no-state mode.
"""
        prompt = f"""
You are {args.agent_id} in a multi-agent chaos trace benchmark over a shared document.

Task JSON path:
{task_path}

Pair metadata path:
{pair_path}

Chaos scenario:
{metadata.get("scenario")}

Full chaos metadata:
{json.dumps(metadata, indent=2, sort_keys=True)}

Coordination profile:
{coordination_profile}

Your operation metadata:
{json.dumps(operation, indent=2, sort_keys=True)}

Trace constraints:
- Edit only doc.txt, and persisted_doc.txt only when the scenario requires reload convergence.
- You may read the Pair metadata path above and the Task JSON path above.
- You may read {stale_snapshot} if it exists; it is your deliberately stale snapshot.
- Do not inspect benchmark run artifacts, expected output files, other pair directories, or any .stateful_bench path other than the two JSON paths above and your stale snapshot file.
- Use apply_patch for edits. Do not use shell redirection, Python, Perl, sed, tee, or other shell write tricks to modify doc.txt or persisted_doc.txt.
- Operation kind notes: full_overwrite replaces the whole document; insert_after inserts one line after an anchor and may include an occurrence field for duplicate anchors; delete_line removes exact matching lines; replace_line replaces an exact line with replacement; move_line moves an exact line after another line; replay_many applies the same op_id count times in no-state but once in stateful; noop only observes.
- In no-state mode, execute your operation as a stale/replayed delivery trace: if your stale snapshot exists, base your write on it even when live doc.txt has changed.
- In stateful mode, declare reservation, read live doc.txt immediately before writing, preserve visible peer edits, make duplicate op_id replays idempotent, and preserve inserts whose anchor was concurrently deleted.
- If {persisted_doc} exists in stateful mode, keep it converged with live doc.txt after your write.
{chaos_instruction}
When finished, leave only the requested chaos trace document changes.
""".strip()
    else:
        prompt = f"""
You are one of multiple concurrent Codex agents in a shared synthetic document-editing benchmark.

Task JSON path:
{task_path}

Task:
{task["problem_statement"]}

Constraints:
- Edit only doc.txt.
- Read doc.txt before editing.
- Use apply_patch for edits. Do not use shell redirection, Python, Perl, sed, tee, or other shell write tricks to modify doc.txt.
- Do not inspect pair.json, the other task JSON, run artifacts, benchmark metadata, gold outputs, or any .stateful_bench path except the Task JSON path above.
- Do not create new files.
- If an apply_patch attempt fails because another agent edited first, read doc.txt again and retry without deleting the other visible edit.
- When the task asks for a line containing exactly a token such as A, B, edit, remote, offline, or inserted, the full line content must be only that token.
{stateful_instruction}
When finished, leave the working tree with only the requested doc.txt change.
""".strip()

    source_env = dict(os.environ)
    command = codex_command(
        workspace=workspace,
        mode=args.mode,
        stateful_binary=args.stateful_binary,
        benchmark_model=args.benchmark_model,
        benchmark_reasoning_effort=args.benchmark_reasoning_effort,
        base_env=source_env,
    )
    env = codex_environment(pair_id=pair_id, agent_id=args.agent_id or "agent", base_env=source_env)
    try:
        seeded_auth = prepare_codex_environment(env, source_env=source_env)
    except UnsafeNestedCodexHome as error:
        print(f"codex synthetic agent setup failed: {error}", file=sys.stderr)
        return 1
    try:
        completed = subprocess.run(
            command,
            input=prompt,
            text=True,
            cwd=workspace,
            check=False,
            env=env,
        )
        return completed.returncode
    finally:
        cleanup_seeded_auth(seeded_auth)


if __name__ == "__main__":
    sys.exit(main())
