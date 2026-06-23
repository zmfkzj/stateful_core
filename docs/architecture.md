# Architecture

`stateful_core` is a coordination layer between agent runtimes and a shared
current-state server.

## MVP Shape

The first implementation target is:

```text
Codex lifecycle hooks and OMP extension hooks
+ MCP tools
+ state server
```

Codex and OMP hooks observe and gate important agent actions. MCP tools give agents a
structured way to read and update coordination state. The state server owns
policy, persistence, TTLs, and conflict checks.

The v1 MVP includes sandboxed test execution through
`sandbox run --fs build --network enabled --write-dir <scratch-purpose>`, which
writes disposable artifacts under `/tmp/stateful/<session>/<scratch-purpose>/`,
plus explicit reconciliation acknowledgements through
`state.reconcile.ack`. Automatic human-write observation and reconciliation
blocks remain target behavior.

V1 is local-only. It coordinates Codex and OMP sessions and agents inside one
machine/workspace boundary. Subagent-specific lifecycle attribution, local human
activity signals, and IDE integrations are future observer or adapter work.
Team-shared, cross-machine, or hosted synchronization is a later v1.5/v2
concern.

The product direction is agent-runtime portability, but the shipped prototype is
Codex-first with OMP extension support. The state server and policy API should
not add new Codex-only assumptions beyond adapter metadata, and broader
non-Codex runtimes remain future integration work.

Implementation defaults for API shape, SQLite tables, hook adapter behavior,
runtime files, CLI commands, and tests are fixed in
[Implementation Contract](implementation-contract.md). Product policy belongs in
this architecture document and the state model; implementation choices may
evolve only when they preserve those policies.

## Layers

```text
Agent hooks and external observers
-> state server API
-> append-only coordination event log
-> active current-state materializer
-> conflict policy
-> prompt context renderer
-> MCP tools for agent access
```

The event log is used for audit evidence and event-backed replay of accepted
session and intent declaration events. The materialized current-state view is
the active coordination source for fast reads, conflict checks, and prompt
rendering.

## Persistence

V1 uses SQLite for local persistence.

The database should contain:

- append-only coordination events
- materialized current-state tables
- conflict decision records
- local outbox sync records

JSONL alone is not the v1 persistence format because conflict checks, TTL
expiration, outbox replay, and audit queries need indexed reads. The event log
remains append-only inside SQLite so shipped coordination events can be
inspected and event-backed state can be replayed where supported.

The shipped retention policy prunes event and audit history older than 14 days
from the state server maintenance loop. This keeps handoff evidence available
without turning current-state coordination into long-term memory. Pruning covers
old events, reconciliations, conflicts, human observations, and expired
notifications; it does not prune active current-state rows or outbox sync
evidence.

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
confirmation. All three generated `*_bash` tools start sandbox commands in the
background, immediately return a background-job start result, capture
stdout/stderr in memory, and deliver final status/output automatically through
OMP; their `async` input is a deprecated compatibility no-op that does not
select execution mode. `sandbox_bash` rejects
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
share the Codex lifecycle model. The isolated OMP `stateful` profile uses OMP
extension entry points and does not expose `UserPromptSubmit`. Its
`session-start` hook prefers `event.sessionId` or
`ctx.sessionManager.session.id`, stores that id in `STATEFUL_SESSION_ID`, and
persists current-session files so session-aware MCP tools resolve the same OMP
session. Later managed Codex hooks should move the same thin hook adapters to
administrator-controlled paths and configure them from `requirements.toml`.

Plugin packaging is a team-beta distribution layer, not the prototype
enforcement path. A plugin can bundle hooks, MCP config, skills, and docs, but
plugin hooks remain non-managed and require user trust. Managed hooks remain the
long-term path for organization-level enforcement.

Repo-local hook scripts must stay thin at the integration boundary:

```text
parse runtime hook input
classify tool call and extract action/targets when supported
call local HTTP state server for store-backed coordination policy
deny unclassified write/execute-capable tools in enabled repos
translate decision into runtime hook output
append local outbox event when observation cannot reach the server
```

They must not own store-backed coordination policy. In v1, these hook adapters
are Rust commands from the compiled `stateful` binary and include fail-closed
tool classification plus Bash sandbox-wrapper validation. If the state server
is unavailable, the hook follows the availability policy: agent writes and
reconciliation fail closed; read/search/diff remains allowed.

For OMP, the generated `sandbox_bash` tool owns non-external sandbox command
execution for read-only, write-targets, build, git, and github-pr profiles;
`ext_ro_bash` owns read-only external commands without OMP UI confirmation; and
`ext_rw_bash` owns external writes with OMP UI confirmation. Each generated tool
always starts the sandbox command in the background, returns only the
background-job start result immediately, captures stdout/stderr in memory, and
relies on automatic OMP completion delivery for the final status/output. Other
stateful allows translate to OMP allow. Stateful deny or unavailable server
translates to a hard block, even when OMP yolo metadata is present.

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
- OMP `SessionStart`
- OMP `PreToolUse`
- OMP `PostToolUse`
- OMP `Stop`
- MCP calls from the agent
- CLI and state server calls

Target and future trigger sources:

- Native Codex/OMP subagent start/stop and tool activity with separate attribution
- git working tree or filesystem observation for conservative human activity
  detection
- IDE extension events for human file open, dirty, save-attempt, and save
  completion signals

Each trigger should carry the session id, actor identity when known, workspace,
branch, timestamp, and source reference.

When subagent trigger sources are implemented, subagents inherit write
authorization only from the parent session's active valid intent scope. They
still record activity and leases with their own `actor_id` so attribution stays
precise.

## Hook Responsibilities

These responsibilities apply to Codex hooks unless noted. OMP supports
`SessionStart`, `PreToolUse`, `PostToolUse`, and `Stop`; it does not expose a
`UserPromptSubmit` hook.

`SessionStart`:

- register or resume the session
- record workspace and branch
- render active neighboring state into model context

`UserPromptSubmit`:

- capture the user's requested goal as an initial intent candidate
- attach relevant active conflicts as context

`PreToolUse`:

- deny supported write calls when the session has no active intent
- deny Codex raw Bash with sandbox guidance. For OMP, raw Bash and the
  Python/JavaScript/JS/Ruby/Julia eval tools are denied at host approval and hook
  levels, even when the raw command invokes `stateful sandbox run`; non-external
  sandbox command work must use `sandbox_bash`, read-only repo-external shell
  work must use `ext_ro_bash` without OMP UI confirmation, and external writes
  must use `ext_rw_bash` with write/create/dir scope and OMP UI confirmation.
  Hook-mediated command execution outside OMP custom tools must be a single
  strict invocation of the trusted absolute `stateful` binary running
  `<absolute-stateful-binary> sandbox run ... --command <cmd>`. Read-only
  command-shaped inspection uses `--fs read-only --network disabled`; process
  inspection uses `sandbox process find <selector>`. Command-shaped repo writes
  use `--fs write-targets` with explicit `--write-target <file>` /
  `--create-target <file>` values and repo intent plus same-session leases. Local
  Git uses `--fs git --network disabled`, GitHub PR operations use
  `--fs github-pr --network enabled`, and external operations use `--fs external`
  with Codex approval, OMP `ext_ro_bash` for read-only/no-write-scope commands,
  or OMP `ext_rw_bash` for writes. A purpose and command are sufficient for
  read-only/no-declared-scope external operations; absolute external targets
  remain required when declaring external write scope. On macOS, external runs
  allow `trustd` and DirectoryService Mach lookups for TLS certificate
  verification by Go tools.
- check leases and planned edits for likely conflicts
- return allow, warning context, or deny based on policy

`PostToolUse`:

- observe files, commands, and results from supported tool calls
- release same-session repo-write leases after completed native edit and
  `write-targets` transactions
