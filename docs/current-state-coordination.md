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
time-bound summary of what an actor is doing now, what it plans to do next,
and which files, tasks, or resources are likely to conflict.

```text
memory = past evidence and recall
current state = active, expiring operational truth
```

The goal is to let an agent reason like this:

```text
Another agent is editing auth validation and plans to run auth tests.
I should avoid auth.ts, work on related tests, or wait for the claim to expire.
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
purpose
goal
phase: exploring | editing | testing | blocked | done | failed
files_read
files_editing
files_planned
resource_claims
next_plan
summary
last_result
last_updated_at
expires_at
confidence
source_refs
```

The shipped phase vocabulary matches the policy enum. `idle` is target
vocabulary for future activity summaries.

The important fields are not only current edits. `next_plan` is often more
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

The default reservation TTL is 15 minutes. Heartbeats can extend an active reservation,
but not beyond a 60-minute rolling maximum from `declared_at`.

## Protocol

The coordination protocol should be explicit:

```text
1. register session
2. declare reservation
3. acquire advisory claim
4. send heartbeat while active
5. update phase as work changes
6. observe tool effects
7. finalize as done, failed, or blocked
8. release or expire claim
9. create claimable reservations for released resources with queued waiters
10. notify sessions with claimable reservations so they can resume
```

Both start and end matter. Start-only reporting creates stale blocking state. End-only
reporting fails to prevent conflicts while work is happening.

## Wait Queue and Resume

Conflict handling is resource-scoped. If one session has an active claim for a
file, other sessions are not globally blocked. They can continue reading,
searching, validating, or editing unrelated files. Only writes that target a
resource with an active claim are denied.

When a denied write asks to queue on conflict, the state server records a
waiter for the normalized workspace resource:

```text
workspace_id + relative_path + action
```

The wait queue is FIFO. A queued hard-conflict request is promoted only when the
requested resource is available. The shipped queue stores one requested path
per wait request, and `/v1/reservation/request` accepts only `write_file` and
`write_directory` scheduling requests. The target multi-resource reservation
model is atomic all-or-nothing: if one file or directory in a multi-resource
request is still blocked, the whole request stays queued. For a multi-resource
request, "available" means the request is at the head of every resource queue it
participates in and every requested resource has no active claim.

`rename_file` and `move_file` remain immediate authorization actions only in the
shipped v1 policy. They require exact source and destination file reservation and
claims, but conflicting rename/move attempts do not enqueue wait records until
the multi-resource scheduler is implemented.

The server promotes every eligible queued waiter when one of these grant
triggers makes the requested resource available:

- explicit claim release
- session or activity finalization
- claim expiry

Each promoted waiter receives a short claimable reservation (the shipped API
state is `reserved`). Claimable reservations prevent a later session from taking
the same resource ahead of an earlier conflicting waiter, but they are not active
write authority. The promotion records a pending notification payload containing
the wait queue row's stored, non-empty purpose. The session with the claimable
reservation must reread the target. Manual MCP/CLI flows then call
`state_reservation_claim` / `stateful reservation claim --wait-id <id>` before writing;
the claim uses the stored reservation purpose, and clients do not provide a new
claim purpose. Native edit hooks and sandbox `write-targets` authorization can
lazy-claim the claimable reservation when the write is retried. Claiming creates
active reservation scope and the active same-session claim. The default
claimable reservation TTL is 120 seconds. If a claimable reservation expires
without being claimed, the server may promote the next eligible FIFO waiter.

Resume is notification-driven rather than process-driven. The state server
records a pending notification with the stored purpose when it promotes a
claimable reservation. Agents and orchestrators discover that signal by polling
notifications, asking for the next claimable reservation, or receiving that
context from a lifecycle hook. Polling returns each pending notification once and
marks it delivered; callers that miss or discard a poll response should use
`stateful resume next` / `state_resume_next` to rediscover any still-active
claimable reservation and its stored purpose. The state server does not wake a sleeping
Codex process by itself; external orchestration can build on the notification
and resume APIs.

