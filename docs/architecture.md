# Architecture

`stateful_core` is a coordination layer between agent runtimes and a shared
current-state server.

## MVP Shape

The first implementation target is:

```text
Codex lifecycle hooks
+ MCP tools
+ state server
```

Codex hooks observe and gate important agent actions. MCP tools give agents a
structured way to read and update coordination state. The state server owns
policy, persistence, TTLs, and conflict checks.

The v1 MVP includes sandboxed test execution through `sandbox run --write-dir`
after exact directory intent and a successful same-session directory lease, plus
human-write reconciliation through `state.reconcile.ack`.

V1 is local-only. It coordinates sessions, agents, subagents, and local human
activity inside one machine/workspace boundary. Team-shared, cross-machine, or
hosted synchronization is a later v1.5/v2 concern.

The project is agent-runtime generic, but the first integration is Codex-first.
The state server and policy API should not encode Codex-only assumptions beyond
adapter metadata.

Implementation defaults for API shape, SQLite tables, hook adapter behavior,
runtime files, CLI commands, and tests are fixed in
[Implementation Contract](implementation-contract.md). Product policy belongs in
this architecture document and the state model; implementation choices may
evolve only when they preserve those policies.

## Layers

```text
Codex hooks and external observers
-> state server API
-> append-only coordination event log
-> active current-state materializer
-> conflict policy
-> prompt context renderer
-> MCP tools for agent access
```

The event log is used for audit and replay. The materialized current-state view
is optimized for fast reads, conflict checks, and prompt rendering.

## Persistence

V1 uses SQLite for local persistence.

The database should contain:

- append-only coordination events
- materialized current-state tables
- conflict decision records
- local outbox sync records

JSONL alone is not the v1 persistence format because conflict checks, TTL
expiration, outbox replay, and audit queries need indexed reads. The event log
remains append-only inside SQLite so state can be replayed or inspected.

Event and audit history is retained for 14 days by default. This default keeps
handoff evidence available without turning current-state coordination into
long-term memory. Projects may configure a longer retention window.

The v1 runtime database lives under user-level `GlobalPaths` in global server
mode: `$STATEFUL_HOME/state.db`, or `$HOME/.stateful_core/state.db` when
`STATEFUL_HOME` is unset. Repo-local `.stateful_core/state.db` remains a
compatibility/runtime fallback. Committed repo configuration stays under
`.stateful/`, while runtime state must not be committed.

## API Transport

The state server should expose a local HTTP API. Hooks, MCP tools, filesystem
watchers, and the future IDE extension all call the same HTTP API.

MCP is an adapter over that API, not the policy owner:

```text
Codex hook script -> local HTTP API -> state server
MCP tool -> local HTTP API -> state server
IDE extension -> local HTTP API -> state server
watcher -> local HTTP API -> state server
```

This keeps policy centralized and avoids duplicating authorization logic inside
hook scripts or MCP tool handlers.

The concrete v1 HTTP API lives under `/v1` and is defined in the implementation
contract. The intended envelope includes `protocol_version`, session identity,
workspace identity, and source metadata. The current implementation enforces
that envelope for write authorization and intent declare/request/claim/cancel;
other POST routes still use flat request bodies.

## Hook Packaging

The prototype supports user-level installation with repo allowlist gating.
`stateful install --yes` configures global Codex hooks and MCP. `stateful enable`
opts the current repo into enforcement. Repo-local hooks remain available through
`stateful enable --repo-local-codex` as a compatibility fallback.

```text
global Codex hooks and MCP config
repo allowlist entry
optional <repo>/.codex/hooks.json compatibility fallback
<repo>/.stateful/config.yml
stateful binary available by absolute path or PATH lookup
```

The runtime event model stays the same across global hooks, repo-local
compatibility hooks, and later managed hooks. The later managed version should
move the same thin hook adapters to administrator-controlled paths and configure
them from `requirements.toml`.

Plugin packaging is a team-beta distribution layer, not the prototype
enforcement path. A plugin can bundle hooks, MCP config, skills, and docs, but
plugin hooks remain non-managed and require user trust. Managed hooks remain the
long-term path for organization-level enforcement.

Repo-local hook scripts must stay thin:

```text
parse Codex hook input
extract action and targets
call local HTTP state server
translate decision into Codex hook output
append local outbox event when observation cannot reach the server
```

