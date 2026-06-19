---
name: stateful-command-policy
description: Use before any Bash or shell command, file write, sandboxed test run, commit, push, or response to stateful hook denials in a repo with stateful Codex hooks
---

# Stateful Command Policy

Stateful hooks are authoritative. Pick commands that match the installed hooks before invoking tools.

## Default Write Flow

- Declare exact file intent first with `state_intent_declare` / `state.intent.declare`.
- Installed Codex config auto-approves Stateful MCP tools, but prompts for `stateful external-run request` through Codex execpolicy rules. Treat that prompt as the approval boundary for validating the external write scope and running the external command.
- Intent declarations add to the session's active scope in that workspace; declaring a build/test directory does not remove earlier file scopes. When adding targets, declare each active file or directory you still need before acquiring leases.
- Keep declared paths narrow; prefer exact files for edits, deletes, renames, and moves.
- Edit repo files with native Codex edit tools such as `apply_patch` or Edit after exact intent and a successful same-session file lease.
- A directory lease for command-shaped artifact writes does not authorize native edits to individual files. For native edits, declare and lease each exact file path first.
- Treat `tmp/` as disposable scratch space only. Anything under `tmp/` must be safe to delete at any time without breaking future work, handoff context, benchmark comparison, or review evidence. If an artifact must survive cleanup, create a separate purpose-named path and use exact `--write-target` / `--create-target` file scopes; when it should stay untracked, update `.gitignore` after declaring and leasing that exact file.
- Use `<absolute-stateful-binary> sandbox run --fs write-targets --write-target <path> ... --command <cmd>` only for command-shaped writes that cannot be expressed as native edits, `--create-target <path>` for command-created files, and `--write-dir tmp/<purpose>` only for disposable artifact directories after exact intent declaration and a successful same-session directory lease matching the exact trailing-slashed directory passed to the sandbox. Use `--fs build --write-dir tmp/<purpose>` for standard build/test commands after declaring and leasing that scoped tmp child; the build profile rejects root `tmp`, sets standard temp variables under `tmp/<purpose>/.stateful-tmp`, and sets Cargo output to `tmp/<purpose>/target`. Configure non-Cargo tool caches and output directories under the same scoped tmp directory when they do not honor standard temp variables. The executable must be the trusted absolute `stateful` binary installed in the hook configuration; after rebuilding stateful itself, install the rebuilt binary to that trusted path before running commands that rely on new sandbox flags.
- Use `<absolute-stateful-binary> sandbox run --fs git --network disabled --command 'git <args>'` for local git operations and `--network enabled` only for networked git operations such as `fetch`, `pull`, or `push`. The git profile accepts a single `git ...` command, rejects explicit write targets, grants sandboxed write access to the repo worktree plus private transient git state, and protects persistent config/hooks while rejecting shell-dispatching options, path or exec overrides, inline/config-env config, disallowed subcommands such as `init` and `submodule`, branch upstream/tracking persistence, `push -u`, `rebase --exec`, grep pager dispatch, archive/fetch/push exec overrides, and config-mutating `git remote` subcommands such as `add`, `set-url`, `rename`, and `remove`.
- Put nested benchmark run artifacts under `target/` when using `sandbox run-nested-codex-benchmark`. Hook authorization requires the installed trusted binary to be built with the `codex-benchmark` feature.
- For Docker-backed nested benchmarks, pass --docker-socket /absolute/path/to/docker.sock so only that Unix socket is exposed to the nested benchmark sandbox; the path must be absolute, exist, and be a Unix socket.
- Use `<absolute-stateful-binary> external-run request --purpose <purpose> --write-target <external-path> --create-target <external-path> --write-dir <external-dir> --command <cmd>` for command-shaped writes whose normalized targets are outside the repo. External-run does not require intent or lease; after Codex approval, it validates the external scope and runs the command directly.
- Re-read a file immediately before native edits, so preserve unrelated user changes.
- `apply_patch`, `Edit`, `Write`, and `file_change` are hook-authorized only when targets are visible to stateful policy. If denied, declare the missing exact scope and acquire the exact same-session file lease before retrying; use sandbox-run write targets only for command-shaped writes.
- If a hook denies an action, read the denial and choose the documented alternative instead of retrying variants.

## Pick The Sandbox Profile First

Choose the narrowest existing entry point before writing the command:

| Need | Use | Do not use |
|------|-----|------------|
| Inspect files with shell tools | `sandbox run --fs read-only --network disabled` | raw Bash, networked read-only |
| Inspect processes | `sandbox process find ...` | `ps`, `pgrep`, or process checks inside `sandbox run --command` |
| Run any `git ...` command, including PR preparation | `sandbox run --fs git --network disabled --command 'git <args>'` for local git, `--network enabled` only for remote git such as `fetch`, `pull`, or `push` | raw git, `write-targets`, `build`, or explicit write targets |
| Run builds, tests, package managers, or generators whose outputs are disposable | `sandbox run --fs build --network enabled --write-dir tmp/<purpose> --command <cmd>` after declaring and leasing that scoped tmp child | raw test/build commands, root `tmp`, retained artifacts under `tmp/` |
| Run a non-git command-shaped repo write | `sandbox run --fs write-targets` with exact `--write-target`, `--create-target`, or scoped disposable `--write-dir tmp/<purpose>` after matching intent and lease | native edits without a file lease, broad write dirs, git commands |
| Run nested Codex benchmark agents | `sandbox run-nested-codex-benchmark ...` | generic relaxed profiles or wrapping it inside another sandbox |
| Request command-shaped writes outside the repo | `external-run request ...` | `sandbox run` |

PR workflows still use the git profile for git work: `status`, `diff`, `log`, `branch`, `switch`/`checkout`, `add`, `commit`, `merge`, `rebase`, `tag`, and `push`. Do not use `--fs write-targets` just because git mutates the worktree or `.git`; the git profile owns that scope. Non-git network CLIs such as PR-hosting tools are not covered by the git profile. If no listed profile matches the needed command, stop and report the exact unsupported command instead of trying raw Bash or a near-miss profile.

## Tmp Retention Rule

`tmp/` means temporary. Store only cache files, test/build outputs, throwaway logs, and other artifacts that can disappear between commands. Do not put plans, notes, generated fixtures needed by later tasks, benchmark baselines, installable binaries, review evidence, or user-requested deliverables in `tmp/`.

If deleting the artifact would change future work, do not use `tmp/`. Create a narrow repo-relative path named for the purpose, use exact `--write-target` or `--create-target` file scopes for command-shaped writes, and add that path to `.gitignore` when the artifact should not be committed. Do not use `--write-dir` for retained repo artifacts; `--write-dir` is limited to disposable `tmp/` subtrees.

## Sandbox Examples

Examples assume `<absolute-stateful-binary>` is the trusted absolute binary installed in Codex hook configuration.

After changing sandbox command behavior in this repo, the user must bootstrap the trusted binary outside hook-mediated Bash before Codex can use new sandbox flags or new hook-authorized sandbox command forms such as `sandbox process find`:

```bash
cp target/debug/stateful <absolute-stateful-binary>
```

Read-only inspection:

```bash
<absolute-stateful-binary> sandbox run --fs read-only --network disabled --command 'rg auth crates'
<absolute-stateful-binary> sandbox process find --name python3 --contains denovo_codex_agent
<absolute-stateful-binary> sandbox process find --name stateful-bench --process-group <pgid>
```

Run a git operation:

```bash
<absolute-stateful-binary> sandbox run --fs git --network disabled --command 'git status --short'
<absolute-stateful-binary> sandbox run --fs git --network disabled --command 'git commit -m "Update command policy"'
<absolute-stateful-binary> sandbox run --fs git --network enabled --command 'git fetch --all'
<absolute-stateful-binary> sandbox run --fs git --network enabled --command 'git push origin HEAD'
```

Run a command-shaped write after declaring exact intent and acquiring the matching same-session lease:

```bash
<absolute-stateful-binary> sandbox run --fs write-targets --write-target docs/report.txt --command 'printf "%s\n" updated > docs/report.txt'
```

Create a command-generated file after declaring exact intent and acquiring the matching same-session file lease:

```bash
<absolute-stateful-binary> sandbox run --fs write-targets --create-target reports/generated.txt --command 'printf "%s\n" notes > reports/generated.txt'
```

Run tests after declaring a scoped tmp child directory intent such as `tmp/test-run/` and acquiring the matching directory lease. Build outputs are disposable by policy because the build profile writes under that scoped tmp child:

Use MCP tools such as `state_intent_declare` and `state_lease_acquire` to prepare the scoped tmp child directory intent and lease before invoking Bash. The `intent declare` and `mcp call` CLI forms are not hook-approved raw Bash commands in Codex sessions.

```bash
<absolute-stateful-binary> sandbox run --fs build --network enabled --write-dir tmp/test-run --command 'cargo test --workspace'
```

Run nested Codex benchmark agents only through the dedicated benchmark profile. This is not a general relaxed profile and must be the outermost sandbox command; do not wrap it inside another `stateful sandbox run`. It requires a trusted binary built with `codex-benchmark`, `--purpose`, `--write-dir target`, `--codex-home-root target/...`, and exactly one `--command`. The profile enables network, grants write access only under the `target/` artifact tree and the nested Codex home root under `target/`, and sets `STATEFUL_NESTED_CODEX_HOME_ROOT` so benchmark launchers can create per-agent `HOME`, `CODEX_HOME`, and XDG directories there. This command is currently supported only on macOS; Linux support needs a separately verified profile.

```bash
<absolute-stateful-binary> sandbox run-nested-codex-benchmark --purpose "run nested Codex chaos benchmark" --write-dir target --codex-home-root target/nested-codex-homes/run-1 --timeout-seconds 120 --command '<benchmark-command>'
```

Request a repo-external write. Codex prompts on `external-run request`; after approval, the command runs directly:

```bash
<absolute-stateful-binary> external-run request --purpose "install rebuilt stateful binaries" --write-dir <external-install-dir> --command 'install -m 755 target/release/stateful <external-install-dir>/stateful'
```

`stateful sandbox run --fs write-targets` targets must be repo-relative. Do not target `.git`, symlinks, paths outside the repo, or paths with control characters. Use `--write-target` for existing files, `--create-target` for new files, and `--write-dir tmp/<purpose>` only for explicitly scoped disposable artifact directories. Declare directory intent with a trailing slash for the exact directory, such as `tmp/reports/`, and acquire the same-session directory lease before using that `--write-dir`; `--write-dir` is limited to children of the `tmp/` artifact tree and rejects root `tmp`. Use `--fs build --write-dir tmp/<purpose>` for standard build/test commands. The read-only profile always requires `--network disabled`. Process inspection must use `stateful sandbox process find`; raw `ps` and `pgrep` inside `sandbox run --command` are disallowed. The git profile manages repo write scope itself; do not pass write targets with `--fs git`, and do not use it for config-mutating remote setup such as `git remote add` or `git remote set-url`. `stateful external-run` is for normalized targets outside the repo and supports exact files, created files, and whole external directories after Codex approval on the request. `/dev/null` is writable inside the sandbox; do not declare it as a target. `stateful sandbox run` is macOS-first and release-verified with Seatbelt. Linux bubblewrap support and `stateful external-run` support are implemented but experimental until verified in a Linux release environment.

## Prefer

- MCP or native read tools for search and inspection when available.
- `<absolute-stateful-binary> sandbox run --fs read-only --network disabled --command <cmd>` for Bash-tool command-shaped read-only inspection that needs a real shell.
- `<absolute-stateful-binary> sandbox process find --contains <literal>` or `--name <comm>` with optional `--pid`, `--parent-pid`, or `--process-group` selectors for process checks.
- `<absolute-stateful-binary> sandbox run --fs write-targets ... --command ...` for Bash-tool command-shaped writes that need a real shell and can be limited to exact file targets, create targets, or explicitly scoped disposable `tmp/` artifact directories.
- `<absolute-stateful-binary> sandbox run --fs build --network enabled --write-dir tmp/<purpose> --command 'cargo test --workspace'` for build or test commands that write disposable build artifacts, after exact scoped tmp child directory intent and a successful same-session directory lease.
- `<absolute-stateful-binary> sandbox run --fs git --network disabled --command 'git <args>'` for local git operations such as status, add, commit, checkout, restore, reset, merge, rebase, and clean; use `--network enabled` for fetch, pull, push, and other remote git operations. Remote metadata mutation such as `git remote add` and `git remote set-url` is rejected.
- `<absolute-stateful-binary> sandbox run-nested-codex-benchmark --purpose ... --write-dir target --codex-home-root target/... --command ...` for nested Codex benchmark runs that need network and isolated per-agent Codex homes under `target/`.
- `<absolute-stateful-binary> external-run request --purpose ... --write-dir <external-dir> --command ...` for approved writes outside the repo; Codex prompts before running the request.
- Stateful diagnostics through MCP tools, native tools, or sandbox-run wrappers through the trusted absolute `stateful` binary.

