# stateful_core

Presence-first coordination for coding agents and humans sharing one live repository checkout.

Git coordinates committed history. `stateful_core` coordinates the live work that
precedes it: who is present, what is fresh, what was handed off, and what context
the next actor needs before changing a shared checkout.

## Status

This is a local-first, macOS-first Rust implementation with a CLI, local HTTP
server, SQLite-backed state, native Stateful tools, Codex and OMP integration,
human observation/reconciliation, and benchmark tooling. The shipped protocol is
`stateful.v2` with a schema-2 event journal.

The runtime starts in **awareness**: reservations and claims describe advisory
intent, and actors receive current-state context and warnings. Select
**enforcement** explicitly when supported writes must be authorized:

```bash
stateful server start --coordination-mode enforcement
```

The product center is presence, freshness, and handoff—not locking. In both
modes, the deliberately thin hard stops reject invalid targets, stale exact-read
evidence, unreconciled high-confidence human writes, an active write fence, and
a prior write whose outcome is unknown. Explicit enforcement additionally can deny
overlapping supported writes; awareness keeps reservations and claims advisory.

## Why

Separate agents, sessions, and humans can all change the same checkout before
Git sees a commit. Without live coordination, they can miss nearby work, treat
stale observations as current, or leave the next actor without a useful handoff.

`stateful_core` records scoped presence and activity, renders current context,
tracks exact-read freshness, and carries explicit handoffs. It is not a task
scheduler, filesystem lock manager, access-control system, durable secret store,
or long-term memory product. Git remains responsible for integration.

Use it for a shared workspace whose useful question is “should this write happen
here now?” Prefer branches, worktrees, containers, or task-level orchestration
when isolation is practical.

## Install from source

Prerequisites:

- Rust 1.85 or newer
- Git
- Codex CLI for Codex hooks or `stateful codex`
- OMP for the OMP `stateful` profile
- A supported sandbox backend for `stateful sandbox run`; macOS Seatbelt is the
  verified backend, while Linux bubblewrap support is experimental

```bash
cargo build --workspace
cargo install --path crates/stateful-cli
```

Without installation, use `target/debug/stateful` below. The benchmark binary is
`target/debug/stateful-bench`.

## Quick start

Install user-level files; omit `--yes` to inspect the dry-run plan:

```bash
stateful install --yes
```

Install the integration you use, then enable the repository:

```bash
# Codex
stateful install --agent codex --yes
stateful enable
stateful codex

# OMP
stateful install --agent omp --yes
stateful enable
```

Check the runtime and inspect shared state before making source changes:

```bash
stateful status
stateful doctor
stateful current
```

`stateful doctor` reports journal footprint and growth and warns when the
configured footprint threshold is reached. The canonical event journal is
indefinite; events are not deleted solely because of their age.

## Day-to-day coordination

The enabled Codex and OMP integrations register sessions, maintain presence, and
render context for the active actor. Rendered context has a delivery ID,
sequence, and workspace version; the client acknowledges the delivered context,
so an unacknowledged delivery is retried for that agent on a later turn or
resumed session.

Before a supported write, inspect current context, declare the known scope, then
complete an exact read of each target. Freshness is evidence from that completed
read, not an assertion made when intent is declared. In an active agent session,
prefer the native Stateful tools; outside one, the CLI can declare scope:

```bash
reservation_id=$(stateful reservation declare --agent-id <agent-id> --purpose "Update README content requested by the user." README.md | jq -r '.reservation_id')
stateful current
```

Humans participate explicitly and remain in control:

```bash
stateful human observe <path> [--summary <text>]
stateful human save-check <paths...>
stateful reconcile ack --reservation-id <reservation_id> --resource <path> --files-reread <path> --summary <text> --decision adopt|reapply|ask_user|abandon
```

The VS Code save gate is advisory. A completed handoff records the work, tests,
and remaining work explicitly; when a session ends without one, the rendered
context carries a fallback handoff rather than treating cleanup as a handoff.

See the [usage reference](docs/usage-reference.md) for command, hook, sandbox,
LAN-sharing, and recovery details.

## Journal migration and recovery

Opening a legacy persistent database performs a guarded migration. The runtime
preflights the source, creates a sibling SQLite backup such as
`<database>.v1.backup.sqlite`, migrates into the schema-2 journal and shadow
projections, validates replay, then commits the cutover. A failed validation does
not silently replace the source.

Durable requests use receipts and outbox replay where the runtime cannot confirm
a completion. An uncertain write remains an outcome-unknown condition until it
is reconciled; do not assume it completed merely because a client disconnected.

## Architecture at a glance

```text
session or tool activity
-> presence and scoped intent
-> rendered current context and exact-read freshness
-> supported write authorization (warn in awareness; enforce only when selected)
-> durable journal receipt, handoff, and next-actor delivery
```

The state server owns policy, persistence, and conflict checks. Hooks and native
tools supply the actor lifecycle and delivery acknowledgements. The canonical
contracts are the [core concept](docs/core-concept.md),
[state model](docs/state-model.md), [architecture](docs/architecture.md),
[implementation contract](docs/implementation-contract.md), and
[current-state coordination guide](docs/current-state-coordination.md).

## LAN runtime sharing

Use LAN mode when one Mac hosts the runtime for another. Connect the remote
machine through an SSH tunnel so the bearer token is not sent to a non-loopback
plain-HTTP address:

```bash
# host Mac
stateful server start --host 0.0.0.0 --workspace-id shared

# remote Mac, after creating the SSH tunnel printed by the host
stateful server join http://127.0.0.1:43873 --token <token> --enable-repo
```

`stateful server join` rejects non-loopback plain-HTTP URLs unless explicitly
allowed. See the [usage reference](docs/usage-reference.md#server-lifecycle)
for the complete flow.

## Benchmark tooling

Benchmark runs distinguish sequential, parallel-off, and explicit parallel-on
arms. `parallel-on` is never implicit: a required three-arm gate must name all
three arms, for example:

```text
--arms sequential,parallel-off,parallel-on
```

A credit-free smoke or a single trial validates plumbing only. It does not prove
causal, statistical, quality, or safety superiority. See the benchmark guidance
in the [usage reference](docs/usage-reference.md).

## Generated local files

Local runtime and integration state can contain absolute paths, local
configuration, benchmark artifacts, or bearer tokens. These generated paths are
ignored by default:

- `.codex/`
- `.stateful/`
- `.stateful_core/`
- `.stateful_bench/`

Global installation writes under `$STATEFUL_HOME`, or `$HOME/.stateful_core` when
`STATEFUL_HOME` is unset. Commit reusable source and documentation, not generated
state.

## Development

```bash
cargo test --workspace
```

See [CONTRIBUTING.md](CONTRIBUTING.md) for contribution expectations.

## Documentation

- [Core concept](docs/core-concept.md)
- [Usage reference](docs/usage-reference.md)
- [State model](docs/state-model.md)
- [Architecture](docs/architecture.md)
- [Implementation contract](docs/implementation-contract.md)
- [Current-state coordination guide](docs/current-state-coordination.md)
- [ADR 0001: State-first, not memory-first](docs/adr/0001-state-first-not-memory-first.md)
- [ADR 0002: Presence-first, not lock-first](docs/adr/0002-presence-first-not-lock-first.md)
- [Historical V1 hardening scope decisions](docs/v1-hardening-scope-decisions.md)

Historical implementation plans and specs under `.superpowers/` are retained for
traceability; they are not current runtime instructions.

## License

`stateful_core` is licensed under the GNU Affero General Public License version
3.0 only. See [LICENSE](LICENSE).
