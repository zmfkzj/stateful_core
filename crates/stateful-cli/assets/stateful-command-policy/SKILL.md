---
name: stateful-command-policy
description: Use when running shell commands, editing files, or responding to stateful hook denials in a repo with stateful Codex hooks
---

# Stateful Command Policy

Stateful hooks are authoritative. Pick commands that match the installed hooks before invoking tools.

## Default Write Flow

- Declare exact file intent first with `state_intent_declare` / `state.intent.declare`.
- Keep declared paths narrow; prefer exact files for edits, deletes, renames, and moves.
- Write repo files with `state_file_write` / `state.file.write` after intent. It authorizes and writes from structured arguments.
- Use `<absolute-stateful-binary> sandbox run --fs write-targets --write-target <path> ... --command <cmd>` for Bash-tool command-shaped writes. The executable must be the trusted absolute `stateful` binary installed in the hook configuration. Add `--create-target` for new files; the wrapper authorizes each target and then runs the command in the OS sandbox.
- Re-read a file immediately before `state_file_write`; it writes full contents, so preserve unrelated user changes.
- `apply_patch`, `Edit`, `Write`, and `file_change` are hook-authorized only when targets are visible to stateful policy. If denied, switch to structured write instead of retrying patch variants.
- If a hook denies an action, read the denial and choose the documented alternative instead of retrying variants.

## Prefer

- MCP or native read tools for search and inspection when available.
- `<absolute-stateful-binary> sandbox run --fs read-only --network disabled --command <cmd>` for Bash-tool command-shaped read-only inspection that needs a real shell.
- `<absolute-stateful-binary> sandbox run --fs write-targets ... --command ...` for Bash-tool command-shaped writes that need a real shell but can be limited to exact file targets.
- Validation: use `state_validation_run` / `state.validation.run` in Codex sessions, or `stateful validate <profile>` outside hook-mediated Bash.
- Stateful diagnostics through MCP tools, native tools, validation profiles, or sandbox-run wrappers through the trusted absolute `stateful` binary.

## Avoid In Bash

- Raw Bash is denied by stateful hooks; use a sandbox-run wrapper through the trusted absolute `stateful` binary, MCP/native tools, or validation profiles instead.
- Shell write syntax: `>`, `>>`, heredocs, and `| tee`.
- Direct file mutation: `rm`, `mv`, `cp`, `mkdir`, `touch`, `chmod`, `chown`.
- Any generator, formatter, package manager, or script that creates, updates, deletes, or moves repo files.
- Raw mutation git commands: `git checkout`, `git switch`, `git restore`, `git reset`, `git clean`, `git apply`, `git merge`, `git rebase`.
- Raw test commands; use validation profiles for commands that write build or test artifacts.
- Most `stateful` control commands through Bash; use MCP tools when available.

## If Blocked

- Do not retry the same command with small variations.
- If the denial asks for scope, declare or narrow intent, then use `state_file_write` for repo changes.
- If raw Bash is blocked, choose MCP/native inspection, structured MCP write, `<absolute-stateful-binary> sandbox run --fs read-only --network disabled`, `<absolute-stateful-binary> sandbox run --fs write-targets`, or a validation profile.
- If a denial mentions a structured tool, prefer the stateful MCP tool in Codex sessions.
- If no policy-compliant path is available, report the exact command and denial reason.