They must not own policy. In v1, these hook adapters are Rust commands from the
compiled `stateful` binary. If the state server is unavailable, the hook follows
the availability policy: agent writes and reconciliation fail closed;
read/search/diff remains allowed.

Hook scripts should resolve paths from the git root. Envelope-enforced routes
include `protocol_version`; a major protocol mismatch fails closed on those
write authorization and intent paths.

## Trigger Sources

Currently implemented trigger sources:

- Codex `SessionStart`
- Codex `UserPromptSubmit`
- Codex `PreToolUse`
- Codex `PostToolUse`
- Codex `Stop`
- Codex subagent start/stop and tool activity
- MCP calls from the agent
- CLI and state server calls

Target and future trigger sources:

- git working tree or filesystem observation for conservative human activity
  detection
- IDE extension events for human file open, dirty, save-attempt, and save
  completion signals

Each trigger should carry the session id, actor identity when known, workspace,
branch, timestamp, and source reference.

Subagents inherit write authorization only from the parent session's active
valid intent scope. They still record activity and leases with their own
`actor_id` so attribution stays precise.

## Hook Responsibilities

`SessionStart`:

- register or resume the session
- record workspace and branch
- render active neighboring state into model context

`UserPromptSubmit`:

- capture the user's requested goal as an initial intent candidate
- attach relevant active conflicts as context

`PreToolUse`:

- deny supported write calls when the session has no active intent
- deny raw Bash and allow Bash only when the outer command is a single strict
  invocation of the trusted absolute `stateful` binary running
  `<absolute-stateful-binary> sandbox run ... --command <cmd>`. Read-only
  command-shaped inspection uses `--fs read-only --network disabled`;
  command-shaped writes use `--fs write-targets` with explicit write/create
  targets.
- check leases and planned edits for likely conflicts
- return allow, warning context, or deny based on policy

`PostToolUse`:

- observe files, commands, and results from supported tool calls
- refresh heartbeat and lease TTLs
- update phase, touched resources, and last result

`Stop`:

- require a final state for active work
- continue the turn when the agent has not finalized
- release or shorten leases for completed work

## MCP Surface

The v1 MCP/tool surface is intentionally narrow:

```text
state.session.register
state.session.heartbeat
state.intent.declare
state.intent.request
state.intent.claim
state.intent.cancel
state.lease.acquire
state.lease.release
state.activity.observe
state.activity.finalize
state.conflicts.check
state.current.read
state.events.read
state.context.render
state.reconcile.ack
state.notifications.poll
state.resume.next
```

Hooks and MCP tools should call the same state server API. Policy must live in
the state server, not in duplicated hook scripts. Native Codex edit tools are
the repo file edit path after exact intent declaration and a successful file
lease; command-shaped shell writes remain outside MCP and go through the
sandbox-run wrapper.

Intent declare and request payloads require a non-empty `purpose`; clients infer
it from the user or agent instruction and send it explicitly.

## Tool Classification

V1 enforcement is strict about write target extraction:

- Native Codex edit tools such as `apply_patch`, `Edit`, and `Write`: enforce
  by inspecting hook-exposed targets after exact intent declaration and a
  successful same-session file lease.
- Bash commands: deny raw Bash. Hook-mediated Bash is allowed only when the
  outer command is a single strict invocation of the trusted absolute `stateful`
  binary running `<absolute-stateful-binary> sandbox run ... --command <cmd>`.
  Read-only command-shaped inspection uses `--fs read-only --network disabled`;
  command-shaped writes use `--fs write-targets` with explicit
  `--write-target` / `--create-target` values and target authorization.
- Test execution: run only through sandboxed test actions such as
  `stateful sandbox run --fs write-targets --write-dir target --command <cmd>`
  after exact `target/` directory intent and a successful same-session
  directory lease.
- Bash command text alone never authorizes tool use, even when it appears
  read-only.

Denied Bash should direct the agent to native Codex edit tools for repo file
edits, the wrapper for command-shaped shell execution, and sandbox-run wrappers
for tests.

MCP does not perform local command-shaped file writes. Hook-mediated shell
execution uses `<absolute-stateful-binary> sandbox run ... --command <cmd>`;
plain CLI-context usage outside hooks can use `stateful sandbox run`. The MVP
ships `read-only` and `write-targets` profiles; `git-metadata` and `workspace`
profiles are deferred and fail closed. `/dev/null` is writable in every sandbox
profile so common shell and Git behavior works. Command text alone does not
authorize `rg`, `git diff`, test runners, stateful operational commands, or any
other Bash command.

