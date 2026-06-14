# stateful_core

Current-state coordination for coding agents working in the same repository.
Human editing signals are part of the target coordination model, but the shipped
prototype is Codex-first: it observes supported agent tools and active file lease
freshness, not editor saves automatically.

Git coordinates committed history. `stateful_core` coordinates active work
before it becomes history.

Before a write occurs, tools can ask a practical question:

```text
Who is doing what now, what might conflict, and when does that claim expire?
```

The project is intentionally about current state, not long-term memory. Memory
recalls what happened before. Current state captures active, scoped, expiring
operational truth that can be checked before writes, test execution, handoff,
and other coordination-sensitive actions.

Git resolves conflicts after they happen. `stateful_core` is designed to let
trusted tools detect and avoid likely write conflicts before the write occurs.

## Status

This repository is an early Rust implementation and local-first, macOS-first
prototype. The current implementation is Codex-first and includes a CLI,
global user-level installation with repo allowlist gating, a local HTTP state
server, MCP adapter, Codex hook adapter, SQLite-backed state store, sandboxed
test execution, sandboxed git operations, outbox sync, and benchmark
tooling. It does not ship a filesystem watcher or IDE save gate for automatic
human edit observation.

APIs, configuration files, and command behavior may change while the project is
pre-release. The current security and support scope is documented in
[SECURITY.md](SECURITY.md).

## Why

Coding agents usually know their own session, but they do not reliably know
what another agent, another session, or a human is doing in the same repository
right now. That creates avoidable failures:

- two agents edit the same file without seeing each other
- a file changes in a shared checkout while an agent still holds a lease on it
- an interrupted session leaves no structured handoff state
- stale memory is treated as active truth
- a tool writes before the session has declared its intended scope
- test execution or reconciliation happens outside the coordination loop

For example, one Codex session can hold a lease on `README.md`, another can keep
updating `docs/`, and a third can wait for the `README.md` lease. The work on
`docs/` can continue while the conflicting `README.md` write is queued instead
of turning into a surprise merge conflict later.

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

Use shared-workspace coordination when separate worktrees or containers would
hide information the actors need right now. Typical cases are:

- the development environment is expensive or fragile to duplicate per agent:
  warm build caches, local dev servers, database state, device state, or
  credentials that are already bound to the canonical checkout
- tightly coupled tasks need each other's uncommitted edits immediately, before
  a commit, rebase, or merge is realistic
- a human is working live in the canonical checkout alongside agents, and the
  agents need to avoid supported writes that collide with fresh shared-state
  claims

When those conditions do not apply, prefer isolation first: separate branches,
worktrees, containers, or task-level orchestration usually give a simpler
failure model. `stateful_core` is complementary to those tools. It is for the
remaining shared-workspace moments where the useful question is not "how do we
merge later?" but "should this write happen here right now?"

The current implementation is Codex-first, but the coordination problem is not
Codex-specific. Any tool that writes into a repository can benefit from a shared
answer to "who is working on this right now?"

Intent and lease are separate on purpose. Intent declares planned work and the
scope a session expects to touch. A lease declares active ownership of a scoped
resource and expires when the session stops being fresh. Intent answers "what
may this session work on?" Lease answers "who is actively holding this now?"

## Wait Queue and Resume

Write conflicts are scoped to the file or resource being modified. If session A
holds an active lease on `src/auth.ts`, sessions B and C can still read,
search, validate, or work on other files. Writes to `src/auth.ts` are blocked.

At the README level, the flow is intentionally simple:

```text
blocked -> queued -> reserved -> claimed -> active
```

A conflicting writer is blocked by the active lease and can enter a FIFO wait
queue. When the active lease is released or the owning activity finalizes, the
first waiter receives a short reservation. The reserved session must reread the
target, call `state.intent.claim` / `stateful intent claim --wait-id <id>`, and
then retry the write. The claim creates write-authorizing intent and the active
same-session lease. The default reservation TTL is 120 seconds.

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
  inspection, intent declaration, MCP, hooks, sandboxed git operations, outbox
  sync, and server lifecycle management.
