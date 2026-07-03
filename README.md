# stateful_core

Current-state coordination for coding agents working in the same repository.

Git coordinates committed history. `stateful_core` coordinates active work before
it becomes history. Before a supported write occurs, a tool can ask:

```text
Who is doing what now, what might conflict, and when does that claim expire?
```

The project is intentionally about current state, not long-term memory. Memory
recalls what happened before; current state captures active, scoped, expiring
operational truth that can be checked before writes, test execution, handoff, and
other coordination-sensitive actions.

## Status

This repository is an early Rust implementation and local-first, macOS-first
prototype. The current implementation is Codex-first with OMP support. It
includes a CLI, global user-level installation, repo allowlist gating, a local
HTTP state server, native Stateful coordination tools, Codex and OMP hook
adapters, SQLite-backed state store, sandboxed command profiles, outbox sync,
and benchmark tooling.

These docs describe the shipped local v1 coordination mechanism. There is not
yet a checked-in empirical paired-agent stateful/no-state result.

It does not ship a filesystem watcher or IDE save gate for automatic human edit
observation. Human editing signals remain part of the target coordination model,
not a fully shipped observer path.

APIs, configuration files, and command behavior may change while the project is
pre-release. The current security and support scope is documented in
[SECURITY.md](SECURITY.md).

## Why

Coding agents usually know their own session, but they do not reliably know what
another agent, another session, or a human is doing in the same repository right
now. That creates avoidable failures:

- two agents edit the same file without seeing each other
- a file changes in a shared checkout while an agent still holds a claim on it
- an interrupted session leaves no structured handoff state
- stale memory is treated as active truth
- a tool writes before the session has declared its intended scope
- test execution or reconciliation happens outside the coordination loop

`stateful_core` provides a small protocol for declaring reservation, tracking active
claims, recording session activity, reading current-state summaries, queuing
blocked writers, and blocking supported writes that have no matching active
reservation.

## When To Use It

`stateful_core` does not replace Git, branches, worktrees, editors, or agent
runners.

Use shared-workspace coordination when separate worktrees or containers would
hide information the actors need right now. Typical cases:

- the development environment is expensive or fragile to duplicate per agent:
  warm build caches, local dev servers, database state, device state, or
  credentials already bound to the canonical checkout
- tightly coupled tasks need each other's uncommitted edits immediately, before a
  commit, rebase, or merge is realistic
- a human is working live in the canonical checkout alongside agents, and the
  agents need to avoid supported writes that collide with fresh shared-state
  claims

This does not mean automatic filesystem or IDE observation of human edits is
shipped. Current protection depends on explicit/shared-state signals and
supported write paths.

When those conditions do not apply, prefer isolation first: separate branches,
worktrees, containers, or task-level orchestration usually give a simpler failure
model. `stateful_core` is for the remaining shared-workspace moments where the
useful question is not "how do we merge later?" but "should this write happen
here right now?"

## What It Provides

- A `stateful` CLI for installation, repo enablement, status/current-state
  inspection, reservation declaration, hooks, sandboxed command profiles, outbox
  sync, and server lifecycle management.
- A local HTTP state server with token-protected non-health endpoints.
- A SQLite event store and materialized current-state summary.
- Codex and OMP lifecycle hook integration for observing and gating important
  actions.
- Native Stateful coordination tools exposed by the active agent harness.
- Sandboxed profiles for build/test output, command-shaped repo writes, git
  operations, GitHub PR commands, and repo-external shell work.
- Benchmark tooling for SWE-bench pair runs, reports, comparisons, synthetic
  coordination experiments, and DeNovoSWE/ProgramBench adapters.

