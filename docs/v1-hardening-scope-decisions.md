# V1 Hardening Scope Decisions

This document locks the product and implementation decisions for the next v1
hardening pass. The project should move beyond the current prototype surface and
implement the stricter recommended direction for scheduling, validation,
protocol metadata, policy routing, local security, IDE save gating, structured
MCP writes, doctor checks, and deployment UX.

The prototype remains local-only. Team-shared, cross-machine, and hosted sync
are still out of scope.

## Scheduling

Implement the full scheduling API now:

```text
POST /v1/intent/request
POST /v1/intent/claim
POST /v1/intent/cancel
```

`intent/request` creates or returns an idempotent request by `request_id`.
Available requests may grant immediately. Conflicting requests queue FIFO.

`intent/claim` is the official reservation claim path. It creates
write-authorizing intent and active leases only for the reservation owner.
Implicit claim-on-authorize should be removed from the official path after the
claim endpoint and clients are migrated.

`intent/cancel` cancels queued or reserved requests owned by the caller. It must
not cancel another session's reservation or reorder waiters.

Remove public `stateful intent wait --timeout` documentation and do not
implement it in this pass. Waiting can be a later CLI convenience over
notifications and `resume next`.

## Validation Profiles

Promote validation profiles from a convenience runner to a policy surface.
Official validation execution paths are:

```text
stateful validate <profile>
POST /v1/validation/run
```

Raw Bash commands are not a write-authorizing or validation execution path. The
hardening target is to deny Bash unless the tool payload carries structured
read-only sandbox metadata with network disabled, and to tell the user to run
named validation profiles for tests instead.

Validation policy semantics:

- `timeout_seconds` is enforced as it is today.
- `exclusive: true` acquires a workspace-level validation lock. While active,
  other validation runs and write-authorizing operations in the same workspace
  are denied or queued according to policy.
- `exclusive: false` allows concurrent validation when no other policy conflict
  exists.
- `denied_writes` always wins. Newly dirty paths matching `denied_writes`
  produce policy failure.
- `allowed_writes` becomes a real post-run policy input. Newly dirty paths must
  be covered by `allowed_writes` unless they are already permitted by a more
  specific profile rule.
- Profile `env` is allowed but explicit. The runner should not blindly preserve
  all sensitive shell environment into validation commands. Secret pass-through
  requires a future allowlist and is not included here.

## Protocol Envelope

Side-effecting HTTP requests must use a v1 envelope:

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
    "kind": "codex_hook",
    "event": "pre_tool_use",
    "source_ref": "tool-call-id"
  },
  "payload": {}
}
```

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

Introduce a two-layer policy architecture:

- `stateful-core` keeps pure, store-free decision logic.
- `stateful-server::policy_service` owns store-aware orchestration.

HTTP handlers should authenticate and parse protocol input, then delegate final
allow, warn, deny, and error decisions to the policy service. Handlers should not
carry independent policy branches.

The server policy service should normalize operations:

```text
authorize_write
request_intent
claim_intent
cancel_intent
acquire_lease
release_lease
run_validation
ack_reconciliation
check_conflicts
```

The policy service may perform store-backed orchestration such as lazy
expiration, reservation lookup, queue promotion, validation lock checks, and
decision-to-event mapping.

## IDE Save Gate

Pull human write observation into scope through an IDE save gate, not a broad
filesystem watcher.

The first IDE integration should check the state server before a save when the
editor exposes a pre-save hook. It should surface warnings or blocks for active
agent leases, pending reservations, and unresolved human-write reconciliation
requirements.

Filesystem watcher inference remains out of scope for this pass.

## Structured MCP Writes

Support structured MCP filesystem and git write tools under stateful
authorization.

MCP write tools must:

- normalize targets before authorization
- call the same policy service as hooks and CLI
- use protocol envelopes
- fail closed on missing state, protocol mismatch, or denied authorization
- record activity after successful writes

Structured git writes should remain narrower than arbitrary git. `stateful
commit` remains the default local wrapper, while MCP git tools can expose
specific staged/commit operations only after authorization.

## Expiration And Retention

Add background expiration and retention pruning.

Lazy expiration remains as the safety net, but a background worker should expire
stale leases, stale reservations, and stale intent state. Expiration should
promote eligible FIFO waiters and create notifications.

Implement the full rolling maximum model for active write-authorizing intent.
The default rolling maximum is 60 minutes unless a narrower profile or future
policy overrides it.

Retention pruning should preserve audit evidence needed for recent context,
conflict explanation, and debugging while removing stale low-value rows.

## Runtime And Security Hardening

Strengthen local runtime security while keeping the product local-only:

- write runtime files with user-only permissions where supported
- verify runtime identity before stopping or trusting an existing server
- keep bearer token authentication for all non-health endpoints
- reject stale or malformed runtime discovery files more aggressively
- keep network binding local by default
- document that local token auth is a trust guard, not an OS isolation boundary

## Doctor

Expand `stateful doctor` from install/config checks to actionable diagnostics:

- active server reachability
- runtime identity consistency
- config schema validation
- SQLite migration/version inspection
- validation profile parseability
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
- Secret pass-through policy for validation environments

## Implementation Order

1. Remove `intent wait` docs.
2. Add protocol envelope builders for CLI, hook, MCP, and outbox.
3. Add server protocol parser and migration tests.
4. Introduce policy service and move `/v1/authorize` first.
5. Implement `intent/request`, `intent/claim`, and `intent/cancel`.
6. Enforce validation profile policy semantics.
7. Add IDE save gate API and structured MCP write tools.
8. Add background expiration, retention pruning, and rolling maximum.
9. Harden runtime files and local trust checks.
10. Expand doctor diagnostics.
11. Add managed hook and plugin deployment UX.
