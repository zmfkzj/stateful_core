# Usage Reference

This is the detailed companion to the human-facing [README](../README.md). It
keeps operational and implementation-adjacent guidance out of the landing page
while preserving the commands and constraints users copy during local setup.

For policy semantics, prefer [State model](state-model.md). For concrete API,
storage, hook, runtime, and test contracts, prefer
[Implementation contract](implementation-contract.md).

## Install Modes

### Global files

```bash
stateful install --yes
```

Installs global stateful files under `$STATEFUL_HOME`, or
`$HOME/.stateful_core` when `STATEFUL_HOME` is unset. Without `--yes`, the
installer prints a dry-run plan.

### Codex

```bash
stateful install --agent codex --yes
```

Installs global stateful files, writes global
`skills/stateful-command-policy/SKILL.md` and
`skills/dispatching-parallel-agents/SKILL.md`, writes Codex external sandbox
prompt rules under the Codex config directory's `rules/stateful.rules`, and
merges the stateful block into Codex
config. The merged config enables hooks, registers the `stateful` MCP server, and
adds lifecycle hooks for:

- `SessionStart` for `startup`, `resume`, `clear`, and `compact`
- `UserPromptSubmit`
- `PreToolUse` for all tools
- `PostToolUse` for `Bash`, `apply_patch`, `Edit`, `Write`, `file_change`, and
  `mcp__filesystem__.*`
- `Stop`

The Codex wrapper runs Codex with pass-through session configuration by default:

```bash
stateful codex [--codex-bin <path>] [--sandbox passthrough] [--no-stateful] -- <args...>
```

`--no-stateful` disables Codex lifecycle hooks for that run.

### OMP

```bash
stateful install --agent omp --yes
```

Installs global stateful files and configures the isolated OMP `stateful` profile
under `~/.omp/profiles/stateful/agent` by default. The installer merges
`config.yml` instead of replacing it and rejects invalid YAML.

The target OMP profile keys are:

```yaml
tools.approvalMode: yolo
stateful.autoApprove: false
bash.enabled: false
eval.py: false
eval.js: false
eval.rb: false
eval.jl: false
```

It removes `tools.approval` from the stateful profile because yolo mode delegates
safety to Stateful hooks. Without `--update`, existing scalar values are
preserved and only missing keys are inserted. With `--update`, existing target
scalar values are updated.

The installer also writes `rules/stateful-required.md`,
`skills/stateful-command-policy/SKILL.md`, and
`skills/dispatching-parallel-agents/SKILL.md` under that isolated agent
directory. The always-apply rule owns activation; the
`stateful-command-policy` manual owns the detailed Stateful procedure.

Generated OMP tools:

- `sandbox_bash` for non-external `stateful sandbox run` profiles: `read-only`,
  `write-targets`, `build`, `git`, and `github-pr`
- `process_find` for process inspection; parameters are `names`, `contains`,
  `pids`, `parent_pids`, `process_groups`, `fields`, `timeout_seconds`, and `async`
- `ext_ro_bash` for read-only `--fs external` purpose-and-command operations
  without OMP UI confirmation
- `ext_rw_bash` asks for a scoped OMP UI grant by default; `stateful.autoApprove: true` or the per-call `auto_approve: true` flag skips only that Stateful-owned prompt while sandbox scope validation, hooks, reservation/claim checks, and grant limits still apply.
  The approval prompt omits raw command text and grants matching calls keyed by
  purpose plus write/create/write-dir/socket/signal/network scope until expiry
  or max uses; defaults are 5 uses and 600 seconds. When auto-approval is
  enabled, no prompt is shown.
- `lazy_edit_resume` for strict replay of blocked, line-based OMP `edit` patches.
  The live extension stores the original patch after `missing_reservation`,
  `missing_claim`, or claim-conflict denials; after the agent fixes the missing
  scope or receives a claimable reservation, it re-authorizes the original edit,
  checks the file has not changed since queue time, then applies the stored patch.

