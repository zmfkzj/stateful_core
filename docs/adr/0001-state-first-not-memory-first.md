# ADR 0001: Current-state first, not memory-first

Status: Proposed

## Context

Agent memory systems are useful for recalling prior information, but they do not
solve live coordination. A retrieved memory can describe what happened before.
It cannot reliably show what another session, agent, or human is doing in the
same codebase right now.

The practical v1 need is current-state coordination:

- active session status
- declared intent
- advisory resource leases
- root agent, subagent, human, and system actor attribution
- next planned action
- freshness through heartbeat and TTL
- intent and conflict checks before important tool calls
- final status for handoff

## Decision

`stateful_core` will focus first on current-state coordination for coding
agents.

The canonical unit is active, expiring coordination state. The first
integration target is:

```text
Codex lifecycle hooks
+ MCP tools
+ state server
```

Long-term memory can provide background context, but it does not own live
coordination truth and cannot directly authorize or block actions.

V1 blocks supported write actions unless the session has active intent with
matching file or directory scope. Abstract task, test, port, or migration intent
can be recorded as context but does not authorize writes.

The default intent TTL is 15 minutes. Heartbeats may extend active intent up to
a 60-minute maximum from declaration. Blocked or finalized work does not
authorize writes.

The original V1 hardening target denied Bash command text as an authorization
source and explored a constrained read-only hook path. The current
implementation supersedes that target: raw Bash is denied by stateful hooks.
Hook-mediated Bash must be a single strict invocation of the trusted absolute
`stateful` binary running `<absolute-stateful-binary> sandbox run ... --command
<cmd>`. Repo write authorization starts with structured paths such as
`state_file_write` / `state.file.write`, where target paths can be checked before
writing, or with `--fs write-targets` wrapper calls that declare explicit
targets. Raw Bash test commands are not allowlisted; validation uses
`state_validation_run` / `state.validation.run` in Codex sessions, or
`stateful validate <profile>` outside hook-mediated Bash.

Overrides are never automatic. A blocked write can proceed only when the user
explicitly instructs the current session to allow a specific resource override.
The user owns the judgment and responsibility for that exception.
Overrides apply only to active lease conflicts and are scoped to the current
session, current turn, and specific resource.

Subagents may write only within the parent session's active valid intent scope,
but their activity and leases are attributed to the subagent actor.

## Consequences

Positive:

- Clear v1 product boundary.
- Direct value for multi-agent and multi-session coding.
- Freshness is explicit through TTL and heartbeat.
- Intent and conflict checks can run before supported Codex tool calls.
- Forking Codex can be delayed until hook limitations are proven.

Negative:

- The model is narrower than a generic state control plane.
- Codex hooks are a guardrail, not a complete enforcement boundary.
- Human activity requires additional observation beyond Codex hooks.
- Advisory leases reduce collisions but do not guarantee exclusive access.
- Requiring intent before writes adds workflow friction.
- Override handling requires clear audit records because the user is taking
  explicit responsibility for the exception.

## Boundary Rule

```text
Memory recalls the past.
Current state coordinates the present.
Only fresh coordination state should influence live conflict decisions.
```
