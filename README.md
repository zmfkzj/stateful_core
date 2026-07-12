# stateful_core

Presence-first coordination for coding agents and humans sharing one live repository checkout.

Git coordinates committed history. `stateful_core` coordinates live presence
before it becomes history. Before a supported write occurs, an actor should see:

```text
Who is nearby, what is fresh, what was handed off, and should this write proceed?
```

The project is intentionally about current state, not long-term memory. Memory
recalls what happened before; current state captures active, scoped, expiring
operational truth that can be checked before writes, test execution, handoff, and
other coordination-sensitive actions.

The product center is one live-checkout presence layer: agents and humans see
nearby work, freshness, and handoff before writing. Blocking exists only as a
thin safety rail at data-loss edges. See
[ADR 0002](docs/adr/0002-presence-first-not-lock-first.md) for this direction
and its evidence.

## Status

This repository is an early Rust implementation and local-first, macOS-first
prototype. The current implementation is Codex-first with OMP support. It
includes a CLI, global user-level installation, repo allowlist gating, a local
HTTP state server, native Stateful coordination tools, Codex and OMP hook
adapters, SQLite-backed state store, sandboxed command profiles, outbox sync,
human observation/reconciliation commands, a VS Code advisory save gate, and
benchmark tooling.

These docs describe the shipped local v1 coordination mechanism. Checked-in
benchmark evidence now includes three-arm forced-overlap plumbing/smoke results;
ProgramBench quality scoring remains blocked on this macOS arm64 host and needs
a Linux amd64-compatible eval rerun before quality comparisons.

Human coordination is explicit and advisory: `stateful watch run`,
`human observe`, `human save-check`, `reconcile ack`, and the VS Code save gate
surface conflicts.

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

`stateful_core` provides a small protocol for recording session activity,
reading current-state summaries, rendering scoped write-time coordination
context (`context_render`), declaring intent as reservation, and tracking active
claim freshness. `context_render` is a briefing, not a task scheduler: task
allocation belongs to an orchestrator or human, and integration belongs to Git.
As a safety rail, v1 still queues blocked writers and blocks supported writes
that lack matching active reservation/claim authority or fresh base observations.

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
- Human coordination commands for `human observe`, `human save-check`, and
  `reconcile ack`, plus an advisory VS Code save gate.
- Sandboxed profiles for build/test output, command-shaped repo writes, git
  operations, GitHub PR commands, and repo-external shell work.
- Benchmark tooling for SWE-bench pair runs, reports, comparisons, awareness
  mode, synthetic coordination experiments, forced-overlap harness scripts, and
  DeNovoSWE/ProgramBench adapters.

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