- refresh heartbeat timestamps and lease TTLs only for remaining active leases
  still covered by active intent
- update phase, touched resources, and last result

`Stop`:

- post activity finalization for the session
- release the session's leases through finalization
- leave explicit `state_activity_finalize` available for manual final status
  updates before shutdown

## MCP Surface

The v1 MCP/tool surface is intentionally narrow. Canonical callable tool names
map to dotted protocol names:

```text
state_session_register (state.session.register)
state_session_heartbeat (state.session.heartbeat)
state_intent_declare (state.intent.declare)
state_intent_request (state.intent.request)
state_intent_claim (state.intent.claim)
state_intent_cancel (state.intent.cancel)
state_lease_acquire (state.lease.acquire)
state_lease_release (state.lease.release)
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

Hooks and MCP tools should call the same state server API. Policy must live in
the state server, not in duplicated hook scripts. Native edit tools with
hook-visible targets are the repo file edit path after exact intent declaration
and a successful file lease; command-shaped shell writes remain outside MCP and
go through the sandbox-run wrapper.

Intent declare and request payloads require a non-empty `purpose`; clients infer
it from the user or agent instruction and send it explicitly. Intent declare
also requires non-empty `files_planned`; empty arrays and empty or normalized-empty
paths fail with `missing_scope`. Intent request also requires a non-empty
`path`; empty or normalized-empty request paths fail with `missing_scope`.

## Tool Classification

V1 enforcement is strict about write target extraction:
Runtime adapters normalize namespaced tool names to their leaf before
classification, so `functions.bash` is Bash,
`functions.python` / `functions.javascript` / `functions.js` /
`functions.ruby` / `functions.julia` are eval tools, and `functions.read` /
`functions.search` remain native read/search tools.

- Native edit tools such as Codex `apply_patch`, `Edit`, and `Write` or OMP
  `edit` and `write`: enforce by inspecting hook-exposed targets after exact
  intent declaration and a successful same-session file lease. The completed
  write transaction releases the lease that authorized it.
- Command execution: Codex raw Bash is denied with sandbox guidance. OMP raw Bash
  and Python/JavaScript/JS/Ruby/Julia eval-tool execution are denied at host
  approval and hook levels, even when the raw command itself invokes
  `stateful sandbox run`. OMP command-shaped sandbox work uses generated tools:
  `sandbox_bash` for read-only, write-targets, build, git, and github-pr
  profiles, `ext_ro_bash` for read-only `--fs external` commands without OMP UI
  confirmation, and `ext_rw_bash` for external writes with OMP UI confirmation.
  These tools always run sandbox commands in the background, return only a
  background-job start result immediately, capture stdout/stderr in memory, and
  deliver final status/output automatically through OMP.
  Ordinary read work should use native read/search/diff tools when available.
  Read-only command-shaped inspection that genuinely needs a shell uses
  `--fs read-only --network disabled`; process inspection uses
  `sandbox process find <selector>`, not raw `ps` or `pgrep`. Command-shaped
  repo writes use `--fs write-targets` with explicit `--write-target <file>` /
  `--create-target <file>` values and target authorization. External operations
  use `--fs external` with no repo intent or lease, and Codex approval or OMP
  `ext_ro_bash` for read-only/no-write-scope commands; OMP external writes use
  `ext_rw_bash` with at least one write target, create target, or write dir. On
  macOS, the external profile permits `trustd` and DirectoryService Mach lookups
  so Go TLS clients such as `gh` can verify certificates.
- Test execution: run only through sandboxed test actions such as
  `stateful sandbox run --fs build --network enabled --write-dir <scratch-purpose> --command <cmd>`.
  Build artifacts live under `/tmp/stateful/<session>/<scratch-purpose>/`.
- Bash command text alone never authorizes repo-internal tool use, even when it
  appears read-only.

Denied Bash should direct the agent to native read/search tools for ordinary
read work, native edit tools for repo file edits, strict sandbox-run wrappers
for Codex command-shaped shell execution, `sandbox_bash` for OMP non-external
sandbox runs, `ext_ro_bash` for OMP read-only external runs, `ext_rw_bash` for
OMP external writes, and build-profile sandbox wrappers for tests.

MCP does not perform local command-shaped file writes. Hook-mediated shell
execution uses `<absolute-stateful-binary> sandbox run ... --command <cmd>`;
plain CLI-context usage outside hooks can use `stateful sandbox run`. The MVP
ships `read-only`, `write-targets`, `build`, `git`, `external`, and `github-pr`
profiles; `workspace` profiles are deferred and fail closed. `/dev/null` is
writable in every sandbox profile so common shell and Git behavior works.
Command text alone does not
authorize `rg`, `git diff`, test runners, stateful operational commands, or any
other Bash command.

## Sandboxed Tests

Agents cannot run raw Bash or Python/JavaScript/JS/Ruby/Julia eval-tool test
commands through hooks. They call the trusted build-profile wrapper with a
scratch purpose; in OMP they call the generated `sandbox_bash` tool for this
build profile:

```text
stateful sandbox run --fs build --network enabled --write-dir test-run --command <cmd>
```

The wrapper creates disposable scratch under
`/tmp/stateful/<session>/test-run/` and the OS sandbox limits build/test writes
to that external artifact tree. Source-tree writes remain outside the allowed
surface unless exact targets are declared and authorized.

The macOS Seatbelt backend is the release-verified first-class backend. Linux
bubblewrap support is implemented but experimental until it is verified in a
Linux release environment.

## State Server

The state server is responsible for:

- appending coordination events
- materializing active current state
- running background and lazy expiration for stale activity, intent, leases, and
  reservations
- extending active intent TTL from explicit heartbeats and authorize-time
  implicit heartbeat events within a 60-minute rolling maximum
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
decision: allow | deny | error
reason
conflicts[]
required_next_action
context_items[]
audit_event
```