Full scheduling works through immediate request/response plus polling. Reservation
request APIs return `queued`, `reserved` (claimable reservation), `claimed`,
`canceled`, or `expired` state without blocking indefinitely. Immediate
availability creates a claimable reservation with API state `reserved`; the
session must still reread the target. Manual MCP/CLI flows claim with
`state_reservation_claim` / `stateful reservation claim --wait-id <id>` before retrying;
the server uses the stored reservation purpose. Native edit hooks and sandbox
`write-targets` authorization can lazy-claim at the retry write boundary.
Waiting is handled by polling `stateful notifications poll` or `stateful resume
next`; both reservation surfaces expose the stored purpose.
`stateful reservation wait --timeout` is not part of the v1 hardening implementation.

`request_id` and non-empty `purpose` are required for idempotency and live state
explanation. Reservation declaration also requires non-empty `files_planned`; empty
arrays and empty or normalized-empty paths are rejected with `missing_scope`.
Reservation request also requires a non-empty `path`; empty or normalized-empty
request paths are rejected with `missing_scope`. Repeating the same request id
returns the existing request state and must not create duplicate queue entries or
replace the original purpose. Repeating an expired request requeues the same
waiter in place, preserving its original FIFO row while requiring a new
reservation and claim before writing. A queued or claimable (`reserved`) request can be
canceled explicitly with `state_reservation_cancel` /
`stateful reservation cancel --request-id <id>`. Session or activity finalization
releases that session's active claims, cancels that session's queued and
claimable (`reserved`) requests, and promotes the next eligible waiter for any
released or canceled resource.

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
session has active reservation. Conflict enforcement is advisory but blocking where
supported runtime adapters expose a hookable action. Hard global locks are a
later policy decision.

## Agent Runtime MVP

The chosen MVP direction is:

```text
Codex lifecycle hooks and OMP extension hooks
+ MCP tools
+ state server
```

This gives useful control without forking Codex or changing the default OMP
profile.

The prototype supports user-level installation with repo allowlist gating.
`stateful install --agent codex --yes` configures global Codex hooks and MCP.
For OMP, `stateful install --agent omp --yes` writes OMP config containing the
stateful extension under the OMP `stateful` profile agent directory
(`~/.omp/profiles/stateful/agent`) and ensures the target keys
`tools.approvalMode: yolo`, `bash.enabled: false`, `eval.py: false`,
`eval.js: false`, `eval.rb: false`, and `eval.jl: false`; it removes
`tools.approval` from the stateful profile because yolo mode delegates safety to
Stateful hooks. Without `--update`, existing scalar values are preserved and
only missing keys are inserted; with `--update`, existing target scalar values
are overwritten. Raw Bash plus the Python/JavaScript/JS/Ruby/Julia eval
tools are denied at the host approval and hook levels. The installer also writes
`rules/stateful-required.md` and `skills/stateful-command-policy/SKILL.md` under
that isolated agent directory:
the always-apply rule tells the model when Stateful policy applies, the skill
keeps the detailed procedure, and hooks remain the enforcement boundary. The
generated extension registers `sandbox_bash` for read-only, write-targets,
build, git, and github-pr sandbox runs, including common sandbox flags,
registers `ext_ro_bash` for read-only `--fs external` commands, and registers
`ext_rw_bash` for external writes that require write/create/dir scope and OMP UI
confirmation. All three generated `*_bash` tools wait for the sandbox command
to finish before returning the tool result, so final stdout/stderr/status are
available before the agent can end the turn. They emit stdout through inline OMP
tool updates so it renders in the tool output panel, and OMP abort/ESC cancels
the foreground tool while the sandbox runner cleans up its child process group.
Their `async` input is a deprecated compatibility no-op that does not select
background execution. `sandbox_bash` rejects
`--fs external` with guidance to use `ext_ro_bash` or `ext_rw_bash`. Raw Bash and
Python/JavaScript/JS/Ruby/Julia
eval-tool calls
are blocked even if their command text
invokes `stateful sandbox run`. The OMP
global/default profile is not modified.
`stateful enable` opts the current repo into enforcement through the user-level
install and repo allowlist.

```text
global Codex hooks and MCP config
isolated OMP `stateful` profile config
repo allowlist entry
optional <repo>/.codex/hooks.json compatibility fallback
<repo>/.stateful/config.yml
stateful binary available by absolute path or PATH lookup
```

