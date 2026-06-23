---
name: stateful-command-policy
description: Detailed procedure for using Stateful MCP coordination, leases, sandbox profiles, and hook-denial recovery after a Stateful rule or denial says this policy applies
---

# Stateful Command Policy

This skill is the procedural manual. Rules decide when Stateful policy applies; this file explains how to satisfy that policy without probing through Bash, bypassing hooks, or widening write scope.

## Skill Authoring Boundary

This skill may teach agents how to use `stateful` correctly: inspect current state, declare intent, acquire and release leases, claim reservations, choose sandbox profiles, and recover from hook denials. Do not add benchmark-specific success strategies, direct orchestration plans, role assignments, or mandates for how main agents and subagents should divide domain work. Work-allocation guidance belongs here only when it is necessary to avoid `stateful` coordination errors such as overlapping leases, shared scratch paths, stale reservations, or same-session lease denials.

Benchmark prompts must not be injected through this skill except for instructions that are explicitly part of testing concurrent-work behavior. The skill must not try to improve benchmark patch quality by telling agents which domain roles to spawn, which files to assign, or which non-stateful task strategy to follow.

## Role Split With Rules

- Rules own activation: short always-apply or TTSR guidance tells agents when to consult this skill.
- This skill owns procedure: exact MCP flow, sandbox profile selection, denial recovery, and edge cases.
- Hooks own enforcement: hook allow means proceed; hook deny or unavailable means stop and choose the documented alternative.

## Default Write Flow

