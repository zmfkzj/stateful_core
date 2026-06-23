# Core Concept

## Problem

Coding agents are bounded by their own sessions. A session can inspect its own
conversation and tools, but it cannot reliably see what another session, another
agent, or a human is doing in the same repository right now.

This creates avoidable coordination failures:

- two agents edit the same file without knowing it
- one session repeats investigation another session already started
- an agent writes a file that changed in the shared checkout after its lease was
  acquired
- stale memory is mistaken for current activity
- an interrupted session leaves no structured handoff state

`stateful_core` exists to answer the operational question:

```text
Who is doing what now, what might conflict, and when does that claim expire?
```

## Core Thesis

The system should manage current state for coordination, not long-term memory.

Memory helps recover prior information. It can provide evidence, summaries, or
background context. It cannot by itself say what is actively true now.

```text
memory = past evidence and recall
current state = active, expiring operational truth
```

## When Shared State Is The Right Tool

The default way to avoid multi-actor collisions should be isolation: separate
branches, worktrees, containers, or an orchestrator that partitions work so
actors do not touch the same checkout. Git merge and review remain the right
tools for reconciling committed history.

`stateful_core` is useful in the narrower case where isolation removes too much
shared context:

- the repository's working environment is too heavy to clone per actor, such as
  warm build artifacts, dev servers, local databases, device state, or
  credentials already attached to the canonical checkout
- tightly coupled tasks need to see and build on each other's uncommitted edits
  before a clean commit or merge point exists
- the canonical checkout can change outside an agent turn, and supported agent
  writes need current, expiring coordination signals before they proceed

If none of those conditions hold, use the simpler isolated workflow and let Git,
the orchestrator, or the platform sandbox carry the coordination burden.
Shared-state coordination is the residual niche for work that must stay in one
live workspace and still needs write-time collision checks.

## Current State

Current state is a scoped, time-bound summary of active work.

Examples:

- An agent session is exploring auth validation.
- A subagent is editing under its own session identity while sharing the
  workspace-level coordination model.
- A session intends to edit `src/auth.ts`.
- A file has an active advisory lease.
- A session is testing after a change.
- A file changed after an agent acquired its exact file lease, or a future
  observer/IDE integration reports nearby human editing activity.
- A session finalized as `done`, `failed`, or `blocked`.

Current state must be compact enough to render into an agent prompt and precise
enough to drive conflict checks before important tool calls.

The shipped v1 prototype observes Codex and OMP sessions, supported tool
effects, MCP calls, exact file lease freshness, and explicit reconciliation
acknowledgements.
It does not automatically watch human editor buffers or filesystem saves.

## Freshness

Current state decays quickly. Every active claim needs a freshness model:

- `last_updated_at` records the latest observation.
- `expires_at` defines when the claim stops being current truth.
- `phase` shows whether the actor is exploring, editing, testing, blocked, done,
  failed, idle, or expired.
- heartbeat updates keep active work alive.

Expired state can remain useful as historical evidence, but it should not block
new work as if it were still active.

V1 intent freshness uses a 15-minute default TTL. Heartbeats can extend active
intent while the phase is `exploring`, `editing`, or `testing`, but never beyond
60 minutes from declaration. Blocked or finalized work is visible but does not
authorize writes.

## Coordination Protocol

The first protocol is intentionally small:

```text
1. register session
2. declare intent
3. acquire advisory lease
4. send heartbeat while active
5. observe tool effects
6. update phase and next intent
7. finalize as done, failed, or blocked
8. release or expire lease
```

Start-only reporting creates stale locks. End-only reporting fails to prevent
collisions while work is happening. The protocol needs both.

## Enforcement Boundary

The system should not rely only on prompting agents to update state. Important
actions should pass through a coordination check:

```text
before important action -> check intent and conflicts
after important action  -> observe effects and refresh state
before turn stops       -> require final status
```

For v1, supported write actions are blocked unless the session has active intent
with matching file or directory scope. Abstract task, test, port, or migration
intent can provide context but does not permit writes. Codex lifecycle hooks and
the OMP extension provide the enforcement surface. This is a coordination
guardrail, not a complete sandbox or security boundary.

V1 only authorizes writes through tool paths with reliable target extraction.
Repo file edits use hook-visible native edit tools such as Codex `apply_patch`,
`Edit`, and `Write`, or OMP `edit` and `write`, after exact intent and a
successful same-session file lease; the lease is released after the completed
write transaction. Bash command text alone is never a repo-internal
authorization source. Runtime tool names are classified by their leaf segment,
so `functions.bash` follows Bash rules, `functions.python` follows Python
rules, and `functions.read` / `functions.search` remain native read/search
tools. Codex raw Bash is denied with sandbox guidance. OMP raw Bash and native
Python execution are denied at host approval and hook levels, even when the raw
command itself invokes `stateful sandbox run`; OMP sessions use generated custom
tools instead. `sandbox_bash` invokes the trusted stateful binary for read-only,
write-targets, build, git, and github-pr sandbox profiles, including common
sandbox flags, and rejects `--fs external` with guidance to use
`external_bash`. `external_bash` prompts
before invoking `stateful sandbox run --fs external --purpose ...`. Ordinary
read work should use agent-native read, search, or diff tools when available.
Read-only inspection that genuinely needs a shell must use the trusted absolute
`stateful` wrapper: `<absolute-stateful-binary> sandbox run --fs read-only
--network disabled --command <cmd>`; in OMP, use `sandbox_bash` for that
profile. Command-shaped repo writes must use the wrapper with
`--fs write-targets` and explicit repo-relative target flags after intent and
same-session lease; in OMP, use `sandbox_bash`. Repo-external operations use
`--fs external` with purpose and command; read-only external commands may omit
targets, while supplied write/create/dir/socket/signal scopes must be absolute
external paths and approved. In OMP, use `external_bash`. Raw Bash test commands
are not allowlisted; use
`stateful sandbox run --fs build --network enabled --write-dir <scratch-purpose>
--command <cmd>` so build artifacts go under
`/tmp/stateful/<session>/<scratch-purpose>/`; in OMP, use `sandbox_bash`.

## Product Shape

The product is useful when an agent can answer:

- Who else is active in this workspace?
- Is this activity from a root agent, subagent, human, or system runner?
- Which files or resources are currently leased?
- What does another actor plan to do next?
- Is my intended edit likely to conflict?
- Is the conflicting state fresh, stale, or expired?
- What final status did the previous session leave behind?