Codex global hooks, repo-local compatibility hooks, and managed Codex hooks
share the Codex lifecycle model: `SessionStart`, `UserPromptSubmit`,
`PreToolUse`, `PostToolUse`, and `Stop`. The isolated OMP `stateful` profile
uses OMP extension entry points for `SessionStart`, `PreToolUse`,
`PostToolUse`, and `Stop`; OMP does not expose `UserPromptSubmit`. OMP
`session-start` prefers the actual runtime id from `event.sessionId` or
`ctx.sessionManager.session.id`, stores it in `process.env.STATEFUL_SESSION_ID`,
and persists the same current-session files used by session-aware CLI and MCP
callers. With that state in place, `state_session_register` ->
`state_reservation_declare` -> `state_claim_acquire` resolves the active OMP session
without a caller-supplied environment override. Plugin packaging is deferred to
team beta for distribution and update UX, while managed hooks remain the
long-term organization-enforcement path.

Hook scripts are thin integration adapters. They parse runtime hook input,
classify runtime-specific tool calls, extract action and targets when supported,
call the local HTTP state server for store-backed coordination policy, and
translate the decision back into runtime hook output. Adapter-local policy is
limited to fail-closed classification and trusted wrapper validation for
command-shaped execution. V1 implementation is Rust-only, so hooks invoke the
compiled `stateful` binary.

Envelope-enforced write authorization, reservation, and reconciliation requests
include `protocol_version`; a major protocol mismatch fails closed on those
paths. OMP `SessionStart`, `PostToolUse`, and `Stop` lifecycle posts use flat
session-event bodies with `metadata` and `source`, while OMP `PreToolUse`
authorization still uses the v1 envelope.

OMP adapters preserve stateful hard blocks: `sandbox_bash` owns non-external
sandbox command execution for read-only, write-targets, build, git, and
github-pr profiles; `ext_ro_bash` owns read-only external commands without OMP
UI confirmation; `ext_rw_bash` owns external writes with OMP UI confirmation;
all generated `*_bash` tools wait for the sandbox command to finish before
returning, emit stdout through inline OMP tool updates, and return final
stdout/stderr/exit status in tool details;
stateful allow maps to allow; and stateful denial or unavailable state maps to
block even when OMP yolo metadata is present.

### Hook Responsibilities

These responsibilities apply to Codex hooks unless noted. OMP supports
`SessionStart`, `PreToolUse`, `PostToolUse`, and `Stop`; it does not expose a
`UserPromptSubmit` hook.

`SessionStart`:

- register the session
- record workspace, branch, and session id
- inject relevant active state into the model context

`UserPromptSubmit`:

- extract or request an initial reservation when possible
- attach nearby active states as context

`PreToolUse`:

- intercept supported tool calls before execution
- deny supported write calls when the session has no active reservation
- deny Codex raw Bash with sandbox guidance. For OMP, raw Bash and the
  Python/JavaScript/JS/Ruby/Julia eval tools are denied at host approval and hook
  levels, even when the raw command itself invokes `stateful sandbox run`;
  non-external sandbox command work must use `sandbox_bash`, read-only
  repo-external command-shaped work must use `ext_ro_bash` without OMP UI
  confirmation, and external writes must use `ext_rw_bash` with write/create/dir
  scope and OMP UI confirmation.
- check whether requested files or resources conflict with active claims
- deny, warn, or add context based on policy

`PostToolUse`:

- observe supported tool results
- update files touched, phase, test results, and last result
- release same-session repo-write claims after completed native edit and
  `write-targets` transactions
- refresh heartbeat timestamps and claim timestamps only for remaining active
  claims still covered by active reservation

`Stop`:

- post activity finalization for the session
- release the session's claims through finalization
- leave explicit `state_activity_finalize` available for manual final status
  updates before shutdown

Subagents:

- coordinate in the same workspace state but use their own effective session
  identity when the adapter exposes one
- record activity and claims with their own `actor_id`
- do not receive automatic override authority from a shared owner

### MCP Responsibilities

The MCP surface should expose structured tools for the agent and hooks. Canonical
callable tool names map to dotted protocol names:

```text
state_session_register (state.session.register)
state_session_heartbeat (state.session.heartbeat)
state_reservation_declare (state.reservation.declare)
state_reservation_request (state.reservation.request)
state_reservation_claim (state.reservation.claim)
state_reservation_cancel (state.reservation.cancel)
state_claim_acquire (state.claim.acquire)
state_claim_release (state.claim.release)
state_activity_observe (state.activity.observe)
state_activity_finalize (state.activity.finalize)
state_conflicts_check (state.conflicts.check)
state_current_read (state.current.read)
state_events_read (state.events.read)
state_context_render (state.context.render)
state_reconcile_ack (state.reconcile.ack)
state_notifications_poll (state.notifications.poll)
state_resume_next (state.resume.next)
```

`state_reservation_declare` and `state_reservation_request` require a non-empty `purpose`.
The caller must infer that purpose from the user or agent instruction when it is
not explicit; the server must not synthesize a fallback purpose.
`state_reservation_declare` also requires non-empty `files_planned`; empty arrays and
empty or normalized-empty entries are rejected with `missing_scope`.
`state_reservation_request` also requires a non-empty `path`; empty or
normalized-empty request paths are rejected with `missing_scope`.
`state_reservation_request` and `state_reservation_cancel` expose the explicit scheduling
queue. `state_reservation_claim` takes a `wait_id` only and uses the stored
reservation purpose; callers must not send a claim purpose.

Hooks should call the same state server API as MCP tools so policy remains
centralized. Native edit tools with hook-visible targets are the repo file edit
path after reservation and claim; sandbox-run remains the Bash wrapper for
command-shaped shell writes.

### State Server Responsibilities

The state server is the source of truth for active coordination state.

It should:

- store append-only state events
- materialize active current state
- evaluate conflict rules
- manage claim TTLs
- expose current-state views for prompt rendering
- retain historical activity as evidence after expiration

Runtime hook integrations should stay thin. Store-backed coordination policy
belongs to the server, not to hook scripts. Hook logic should only handle
runtime parsing, fail-closed unknown-tool handling, and trusted wrapper validation.

V1 persistence is SQLite with append-only event tables and materialized
current-state views. JSONL may be used for debugging exports, but not as the
primary store.

V1 transport is local HTTP. Codex hooks, OMP hooks, MCP tools, filesystem
watchers, and the future IDE extension should call the same local HTTP API. MCP
remains an adapter, not a separate policy implementation.

Policy evaluation should use one entry point:

```text
authorize_action(input) -> decision
```

The shipped `/v1/authorize` decision includes `allow`, `deny`, or `error` plus a
reason and required next action when needed. A `warn` decision, response-level
conflicts, rendered context items, and response-level audit-event fields are
target response vocabulary. Deny decisions are audited as
`AuthorizationDenied`; shipped lifecycle mutations also append
`ClaimReleased`, `ReservationClaimed`, `ReservationCanceled`, and `ActivityFinalized`
events.

### Tool Classification

V1 write enforcement is limited to tool paths where targets can be determined
reliably:

```text
namespaced runtime tool names -> classify by leaf
  (functions.bash as Bash; functions.python/javascript/js/ruby/julia as eval
  tools; functions.read / functions.search as native read/search)
native read/search/diff tools -> preferred path for ordinary read work
native edit tools with hook-visible targets -> enforce by inspecting targets
  after task-level reservation covers the target and a same-session claim; release the claim after the
  completed write transaction
Codex Bash read-only inspection that genuinely needs a shell -> require a strict
  trusted wrapper:
  <absolute-stateful-binary> sandbox run --fs read-only --network disabled
  --command <cmd>
OMP read-only/write-targets/build/git/github-pr sandbox runs -> require
  `sandbox_bash`
process inspection -> use sandbox process find <selector>, not raw ps/pgrep
Codex Bash command-shaped repo writes -> require the trusted wrapper with
  --fs write-targets plus explicit --write-target <file>/--create-target <file> values
test execution -> run through sandbox run --fs build --network enabled with
  --write-dir <scratch-purpose>; scratch lives under /tmp/stateful/<session>/
Codex raw Bash, OMP raw Bash, or OMP Python/JavaScript/JS/Ruby/Julia eval tools
  -> deny
repo-external OMP command-shaped work -> require `ext_ro_bash` for reads or `ext_rw_bash` for writes
```