`warn`, `conflicts[]`, `context_items[]`, and `audit_event` are target response
vocabulary. The shipped `/v1/authorize` path returns allow/deny/error with
optional `wait` or `reservation` details and appends `AuthorizationDenied` events
for deny decisions.

The target policy engine owns:

- active intent checks
- file and directory scope checks
- lease conflict checks
- collision-domain evaluation
- human-write reconciliation checks
- state-server availability behavior

Hooks and adapters classify runtime-specific tool calls, extract tool intent and
targets when supported, and call the policy API for store-backed coordination
decisions. Adapter-local policy is limited to fail-closed classification and
trusted wrapper validation for command-shaped execution. In the shipped
prototype, `stateful-core` owns pure scope-policy primitives and
`stateful-server::policy_service` owns store-backed authorization orchestration.

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

V1 does not require the IDE extension. Target human-write reconciliation is
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
- run sandboxed tests with an external `/tmp/stateful/<session>/<scratch-purpose>/`
  artifact tree

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
- expired intent or intent beyond its 60-minute rolling window: deny as missing
  active intent
- blocked phase or finalized session before supported write action: target
  phase-aware deny behavior
- directory intent and directory lease authorize only `write_directory` for the
  exact directory resource; they do not authorize `write_file`, delete, rename,
  or move actions on child paths
- `write_directory` requires exact directory intent, and a matching directory
  lease fences the whole subtree because command-shaped `--write-dir` execution
  can write anywhere below that directory
- delete operation without exact file scope: deny
- rename or move without exact file scope for both source and destination: deny
- active lease in the hard conflict domain by another actor: deny unless the
  current session contains an explicit user override for that resource
- same `workspace_id` and same normalized `relative_path`: shipped hard conflict
  domain for active lease and wait-queue checks
- same normalized `absolute_path`: target physical-file hard conflict domain
- planned edit in the same area: target warning context
- same `repo_id` and same `relative_path` across different workspace, worktree,
  or branch: target soft repo-relative warning