- A local HTTP state server with token-protected non-health endpoints.
- A SQLite event store and materialized current-state summary.
- Codex lifecycle hook integration for observing and gating important actions.
- An MCP adapter exposing the current-state protocol to compatible tools.
- A Codex integration path, including lifecycle hooks, MCP, and an optional
  wrapper that starts Codex without forcing a session sandbox by default.
- Sandboxed test and check execution through explicit artifact write
  directories.
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
- Codex CLI, when using the bundled lifecycle hooks or `stateful codex`
- A supported OS sandbox backend for `stateful sandbox run`: macOS Seatbelt via
  `/usr/bin/sandbox-exec` is the verified first-class backend. Linux bubblewrap
  support is implemented but experimental and not yet release-verified.

Build the workspace:

```bash
cargo build --workspace
```

Install the CLI from this repository:

```bash
cargo install --path crates/stateful-cli
```

If you do not install it, use `target/debug/stateful` in the commands below.
The benchmark binary is built as `target/debug/stateful-bench`.

## Quick Start

Install the user-level stateful files. Without `--yes`, this command prints a
dry-run plan.

```bash
stateful install --yes
```

Install the Codex integration when you want global Codex hooks, MCP, and the
stateful command-policy skill:

```bash
stateful install --agent codex --yes
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

### LAN Runtime Sharing

Use LAN mode when one Mac should host the stateful HTTP runtime for another Mac.
The MCP adapter still runs locally inside each Codex process, and the remote
Mac should reach the runtime through an SSH tunnel so the bearer token is not
sent to a non-loopback `http://` address.

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
endpoint instead.

If the host uses `stateful server start --workspace-id <id>`, run the printed join
command as-is; it includes the matching workspace id.

## Useful Next Steps

The commands in this section are optional diagnostics and manual coordination
examples. In normal `stateful codex` use, lifecycle hooks and MCP bind the
current session; stateful hook messages tell the agent when an explicit
coordination step is needed.

Start the local server explicitly. Codex hooks also start it lazily for enabled
repos when needed.

```bash
stateful server start
stateful server status
```

For manual CLI use outside the default Codex workflow, declare what you plan to
edit and inspect the active current state. In `stateful codex`, lifecycle hooks
bind the current Codex thread id as the Stateful session id by default. Outside
hooks, pass session and workspace IDs explicitly.

```bash
stateful intent declare --purpose "Update README content requested by the user." README.md
stateful current
```

```bash
stateful intent declare --session-id demo --workspace-id <workspace> --purpose "Update README content requested by the user." README.md
```

Run tests normally from a plain checkout:

```bash
cargo test --workspace
```

Run tests in a stateful sandbox from inside an active `stateful codex` session
and check local installation health. The sandbox runner reads the current
session file created by lifecycle hooks, so declaring an arbitrary
`--session-id` in a plain terminal is not enough for `--fs write-targets`.

```bash
stateful intent declare --purpose "Run the workspace test suite." tmp/
stateful mcp call state_lease_acquire '{"session_id":"<current-session>","workspace_id":"<workspace>","path":"tmp/"}'
stateful sandbox run --fs build --network enabled --command 'cargo test --workspace'
stateful doctor
```

## CLI Overview

- `stateful install [--yes]` installs global stateful files. It is a dry run
  unless `--yes` is passed.
- `stateful install --agent codex [--yes] [--codex-config <path>] [--binary <path>]`
  installs global stateful files, installs the global
  `stateful-command-policy` skill, and merges Codex config.
- `stateful enable [--repo <path>]`, `stateful disable`, and
  `stateful repos list` manage the repo allowlist used by global hooks.
- `stateful server start` starts the HTTP state server detached by default and
  always prints `stateful server join ...` commands. Commands for LAN-reachable
  hosts target loopback and are intended to be used after creating an SSH
  tunnel. Use `stateful server start --foreground`,
  `stateful server restart`, `stateful server status`, and
  `stateful server stop` for lifecycle control. Bare `stateful server` remains
  a foreground compatibility form.