Bash denial should tell the agent to use native read/search/diff tools for
ordinary read work,
`<absolute-stateful-binary> sandbox run --fs read-only --network disabled
--command <cmd>` for Codex shell-based read-only inspection,
`<absolute-stateful-binary> sandbox run --fs write-targets --write-target <file> ... --command <cmd>`
for Codex command-shaped repo writes after reservation and same-session claim,
OMP `sandbox_bash` for read-only, write-targets, build, git, and github-pr
sandbox runs,
Codex `<absolute-stateful-binary> sandbox run --fs external --purpose ...
--command <cmd>`, OMP `ext_ro_bash` for read-only external work, or OMP
`ext_rw_bash` for approved repo-external writes,
and native edit tools for repo file edits.

The read-only sandbox profile is a write-confinement profile. It does not
provide full process containment, and it cannot be combined with
`--network enabled`.

There is no command-text authorization path. Command text alone does not
authorize `rg`, `git diff`, test runners, stateful operational commands, or any
other Bash command. Test commands should run through the trusted
`stateful sandbox run --fs build --network enabled --write-dir <scratch-purpose>
--command <cmd>` wrapper. Repo writes require task-level reservation covering
the target plus a matching same-session claim and must use native edit tools or
`--fs write-targets` with explicit targets.

Minimum sandboxed test shape:

```text
stateful sandbox run --fs build --network enabled --write-dir test-run --command <cmd>
```

The build profile writes disposable artifacts under
`/tmp/stateful/<session>/<scratch-purpose>/`. Source-tree edits use native edit tools
with hook-visible targets, such as Codex `apply_patch` or Edit, after exact
reservation declaration and a successful same-session file claim; the completed write
transaction releases the authorizing claim. Command-shaped source writes must
use exact `--write-target <file>` or `--create-target <file>` entries.

## Conflict Policy

Initial policy should prefer advisory claims:

- supported write with no active reservation: deny
- supported write with only abstract task/test/port/migration reservation: deny
- supported write with expired reservation: deny
- supported write while phase is `blocked`: deny
- supported write after session finalization: deny
- supported write outside matching exact file scope or exact directory
  `write_directory` scope: deny
- Codex raw Bash or non-wrapper Bash: deny, including repo-external
  command-shaped work
- OMP raw Bash and Python/JavaScript/JS/Ruby/Julia eval-tool execution: deny at
  host approval and hook levels, even when the raw command invokes
  `stateful sandbox run`; use `sandbox_bash` for non-external sandbox profiles,
  `ext_ro_bash` for read-only `--fs external`, and `ext_rw_bash` for external
  writes
- directory reservation and directory claim authorize only `write_directory` for the
  exact directory resource; they do not authorize `write_file`, delete, rename,
  or move actions on child paths
- `write_directory` requires exact directory scope, and the matching directory
  claim blocks the whole subtree because command-shaped `--write-dir` execution
  receives writable access to that subtree
- delete without exact file scope: deny
- rename or move without exact file scope for both source and destination: deny
- active claim in the hard conflict domain by another actor: deny unless the
  current session has an explicit user override for that resource
- same `workspace_id` and same normalized `relative_path`: shipped hard conflict
  domain for active claim and wait-queue checks
- same normalized `absolute_path`: target physical-file hard conflict domain
- same area planned by another actor: target warning context
- same `repo_id` and same `relative_path` across different workspace, worktree,
  or branch: target soft repo-relative warning
- unknown repository identity: target behavior is to hard-block only on same
  normalized absolute path and render weaker matches as unknown-confidence
  context
- expired claim: allow but surface stale-state context
- test execution: allow only through trusted sandbox-run wrappers with
  authorized targets
- task, port, or migration resource conflict: warn or info only in v1
- human local changes detected: warn before edits and require extra care
- human save observed after an agent claim or write: deny further agent writes
  until the agent acknowledges reconciliation or receives an explicit user
  instruction
