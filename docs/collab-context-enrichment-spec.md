# Collaboration Context Enrichment — Design Spec

Status: Draft (approved for spec, pending implementation plan)
Date: 2026-07-05

## Problem

Agents learn about peer work through four channels, and the effective one fires
exactly once at the least-informative moment:

| Channel | Timing | Content | Limit |
|---|---|---|---|
| UserPromptSubmit render | Once per session | brief render + policy reminder | permanent marker blocks re-render |
| PreToolUse denial | At the moment of conflict | reason + next action + `blocking_agent_id`/`queue_position` | reactive; already collided |
| SSE notification | Only for what you queued on | `reservation_granted` only | learns only what you waited for |
| `state_context_render` | On demand | full state | policy discourages routine calls |

Two grounding facts:

1. Context is emitted once, at first prompt, then blocked forever by an on-disk
   marker (`crates/stateful-cli/src/hook.rs:1359-1364`,
   `.stateful_core/runtime/prompt_context/{agent_id}.sent` at `hook.rs:1496-1502`).
   The first-prompt snapshot is the last snapshot; peers who reserve afterward
   are invisible until a denial.
2. The only notification producer is `reservation_granted`
   (`crates/stateful-store/src/lib.rs:2652`), and the OMP extension handles only
   that event (`crates/stateful-cli/assets/stateful-omp-extension.js:310`). No
   "a peer reserved near your scope" signal exists.

Benchmark evidence: the 8 claim conflicts in recent DeNovo runs are all
"collided without warning" cases — avoidable if seen before the collision.

## Goals

- Agents perceive overlapping peer work **before** hitting a denial.
- Every render answers who / what / why / until-when.
- No change to the permission model (file-level reservation/claim).
- **No DB schema change.**

## Non-goals

- Line/range-level reservation or claim (permission model stays file-level).
- `path_released` push to non-queued agents (encourages racing; wait queue +
  `reservation_granted` already own that path).
- Bidirectional overlap push (declarer already sees holders / hits denial).

## Architecture

```
declare / claim ---overlap check---> notifications(kind=scope_overlap)
                                           |
                        SSE 1s poll -------+------- state_notifications_poll
                              |                            |
                       OMP extension                    Codex
                     (nextTurn inject)          (manual poll + re-render)

UserPromptSubmit hook (Codex) --render + fingerprint diff--> inject only on change
context renderer --owner/expiry/wait/next--> prompt + state_context_render
```

## Component 1: `scope_overlap` push

### Producer (store)

Two triggers:

- After `ReservationDeclared` handling (`lib.rs` `apply_event`, near `:2344`):
  for each new scope, find owners of overlapping **active reservation scopes ∪
  active claims** belonging to *other* agents; `append_notification` to each.
- After `acquire_claim_with_observation_inner` success
  (`crates/stateful-store/src/claims.rs:183`): notify owners of overlapping
  active reservations held by other agents. (Claim-vs-claim overlap is
  structurally impossible — already rejected as conflict.)

Overlap predicate reuses existing conflict semantics (exact file / directory
prefix). Reservation scopes parsed from `scopes_json`, matched via
`ReservationScope::allows_*`.

### Payload

```json
{
  "relative_path": "src/auth.ts",
  "action": "write_file",
  "by_agent_id": "s2",
  "purpose": "Fix auth validation",
  "source": "reservation_declared",
  "overlaps_your": "src/auth.ts"
}
```

### Direction

Push only to existing holders. The new declarer already learns via denial at
claim time — bidirectional push is noise.

### Dedup (producer)

Skip append if a `(target, kind=scope_overlap, by_agent, path)` notification
exists within the last 120s. Required because OMP auto-declare mints a new
reservation per edit boundary. Implemented by parsing the target's recent
notification rows in Rust (avoids schema change).

### Server

`notification_sse_event` forwards kind as event name unchanged. `scope_overlap`
carries **no `required_next_action`** (FYI, not action-forcing).

### Consumer (OMP extension)

Handle `scope_overlap` event. Dedup set keyed `${by_agent}:${path}` mirroring
`seenReservationWaitIds`. Message:

```
Stateful overlap: agent s2 declared write intent on src/auth.ts (overlaps your active scope).
purpose: Fix auth validation
FYI only — coordinate or adjust file split if needed. Do not redeclare or steal claims.
```

Delivered `{ deliverAs: "nextTurn", triggerTurn: false }` — does **not** wake an
idle agent (unlike `reservation_granted`, which should).