`stateful_core` is not a sandbox, access-control system, file lock manager,
distributed lock service, durable secret store, or long-term memory product. It
stores shared operational state that trusted local tools can consult before
coordination-sensitive actions. See the
[security model](SECURITY.md#local-trust-model) for details.

## Install From Source

Prerequisites:

- Rust 1.85 or newer
- Git
- Codex CLI, when using bundled Codex hooks or `stateful codex`
- OMP, when using the generated OMP `stateful` profile
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

If you do not install it, use `target/debug/stateful` in the commands below. The
benchmark binary is built as `target/debug/stateful-bench`.

## Quick Start

Install the user-level stateful files. Without `--yes`, this command prints a
dry-run plan.

```bash
stateful install --yes
```

### Codex

Install Codex integration when you want global Codex hooks,
`skills/stateful-command-policy/` (`SKILL.md`, `omp-tools.md`,
`sandbox-tools.md`, `denial-recovery.md`, `subagent-write-recovery.md`), and
`skills/dispatching-parallel-agents/SKILL.md`:

```bash
stateful install --agent codex --yes
stateful enable
stateful codex
```

### OMP

Install OMP integration when you want the isolated OMP `stateful` profile,
stateful hooks, native Stateful tool injection, built-in Bash preflight, OMP
edit/write auto-declare/claim for missing scope, lazy resume fallbacks, and
`skills/stateful-command-policy/` (`SKILL.md`, `omp-tools.md`,
`sandbox-tools.md`, `denial-recovery.md`, `subagent-write-recovery.md`):

```bash
stateful install --agent omp --yes
stateful enable
```

Check local setup health:

```bash
stateful status
stateful doctor
```

For a no-source-change first use, inspect current state:

```bash
stateful current
```

Then read the write-flow notes below and use either the active-session native
tools path or the manual reservation/claim path before making writes.

## Day-To-Day Coordination

In normal `stateful codex` or OMP `stateful` profile use, lifecycle hooks bind
the active `agent_id` and `workspace_id` for state operations. OMP derives its
Stateful `agent_id` only from `ctx.sessionManager`: `getSessionId()` supplies the
required session UUID and `getLeafId()`, when present, supplies the active branch.
Stateful uses `omp-${sessionId}-${leafId}` when a leaf id exists and
`omp-${sessionId}` otherwise. If `getSessionId()` is unavailable or invalid, OMP
Stateful actions fail closed instead of reading event/ctx identity fields or
inventing process, environment, or current-session-file identity. Codex hooks map
Codex's hook `session_id` parameter to Stateful `agent_id`. There is no
environment-variable fallback path for agents to maintain. Hook messages tell
the agent when an explicit
coordination step is needed.

Manual CLI use outside an active agent session can declare scope, keep the
returned reservation id, and inspect the current state:

```bash
reservation_id=$(stateful reservation declare --purpose "Update README content requested by the user." README.md | jq -r '.reservation_id')
stateful current
```

Inside an active Codex or OMP session, use the active Stateful coordination tools
directly instead of shelling out to `stateful reservation declare`. Simple OMP
native `edit`/`write` calls can rely on auto-declare/claim when no explicit
reservation id is supplied and the only denial is missing reservation/scope. The
explicit multi-resource write flow remains:

```text
read current state -> declare task reservation with known file set -> keep reservation_id -> acquire exact same-reservation claims for reserved paths -> reread targets -> write with the same reservation_id
```

Reservation and claim are separate on purpose. A reservation groups the task's
known file and directory scopes under one purpose and one `reservation_id`, and
can be expanded when the task discovers another target. Claim acquisition uses
`reservation_id` plus `paths: string[]` so callers can acquire a batch from that
reservation in one request. Each resulting claim still owns one exact file or
directory resource and expires when the agent stops being fresh.

For native OMP `edit` and `write`, this fallback is the default simple-write
path: when no explicit reservation id is supplied and the only denial is missing
reservation/scope, the extension declares the exact file scope, acquires
same-reservation claims, and retries authorization. When another active claim
blocks a write, the writer can queue for that resource. When the resource is
released or expires, the server reserves it for the next eligible waiter and
stores a resume notification with a monotonic sequence for that target agent in
that workspace. Polling returns pending notifications and marks the returned
rows delivered. The SSE stream sends `id: <sequence>` plus the same `sequence`
in JSON data without marking the notification delivered immediately; on
reconnect, `Last-Event-ID` / `last-event-id` acknowledges notifications through
that sequence and the server streams later pending notifications. During
queued-reservation compatibility, wait records expose the same id as both
`wait_id` and `reservation_id`; use that id for the eventual claim and write. In
OMP, queued/conflicting edits, writes whose target changed before replay,
unavailable authorization runtime, unsupported targets, explicit bad reservation
ids, other denials, and external Bash commands waiting on a scoped grant remain
lazy-resume cases. Agents call
`lazy_edit_resume` for strict line-based patch replay, `lazy_write_resume` for
captured write replay, or `lazy_bash_resume` to rerun a blocked external Bash
command after approving its grant. If a lazy edit/write operation captured a
`wait_id`, OMP first claims that queued reservation, then re-authorizes and
applies with stale-target guards. Generated no-wait operation ids still require
resolving the missing scope or claim externally before resume.

Detailed queue states, claim expiry behavior, and promotion rules are documented
in [State model](docs/state-model.md),
[Current-state coordination rationale/index](docs/current-state-coordination.md), and
[Concurrency control spec](docs/concurrency-control-spec.md).

## Command Execution

Use native read/search tools for ordinary inspection. Do not use raw shell tools
inside enabled Codex or OMP sessions when a stateful-native path exists.

| Need | Use |
| --- | --- |
| Simple OMP repo file edit | Native `edit`/`write`; auto-declare/claim handles missing reservation/scope when no explicit reservation id is supplied |
| Other repo file edit | Native edit/write tools after task-level reservation and exact same-reservation file claim |
| Build or test command | `stateful sandbox run --fs build --network enabled --write-dir <scratch-purpose> --command <cmd>` |
| Command-shaped repo write | `stateful sandbox run --fs write-targets --reservation-id <reservation_id> --write-target <file> --command <cmd>` |
| Local git operation | `stateful sandbox run --fs git --network disabled --command 'git <args>'` |
| Remote git operation | Git sandbox profile with `--network enabled` |
| GitHub PR list/view/status/create | `stateful sandbox run --fs github-pr --network enabled --command 'gh pr <list|view|status|create> ...'` |
| Repo-external shell operation | `stateful sandbox run --fs external --purpose <purpose> --command <cmd>` for reads; add exact `--write-target`, `--create-target`, or `--write-dir` scopes for writes |

Use repeated `--sequence <cmd>` instead of outer Bash `&&`/`;` when a sandboxed operation needs multiple setup steps. Stateful compiles the sequence into one sandbox-internal script, so the outer Bash call remains a single trusted `stateful sandbox run ...` command. Add `--sequence-shell /bin/zsh` only when the script needs a shell other than `/bin/sh`. The `git` and `github-pr` profiles reject `--sequence` because they validate one direct `git` or `gh pr` command.

By default, `stateful sandbox run` passes the wrapped command's `stdout`,
`stderr`, and exit code through unchanged. Add `--json` when automation needs the
structured result envelope with `status`, `exit_code`, captured streams, and
authorization metadata.

In OMP, built-in Bash may run only strict trusted `stateful sandbox run ...`
and `stateful sandbox process find ...` commands. Bare `stateful` is trusted
only after session-start or per-tool preflight hash-verifies the first PATH
`stateful` binary against the installed Stateful binary; otherwise use the
installed absolute binary path. External write/create/write-dir/socket/signal
scope and repo-external OMP native `edit`/`write` file targets auto-approve the
scoped Stateful-owned OMP grant prompt by default through
`stateful.autoApprove: true`, while sandbox scope validation, hooks,
reservation/claim checks, and grant limits still apply. Set
`stateful.autoApprove: false` to require the prompt. Repo-internal native OMP
`edit` and `write` use auto-declare/claim as the default simple-write path when
no explicit reservation id is supplied and
the only denial is missing reservation/scope. Use `lazy_edit_resume` for
queued/conflicting line-based OMP `edit` patches and `lazy_write_resume` for
captured full OMP `write` replay; captured wait ids are claimed before
re-authorization, while generated no-wait ids still require the missing scope or
claim to be resolved externally. Use `lazy_bash_resume` to rerun a queued
external Bash command after the scoped grant is approved.

See [Usage reference](docs/usage-reference.md) for detailed CLI, hook, sandbox,
LAN sharing, generated-file, and release notes.

## Architecture At A Glance

```text
observe session or tool activity
-> register session and declare reservation
-> acquire or refresh exact advisory claims for reserved paths
-> authorize, queue, warn, or block coordination-sensitive actions
-> record activity and heartbeat
-> finalize activity and release claims
-> reserve released resources for queued waiters
-> notify reserved sessions so they can resume
```

The state server owns policy, persistence, TTLs, and conflict checks. Codex/OMP
hooks observe and gate important agent actions. Native Stateful tools give
agents a structured way to read and update coordination state. See
[Architecture](docs/architecture.md) and
[Implementation contract](docs/implementation-contract.md) for the concrete API,
hook, runtime, storage, and test contracts.

## LAN Runtime Sharing

Use LAN mode when one Mac should host the stateful HTTP runtime for another Mac.
The remote machine should connect through an SSH tunnel so the bearer token is
not sent to a non-loopback `http://` address.

```bash
# host Mac
stateful server start --host 0.0.0.0 --workspace-id shared

# remote Mac, after creating the SSH tunnel printed by the host
stateful server join http://127.0.0.1:43873 --token <token> --enable-repo
```

`stateful server join` rejects non-loopback plain `http://` URLs unless explicitly
allowed. See [Usage reference](docs/usage-reference.md#lan-runtime-sharing) for
the full flow.

## Generated Local Files

`stateful_core` generates local runtime and integration files. These paths may
contain absolute paths, local configuration, runtime state, benchmark artifacts,
or bearer tokens and are ignored by default:

- `.codex/`
- `.stateful/`
- `.stateful_core/`
- `.stateful_bench/`

Global installation writes under `$STATEFUL_HOME`, or `$HOME/.stateful_core`
when `STATEFUL_HOME` is unset. Commit reusable documentation and source code, not
local generated state.

## Project Layout

- `crates/stateful-core`: domain types, resource scope matching, current-state
  rendering, reconciliation, and pure policy primitives.
- `crates/stateful-store`: SQLite event store and current-state persistence,
  split by mutation domain for claims, reservations, notifications, and activity.
- `crates/stateful-server`: local HTTP API and store-backed policy service.
- `crates/stateful-cli`: CLI, hook adapter, runtime discovery, repo registry,
  outbox sync, native-tool guidance assets, and sandbox wrappers.
- `crates/stateful-bench`: benchmark tooling for paired-agent, synthetic,
  DeNovoSWE, and ProgramBench experiments.
- `docs/`: concept, state model, architecture, implementation contract,
  coordination, hardening-scope, ADR, and benchmark guidance.

## Benchmark Tooling

`stateful-bench` supports SWE-bench pair preparation/runs, reports, comparisons,
synthetic coordination experiments, and DeNovoSWE adapters for official AweAgent,
host Codex CLI, and OMP CLI workflows, and ProgramBench stateful/no-state
condition runs with official ProgramBench evaluation and efficiency reporting.

Benchmark artifacts live under `.stateful_bench/` and are intentionally ignored.
The checked-in synthetic fixture is a smoke test for report plumbing, not
empirical evidence that real paired agents avoid conflicts. This repository does
not currently ship a checked-in empirical paired-agent stateful/no-state result.

For DeNovoSWE and ProgramBench setup, interpretation rules, and reusable command
lines, read [DeNovoSWE Benchmark Guide](docs/denovo-benchmark-guide.md),
[DeNovoSWE Benchmark Commands](docs/denovo-benchmark-commands.md), and
[ProgramBench Benchmark Guide](docs/programbench-benchmark-guide.md).

## Development

Run the Rust test suite:

```bash
cargo test --workspace
```

Run formatting, linting, and tests:

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

See [CONTRIBUTING.md](CONTRIBUTING.md) for contribution expectations and
[Usage reference](docs/usage-reference.md#versioning-and-reclaims) for the local
release workflow notes.

## Documentation

- [Core concept](docs/core-concept.md)
- [Usage reference](docs/usage-reference.md)
- [State model](docs/state-model.md)
- [Architecture](docs/architecture.md)
- [Implementation contract](docs/implementation-contract.md)
- [Current-state coordination rationale/index](docs/current-state-coordination.md)
- [Concurrency control spec](docs/concurrency-control-spec.md)
- [V1 hardening scope decisions](docs/v1-hardening-scope-decisions.md)
- [DeNovoSWE Benchmark Guide](docs/denovo-benchmark-guide.md)
- [DeNovoSWE Benchmark Commands](docs/denovo-benchmark-commands.md)
- [ProgramBench Benchmark Guide](docs/programbench-benchmark-guide.md)
- [ADR 0001: State-first, not memory-first](docs/adr/0001-state-first-not-memory-first.md)

Some historical implementation plans and specs are tracked under
`docs/superpowers/` for traceability. New local scratch plans under that tree are
ignored by default.

## License

`stateful_core` is licensed under the GNU Affero General Public License version
3.0 only. See [LICENSE](LICENSE).

Why AGPL? `stateful_core` is intended to remain open even when it is used behind
local or network services. The license is part of keeping improvements to the
coordination layer available to the people who depend on it.