- reads, searches, diffs, and sandboxed tests after human writes: allow

The human-local-change and human-save bullets are target behavior for future
watcher or IDE integrations. Shipped v1 covers external changes to files with
active exact file claims by comparing the claim-time file observation at
hook-originated write authorization.

This avoids making the system too rigid while still preventing the highest-risk
collisions.

## Collision Domains

V1 should distinguish shipped physical write collisions from target
repository-relative coordination risks.

Shipped hard conflict domain:

```text
same workspace_id + same normalized relative_path
```

This represents the same workspace-relative file in the same enabled workspace.
Supported writes in this domain are denied when another actor has an active
write claim or reservation. `absolute_path` is part of the target physical-file
domain but is not populated for shipped claim conflict checks.

Target soft conflict domain:

```text
same repo_id + same relative_path + different workspace/worktree/branch
```

This represents likely future integration risk, not immediate file overwrite
risk. It should warn and add context, but should not deny by default. The shipped
authorization path does not yet emit warn-tier decisions for these matches.

Branch-aware warning:

```text
same repo_id + same relative_path + different branch
```

This should be rendered prominently because merge conflicts are likely, but it
should remain a warning unless a later policy explicitly promotes it.

If `repo_id` cannot be determined, the state server should only hard-block on
same normalized `absolute_path` once that target domain is populated. Any weaker
match should be rendered as unknown-confidence context.

## Human Activity

Human activity cannot be captured only through agent runtime hooks. The system
should also support external observation:

- git working tree diff
- filesystem watcher
- IDE integration
- pre-commit or pre-push hooks
- explicit human status updates

Human-derived state should carry lower confidence when inferred indirectly. For
example, a file watcher can say a file changed, but it cannot always know the
human's goal.

The current implementation does not ship a filesystem watcher, git working-tree
observer, or IDE integration for human activity. `state.reconcile.ack` exists as
an explicit acknowledgement record, but human-write observation and automatic
reconciliation blocks are future integrations.

### V2 IDE Soft Save Gate

The chosen IDE direction is a soft save gate. A dedicated IDE extension should
observe human editing signals and check the state server before a save when the
editor exposes a pre-save event.

The extension should:

- report opened files, selected files, dirty buffers, save attempts, and save
  completions
- warn the user when a save conflicts with an active agent claim
- allow the user to continue the save explicitly
- record the user's decision as a human activity event
- fail open with a warning if the state server cannot be reached

This is not a hard lock. Autosave, external processes, other editors, git
operations, and editor-specific save paths can bypass the extension. The state
server should therefore treat IDE save-gate data as high-quality coordination
signal, not as a security boundary.

If a human save proceeds over an active agent claim, the system should mark the
affected file as changed by a human and warn or deny later agent writes until
the agent has refreshed context and reconciled the file.

### Agent Reconciliation After Human Writes

The current implementation records reconciliation acknowledgements but does not
yet emit `HumanWriteObserved` or deny writes because of observed human writes.
The target policy is:

After `HumanWriteObserved` on a file with active agent work, the next write by
that agent to the affected file should be denied. The block is not meant to stop
investigation; the agent may still read the file, search, inspect diffs, and run
sandboxed tests.

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
`reapply` means the agent will reapply its planned change on top of the human
change. `ask_user` keeps writes blocked until the user gives direction.
`abandon` releases or shortens the affected claim and keeps writes blocked for
that file.

Even after reconciliation, writes still require active, unexpired reservation with
matching file or directory scope.

If the state server is unavailable, reconciliation cannot be acknowledged. The
agent may continue read/search/diff investigation, but writes stay blocked until
the server is reachable and `state.reconcile.ack` succeeds.

## State Server Availability

V1 uses a split failure policy:

```text
agent write path: fail closed
sandbox write-target path: fail closed
reconciliation path: fail closed
human save path: fail open with warning
non-Bash read/search/diff path: allow
```

At the OMP adapter boundary, a stateful hook deny or unavailable result is
returned as block, not warning, regardless of OMP yolo metadata. Non-external
sandbox runs must use `sandbox_bash`, repo-external command-shaped reads must
use `ext_ro_bash`, external writes must use `ext_rw_bash`, and raw OMP Bash plus
Python/JavaScript/JS/Ruby/Julia
eval-tool sandbox invocations are denied.