Generated OMP bash tools run sandbox commands in the background by default:
omitted or `true` `async` returns a job id immediately, streams output through
OMP messages, and posts final stdout/stderr/status later. Set `async: false` to
wait for the final result in the initial tool response. OMP abort/ESC cancels
the running sandbox command.

Raw Bash and eval tool calls are denied by stateful hooks; use the generated
OMP tools instead.

### Repo allowlist

```bash
stateful enable [--repo <path>]
stateful disable [--repo <path>]
stateful repos list
```

Global hooks are gated by the repo allowlist. Disabled repos are left alone:
hooks do not start the server or append outbox records, and MCP calls report
that the repo is not enabled.

## Server Lifecycle

```bash
stateful server start
stateful server status
stateful server restart
stateful server stop
```

`stateful server start` starts the HTTP state server detached by default and
prints join commands. Use `stateful server start --foreground` for a foreground
compatibility form. Bare `stateful server` remains a foreground compatibility
form.

## LAN Runtime Sharing

Use LAN mode when one Mac should host the stateful HTTP runtime for another Mac.
The MCP adapter still runs locally inside each Codex process, and the remote Mac
should reach the runtime through an SSH tunnel so the bearer token is not sent to
a non-loopback `http://` address.

On the host Mac:

```bash
stateful server start --host 0.0.0.0 --workspace-id shared
```

On the remote Mac, create an SSH tunnel to the host, then run the printed join
command against the local tunnel endpoint:

```bash
ssh -L 43873:127.0.0.1:43873 <host-mac>
stateful server join http://127.0.0.1:43873 --token <token>
```

This installs global stateful/Codex MCP configuration and writes remote runtime
discovery under `$STATEFUL_HOME/runtime/server.json`. It does not enable the
current repo. To enable the current repo on the remote Mac, pass:

```bash
stateful server join http://127.0.0.1:43873 --token <token> --enable-repo
```

`stateful server join` rejects non-loopback plain `http://` URLs before writing
runtime discovery or Codex config. Use an SSH tunnel and join the loopback
endpoint instead. If the host uses `stateful server start --workspace-id <id>`,
run the printed join command as-is; it includes the matching workspace id.

## Manual Coordination Commands

In active Codex or OMP sessions, prefer MCP tools over shelling out to
`stateful reservation ...` or `stateful mcp call ...`; lifecycle hooks bind the active
runtime session as the Stateful session id.

Manual CLI use outside active hooks can pass session and workspace explicitly:

```bash
stateful reservation declare \
  --session-id demo \
  --workspace-id <workspace> \
  --purpose "Update README content requested by the user." \
  README.md
```

Reservation commands:

```bash
stateful reservation declare --purpose <purpose> <paths...>
stateful reservation request --request-id <id> --action write_file|write_directory --path <path> --purpose <purpose>
stateful reservation claim --wait-id <id>
stateful reservation cancel --request-id <id>
```

Notes:

- `declare` requires at least one non-empty path and a non-empty purpose.
- Declarations add to the session's active scope in that workspace.
- Re-declaring the same path updates the purpose used for future claim
  acquisition.
- MCP `state_claim_acquire` uses `paths: string[]` to acquire one or more exact
  file or directory resources from active reservation scope.
- `request` creates or returns an idempotent queued or claimable (`reserved`)
  write request.
- `claim` uses the stored reservation purpose; clients do not pass a new claim
  purpose.
- Native edit hooks and sandbox `write-targets` authorization may lazy-claim a
  claimable request at the retry write boundary.

Resume commands:

```bash
stateful notifications poll
stateful resume next
```

Reservation notifications and resume payloads include the stored request purpose.

## CLI Overview

Common commands:

- `stateful install [--yes]`
- `stateful install --agent codex [--yes] [--codex-config <path>] [--binary <path>]`
- `stateful install --agent omp [--yes] [--update]`
- `stateful enable [--repo <path>]`, `stateful disable`, `stateful repos list`
- `stateful tools list`, `stateful tools allow <tool>`, `stateful tools deny <tool>`
- `stateful server start|restart|status|stop|join`
- `stateful status`, `stateful doctor`, `stateful current`, `stateful events`
- `stateful reservation declare|request|claim|cancel`
- `stateful notifications poll`
- `stateful resume next`
- `stateful sandbox run ...`
- `stateful sandbox process find ...`
- `stateful codex ...`
- `stateful mcp serve`
- `stateful mcp call <tool> [arguments_json]`
- `stateful sync-outbox`
- `stateful hook codex <event>`
- `stateful hook omp <event>`

Run `stateful <command> --help` for command-specific options.

## Tool Allowlist

`stateful tools list`, `stateful tools allow <tool>`, and
`stateful tools deny <tool>` manage repo-scoped exceptions for unclassified
tools. The default allowlist is assembled from Codex-specific and OMP-specific
entries; both hooks record unknown tool names into this list.

This allowlist does not bypass hard-denied write or execution classifications.
Write/execute paths still require the normal stateful authorization flow.

## Hooks And Sessions

`SessionStart` registers the active session and writes the current-session file
used by CLI and MCP calls. In OMP, `stateful hook omp session-start` prefers the
actual OMP session id from `event.sessionId` or `ctx.sessionManager.session.id`,
stores that id in `process.env.STATEFUL_SESSION_ID`, and persists current-session
files before session-aware MCP tools run. In Codex, `UserPromptSubmit` renders
current-state context.

`PreToolUse` authorizes supported tool actions. Server-side authorization records
an implicit session heartbeat for the checked session. `PostToolUse` records
activity or heartbeats; Codex `PostToolUse` also releases same-session repo-write
claims after completed native edit and `write-targets` transactions. `Stop` posts
`state_activity_finalize`, finalizing activity and releasing the session's
claims.

Hooks write `.stateful_core/runtime/sessions/<session_id>.json` plus the current
session alias `.stateful_core/runtime/session.json`. Session-bound callers use
`STATEFUL_SESSION_ID` to select the matching session-bound file, except Codex MCP
callers prefer `CODEX_THREAD_ID` when it is present. If no session environment
variable is present, callers fall back to `.stateful_core/runtime/session.json`
only after verifying that its `session_id` has a matching session-bound file with
identical contents.

## Write Authorization

Write authorization is the current implementation's coordination gate. It is a
policy check over shared operational state, not a security boundary or a global
file lock manager.

The v1 authorization API supports:

- `write_file`
- `write_directory`
- `delete_file`
- `rename_file`
- `move_file`

Task-level reservation authorizes writes only when its file set includes the exact file or directory resource. Directory scope authorizes only `write_directory` for the exact directory resource. File writes, deletes, renames, and moves require exact file scopes for the affected paths; directory scope does not authorize them.

Writes without matching active reservation are denied. Active claims held by another
session block conflicting writes. A blocked writer can queue with
`queue_on_conflict`; after promotion, the reservation notification and resume
payload carry the stored request purpose, and the session with the claimable
reservation must reread the target.

Repo file edits should use native edit tools with hook-visible targets after
task-level reservation covers the target and a successful same-session file claim. Hooks extract
the native tool target, call `/v1/authorize` with the operation-specific action,
allow the edit only after an allow decision, and release the authorizing claim
after the completed write transaction.

## Sandbox Profiles

Raw shell commands are denied by hooks in enabled sessions. Use the narrowest
sandbox profile that matches the command.

| Profile | Use |
| --- | --- |
| `read-only` | Shell-based read-only repo inspection when native read/search tools are insufficient. Network must be disabled. |
| `build` | Build/test/package commands that write disposable artifacts under `/tmp/stateful/<session>/<scratch-purpose>/`. |
| `write-targets` | Command-shaped repo writes after matching reservation and same-session file or directory claim. |
| `git` | Local or remote `git ...` operations. Use `--network enabled` only for remote git operations. |
| `github-pr` | Non-interactive `gh pr list|view|status|create` commands. |
| `external` | Repo-external shell operations with a purpose; add explicit external write/create/dir scope only when needed. |