- First inspect current state with canonical Stateful MCP tool names: `state_context_render` or `state_current_read`. Confirm who is active, what this session already holds, pending reservations, and likely conflicts before choosing files, commands, or subagent work.
- Use canonical Stateful MCP tool names in guidance and reasoning: `state_context_render`, `state_current_read`, `state_session_register`, `state_intent_declare`, `state_lease_acquire`, `state_intent_request`, `state_notifications_poll`, `state_resume_next`, and `state_intent_claim`.
- If the active tool list exposes only runtime-specific tool names, call the exact shown equivalent: Codex may expose `mcp__stateful__state_current_read`-style names and OMP may expose `mcp__stateful_state_current_read`-style names. Treat those wrappers as host-specific aliases for the canonical `state_*` tools, not as the canonical API.
- In OMP, if Stateful MCP tools are not in the active tool list yet, use `search_tool_bm25` once with a query such as `stateful state current read` to activate the installed stateful MCP tools, then call the activated runtime-specific tool names. Do not fall back to Bash for stateful coordination.
- Declare exact file intent first using `state_intent_declare`, then acquire the same-session file lease with `state_lease_acquire`. Do not run `stateful intent declare` or `stateful mcp call` through Bash.
- Installed Codex config auto-approves Stateful MCP tools, but prompts for `stateful sandbox run --fs external --purpose ...` through Codex execpolicy rules. Treat that prompt as the approval boundary for validating an absolute repo-external write scope and running the command through the external sandbox profile.
- OMP installs the stateful integration into the live OMP profile agent directory (default `~/.omp/profiles/stateful/agent`, or the explicit OMP agent directory passed to install) with the stateful extension, an always-apply `rules/stateful-required.md` rule, this `skills/stateful-command-policy/SKILL.md` manual, and the `sandbox_bash` and `external_bash` custom tools. It merges an existing `config.yml` instead of replacing it and rejects invalid YAML. Without `stateful install --agent omp --update`, existing OMP config key values are preserved and only missing Stateful keys are inserted. With `--update`, the installer overwrites the targeted OMP values to `tools.approvalMode: write`, `tools.approval.sandbox_bash: allow`, `tools.approval.external_bash: prompt`, `eval.py: false`, `eval.js: false`, `eval.rb: false`, `eval.jl: false`, and `bash.enabled: false`.
- Raw OMP Bash and Python/JavaScript/JS/Ruby/Julia eval tools are denied by Stateful hooks before command text matters, so wrapping `stateful sandbox run ...` inside raw Bash or an eval tool is still blocked. OMP supports stateful `session-start`, `pre-tool-use`, `post-tool-use`, and `stop` hook lifecycles, but not `user-prompt-submit`. For non-external sandbox command work, use `sandbox_bash`; for repo-external shell work, use `external_bash`, which asks OMP UI confirmation before spawning the trusted stateful binary with `sandbox run --fs external`. Other stateful hook authorization remains a hard authorization boundary in OMP: hook allow -> OMP allow; hook deny/unavailable -> OMP block. OMP yolo metadata does not bypass `missing_intent`, `missing_lease`, lease conflicts, raw Bash/eval-tool blocks, or any stateful denial, and must not downgrade them to warnings or allow decisions.
- OMP built-ins that do not write repo files, including `ask`, `ast_grep`, `job`, `irc`, `task`, `todo`, `report_tool_issue`, `generate_image`, and native read/search tools, are allowed by default. Other OMP-origin tools that are not built into the stateful classifier are recorded in `stateful tools list` and may be explicitly permitted with `stateful tools allow <tool>` for the enabled repo. This allowlist does not bypass hard-denied write or execution paths such as raw Bash, Python/JavaScript/JS/Ruby/Julia eval tools, or file writes without stateful authorization.
- In OMP, the extension uses the actual OMP session id (`event.sessionId` / `ctx.sessionManager.session.id`), stores it in `process.env.STATEFUL_SESSION_ID`, and the `stateful hook omp session-start` hook persists current-session files so MCP session-aware tools resolve the same OMP session. `state_session_register` -> `state_intent_declare` -> `state_lease_acquire` works from OMP current-session state without a manually pre-set env var.
- In stateful-bench DeNovo OMP runs, the adapter creates a per-instance OMP home instead of using the host default. `stateful:on` conditions install the OMP integration into that isolated home; Docker-backed stateful OMP runs use `/home/stateful` as the container runtime home so the mounted integration path is visible inside the agent. `stateful:off` / no-state conditions may still use the isolated OMP home but must not be treated as stateful sessions or repaired with Stateful MCP/sandbox guidance unless the benchmark prompt explicitly enables stateful.
- In OMP, never use raw Bash or Python/JavaScript/JS/Ruby/Julia eval tools for command-shaped work, including valid `stateful sandbox run` wrappers. Use `sandbox_bash` for read-only, write-targets, build, git, and github-pr sandbox profiles, with the same profile flags, common sandbox flags, targets, intent, and lease rules described below; use `external_bash` for `--fs external` with absolute external targets because `sandbox_bash` rejects the external profile. Do not rely on OMP yolo/write approval for shell redirects, `cp`, `rm`, generators, raw eval snippets, raw git/test commands, or raw sandbox invocations.
- Intent declarations add to the session's active scope in that workspace; declaring a build/test directory does not remove earlier file scopes. When adding targets, declare each active file or directory you still need before acquiring leases.
- Keep declared paths narrow; prefer exact files for edits, deletes, renames, and moves.
- Edit repo files using native edit tools with hook-visible targets, such as Codex `apply_patch` or Edit, after exact intent and a successful same-session file lease.
- A directory lease for command-shaped artifact writes does not authorize native edits to individual files. For native edits, declare and lease each exact file path first.
- Directory intent and directory leases authorize only `write_directory` actions; any `write_file`, delete, rename, or move authorization requires exact file intent and exact same-session file leases for every affected file path.
- Treat `tmp/` as disposable scratch space only. Anything under `tmp/` must be safe to delete at any time without breaking future work, handoff context, benchmark comparison, or review evidence. If an artifact must survive cleanup, create a separate purpose-named path and use exact `--write-target` / `--create-target` file scopes; when it should stay untracked, update `.gitignore` after declaring and leasing that exact file.
- Use `<absolute-stateful-binary> sandbox run --fs write-targets --write-target <path> ... --command <cmd>` only for command-shaped writes that cannot be expressed as native edits, `--create-target <path>` for command-created files, and `--write-dir <repo-dir>` for command-shaped directory writes after exact intent declaration and a successful same-session directory lease matching the exact trailing-slashed directory passed to the sandbox. Repo `tmp/` has no special write-dir exemption; it is a normal repo path and still needs a matching lease. Use `--fs build --write-dir <scratch-purpose>` for standard build/test commands; the build profile maps that purpose to `/tmp/stateful/<session>/<scratch-purpose>/`, sets standard temp variables under `/tmp/stateful/<session>/<scratch-purpose>/.stateful-tmp`, and sets Cargo output to `/tmp/stateful/<session>/<scratch-purpose>/target`. Configure non-Cargo tool caches and output directories under that external scratch root when they do not honor standard temp variables. The executable must be the trusted absolute `stateful` binary installed in the hook configuration; after rebuilding stateful itself, install the rebuilt binary to that trusted path before running commands that rely on new sandbox flags.
- Use `<absolute-stateful-binary> sandbox run --fs git --network disabled --command 'git <args>'` for local git operations and `--network enabled` only for networked git operations such as `fetch`, `pull`, or `push`. The git profile accepts a single `git ...` command, rejects explicit write targets, grants sandboxed write access to the repo worktree plus private transient git state, and protects persistent config/hooks while rejecting shell-dispatching options, path or exec overrides, inline/config-env config, disallowed subcommands such as `init` and `submodule`, branch upstream/tracking persistence, `push -u`, `rebase --exec`, grep pager dispatch, archive/fetch/push exec overrides, and config-mutating `git remote` subcommands such as `add`, `set-url`, `rename`, and `remove`.
- Use `<absolute-stateful-binary> sandbox run --fs github-pr --network enabled --command 'gh pr <list|view|status|create> ...'` for GitHub pull request inspection and creation. The profile accepts only one `gh pr` command, rejects explicit write targets, disables prompts, and denies browser/editor flags and PR mutation subcommands outside creation.
- Put nested benchmark run artifacts under `target/` when using `sandbox run-nested-codex-benchmark`. Hook authorization requires the installed trusted binary to be built with the `codex-benchmark` feature.
- For Docker-backed nested benchmarks, pass --docker-socket /absolute/path/to/docker.sock so only that Unix socket is exposed to the nested benchmark sandbox; the path must be absolute, exist, and be a Unix socket.
- Do not confuse Docker-backed nested benchmark sandboxing with DeNovo OMP agent Docker isolation. `sandbox run-nested-codex-benchmark --docker-socket ...` exposes only the Docker socket to the outer nested benchmark sandbox; stateful-bench DeNovo `--agent-docker-image` uses an adapter-owned image with Bun-installed `omp` and Linux `stateful`, mounts only the workspace, prompt file, and isolated OMP home into the agent container, and normalizes `$STATEFUL_HOME/config.yml` repo registry plus `repos/*.json` metadata paths to `/workspace`.
- Use `<absolute-stateful-binary> sandbox run --fs external --purpose <purpose> --write-target <absolute-external-path> --create-target <absolute-external-path> --write-dir <absolute-external-dir> --command <cmd>` for command-shaped writes whose normalized targets are outside the repo. The external profile requires `--purpose`, rejects targets resolving inside the repo, does not require repo intent or lease, prompts for Codex approval when called directly in Codex, and runs through the sandbox after approval; in OMP call the generated `external_bash` tool, which prompts before invoking this same profile internally. The profile also supports `--connect-socket`, `--allow-signal`, and `--network` for approved external operations. Do not use it to bypass repo leases; use `sandbox run --fs write-targets` for retained repo writes and `sandbox run --fs build --write-dir <scratch-purpose>` for disposable build/test scratch under `/tmp/stateful`.
- Copying, exporting, syncing, or rehydrating a directory into a repo-external worktree is a write even when file bytes match the source. External worktrees such as `~/.config/superpowers/worktrees/...` still require the external sandbox approval boundary unless the operation stays inside an already approved sandbox profile. Filter local stateful artifact directories (`.codex`, `.stateful`, `.stateful_bench`, `.stateful_core`) out of exported workspaces unless the user explicitly approved copying them.
- Re-read a file immediately before native edits, so preserve unrelated user changes.
- `apply_patch`, `Edit`, `Write`, and `file_change` are hook-authorized only when targets are visible to stateful policy. If denied, declare the missing exact scope and acquire the exact same-session file lease before retrying; use sandbox-run write targets only for command-shaped writes.
- Native edit hooks and `sandbox run --fs write-targets` release their authorized same-session leases after the write transaction completes. If you need another edit after that boundary, reread the target and reacquire or let a claimable reservation lazy-claim at the next write boundary.
- In native Codex or OMP subagents, the active tool session is the session authority. Do not repair session identity with hook commands, shell environment overrides, or hook-state archaeology. If a native edit or write-target command is denied for missing intent or same-session lease, use the same subagent session's Stateful MCP tools: `state_session_register` if needed, then `state_intent_declare` and `state_lease_acquire` for the exact path. If the active tool list exposes only runtime-specific names, call the exact shown equivalent such as `mcp__stateful__state_intent_declare` or `mcp__stateful_state_intent_declare`.
- If a hook denies an action, read the denial and choose the documented alternative instead of retrying variants.

