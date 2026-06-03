# Current-State Coordination

This document summarizes the initial product direction for observing and
coordinating the present activity of agents, sessions, and humans working in the
same codebase.

## Problem

Coding agents are usually bounded by their own session. A session can know its
conversation, tools, and local context, but it cannot reliably see:

- what another session is doing now
- what another agent plans to modify next
- whether a human is already editing the same area
- whether a stale memory reflects current work or old work

Memory helps recover prior information. It does not provide a live view of
neighboring activity. The missing capability is observation of the present.

## Thesis

`stateful_core` should model "current state" as a first-class coordination
surface.

Current state is not raw logs and not long-term memory. It is a scoped,
time-bound summary of what an actor is doing now, what it intends to do next,
and which files, tasks, or resources are likely to conflict.

```text
memory = past evidence and recall
current state = active, expiring operational truth
```

The goal is to let an agent reason like this:

```text
Another agent is editing auth validation and plans to run auth tests.
I should avoid auth.ts, work on related tests, or wait for the lease to expire.
```

## Expected Value

The primary value is safer concurrency.

Current-state coordination can reduce:

- overlapping edits to the same file
- duplicated investigation
- accidental rollback of another actor's changes
- stale assumptions after a session is interrupted
- hidden contention between agents and humans

It also improves handoff quality because a stopped or blocked session leaves a
structured status rather than only an implicit transcript.

## State Shape

An agent activity state should be compact, fresh, and directly useful for
coordination.

```text
session_id
actor_id
actor_type: agent | subagent | human | system
owner_id
parent_session_id
parent_actor_id
workspace
branch
goal
phase: exploring | editing | testing | blocked | done | failed | idle
files_read
files_editing
files_planned
resource_leases
next_intent
summary
last_result
last_updated_at
expires_at
confidence
source_refs
```

The important fields are not only current edits. `next_intent` is often more
valuable than `files_editing`, because it allows other actors to avoid future
conflicts before they happen.

## Freshness Model

Current state decays quickly. Every active state must therefore include:

- `last_updated_at`
- `expires_at`
- `phase`
- `confidence`
- a source or trigger reference

When `expires_at` passes, the state is no longer treated as authoritative
current truth. It may still be retained as historical evidence, but conflict
checks should label it as stale or expired.

Heartbeat updates keep active work fresh. Missing heartbeats should not be
treated as success. They should move the state toward `unknown`, `idle`, or
`expired` depending on policy.

The default intent TTL is 15 minutes. Heartbeats can extend an active intent,
but not beyond a 60-minute rolling maximum from `declared_at`.

## Protocol

The coordination protocol should be explicit:

```text
1. register session
2. declare intent
3. acquire advisory lease
4. send heartbeat while active
5. update phase as work changes
6. observe tool effects
7. finalize as done, failed, or blocked
8. release or expire lease
9. reserve released resources for queued waiters
10. notify reserved sessions so they can resume
```

Both start and end matter. Start-only reporting creates stale locks. End-only
reporting fails to prevent conflicts while work is happening.

## Wait Queue and Resume

Conflict handling is resource-scoped. If one session has an active lease for a
file, other sessions are not globally blocked. They can continue reading,
searching, validating, or editing unrelated files. Only writes that target the
leased resource are denied.

When a denied write asks to queue on conflict, the state server records a
waiter for the normalized workspace resource:

```text
workspace_id + relative_path + action
```

The wait queue is FIFO. A queued hard-conflict request is promoted only when all
requested resources are available. V1 grant is atomic all-or-nothing: if one
file or directory in a multi-resource request is still blocked, the whole
request stays queued. For a multi-resource request, "available" means the
request is at the head of every resource queue it participates in and every
requested resource has no active lease.

The server promotes the first eligible queued waiter when one of these grant
triggers makes the requested resources available:

- explicit lease release
- session or activity finalization
- lease expiry