Install OMP integration when you want the isolated OMP `stateful` profile
(or another profile selected with `--profile <name>`), stateful hooks, native Stateful tool injection, built-in Bash preflight, OMP
edit/write predeclare/claim for simple repo files, lazy resume fallbacks, and
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
the active `agent_id` and `workspace_id` for state operations. See the detailed
canonical identity rules in
[Hooks And Identity](docs/usage-reference.md#hooks-and-identity).

Manual CLI use outside an active agent session can declare scope, keep the
returned reservation id, and inspect the current state:

```bash
reservation_id=$(stateful reservation declare --purpose "Update README content requested by the user." README.md | jq -r '.reservation_id')
stateful current
```

Server coordination defaults to enforcement; use
`stateful server start --coordination-mode awareness` for warn-only awareness
mode.

Agents also receive `scope_overlap` notifications when a peer declares overlapping scope, and Codex UserPromptSubmit context re-renders when the relevant reservation/wait/observed-write coordination fingerprint changes instead of only once per session.

Human-side coordination is explicit:

```bash
stateful watch run [--repo <path>]
stateful human observe <path> [--summary <text>]
stateful human save-check <paths...>
stateful reconcile ack --reservation-id <reservation_id> --files-reread <path> --summary <text> --decision adopt|reapply|ask_user|abandon
```

The VS Code extension is an advisory save gate: it warns on server-reported
human save conflicts and lets the human decide.

Inside an active Codex or OMP session, use the active Stateful coordination tools
directly instead of shelling out to `stateful reservation declare`. Simple
repo-internal OMP native `edit`/`write` calls with no explicit `reservation_id`
predeclare the exact tool-visible file scope and acquire same-reservation claims
before the first authorization. The explicit multi-resource write flow remains:

```text
read current state -> declare task reservation with known file set -> keep reservation_id -> acquire exact same-reservation claims for reserved paths -> reread targets -> write with the same reservation_id
```

Reservation and claim are separate on purpose. A reservation groups the task's
known file and directory scopes under one purpose and one `reservation_id`, and
can be expanded when the task discovers another target. Claim acquisition uses
`reservation_id` plus `paths: string[]` so callers can acquire a batch from that
reservation in one request. Each resulting claim still owns one exact file or
directory resource and expires when the agent stops being fresh.

For native OMP `edit` and `write`, this predeclare-first behavior is the default
simple-write path: when no explicit reservation id is supplied and the
tool-visible target is one simple repo file, the extension declares exact file
scope and acquires same-reservation claims before the first authorization.
When another active claim
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
`wait_id`, OMP first checks resume notifications for that saved id, then claims
the queued reservation, re-authorizes, and applies with stale-target guards.
Generated no-wait operation ids still require resolving the missing scope or
claim externally before resume.

Detailed queue states, claim expiry behavior, and promotion rules are documented
in [State model](docs/state-model.md),
[Current-state coordination rationale/index](docs/current-state-coordination.md), and
[Concurrency control spec](docs/concurrency-control-spec.md).

## Command Execution

Use native read/search tools for ordinary inspection. Do not use raw shell tools
inside enabled Codex or OMP sessions when a stateful-native path exists.

| Need | Use |
| --- | --- |
| Simple OMP repo file edit | Native `edit`/`write`; predeclare/claim exact simple file target before first authorization when no explicit reservation id is supplied |
| Other repo file edit | Native edit/write tools after task-level reservation and exact same-reservation file claim |
| Build or test command | `stateful sandbox run --fs build --network enabled --write-dir <scratch-purpose> --command <cmd>` |
| Command-shaped repo write | `stateful sandbox run --fs write-targets --reservation-id <reservation_id> --write-target <file> --command <cmd>` |
| Local git operation | `stateful sandbox run --fs git --network disabled --command 'git <args>'` |
| Remote git operation | Git sandbox profile with `--network enabled` |
| GitHub PR list/view/status/create | `stateful sandbox run --fs github-pr --network enabled --command 'gh pr <list|view|status|create> ...'` |
| Repo-external shell operation | `stateful sandbox run --fs external --purpose <purpose> --command <cmd>` for reads; add exact `--write-target`, `--create-target`, or `--write-dir` scopes for writes |

For external-to-repo file imports, use the external profile with an exact repo-relative `--write-target` or `--create-target`. When no `--reservation-id` is supplied, Stateful retries a missing-reservation/scope denial by auto-declaring and claiming that exact file target. Repo-relative external `--write-dir` remains explicit-reservation only.

Use repeated `--command <cmd>` instead of outer Bash `&&`/`;` when a sandboxed operation needs multiple setup steps. Stateful compiles repeated commands into one sandbox-internal script, so the outer Bash call remains a single trusted `stateful sandbox run ...` command. Add `--command-shell /bin/zsh` only when the script needs a shell other than `/bin/sh`. The `git` and `github-pr` profiles reject repeated `--command` because they validate one direct `git` or `gh pr` command.

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
`edit` and `write` predeclare/claim exact simple file targets before first
authorization when no explicit reservation id is supplied. Use `lazy_edit_resume` for
queued/conflicting line-based OMP `edit` patches and `lazy_write_resume` for
captured full OMP `write` replay; captured wait ids are checked through resume
notifications before claiming and re-authorization, while generated no-wait ids
still require the missing scope or claim to be resolved externally. Use
`lazy_bash_resume` to rerun a queued
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

The state server owns policy, persistence, TTLs, and authorization conflict
evaluation; Codex/OMP hooks observe and gate important agent actions. Native
Stateful tools give agents a structured way to read and update coordination
state. See
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
allowed with `--allow-plain-http`; that opt-in sends the bearer token in cleartext.
See [Usage reference](docs/usage-reference.md#lan-runtime-sharing) for the full flow.

If a previously joined LAN runtime endpoint is stale or unreachable, Stateful
commands fail with a bounded connect error instead of hanging. Restart the host
runtime or rerun `stateful server join ...`, then check `stateful status` and
`stateful doctor` to refresh and verify the local runtime files.

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

`stateful-bench run --mode no-state|awareness|stateful` covers the three
benchmark arms: no coordination, awareness as the warn-only middle arm, and
stateful enforcement. `stateful-bench compare` accepts `--awareness-run-dir`
alongside stateful/no-state runs. The benchmark crate also includes synthetic
coordination experiments, forced-overlap scripts
(`overlap_manifest_generator.py`, `overlap_omp_agent.py`,
`overlap_harness.py`), DeNovoSWE adapters for official AweAgent, host Codex CLI,
and OMP CLI workflows, and ProgramBench condition runs with official
ProgramBench evaluation and efficiency reporting.

The visible `stateful sandbox run-nested-codex-benchmark` command is only for the
feature-gated nested Codex benchmark harness on supported sandboxes; it is not a
general operator sandbox. Use `stateful sandbox run` for normal operator commands.

Benchmark artifacts live under `.stateful_bench/` and are intentionally ignored.
Checked-in benchmark summaries live under [docs/benchmarks](docs/benchmarks/):
the forced-overlap result verifies three-arm runner/compare plumbing without a
differentiated safety outcome, and the ProgramBench note records completed
inference trials plus the official-eval blocker. Do not infer quality wins from
either artifact.

The full task-graph StatefulBench protocol is cancelled. The maintained
[statefulbench-lite](docs/statefulbench-lite.md) is a small launcher smoke, not
real-world evidence: its `generate` command creates an intentionally RED
workspace without launching agents or consuming model credits:

```sh
python3 crates/stateful-bench/scripts/statefulbench_lite.py generate --dest tmp/statefulbench-lite-smoke
```

`statefulbench_lite.py run` does launch live agents and consumes credits; its
results do not establish behavioral quality, safety, or statistical superiority.

For the real-world corpus, use the
[detailed StatefulBench workflow](docs/statefulbench-realworld-design.md).
It contains 100 coding tasks across ten pinned repositories. Qualification is
required before any live run: it verifies the base suite, base-RED/reference-
GREEN task evaluators, integrated reference, upstream suite, and a non-isolated
overlap graph.
The manifest binds each downloaded archive to its canonical GitHub repository,
exact commit, and SHA-256; qualification also ignores host Git configuration
and hooks when checking reference patches.


```sh
python3 crates/stateful-bench/scripts/statefulbench_realworld.py qualify --manifest datasets/statefulbench-realworld/manifest.json --cache tmp/statefulbench-realworld-cache
```

Evaluators and reference patches are withheld from task agents; after those
agents finish, the final reviewer and then the harness run the evaluators and
upstream suite. A full one-trial result runs all three arms over all ten
repositories (330 live agents, so budget model credits accordingly):

```sh
python3 crates/stateful-bench/scripts/statefulbench_realworld.py run --manifest datasets/statefulbench-realworld/manifest.json --cache tmp/statefulbench-realworld-cache --out tmp/statefulbench-realworld/$(date -u +%Y%m%d-%H%M%S)
```

It is a full result only when every arm clears: all task and final agents exit
successfully without timing out, no arm error occurs, and the post-final
evaluators and upstream suite pass. Preserve `summary.json`, each trial's
`results.json`, command/log artifacts, and the provenance manifest.
`results.json` and `summary.json` are published atomically, so automation
never consumes partial JSON records. Interpret only tokens, tool calls, and
wall time as descriptive efficiency metrics.

For DeNovoSWE and ProgramBench setup, interpretation rules, and reusable command
lines, read [DeNovoSWE Benchmark Guide](docs/denovo-benchmark-guide.md),
[DeNovoSWE Benchmark Commands](docs/denovo-benchmark-commands.md), and
[ProgramBench Benchmark Guide](docs/programbench-benchmark-guide.md). Official
ProgramBench setup currently needs Python >=3.10; install from the upstream
`facebookresearch/ProgramBench` source when PyPI does not provide a usable
package for your interpreter.

## Development

Run the Rust test suite:

```bash
cargo test --workspace
```

Run the CI gates from `.github/workflows/rust.yml` before publishing:

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
env -u STATEFUL_CODEX_RUN_ID -u CODEX_THREAD_ID cargo test --workspace
cargo test -p stateful-cli --features codex-benchmark --test hook
python3 -m venv .venv && . .venv/bin/activate && python -m pip install pytest && python -m pytest crates/stateful-bench/scripts/tests
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