## Subagent Write Recovery

When a native Codex or OMP subagent hits `apply_patch writes require ... same-session file lease`, `Target is outside active intent scope`, `missing_intent`, `missing_lease`, `lease_conflict`, or `state_intent_declare cannot resolve current stateful session`:

1. Stop retrying command variants; denials are the API.
2. Re-read the exact target if it exists.
3. Use MCP in this subagent session: `state_session_register` if needed, `state_intent_declare(purpose=<task purpose>, files_planned=[...])`, then `state_lease_acquire(path=...)` for the exact file path. If the active tool list exposes only runtime-specific tool names, call the exact shown equivalent such as `mcp__stateful__state_intent_declare` or `mcp__stateful_state_intent_declare`. For new files, declare and lease the exact new file path, not only the parent directory. If the first recovery call returns `unsupported call`, stop and report the exact call name, active tool list if visible, and denial instead of retrying guessed variants.
4. Edit with native tools such as `apply_patch`, or use `sandbox run --fs write-targets` with the matching `--write-target` or `--create-target` for command-shaped writes.
5. If another session owns the lease, do not retry or steal it. Follow the wait queue when available; otherwise report the path, blocking session, and wait or reservation id to the parent agent.

For disposable repo `tmp/` directory lease conflicts, prefer a different session-unique scratch child over waiting when the artifact is truly disposable. A conflict on `tmp/<purpose>/` usually means another agent chose the same generic scratch name, not that the source-tree edit is blocked. Declare and lease a new child such as `tmp/<purpose>-<short-session-id>/`, then retry the command-shaped repo write with the exact same `--write-dir`. Build/test commands should use the build profile's external `/tmp/stateful` scratch instead of repo `tmp/`.