The promoted waiter receives a short reservation. Reservations prevent a later
session from taking the resource ahead of the first waiter, but they are not
active write authority. The reserved session claims the reservation before it
becomes active intent and active leases. The default reservation TTL is 120
seconds. If the reservation expires without being claimed, the server may promote
the next eligible FIFO waiter.

Resume is notification-driven rather than process-driven. The state server
records a pending notification when it grants a reservation. Agents and
orchestrators discover that signal by polling notifications, asking for the
next resumable reservation, or receiving that context from a lifecycle hook.
The state server does not wake a sleeping Codex process by itself; external
orchestration can build on the notification and resume APIs.

Full scheduling should work through immediate request/response plus polling.
Future intent request APIs should return `granted`, `queued`, `reserved`,
`canceled`, or `expired` state without blocking indefinitely. The prototype
exposes notifications and `resume next`; a future CLI may provide
`intent wait --timeout <seconds>` as a convenience wrapper around polling.

`request_id` is required for idempotency. Repeating the same request id returns
the existing request state and must not create duplicate queue entries. A queued
or reserved request can be canceled explicitly. Session or activity finalization
cancels that session's queued and reserved requests.

## Enforcement Direction

The system should not rely only on prompt instructions such as "remember to
update state." That is too weak. Sessions can be interrupted, tools can fail,
context can be compacted, and models can forget.

Instead, important actions should pass through a state protocol:

```text
before important action -> authorize against current state
after important action  -> observe and update current state
before session stops    -> require final status
```

For the first implementation, supported write actions are blocked unless the
session has active intent. Conflict enforcement is advisory but blocking where
the Codex runtime exposes a hookable action. Hard global locks are a later
policy decision.

## Codex CLI MVP

The chosen MVP direction is:

```text
Codex lifecycle hooks
+ MCP tools
+ state server
```

This gives useful control without forking Codex immediately.

The prototype supports user-level installation with repo allowlist gating.
`stateful install --yes` configures global Codex hooks and MCP. `stateful enable`
opts the current repo into enforcement. Repo-local hooks remain available through
`stateful enable --repo-local-codex` as a compatibility fallback.

```text
global Codex hooks and MCP config
repo allowlist entry
optional <repo>/.codex/hooks.json compatibility fallback
<repo>/.stateful/validation.yml
stateful binary available by absolute path or PATH lookup
```

The hook event model stays the same across global hooks, repo-local
compatibility hooks, and managed hooks; only the configuration and script
location change. Plugin packaging is deferred to team beta for distribution and
update UX, while managed hooks remain the long-term organization-enforcement
path.

Hook scripts are thin adapters. They parse Codex hook input, extract action and
targets, call the local HTTP state server, and translate the policy decision
back into Codex hook output. Policy stays in the state server. V1 implementation
is Rust-only, so hooks invoke the compiled `stateful` binary.

Every hook request should include a `protocol_version`. A major protocol
mismatch fails closed for write, validation, and reconciliation paths.

### Hook Responsibilities

`SessionStart`:

- register the session
- record workspace, branch, and session id
- inject relevant active state into the model context

`UserPromptSubmit`:

- extract or request an initial intent when possible
- attach nearby active states as context

`PreToolUse`:

- intercept supported tool calls before execution
- deny supported write calls when the session has no active intent
- deny Bash write commands by default when write targets cannot be safely
  authorized
- check whether requested files or resources conflict with active leases
- deny, warn, or add context based on policy

`PostToolUse`:

- observe supported tool results
- update files touched, phase, test results, and last result
- refresh heartbeat and lease timestamps

`Stop`:

- check whether the turn has a final state
- continue the turn if the agent has not reported `done`, `failed`, or
  `blocked`
- release or shorten leases when work is complete

Subagents:

- may write only inside the parent session's active valid intent scope
- record activity and leases with their own `actor_id`
- do not receive automatic override authority from a shared owner

### MCP Responsibilities

The MCP surface should expose structured tools for the agent and hooks:

```text
state.session.register
state.session.heartbeat
state.intent.declare
state.lease.acquire
state.lease.release
state.activity.observe
state.activity.finalize
state.conflicts.check
state.current.read
state.events.read
state.context.render
state.reconcile.ack
state.validation.run
state.notifications.poll
state.resume.next
```

Future scheduling tools should add `state.intent.request`, `state.intent.claim`,
and `state.intent.cancel` when the corresponding server endpoints are
implemented.

Hooks should call the same state server API as MCP tools so policy remains
centralized.

### State Server Responsibilities

The state server is the source of truth for active coordination state.

It should:

- store append-only state events
- materialize active current state
- evaluate conflict rules
- manage lease TTLs
- expose current-state views for prompt rendering
- retain historical activity as evidence after expiration

The Codex integration should stay thin. Policy belongs in the state server, not
inside hook scripts.

V1 persistence is SQLite with append-only event tables and materialized
current-state views. JSONL may be used for debugging exports, but not as the
primary store.

V1 transport is local HTTP. Codex hooks, MCP tools, filesystem watchers, and the
future IDE extension should call the same local HTTP API. MCP remains an adapter,
not a separate policy implementation.

Policy evaluation should use one entry point:

```text
authorize_action(input) -> decision
```

The decision includes `allow`, `warn`, `deny`, or `error`, plus reason,
conflicts, required next action, rendered context items, and an audit event.

### Tool Classification

V1 write enforcement is limited to tool paths where targets can be determined
reliably:

```text
apply_patch -> enforce from patch file headers
MCP filesystem write/edit/delete/rename -> enforce from structured arguments
Bash read/search -> allow when no mutation is detected
test execution -> run through controlled validation action
Bash write or ambiguous mutation -> deny by default
```

Bash write denial should tell the agent to use `apply_patch` or a structured
MCP filesystem write tool after declaring file or directory intent.

The Bash classifier is allowlist-based. Initial read/search commands include:

```text
pwd
ls
find
rg
cat
sed -n
head
tail
wc
git status
git diff
git show
git log
git branch
git rev-parse
```

Anything outside the allowlist that appears to mutate files, start long-running
processes, run unrecognized tests, install packages, redirect output, pipe into
mutation commands, or produce ambiguous side effects is denied by default.
Common read-only test commands such as `cargo test`, `npm test`, `pnpm test`,
`yarn test`, `pytest`, and `go test` are allowlisted by the prototype Bash
classifier. Arbitrary or project-specific test commands should run through
`state.validation.run` or an equivalent controlled validation action backed by a
profile. Validation profiles may allow cache or artifact writes, but source-tree
writes must be denied unless a later policy explicitly permits them.

Validation profiles are static repo-defined config at `.stateful/validation.yml`.
Agents cannot provide arbitrary commands at runtime.

Minimum v1 profile shape:

```text
profile_id
description
command
cwd
timeout_seconds
allowed_writes
denied_writes
exclusive
env
result_parser
```

Default result parsing is `exit_code`. Validation status values are `passed`,
`failed`, `failed_policy`, `timeout`, and `error`. Source-tree writes denied by
the profile produce `failed_policy`, not ordinary test failure.

V1 source-write detection uses `git status --porcelain` before and after the
validation command. If a path matching `denied_writes` is already dirty before
the run, validation does not start and returns `error`. If the run creates a new
dirty path matching `denied_writes`, validation returns `failed_policy`.
`allowed_writes` paths are ignored for policy failure.

If a validation profile is marked `exclusive`, concurrent runs of the same
profile in the workspace are denied. Non-exclusive concurrent runs produce
warning context only.

## Conflict Policy

Initial policy should prefer advisory leases:

- supported write with no active intent: deny
- supported write with only abstract task/test/port/migration intent: deny
- supported write with expired intent: deny
- supported write while phase is `blocked`: deny
- supported write after session finalization: deny
- supported write outside matching file or directory scope: deny
- Bash write or ambiguous mutation command: deny
- directory scope permits writes only up to depth 2 below that directory
- delete without exact file scope: deny
- rename or move without exact file scope for both source and destination: deny
- active lease in the hard conflict domain by another actor: deny unless the
  current session has an explicit user override for that resource
