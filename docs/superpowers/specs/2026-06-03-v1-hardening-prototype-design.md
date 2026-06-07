# V1 Hardening Prototype Design

> Public documentation note: this is a historical implementation record kept for traceability, not current user-facing guidance. See `README.md` and the top-level `docs/` contract documents for current behavior.

> Historical note: validation profile references in this design are obsolete. Current test/build execution uses `stateful sandbox run --fs write-targets` with explicit write targets or `--write-dir target`.

## Summary

The prototype moves from MVP behavior to a harder v1 coordination contract. The
implementation should prioritize agent-agent coordination, explicit scheduling
APIs, a shared policy entry point, protocol hardening, thin but portable hook
wrappers, minimal useful prompt context, and lazy expiration.

This design intentionally does not expand into human write observation,
structured MCP filesystem or git tools, full prompt rendering, background
expiration, retention pruning, or validation profile policy changes. Those are
future work unless explicitly pulled back into scope.

## Goals

- Provide explicit scheduling APIs for request, claim, and cancel.
- Require v1 protocol metadata for side-effecting requests.
- Use `request_id` as an idempotency key for scheduling and write-authorizing
  state changes.
- Route write, scheduling, lease, validation, and reconciliation decisions
  through one policy service entry point.
- Complete agent-agent lease conflict behavior before human observation.
- Support hooks from Codex and other hook-capable agents through a normalized
  wrapper layer.
- Render minimal current-state context that is useful during blocked or
  resumable sessions.
- Replace fixed timestamps with real clock-driven lazy expiration.
- Align public docs with prototype install and lazy server lifecycle.

## Non-Goals

- Do not implement `stateful intent wait --timeout`; remove it from public docs.
- Do not implement structured MCP filesystem or git write tools. The prototype
  supports `Edit`, `Write`, `apply_patch`, and `stateful commit`.
- Do not implement human write filesystem watching or human-write policy blocks.
- Do not implement full rich prompt rendering beyond current lease, queue,
  reservation, conflict, and required-next-action context.
- Do not implement a background expiration loop, retention pruning, or the full
  60-minute rolling maximum model.
- Do not change validation profile policy until the product semantics are
  clarified.

## Scheduling API

The server adds three explicit HTTP endpoints:

```text
POST /v1/intent/request
POST /v1/intent/claim
POST /v1/intent/cancel
```

`intent/request` creates or returns a request for one or more resources. If all
resources are available and policy allows the request, it can return a grant. If
another session has a conflicting active lease or active reservation, it enqueues
the request. Repeating the same `request_id` returns the same request state and
does not enqueue a duplicate.

`intent/claim` is the official reservation claim path. It requires the reserved
session, workspace, request id or wait id, and the resources being claimed. A
successful claim creates active write-authorizing intent and active leases for
the claimed resources. A non-owner claim is denied.

`intent/cancel` cancels queued or reserved requests owned by the session. It
does not cancel another session's reservation and does not reorder waiters.

The existing `/v1/resume/next` remains a discovery API. It returns the next
reservation and instructs the client to reread the resource and call
`intent/claim`. The previous implicit claim-on-write behavior is removed from
the official path. If a reserved session retries a write before claiming, the
server returns a denial that points to `intent/claim`.

## Protocol Hardening

Side-effecting POST requests must carry v1 protocol metadata:

```text
protocol_version: stateful.v1
request_id
session
workspace
source
```

The server validates `protocol_version` before mutating state. A major mismatch
fails closed for write authorization, scheduling, lease acquisition, validation,
reconciliation, and outbox sync. Read-only endpoints may return a minimal
protocol error with no side effects.

The implementation can introduce typed envelope wrappers incrementally, but the
public request handlers should converge on one extraction path so protocol
checks do not drift by endpoint.

`request_id` is an idempotency key for scheduling and other write-authorizing
state transitions. The store should persist enough request metadata to return
the prior result for repeated requests instead of appending duplicate queue or
grant events.

GET endpoints remain token-authenticated read paths. They do not need a JSON
body envelope. If a future client needs protocol negotiation for reads, it can
use headers or a POST read endpoint.

## Policy Service

The prototype should introduce a policy service layer that owns final
allow/warn/deny/error decisions. Adapters and route handlers parse inputs, but
they do not own policy branches.

The policy service should accept normalized operations such as:

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

