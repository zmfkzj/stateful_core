# Core Concept

## Problem

Coding agents are bounded by their own sessions. A session can inspect its own
conversation and tools, but it cannot reliably see what another session, another
agent, or a human is doing in the same repository right now.

This creates avoidable coordination failures:

- two agents edit the same file without knowing it
- one session repeats investigation another session already started
- an agent writes a file that changed in the shared checkout after its claim was
  acquired
- stale memory is mistaken for current activity
- an interrupted session leaves no structured handoff state

These are information and freshness failures first: unknown neighbor activity,
stale local belief, and missing handoff state. Access control enters only as a
thin safety rail where information alone cannot prevent data loss.

`stateful_core` exists to answer the write-time operational question:

```text
Who is present nearby, what is fresh, what was handed off, and should this write proceed?
```

## Core Thesis

The system should manage current state for coordination, not long-term memory.

Memory helps recover prior information. It can provide evidence, summaries, or
background context. It cannot by itself say what is actively true now.

```text
memory = past evidence and recall
current state = active, expiring operational truth
```

The primary nouns are presence, freshness, and handoff for one live checkout:
who is touching what, whether that belief is still fresh, and what state a
stopped session left behind. Reservations and claims exist to feed that picture
and to protect thin safety edges. Blocking is a safety rail, not the product
([ADR 0002](adr/0002-presence-first-not-lock-first.md)).

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

- An agent is exploring auth validation.
- An OMP subagent branch is editing under the `agent_id` derived for that branch
  while sharing the workspace-level coordination model.
- An agent plans to edit `src/auth.ts`.
- A file has an active advisory claim.
- An agent is testing after a change.
- A file changed after an agent acquired its exact file claim, or an IDE/human
  observer reports nearby human activity.
- A session finalized as `done`, `failed`, or `blocked`.

Current state must be compact enough to render into an agent prompt or
`context_render` briefing and precise enough to drive conflict checks before
important tool calls.

The shipped v1 prototype observes Codex and OMP sessions, supported tool
effects, native Stateful tool calls, exact file claim freshness, explicit
reconciliation acknowledgements, low-confidence VS Code open/presence and dirty
signals, and high-confidence save observations. Selected-file telemetry and
broader arbitrary filesystem/editor watching remain future work.

## Freshness

Current state decays quickly. Every active claim needs a freshness model:

- `last_updated_at` records the latest observation.
- `expires_at` defines when the claim stops being current truth.
- `phase` shows whether the actor is `exploring`, `editing`, `testing`,
  `blocked`, `done`, or `failed`.
  `idle` and `expired` are context/status labels, not activity phases.
- heartbeat updates keep active work alive.

Expired state can remain useful as historical evidence, but it should not block
new work as if it were still active.