## Sandboxed Tests

Agents cannot run raw Bash test commands through hooks. They call the trusted
wrapper after exact `target/` directory intent and a successful same-session
directory lease:

```text
stateful intent declare --session-id <session> --workspace-id <workspace> --purpose "Run the requested tests." target/
stateful mcp call state_lease_acquire '{"session_id":"<session>","workspace_id":"<workspace>","path":"target/"}'
stateful sandbox run --fs write-targets --network enabled --write-dir target --command <cmd>
```

The wrapper authorizes the `target/` artifact directory before execution and the
OS sandbox limits writes to declared file targets, create targets, and the
target artifact tree. Source-tree writes remain outside the allowed surface
unless exact targets are declared and authorized.

## State Server

The state server is responsible for:

- appending coordination events
- materializing active current state
- expiring stale activity and leases
- extending active intent TTL from heartbeat events within a 60-minute rolling
  maximum
- evaluating conflict policy
- promoting FIFO wait queue reservations after explicit lease release,
  session/activity finalization, or lease expiry
- requiring reservation claim before creating active write-authorizing intent
  and active leases
- rendering concise prompt context
- retaining expired activity as historical evidence

The server should block supported write actions without active intent and
support advisory blocking for high-risk conflicts. V1 does not treat leases as
hard distributed locks.

## Policy Engine

All write, reconciliation, and conflict checks should flow through a
single policy entry point:

```text
authorize_action(input) -> decision
```

The decision result should include:

```text
decision: allow | warn | deny | error
reason
conflicts[]
required_next_action
context_items[]
audit_event
```

The policy engine owns:

- active intent checks
- file and directory scope checks
- lease conflict checks
- collision-domain evaluation
- human-write reconciliation checks
- state-server availability behavior

Hooks and adapters only extract tool intent and targets, then call the policy
engine. They should not implement separate policy branches.

## IDE Soft Save Gate

V2 should include a dedicated IDE extension for human activity. Its purpose is
to create a soft save gate, not a guaranteed lock. The extension should:

- report opened, dirty, selected, and saved files as human activity signals
- check the state server before a human save when the IDE exposes a pre-save
  event
- warn the user when the save target conflicts with an active agent lease
- let the user explicitly continue the save, recording that decision as an
  audited human event
- fail open with a visible warning if the state server is unavailable, because
  losing human work is worse than missing a coordination warning

The save gate does not grant agent override authority. If a human continues a
conflicting save, later agent writes to that file should be denied or warned
until the agent refreshes state and reconciles the change.

V1 does not require the IDE extension. Human-write reconciliation in v1 is
driven by conservative git working-tree or filesystem observation. The observer
should prefer warnings and reconciliation blocks over pretending to know the
human's intent.

## Human Write Reconciliation

The current implementation records `state.reconcile.ack` acknowledgements but
does not yet observe `HumanWriteObserved` events or block writes because of
human-written files. The target policy is:

When `HumanWriteObserved` affects a file with active agent work, the next agent
write to that file is blocked until reconciliation is acknowledged.

During this blocked state, the agent may still:

- read the affected file
- search the repository
- inspect diffs
- run sandboxed tests with the authorized `target/` artifact tree

To resume writing, the agent must call:

```text
state.reconcile.ack(resources, files_reread, human_change_summary, conflict_with_plan, decision)
```

The acknowledgement must show that the agent reread the affected file, summarize
the human change, state whether it conflicts with the agent's previous plan, and
choose `adopt`, `reapply`, `ask_user`, or `abandon`.

Only `adopt` and `reapply` can clear the human-write block, and only when the
session still has active, unexpired, matching intent. `ask_user` keeps writes
blocked until the user responds. `abandon` should release or shorten the
affected lease.

If the state server is unavailable, `state.reconcile.ack` fails closed and cannot
clear the block.

## Conflict Policy

Initial policy:

- no active intent before supported write action: deny
- intent without matching file or directory scope before supported write action:
  deny
- expired intent or intent beyond its 60-minute rolling window: deny
- blocked phase or finalized session before supported write action: deny
- directory intent scope only permits targets up to depth 2 below that directory
- delete operation without exact file scope: deny
- rename or move without exact file scope for both source and destination: deny
- active lease in the hard conflict domain by another actor: deny unless the
  current session contains an explicit user override for that resource