Do not diagnose these failures by running `stateful hook codex session-start`, `stateful hook omp session-start`, `stateful current`, `stateful notifications`, `stateful resume`, `strings <stateful>`, reading `$CODEX_HOME/shell_snapshots`, or manually setting `STATEFUL_SESSION_ID`. Those probes do not create the same-session lease required by hooks and usually waste the task budget.

For large source-tree reconstruction work, the parent agent must prove the write path before dispatching implementation subagents:

1. Build a concrete file manifest from the failing task and similar successful artifacts.
2. Declare and lease the first exact file, then land a tiny native edit or command-shaped write.
3. Use a write smoke test before bulk delegation: either dispatch one subagent to prove an exact-file MCP write path, or require each implementation subagent to prove its first exact-file MCP write before receiving many files. Otherwise give them a narrow read-only research task.
4. Assign each subagent disjoint source files and disjoint scratch directories. The parent should not lease a broad source-tree directory or shared `tmp/upstream` directory and then ask subagents to write under it.
5. Do not send subagents CLI `intent declare/request/claim` recipes to repair their session. Native subagents should use MCP tools; if MCP is missing or denied, they must report the exact denial and next file instead of probing session state.
6. Avoid reconstructing many files through large shell heredocs, `tee`, interactive stdin, or command-length experiments. Prefer native `apply_patch` after exact leases, or a verified git/network source through the git profile when that is available.

