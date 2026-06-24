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
HTTP state server, MCP adapter, Codex and OMP hook adapters, SQLite-backed state
store, sandboxed command profiles, outbox sync, and benchmark tooling.

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

When those conditions do not apply, prefer isolation first: separate branches,
worktrees, containers, or task-level orchestration usually give a simpler failure
model. `stateful_core` is for the remaining shared-workspace moments where the
useful question is not "how do we merge later?" but "should this write happen
here right now?"

## What It Provides

- A `stateful` CLI for installation, repo enablement, status/current-state
  inspection, reservation declaration, MCP, hooks, sandboxed command profiles, outbox
  sync, and server lifecycle management.
- A local HTTP state server with token-protected non-health endpoints.
- A SQLite event store and materialized current-state summary.
- Codex and OMP lifecycle hook integration for observing and gating important
  actions.
- An MCP adapter exposing the current-state protocol to compatible tools.
- Sandboxed profiles for build/test output, command-shaped repo writes, git
  operations, GitHub PR commands, and repo-external shell work.
- Benchmark tooling for SWE-bench pair runs, reports, comparisons, synthetic
  coordination experiments, and DeNovoSWE adapters.

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

Install Codex integration when you want global Codex hooks, MCP, and the
stateful command-policy skill:

```bash
stateful install --agent codex --yes
stateful enable
stateful codex
```

### OMP

Install OMP integration when you want the isolated OMP `stateful` profile,
stateful hooks, MCP, and generated sandbox command tools:

```bash
stateful install --agent omp --yes
stateful enable
```

Check local setup health:

```bash
stateful status
stateful doctor
```

## Day-To-Day Coordination

In normal `stateful codex` or OMP `stateful` profile use, lifecycle hooks and MCP
bind the active session. Hook messages tell the agent when an explicit
coordination step is needed.

Manual CLI use outside an active agent session can declare scope and inspect the
current state:

```bash
stateful reservation declare --purpose "Update README content requested by the user." README.md
stateful current
```

Inside an active Codex or OMP session, use the Stateful MCP tools directly
instead of routing `stateful reservation declare` or `stateful mcp call` through a
shell. The usual write flow is:

```text
read current state -> declare task reservation with known file set -> acquire exact claims for reserved paths -> reread targets -> write
```

Reservation and claim are separate on purpose. A reservation groups the task's
known file and directory scopes under one purpose, and can be expanded when the
task discovers another target. MCP claim acquisition uses `paths: string[]` so
callers can acquire a batch from that reservation in one request. Each resulting
claim still owns one exact file or directory resource and expires when the
session stops being fresh.

When another active claim blocks a write, the writer can queue for that resource.
When the resource is released or expires, the server reserves it for the next
eligible waiter and sends a resume notification. The reserved session rereads the
target, claims or lazy-claims the reservation, then retries the write.

Detailed queue states, claim expiry behavior, and promotion rules are documented
in [State model](docs/state-model.md),
[Current-state coordination](docs/current-state-coordination.md), and
[Concurrency control spec](docs/concurrency-control-spec.md).

## Command Execution

Use native read/search tools for ordinary inspection. Do not use raw shell tools
inside enabled Codex or OMP sessions when a stateful-native path exists.

| Need | Use |
| --- | --- |
| Repo file edit | Native edit/write tools after task-level reservation and exact same-session file claim |
| Build or test command | `stateful sandbox run --fs build --network enabled --write-dir <scratch-purpose> --command <cmd>` |
| Command-shaped repo write | `stateful sandbox run --fs write-targets --write-target <file> --command <cmd>` |
| Local git operation | `stateful sandbox run --fs git --network disabled --command 'git <args>'` |
| Remote git operation | Git sandbox profile with `--network enabled` |
| GitHub PR list/view/status/create | `stateful sandbox run --fs github-pr --network enabled --command 'gh pr <list|view|status|create> ...'` |
| Repo-external shell operation | `stateful sandbox run --fs external --purpose <purpose> --command <cmd>` |

In OMP, use the generated tools instead of raw Bash: `sandbox_bash` for
non-external sandbox profiles, `ext_ro_bash` for read-only external shell work,
and `ext_rw_bash` for external writes with declared scope.

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
hooks observe and gate important agent actions. MCP tools give agents a
structured way to read and update coordination state. See
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
- `crates/stateful-store`: SQLite event store and current-state persistence.
- `crates/stateful-server`: local HTTP API and store-backed policy service.
- `crates/stateful-cli`: CLI, hook adapter, runtime discovery, repo registry,
  outbox sync, and sandbox wrappers.
- `crates/stateful-mcp`: MCP tool surface.
- `crates/stateful-bench`: benchmark tooling for paired-agent, synthetic, and
  DeNovoSWE experiments.
- `docs/`: concept, state model, architecture, implementation contract,
  coordination, hardening-scope, ADR, and benchmark guidance.

## Benchmark Tooling

`stateful-bench` supports SWE-bench pair preparation/runs, reports, comparisons,
synthetic coordination experiments, and DeNovoSWE adapters for official AweAgent,
host Codex CLI, and OMP CLI workflows.

Benchmark artifacts live under `.stateful_bench/` and are intentionally ignored.
The checked-in synthetic fixture is a smoke test for report plumbing, not
empirical evidence that real paired agents avoid conflicts. This repository does
not currently ship a checked-in empirical paired-agent stateful/no-state result.

For DeNovoSWE setup, interpretation rules, and reusable command lines, read
[DeNovoSWE Benchmark Guide](docs/denovo-benchmark-guide.md) and
[DeNovoSWE Benchmark Commands](docs/denovo-benchmark-commands.md).

## Development

Run the Rust test suite:

```bash
cargo test --workspace
```

Run formatting and tests:

```bash
cargo fmt --all --check
env -u STATEFUL_CODEX_RUN_ID -u CODEX_THREAD_ID cargo test --workspace
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
- [Current-state coordination](docs/current-state-coordination.md)
- [Concurrency control spec](docs/concurrency-control-spec.md)
- [V1 hardening scope decisions](docs/v1-hardening-scope-decisions.md)
- [DeNovoSWE Benchmark Guide](docs/denovo-benchmark-guide.md)
- [DeNovoSWE Benchmark Commands](docs/denovo-benchmark-commands.md)
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