- planned edit in the same area: warn and add context
- same `workspace_id` and same normalized `absolute_path`: treat as same-file
  hard conflict
- same `repo_id` and same `relative_path` across different workspace, worktree,
  or branch: warn as a soft repo-relative conflict
- unknown repository identity: only same normalized absolute path can deny;
  repo-relative similarity is an unknown-confidence warning
- expired lease: allow and surface stale context
- human working tree change near the target: warn before edits
- human save observed after an agent lease or write: deny further agent writes
  until the agent acknowledges reconciliation or receives an explicit user
  instruction
- unrelated reads and searches: allow
- reads, searches, diffs, and sandboxed tests after human writes: allow
- tests: allow only through trusted sandbox-run wrappers with authorized targets
- task, port, or migration resource conflict: warn or info only in v1

Conflict decisions must be auditable. Overrides are never automatic. They are
valid only when the user explicitly instructs the current session to allow a
specific resource override, for example: "Allow override for `src/auth.ts`."
The user owns the judgment and responsibility for that exception.

Overrides apply only to active lease conflicts. They do not bypass missing
intent, expired intent, blocked/finalized state, file or directory scope
matching, delete exact-scope rules, or rename/move exact-scope rules. V1
overrides are scoped to the current session, current turn, and specific
resource.

Overrides do not act as queue priority. They cannot reorder FIFO waiters,
transfer a reservation, or let a later waiter take a resource ahead of the
reserved session.

## Prompt Rendering

Prompt context should be concise and actionable:

```text
Blocking
Required Next Action
Warnings
Nearby Activity
Stale/Expired
```

Raw event logs should not be dumped into prompts. The rendered view should help
the agent decide what to avoid, wait for, or coordinate.

`state.context.render` supports `brief` and `detailed` modes plus an optional
resource filter. `brief` is for session start and prompt submit context.
`detailed` is for denied actions or focused resource checks. Rendered output
must include concrete next actions when a block or warning is present.

The current server route accepts these inputs but returns an empty context
package. Store-backed rendering of active conflicts, warnings, and stale state
is future hardening work.

The renderer should return both structured data and prompt-ready markdown:

```text
status: clear | warn | blocked
mode: brief | detailed
generated_at
scope
items[]
prompt_text
```

Prompt items must use this shape:

```text
- [severity] resource: summary.
  next: concrete action.
  evidence: detailed mode only.
```

Severity values are `block`, `warn`, and `info`. `brief` output is limited to 8
bullets total. `detailed` output is limited to 20 bullets total. Empty sections
are omitted except `Blocking: None` when a resource filter is present.

Block and warning items must include `next:`. `Required Next Action` appears
immediately after `Blocking` so the agent sees the recovery path before nearby
context.

`Stale/Expired` is informational only. It can never be the sole reason for a
live denial.

## Failure Modes

The system should prefer explicit uncertainty:

- missing heartbeat -> expire or mark unknown, not success
- interrupted session -> keep last state until TTL expires
- hook failure -> warn and fail closed only for high-risk writes
- state server unavailable -> deny supported writes that cannot prove active
  intent
- state server unavailable -> deny raw Bash and any Bash call that is not a
  strict `<absolute-stateful-binary> sandbox run ... --command <cmd>` wrapper;
  command-shaped writes through `--fs write-targets` fail closed when target
  authorization cannot be proven
- state server unavailable -> write-target sandbox authorization fails closed
  and does not run the command
- state server unavailable -> fail closed for `state.reconcile.ack`, intent
  declaration, lease acquisition, and lease refresh
- state server unavailable -> allow non-Bash read, search, and diff actions
- state server unavailable during IDE human save gate -> warn the user and allow
  the save
- heartbeat, finalization, and human observer events that cannot reach the state
  server should be appended to a local outbox for later sync
- local outbox events are audit/recovery evidence only; they cannot authorize
  writes while the state server is unavailable
- outbox sync uses `outbox_id` as an idempotency key; duplicate sync attempts
  must not create duplicate state events
- outbox sync preserves local creation order per session when replaying pending
  events
- cached write grace periods are not part of v1
- stale conflict -> allow with context, not hard block

V1 defaults to strict enforcement. Supported writes and reconciliation fail
closed when state cannot be trusted. To keep this usable,
denial responses must explain the missing precondition and the next action
instead of returning opaque policy failures. The prototype `stateful doctor`
reports install/config/repo-enabled state plus global path and registry errors;
prescriptive next-action guidance is future doctor UX work.
