# stateful_core

`stateful_core` is a local-first current-state coordination layer for coding
agents.

It helps agents, sessions, subagents, and nearby human work answer a practical
question before important actions:

```text
Who is doing what now, what might conflict, and when does that claim expire?
```

The project is intentionally about current state, not long-term memory. Memory
recalls what happened before. Current state captures active, scoped, expiring
operational truth that can be checked before writes, validation, handoff, and
other coordination-sensitive actions.

## Status

This repository is an early Rust implementation and local-first prototype. The
current implementation is Codex-first and includes a CLI, local HTTP state
server, MCP adapter, Codex hook adapter, SQLite-backed state store, validation
runner, and benchmark tooling.

APIs, configuration files, and command behavior may change while the project is
pre-release. The current security and support scope is documented in
[SECURITY.md](SECURITY.md).

## Why

Coding agents usually know their own session, but they do not reliably know
what another agent, another session, or a human is doing in the same repository
right now. That creates avoidable failures:

- two agents edit the same file without seeing each other
- an interrupted session leaves no structured handoff state
- stale memory is treated as active truth
- a tool writes before the session has declared its intended scope
- validation or reconciliation happens outside the coordination loop

`stateful_core` provides a small protocol for declaring intent, tracking active
leases, observing tool effects, rendering current state, and blocking supported
writes that have no matching active intent.

## Wait Queue and Resume

Write conflicts are scoped to the file or resource being modified. If session A
holds an active lease on `src/auth.ts`, sessions B and C can still read,
search, validate, or work on other files. Writes to `src/auth.ts` are blocked.

When a blocked write uses the hook or API with `queue_on_conflict: true`,
`stateful_core` records the blocked session in a FIFO wait queue for that
resource. When A explicitly releases the lease, finalizes the activity, or lets
the lease expire, the first waiter receives a short reservation and a pending
notification. While that reservation is active, later sessions cannot take the
same resource ahead of the reserved session.

Reservations are not active write authority by themselves. The reserved session
must claim the reservation before it becomes active intent and an active lease.
The default reservation TTL is 120 seconds.

Resume signals are available through:

- `stateful notifications poll`
- `stateful resume next`
- MCP tools `state.notifications.poll` and `state.resume.next`
- the `/v1/notifications/poll` and `/v1/resume/next` HTTP endpoints

## What It Provides

- A `stateful` CLI for initialization, status, current-state inspection,
  intent declaration, validation, MCP, hooks, and structured commits.
- A local HTTP state server with token-protected non-health endpoints.
- A SQLite event store and materialized current-state view.
- Codex lifecycle hook integration for observing and gating important actions.
- An MCP adapter exposing the current-state protocol to compatible tools.
- Repo-local validation profiles for controlled test and check execution.
- Benchmark tooling for coordination experiments.

## What It Is Not

`stateful_core` is not a sandbox, access-control system, distributed lock
service, durable secret store, or long-term memory product. It is designed to
coordinate trusted local tools in one workspace. See the
[security model](SECURITY.md#local-trust-model) for details.

## Install From Source

Prerequisites:

- Rust 1.85 or newer
- Git

Build the workspace:

```bash
cargo build --workspace
```

Optionally install the CLI into your Cargo bin directory:

```bash
cargo install --path crates/stateful-cli
```

If you do not install it, use `target/debug/stateful` in the commands below.

## Quick Start

Initialize repo-local Codex hook and `stateful` configuration:

```bash
target/debug/stateful init --binary target/debug/stateful
```

Start the local state server in one terminal:

```bash
target/debug/stateful server
```

In another terminal, declare the files you intend to edit:

```bash
target/debug/stateful intent declare --session-id demo --workspace-id local README.md
```

Inspect the current coordination state:

```bash
target/debug/stateful current
```

Run a configured validation profile:

```bash
target/debug/stateful validate cargo-test
```

Check local installation health:

```bash
target/debug/stateful doctor
```

## CLI Overview

- `stateful init` writes repo-local `.codex/` and `.stateful/`
  configuration.
- `stateful server` runs the local HTTP state server.
- `stateful status` and `stateful doctor` report local setup health.
- `stateful current` renders active current state.
- `stateful events` prints recent stored state events.
- `stateful intent declare <paths...>` declares planned file scope.
- `stateful notifications poll` reads pending coordination notifications.
- `stateful resume next` reads the next reservation available to the active
  session.
- `stateful validate <profile>` runs a configured validation profile.
- `stateful commit -m <message> -- <paths...>` creates a structured commit.
- `stateful enable`, `stateful disable`, and `stateful repos list` manage
  registered repositories for global integration.
- `stateful mcp serve` exposes the MCP adapter over stdio.
- `stateful hook <event>` runs Codex hook integration entry points.

Run `stateful <command> --help` for command-specific options.

## Core Loop

```text
observe session or tool activity
-> register session and declare intent
-> acquire or refresh advisory lease
-> check conflicts against active current state
-> queue blocked conflicting writes by resource
-> block write actions without active intent
-> authorize, warn, or block remaining important actions
-> record tool effects and heartbeat
-> finalize as done, failed, or blocked
-> release or expire leases
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

Commit reusable documentation and source code, not local generated state.

## Project Layout

- `crates/stateful-core`: domain types, resource scope matching, current-state
  rendering, reconciliation, Bash classification, and policy engine.
- `crates/stateful-store`: SQLite event store and current-state persistence.
- `crates/stateful-server`: local HTTP API over the shared policy and store.
- `crates/stateful-cli`: user-facing CLI, hook adapter, runtime discovery,
  repo registry, structured commit, outbox sync, and validation commands.
- `crates/stateful-mcp`: MCP tool surface.
- `crates/stateful-validation`: validation profile parser and runner.
- `crates/stateful-bench`: benchmark tooling and experiment harnesses.
- `docs/`: architecture, state model, protocol, concept, and ADR documents.

## Development

Run the Rust test suite:

```bash
cargo test --workspace
```

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
- [ADR 0001: State-first, not memory-first](docs/adr/0001-state-first-not-memory-first.md)

Internal implementation plans and scratch specs are intentionally kept outside
the public repository.

## License

`stateful_core` is licensed under the GNU Affero General Public License version
3.0 only. See [LICENSE](LICENSE).