## Pick The Sandbox Profile First

Choose the narrowest existing entry point before writing the command. In Codex, the `Use` entries below are direct trusted `stateful sandbox run` wrappers through the configured absolute binary. In OMP, call `sandbox_bash` for the non-external `sandbox run --fs ...` profiles (`read-only`, `write-targets`, `build`, `git`, and `github-pr`) and call `external_bash` for `--fs external`; raw Bash and Python/JavaScript/JS/Ruby/Julia eval tools are denied even if they contain the listed wrapper text.

| Need | Use | Do not use |
|------|-----|------------|
| Inspect files after native read/search tools are unavailable or insufficient | `sandbox run --fs read-only --network disabled` | raw Bash, eval tools, networked read-only |
| Inspect processes | `sandbox process find ...` | `ps`, `pgrep`, or process checks inside `sandbox run --command` |
| Run any `git ...` command, including PR preparation | `sandbox run --fs git --network disabled --command 'git <args>'` for local git, `--network enabled` only for remote git such as `fetch`, `pull`, or `push` | raw git, `write-targets`, `build`, or explicit write targets |
| List, view, status-check, or create GitHub pull requests | `sandbox run --fs github-pr --network enabled --command 'gh pr <list|view|status|create> ...'` for a single non-interactive `gh pr` command; use the GitHub connector instead when explicitly allowlisted for the repo | raw `gh`, browser/editor PR flows, PR mutation subcommands outside creation, or explicit write targets |
| Run builds, tests, package managers, or generators whose outputs are disposable | `sandbox run --fs build --network enabled --write-dir <scratch-purpose> --command <cmd>`; scratch is created under `/tmp/stateful/<session>/<scratch-purpose>/` | raw test/build commands, repo `tmp` as build scratch, retained artifacts under `tmp/` |
| Run a non-git command-shaped repo write | `sandbox run --fs write-targets` with exact `--write-target`, `--create-target`, or narrow `--write-dir <repo-dir>` after matching intent and same-session lease | native edits without a file lease, broad write dirs, git commands |
| Run nested Codex benchmark agents | `sandbox run-nested-codex-benchmark ...` | generic relaxed profiles or wrapping it inside another sandbox |
| Request command-shaped writes outside the repo | Codex: `sandbox run --fs external --purpose ...` with absolute external targets for the approval prompt. OMP: `external_bash` with the same external scope so the tool can prompt before spawning the sandbox. | raw Bash, eval tools, or `write-targets` |

PR workflows still use the git profile for git work: `status`, `diff`, `log`, `branch`, `switch`/`checkout`, `add`, `commit`, `merge`, `rebase`, `tag`, and `push`. Do not use `--fs write-targets` just because git mutates the worktree or `.git`; the git profile owns that scope. GitHub pull request inspection and creation use the `github-pr` profile for `gh pr list`, `gh pr view`, `gh pr status`, and `gh pr create`; use the GitHub connector instead when that connector is explicitly allowlisted for the repo. If no listed profile matches the needed command, stop and report the exact unsupported command instead of trying raw Bash or a near-miss profile.

## Tmp Retention Rule

`tmp/` means temporary. Store only cache files, test/build outputs, throwaway logs, and other artifacts that can disappear between commands. Do not put plans, notes, generated fixtures needed by later tasks, benchmark baselines, installable binaries, review evidence, or user-requested deliverables in `tmp/`.

If deleting the artifact would change future work, do not use `tmp/`. Create a narrow repo-relative path named for the purpose, use exact `--write-target` or `--create-target` file scopes for command-shaped writes, and add that path to `.gitignore` when the artifact should not be committed. Use `--write-dir <repo-dir>` only when the command genuinely needs directory-level repo writes, and keep that directory narrow, declared, and leased. In parallel or subagent work, treat repo `tmp/<purpose>/` names as leased resources, not shared workspaces; use unique children for each active session.

## Sandbox Examples

Examples assume `<absolute-stateful-binary>` is the trusted absolute binary installed in Codex hook configuration.