When the state server is unavailable:

- supported writes are denied because active reservation, claim conflict, and
  reconciliation state cannot be proven
- Codex raw Bash and repo-internal Bash calls remain denied unless they use a
  strict trusted `<absolute-stateful-binary> sandbox run ... --command <cmd>`
  wrapper
- OMP raw Bash and Python/JavaScript/JS/Ruby/Julia eval-tool execution remains
  denied at host approval and hook levels; non-external sandbox runs must use
  `sandbox_bash`
- sandbox-run wrappers that need authorization fail closed and do not run the
  command
- `state.reconcile.ack` fails and cannot clear an unreconciled-human-write block
- reservation declaration, claim acquisition, and claim refresh fail
- non-Bash read, search, and diff actions are allowed
- the IDE save gate warns the user but allows the human save to proceed
- hook and observer events that cannot be sent should be appended to a local
  outbox for later sync

The local outbox is append-only recovery evidence. It may hold heartbeat,
finalization, human activity, and observer events until the server becomes
reachable. It cannot authorize writes, clear reconciliation blocks, or extend
claims while the server is unavailable.

Outbox sync uses `outbox_id` as an idempotency key. Replaying the same outbox
entry must not create duplicate state events. Pending events should be synced in
local creation order per session; failed entries stay in the outbox with failure
metadata for later retry or inspection.

V1 has no cached write grace period. A recent successful claim or reservation check is
not enough to continue writing while the state server is down.

## Known Limits

Agent runtime hooks are a practical guardrail, not a complete enforcement
boundary. They can intercept important supported paths such as shell commands,
`apply_patch`, and MCP calls, but they do not make the state protocol a full
security boundary. Post-action hooks also cannot undo side effects that already
happened.

For OMP, yolo metadata does not downgrade a stateful denial to a warning; deny
and unavailable-state decisions remain hard blocks.

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

Target shared policy primitives should still live in `stateful_core` so the fork
stays small and easier to rebase. The shipped prototype keeps store-backed
authorization orchestration in `stateful-server::policy_service` and uses
`stateful_core` for protocol types and pure scope-policy primitives.

## Initial Decision

Build the first version with Codex lifecycle hooks, OMP extension hooks, MCP
tools, and a `stateful_core` state server.

The v1 MVP includes Codex and OMP hooks, MCP tools, the state server, sandboxed
test execution, and explicit reconciliation acknowledgements. Automatic
human-write observation and reconciliation blocks remain target behavior.

V1 is local-only. It coordinates one machine/workspace boundary. Team-shared,
cross-machine, or hosted state sync is deferred to v1.5/v2.

The project should keep the policy model portable across agent runtimes. The
shipped implementation remains Codex-first, with OMP extension support for the
runtime lifecycle events OMP exposes. Runtime-specific behavior belongs in
adapters, and broader non-Codex runtime support remains future work.

V1 defaults to strict enforcement. Supported writes and reconciliation fail
closed when state cannot be trusted. Usability comes from
clear denial messages and diagnostics, not from silent grace periods.

The shipped retention policy prunes event and audit history older than 14 days.
Expired state may be shown as handoff evidence until it leaves the retention
window, but retention does not extend live write authority. Pruning preserves
active current-state rows, pending notifications, and outbox sync evidence.

The v1 hard block policy is:

```text
supported write action + no active reservation -> deny
supported write action + expired reservation -> deny as missing active reservation
supported write action + reservation without file/directory scope -> deny
supported write action + target outside reservation scope -> deny
Codex raw Bash -> deny; OMP raw Bash plus Python/JavaScript/JS/Ruby/Julia
  eval tools -> deny even when the command invokes `stateful sandbox run`; use
  `sandbox_bash` for non-external sandbox profiles, `ext_ro_bash` for read-only
  external work, and `ext_rw_bash` for external writes
delete action + non-exact file scope -> deny
rename/move action + non-exact source or destination scope -> deny
active write claim in hard conflict domain -> deny
```