Examples:

```bash
stateful sandbox run --fs build --network enabled --write-dir test-run --command 'cargo test --workspace'
stateful sandbox run --fs write-targets --write-target README.md --command 'python3 update_readme.py'
stateful sandbox run --fs git --network disabled --command 'git status --short'
stateful sandbox run --fs github-pr --network enabled --command 'gh pr status'
stateful sandbox run --fs external --purpose "inspect external tool version" --command 'some-external-tool --version'
```

Codex process inspection uses `stateful sandbox process find`:

```bash
stateful sandbox process find --name stateful-bench
stateful sandbox process find --contains denovo_codex_agent
```

In OMP, call the generated `process_find` tool directly instead of routing
process inspection through `sandbox_bash` or raw Bash. By default, result JSON
includes safe process metadata fields: `pid`, `ppid`, `pgid`, `user`, `uid`,
`stat`, `start`, `etime`, `time`, `pcpu`, `pmem`, `rss`, `vsz`, `nice`, `pri`,
`tty`, and `comm`. Command strings, argv, and environment data are never exposed
in result JSON.

## HTTP And MCP Surface

The HTTP server exposes `/health`, `/v1/current`, `/v1/events`,
`/v1/runtime/identity`, and POST endpoints for session registration,
heartbeats, reservation declaration, reservation request, reservation claim, reservation cancel,
claims, activity observation/finalization, authorization, conflict checks,
context rendering, reconciliation ack, notifications, resume, and outbox sync.

The MCP adapter exposes agent-friendly tool names mapped to dotted protocol
names:

- `state_session_register` / `state.session.register`
- `state_session_heartbeat` / `state.session.heartbeat`
- `state_reservation_declare` / `state.reservation.declare`
- `state_reservation_request` / `state.reservation.request`
- `state_reservation_claim` / `state.reservation.claim`
- `state_reservation_cancel` / `state.reservation.cancel`
- `state_claim_acquire` / `state.claim.acquire`
- `state_claim_release` / `state.claim.release`
- `state_activity_observe` / `state.activity.observe`
- `state_activity_finalize` / `state.activity.finalize`
- `state_conflicts_check` / `state.conflicts.check`
- `state_current_read` / `state.current.read`
- `state_events_read` / `state.events.read`
- `state_context_render` / `state.context.render`
- `state_reconcile_ack` / `state.reconcile.ack`
- `state_notifications_poll` / `state.notifications.poll`
- `state_resume_next` / `state.resume.next`

`state_claim_acquire` takes `paths: string[]`; each path must match active exact
file or directory reservation scope, and the server creates one exact resource
claim per entry. Legacy server requests with `path` are still accepted for
compatibility. `state_claim_release` remains single-resource and takes
`path: string`.

`state_file_write` / `state.file.write` and `state_bash_write` /
`state.bash.write` were removed. Use native edit tools with hook-visible targets
for file edits after task-level reservation and exact claim, and use `stateful sandbox run --fs
write-targets ...` for command-shaped writes.

The `/v1/authorize` endpoint and the reservation declare/request/claim/cancel
endpoints require the `stateful.v1` request envelope with `payload`. Flat legacy
bodies are rejected with `protocol_mismatch` for those paths. Other POST routes
still accept their current flat request bodies.

## Generated Local Files

Ignored local paths:

- `.codex/`
- `.stateful/`
- `.stateful_core/`
- `.stateful_bench/`

Global installation writes under `$STATEFUL_HOME`, or `$HOME/.stateful_core`
when `STATEFUL_HOME` is unset. That directory can contain `config.yml`,
`state.db`, `runtime/server.json`, `runtime/server.lock`, `runtime/server.log`,
and repo metadata under `repos/`.

Repo hook runtime state lives under `.stateful_core/`. That directory can contain
`runtime/server.json`, `runtime/session.json`, session-bound
`runtime/sessions/*.json` files, local `state.db`, and outbox JSONL files under
`outbox/`.

