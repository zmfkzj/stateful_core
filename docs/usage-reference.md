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

Installs global stateful files, writes global `skills/stateful-command-policy/`
(`SKILL.md`, `omp-tools.md`, `sandbox-tools.md`, `denial-recovery.md`,
`subagent-write-recovery.md`) and
`skills/dispatching-parallel-agents/SKILL.md`, writes Codex external sandbox
prompt rules under the Codex config directory's `rules/stateful.rules`, and
merges the stateful block into Codex config. The merged config enables hooks and
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
bash.enabled: true
eval.py: false
eval.js: false
eval.rb: false
eval.jl: false
```

It removes `tools.approval` from the stateful profile because yolo mode delegates
safety to Stateful hooks. Without `--update`, existing scalar values are
preserved and only missing keys are inserted. With `--update`, existing target
scalar values are updated.

The installer also writes `rules/stateful-required.md` and
`skills/stateful-command-policy/` (`SKILL.md`, `omp-tools.md`,
`sandbox-tools.md`, `denial-recovery.md`, `subagent-write-recovery.md`) under
that isolated agent directory. The always-apply rule owns activation; the
`stateful-command-policy` manual owns the detailed Stateful procedure.

Installed OMP support:

- Built-in Bash for strict trusted `stateful sandbox run ...` commands with the
  narrowest valid sandbox profile and any required Stateful reservation/claim
  preflight.
- Built-in Bash for strict trusted `stateful sandbox process find ...` process
  inspection commands.
- External write/create/write-dir/socket/signal scope asks for a scoped OMP UI
  grant by default; `stateful.autoApprove: true` skips only that
  Stateful-owned prompt while sandbox scope validation, hooks,
  reservation/claim checks, and grant limits still apply. The approval prompt
  omits raw command text and grants matching calls keyed by purpose plus
  write/create/write-dir/socket/signal/network scope until expiry or max uses;
  defaults are 5 uses and 600 seconds. When auto-approval is enabled, no prompt
  is shown.
- `lazy_bash_resume` for a blocked external Bash command that could not prompt
  for its scoped grant on the original tool call. The live extension stores the
  trusted `stateful sandbox run --fs external ...` command, asks for the same
  grant during resume, re-authorizes the original Bash tool call, and reruns the
  stored command.
- `lazy_edit_resume` for strict replay of blocked, line-based OMP `edit` patches.
  The live extension stores the original patch after `missing_reservation`,
  `missing_claim`, or claim-conflict denials; after the agent fixes the missing
  scope or receives a claimable reservation, it re-authorizes the original edit,
  checks the file has not changed since queue time, then applies the stored
  line-based patch.
- `lazy_write_resume` for replay of blocked full OMP `write` content. It uses the
  same queued wait id or generated operation id path, re-authorizes after the
  agent fixes the missing scope or receives a claimable reservation, and fails
  if the target changed since the write was queued.


### Repo allowlist

```bash
stateful enable [--repo <path>]
stateful disable [--repo <path>]
stateful repos list
```

Global hooks are gated by the repo allowlist. Disabled repos are left alone:
hooks do not start the server or append outbox records, and native state calls
report that the repo is not enabled.

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
Agent-facing native tools and hooks on the remote Mac should reach the runtime
through an SSH tunnel so the bearer token is not sent to a non-loopback
`http://` address.

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

This installs global stateful/Codex configuration and writes remote runtime
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

In active Codex or OMP sessions, prefer the active Stateful coordination tools
over shelling out to `stateful reservation ...`; lifecycle hooks bind the active
`agent_id` and `workspace_id`.

Manual CLI use outside active hooks can pass an explicit agent and workspace.
Capture the returned `reservation_id` and pass it through later write
authorization steps:

```bash
reservation_id=$(stateful reservation declare \
  --agent-id demo-agent \
  --workspace-id <workspace> \
  --purpose "Update README content requested by the user." \
  README.md | jq -r '.reservation_id')
```

Reservation commands:

```bash
stateful reservation declare --purpose <purpose> <paths...>
stateful reservation request --reservation-id <reservation_id> --request-id <id> --action write_file|write_directory --path <path> --purpose <purpose>
stateful reservation claim --reservation-id <reservation_id> --wait-id <wait_id>
stateful reservation cancel --request-id <id>
```

Notes:

- `declare` requires at least one non-empty path and a non-empty purpose, and
  returns `reservation_id`.
- Declarations add to the session's active scope under that `reservation_id`.
- Re-declaring the same path updates the purpose used for future claim
  acquisition.
- `state_claim_acquire` uses `reservation_id` plus `paths: string[]` to acquire
  one or more exact file or directory resources from that reservation.