For wait queue scheduling, the same hard conflict becomes a queued request when
the caller asks to queue on conflict. Queue promotion happens only after
explicit release, session or activity finalization, or claim expiry. Soft
repo-relative conflicts remain warning context in v1.

Explicit overrides and phase-aware authorization are target policy and are not
implemented in the current server. When implemented, an explicit override can
allow a direct write
authorization exception for the current session and resource, but it must not
reorder existing waiters, steal a claimable reservation, or move the overriding session to
the head of a queue.

A v1 task reservation must be active, unexpired, belong to the same
session, and include matching exact file or directory scope. Abstract task,
test, port, or migration reservation can be stored as context but does not permit
writes. Directory scope authorizes `write_directory` for the exact directory
resource; file writes, deletes, renames, and moves require exact file scope.

Resource authorization classes:

```text
file/directory -> write-authorizing
test -> sandboxed test coordination only
task/port/migration -> prompt context and warning only
```
Delete operations require exact file scope. Rename and move operations require
exact file scope for both source and destination.

Reservation freshness defaults:

```text
default reservation TTL: 15 minutes
default claim TTL: 5 minutes
default reservation TTL: 120 seconds
heartbeat extension: shipped for explicit heartbeats and implicit authorize-time
heartbeat events on active unexpired reservation rows
maximum rolling reservation lifetime: 60 minutes from declared_at
target phase gating: blocked is visible but not write-authorizing
target finalization statuses: done/failed/blocked
```

The shipped server does not persist or populate activity phase in store-backed
authorization. Blocked-phase and finalized-session deny reasons are target
policy behavior; current finalization completes active reservations, so later writes
fail because no active task reservation remains.

The shipped hook path records target existence and content hash when an exact
file claim is acquired with `root`, denies hook-originated native file writes
when that file changes before authorization, and releases the same-session claim
after a completed native edit or `write-targets` transaction. This is a
per-claim freshness check, not a filesystem watcher or IDE human-save observer.

Override policy:

```text
automatic override: never allowed
override authority: explicit user instruction only
scope: current session + current turn + specific resource
applies to: active claim conflict only
does not bypass: missing reservation, expired reservation, target blocked/finalized state
does not bypass: file/directory scope matching
does not bypass: delete/rename/move exact-scope rules
example: "Allow override for src/auth.ts."
responsibility: user owns the judgment and risk
```

This override policy is future work until the server has an explicit override
record and authorization path.

Prompt context rendering:

```text
state_context_render(workspace, session_id, resources?, mode)
mode: brief | detailed
sections: Blocking, Required Next Action, Warnings, Nearby Activity, Stale/Expired
```

`brief` is used for session start and user prompt context. `detailed` is used
after denied actions or for focused resource checks. Rendering should be
actionable, not a raw event dump.

The current server route renders store-backed live context from active reservations,
active claims, and queued or claimable (`reserved`) wait records. The response includes
summary counts, structured `items`, and prompt-ready `prompt_text`.

The shipped route returns structured data plus prompt-ready markdown:

```text
status: ok | error
mode: brief | detailed
current
items[]
prompt_text
```

A separate context-level `clear | warn | blocked` status is target model
vocabulary. The current route reports request success or failure in `status` and
leaves block/warn/info semantics on individual items.

Each rendered item uses:

```text
- [severity] resource: summary.
  next: concrete action.
  evidence kind: coordination signal classification.
  evidence: detailed mode only.
```

Severity values are `block`, `warn`, and `info`. `brief` mode has at most 8
bullets. `detailed` mode has at most 20 bullets. Empty sections are omitted
except `Blocking: None` when a resource filter is present. Raw event dumps are
forbidden.

Evidence kind distinguishes declared reservation, claim-only blockers, queue or
reservation state, observed writes, and verified diffs; `evidence` remains
optional supporting detail.

When the block is an unreconciled human write, `Required Next Action` should tell
the agent to reread the file, summarize the human change, decide whether to
adopt, reapply, ask the user, or abandon, and call `state.reconcile.ack`.

The renderer can show supplied expired and finalized state only in
`Stale/Expired`; the shipped store-backed route currently emits live reservations,
claims, and wait records only. Stale or finalized evidence never blocks a live
action by itself.

Target summary windows for future historical context:

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