- same area planned by another actor: warn and add context
- same `workspace_id` and same normalized `absolute_path`: treat as same-file
  hard conflict
- same `repo_id` and same `relative_path` across different workspace, worktree,
  or branch: warn as a soft repo-relative conflict
- unknown repository identity: only same normalized absolute path can deny;
  repo-relative similarity is an unknown-confidence warning
- expired lease: allow but surface stale-state context
- test execution: allow only through controlled validation action
- same non-exclusive validation profile active elsewhere: warn
- same exclusive validation profile active elsewhere: deny
- task, port, or migration resource conflict: warn or info only in v1
- human local changes detected: warn before edits and require extra care
- human save observed after an agent lease or write: deny further agent writes
  until the agent acknowledges reconciliation or receives an explicit user
  instruction
- reads, searches, diffs, and controlled validation after human writes: allow

This avoids making the system too rigid while still preventing the highest-risk
collisions.

## Collision Domains

V1 should distinguish physical write collisions from repository-relative
coordination risks.

Hard conflict domain:

```text
same workspace_id + same normalized absolute_path
```

This represents the same physical file in the same working tree. Supported writes
in this domain should be denied when another actor has an active write lease,
unless the current session has an explicit user override for that resource.

Soft conflict domain:

```text
same repo_id + same relative_path + different workspace/worktree/branch
```

This represents likely future integration risk, not immediate file overwrite
risk. V1 should warn and add context, but should not deny by default.

Branch-aware warning:

```text
same repo_id + same relative_path + different branch
```

This should be rendered prominently because merge conflicts are likely, but it
should remain a warning unless a later policy explicitly promotes it.

If `repo_id` cannot be determined, the state server should only hard-block on
same normalized `absolute_path`. Any weaker match should be rendered as
unknown-confidence context.

## Human Activity

Human activity cannot be captured only through Codex hooks. The system should
also support external observation:

- git working tree diff
- filesystem watcher
- IDE integration
- pre-commit or pre-push hooks
- explicit human status updates

Human-derived state should carry lower confidence when inferred indirectly. For
example, a file watcher can say a file changed, but it cannot always know the
human's goal.

V1 uses conservative git working-tree or filesystem observation for human
activity. This is enough to trigger warnings and reconciliation blocks, but it
should not claim to understand the human's intent. The dedicated IDE save gate is
deferred to v2.

### V2 IDE Soft Save Gate

The chosen IDE direction is a soft save gate. A dedicated IDE extension should
observe human editing signals and check the state server before a save when the
editor exposes a pre-save event.

The extension should:

- report opened files, selected files, dirty buffers, save attempts, and save
  completions
- warn the user when a save conflicts with an active agent lease
- allow the user to continue the save explicitly
- record the user's decision as a human activity event
- fail open with a warning if the state server cannot be reached

This is not a hard lock. Autosave, external processes, other editors, git
operations, and editor-specific save paths can bypass the extension. The state
server should therefore treat IDE save-gate data as high-quality coordination
signal, not as a security boundary.

If a human save proceeds over an active agent lease, the system should mark the
affected file as changed by a human and warn or deny later agent writes until
the agent has refreshed context and reconciled the file.

### Agent Reconciliation After Human Writes

After `HumanWriteObserved` on a file with active agent work, the next write by
that agent to the affected file should be denied. The block is not meant to stop
investigation; the agent may still read the file, search, inspect diffs, and run
controlled validation.

To resume writing, the agent must acknowledge reconciliation:

```text
state.reconcile.ack(resources, files_reread, human_change_summary, conflict_with_plan, decision)
```

The acknowledgement must include:

- the affected file read after the human write
- a summary of the human change
- whether the change conflicts with the agent's previous plan
- the next direction: `adopt`, `reapply`, `ask_user`, or `abandon`