- Writes must carry the same `reservation_id` through native edit/write
  integration or `stateful sandbox run --reservation-id <reservation_id>`.
- `request` creates or returns an idempotent queued or claimable (`reserved`)
  write request. During queued-reservation compatibility, wait records expose
  the same id as both `wait_id` and `reservation_id`.
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
- `stateful install --agent omp [--yes] [--update] [--binary <path>]`
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

## Hooks And Identity

`SessionStart` registers the active `agent_id` and `workspace_id` for hook and
state operations. In OMP, the extension/native tool bridge injects those
identifiers explicitly, using OMP-provided agent/session identity and failing
closed when no active agent id is available; agents do not maintain
current-session files or select coordination identity through environment
variables. In Codex, hooks receive Codex's `session_id` parameter and map it to
Stateful `agent_id`; `UserPromptSubmit` renders current-state context.

`PreToolUse` authorizes supported tool actions. Server-side authorization records
an implicit agent heartbeat for the checked agent. `PostToolUse` records
activity or heartbeats; Codex `PostToolUse` also releases authorizing
same-reservation repo-write claims after completed native edit and
`write-targets` transactions. `Stop` posts
`state_activity_finalize`, finalizing activity and releasing the agent's
claims.

Stateful no longer exposes an agent-facing current-session fallback. The removed
legacy path wrote `.stateful_core/runtime/sessions/<session_id>.json` plus the
alias `.stateful_core/runtime/session.json` and allowed callers to choose state
through session environment variables. Active integrations now pass explicit
identity into hooks and native tools instead.

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

Task-level reservation authorizes writes only when its file set includes the exact file or directory resource and the write supplies that `reservation_id`. Directory scope authorizes only `write_directory` for the exact directory resource. File writes, deletes, renames, and moves require exact file scopes for the affected paths; directory scope does not authorize them.

Writes without matching active reservation are denied. A write is allowed only
when the `reservation_id` has active scope for the target and an active claim
under the same `reservation_id` covers the target. Conflicting claims outside
the authorizing reservation block writes. A blocked writer can queue with
`queue_on_conflict`; after promotion, the reservation notification and resume
payload carry the stored request purpose, and the agent with the claimable
reservation must reread the target.

Repo file edits should use native edit tools with hook-visible targets after
task-level reservation covers the target and a successful same-reservation file
claim. Hooks extract the native tool target, call `/v1/authorize` with the
operation-specific action and `reservation_id`, allow the edit only after an
allow decision, and release the authorizing claim after the completed write
transaction.

## Sandbox Profiles

Raw shell commands are denied by hooks in enabled sessions. Use the narrowest
sandbox profile that matches the command.

| Profile | Use |
| --- | --- |
| `read-only` | Shell-based read-only repo inspection when native read/search tools are insufficient. Network must be disabled. |
| `build` | Build/test/package commands that write disposable artifacts under `/tmp/stateful/<session>/<scratch-purpose>/`. |
| `write-targets` | Command-shaped repo writes after matching reservation and same-reservation file or directory claim; pass the same id with `--reservation-id <reservation_id>`. |
| `git` | Local or remote `git ...` operations. Use `--network enabled` only for remote git operations. |
| `github-pr` | Non-interactive `gh pr list|view|status|create` commands. |
| `external` | Repo-external shell operations with a purpose; add explicit external write/create/dir scope only when needed. |

`stateful sandbox run` defaults to command-like pass-through: the wrapped
command's `stdout`, `stderr`, and exit code become the wrapper's `stdout`,
`stderr`, and exit code. Add `--json` when callers need the structured result
envelope with `status`, `exit_code`, captured streams, and authorization
metadata.

Examples:

```bash
reservation_id=$(stateful reservation declare --purpose "Update README content requested by the user." README.md | jq -r '.reservation_id')
stateful sandbox run --fs build --network enabled --write-dir test-run --command 'cargo test --workspace'
stateful sandbox run --fs write-targets --reservation-id "$reservation_id" --write-target README.md --command 'python3 update_readme.py'
stateful sandbox run --fs git --network disabled --command 'git status --short'
stateful sandbox run --fs github-pr --network enabled --command 'gh pr status'
stateful sandbox run --fs external --purpose "inspect external tool version" --command 'some-external-tool --version'
```

Codex process inspection uses `stateful sandbox process find`:

```bash
stateful sandbox process find --name stateful-bench
stateful sandbox process find --contains denovo_codex_agent
```

