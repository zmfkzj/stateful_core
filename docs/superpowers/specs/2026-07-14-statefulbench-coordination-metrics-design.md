# StatefulBench Coordination Metrics Design

## Goal

Add value-free coordination measurements to StatefulBench so a `parallel-on` row can distinguish advisory overlap notifications, delivered notifications, reservation waits, authorization conflicts, and live context rendering. Preserve the existing benchmark outcome and efficiency metrics; these measurements explain *why* an on-arm behaved differently without redefining `cleared`.

## Scope

This change covers the Docker real-world runner and the shared diagnostics helper used by it:

- `crates/stateful-bench/scripts/statefulbench_realworld.py`
- `crates/stateful-bench/scripts/statefulbench_container_diagnostics.py`
- the existing OMP JSON usage parser in `statefulbench_lite.py`
- the Stateful server context-render route
- focused Python and Rust tests
- StatefulBench user and agent documentation that describes result fields

It does not change task prompts, repository corpora, evaluator behavior, arm scheduling, `cleared`, qualification identity, or benchmark timing boundaries.

## Decisions

### Measure server renders and explicit tool calls separately

`context_renders.server` counts successful `/v1/context/render` requests, including automatic hook requests and explicit native-tool requests. `context_renders.explicit_tool_calls` counts only agent-initiated `state_context_render` tool executions in OMP JSON logs. The explicit count is a subset of the server count when every explicit request reaches the endpoint; the benchmark does not manufacture a derived automatic count.

### Use server-log markers, not Stateful DB writes

A successful context-render route writes one fixed, value-free marker to the detached server log immediately before returning its successful response. The marker contains no agent ID, workspace, resource, path, prompt, item, token, or timing value. The diagnostics helper counts exact marker lines in `.stateful/runtime/server.log` while the arm server is still alive.

Context rendering remains read-only with respect to the Stateful SQLite coordination store. Recording a `ContextRendered` event or counter in that store was rejected because measurement would add SQLite writes and could change the contention being measured.

Failed validation and failed store reads do not increment the successful-render count. A server-log read that changes during capture is unavailable evidence, not a zero count.

### Derive coordination aggregates from the private SQLite snapshot

The diagnostics helper already copies each SQLite database and its sidecars to a private directory before opening the copy read-only. It will recognize the Stateful coordination schema and compute value-free aggregates from that same private copy:

- notifications grouped by protocol `kind` and final `status`
- wait records grouped by final `status`
- reservation-grant wait duration
- authorization denied and warned events grouped by protocol `reason_code`

The helper never emits notification payloads, wait IDs, reservation IDs, agent IDs, paths, resources, timestamps, or free-form messages.

### Preserve phase boundaries

Server marker snapshots use the existing diagnostic phases:

- task count: `after-tasks - before-tasks`
- final-review count: `after-final - after-tasks`
- total: task count plus final-review count

A decreasing cumulative marker count is inconsistent evidence and makes the on-arm diagnostics incomplete.

Explicit tool-call counts use the corresponding OMP logs:

- task count: sum across task-agent logs
- final-review count: final-agent log
- total: task count plus final-review count

## Result Schema

Every result row gains `coordination_metrics`.

- `parallel-on`: a populated object when diagnostics are complete
- `sequential` and `parallel-off`: `null`
- failed or incomplete on-arm diagnostics: `null`, with the existing diagnostic failure path keeping the row uncleared

```json
{
  "coordination_metrics": {
    "notifications": {
      "by_kind": {
        "scope_overlap": {
          "created": 0,
          "delivered": 0,
          "pending": 0,
          "expired": 0
        },
        "reservation_granted": {
          "created": 0,
          "delivered": 0,
          "pending": 0,
          "expired": 0
        }
      }
    },
    "waits": {
      "by_final_status": {},
      "grant_wait_time_s": {
        "count": 0,
        "total": 0.0,
        "mean": null,
        "max": null
      },
      "unmeasured_grants": 0
    },
    "authorization": {
      "denied_by_reason": {},
      "warned_by_reason": {}
    },
    "context_renders": {
      "server": {
        "tasks": 0,
        "final": 0,
        "total": 0
      },
      "explicit_tool_calls": {
        "tasks": 0,
        "final": 0,
        "total": 0
      }
    }
  }
}
```

`notifications.by_kind` always includes `scope_overlap` and `reservation_granted`, even when their counts are zero. Each `created` value is the sum of that kind's retained status counts. Additional protocol kinds may appear as additional sorted keys.

