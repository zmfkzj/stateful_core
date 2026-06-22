# V1 Hardening Scope Decisions

This is a dated planning artifact for the v1 hardening pass, not a complete
description of the current implementation. The README and implementation
contract are authoritative for shipped behavior.

This document locks the product and implementation decisions for the next v1
hardening pass. The project should move beyond the current prototype surface and
implement the stricter recommended direction for scheduling, sandboxed tests,
protocol metadata, policy routing, local security, IDE save gating, structured
MCP writes, doctor checks, and deployment UX.

The prototype remains local-only. Team-shared, cross-machine, and hosted sync
are still out of scope.

## Scheduling

Implement the full scheduling API:

```text
POST /v1/intent/request
POST /v1/intent/claim
POST /v1/intent/cancel
```

`intent/request` creates or returns an idempotent request by `request_id`.
Available requests may grant immediately. Conflicting requests queue FIFO.
Retrying the same `request_id` after reservation expiry requeues the same waiter
in place instead of creating a duplicate or permanently consuming the key.

`intent/claim` is the manual reservation claim path. It creates
write-authorizing intent and active leases only for the reservation owner.
Clients must reread the reserved target before writing. Manual MCP/CLI flows
then call `state_intent_claim` or `stateful intent claim --wait-id <id>`;
native edit hooks and sandbox `write-targets` authorization may lazy-claim the
reservation at the retried write boundary.

Current implementation status: `/v1/intent/request`, `/v1/intent/claim`, and
`/v1/intent/cancel` are implemented with MCP tools and CLI commands. Immediate
availability returns a `reserved` request state; the reserved session must still
reread the target. Manual MCP/CLI flows call `state_intent_claim`; hook and sandbox
authorization sources may lazy-claim the reservation when the write is retried.

`intent/cancel` cancels queued or reserved requests owned by the caller. It must
not cancel another session's reservation or reorder waiters.

Remove public `stateful intent wait --timeout` documentation and do not
implement it in this pass. Waiting can be a later CLI convenience over
notifications and `resume next`.

## Sandboxed Tests

Raw Bash commands are not a write-authorizing or test execution path. Official
test execution uses the trusted sandbox wrapper:

```text
stateful sandbox run --fs build --network enabled --write-dir test-run --command <cmd>
```

Hook-mediated Bash must be a single strict invocation of the trusted absolute
`stateful` binary running `<absolute-stateful-binary> sandbox run ... --command
<cmd>`. The build profile writes disposable artifacts under
`/tmp/stateful/<session>/<purpose>/`; source-tree edits use native edit tools
with hook-visible targets, such as Codex `apply_patch` or Edit, after exact
intent declaration and a successful same-session file lease. The completed write
transaction releases the authorizing lease. Command-shaped source writes require
exact `--write-target` or `--create-target` entries.

## Protocol Envelope

Envelope-enforced write authorization, intent, and reconciliation HTTP requests
must use a v1 envelope:

```json
{
  "protocol_version": "stateful.v1",
  "request_id": "stable-idempotency-key",
  "session": {
    "session_id": "s1",
    "actor_id": "agent-1",
    "actor_type": "agent"
  },
  "workspace": {
    "workspace_id": "local",
    "root": "/repo",
    "repo_id": "repo-id",
    "worktree_id": "worktree-id",
    "branch": "main"
  },
  "source": {
    "kind": "hook",
    "event": "pre_tool_use",
    "source_ref": "tool-call-id"
  },
  "payload": {}
}
```

Lifecycle session events are intentionally smaller in the shipped OMP adapter:
`SessionStart`, `PostToolUse`, and `Stop`/finalize post flat session-event
bodies with `session_id`, `workspace_id`, `source`, and `metadata`. OMP
`PreToolUse` authorization remains envelope-based.

Recommended rollout:

1. Add shared envelope builders in CLI, hook, MCP, and outbox callers.
2. Add server parsing that accepts both envelope and legacy bodies while tests
   prove all internal callers have migrated.
3. Fail closed for legacy side-effecting requests.

After enforcement, missing or incompatible protocol metadata returns a
`protocol_mismatch` error and does not mutate state.

`request_id` is required for idempotent scheduling and should also be used for
other write-authorizing state transitions where duplicate retries are possible.

## Policy Service

The codebase uses a two-layer policy architecture:

- `stateful-core` keeps pure, store-free decision logic.
- `stateful-server::policy_service` owns store-aware orchestration.