After changing sandbox command behavior in this repo, the user must bootstrap the trusted binary outside hook-mediated Bash before Codex can use new sandbox flags or new hook-authorized sandbox command forms such as `sandbox process find`:

```bash
cp target/debug/stateful <absolute-stateful-binary>
```

Read-only shell inspection fallback when native read/search tools are unavailable or insufficient:

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

Run tests with a scratch purpose. Build outputs are disposable by policy because the build profile writes under `/tmp/stateful/<session>/<scratch-purpose>/`:

```bash
<absolute-stateful-binary> sandbox run --fs build --network enabled --write-dir test-run --command 'cargo test --workspace'
```

Run nested Codex benchmark agents only through the dedicated benchmark profile. This is not a general relaxed profile and must be the outermost sandbox command; do not wrap it inside another `stateful sandbox run`. It requires a trusted binary built with `codex-benchmark`, `--purpose`, `--write-dir target`, `--codex-home-root target/...`, and exactly one `--command`. The profile enables network, grants write access only under the `target/` artifact tree and the nested Codex home root under `target/`, and sets `STATEFUL_NESTED_CODEX_HOME_ROOT` so benchmark launchers can create per-agent `HOME`, `CODEX_HOME`, and XDG directories there. This command is currently supported only on macOS; Linux support needs a separately verified profile.

```bash
<absolute-stateful-binary> sandbox run-nested-codex-benchmark --purpose "run nested Codex chaos benchmark" --write-dir target --codex-home-root target/nested-codex-homes/run-1 --timeout-seconds 120 --command '<benchmark-command>'
```

Request a repo-external write. Codex prompts on `sandbox run --fs external --purpose ...`; OMP uses `external_bash`, which asks UI confirmation before spawning the trusted stateful binary with the same external profile. After approval, the command runs through the external sandbox profile:

```bash
<absolute-stateful-binary> sandbox run --fs external --purpose "install rebuilt stateful binaries" --write-dir <external-install-dir> --command 'install -m 755 target/release/stateful <external-install-dir>/stateful'
```

`stateful sandbox run --fs write-targets` targets must be repo-relative. Do not target `.git`, symlinks, paths outside the repo, or paths with control characters. Use `--write-target` for existing files, `--create-target` for new files, and `--write-dir <repo-dir>` only after declaring directory intent with a trailing slash for the exact directory, such as `reports/generated/` or `tmp/reports/`, and acquiring the same-session directory lease before using that `--write-dir`. Repo `tmp/` is not special for write-targets; it follows the same lease and safety rules as any other repo directory. Use `--fs external --purpose <purpose>` for normalized absolute targets outside the repo; it supports exact files, created files, whole external directories, `--connect-socket`, `--allow-signal`, and `--network` after Codex approval on the sandbox-run request, and rejects targets resolving inside the repo. Use `--fs build --write-dir <scratch-purpose>` for standard build/test commands; the scratch root is `/tmp/stateful/<session>/<scratch-purpose>/`, not repo `tmp/`. The read-only profile always requires `--network disabled`. Process inspection must use `stateful sandbox process find`; raw `ps` and `pgrep` inside `sandbox run --command` are disallowed. The git profile manages repo write scope itself; do not pass write targets with `--fs git`, and do not use it for config-mutating remote setup such as `git remote add` or `git remote set-url`. `/dev/null` is writable inside the sandbox; do not declare it as a target. `stateful sandbox run` is macOS-first and release-verified with Seatbelt. Linux bubblewrap support is implemented but experimental until verified in a Linux release environment.

## Prefer