In OMP, use built-in Bash with a single trusted
`stateful sandbox process find ...` command after Stateful preflight. Result JSON
includes safe process metadata fields: `pid`, `ppid`, `pgid`, `user`, `uid`,
`stat`, `start`, `etime`, `time`, `pcpu`, `pmem`, `rss`, `vsz`, `nice`, `pri`,
`tty`, and `comm`. Command strings, argv, and environment data are never exposed
in result JSON.

## HTTP And Native Tool Surface

The HTTP server exposes `/health`, `/v1/current`, `/v1/events`,
`/v1/runtime/identity`, and POST endpoints for agent registration,
heartbeats, reservation declaration, reservation request, reservation claim, reservation cancel,
claims, activity observation/finalization, authorization, conflict checks,
context rendering, reconciliation ack, notifications, resume, and outbox sync.

Native Stateful tools expose agent-friendly tool names. Agent identity tools use
native names directly; other tools map to dotted protocol names:

- `state_session_register` — registers the injected `agent_id`/`workspace_id`
- `state_session_heartbeat` — heartbeats the injected `agent_id`/`workspace_id`
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

`state_context_render` is for planning/manual inspection. Routine write and
denial recovery should use reservation, claim, resume, and the denial's direct
next action instead of rendering ambient context.

- `state_reconcile_ack` / `state.reconcile.ack`
- `state_notifications_poll` / `state.notifications.poll`
- `state_resume_next` / `state.resume.next`

`state_claim_acquire` takes `reservation_id` and `paths: string[]`; each path
must match active exact file or directory reservation scope under that
reservation, and the server creates one exact resource claim per entry. Legacy
server requests with `path` are still accepted for compatibility.

`state_claim_release` remains single-resource and takes `path: string`.

`state_file_write` / `state.file.write` and `state_bash_write` /
`state.bash.write` were removed. Use native edit tools with hook-visible targets
for file edits after task-level reservation and exact same-reservation claim,
and use `stateful sandbox run --fs write-targets --reservation-id <reservation_id> ...`
for command-shaped writes.

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
`runtime/server.json`, local `state.db`, and outbox JSONL files under `outbox/`.

Commit reusable documentation and source code, not local generated state. For
public source archives, prefer `git archive` or a clean clone instead of a
working-tree tarball so ignored runtime and benchmark artifacts are not bundled.

## Environment Variables

- `STATEFUL_HOME` overrides the user-level state directory. When unset,
  `$HOME/.stateful_core` is used.
- `STATEFUL_SERVER_URL` and `STATEFUL_SERVER_TOKEN` override runtime discovery
  when both are set. The referenced server must expose the current runtime
  capabilities.
- Legacy agent-facing identity environment variables were removed. Active Codex and
  OMP integrations inject `agent_id` and `workspace_id` into hooks and native
  tools; OMP fails closed when adapter identity is unavailable. Agents should
  not set environment variables, use process ids, or repair runtime session
  files to select coordination identity.
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
- `programbench`: run Codex or OMP agents on ProgramBench instances, evaluate the
  resulting `submission.tar.gz` artifacts with official ProgramBench tooling, and
  report stateful/no-state quality plus time/token efficiency deltas.
  ProgramBench Codex/OMP runs use host CLI adapters in an empty temporary
  airlock seeded from the target container's `/workspace`; see the ProgramBench
  guide for the run-behavior details.

For DeNovoSWE, new official-style run commands should pass `--prompt-version v2`;
the CLI's default remains compatible with historical behavior. See
[DeNovoSWE Benchmark Guide](denovo-benchmark-guide.md) for interpretation rules
and [DeNovoSWE Benchmark Commands](denovo-benchmark-commands.md) for reusable
command lines.

For ProgramBench, scored runs require Linux `amd64` Docker support and official
`programbench eval`; see [ProgramBench Benchmark Guide](programbench-benchmark-guide.md).

Benchmark artifacts live under `.stateful_bench/` and are intentionally ignored.

## Versioning And Reclaims

PR titles are the release input. Keep squash merge enabled and set GitHub's
squash commit message default to the PR title, so the commit that lands on
`main` is the same Conventional Commit title reviewed on the PR.

Use these PR title forms:

```text
fix(hook): reject stateful coordination through Bash
feat(cli): add sandboxed git status support
feat(native-tools)!: rename the claim acquisition method
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
`stateful-core`, `stateful-store`, `stateful-server`, and `stateful-cli`. The
benchmark crate is `publish = false` and is not a release component.

The release workflow uses `GITHUB_TOKEN` by default. Configure a repository
secret named `RELEASE_PLEASE_TOKEN` with a suitable personal access token when
release-please-created PRs or tags need to trigger other GitHub Actions
workflows. Repository settings must also allow GitHub Actions to create pull
requests.