Reservation, claim, heartbeat, and retention defaults are canonicalized in
[Current-State Coordination](current-state-coordination.md#canonical-freshness-defaults). Blocked
or finalized work is visible but does not authorize writes, so long-running
test/build work must keep heartbeating and reacquire authority before writing
after its reservation window.

## Coordination Protocol

The first protocol is intentionally small:

```text
1. register session
2. declare reservation
3. acquire advisory claim
4. send heartbeat while active
5. observe tool effects
6. update phase and next reservation
7. finalize as done, failed, or blocked
8. release or expire claim
```

Start-only reporting creates stale blocking state. End-only reporting fails to prevent
collisions while work is happening. The protocol needs both.

## Enforcement Boundary

The system should not rely only on prompting agents to update state. Important
actions should pass through a coordination check:

```text
before important action -> check reservation and conflicts
after important action  -> observe effects and refresh state
before turn stops       -> require final status
```

V1 ships a broad guardrail: the recorded direction narrows hard denial toward
thin safety edges - stale-base edits, simultaneous same-file writes,
multi-file operation leases, git index serialization, destructive operations,
unsafe raw commands - while presence, freshness, and handoff carry the
coordination value ([ADR 0002](adr/0002-presence-first-not-lock-first.md)).

For v1, supported write actions are blocked unless the active agent has an active
task reservation whose file or directory set covers the target, plus a fresh
same-reservation claim on the exact resource being written. Abstract task, test,
port, or migration resources can provide context but do not permit writes by
themselves. Codex lifecycle hooks and the OMP extension provide the enforcement
surface. This is a coordination guardrail, not a complete sandbox or security
boundary.

V1 only authorizes writes through tool paths with reliable target extraction.
Repo file edits use hook-visible native edit tools such as Codex `apply_patch`,
`Edit`, and `Write`, or OMP `edit` and `write`. OMP `edit` and `write`
predeclare/claim the exact tool-visible file scope before first authorization
for the default simple-write path when no explicit reservation id is supplied;
other native edit paths require task-level reservation and a
successful same-reservation file claim. The claim is released after the completed
write transaction. Bash command text alone is never a repo-internal
authorization source. Runtime tool names are classified by their leaf segment,
so `functions.bash` follows Bash rules, `functions.python` follows Python
rules, and `functions.read` / `functions.search` remain native read/search
tools. Codex raw Bash is denied with sandbox guidance. OMP built-in Bash may run
only strict trusted `stateful sandbox run ...` and
`stateful sandbox process find ...` commands after Stateful preflight; arbitrary
raw Bash and native Python execution are denied at host approval and hook
levels. Command execution and process inspection are not generated tool calls.
External write/create/write-dir/socket/signal scope and repo-external OMP native
`edit`/`write` file targets auto-approve the scoped Stateful-owned OMP grant
prompt by default through `stateful.autoApprove: true`. Auto-approval skips only
the Stateful-owned UI prompt; sandbox scope validation, hooks, reservation/claim
checks, and grant limits still apply. Set `stateful.autoApprove: false` to
require the prompt. When prompted, the prompt shows purpose, declared scope,
examples, max uses, and expiry rather than raw command text, and matching calls
reuse the grant until it expires or reaches its use limit. The
generated extension subscribes to replayable Stateful SSE reservation notifications:
each event uses the per-agent/workspace notification sequence as the SSE `id`
and JSON `sequence`, and reconnecting with `Last-Event-ID` / `last-event-id`
acknowledges delivered events before later pending notifications are replayed. The extension injects a next-turn OMP
message when a queued `wait_id` becomes a claimable reservation (API state
`reserved`); the claim and write still use normal Stateful tools.
Ordinary read work should use agent-native read, search, or diff tools when
available.
Read-only inspection that genuinely needs a shell must use a single trusted
`stateful sandbox run --fs read-only --network disabled --command <cmd>` command;
in OMP, run that command through built-in Bash after Stateful preflight. Process
inspection uses `stateful sandbox process find <selector>`; in OMP, run that
command through built-in Bash, not a generated process tool. Command-shaped repo
writes must use `stateful sandbox run --fs write-targets` with explicit
repo-relative target flags after reservation and same-reservation claim; in OMP, use
built-in Bash with the same trusted command. Repo-external command-shaped
operations use `stateful sandbox run --fs external` with purpose and command;
external sandbox writes must declare write/create/dir scope. Repo-external OMP
native `edit`/`write` file targets and external sandbox writes auto-approve the
scoped OMP UI grant by default; set `stateful.autoApprove: false` to require it.
Raw Bash test commands are not allowlisted; use
`stateful sandbox run --fs build --network enabled --write-dir
<scratch-purpose> --command <cmd>` so build artifacts go under
`/tmp/stateful/<session>/<scratch-purpose>/`.

## Product Shape

The product is useful when an agent can answer:

- Who else is active in this workspace?
- Is this activity from a root agent, subagent, human, or system runner?
- Which files or resources are currently claimed?
- What does another actor plan to do next?
- Is my planned edit likely to conflict?
- Is the conflicting state fresh, stale, or expired?
- What final status did the previous session leave behind?

Rendered coordination context (`context_render`) is a scoped write-time
briefing, not a task scheduler: it shows what nearby work is active, what
changed since the actor last looked, and whether to reread, wait, or proceed.
Task allocation belongs to the orchestrator or human; merge-time integration
belongs to Git.