### Codex

No SSE consumer; covered by `state_notifications_poll` + Component 2.

## Component 2: change-detection re-render (Codex UserPromptSubmit)

Replace marker logic at `hook.rs:1359-1364`:

- Marker content: `"rendered\n"` → coordination **fingerprint hash**.
- Fingerprint = SHA-256 of sorted `kind|resource|agent_id|purpose|status` lines,
  over **reservations + wait queue + human writes only**. Exclude claims / write
  fences (sub-second transient; post-tool auto-release ⇒ churn). Exclude
  `expires_at`/timestamps (heartbeat refreshes TTL ⇒ meaningless hash drift).
- Flow:
  - marker absent → emit policy reminder + full context, store fingerprint
    (identical to current first render).
  - marker present → render, compare fingerprint. Changed ⇒ re-emit **context
    only** (no repeated reminder), update marker. Same ⇒ empty string (current
    token cost).
- Upgrade path: existing `"rendered\n"` markers mismatch the hash, re-emit once,
  then behave normally. No migration.

## Component 3: renderer enrichment (both runtimes)

`crates/stateful-core/src/context.rs` + store item strings; no struct field add:

1. Show `next` for info items in Your Active Scope — currently `severity != Info`
   only (`context.rs:495-499`), so "keep claim while writing" guidance is
   dropped.
2. Show `evidence` for Block items in brief — wait info (`blocked by s1; wait_id
   …`) is currently detailed-only (`context.rs:503-506`).
3. Include `#queue_position` in wait-queue item summary — reuse
   `queue_position(wait_id)` (`crates/stateful-store/src/reservations.rs:332`);
   store injects into summary: `"Agent s3 is queued (#2) for write_file on
   src/auth.ts"`.
4. Compress multi-file reservations (prompt only): same `(agent, purpose)` with
   ≥4 files → one line `"s2 reserved 8 files: a.rs, b.rs, c.rs, +5 more"`. JSON
   `items` stays per-file.
5. Populate `age_seconds`: store computes from `observed_at`; renderer appends
   `(43s ago)` when present. Negligible token growth within the 8-item cap.

## Component 4 (final, conditional): OMP throttled context refresh

OMP has no per-prompt hook. Add a `context_update` field to the post-tool-use
hook result; the extension `tool_result` handler injects it via
`sendMessage(nextTurn)`. 30s throttle + fingerprint diff for cost control.

**Deferred**: Component 1 (SSE push) already fills OMP's main gap. Pursue only if
benchmarks after 1–3 show remaining shortfall.

## Error handling

- Overlap producer propagates with `?` inside the store transaction — same
  policy as the `reservation_granted` producer (`lib.rs:2650`). Append failure ≈
  DB-level failure.
- Extension side wraps in try/catch (notification failure must never block an
  edit).
- Fingerprint compute failure (parse etc.) → fall back to re-emit (duplicate is
  safer than omission).

## Testing

| Target | File | Cases |
|---|---|---|
| overlap producer | `crates/stateful-store/tests/event_store.rs` | file/file, dir⊃file, file⊂dir overlap notify; self excluded; 120s dedup; claim-trigger; non-overlap no-notify |
| routes | `crates/stateful-server/tests/routes.rs` | holder poll sees scope_overlap after declare; SSE event kind; no required_next_action |
| renderer | `crates/stateful-core/tests/context.rs` | active-scope info shows next; block evidence in brief; 4+ file grouping; queue_position string |
| marker/fingerprint | `crates/stateful-cli/tests/hook.rs` | first emit; no-change→empty; reservation change→re-emit; claim churn→no-op; legacy marker upgrade |
| extension | `crates/stateful-cli/assets/stateful-omp-extension.test.mjs` | scope_overlap delivered once + dedup; triggerTurn false |

## Rollout order

1. Component 3 (renderer) — independent, immediate value.
2. Component 1 (scope_overlap) — store → server → extension.
3. Component 2 (fingerprint re-render).
4. Docs/skill sync (stateful-command-policy skill, omp-tools.md, README) — as
   AGENTS.md follow-up checks.
5. Component 4 decided after measurement.

## Docs to sync

- `crates/stateful-cli/assets/stateful-command-policy/omp-tools.md` — SSE
  scope_overlap event behavior.
- `crates/stateful-cli/assets/stateful-command-policy/SKILL.md` /
  `denial-recovery.md` — scope_overlap is FYI, not a denial.
- `README.md` — collaboration awareness description.