## Avoid In Bash

- Raw Bash is denied by stateful hooks; use a sandbox-run wrapper through the trusted absolute `stateful` binary or MCP/native tools instead. Raw read-only Bash is also denied, including commands such as `rg`, `git status`, and `sed`.
- Outer shell wrappers around the sandbox command: environment assignments, command substitution, outer redirects, outer pipelines, multiple commands, duplicate `--command`, or an untrusted executable path.
- Shell write syntax outside a sandbox-run `--command`: `>`, `>>`, heredocs, and `| tee`.
- Direct file mutation: `rm`, `mv`, `cp`, `mkdir`, `touch`, `chmod`, `chown`.
- Raw process inspection such as `ps`, `pgrep`, `ps auxww`, `ps -ef`, `ps -eo pid,args`, or `ps e...` inside `sandbox run --command`; use `stateful sandbox process find` instead.
- Any generator, formatter, package manager, or script that creates, updates, deletes, or moves repo files.
- Storing anything in `tmp/` that must survive cleanup or be available to later work; use exact retained file targets in a purpose-named path and `.gitignore` as needed.
- `sandbox run --fs read-only --network enabled`; the read-only profile rejects network access.
- `sandbox run --fs build` without exactly one `--write-dir tmp/<purpose>`, or with explicit `--write-target` / `--create-target`.
- Raw git commands; use `sandbox run --fs git` for both read-only and mutating git operations.
- Raw test commands; use `sandbox run --fs build --write-dir tmp/<purpose>` after exact scoped tmp child directory intent and a successful same-session directory lease for commands that write build or test artifacts.
- Generic relaxed sandbox profiles for nested Codex. Use only `sandbox run-nested-codex-benchmark`; it is intentionally narrower than a reusable relaxed profile.
- Most `stateful` control commands through Bash; use MCP tools when available.
- Repo-external writes through `sandbox run`; use `external-run request` so Codex prompts with the write scope, purpose, and command before execution.

## If Blocked

- Do not retry the same command with small variations.
- If the denial asks for scope, declare the missing exact file scope, acquire the exact same-session file lease, then use native Codex edit tools for repo changes.
- If lease acquisition reports `lease_conflict`, do not retry the lease call. To wait for the path, call `state.intent.request` with a stable `request_id`, the denied `action`, `path`, and `purpose`; then poll `state.notifications.poll` or `state.resume.next` for the reservation. When reserved, reread the target, call `state.intent.claim` with the `wait_id`, and only then reacquire or retry the write.
- If a denial includes `wait_id`, `queue_position`, or reservation guidance, follow that wait queue protocol: poll `state.notifications.poll` / `state.resume.next`, reread the target after reservation, then call `state.intent.claim` before retrying the write.
- If raw Bash is blocked, choose MCP/native inspection, native Codex edit tools after exact file intent declaration and a successful same-session file lease for repo edits, `<absolute-stateful-binary> sandbox run --fs read-only --network disabled`, `<absolute-stateful-binary> sandbox run --fs write-targets` for command-shaped writes, or `<absolute-stateful-binary> sandbox run --fs git` for git operations.
- If an artifact needs to be retained and `tmp/` is the only authorized directory, stop and declare/lease a purpose-named path instead; add `.gitignore` intent and lease when that retained artifact should remain untracked.
- If a repo-external write is needed, use `<absolute-stateful-binary> external-run request ...`; do not try to bypass it with raw `cp`, `install`, `cargo install`, or shell redirection.
- If a denial mentions a stateful MCP coordination tool, prefer that MCP tool in Codex sessions.
- If a denial says the running server does not support sandbox write directories, restart the stateful server with a binary that supports sandbox write-directory authorization, then retry after rereading and reacquiring the needed lease.
- If `run-nested-codex-benchmark` is denied for missing feature support, install or rebuild the trusted binary with the `codex-benchmark` feature before retrying.
- If MCP tools are unavailable, do not try `stateful mcp call` through a read-only network-enabled sandbox; the read-only profile requires network disabled and most stateful control commands are not hook-approved raw Bash.
- If no policy-compliant path is available, report the exact command and denial reason.