- MCP or native read tools for search and inspection when available.
- Codex: `<absolute-stateful-binary> sandbox run --fs read-only --network disabled --command <cmd>` only as the fallback for Bash-tool command-shaped read-only inspection when native read/search tools are unavailable or insufficient. OMP: use `sandbox_bash` with `--fs read-only --network disabled --command <cmd>`.
- `<absolute-stateful-binary> sandbox process find --contains <literal>` or `--name <comm>` with optional `--pid`, `--parent-pid`, or `--process-group` selectors for process checks.
- Codex: `<absolute-stateful-binary> sandbox run --fs write-targets ... --command ...` for Bash-tool command-shaped writes that need a real shell, can be limited to exact file targets, create targets, or a narrow leased repo directory, and have matching intent plus a same-session lease. OMP: use `sandbox_bash` with the same write-targets flags after matching intent and lease.
- Codex: `<absolute-stateful-binary> sandbox run --fs build --network enabled --write-dir <scratch-purpose> --command 'cargo test --workspace'` for build or test commands that write disposable build artifacts under `/tmp/stateful/<session>/<scratch-purpose>/`. OMP: use `sandbox_bash` with the same build profile flags.
- Codex: `<absolute-stateful-binary> sandbox run --fs git --network disabled --command 'git <args>'` for local git operations such as status, add, commit, checkout, restore, reset, merge, rebase, and clean; use `--network enabled` for fetch, pull, push, and other remote git operations. OMP: use `sandbox_bash` with the same git profile flags. Remote metadata mutation such as `git remote add` and `git remote set-url` is rejected.
- Codex: `<absolute-stateful-binary> sandbox run --fs github-pr --network enabled --command 'gh pr <list|view|status|create> ...'` for non-interactive GitHub pull request listing, viewing, status checks, and creation; OMP: use `sandbox_bash` with the same github-pr profile flags. Use the GitHub connector instead when explicitly allowlisted for the repo.
- `<absolute-stateful-binary> sandbox run-nested-codex-benchmark --purpose ... --write-dir target --codex-home-root target/... --command ...` for nested Codex benchmark runs that need network and isolated per-agent Codex homes under `target/`.
- Codex: `<absolute-stateful-binary> sandbox run --fs external --purpose ... --write-dir <absolute-external-dir> --command ...` for approved sandboxed writes outside the repo. OMP: use `external_bash`, which prompts before invoking this profile internally; `sandbox_bash` rejects `--fs external`.
- Stateful diagnostics through MCP tools, native tools, or sandbox-run wrappers through the trusted absolute `stateful` binary.

## Avoid In Bash Or Eval Tools

- Codex raw Bash is denied by stateful hooks, including namespaced runtime aliases whose leaf tool is `bash` such as `functions.bash`; use a sandbox-run wrapper through the trusted absolute `stateful` binary or MCP/native tools instead. OMP raw Bash and Python/JavaScript/JS/Ruby/Julia eval-tool execution are always denied, including aliases whose leaf tool is `bash`, `python`, `javascript`, `js`, `ruby`, or `julia`, and including commands that invoke a valid `stateful sandbox run` wrapper. Namespaced read/search aliases follow their leaf tool and remain native read/search tools. Raw read-only Bash and eval snippets are also denied for repo-internal commands such as `rg`, `git status`, `sed`, or quick language snippets; in OMP use native read/search tools or `sandbox_bash` with the read-only profile. Repo-external Bash or eval-tool execution in OMP must use `external_bash`; raw Bash/eval calls that try `sandbox run --fs external` are denied.
- Outer shell wrappers around the sandbox command: environment assignments, command substitution, outer redirects, outer pipelines, multiple commands, duplicate `--command`, or an untrusted executable path.
- Session-repair probes such as `stateful hook codex session-start`, `stateful hook omp session-start`, `stateful current`, `stateful notifications`, `stateful resume`, `stateful intent declare/request/claim`, `strings <stateful>`, `$CODEX_HOME/shell_snapshots`, or outer `STATEFUL_SESSION_ID=...` assignments to force a lease. Use MCP session, intent, and lease tools instead.
- Shell write syntax outside a sandbox-run `--command`: `>`, `>>`, heredocs, and `| tee`.
- Direct file mutation: `rm`, `mv`, `cp`, `mkdir`, `touch`, `chmod`, `chown`.
- Raw process inspection such as `ps`, `pgrep`, `ps auxww`, `ps -ef`, `ps -eo pid,args`, or `ps e...` inside `sandbox run --command`; use `stateful sandbox process find` instead.
- Any generator, formatter, package manager, or script that creates, updates, deletes, or moves repo files.
- Storing anything in `tmp/` that must survive cleanup or be available to later work; use exact retained file targets in a purpose-named path and `.gitignore` as needed.
- `sandbox run --fs read-only --network enabled`; the read-only profile rejects network access.
- `sandbox run --fs build` without exactly one `--write-dir <scratch-purpose>`, with repo `tmp/...` as that scratch purpose, or with explicit `--write-target` / `--create-target`.
- Raw git commands; use `sandbox run --fs git` for both read-only and mutating git operations.
- Raw test commands; use `sandbox run --fs build --write-dir <scratch-purpose>` for commands that write disposable build or test artifacts.
- Generic relaxed sandbox profiles for nested Codex. Use only `sandbox run-nested-codex-benchmark`; it is intentionally narrower than a reusable relaxed profile.
- Most `stateful` control commands through Bash; use MCP tools when available.
- Repo-external writes through repo-internal profiles such as `--fs write-targets`; use `sandbox run --fs external --purpose ...` in Codex or `external_bash` in OMP so the approval prompt shows the external write scope, purpose, and command before execution.