- `stateful server join` installs global MCP config and writes remote runtime
  discovery, enabling the current repo only when `--enable-repo` is supplied.
- `stateful status` and `stateful doctor` report setup health, including global
  install fields and repo-enabled status. Legacy `.codex/hooks.json` and
  repo-local `.stateful_core/state.db` artifacts are labeled separately and do
  not count as an installed current integration.
- `stateful current` prints current-state summary counts.
- `stateful events` prints recent stored state events.
- `stateful intent declare --purpose <purpose> <paths...>` declares planned file
  or directory scope. At least one non-empty path is required. The purpose is
  required and must be supplied by the caller,
  inferred from the user or agent instruction when it is not explicit. Session
  and workspace may be explicit or inferred from the current hook session file.
  Declarations add to that session's active scope in that workspace; if the same
  path is declared again, the latest matching declaration supplies the purpose
  used for future lease acquisition.
- `stateful intent request --request-id <id> --action write_file|write_directory
  --path <path> --purpose <purpose>` creates or returns an idempotent
  queued/reserved write request. Retrying the same `request_id` after its
  reservation expires requeues the same waiter instead of creating a duplicate.
  The path must be non-empty after normalization.
- `stateful intent claim --wait-id <id>` claims a reserved request after the
  session rereads the target; the claim creates write-authorizing intent and an
  active lease for the reservation owner.
- `stateful intent cancel --request-id <id>` cancels a queued or reserved
  request owned by the active session.
- `stateful notifications poll` reads pending coordination notifications.
- `stateful resume next` reads the next reservation available to the active
  session.
- `stateful sandbox run --fs build --network enabled --command <cmd>` runs
  build or test commands with artifact writes limited to `tmp/` after exact
  directory intent and a successful same-session directory lease. The profile is
  language-independent for filesystem access and sets `CARGO_TARGET_DIR` to
  `tmp/target` for Cargo commands; configure other tool-specific build caches
  to live under `tmp/` when they do not use standard temp variables.
- `stateful sandbox run --fs write-targets --write-dir tmp --command <cmd>`
  runs command-shaped artifact writes with writes limited to `tmp/` after exact
  directory intent and a successful same-session directory lease.
- `stateful sandbox run --fs git --network enabled --command 'git <args>'`
  runs a single git command with the repo worktree and Git internals writable
  inside the OS sandbox. The wrapper rejects shell-dispatching git options and
  local config persistence surfaces such as `git init`, branch upstream/tracking
  setters, `push -u`, and config-mutating `git remote` subcommands such as
  `add`, `set-url`, `rename`, and `remove`. Use `--network disabled` for
  local-only git operations.
- `stateful codex [--codex-bin <path>] [--sandbox passthrough|read-only-tmp] [--no-stateful] -- <args...>`
  runs Codex with pass-through session configuration by default.
  `--sandbox read-only-tmp` remains available as a Codex filesystem profile,
  not as a Bash authorization signal, and `--no-stateful` disables Codex
  lifecycle hooks for that run.
- `stateful mcp serve` exposes the MCP adapter over stdio.
- `stateful mcp call <tool> [arguments_json]` calls an MCP tool. Most tools map
  to the local HTTP server. Stale `state_file_write` / `state.file.write` and
  `state_bash_write` / `state.bash.write` calls are removed; use native Codex
  edit tools such as `apply_patch` or Edit for file edits after exact intent
  declaration and a successful same-session file lease, and use
  `stateful sandbox run --fs write-targets ... --command ...` for
  command-shaped writes.
- `stateful sync-outbox` replays pending local outbox records to the server.
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
- `PreToolUse` for all tools, with explicit allow/deny classification in the
  hook
- `PostToolUse` for `Bash`, `apply_patch`, `Edit`, `Write`, `file_change`,
  and `mcp__filesystem__.*`
- `Stop`