`scope_overlap.created` is the number of deduplicated advisory notifications created by Stateful. It is not presented as a count of raw edit collisions. `scope_overlap.delivered` is the number marked delivered through the poll/SSE delivery path. The same status split applies to `reservation_granted`.

`authorization.denied_by_reason` and `warned_by_reason` expose protocol reason codes such as `active_claim_conflict`, `missing_claim`, and `missing_reservation`. They do not include decision messages.

## Wait-Time Calculation

For each `reservation_granted` notification:

1. Parse only its protocol `wait_id` from the private-copy payload.
2. Find the matching wait record.
3. Subtract `wait_queue.requested_at` from the notification `created_at`.
4. Accept only finite, non-negative durations.

`grant_wait_time_s.count`, `total`, and `max` summarize linked grants. `mean` is `total / count`, or `null` when count is zero. A missing wait row, missing timestamp, malformed timestamp, missing `wait_id`, or negative duration increments `unmeasured_grants`; it is never silently converted to zero seconds.

Durations are serialized with a stable microsecond precision. The metric measures queue request to grant availability, not grant to claim and not total agent blocking time.

## OMP Tool-Call Identification

The existing line-oriented OMP JSON parser counts context-render executions only on `tool_execution_start` records. It accepts the canonical names and runtime-qualified aliases already supported by Stateful, including underscore, dotted, and MCP-prefixed forms whose normalized tool name is `state_context_render` or `state.context.render`.

Malformed JSON lines remain ignored as they are for token and total-tool-call extraction. Text mentions of `state_context_render` in prompts, skills, arguments, tool results, or assistant prose do not count.

Each agent result may retain its own explicit-render count alongside token and tool-call usage so the row total is auditable without rereading logs.

## Summary Aggregation

`results.json` rows and `summary.json.results` preserve the complete row metric object.

For a repository/arm aggregate:

- off-arm `coordination_metrics` is `null`
- on-arm metrics are aggregated only when every scheduled trial row is present and contains complete coordination metrics
- notification, wait-status, authorization-reason, render, grant-count, total, and unmeasured counts are summed
- aggregate wait `mean` is aggregate `total / count`
- aggregate wait `max` is the maximum row value, or `null` when aggregate count is zero

If any scheduled on-arm row is absent or lacks complete metrics, aggregate `coordination_metrics` is `null`; existing row failures remain the authoritative explanation. The summary does not present a partial coordination total as a complete multi-trial result.

## Diagnostic Integrity and Privacy

The coordination metrics are observational evidence. They must not weaken existing diagnostic checks.

An on-arm row remains uncleared when:

- the Stateful SQLite copy is unavailable, locked, or malformed
- the expected coordination schema is absent
- the server log marker count cannot be captured consistently
- phase marker counts decrease
- the result assembler cannot construct the required metrics

The diagnostics continue to be value-free. Dynamic output keys are protocol categories (`kind`, `status`, and `reason_code`), not user content. No raw database row or server-log line is copied into `results.json`.

## Testing

### Python diagnostics

Use temporary SQLite fixtures to verify:

- notification grouping by kind and status
- required zero-valued notification kinds
- delivered `scope_overlap` and `reservation_granted` counts
- wait final-status grouping
- linked wait duration count, total, mean, and max
- malformed, absent, and negative-duration grants counted as unmeasured
- authorization denied and warned grouping by reason code
- no payload, ID, path, timestamp, or message leakage
- exact server marker counting and changed-file failure

### OMP log parsing

Verify:

- canonical and supported alias tool names count
- ordinary text mentions do not count
- malformed JSON does not count
- total tokens and total tool-call behavior remains unchanged

### Result assembly and summaries

Verify:

- task/final/total phase deltas
- decreasing marker counts reject on-arm evidence
- off arms serialize `coordination_metrics: null`
- on-arm rows retain metrics in `results.json` and `summary.json.results`
- multi-trial maps and counts sum correctly
- wait mean is weighted from total/count, not the mean of row means
- wait max and zero-count null behavior
- missing or incomplete trial metrics do not produce a partial aggregate

### Rust server

Verify that:

- one successful context render emits one exact value-free marker
- validation and store failures emit no success marker
- the route response contract is unchanged

Run the focused Python benchmark suites and the focused Stateful server route tests. A live model-backed benchmark rerun is not required to validate serialization, but the next authorized smoke must report the new metrics before they are used for an on/off conclusion.
