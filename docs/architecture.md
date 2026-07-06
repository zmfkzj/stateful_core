# Architecture

`stateful_core` is a coordination layer between agent runtimes and a shared
current-state server.

## MVP Shape

The first implementation target is:

```text
Codex lifecycle hooks and OMP extension hooks
+ native Stateful coordination tools
+ state server
```

Codex and OMP hooks observe and gate important agent actions. Native Stateful
tools give agents a structured way to read and update coordination state. The
state server owns policy, persistence, TTLs, and conflict checks.

The v1 MVP includes sandboxed test execution through
`sandbox run --fs build --network enabled --write-dir <scratch-purpose>`, which
writes disposable artifacts under `/tmp/stateful/<session>/<scratch-purpose>/`,
plus two server coordination modes:

```text
stateful server --coordination-mode enforcement|awareness
```

`enforcement` is the default shipped guardrail. `awareness` keeps presence and
context rendering, but softens reservation/scope/claim/phase coordination denials
into warnings so the presence-first path can be measured without queue/claim
blocking. Freshness, human-write, unsafe-command, and write-fence denials remain
hard when those checks apply.

Human coordination is implemented through `human observe`, `human save-check`,
and `reconcile ack`, with an advisory VS Code save gate under
`integrations/vscode`. High-confidence human save/change/delete observations
block later agent writes to those paths until the agent rereads and acknowledges
reconciliation.

V1 is local-only. It coordinates Codex and OMP sessions and agents inside one
machine/workspace boundary. Native subagent lifecycle attribution, richer human
observer coverage, and hosted/team-shared synchronization remain later work.

The product center is presence, freshness, and handoff
([ADR 0002](adr/0002-presence-first-not-lock-first.md)). The shipped target
shape is presence plus mechanical freshness plus thin write fences. Supported
file-changing writes send base observations, stale observations deny, and
same-file in-flight writes acquire short write fences. Enforcement mode still
requires active reservation scope plus same-reservation claims; awareness mode is
the shipped comparison mode for demoting that broad guardrail to warnings while
retaining hard safety denials when the corresponding checks apply.
Agent-runtime portability remains a standing requirement because a shared
presence layer matters most across heterogeneous runtimes. The shipped prototype
is Codex-first with OMP extension support. The
state server and policy API should not add new Codex-only assumptions beyond
adapter metadata, and broader non-Codex runtimes remain future integration work.

Implementation defaults for API shape, SQLite tables, hook adapter behavior,
runtime files, CLI commands, and tests are fixed in
[Implementation Contract](implementation-contract.md). Product policy belongs in
this architecture document and the state model; implementation choices may
evolve only when they preserve those policies.

## Layers

```text
Agent hooks, native tools, and external observers
-> state server API
-> append-only coordination event log
-> active current-state materializer
-> conflict policy
-> prompt context renderer
-> native tools for agent access
```

The event log is used for audit evidence and event-backed replay of accepted
session and reservation declaration events. The materialized current-state view is
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