`SessionStart` registers the active session and writes the current-session file
used by CLI and MCP calls. `UserPromptSubmit` renders brief current-state
context. `PreToolUse` authorizes supported tool actions; server-side
authorization records an implicit session heartbeat for the checked session.
`PostToolUse` records activity or heartbeats and refreshes same-session exact
file lease observations after supported file tools complete. `Stop` records
turn-end activity without finalizing or releasing leases; explicit
`state.activity.finalize` finalizes activity and releases the session's leases.

Hooks use the runtime payload `session_id` as the Stateful session id. They
write `.stateful_core/runtime/sessions/<session_id>.json` plus the current
session alias `.stateful_core/runtime/session.json`. Session-bound callers use
`STATEFUL_SESSION_ID` to select the matching session-bound file. If that
variable is not present, they fall back to `.stateful_core/runtime/session.json`
only after verifying that its `session_id` has a matching session-bound file
with identical contents. The fallback is rejected when the legacy alias does not
match the session-bound file, or when multiple session-bound files exist because
the alias is ambiguous across concurrent sessions; set `STATEFUL_SESSION_ID` to
the active session id in that case.

## Write Authorization

Write authorization is the current implementation's coordination gate. It is a
policy check over shared operational state, not a security boundary or a global
file lock manager.

The v1 authorization API currently supports `write_file`, `write_directory`,
`delete_file`, `rename_file`, and `move_file`.

File intent authorizes writes only to the exact file. Directory intent
authorizes writes one or two path segments below that directory. Delete,
rename, and move operations require exact file intents for the affected paths;
directory intent does not authorize them. Writes without matching active intent
are denied, and active leases held by another session block conflicting writes.
The depth limit applies to `write_file` authorization from a directory intent.
`write_directory` is separate: it requires exact directory intent, and the
matching directory lease fences the whole directory subtree because
`--write-dir` grants a command-shaped sandbox writable access to that subtree.
A blocked writer can queue with `queue_on_conflict`; after promotion, the
reserved session must reread the target, claim the reservation with
`state.intent.claim` / `stateful intent claim --wait-id <id>`, and then retry
the write.

Repo file edits should use native Codex edit tools such as `apply_patch`, Edit,
or `Write` after exact intent declaration and a successful same-session file lease. Hooks
extract the native tool target, call `/v1/authorize` with the
operation-specific action such as `write_file`, `delete_file`, or `move_file`
with source `path` / `old_path` and destination `new_path`, and allow the edit
only after an allow decision.

Raw Bash commands are denied by stateful hooks. Bash tool calls are authorized
only when the outer command is a single strict invocation of the trusted
absolute `stateful` binary running
`<absolute-stateful-binary> sandbox run ... --command <cmd>` or
`<absolute-stateful-binary> external-run ...`. Use
`<absolute-stateful-binary> sandbox run --fs read-only --network disabled
--command <cmd>` for Bash-hook command-shaped read-only inspection that needs a
shell, and use `--fs write-targets` with explicit targets for Bash-hook
command-shaped writes. Git operations use `--fs git`, which accepts a single
`git ...` command, rejects explicit write targets, and opens the repo worktree
and Git internals as the writable sandbox scope while filtering
shell-dispatching options, branch upstream/tracking persistence, `git init`,
`push -u`, and config-mutating `git remote` subcommands.

Build and test commands should use
`stateful sandbox run --fs build --network enabled --command <cmd>` after exact
`tmp/` directory intent and a successful same-session directory lease. The build
profile grants writable `tmp/`, standard temp variables, and `CARGO_TARGET_DIR`
pointing at `tmp/target`; tools with other language-specific build directories
should be configured to place those directories under `tmp/`.