## If Blocked

- Do not retry the same command with small variations.
- If the denial asks for scope, declare the missing exact file scope, acquire the exact same-session file lease, then use native edit tools with hook-visible targets for repo changes.
- If lease acquisition reports `lease_conflict`, do not retry the lease call. To wait for the path, call `state_intent_request` with a stable `request_id`, the denied `action`, `path`, and `purpose`; then poll `state_notifications_poll` or `state_resume_next` for the reservation. When reserved, reread the target. Native edit hooks and `sandbox run --fs write-targets` can lazy-claim the reservation at the next write attempt; manual MCP/CLI flows should call `state_intent_claim` with the `wait_id` before retrying.
- If a denial includes `wait_id`, `queue_position`, or reservation guidance, follow that wait queue protocol: poll `state_notifications_poll` / `state_resume_next`, reread the target after reservation, then either retry the native/sandbox write so the write boundary lazy-claims it, or call `state_intent_claim` first for manual MCP/CLI flows.
- If raw Bash or eval-tool execution is blocked, choose MCP/native inspection first and native edit tools after exact file intent declaration plus a successful same-session file lease for repo edits. In Codex, use `<absolute-stateful-binary> sandbox run --fs read-only --network disabled` only as the read-only shell/Python fallback, `<absolute-stateful-binary> sandbox run --fs write-targets` for command-shaped writes after matching intent and same-session lease, or `<absolute-stateful-binary> sandbox run --fs git` for git operations. In OMP, use `sandbox_bash` for read-only, write-targets, build, git, and github-pr profiles, or `external_bash` for `--fs external`.
- If an artifact needs to be retained and `tmp/` is the only authorized directory, stop and declare/lease a purpose-named path instead; add `.gitignore` intent and lease when that retained artifact should remain untracked.
- If a repo-external write is needed, use `<absolute-stateful-binary> sandbox run --fs external --purpose ...` with absolute external targets for the Codex approval prompt, or `external_bash` for OMP UI confirmation; `sandbox_bash` rejects `--fs external`, and raw OMP Bash/eval-tool external sandbox calls are denied. Do not try to bypass the external profile with raw `cp`, `install`, `cargo install`, shell redirection, or repo-internal sandbox profiles.
- If a denial mentions a stateful MCP coordination tool, prefer that MCP tool in Codex sessions.
- If a denial says the running server does not support sandbox write directories, restart the stateful server with a binary that supports sandbox write-directory authorization, then retry after rereading and reacquiring the needed lease.
- If `run-nested-codex-benchmark` is denied for missing feature support, install or rebuild the trusted binary with the `codex-benchmark` feature before retrying.
- If MCP tools are unavailable, do not try `stateful mcp call` through a read-only network-enabled sandbox; the read-only profile requires network disabled and most stateful control commands are not hook-approved raw Bash.
- If no policy-compliant path is available, report the exact command and denial reason.