Commit reusable documentation and source code, not local generated state. For
public source archives, prefer `git archive` or a clean clone instead of a
working-tree tarball so ignored runtime and benchmark artifacts are not bundled.

## Environment Variables

- `STATEFUL_HOME` overrides the user-level state directory. When unset,
  `$HOME/.stateful_core` is used.
- `STATEFUL_SERVER_URL` and `STATEFUL_SERVER_TOKEN` override runtime discovery
  when both are set. The referenced server must expose the current runtime
  capabilities.
- `STATEFUL_SESSION_ID` selects the session-bound current-session file at
  `.stateful_core/runtime/sessions/<session_id>.json` for MCP tools and other
  session-bound callers. The OMP extension sets it from `event.sessionId` /
  `ctx.sessionManager.session.id`; Codex MCP resolution prefers
  `CODEX_THREAD_ID` when present.
- `STATEFUL_HOOK_TRUSTED_SANDBOX` is a legacy integration signal and does not
  authorize Bash. Bash authorization goes through a trusted
  `<absolute-stateful-binary> sandbox run` wrapper command.

## Benchmark Commands

`stateful-bench` supports:

- `fetch`: cache SWE-bench Verified rows.
- `prepare-pairs` and `generate-fallback-preflight`: build eligible pair
  manifests.
- `sample`: create deterministic stratified samples.
- `run`: execute no-state or stateful paired-agent runs.
- `report` and `compare`: summarize one run or compare stateful/no-state runs.
- `synthetic`: run the built-in synthetic coordination benchmark.
- `denovo`: wrap AweAgent DeNovoSWE extract/evaluation workflows and run the
  official AweAgent agent recipe, a host Codex CLI adapter, or an OMP CLI adapter
  while recording `stateful`, `subagent`, and `running_time_ms` comparison axes.

For DeNovoSWE, new official-style run commands should pass `--prompt-version v2`;
the CLI's default remains compatible with historical behavior. See
[DeNovoSWE Benchmark Guide](denovo-benchmark-guide.md) for interpretation rules
and [DeNovoSWE Benchmark Commands](denovo-benchmark-commands.md) for reusable
command lines.

Benchmark artifacts live under `.stateful_bench/` and are intentionally ignored.

## Versioning And Reclaims

PR titles are the release input. Keep squash merge enabled and set GitHub's
squash commit message default to the PR title, so the commit that lands on
`main` is the same Conventional Commit title reviewed on the PR.

Use these PR title forms:

```text
fix(hook): reject stateful coordination through Bash
feat(cli): add sandboxed git status support
feat(mcp)!: rename the claim acquisition method
docs: clarify sandbox write targets
chore: update release automation
```

Release Please reads the merged commits on `main`, opens or updates a release PR,
and updates crate versions and `CHANGELOG.md` when that release PR is merged. The
configured release rules are:

- `fix:` creates a patch release.
- `feat:` normally creates a minor release, but while the project is `0.x`, it
  creates a patch release.
- `!` or a `BREAKING CHANGE:` footer normally creates a major release, but while
  the project is `0.x`, it creates the next minor release.
- `docs:`, `test:`, `ci:`, `build:`, `chore:`, and `refactor:` are allowed PR
  titles but do not create a version bump unless they are marked breaking.

The release configuration uses manifest-driven `release-please` with the Rust
`cargo-workspace` plugin and a linked version group for the public crates:
`stateful-core`, `stateful-store`, `stateful-server`, `stateful-cli`, and
`stateful-mcp`. The benchmark crate is `publish = false` and is not a release
component.

The release workflow uses `GITHUB_TOKEN` by default. Configure a repository
secret named `RELEASE_PLEASE_TOKEN` with a suitable personal access token when
release-please-created PRs or tags need to trigger other GitHub Actions
workflows. Repository settings must also allow GitHub Actions to create pull
requests.
