# stateful_core

`stateful_core` is a local coordination service for trusted coding-agent sessions
sharing one checkout. It records task and resource observations, serializes
conflicting writes with short exclusive leases, and keeps an inspectable audit
journal.

It is **not** a replacement for Git, a job scheduler, or a security sandbox.
Use Git for history, branches, review, and merge; use an agent runner or
scheduler to choose and start work; use OS and tool sandboxing for security.
Stateful only coordinates activity that uses its protocol on one machine.

## Quick start

Build and install the CLI from this checkout:

```bash
cargo install --path crates/stateful-cli
```

On Linux, install `bubblewrap`. On Ubuntu/AppArmor hosts that restrict
unprivileged user namespaces, grant `/usr/bin/bwrap` the `userns` rule before
using sandboxed commands.

Install the OMP integration and opt the repository in:

```bash
stateful install --agent omp --yes
stateful enable
stateful server start
stateful status
```

`stateful install --agent omp --yes` creates the isolated OMP `stateful`
profile. Start OMP with that profile. The extension derives agent ownership only
from `ctx.sessionManager.getSessionId()` and `ctx.sessionManager.getLeafId()`;
it does not use event, environment, or process identity as a fallback.

Codex native-tool conformance is not verified and therefore fails closed. Its
hook integration supplies the task and agent ids plus the installed binary in
context; only the exact installed-binary adapter wrapper is permitted. After
`UserPromptSubmit`, the hidden Codex heartbeat helper refreshes the root task
at the configured heartbeat interval while its owner record and parent
PID/start identity match, so prompt inactivity alone does not expire a live
task. `Stop` removes that record; the helper exits when that record is removed
or changes, or when the parent exits or its start identity no longer matches.

For a structured commit, name every file explicitly:

```bash
stateful commit --task-id <id> --agent-id <id> -m "Explain lease flow" README.md docs/architecture.md
```

If a lease must be released explicitly:

```bash
stateful lease release <batch-id> --task-id <id> --agent-id <id>
```

See [usage reference](docs/usage-reference.md) for the command and HTTP
contracts.

## What happens on a write

1. A task reads exact, stable resource observations.
2. It prepares a write. Stateful either returns a permit, asks for a reread, or
   queues one nonterminal lease request for the task.
3. The owner of an overlapping physical resource holds the only active exclusive
   lease. A queued writer polls its request; if superseded, it follows
   `superseded_by`. When offered, it takes post-offer fresh exact reads,
   activates the offer, takes fresh exact reads again, and prepares again to
   receive a ready attempt and permit.
4. The permitted operation completes with an explicit terminal result. A lease
   is never released while its write is in flight.

The service is intentionally advisory at the integration boundary: direct file
writes that do not use it cannot be prevented or observed by Stateful.

## Local runtime

`STATEFUL_HOME` selects the global runtime directory. If it is unset, the
location is `$HOME/.stateful_core`. The database is
`$STATEFUL_HOME/state.db`; runtime metadata and the enabled-repository registry
are also under that directory. Do not commit this state.

The server accepts loopback listeners only. `/health` is unauthenticated; every
other endpoint requires the runtime bearer token. There is no remote, LAN, or
cross-machine coordination mode.

## Documentation

- [Architecture](docs/architecture.md) — components, flow, and invariants
- [Usage reference](docs/usage-reference.md) — CLI, HTTP, and hook use
- [State model](docs/state-model.md) — SQLite schema, audit, projection, and migration
- [ADR 0002: Lease-first collision prevention](docs/adr/0002-lease-first-collision-prevention.md)

## License

`stateful_core` is licensed under the GNU Affero General Public License version
3.0 only. See [LICENSE](LICENSE).