Other command-shaped writes should use
`stateful sandbox run --fs write-targets --write-target <path> ... --command <cmd>`,
optionally with `--create-target` for files that should be pre-created before
sandboxing. Artifact-producing commands that are not build/test commands should
declare a directory intent such as `tmp/`, acquire the same-session directory
lease, and use `--write-dir tmp`. Inside a Bash hook tool call, the
outer executable must be the trusted absolute binary path from the hook
configuration, for example `<absolute-stateful-binary> sandbox run --fs
write-targets ... --command <cmd>`. The wrapper authorizes `--write-target` and
`--create-target` entries with `/v1/authorize` as `write_file`, and authorizes
`--write-dir` entries as `write_directory`; if any target is denied, the command
is not executed and the response includes both allowed and denied target lists.
When all targets are allowed, the command runs through an OS sandbox with only
the listed files or directory subtrees writable. This is write confinement, not
full process containment: sandboxed commands can still read host files permitted
by the OS sandbox profile. The hook and runner reject
`--fs read-only --network enabled` so read-only shell inspection cannot combine
broad host reads with network egress. macOS uses Seatbelt via
`/usr/bin/sandbox-exec`; this is the verified first-class backend. Linux
bubblewrap (`bwrap`) support is implemented with a read-only root bind plus
writable file and directory binds, but it is experimental until it is verified
in a Linux release environment.

Repo-external command-shaped writes use `stateful external-run`, not
`sandbox run`. `external-run` classifies targets by normalized path: targets
that resolve inside the repo are rejected, while targets outside the repo can
be listed with `--write-target`, `--create-target`, or `--write-dir`. These
requests do not require intent or lease. Instead, the first command records an
approval request and prints a copy-paste command for a user to approve and run:

```bash
stateful external-run request \
  --purpose "install rebuilt stateful binaries" \
  --write-dir "$HOME/.cargo/bin" \
  --command 'install -m 755 target/release/stateful "$HOME/.cargo/bin/stateful"'
```

The output includes the purpose, normalized write scope, command, and an
approval command like:

```bash
<absolute-stateful-binary> external-run approve <request-id> --run
```

## HTTP And MCP Surface

The HTTP server exposes `/health`, `/v1/current`, `/v1/events`,
`/v1/runtime/identity`, and POST endpoints for session registration,
heartbeats, intent declaration, intent request, intent claim, intent cancel,
leases, activity observation/finalization, authorization, conflict checks,
context rendering, reconciliation ack, notifications, resume, and outbox sync.

The MCP adapter exposes Codex-friendly tool names mapped to dotted protocol
names:

- `state_session_register` / `state.session.register`
- `state_session_heartbeat` / `state.session.heartbeat`
- `state_intent_declare` / `state.intent.declare`
- `state_intent_request` / `state.intent.request`
- `state_intent_claim` / `state.intent.claim`
- `state_intent_cancel` / `state.intent.cancel`
- `state_lease_acquire` / `state.lease.acquire`
- `state_lease_release` / `state.lease.release`
- `state_activity_observe` / `state.activity.observe`
- `state_activity_finalize` / `state.activity.finalize`
- `state_conflicts_check` / `state.conflicts.check`
- `state_current_read` / `state.current.read`
- `state_events_read` / `state.events.read`
- `state_context_render` / `state.context.render`
- `state_reconcile_ack` / `state.reconcile.ack`
- `state_notifications_poll` / `state.notifications.poll`
- `state_resume_next` / `state.resume.next`

Most MCP tools map directly to HTTP routes. `state_file_write` /
`state.file.write` and `state_bash_write` / `state.bash.write` were removed.
Use native Codex edit tools for file edits after exact intent declaration and a
successful same-session file lease, and use
`stateful sandbox run --fs write-targets ... --command ...` for command-shaped
writes. Use `stateful sandbox run --fs git ... --command 'git <args>'` for git
operations; config-mutating `git remote` commands such as `remote add` and
`remote set-url`, `git init`, branch upstream/tracking setters, and `push -u`
are rejected by the git profile.

The `/v1/authorize` endpoint and the intent declare/request/claim/cancel
endpoints require the `stateful.v1` request envelope with `payload`. Flat
legacy bodies are rejected with `protocol_mismatch` for those paths. Other POST
routes still accept their current flat request bodies.
Intent declare and request payloads require a non-empty `purpose`; clients must
infer it from the user or agent instruction and send it explicitly. Intent
declare payloads also require non-empty `files_planned`; empty arrays and empty
or normalized-empty paths are rejected with `missing_scope`. Intent request
payloads also reject empty or normalized-empty `path` with `missing_scope`.

