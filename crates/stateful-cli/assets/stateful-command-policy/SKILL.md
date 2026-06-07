---
name: stateful-command-policy
description: Use before any Bash or shell command, file write, sandboxed test run, commit, push, or response to stateful hook denials in a repo with stateful Codex hooks
---

# Stateful Command Policy

Stateful hooks are authoritative. Pick commands that match the installed hooks before invoking tools.

## Default Write Flow

- Declare exact file intent first with `state_intent_declare` / `state.intent.declare`.
- Intent declarations replace the session's active scope in that workspace; they do not append. When adding or changing targets, redeclare the complete intended file set for that session and workspace.
- Keep declared paths narrow; prefer exact files for edits, deletes, renames, and moves.
- Edit repo files with native Codex edit tools such as `apply_patch` or Edit after exact intent and a successful same-session file lease.
- Use `<absolute-stateful-binary> sandbox run --fs write-targets --write-target <path> ... --command <cmd>` only for command-shaped writes that cannot be expressed as native edits, `--create-target <path>` for command-created files, and `--write-dir target` for the `target/` build artifact tree after exact intent declaration and a successful same-session file or directory lease matching the target. The executable must be the trusted absolute `stateful` binary installed in the hook configuration; after rebuilding stateful itself, install the rebuilt binary to that trusted path before running commands that rely on new sandbox flags.
- Use `<absolute-stateful-binary> external-run request --purpose <purpose> --write-target <external-path> --create-target <external-path> --write-dir <external-dir> --command <cmd>` for command-shaped writes whose normalized targets are outside the repo. External-run does not require intent or lease; it records the request and prints a copy-paste `external-run approve <id> --run` command for the user to approve and execute.
- Re-read a file immediately before native edits, so preserve unrelated user changes.
- `apply_patch`, `Edit`, `Write`, and `file_change` are hook-authorized only when targets are visible to stateful policy. If denied, redeclare the complete intended scope and acquire the exact same-session file lease before retrying; use sandbox-run write targets only for command-shaped writes.
- If a hook denies an action, read the denial and choose the documented alternative instead of retrying variants.

## Sandbox Examples

Examples assume `<absolute-stateful-binary>` is the trusted absolute binary installed in Codex hook configuration.

After changing sandbox-run behavior in this repo, the user must bootstrap the trusted binary outside hook-mediated Bash before Codex can use new sandbox flags:

```bash
cp target/debug/stateful <absolute-stateful-binary>
```

Read-only inspection:

```bash
<absolute-stateful-binary> sandbox run --fs read-only --network disabled --command 'rg auth crates'
```

Run a command-shaped write after declaring exact intent and acquiring the matching same-session lease:

```bash
<absolute-stateful-binary> sandbox run --fs write-targets --network enabled --write-target target/report.txt --command 'printf "%s\n" updated > target/report.txt'
```

Create a command-generated file after declaring exact intent and acquiring the matching same-session file lease:

```bash
<absolute-stateful-binary> sandbox run --fs write-targets --network enabled --create-target target/generated.txt --command 'printf "%s\n" notes > target/generated.txt'
```

Run tests after declaring directory intent such as `target/` and acquiring the matching directory lease:

```bash
<absolute-stateful-binary> intent declare --session-id <session> --workspace-id <workspace> --purpose "<purpose inferred from the user or agent instruction>" target/
<absolute-stateful-binary> mcp call state_lease_acquire '{"session_id":"<session>","workspace_id":"<workspace>","path":"target/"}'
<absolute-stateful-binary> sandbox run --fs write-targets --network enabled --write-dir target --command 'cargo test --workspace'
```

Request a repo-external write. The first command only records the request and prints the approval command; the user copies and runs that approval command:

```bash
<absolute-stateful-binary> external-run request --purpose "install rebuilt stateful binaries" --write-dir <external-install-dir> --command 'install -m 755 target/release/stateful <external-install-dir>/stateful'
```

`stateful sandbox run` targets must be repo-relative. Do not target `.git`, symlinks, paths outside the repo, or paths with control characters. Use `--write-target` for existing files, `--create-target` for new files, and `--write-dir target` only for the `target/` artifact tree. Declare directory intent with a trailing slash, `target/`, and acquire the same-session directory lease before using `--write-dir target`. `stateful external-run` is for normalized targets outside the repo and supports exact files, created files, and whole external directories after user approval. `/dev/null` is writable inside the sandbox; do not declare it as a target. `stateful sandbox run` is macOS-first and release-verified with Seatbelt. Linux bubblewrap support and `stateful external-run` support are implemented but experimental until verified in a Linux release environment.

## Prefer

- MCP or native read tools for search and inspection when available.
- `<absolute-stateful-binary> sandbox run --fs read-only --network disabled --command <cmd>` for Bash-tool command-shaped read-only inspection that needs a real shell.
- `<absolute-stateful-binary> sandbox run --fs write-targets ... --command ...` for Bash-tool command-shaped writes that need a real shell and can be limited to exact file targets, create targets, or the `target/` artifact tree.
- `<absolute-stateful-binary> sandbox run --fs write-targets --network enabled --write-dir target --command 'cargo test --workspace'` for test commands that write build artifacts and need loopback networking, after exact directory intent and a successful same-session directory lease.
- `<absolute-stateful-binary> external-run request --purpose ... --write-dir <external-dir> --command ...` for approved writes outside the repo; copy the printed approval command for the user.
- Use `stateful commit` / `stateful push` for structured commit and push flows when available.
- Stateful diagnostics through MCP tools, native tools, or sandbox-run wrappers through the trusted absolute `stateful` binary.

## Avoid In Bash

- Raw Bash is denied by stateful hooks; use a sandbox-run wrapper through the trusted absolute `stateful` binary or MCP/native tools instead. Raw read-only Bash is also denied, including commands such as `rg`, `git status`, and `sed`.
- Shell write syntax outside a sandbox-run `--command`: `>`, `>>`, heredocs, and `| tee`.
- Direct file mutation: `rm`, `mv`, `cp`, `mkdir`, `touch`, `chmod`, `chown`.
- Any generator, formatter, package manager, or script that creates, updates, deletes, or moves repo files.
- Raw mutation git commands: `git checkout`, `git switch`, `git restore`, `git reset`, `git clean`, `git apply`, `git merge`, `git rebase`.
- Raw test commands; use `sandbox run --fs write-targets --write-dir target` after exact directory intent and a successful same-session directory lease for commands that write build or test artifacts.
- Most `stateful` control commands through Bash; use MCP tools when available.
- Repo-external writes through `sandbox run`; use `external-run request` so the user sees the write scope, purpose, command, and copy-paste approval command.

## If Blocked

- Do not retry the same command with small variations.
- If the denial asks for scope, redeclare the complete intended file set for that session and workspace, acquire the exact same-session file lease, then use native Codex edit tools for repo changes.
- If raw Bash is blocked, choose MCP/native inspection, native Codex edit tools after exact file intent declaration and a successful same-session file lease for repo edits, `<absolute-stateful-binary> sandbox run --fs read-only --network disabled`, or `<absolute-stateful-binary> sandbox run --fs write-targets` for command-shaped writes.
- If a repo-external write is needed, use `<absolute-stateful-binary> external-run request ...`; do not try to bypass it with raw `cp`, `install`, `cargo install`, or shell redirection.
- If a denial mentions a stateful MCP coordination tool, prefer that MCP tool in Codex sessions.
- If no policy-compliant path is available, report the exact command and denial reason.