- unknown repository identity: target behavior is to hard-block only on same
  normalized absolute path and render weaker matches as unknown-confidence
  context
- expired lease: allow and surface stale context
- human working tree change near the target: warn before edits
- human save observed after an agent lease or write: deny further agent writes
  until the agent acknowledges reconciliation or receives an explicit user
  instruction
- current shipped hook path records file target observations on exact file lease
  acquire, denies hook-originated writes when the leased file changes before
  authorization, and refreshes that observation after same-session supported file
  tools complete
- unrelated reads and searches: allow
- reads, searches, diffs, and sandboxed tests after human writes: allow
- tests: allow only through trusted sandbox-run wrappers with authorized targets
- task, port, or migration resource conflict: warn or info only in v1

Conflict decisions must be auditable. Overrides are never automatic. They are
valid only when the user explicitly instructs the current session to allow a
specific resource override, for example: "Allow override for `src/auth.ts`."
The user owns the judgment and responsibility for that exception.

Overrides apply only to active lease conflicts. They do not bypass missing
intent, expired intent, target blocked/finalized state, file or directory scope
matching, delete exact-scope rules, or rename/move exact-scope rules. Overrides
are scoped to the current session, current turn, and specific resource when the
target override policy is implemented.

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

`state_context_render` supports `brief` and `detailed` modes plus an optional
resource filter. `brief` is for session start and prompt submit context.
`detailed` is for denied actions or focused resource checks. Rendered output
must include concrete next actions when a block or warning is present.

The current server route renders store-backed live context from active intents,
active leases, and queued or reserved wait records. The response includes
summary counts, structured `items`, and prompt-ready `prompt_text`.
Released leases are absent from this live render. A later same-session write
must acquire a fresh lease or claim an eligible reservation before authorization
can succeed.

The shipped `/v1/context/render` route returns structured data and prompt-ready
markdown:

```text
status: ok | error
mode: brief | detailed
current
items[]
prompt_text
```

A separate context-package status enum such as `clear | warn | blocked` is target
model vocabulary. The current route exposes block/warn/info state per item and
uses the top-level `status` field for request success or failure.

Prompt items must use this shape:

```text
- [severity] resource: summary.
  next: concrete action.
  evidence kind: detailed mode only.
  evidence: detailed mode only.
```

Severity values are `block`, `warn`, and `info`. `brief` output is limited to 8
bullets total. `detailed` output is limited to 20 bullets total. Empty sections
are omitted except `Blocking: None` when a resource filter is present.

Evidence kind distinguishes declared intent, lease-only blockers, queue or
reservation state, observed writes, and verified diffs; `evidence` remains
optional supporting detail.

Block and warning items must include `next:`. `Required Next Action` appears
immediately after `Blocking` so the agent sees the recovery path before nearby
context.

The renderer can place stale or expired items supplied by callers under
`Stale/Expired`, but the current store-backed route does not emit historical
stale/finalized items. Target historical context windows for `Stale/Expired` are
informational only and can never be the sole reason for a live denial.

## Failure Modes

The system should prefer explicit uncertainty:

- missing heartbeat -> expire or mark unknown, not success
- interrupted session -> keep last state until TTL expires
- hook failure -> warn and fail closed only for high-risk writes
- OMP stateful hook deny or unavailable result -> block, never warn because of
  yolo metadata; repo-external command-shaped work must still use `ext_ro_bash`
  for reads or `ext_rw_bash` for writes, not raw Bash or eval tools
- state server unavailable -> deny supported writes that cannot prove active
  intent
- state server unavailable -> deny Codex raw Bash, repo-internal Bash, and all
  OMP raw Bash plus Python/JavaScript/JS/Ruby/Julia eval-tool execution. OMP
  non-external sandbox runs must use `sandbox_bash`, and command-shaped writes
  through `--fs write-targets` fail
  closed when target authorization cannot be proven.
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