## Sandboxed Tests

Raw Bash test commands are denied by hooks. Use the trusted `stateful sandbox
run` wrapper after exact `tmp/` directory intent and a successful same-session
directory lease before commands that write build output:

```bash
stateful intent declare --session-id <session> --workspace-id <workspace> --purpose "Run the requested tests." tmp/
stateful mcp call state_lease_acquire '{"session_id":"<session>","workspace_id":"<workspace>","path":"tmp/"}'
stateful sandbox run --fs build --network enabled --command 'cargo test --workspace'
```

Use `--network enabled` when tests bind or connect loopback sockets. The build
profile is limited to the `tmp/` artifact tree; source file edits should use
native Codex edit tools such as `apply_patch` or Edit after exact intent and a
successful same-session file lease.

## Core Loop

```text
observe session or tool activity
-> register session and declare intent
-> acquire or refresh advisory lease
-> check conflicts against active current state
-> queue blocked conflicting writes by resource
-> block write actions without active intent
-> authorize, warn, or block remaining important actions
-> record activity and heartbeat
-> finalize as done, failed, or blocked
-> release leases on explicit release or finalization
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

Repo hook runtime state lives under `.stateful_core/`. That directory can
contain `runtime/server.json`, `runtime/session.json`, session-bound
`runtime/sessions/*.json` files, local `state.db`, and outbox JSONL files
under `outbox/`.

Commit reusable documentation and source code, not local generated state. For
public source archives, prefer `git archive` or a clean clone instead of a
working-tree tarball so ignored runtime and benchmark artifacts are not
bundled.

## Environment Variables

- `STATEFUL_HOME` overrides the user-level state directory. When unset,
  `$HOME/.stateful_core` is used.
- `STATEFUL_SERVER_URL` and `STATEFUL_SERVER_TOKEN` override runtime discovery
  when both are set. The referenced server must expose the current runtime
  capabilities, including sandbox write-directory authorization.
- `STATEFUL_SESSION_ID` selects the session-bound current-session file at
  `.stateful_core/runtime/sessions/<session_id>.json` for MCP tools and other
  session-bound callers. Codex-specific session env aliases are ignored.
- `STATEFUL_HOOK_TRUSTED_SANDBOX` is a legacy integration signal and does not
  authorize Bash. Bash authorization goes through a trusted
  `<absolute-stateful-binary> sandbox run` wrapper command.

## Project Layout

- `crates/stateful-core`: domain types, resource scope matching, current-state
  rendering, reconciliation, and pure policy primitives.
- `crates/stateful-store`: SQLite event store and current-state persistence.
- `crates/stateful-server`: local HTTP API and store-backed policy service over
  the core primitives and SQLite store.
- `crates/stateful-cli`: user-facing CLI, hook adapter, runtime discovery,
  repo registry, outbox sync, and sandbox wrappers for command-shaped writes and
  git operations.
- `crates/stateful-mcp`: MCP tool surface.
- `crates/stateful-bench`: benchmark tooling for fetching datasets, preparing
  pairs, sampling, running paired agents, reporting, comparing, and synthetic
  experiments.
- `docs/`: concept, state model, architecture, implementation contract,
  coordination, hardening-scope, ADR, and selected historical
  implementation-plan documents.

## Benchmark Tooling

`stateful-bench` currently supports:

- `fetch`: cache SWE-bench Verified rows.
- `prepare-pairs` and `generate-fallback-preflight`: build eligible pair
  manifests.
- `sample`: create deterministic stratified samples.
- `run`: execute no-state or stateful paired-agent runs.
- `report` and `compare`: summarize one run or compare stateful/no-state runs.
- `synthetic`: run the built-in synthetic coordination benchmark.
- `denovo`: wrap the official AweAgent DeNovoSWE recipe for extract, run,
  report, and compare workflows while recording `stateful`, `subagent`, and
  `running_time_ms` comparison axes.

The synthetic benchmark is a deterministic fixture for exercising report and
comparison plumbing. Treat its positive delta as a smoke test for the metric
pipeline, not as empirical evidence that real paired agents avoid conflicts.
`compare` emits `evidence_kind`, `empirical_claim_allowed`, structured
`evidence_notes`, and `coordination_effects`, including prevented same-file
collisions, prevented lost edits, additional coordination-friction events, and
additional wall time. The current synthetic fixture reports 4 prevented
same-file collisions, 4 prevented lost edits, and 10 additional friction events;
those numbers are still fixture output and render as `synthetic_fixture`, not
empirical evidence. This repository does not currently ship a checked-in
empirical paired-agent stateful/no-state result. Use `run` plus `compare` on a
reviewed manifest before citing benchmark evidence for real conflict prevention.

Benchmark artifacts live under `.stateful_bench/` and are intentionally ignored.

### DeNovoSWE Official Wrapper

`stateful-bench denovo` follows the official AweAgent DeNovoSWE workflow instead
of reimplementing the evaluator. Provide an AweAgent checkout with
`--aweagent-root` or `AWEAGENT_ROOT`; the wrapper invokes
`recipes/denovo_swe/extract_patch.py` and `recipes/denovo_swe/run.py` from that
checkout.

Preprocess raw DeNovoSWE JSONL:

```bash
stateful-bench denovo extract \
  --aweagent-root ../AweAgent \
  --input /path/to/ready_denovoswe.jsonl \
  --output .stateful_bench/denovo/extracts/dev \
  --config configs/tasks/denovoswe.yaml \
  --max-concurrent 10
```

Run a comparison matrix. If no `--condition` is provided, the wrapper records
the four canonical axis combinations. To make an axis change agent behavior,
provide official-compatible AweAgent configs through `--condition` entries.

```bash
stateful-bench denovo run \
  --aweagent-root ../AweAgent \
  --data-file .stateful_bench/denovo/extracts/dev/extract_patch_*/results.jsonl \
  --run-id dev-denovo \
  --condition stateful:off,subagent:off,config:configs/tasks/denovoswe.yaml \
  --condition stateful:on,subagent:off,config:configs/tasks/denovoswe-stateful.yaml \
  --condition stateful:off,subagent:on,config:configs/tasks/denovoswe-subagent.yaml \
  --condition stateful:on,subagent:on,config:configs/tasks/denovoswe-stateful-subagent.yaml \
  --mode batch \
  --max-concurrent 4 \
  --eval-iters 1
