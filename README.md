# stateful_core

Current-state coordination for coding agents and humans working in the same
repository.

Git coordinates committed history. `stateful_core` coordinates active work
before it becomes history.

Before a write occurs, tools can ask a practical question:

```text
Who is doing what now, what might conflict, and when does that claim expire?
```

The project is intentionally about current state, not long-term memory. Memory
recalls what happened before. Current state captures active, scoped, expiring
operational truth that can be checked before writes, validation, handoff, and
other coordination-sensitive actions.

Git resolves conflicts after they happen. `stateful_core` tries to prevent
avoidable conflicts before the write occurs.

## Status

This repository is an early Rust implementation and local-first prototype. The
current implementation is Codex-first and includes a CLI, global user-level
installation with repo allowlist gating, a repo-local compatibility mode, a
local HTTP state server, MCP adapter, Codex hook adapter, SQLite-backed state
store, validation runner, structured commit/push wrappers, outbox sync, and
benchmark tooling.

Several surfaces are intentionally narrow in this prototype: `/v1/current`
returns count summaries, prompt context rendering is still a placeholder, and
leases are advisory coordination records rather than automatic file locks.
The Rust crates expose library APIs used by the workspace, but those APIs are
also pre-release; CLI, HTTP, and MCP behavior are the primary documented
integration surfaces today.

APIs, configuration files, and command behavior may change while the project is
pre-release. The current security and support scope is documented in
[SECURITY.md](SECURITY.md).

## Why

Coding agents usually know their own session, but they do not reliably know
what another agent, another session, or a human is doing in the same repository
right now. That creates avoidable failures:

- two agents edit the same file without seeing each other
- a human edits one file while an agent rewrites nearby docs
- an interrupted session leaves no structured handoff state
- stale memory is treated as active truth
- a tool writes before the session has declared its intended scope
- validation or reconciliation happens outside the coordination loop

For example, a human can be editing `README.md`, one Codex session can be
updating `docs/`, and another Codex session can be waiting for `README.md`.
The work on `docs/` can continue while the conflicting `README.md` write is
queued instead of turning into a surprise merge conflict later.

`stateful_core` provides a small protocol for declaring intent, tracking active
leases, recording session activity, reading current-state summaries, and
blocking supported writes that have no matching active intent.

## How It Fits

`stateful_core` does not replace Git, branches, worktrees, editors, or agent
runners.

- Git coordinates committed history.
- Branches and worktrees isolate longer-running lines of work.
- Editors and agent runners manage task execution and local context.
- `stateful_core` coordinates active work in a shared workspace: current
  claims, active leases, waiting sessions, and resume signals.

The current implementation is Codex-first, but the coordination problem is not
Codex-specific. Any tool that writes into a repository can benefit from a shared
answer to "who is working on this right now?"

Intent and lease are separate on purpose. Intent declares planned work and the
scope a session expects to touch. A lease declares active ownership of a scoped
resource with its own TTL; session heartbeats do not refresh that TTL. Lease
acquisition is currently explicit, except that a promoted wait-queue reservation
is claimed as an active lease when its write is authorized. Intent answers "what
may this session work on?" Lease answers "who is actively holding this now?"

## Wait Queue and Resume

Write conflicts are scoped to the file or resource being modified. If session A
holds an active lease on `src/auth.ts`, sessions B and C can still read,
search, validate, or work on other files. Writes to `src/auth.ts` are blocked.

At the README level, the flow is intentionally simple:

```text
blocked -> waiting -> reserved -> active
```

A conflicting writer is blocked by the active lease and can enter a FIFO wait
queue. When the active lease is released or the owning activity finalizes, the
first waiter receives a short reservation. The reserved session must reread the
target and retry the write; that retry claims the reservation and acquires the
active lease. The default reservation TTL is 120 seconds. Reservation
notifications are currently pending records that remain visible until they
expire; polling does not consume them.

Detailed queue states, lease expiry behavior, and promotion rules are covered
in the state model and implementation contract docs.

Resume signals are available through:

- `stateful notifications poll`
- `stateful resume next`
- MCP tools `state_notifications_poll` and `state_resume_next` using protocol
  names `state.notifications.poll` and `state.resume.next`
- the `/v1/notifications/poll` and `/v1/resume/next` HTTP endpoints

## What It Provides