HTTP handlers should authenticate and parse protocol input, then delegate final
allow, warn, deny, and error decisions to the policy service. Handlers should not
carry independent policy branches.

The server policy service normalizes `authorize_write` and `claim_intent`.
Remaining scheduling and orchestration work should extend it with:

```text
request_intent
cancel_intent
acquire_lease
release_lease
ack_reconciliation
check_conflicts
```

The policy service may perform store-backed orchestration such as lazy
expiration, reservation lookup, queue promotion, and decision-to-event mapping.

## IDE Save Gate

Pull human write observation into scope through an IDE save gate, not a broad
filesystem watcher.

The first IDE integration should check the state server before a save when the
editor exposes a pre-save hook. It should surface warnings or blocks for active
agent leases, pending reservations, and unresolved human-write reconciliation
requirements.

Filesystem watcher inference remains out of scope for this pass.

## Repo File Edits

MCP file-write tools are not the current repo edit path. Repo file edits should
use native edit tools with hook-visible targets, such as Codex `apply_patch` or
Edit, after exact intent declaration and a successful same-session file lease.
Hooks normalize hook-exposed targets, call the same policy service as MCP and
CLI, fail closed on missing state, protocol mismatch, or denied authorization,
record activity after successful edits, and release the authorizing lease after
the completed write transaction.

Command-shaped writes remain outside MCP and must use
`stateful sandbox run --fs write-targets` with exact `--write-target` or
`--create-target` entries. Artifact-producing tests use
`--fs build --write-dir <scratch-purpose>`, which writes disposable artifacts
under `/tmp/stateful/<session>/<purpose>/`.

Structured git writes should remain narrower than arbitrary git. `stateful
commit` remains the default local wrapper, while MCP git tools can expose
specific staged/commit operations only after authorization.

## Expiration And Retention

Background expiration and retention pruning are shipped in the state server,
with lazy expiration kept as the safety net on read/write paths. Expiration
paths expire stale leases, stale reservations, and stale intent state, promote
eligible FIFO waiters, and create notifications.

Implement the full rolling maximum model for active write-authorizing intent.
The default rolling maximum is 60 minutes unless a narrower profile or future
policy overrides it.

Retention pruning removes historical evidence older than the built-in 14-day
window from events, reconciliations, conflicts, human observations, and expired
notifications. It preserves active current-state rows, pending notifications,
and outbox sync evidence.

## Runtime And Security Hardening

Strengthen local runtime security while keeping the product local-only:

- done: write runtime files with user-only permissions where supported
- done: verify runtime identity before stopping or trusting an existing server
- done: keep network binding local by default
- done: reject non-loopback plain `http://` joins before sending bearer tokens
- keep bearer token authentication for all non-health endpoints
- reject stale or malformed runtime discovery files more aggressively
- document that local token auth is a trust guard, not an OS isolation boundary

## Doctor

Expand `stateful doctor` from install/config checks to actionable diagnostics:

- active server reachability
- runtime identity consistency
- config schema validation
- SQLite migration/version inspection
- sandbox-run smoke checks
- repo enabled/disabled status
- hook and MCP installation status
- prescriptive next action for common failure modes

## Deployment UX

Pull plugin packaging and managed hook deployment UX into scope now.

The CLI remains the bootstrap path, but the project should add a managed
distribution story for hook-capable agents:

- install/update workflow for stateful-managed hooks
- wrapper support for hook-capable agents beyond Codex
- clear version reporting for hook, CLI, and server
- rollback or repair command for broken installs

## Still Out Of Scope

- Team-shared state sync
- Cross-machine coordination
- Hosted state service
- Broad filesystem watcher as the primary human observation mechanism
- Full arbitrary git command authorization
- Secret pass-through policy for sandboxed test environments

## Implementation Order

1. Done: remove `intent wait` docs.
2. Done: add protocol envelope builders for CLI, hook, MCP, and outbox.
3. Done: add server protocol parser and migration tests.
4. Done: introduce policy service and move `/v1/authorize` first.
5. Done: implement explicit `intent/request`, `intent/claim`, and
   `intent/cancel`.
6. Done: enforce sandbox-run write-target policy semantics.
7. Remaining: add IDE save gate API and harden native edit hook target
   extraction.
8. Done: add background expiration, retention pruning, and active-intent rolling
   maximum.
9. Remaining: reject stale or malformed runtime discovery files more aggressively.
10. Remaining: expand doctor diagnostics.
11. Remaining: add managed hook and plugin deployment UX.