```

Generate reports:

```bash
stateful-bench denovo report \
  --run-dir .stateful_bench/denovo/runs/dev-denovo \
  --format markdown
```

`running_time_ms` is measured around each official recipe process invocation.
Official DeNovoSWE clean, test patch, binary fixture, evaluation, and anti-hack
semantics remain delegated to AweAgent.

## Development

Run the Rust test suite:

```bash
cargo test --workspace
```

Some benchmark tests validate ignored `.stateful_bench/agent_synthetic/`
fixtures when they are present. The local fixture validation is skipped when
those files are absent; regenerate the benchmark fixtures before changing or
evaluating chaos manifest coverage.

Run formatting and lint checks:

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
```

## Documentation

- [Core concept](docs/core-concept.md)
- [State model](docs/state-model.md)
- [Architecture](docs/architecture.md)
- [Implementation contract](docs/implementation-contract.md)
- [Current-state coordination](docs/current-state-coordination.md)
- [V1 hardening scope decisions](docs/v1-hardening-scope-decisions.md)
- [ADR 0001: State-first, not memory-first](docs/adr/0001-state-first-not-memory-first.md)

Some historical implementation plans and specs are tracked under
`docs/superpowers/` for traceability. New local scratch plans under that tree
are ignored by default.

## License

`stateful_core` is licensed under the GNU Affero General Public License version
3.0 only. See [LICENSE](LICENSE).

Why AGPL? `stateful_core` is intended to remain open even when it is used
behind local or network services. The license is part of keeping improvements to
the coordination layer available to the people who depend on it.