The shipped retention and freshness defaults are summarized canonically in
[Current-State Coordination](current-state-coordination.md#canonical-freshness-defaults). The
state server prunes old event history and expired or delivered notifications; it
does not prune active current-state rows or outbox sync evidence.

The v1 runtime database lives under user-level `GlobalPaths` in global server
mode: `$STATEFUL_HOME/state.db`, or `$HOME/.stateful_core/state.db` when
`STATEFUL_HOME` is unset. Repo-local `.stateful_core/state.db` remains a
compatibility/runtime fallback. Committed repo configuration stays under
`.stateful/`, while runtime state must not be committed.

## API Transport

The state server exposes a local HTTP API. Hooks, native Stateful tools,
filesystem watchers, and the advisory IDE extension all call the same HTTP API.

Agent-facing tools are adapters over that API, not policy owners:

```text
Codex hook script -> local HTTP API -> state server
native Stateful tool -> local HTTP API -> state server
IDE extension -> local HTTP API -> state server
watcher -> local HTTP API -> state server
```

This keeps policy centralized and avoids duplicating authorization logic inside
hook scripts or native tool handlers.

The concrete v1 HTTP API lives under `/v1` and is defined in the implementation
contract. The target envelope includes `protocol_version`, agent identity,
workspace identity, and source metadata. The current implementation enforces
that envelope for write authorization and reservation declare/request/claim/cancel;
other POST routes still use flat request bodies.

## Hook Packaging

The prototype supports user-level installation with repo allowlist gating.
`stateful install --agent codex --yes` configures global Codex hooks and writes
`skills/stateful-command-policy/` (`SKILL.md`, `omp-tools.md`,
`sandbox-tools.md`, `denial-recovery.md`, `subagent-write-recovery.md`) and
`skills/dispatching-parallel-agents/SKILL.md`.
For OMP, `stateful install --agent omp --yes` writes OMP config containing the
stateful extension under the OMP `stateful` profile agent directory
(`~/.omp/profiles/stateful/agent`), or under `~/.omp/profiles/<name>/agent`
when `--profile <name>` is supplied, and ensures the target keys
`tools.approvalMode: yolo`, `stateful.autoApprove: true`,
`bash.enabled: true`, `eval.py: false`, `eval.js: false`, `eval.rb: false`,
and `eval.jl: false`. The installer removes `tools.approval` from the stateful
profile because yolo mode delegates safety to
Stateful hooks. Without `--update`, existing scalar values are preserved and
only missing keys are inserted; with `--update`, existing target scalar values
are overwritten. Raw Bash plus the Python/JavaScript/JS/Ruby/Julia eval
tools are denied at the host approval and hook levels. The installer also writes
`rules/stateful-required.md` and `skills/stateful-command-policy/` (`SKILL.md`,
`omp-tools.md`, `sandbox-tools.md`, `denial-recovery.md`,
`subagent-write-recovery.md`) under that isolated agent directory: the
always-apply rule tells the model when
Stateful policy applies, the `stateful-command-policy` manual keeps the detailed
procedure, and hooks remain the enforcement boundary. The
generated extension keeps built-in Bash preflight plus lazy edit/write resume
tools. Built-in Bash may run only strict trusted `stateful sandbox run ...` and
`stateful sandbox process find ...` commands after Stateful preflight; command
execution and process inspection are not generated tool calls. External
write/create/write-dir/socket/signal scope and repo-external OMP native
`edit`/`write` file targets auto-approve the scoped Stateful-owned OMP grant
prompt by default through `stateful.autoApprove: true`, while sandbox scope
validation, hooks, reservation/claim checks, and grant limits still apply. Set
`stateful.autoApprove: false` to require the prompt. The grant prompt shows
purpose, declared scope, network mode, examples, max uses, and expiry instead
of raw command text, and matching calls can reuse it until the limit is reached.
Raw Bash and Python/JavaScript/JS/Ruby/Julia
eval-tool calls
are blocked even if their command text
invokes `stateful sandbox run`. The OMP
global/default profile is not modified.
`stateful enable` opts the current repo into enforcement through the user-level
install and repo allowlist.

```text
global Codex hooks and generated skills
isolated OMP `stateful` profile config
repo allowlist entry
optional <repo>/.codex/hooks.json compatibility fallback
<repo>/.stateful/config.yml
stateful binary available by absolute path or PATH lookup
```

Codex global hooks, repo-local compatibility hooks, and managed Codex hooks
share the Codex lifecycle model. The isolated OMP `stateful` profile uses OMP
extension entry points and does not expose `UserPromptSubmit`. Its
`session-start` hook resolves OMP runtime identity as specified in
[Hooks and identity](usage-reference.md#hooks-and-identity). Without valid
runtime identity, Stateful actions fail closed. Native tools receive the
extension-provided identity, and state operations scope conflicts by
`workspace_id`. Later managed
Codex hooks should move the same
thin hook adapters to administrator-controlled paths and configure them from
`requirements.toml`.

Plugin packaging is a team-beta distribution layer, not the prototype
enforcement path. A plugin can bundle hooks, native-tool guidance, skills, and
docs, but plugin hooks remain non-managed and require user trust. Managed hooks
remain the long-term path for organization-level enforcement.

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

Connection attempts to stale or unreachable runtime endpoints are bounded; they
fail under that same availability policy rather than hanging hook startup.

For OMP, built-in Bash owns sandbox command execution and process inspection
only when the command is a single trusted `stateful sandbox run ...` or
`stateful sandbox process find ...` invocation that passes Stateful preflight.
External write/create/write-dir/socket/signal scope and repo-external OMP native
`edit`/`write` file targets auto-approve the scoped Stateful-owned OMP grant
prompt by default through `stateful.autoApprove: true`, while sandbox scope
validation, hooks, reservation/claim checks, and grant limits still apply. Set
`stateful.autoApprove: false` to require the prompt. The grant prompt shows
purpose, declared scope, examples, max uses, and expiry rather than raw command
text, and matching calls reuse the grant until it expires or reaches its use
limit. When auto-approval is enabled, no prompt is shown.
Other stateful allows translate to OMP allow. After OMP runtime identity and
discovery succeed, Stateful authorization failures can return structured block
JSON. If runtime identity/discovery subprocesses fail before that point, the OMP
extension maps the non-zero failure to a hard block. OMP yolo metadata never
turns those blocks into warnings.

Hook scripts should resolve paths from the git root. Envelope-enforced routes
include `protocol_version`; a major protocol mismatch fails closed on those
write authorization and reservation paths.

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
- Native Stateful tool calls from the agent
- CLI and state server calls
- CLI/watch human observation through `/v1/human/observe`
- VS Code low-confidence open/presence and dirty observations through
  `/v1/human/observe`
- VS Code high-confidence save observations through `/v1/human/observe`
- CLI and VS Code human save checks through `/v1/human/save-check`
- Reconciliation acknowledgements through `/v1/reconcile/ack`

Target and future trigger sources:

- Native Codex/OMP subagent start/stop and tool activity with separate attribution
- selected-file IDE telemetry and broader arbitrary filesystem/editor watching

Each trigger should carry the active agent id, actor identity when known,
workspace, branch, timestamp, and source reference.

When subagent trigger sources are implemented, subagents inherit write
authorization only from the parent agent's active valid reservation scope. They
still record activity and claims with their own `actor_id` so attribution stays
precise.

## Hook Responsibilities

These responsibilities apply to Codex hooks unless noted. OMP supports
`SessionStart`, `PreToolUse`, `PostToolUse`, and `Stop`; it does not expose a
`UserPromptSubmit` hook.

`SessionStart`:

- register the active agent
- record agent id, workspace, and branch
- render active neighboring state into model context

`UserPromptSubmit`:

- capture the user's requested goal as an initial reservation candidate
- attach relevant active conflicts as context

`PreToolUse`:

- in enforcement mode, deny supported write calls when the active agent has no
  active reservation scope and same-reservation claim
- in awareness mode, convert reservation/scope/claim coordination denials into
  warnings while keeping later safety denials hard when those checks apply
- deny file-changing calls with stale base observations, or with missing base
  observations when the adapter can identify and read the target
- deny overlapping same-file in-flight write fences
- deny writes to unreconciled high-confidence human save/change/delete observations
- deny Codex raw Bash with sandbox guidance. For OMP, built-in Bash may run
  only strict trusted `stateful sandbox run ...` and `stateful sandbox process
  find ...` commands after Stateful preflight; arbitrary raw Bash and the
  Python/JavaScript/JS/Ruby/Julia eval tools remain denied at host approval and
  hook levels. Scoped external writes and repo-external OMP native `edit`/`write`
  file targets auto-approve the scoped Stateful-owned OMP grant prompt by default
  through `stateful.autoApprove: true`, while sandbox scope validation, hooks,
  reservation/claim checks, and grant limits still apply. Set
  `stateful.autoApprove: false` to require the prompt.
  Hook-mediated command execution outside OMP built-in Bash must be a single
  strict invocation of the trusted absolute `stateful` binary. Read-only
  command-shaped inspection uses `--fs read-only --network disabled`; Codex
  process inspection uses `<absolute-stateful-binary> sandbox process find <selector>`. Command-shaped repo
  writes use `--fs write-targets` with explicit `--write-target <file>` /
  `--create-target <file>` values and repo reservation plus same-reservation claims. Local
  Git uses `--fs git --network disabled`, GitHub PR operations use
  `--fs github-pr --network enabled`, and external operations use `--fs external`
  through the trusted stateful command path. A purpose and command are sufficient for
  read-only/no-declared-scope external operations; repo-relative exact file scopes
  can auto-declare/claim, while repo-relative external directories still require
  explicit reservation and same-reservation claims. On macOS, external runs
  allow `trustd` and DirectoryService Mach lookups for TLS certificate
  verification by Go tools.
- check claims and planned edits for likely conflicts
- return allow, warning context, or deny based on policy

`PostToolUse`:

- observe files, commands, and results from supported tool calls
- release same-reservation repo-write claims after completed native edit and
  `write-targets` transactions
- refresh heartbeat timestamps and claim TTLs only for remaining active claims
  still covered by active reservation
- update phase, touched resources, and last result

`Stop`:

- post activity finalization for the agent
- release the agent's claims through finalization
- leave explicit `state_activity_finalize` available for manual final status
  updates before shutdown

## Native Stateful Tool Surface

The v1 native tool surface is intentionally narrow. Agent identity tools use
native names directly; other canonical callable tool names map to dotted
protocol names:

```text
state_session_register
state_session_heartbeat
state_reservation_declare (state.reservation.declare)
state_reservation_request (state.reservation.request)
state_reservation_claim (state.reservation.claim)
state_reservation_cancel (state.reservation.cancel)
state_claim_acquire (state.claim.acquire)
state_claim_release (state.claim.release)
state_activity_finalize (state.activity.finalize)
state_current_read (state.current.read)
state_events_read (state.events.read)
state_context_render (state.context.render)
state_reconcile_ack (state.reconcile.ack; some runtimes expose underscore or
stateful-prefixed aliases)
state_notifications_poll (state.notifications.poll)
state_resume_next (state.resume.next)
```

Hooks and native Stateful tools should call the same state server API. Policy
must live in the state server, not in duplicated hook scripts. Native edit tools
with hook-visible targets are the repo file edit path after task-level
reservation covers the target and a successful file claim is active;
command-shaped shell writes go through the sandbox-run wrapper.

Reservation declare and request payloads require a non-empty `purpose`; clients infer
it from the user or agent instruction and send it explicitly. Reservation declare
also requires non-empty `files_planned`; empty arrays and empty or normalized-empty
paths fail with `missing_scope`. Reservation request also requires a non-empty
`path`; empty or normalized-empty request paths fail with `missing_scope`.

Claim acquisition accepts `paths: string[]` so native tool clients can claim a
declared batch in one request. Each path still creates an exact file or directory
claim; directory claims do not authorize child file actions. The server still
accepts legacy claim-acquire requests with `path` for compatibility, and
`state_claim_release` remains single-resource with `path`.

## Tool Classification

V1 enforcement is strict about write target extraction:
Runtime adapters normalize namespaced tool names to their leaf before
classification, so `functions.bash` is Bash,
`functions.python` / `functions.javascript` / `functions.js` /
`functions.ruby` / `functions.julia` are eval tools, and `functions.read` /
`functions.search` / `functions.goal` remain native non-writing tools.

- Native edit tools such as Codex `apply_patch`, `Edit`, and `Write` or OMP
  `edit` and `write`: enforce by inspecting hook-exposed targets. OMP `edit`
  and `write` auto-declare/claim the exact tool-visible file scope for the
  default simple-write path when no explicit reservation id is supplied and the
  only denial is missing reservation/scope. Other native edit flows require the
  target to be covered by a task-level reservation and active same-reservation
  claim. The completed write transaction releases the claim that authorized it.
- Command execution: Codex raw Bash is denied with sandbox guidance. OMP built-in
  Bash may run only strict trusted `stateful sandbox run ...` and
  `stateful sandbox process find ...` commands after Stateful preflight;
  arbitrary raw Bash and Python/JavaScript/JS/Ruby/Julia eval-tool execution
  are denied at host approval and hook levels. External write/create/write-dir,
  socket, or signal scope and repo-external OMP native `edit`/`write` file
  targets auto-approve the scoped Stateful-owned OMP grant prompt by default
  through `stateful.autoApprove: true`, while sandbox scope validation, hooks,
  reservation/claim checks, and grant limits still apply.
  Ordinary read work should use native read/search/diff tools when available.
  Read-only command-shaped inspection that genuinely needs a shell uses
  `--fs read-only --network disabled`; process inspection uses
  `<absolute-stateful-binary> sandbox process find <selector>` in Codex and a
  strict trusted `stateful sandbox process find ...` command through built-in
  Bash in OMP. Command-shaped repo writes use `--fs write-targets` with explicit
  `--write-target <file>` / `--create-target <file>` values and target
  authorization. External operations use `--fs external`; exact repo-relative
  external file targets auto-declare/claim after missing-reservation/scope
  denials, while repo-relative external directories still require explicit
  reservation and same-reservation claims. OMP external write/create/write-dir/socket/signal scope asks
  for the scoped grant described above.
  On
  macOS, the external profile permits `trustd` and DirectoryService Mach lookups
  so Go TLS clients such as `gh` can verify certificates.
- Test execution: run only through sandboxed test actions such as
  `stateful sandbox run --fs build --network enabled --write-dir <scratch-purpose> --command <cmd>`.
  Build artifacts live under `/tmp/stateful/<session>/<scratch-purpose>/`.
- Bash command text alone never authorizes repo-internal tool use, even when it
  appears read-only.

Denied Bash should direct the agent to native read/search tools for ordinary
read work, native edit tools for repo file edits, strict sandbox-run wrappers
for Codex command-shaped shell execution, OMP built-in Bash for strict trusted
`stateful sandbox run ...` and `stateful sandbox process find ...` commands,
and build-profile sandbox wrappers for tests.

Native Stateful tools do not perform local command-shaped file writes.
Hook-mediated shell execution uses a single strict invocation of the trusted `stateful` binary running `stateful sandbox run ...` with one or more `--command <cmd>` flags. Repeated `--command` values are compiled into one sandbox-internal script; outer Bash wrappers with multiple commands, redirects, pipelines, substitutions, or environment assignments remain denied.
Plain CLI-context usage outside hooks can use `stateful sandbox run`. The MVP
ships `read-only`, `write-targets`, `build`, `git`, `external`, and `github-pr`
profiles; `workspace` profiles are deferred and fail closed. `/dev/null` is
writable in every sandbox profile so common shell and Git behavior works.
Command text alone does not
authorize `rg`, `git diff`, test runners, stateful operational commands, or any
other Bash command.

## Sandboxed Tests

Agents cannot run raw Bash or Python/JavaScript/JS/Ruby/Julia eval-tool test
commands through hooks. They call the trusted build-profile wrapper with a
scratch purpose; in OMP they run the same strict trusted `stateful sandbox run`
command through built-in Bash:

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
- running background and lazy expiration for stale activity, reservation, claims,
  claimable reservations, write fences, and human observations
- extending active reservation TTL from explicit heartbeats and authorize-time
  implicit heartbeat events within the canonical rolling maximum
- evaluating conflict policy
- promoting FIFO wait queue requests into claimable reservations after explicit
  claim release, session/activity finalization, or claim expiry, and emitting
  notification payloads that carry the stored reservation purpose with monotonic
  per-target-agent/workspace sequence ids for poll and SSE replay
- requiring reservation claim before creating active reservation scope and active
  claims from that stored purpose in enforcement mode
- acquiring and releasing short same-file write fences around authorized write
  boundaries
- recording human observations, save-check warnings, and reconciliation
  acknowledgements
- rendering concise prompt context
- retaining expired activity as historical evidence

The shipped server supports `enforcement` and `awareness` coordination modes.
Enforcement blocks supported writes without active reservation scope and
same-reservation claims, and it denies blocked or finalized activity phases.
Awareness softens reservation/scope/claim/phase coordination denials to warnings.
Stale edits, unreconciled human writes, same-file write fence conflicts, git
index/stage/commit serialization, destructive operations, and unsafe raw
commands remain hard enforcement in both modes. V1 does not treat claims as hard
distributed locks beyond those write-boundary guardrails.

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

`warn` is shipped for awareness-mode coordination denials. The shipped
`/v1/authorize` path returns allow/warn/deny/error with optional `wait` or
`reservation` details, appends `AuthorizationDenied` events for deny decisions,
and appends warning audit events for awareness-mode warnings.

The shipped policy engine owns:

- active reservation checks
- file and directory scope checks
- claim conflict checks
- base-observation freshness checks where adapters can identify and read the
  target
- same-file write-fence acquisition and conflict checks
- unreconciled human-write checks
- collision-domain evaluation
- recorded reconciliation acknowledgement checks
- state-server availability behavior

The policy direction narrows broad reservation blocking toward rendered presence
plus freshness plus thin fences. Shipped awareness mode is the comparison rail:
reservation/scope/claim/phase denials become warnings, while freshness, human-write,
write-fence, destructive-operation, and unsafe-command denials remain hard when
those checks apply.

Hooks and adapters classify runtime-specific tool calls, extract tool reservation and
targets when supported, and call the policy API for store-backed coordination
decisions. Adapter-local policy is limited to fail-closed classification and
trusted wrapper validation for command-shaped execution. In the shipped
prototype, `stateful-core` owns pure scope-policy primitives and
`stateful-server::policy_service` owns store-backed authorization orchestration.

## IDE Soft Save Gate

V1 includes an advisory VS Code extension under `integrations/vscode`. It calls
`/v1/human/save-check` for the active editor and save attempts, warns when the
target conflicts with an active agent claim or write fence, and fails open with a
visible warning if the state server is unavailable because losing human work is
worse than missing a coordination warning.

The save gate does not grant agent override authority. If a human continues a
conflicting save and that save is later observed as high-confidence human work,
agent writes to that file are denied until the agent refreshes state and
acknowledges reconciliation.

The VS Code extension also posts low-confidence open/presence and dirty signals
and high-confidence save observations. Selected-file telemetry and broader
arbitrary filesystem/editor watching remain future work.

## Human Write Reconciliation

The current implementation records human presence/save observations and blocks
agent writes for unreconciled high-confidence human `save`, `change`, and
`delete` observations on affected file paths. Human observations recorded while
an active same-file write fence is held are attributed to the agent write and do
not create a human-write block.

During this blocked state, the agent may still:

- read the affected file
- search the repository
- inspect diffs
- run sandboxed tests with an external `/tmp/stateful/<session>/<scratch-purpose>/`
  artifact tree

To resume writing, the agent must reread the affected file and acknowledge:

```text
state.reconcile.ack(reservation_id, files_reread, human_change_summary, decision)
```

The CLI form is:

```text
stateful reconcile ack --reservation-id <id> --files-reread <path> --summary <text> --decision adopt|reapply|ask_user|abandon
```

The HTTP route requires non-empty `files_reread`, an active reservation id whose
exact file intent covers every reread file, a summary, and a decision. Only
`adopt` and `reapply` clear the human-write block. `ask_user` records the
acknowledgement but keeps writes blocked until the user responds. `abandon`
records the decision without clearing the block; the agent should release or
shorten affected work separately.

If the state server is unavailable, reconciliation acknowledgement fails closed
and cannot clear the block.

## Conflict Policy

Initial shipped policy:

- no active reservation before supported write action in enforcement mode: deny
- no active reservation before supported write action in awareness mode: warn,
  unless another hard safety denial applies
- reservation without matching file or directory scope before supported write
  action in enforcement mode: deny
- reservation without matching file or directory scope before supported write
  action in awareness mode: warn, unless another hard safety denial applies
- expired reservation or reservation beyond the canonical rolling lifetime:
  enforcement mode denies as missing active reservation; awareness mode warns
- blocked phase or finalized agent flow before supported write action: deny in
  enforcement, warn in awareness
- directory reservation and directory claim authorize only `write_directory` for
  the exact directory resource; they do not authorize `write_file`, delete,
  rename, or move actions on child paths
- `write_directory` requires exact directory reservation, and a matching
  directory claim fences the whole subtree because command-shaped `--write-dir`
  execution can write anywhere below that directory
- delete operation without exact file scope: deny
- rename or move without exact file scope for both source and destination: deny
- stale base observation on a file-changing write: deny with reread/retry
- missing base observation on a file-changing write whose target can be
  identified and read by the adapter: deny until the adapter supplies it
- active claim in the hard conflict domain by another actor: enforcement mode
  denies unless the current agent has an explicit user override for that
  resource; awareness mode warns unless another hard safety denial applies
- same-file in-flight write fence conflict: deny
- same `workspace_id` and same normalized `relative_path`: shipped hard conflict
  domain for active claim and wait-queue checks
- same normalized `absolute_path`: target physical-file hard conflict domain
- planned edit in the same area: target warning context
- same `repo_id` and same `relative_path` across different workspace, worktree,
  or branch: target soft repo-relative warning
- unknown repository identity: target behavior is to hard-block only on same
  normalized absolute path and render weaker matches as unknown-confidence
  context
- expired claim: allow and surface stale context
- human working tree change near the target: warn before edits
- human save observed after an agent claim or write: deny further agent writes
  until the agent acknowledges reconciliation or receives an explicit user
  instruction
- current shipped hook path records file target observations on exact file claim
  acquire, denies hook-originated writes when the claimed file changes before
  authorization, and refreshes that observation after same-reservation supported file
  tools complete
- git index, stage, and commit operations in one checkout: serialize through the
  controlled git sandbox path
- destructive delete, rename, reset, or equivalent operations without exact
  target scope and explicit policy approval: deny
- unsafe raw commands and eval-tool execution outside trusted sandbox/scope
  policy: deny
- unrelated reads and searches: allow
- reads, searches, diffs, and sandboxed tests after human writes: allow
- tests: allow only through trusted sandbox-run wrappers with authorized targets
- task, port, or migration resource conflict: warn or info only in v1

Conflict decisions must be auditable. Overrides are never automatic. They are
valid only when the user explicitly instructs the current agent to allow a
specific resource override, for example: "Allow override for `src/auth.ts`."
The user owns the judgment and responsibility for that exception.

Overrides apply only to active claim conflicts. They do not bypass missing
reservation, expired reservation, blocked/finalized activity phase, file or
directory scope matching, delete exact-scope rules, or rename/move exact-scope
rules. Overrides are scoped to the current agent, current turn, and specific
resource when the target override policy is implemented.

Overrides do not act as queue priority. They cannot reorder FIFO waiters,
transfer a reservation, or let a later waiter take a resource ahead of the
agent with the claimable reservation.

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

Presence and `context_render` are safe to center now: they explain who is nearby,
what changed, and when to reread. They do not prove write authority. In
enforcement mode, a later write still needs active reservation scope plus a fresh
same-reservation claim. In awareness mode, reservation/scope/claim problems are
rendered and warned, but stale observations, unreconciled human writes, and
same-file write fences remain hard write-boundary safety rails.

`state_context_render` supports `brief` and `detailed` modes plus an optional
singular `resource` filter. `brief` is for hook start, prompt submit context,
and planning-time known-target resource checks. `detailed` is for manual deep
inspection when planning context lacks enough evidence. Denial recovery should
follow the denial's direct next action rather than rendering ambient context.

The current server route renders store-backed live context from active reservations,
active claims, and queued or claimable (`reserved`) wait records. The response
includes summary counts, structured `items`, and prompt-ready `prompt_text`.
Released claims are absent from this live render. A later write by that agent
must acquire a fresh same-reservation claim or claim a claimable reservation before authorization
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

Evidence kind distinguishes declared reservation, claim-only blockers, queue or
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
- interrupted agent -> keep last state until TTL expires
- hook failure -> warn and fail closed only for high-risk writes
- OMP stateful hook deny or unavailable result -> block, never warn because of
  yolo metadata. After runtime identity/discovery succeeds, authorize failures
  can produce structured block JSON; identity/discovery subprocess failures are
  mapped by the OMP extension to a hard block.
- state server unavailable -> deny supported writes that cannot prove active
  reservation
- state server unavailable -> deny Codex raw Bash, arbitrary OMP raw Bash, and
  all Python/JavaScript/JS/Ruby/Julia eval-tool execution. OMP built-in Bash
  passthrough is limited to strict trusted Stateful sandbox/process commands,
  and command-shaped writes through `--fs write-targets` fail closed when target
  authorization cannot be proven.
- state server unavailable -> write-target sandbox authorization fails closed
  and does not run the command
- state server unavailable -> fail closed for `state.reconcile.ack`, reservation
  declaration, claim acquisition, and claim refresh
- state server unavailable -> allow non-Bash read, search, and diff actions
- state server unavailable during IDE human save gate -> warn the user and allow
  the save
- heartbeat, finalization, and human observer events that cannot reach the state
  server should be appended to a local outbox for later sync
- local outbox events are audit/recovery evidence only; they cannot authorize
  writes while the state server is unavailable
- outbox sync uses `outbox_id` as an idempotency key; duplicate sync attempts
  must not create duplicate state events
- outbox sync preserves local creation order per agent when replaying pending
  events
- cached write grace periods are not part of v1
- stale conflict -> allow with context, not hard block

V1 defaults to strict enforcement. Supported writes and reconciliation fail
closed when state cannot be trusted. To keep this usable,
denial responses must explain the missing precondition and the next action
instead of returning opaque policy failures. The prototype `stateful doctor`
reports install/config/repo-enabled state plus global path and registry errors;
prescriptive next-action guidance is future doctor UX work.