`adopt` means the agent accepts the human change and continues from it.
`reapply` means the agent will reapply its intended change on top of the human
change. `ask_user` keeps writes blocked until the user gives direction.
`abandon` releases or shortens the affected lease and keeps writes blocked for
that file.

Even after reconciliation, writes still require active, unexpired intent with
matching file or directory scope.

If the state server is unavailable, reconciliation cannot be acknowledged. The
agent may continue read/search/diff investigation, but writes stay blocked until
the server is reachable and `state.reconcile.ack` succeeds.

## State Server Availability

V1 uses a split failure policy:

```text
agent write path: fail closed
validation path: fail closed
reconciliation path: fail closed
human save path: fail open with warning
read/search/diff path: allow
```

When the state server is unavailable:

- supported writes are denied because active intent, lease conflict, and
  reconciliation state cannot be proven
- Bash writes and ambiguous mutation commands remain denied
- `state.validation.run` returns `error: state_unavailable` and does not run the
  validation command
- `state.reconcile.ack` fails and cannot clear an unreconciled-human-write block
- intent declaration, lease acquisition, and lease refresh fail
- read, search, and diff actions are allowed
- the IDE save gate warns the user but allows the human save to proceed
- hook and observer events that cannot be sent should be appended to a local
  outbox for later sync

The local outbox is append-only recovery evidence. It may hold heartbeat,
finalization, human activity, and observer events until the server becomes
reachable. It cannot authorize writes, clear reconciliation blocks, or extend
leases while the server is unavailable.

Outbox sync uses `outbox_id` as an idempotency key. Replaying the same outbox
entry must not create duplicate state events. Pending events should be synced in
local creation order per session; failed entries stay in the outbox with failure
metadata for later retry or inspection.

V1 has no cached write grace period. A recent successful lease or intent check is
not enough to continue writing while the state server is down.

## Known Limits

Codex hooks are a practical guardrail, not a complete enforcement boundary.
They can intercept important supported paths such as shell commands,
`apply_patch`, and MCP calls, but they do not make the state protocol a full
security boundary. Post-action hooks also cannot undo side effects that already
happened.

IDE save interception has the same character. It can warn before many human
saves, but it cannot guarantee exclusive file ownership.

This is acceptable for the MVP because the goal is coordination, not sandbox
security.

## Fork Option

Forking Codex CLI remains a possible later step.

A fork would be justified if the MVP proves that state coordination is valuable
but hooks cannot enforce the needed coverage. In that case, the fork should add
thin runtime primitives rather than product policy:

```text
ToolGate.before(tool_call) -> state-server authorize
ToolGate.after(tool_result) -> state-server observe
TurnLifecycle.before_stop -> state-server finalize check
HeartbeatLoop -> state-server heartbeat
```

The policy engine should still live in `stateful_core` so the fork stays small
and easier to rebase.

## Initial Decision

Build the first version with Codex lifecycle hooks, MCP tools, and a
`stateful_core` state server.

The v1 MVP includes Codex hooks, MCP tools, the state server, controlled
validation, and human-write reconciliation.

V1 is local-only. It coordinates one machine/workspace boundary. Team-shared,
cross-machine, or hosted state sync is deferred to v1.5/v2.

The project should remain agent-runtime generic, with Codex as the first
integration target. Codex-specific behavior belongs in adapters, not in the
policy model.

V1 defaults to strict enforcement. Supported writes, validation, and
reconciliation fail closed when state cannot be trusted. Usability comes from
clear denial messages and diagnostics, not from silent grace periods.

Event and audit history is retained for 14 days by default. Expired state may be
shown as handoff evidence, but retention does not extend live write authority.

The v1 hard block policy is:

```text
supported write action + no active intent -> deny
supported write action + expired intent -> deny
supported write action + blocked phase or finalized session -> deny
supported write action + intent without file/directory scope -> deny
supported write action + target outside intent scope -> deny
Bash write or ambiguous mutation command -> deny
delete action + non-exact file scope -> deny
rename/move action + non-exact source or destination scope -> deny
active write lease in hard conflict domain -> deny unless explicit user override
is present in the current session
```

For wait queue scheduling, the same hard conflict becomes a queued request when
the caller asks to queue on conflict. Queue promotion happens only after
explicit release, session or activity finalization, or lease expiry. Soft
repo-relative conflicts remain warning context in v1.

Overrides are not scheduling priority. An explicit override can allow a direct
write authorization exception for the current session and resource, but it does
not reorder existing waiters, steal a reservation, or move the overriding
session to the head of a queue.

A v1 write-authorizing intent must be active, unexpired, belong to the same
session, and include matching file or directory scope. Abstract task, test, port,
or migration intent can be stored as context but does not permit writes.
Directory scope permits writes only up to two path segments below the scoped
directory.

Resource authorization classes:

```text
file/directory -> write-authorizing
test -> controlled validation concurrency only
task/port/migration -> prompt context and warning only
```
Delete operations require exact file scope. Rename and move operations require
exact file scope for both source and destination.

Intent freshness defaults:

```text
default intent TTL: 15 minutes
default lease TTL: 5 minutes
default reservation TTL: 120 seconds
heartbeat extension: allowed only for active exploring/editing/testing work
maximum rolling intent lifetime: 60 minutes from declared_at
phase = blocked: visible but not write-authorizing
finalization done/failed/blocked: intent finalized
```

Override policy:

```text
automatic override: never allowed
override authority: explicit user instruction only
scope: current session + current turn + specific resource
applies to: active lease conflict only
does not bypass: missing intent, expired intent, blocked/finalized state
does not bypass: file/directory scope matching
does not bypass: delete/rename/move exact-scope rules
example: "Allow override for src/auth.ts."
responsibility: user owns the judgment and risk
```

Prompt context rendering:

```text
state.context.render(workspace, session_id, resources?, mode)
mode: brief | detailed
sections: Blocking, Required Next Action, Warnings, Nearby Activity, Stale/Expired
```

`brief` is used for session start and user prompt context. `detailed` is used
after denied actions or for focused resource checks. Rendering should be
actionable, not a raw event dump.

The renderer returns structured data plus prompt-ready markdown:

```text
status: clear | warn | blocked
mode: brief | detailed
generated_at
scope
items[]
prompt_text
```

Each rendered item uses:

```text
- [severity] resource: summary.
  next: concrete action.
  evidence: detailed mode only.
```

Severity values are `block`, `warn`, and `info`. `brief` mode has at most 8
bullets. `detailed` mode has at most 20 bullets. Empty sections are omitted
except `Blocking: None` when a resource filter is present. Raw event dumps are
forbidden.

When the block is an unreconciled human write, `Required Next Action` should tell
the agent to reread the file, summarize the human change, decide whether to
adopt, reapply, ask the user, or abandon, and call `state.reconcile.ack`.

Expired and finalized state is shown only in `Stale/Expired`. It never blocks a
live action by itself.

Summary windows:

```text
brief: max 3 stale/expired items, resource-relevant only
detailed: max 10 stale/expired items, resource-relevant first
expired active state without finalization: show for 24h
finalized done: show only when resource-filtered or directly relevant
finalized failed/blocked: show for 7d when resource-relevant
```

For expired active state without finalization, the renderer should say `final
status unknown`. For finalized work, it should include status, changed files,
remaining work, and age when available.

Do not fork Codex at the start. Use hooks to validate the protocol and identify
the exact enforcement gaps. Reconsider a fork only after those gaps are proven
by usage.

## References

- Codex Hooks: https://developers.openai.com/codex/hooks
- Codex Advanced Configuration, Hooks:
  https://developers.openai.com/codex/config-advanced#hooks
- Codex Configuration Reference:
  https://developers.openai.com/codex/config-reference#configtoml