The existing `authorize_action` write-scope rules should move behind this
service rather than remain as the only policy function. Bash classification and
hook tool target extraction remain adapter responsibilities, but the final
decision is produced by the policy service.

Validation and reconciliation still use their current behavior unless the
policy service blocks them for protocol, session, or state reasons. Validation
profile semantics beyond the existing runner are out of scope.

## Agent-Agent Conflict Priority

The first completed coordination path is agent-agent resource conflict:

- session declares or requests intent for target resources
- session acquires active leases on allowed write resources
- conflicting sessions are denied or queued
- released/finalized/expired leases promote the next waiter
- promoted waiters receive reservations and notifications
- the reserved session claims explicitly before writing

Human write observation and reconciliation blocks are not part of this pass.
The schema may keep existing human observation tables and reconciliation
records, but no watcher or human-write policy block is required.

## Hook Wrapper

Hooks should stop being Codex-only in their internal shape. The CLI should
support a normalized hook entry point that any hook-capable agent can call.

Codex hook parsing remains as an adapter. The adapter converts Codex-specific
input into a normalized event:

```text
session_start
user_prompt_submit
pre_tool_use
post_tool_use
stop
```

The normalized event carries session id, workspace identity when known, source
metadata, tool name, tool input, command text, and observed time. After
normalization, all hook sources use the same policy and server calls.

The CLI should add explicit wrapper surfaces:

```text
stateful hook codex <event>
stateful hook run <event>
```

The existing `stateful hook <event>` commands remain as Codex compatibility
aliases during the prototype.

## Minimal Context Renderer

`/v1/context/render` should stop returning only empty context. The prototype
renderer should include:

- active leases relevant to the session or requested resource
- queued requests owned by the session
- active reservations owned by the session
- active reservations blocking the session
- required next action for claim, wait, redeclare, or release
- concise conflict summary

The renderer should not attempt full human-write reconciliation context, stale
history sections, or deep neighboring activity. Those remain future work.

## Lazy TTL and Expiration

Fixed timestamp strings should be replaced by real time from an injectable
clock. Tests should use a fake clock or deterministic clock helper.

Minimum lazy expiration:

- expired leases no longer block authorization
- expired reservations no longer block later sessions
- expiring a reservation promotes the next queued waiter when available
- reads that expose current state should lazily expire relevant stale rows

Background loops, retention pruning, and the full 60-minute rolling intent
maximum are future work. The implementation should leave the data model open for
those later additions.

## Validation Profiles

Validation profile semantics are intentionally deferred. The current runner may
remain:

- load `.stateful/validation.yml`
- run a named profile command
- enforce timeout
- compare `git status --porcelain` before and after
- fail when newly dirty paths match `denied_writes`

This pass should not decide whether common Bash tests stay allowed, whether
`exclusive` controls concurrency, or whether `env` and `allowed_writes` become
enforced policy inputs.

## Documentation Alignment

The public docs should be updated with the implementation direction:

- default quick start is `stateful install --yes` followed by `stateful enable`
- lazy server lifecycle is the prototype default
- foreground server start is a compatibility/debug path
- `stateful intent wait --timeout` is removed
- structured MCP filesystem and git write tools are future work
- official reservation flow is `resume next`, reread resource, `intent claim`,
  then write
- validation profile policy expansion is deferred
- human write observation is future work

## Testing

The implementation plan should add focused tests before changing behavior:

- protocol mismatch fails closed for side-effecting endpoints
- repeated scheduling `request_id` does not duplicate queue entries
- `intent/request` grants available resources and queues blocked resources
- `intent/claim` succeeds only for the reservation owner
- `intent/cancel` cancels only owned queued or reserved requests
- lazy expiration frees leases and reservations
- context rendering includes queued, reserved, and blocking state
- Codex hook input and normalized hook input reach the same policy path
- docs no longer mention removed `intent wait`

## Rollout

Implement in small slices:

1. Add envelope and request id storage support.
2. Introduce the policy service while preserving existing write decisions.
3. Add scheduling request, claim, and cancel APIs.
4. Add lazy expiration with an injectable clock.
5. Add normalized hook wrapper and keep Codex compatibility.
6. Add minimal context rendering from store state.
7. Align README and architecture/contract docs.

Each slice should keep `cargo test --workspace` passing before moving to the
next slice.