- A `stateful` CLI for installation, repo enablement, status, current-state
  inspection, intent declaration, validation, MCP, hooks, structured commit and
  push wrappers, outbox sync, and server lifecycle management.
- A local HTTP state server with token-protected non-health endpoints.
- A SQLite event store and materialized current-state count summary.
- Codex lifecycle hook integration for session registration, heartbeats, stop
  finalization, and write gating.
- An MCP adapter exposing the current-state protocol to compatible tools,
  including a local Bash-write bridge.
- A Codex integration path, including lifecycle hooks, MCP, and an optional
  wrapper that binds Codex runs without forcing a session sandbox by default.
- Repo-local validation profiles for controlled test and check execution.
- Benchmark tooling for SWE-bench pair runs, reports, comparisons, and
  synthetic coordination experiments.

## What It Is Not

`stateful_core` is not a sandbox, access-control system, file lock manager,
distributed lock service, durable secret store, or long-term memory product. It
stores shared operational state that trusted local tools can consult before
coordination-sensitive actions. See the
[security model](SECURITY.md#local-trust-model) for details.

## Install From Source

Prerequisites:

- Rust 1.85 or newer
- Git

Build the workspace:

```bash
cargo build --workspace
```

Install the CLI from this repository:

```bash
cargo install --path crates/stateful-cli
```

The Codex MCP and hook config defaults to the `stateful` binary on `PATH`.
If you do not install it, pass `--binary target/debug/stateful` for repo-local setup.
The benchmark binary is built as `target/debug/stateful-bench`.

## Quick Start

Install the user-level Codex integration. Without `--yes`, this command prints
a dry-run plan. Applying the install merges a marked stateful block into the
Codex config, enables `features.hooks`, initializes the global state DB, and
backs up an existing Codex config before replacing it.

```bash
stateful install --yes
```

Opt the current git repository into stateful enforcement:

```bash
stateful enable
```

Global hooks are gated by the repo allowlist. Disabled repos are left alone:
hooks do not start the server or append outbox records, and MCP calls report
that the repo is not enabled.

Run Codex through the stateful wrapper:

```bash
stateful codex
```

## Useful Next Steps

Start the local server explicitly. Codex hooks also start it lazily for enabled
repos when needed.

```bash
stateful server start
stateful server status
```

Declare what you plan to edit and inspect the active current state. In
`stateful codex`, use the MCP tools directly; lifecycle hooks bind the current
Codex session to the wrapper run and use that run-bound session by default.

```text
state_intent_declare files_planned=["README.md"]
```

Outside Codex hooks, pass session and workspace IDs explicitly.

```bash
stateful intent declare --session-id demo --workspace-id local README.md
stateful current
```

Run controlled write-capable shell commands through the MCP bridge, and check
local installation health outside hooks:

```text
state_bash_write command="printf updated > README.md" write_targets=["README.md"]
```

```bash
stateful validate cargo-test
stateful doctor
```

Repo-local compatibility setup is still available when global Codex config is
not desired:

```bash
stateful init
stateful enable --repo-local-codex
```

## CLI Overview

- `stateful install [--yes] [--codex-config <path>] [--binary <path>]`
  installs global stateful files and merges Codex config. It is a dry run
  unless `--yes` is passed; apply mode backs up an existing Codex config and
  refuses an unmarked pre-existing `[mcp_servers.stateful]` block.
- `stateful enable [--repo <path>] [--repo-local-codex]`, `stateful disable`,
  and `stateful repos list` manage the repo allowlist used by global hooks.
- `stateful init` writes repo-local `.codex/config.toml`, `.stateful/`
  configuration, and the repo-local `stateful-command-policy` skill. It refuses
  to overwrite non-stateful-owned Codex config files.
- `stateful server start` starts the local HTTP state server detached by
  default. Use `stateful server start --foreground`, `stateful server status`,
  and `stateful server stop` for lifecycle control. Bare `stateful server`
  remains a foreground compatibility form.
- `stateful status` and `stateful doctor` report setup health, including global
  install fields and repo-enabled status.
- `stateful current` prints current-state summary counts:
  `session_count`, `active_intent_count`, and `event_count`.
- `stateful events` prints the stored event page currently returned by the
  server, up to 100 rows in insertion order.
- `stateful intent declare <paths...>` declares planned file or directory
  scope. Session and workspace may be explicit or inferred from the current
  hook session file. Directory scopes must end with `/`; otherwise the path is
  treated as an exact file scope.
- `stateful notifications poll` reads pending coordination notifications.
- `stateful resume next` reads the next reservation available to the active
  session.
- `stateful validate <profile>` runs a configured validation profile.
- `stateful commit -m <message> -- <paths...>` creates a structured commit for
  explicit file paths. The `--` separator is required. The wrapper rejects broad
  paths, globs, directories, Git-detected renames, and unrelated staged changes.
- `stateful push [remote branch]` pushes the current branch through a structured
  wrapper. With no arguments it uses the configured upstream; with arguments it
  requires both remote and branch, requires the branch to match the current
  branch, requires a configured remote and clean worktree, and rejects
  force-like or malformed target values.
- `stateful codex [--codex-bin <path>] [--sandbox passthrough|read-only-tmp] [--no-stateful] -- <args...>`
  runs Codex with pass-through session configuration by default while setting a
  run-bound session id. `--sandbox read-only-tmp` remains available as an
  explicit wrapper profile, and `--no-stateful` disables Codex lifecycle hooks
  for that run.
- `stateful mcp serve` exposes the MCP adapter over stdio.
- `stateful mcp call <tool> [arguments_json]` calls an MCP tool. Most tools map
  to the local HTTP server; `state.bash.write` is handled by the CLI bridge
  because it writes repo files after authorization.
- `stateful sync-outbox` replays pending local outbox records to the server. The
  current outbox path is limited to failed hook heartbeat records and minimal
  server-side outbox rows.
- `stateful hook <event>` runs Codex hook integration entry points:
  `session-start`, `user-prompt-submit`, `pre-tool-use`, `post-tool-use`, and
  `stop`.

Run `stateful <command> --help` for command-specific options.

## Codex Hooks and Sessions

Global installation merges stateful MCP and hook configuration into the Codex
config. `stateful enable` opts a repo into enforcement, while disabled repos are
no-ops for hooks and MCP.

The generated hook configuration covers:

- `SessionStart` for `startup`, `resume`, `clear`, and `compact`
- `UserPromptSubmit`
- `PreToolUse` and `PostToolUse` for `Bash`, `apply_patch`, `Edit`, `Write`,
  `file_change`, and `mcp__filesystem__.*`
- `Stop`

`SessionStart` registers the active session and writes the current-session file
used by CLI and MCP calls. `UserPromptSubmit` calls the context-render endpoint;
the server currently returns a placeholder empty context package. `PreToolUse`
authorizes supported write actions and enforces the Bash policy. `PostToolUse`
posts a session heartbeat. `Stop` finalizes activity and releases leases.

The `stateful codex` wrapper sets a run-specific `STATEFUL_CODEX_RUN_ID`, so
MCP calls can prefer the run-bound session file. The legacy
`.stateful_core/runtime/session.json` file is still maintained as a
compatibility fallback.

## Write Authorization

Write authorization is the current implementation's coordination gate. It is a
policy check over shared operational state, not a security boundary or a global
file lock manager.

The v1 authorization API currently supports `write_file`, `delete_file`,
`rename_file`, and `move_file`.

File intent authorizes writes only to the exact file. Directory intent
authorizes writes one or two path segments below that directory, and directory
intent is recognized by a trailing `/` in declared paths. Delete, rename, and
move operations require exact file intents for the affected paths; directory
intent does not authorize them. Rename and move are supported by the policy and
HTTP API, and the current Codex hook bridge extracts write, delete, and
`apply_patch` move targets from supported tool payloads. Writes without matching
active intent are denied, and active leases held by another session block
conflicting writes. A blocked writer can queue with `queue_on_conflict`; after
promotion, the reserved session must reread and retry the write.

Repo file writes should use `state_bash_write` / `state.bash.write` after
declaring intent. The tool requires a command and an explicit repo-relative
`write_targets` list, optionally plus `create_targets` for files that should be
pre-created before sandboxing. The CLI bridge normalizes each target, rejects
paths outside the repo, Git internals, symlink file targets or parent
directories, and current-session mismatches, then authorizes every listed
target with `/v1/authorize` as `write_file`. If any target is denied, the
command is not executed and the response includes both allowed and denied
target lists. When all targets are allowed, the command runs through an OS
sandbox that restricts writes to the listed files. The read scope is broader
than the write scope: macOS allows file reads, and Linux uses a read-only root
bind. `write_targets` must be non-empty, `cwd` must be an existing
repo-relative directory when supplied, write targets must already exist unless
also listed in `create_targets`, and the default command timeout is 300
seconds. The tool waits for the sandboxed command to finish and returns its
exit code, stdout, and stderr.

Known read-only Bash inspection commands are allowed only when top-level Bash
sandbox metadata declares read-only mode, network access disabled, and only
trusted tmp writable roots.
Unknown Bash, write-capable commands, shell write syntax, command substitution,
process substitution, background jobs, and newline-separated command text are
denied. Sandbox metadata inside `tool_input`, and trusted sandbox environment
values, do not authorize Bash.
macOS uses Seatbelt via `/usr/bin/sandbox-exec`; Linux uses bubblewrap
(`bwrap`) with a read-only root bind and writable exact-file binds. The Linux
bubblewrap backend is implemented but has not been verified in this macOS
development environment.

## HTTP And MCP Surface

The HTTP server exposes `/health`, `/v1/current`, `/v1/events`,
`/v1/runtime/identity`, and POST endpoints for session registration,
heartbeats, intent declaration, leases, activity observation/finalization,
authorization, conflict checks, context rendering, reconciliation ack,
validation, notifications, resume, and outbox sync.

`/v1/current` is currently a compact summary with session, active-intent, and
event counts. `/v1/events` returns the stored event page; the materialized event
types currently used for the count summary are session registration, session
heartbeat, and intent declaration. `/v1/context/render` is wired through the
server and Codex hook flow, but currently renders a placeholder empty package
rather than detailed nearby activity.

The MCP adapter exposes Codex-friendly tool names mapped to dotted protocol
names:

- `state_session_register` / `state.session.register`
- `state_session_heartbeat` / `state.session.heartbeat`
- `state_intent_declare` / `state.intent.declare`
- `state_lease_acquire` / `state.lease.acquire`
- `state_lease_release` / `state.lease.release`
- `state_conflicts_check` / `state.conflicts.check`
- `state_reconcile_ack` / `state.reconcile.ack`
- `state_bash_write` / `state.bash.write`
- `state_notifications_poll` / `state.notifications.poll`
- `state_resume_next` / `state.resume.next`

Most MCP tools map directly to HTTP routes. `state.bash.write` is intentionally
CLI-local: it authorizes each requested write/create target before executing the
command in the platform sandbox. It waits for the sandboxed command to finish
and returns the completed command result, including exit code, stdout, and
stderr.

Side-effecting write authorization and intent declaration endpoints require the
`stateful.v1` request envelope with `payload`. Flat legacy bodies are rejected
with `protocol_mismatch` for those paths.

## Validation Profiles

Validation profiles live at `.stateful/validation.yml`. The current runner
executes a repo-defined shell command, enforces `timeout_seconds`, reports
exit-code status, and uses `git status --porcelain` before and after the run to
fail when a path matching `denied_writes` is already dirty or becomes newly
dirty.

`allowed_writes` and `exclusive` are parsed as profile fields, but the current
runner does not yet enforce an exclusive validation lock or a full
allowlist-only write policy. Raw Bash test and build commands are denied by the
Codex hook; use `state_bash_write` / `state.bash.write` in Codex when the
command's write targets can be declared explicitly, or `stateful validate
<profile>` outside Codex for project-specific commands that need controlled
artifact writes. The default repo-local validation template defines
`cargo-test`, `cargo-fmt-check`, and `cargo-clippy`. Dirty development trees can
add local open-tree variants such as `cargo-test-open-tree`,
`cargo-fmt-open-tree`, and `cargo-clippy-open-tree` in
`.stateful/validation.yml`.

## Core Loop

```text
register session and declare intent
-> optionally acquire an advisory lease
-> check conflicts against active leases and reservations
-> queue blocked conflicting writes by resource when requested
-> block supported write actions without active intent
-> authorize or deny supported writes
-> record heartbeats after tools run
-> finalize activity on stop
-> release leases on explicit release or stop finalization
-> reserve the released resource for the next waiter
-> notify the reserved session so it can resume
```

## Generated Local Files

`stateful_core` generates local runtime and integration files. These paths may
contain absolute paths, local configuration, runtime state, benchmark artifacts,
or bearer tokens and are ignored by default:

- `.codex/`
- `.stateful/`
- `.stateful_core/`
- `.stateful_bench/`

Global installation writes under `$STATEFUL_HOME`, or `$HOME/.stateful_core`
when `STATEFUL_HOME` is unset. That directory can contain `config.yml`,
`state.db`, `runtime/server.json`, `runtime/server.lock`, `runtime/server.log`,
and repo metadata under `repos/`.

Repo-local compatibility and hook runtime state live under `.stateful_core/`.
That directory can contain `runtime/server.json`, `runtime/session.json`,
run-bound `runtime/sessions/*.json` files, repo-local `state.db`, and outbox
JSONL files under `outbox/`.

Commit reusable documentation and source code, not local generated state.

## Environment Variables

- `STATEFUL_HOME` overrides the user-level state directory. When unset,
  `$HOME/.stateful_core` is used.
- `STATEFUL_SERVER_URL` and `STATEFUL_SERVER_TOKEN` override runtime discovery
  when both are set.
- `STATEFUL_CODEX_RUN_ID` selects the run-bound current-session file under
  `.stateful_core/runtime/sessions/`. The `stateful codex` wrapper sets it
  automatically.
- `STATEFUL_HOOK_TRUSTED_SANDBOX` is a legacy wrapper constant and does not
  authorize Bash. Use top-level structured sandbox metadata when available.

## Project Layout

- `crates/stateful-core`: domain types, resource scope matching, current-state
  rendering, reconciliation, Bash classification, and policy engine.
- `crates/stateful-store`: SQLite event store and current-state persistence.
- `crates/stateful-server`: local HTTP API over the shared policy and store.
- `crates/stateful-cli`: user-facing CLI, hook adapter, runtime discovery,
  repo registry, structured commit/push wrappers, outbox sync, and validation
  commands.
- `crates/stateful-mcp`: MCP tool surface.
- `crates/stateful-validation`: validation profile parser and runner.
- `crates/stateful-bench`: unpublished benchmark tooling for fetching datasets,
  preparing pairs, sampling, running paired agents, reporting, comparing, and
  synthetic experiments.
- `docs/`: concept, state model, architecture, implementation contract,
  coordination, hardening-scope, and ADR documents.

## Benchmark Tooling

`stateful-bench` currently supports:

- `fetch`: cache SWE-bench Verified rows.
- `prepare-pairs` and `generate-fallback-preflight`: build eligible pair
  manifests.
- `sample`: create deterministic stratified samples.
- `run`: execute no-state or stateful paired-agent runs, with pair filters,
  parallel jobs, and auth, budget, setup, and harness command templates.
- `report` and `compare`: summarize one run or compare stateful/no-state runs
  in JSON or Markdown.
- `synthetic`: run the built-in synthetic coordination benchmark.

Benchmark artifacts live under `.stateful_bench/` and are intentionally ignored.

## Development

In Codex, run write-capable shell commands through the MCP bridge instead of raw
cargo commands when the writes can be declared explicitly:

```text
state_bash_write command="your command" write_targets=["path/to/file"] timeout_seconds=300
```

Some benchmark tests use ignored `.stateful_bench/agent_synthetic/` fixtures. If
those local fixtures are absent, run focused crate tests or regenerate the
benchmark fixtures before running the full workspace suite.

Outside Codex hooks, the same local profiles can be run through the CLI:

```bash
stateful validate cargo-test
stateful validate cargo-fmt-check
stateful validate cargo-clippy
```

When the working tree already has uncommitted source changes, use local
open-tree profiles instead if they are defined in `.stateful/validation.yml`.

## Documentation

- [Core concept](docs/core-concept.md)
- [State model](docs/state-model.md)
- [Architecture](docs/architecture.md)
- [Implementation contract](docs/implementation-contract.md)
- [Current-state coordination](docs/current-state-coordination.md)
- [V1 hardening scope decisions](docs/v1-hardening-scope-decisions.md)
- [ADR 0001: State-first, not memory-first](docs/adr/0001-state-first-not-memory-first.md)

Some local implementation plans and specs may exist under `docs/superpowers/`
for traceability in this working tree. Treat that tree as internal planning
material rather than stable public documentation.

## License

`stateful_core` is licensed under the GNU Affero General Public License version
3.0 only. See [LICENSE](LICENSE).

Why AGPL? `stateful_core` is intended to remain open even when it is used
behind local or network services. The license is part of keeping improvements to
the coordination layer available to the people who depend on it.
