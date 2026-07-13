# StatefulBench Coordination Metrics Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add value-free Stateful coordination, wait, conflict, and live-context-render counts to Docker real-world benchmark rows and multi-trial summaries.

**Architecture:** The Stateful server emits one fixed log marker after each successful context render without writing to the coordination database. The Docker diagnostics helper counts that marker and derives notification, wait, and authorization aggregates from its existing private SQLite copy; the OMP JSON usage parser independently counts explicit context-render tool calls. The real-world runner validates phase deltas, attaches the resulting object only to `parallel-on`, and sums complete multi-trial metrics without presenting partial aggregates.

**Tech Stack:** Rust 1.90 (`axum`, `tokio`, `AtomicU64`), Python 3.14 standard library (`sqlite3`, `datetime`, `unittest`), OMP line-oriented JSON logs, StatefulBench Docker diagnostics.

## Global Constraints

- Keep context rendering read-only with respect to the Stateful SQLite coordination store; do not append a `ContextRendered` event or database counter.
- The server marker must be one fixed value-free line and must contain no agent ID, workspace, resource, path, prompt, item, token, timestamp, or timing value.
- Count only successful `/v1/context/render` responses; validation and store failures must not increment the count.
- Never emit raw notification payloads, wait IDs, reservation IDs, agent IDs, paths, resources, timestamps, or decision messages in diagnostics or results.
- Do not change prompts, corpora, evaluators, arm scheduling, `cleared`, qualification identity, or benchmark timing boundaries.
- `parallel-on` requires complete coordination metrics; `sequential` and `parallel-off` serialize `coordination_metrics: null`.
- Use Python 3.14 standard-library `unittest`; do not add dependencies.
- Do not launch a live model-backed OMP benchmark while implementing this plan.
- Follow `skill://stateful-command-policy` for every command and repository write.

---

### Task 1: Emit and verify successful context-render markers

**Files:**
- Modify: `crates/stateful-server/src/lib.rs:33-53,1205-1277,2150`

**Interfaces:**
- Produces: `CONTEXT_RENDER_SUCCESS_MARKER: &str`, fixed as `[stateful-metric] context_render_success`.
- Produces: `ServerConfig::record_context_render_success(&self)`; increments a shared `Arc<AtomicU64>` and writes the exact marker to stderr.
- Produces: `ServerConfig::context_render_success_count(&self) -> u64`; private helper used by in-file tests.
- Contract for Task 2: successful detached-server stderr contains one exact marker line per successful render.

- [ ] **Step 1: Add failing in-file route metric tests**

Append a `#[cfg(test)] mod context_render_metric_tests` to `crates/stateful-server/src/lib.rs`. Construct `ContextRenderRequest` directly and call the private `context_render` handler so the tests can inspect the private counter without adding a public test API:

```rust
#[cfg(test)]
mod context_render_metric_tests {
    use super::*;
    use axum::extract::State;

    fn request(workspace_id: Option<&str>) -> ContextRenderRequest {
        ContextRenderRequest {
            mode: Some("brief".to_string()),
            resource: None,
            workspace_id: workspace_id.map(str::to_string),
            agent_id: Some("agent-a".to_string()),
            repo_id: None,
            worktree_id: None,
            root: None,
        }
    }

    #[tokio::test]
    async fn context_render_metric_counts_only_successful_renders() {
        let config = ServerConfig::new("token");
        let (ok_status, _) = context_render(
            State(config.clone()),
            Json(request(Some("workspace-a"))),
        )
        .await;
        let (bad_status, _) =
            context_render(State(config.clone()), Json(request(None))).await;

        assert_eq!(ok_status, StatusCode::OK);
        assert_eq!(bad_status, StatusCode::BAD_REQUEST);
        assert_eq!(config.context_render_success_count(), 1);
        assert_eq!(
            CONTEXT_RENDER_SUCCESS_MARKER,
            "[stateful-metric] context_render_success"
        );
    }

    #[tokio::test]
    async fn context_render_metric_does_not_count_store_failure() {
        let config = ServerConfig::new("token");
        let store = config.store.clone();
        let _ = std::thread::spawn(move || {
            let _guard = store.lock().expect("store should initially lock");
            panic!("poison test store");
        })
        .join();

        let (status, _) = context_render(
            State(config.clone()),
            Json(request(Some("workspace-a"))),
        )
        .await;

        assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(config.context_render_success_count(), 0);
    }
}
```

- [ ] **Step 2: Run the focused Rust tests and verify RED**

Run:

```sh
stateful sandbox run --fs build --network disabled --write-dir statefulbench-server-metrics-tests --command 'cargo test -p stateful-server context_render_metric -- --nocapture'
```

Expected: compilation fails because `CONTEXT_RENDER_SUCCESS_MARKER`, `context_render_success_count`, and the counter field do not exist.

- [ ] **Step 3: Add the shared atomic counter and marker emission**

Extend imports and `ServerConfig`:

```rust
use std::{
    collections::VecDeque,
    convert::Infallible,
    net::SocketAddr,
    str::FromStr,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

const CONTEXT_RENDER_SUCCESS_MARKER: &str = "[stateful-metric] context_render_success";

#[derive(Debug, Clone)]
pub struct ServerConfig {
    bearer_token: String,
    store: SharedStore,
    maintenance_interval: Duration,
    coordination_mode: CoordinationMode,
    context_render_success_count: Arc<AtomicU64>,
}
```

Initialize the counter in `ServerConfig::with_store` and add private methods:

```rust
context_render_success_count: Arc::new(AtomicU64::new(0)),
```

```rust
fn record_context_render_success(&self) {
    self.context_render_success_count.fetch_add(1, Ordering::Relaxed);
    eprintln!("{CONTEXT_RENDER_SUCCESS_MARKER}");
}

fn context_render_success_count(&self) -> u64 {
    self.context_render_success_count.load(Ordering::Relaxed)
}
```

Call `config.record_context_render_success()` only after `live_current_state_for_workspace_identity` succeeds and after prompt rendering, immediately before constructing the `StatusCode::OK` response. Do not call it in either error branch.

- [ ] **Step 4: Run the focused Rust tests and verify GREEN**

Run the Step 2 command again.

Expected: both `context_render_metric_*` tests pass. The successful test prints exactly one `[stateful-metric] context_render_success` line under `--nocapture`; the bad request and poisoned-store request add none.

- [ ] **Step 5: Commit the server marker**

```sh
git add crates/stateful-server/src/lib.rs
git commit -m "feat: count successful context renders"
```

---

### Task 2: Derive value-free coordination metrics in Docker diagnostics

**Files:**
- Modify: `crates/stateful-bench/scripts/statefulbench_container_diagnostics.py:15-18,68-186,213-239`
- Modify: `crates/stateful-bench/scripts/tests/test_statefulbench_docker.py:796-950`

**Interfaces:**
- Consumes: exact server marker `[stateful-metric] context_render_success` from Task 1.
- Produces: `_coordination_metrics(connection: sqlite3.Connection, table_names: set[str]) -> dict | None`.
- Produces: `_count_context_render_markers(path: Path, expected: os.stat_result) -> int`.
- Produces in each snapshot: `runtime_metrics.context_render_success_count: int | None`.
- Produces in the recognized Stateful DB record: `coordination_metrics` with `notifications`, `waits`, and `authorization` keys matching the approved design.

- [ ] **Step 1: Add failing SQLite aggregation tests**

In `DockerDiagnosticTests`, add a helper that creates `.stateful/state.db` with the production columns used by the collector:

```python
def coordination_database(self):
    import sqlite3

    database = self.home / ".stateful" / "state.db"
    database.parent.mkdir(parents=True, exist_ok=True)
    connection = sqlite3.connect(database)
    connection.executescript(
        """
        CREATE TABLE notifications (
            notification_id TEXT PRIMARY KEY,
            kind TEXT NOT NULL,
            payload_json TEXT NOT NULL,
            status TEXT NOT NULL,
            created_at TEXT NOT NULL
        );
        CREATE TABLE wait_queue (
            wait_id TEXT PRIMARY KEY,
            status TEXT NOT NULL,
            requested_at TEXT NOT NULL
        );
        CREATE TABLE events (
            event_id TEXT PRIMARY KEY,
            event_type TEXT NOT NULL,
            payload_json TEXT NOT NULL
        );
        """
    )
    return connection, database
```

Add `test_snapshot_aggregates_value_free_coordination_metrics`. Insert:

- delivered and pending `scope_overlap` rows
- one delivered `reservation_granted` linked to a wait requested 2.5 seconds earlier
- one malformed or missing-link `reservation_granted`
- queued and claimed waits
- `AuthorizationDenied` with `active_claim_conflict`
- `AuthorizationWarned` with `missing_claim`

Assert the recognized database record contains:

```python
{
    "notifications": {
        "by_kind": {
            "reservation_granted": {
                "created": 2,
                "delivered": 1,
                "pending": 1,
                "expired": 0,
            },
            "scope_overlap": {
                "created": 2,
                "delivered": 1,
                "pending": 1,
                "expired": 0,
            },
        }
    },
    "waits": {
        "by_final_status": {"claimed": 1, "queued": 1},
        "grant_wait_time_s": {
            "count": 1,
            "total": 2.5,
            "mean": 2.5,
            "max": 2.5,
        },
        "unmeasured_grants": 1,
    },
    "authorization": {
        "denied_by_reason": {"active_claim_conflict": 1},
        "warned_by_reason": {"missing_claim": 1},
    },
}
```

Serialize the full snapshot and assert that inserted IDs, paths, free-form messages, and timestamps are absent.

- [ ] **Step 2: Add failing zero, invalid-duration, and marker tests**

Add focused tests:

```python
def test_coordination_metrics_keep_required_zero_notification_kinds(self):
    # Create empty production-shaped tables, snapshot, and assert both
    # scope_overlap and reservation_granted have created/delivered/pending/expired = 0.
```

```python
def test_coordination_metrics_reject_negative_grant_duration(self):
    # Insert a grant timestamp before requested_at and assert count == 0,
    # mean/max are None, and unmeasured_grants == 1.
```

```python
def test_snapshot_counts_exact_context_render_markers(self):
    log = self.home / ".stateful" / "runtime" / "server.log"
    log.parent.mkdir(parents=True)
    log.write_text(
        "startup\n"
        "[stateful-metric] context_render_success\n"
        "not [stateful-metric] context_render_success extra\n"
        "[stateful-metric] context_render_success\n",
        encoding="utf-8",
    )
    snapshot = self.diagnostics.snapshot_home(self.home)
    self.assertEqual(
        snapshot["runtime_metrics"]["context_render_success_count"], 2
    )
```

Extend an existing swap/race test or add one that changes `server.log` between `lstat` and stable read and expects an `OSError`, proving a changing log is not reported as zero.

- [ ] **Step 3: Run the focused diagnostics tests and verify RED**

Run:

```sh
stateful sandbox run --fs build --network disabled --write-dir statefulbench-diagnostics-tests --command "python3 -m unittest discover -s crates/stateful-bench/scripts/tests -t crates/stateful-bench/scripts -p 'test_statefulbench_docker.py' -k coordination_metrics -v"
stateful sandbox run --fs build --network disabled --write-dir statefulbench-diagnostics-tests --command "python3 -m unittest discover -s crates/stateful-bench/scripts/tests -t crates/stateful-bench/scripts -p 'test_statefulbench_docker.py' -k context_render_markers -v"
```

Expected: failures because the new diagnostic fields and helpers do not exist.

- [ ] **Step 4: Implement categorical counting and safe timestamp parsing**

Add constants and small helpers:

```python
_CONTEXT_RENDER_SUCCESS_MARKER = b"[stateful-metric] context_render_success"
_COORDINATION_TABLES = {"events", "notifications", "wait_queue"}
_REQUIRED_NOTIFICATION_KINDS = ("reservation_granted", "scope_overlap")
_NOTIFICATION_STATUSES = ("delivered", "expired", "pending")


def _group_counts(rows) -> dict[str, int]:
    counts: dict[str, int] = {}
    for value, count in rows:
        if isinstance(value, str) and isinstance(count, int) and count >= 0:
            counts[value] = count
    return dict(sorted(counts.items()))


def _parse_timestamp(value: object) -> datetime | None:
    if not isinstance(value, str):
        return None
    try:
        parsed = datetime.fromisoformat(value.replace("Z", "+00:00"))
    except ValueError:
        return None
    return parsed if parsed.tzinfo is not None else parsed.replace(tzinfo=timezone.utc)
```

Import `datetime` and `timezone` from `datetime` and `isfinite` from `math`.

- [ ] **Step 5: Implement `_coordination_metrics` against the private copy**

Return `None` unless `_COORDINATION_TABLES` is a subset of `table_names`. Query only categorical columns plus the timestamps and payloads required for aggregation.

Build notification counts by `(kind, status)`. Seed `scope_overlap` and `reservation_granted` with all required status keys at zero, add observed protocol kinds in sorted order, and define `created` as the sum of retained status counts.

Build wait status counts from `SELECT status, count(*) ... GROUP BY status`.

For grants, load `wait_id -> requested_at`, iterate only `reservation_granted` notification rows, parse `payload_json`, extract only string `wait_id`, and compute:

```python
seconds = (created_at - requested_at).total_seconds()
if not isfinite(seconds) or seconds < 0:
    unmeasured_grants += 1
    continue
durations.append(round(seconds, 6))
```

Serialize:

```python
wait_total = round(sum(durations), 6)
wait_count = len(durations)
wait_stats = {
    "count": wait_count,
    "total": wait_total,
    "mean": None if wait_count == 0 else round(wait_total / wait_count, 6),
    "max": None if wait_count == 0 else max(durations),
}
```

For `AuthorizationDenied` and `AuthorizationWarned`, parse only `reason_code` from JSON-object payloads and count non-empty string codes. Ignore malformed payloads rather than exposing them.

After `table_counts` is built in `_sqlite_record`, call `_coordination_metrics(connection, set(names))` and add `record["coordination_metrics"]` only when the return value is not `None`.

- [ ] **Step 6: Implement stable exact marker counting**

Use `_regular_descriptor(path, expected)`, read bytes in blocks, count only complete lines equal to `_CONTEXT_RENDER_SUCCESS_MARKER`, and compare the final `fstat` tuple with the opened metadata before returning. Preserve a trailing partial line by carrying bytes between blocks; count it only if it exactly equals the marker at EOF. Always close the descriptor.

In `snapshot_home`, inspect the exact benchmark server-log path `.stateful/runtime/server.log` after the normal file walk:

```python
server_log = home / ".stateful" / "runtime" / "server.log"
try:
    server_log_metadata = server_log.lstat()
except FileNotFoundError:
    context_render_count = None
else:
    context_render_count = _count_context_render_markers(
        server_log, server_log_metadata
    )
```

Add:

```python
"runtime_metrics": {
    "context_render_success_count": context_render_count,
},
```

Do not inspect any other log content or emit matched lines.

- [ ] **Step 7: Run the focused diagnostics tests and verify GREEN**

Run the Step 3 command(s).

Expected: all new coordination, duration, privacy, and marker tests pass; existing SQLite WAL and redaction tests still pass.

- [ ] **Step 8: Commit the diagnostics collector**

```sh
git add crates/stateful-bench/scripts/statefulbench_container_diagnostics.py crates/stateful-bench/scripts/tests/test_statefulbench_docker.py
git commit -m "feat: collect StatefulBench coordination diagnostics"
```

---

### Task 3: Count explicit context-render tool executions

**Files:**
- Modify: `crates/stateful-bench/scripts/statefulbench_lite.py:288-317`
- Modify: `crates/stateful-bench/scripts/tests/test_statefulbench_lite.py:132-162`

**Interfaces:**
- Produces: `usage_from_log(path)` adds integer `context_render_tool_calls` while preserving `total_tokens` and `tool_calls` semantics.
- Contract for Task 4: every Docker agent record receives `context_render_tool_calls` through the existing `**usage` expansion in `statefulbench_docker.wait_agent`.

- [ ] **Step 1: Extend the failing usage-parser test**

Add `tool_execution_start` lines with these tool names:

```python
"state_context_render"
"state.context.render"
"mcp__stateful__state_context_render"
"mcp__stateful_state_context_render"
```

Also add prompt/tool-result text containing `state_context_render` and a different tool name. Assert the returned dictionary is:

```python
{
    "total_tokens": 18,
    "tool_calls": 5,
    "context_render_tool_calls": 4,
}
```

Update the execution-event fallback assertion to include `"context_render_tool_calls": 0`.

- [ ] **Step 2: Run the usage-parser test and verify RED**

Run:

```sh
stateful sandbox run --fs build --network disabled --write-dir statefulbench-lite-usage-tests --command "python3 -m unittest discover -s crates/stateful-bench/scripts/tests -t crates/stateful-bench/scripts -p 'test_statefulbench_lite.py' -k usage_parser -v"
```

Expected: assertion failure because `context_render_tool_calls` is absent.

- [ ] **Step 3: Implement exact tool-name normalization**

Add:

```python
def _is_context_render_tool(name: object) -> bool:
    if not isinstance(name, str):
        return False
    return name in {
        "state_context_render",
        "state.context.render",
        "mcp__stateful__state_context_render",
        "mcp__stateful_state_context_render",
    }
```

In `usage_from_log`, initialize `context_render_tool_calls = 0`. On `tool_execution_start`, keep incrementing `execution_tool_calls` and increment the new counter only when `_is_context_render_tool(value.get("toolName"))` is true. Return the new key even when the file is absent or contains no matching tool.

Do not count `toolName` on end/result records and do not search serialized text.

- [ ] **Step 4: Run the usage-parser test and verify GREEN**

Run the Step 2 command again.

Expected: the token/tool-call compatibility assertions and all explicit-render assertions pass.

- [ ] **Step 5: Commit the usage metric**

```sh
git add crates/stateful-bench/scripts/statefulbench_lite.py crates/stateful-bench/scripts/tests/test_statefulbench_lite.py
git commit -m "feat: count explicit context render tools"
```

---

### Task 4: Assemble row metrics and complete multi-trial summaries

**Files:**
- Modify: `crates/stateful-bench/scripts/statefulbench_realworld.py:1310-1360,1561-1612,2012-2347,2626-2650,2679-2789`
- Modify: `crates/stateful-bench/scripts/tests/test_statefulbench_realworld.py:2226-2472,3234-3375,3861-4054`

**Interfaces:**
- Consumes: snapshot `runtime_metrics.context_render_success_count` and recognized DB `coordination_metrics` from Task 2.
- Consumes: agent-record `context_render_tool_calls` from Task 3.
- Produces: `_build_coordination_metrics(arm: str, snapshots: dict[str, dict], agents: list[dict]) -> dict | None`.
- Produces: `_aggregate_coordination_metrics(arm: str, rows: list[dict], trials: int) -> dict | None`.
- Produces: `coordination_metrics` in every row and repository/arm aggregate.

- [ ] **Step 1: Add failing row-assembly tests**

Extend the Docker runner fixture diagnostics so on-arm snapshots contain:

```python
"runtime_metrics": {
    "context_render_success_count": {
        "initialized": 0,
        "before-tasks": 2,
        "after-tasks": 11,
        "after-final": 16,
        "after-grading": 16,
        "before-remove": 16,
    }[phase],
},
```

and `.stateful/state.db` has `coordination_metrics` only in the recognized database record. Return task agent records with counts totaling 3 and a final record with count 2. Run a `parallel-on` fixture and assert:

```python
result["coordination_metrics"]["context_renders"] == {
    "server": {"tasks": 9, "final": 5, "total": 14},
    "explicit_tool_calls": {"tasks": 3, "final": 2, "total": 5},
}
```

Assert the notification/wait/authorization object equals the `after-final` DB aggregate without mutation. Add an off-arm assertion that `coordination_metrics is None`.

Add a test with `after-tasks` marker count below `before-tasks`; assert the row is uncleared, `coordination_metrics is None`, and the error names decreasing context-render evidence.

- [ ] **Step 2: Add failing summary aggregation tests**

Extend `RealWorldReportingTests.result` with optional `coordination_metrics=None` and include the field in returned rows.

Create two complete `parallel-on` trial rows whose nested maps have different keys and wait stats. Assert aggregate behavior:

- notification status maps sum by kind and status
- wait final statuses sum by key
- authorization reason maps sum by reason
- render tasks/final/total sum
- grant count and total sum
- grant mean equals summed total divided by summed count, not the average of row means
- grant max is the maximum non-null row max
- unmeasured grants sum

Add one missing/incomplete scheduled trial and assert aggregate `coordination_metrics is None`. Assert sequential and parallel-off aggregates use `None` even if helper fixtures omit the row field.

- [ ] **Step 3: Run the focused runner tests and verify RED**

Run:

```sh
stateful sandbox run --fs build --network disabled --write-dir statefulbench-realworld-metrics-tests --command "python3 -m unittest discover -s crates/stateful-bench/scripts/tests -t crates/stateful-bench/scripts -p 'test_statefulbench_realworld.py' -k coordination_metrics -v"
```

Expected: failures because row and aggregate builders are absent.

- [ ] **Step 4: Implement strict snapshot extraction and phase deltas**

Add helpers that reject booleans as integers and reject malformed nested structures:

```python
def _nonnegative_int(value: object, label: str) -> int:
    if type(value) is not int or value < 0:
        raise ValueError(f"{label} is invalid")
    return value


def _context_render_snapshot_count(snapshot: dict, phase: str) -> int:
    runtime_metrics = snapshot.get("runtime_metrics")
    if type(runtime_metrics) is not dict:
        raise ValueError(f"missing context render metrics at {phase}")
    return _nonnegative_int(
        runtime_metrics.get("context_render_success_count"),
        f"context render count at {phase}",
    )
```

Locate exactly one database record in `after-final.databases` whose `coordination_metrics` is a dictionary. Zero or multiple recognized records are contradictory.

In `_build_coordination_metrics`:

- return `None` immediately unless `arm == "parallel-on"`
- read `before-tasks`, `after-tasks`, and `after-final` marker counts
- raise `ValueError("context render counts decreased across phases")` unless they are nondecreasing
- compute task/final/total server deltas
- sum `context_render_tool_calls` from records with `kind == "task"` and `kind == "final"`, validating each value as a non-negative integer
- deep-copy the DB coordination object before adding `context_renders`

Do not derive `server - explicit`.

- [ ] **Step 5: Integrate row construction and diagnostic failure handling**

Initialize `coordination_metrics = None` before the Docker arm `try`. Immediately after `after-final` snapshot and shared-HOME validation, call `_build_coordination_metrics`. Let its `ValueError` follow the existing exception path, which stops grading and records the exact error.

Add `"coordination_metrics": coordination_metrics` to:

- `_empty_run_result`
- Docker `_run_container_repo_arm` result
- non-Docker/fixture `run_repo_arm` result, always `None` unless it uses the same complete Docker evidence path

Keep `cleared` unchanged except that an on-arm extraction error already makes `error` non-null. Do not add coordination collection time to agent-only wall time.

- [ ] **Step 6: Implement complete-trial summary aggregation**

Add recursive integer-map summation helpers scoped to the known metric schema; validate exact numeric types and reject booleans. `_aggregate_coordination_metrics` must return `None` when:

- `arm != "parallel-on"`
- present row count differs from `trials`
- any row metric is not a dictionary
- any required nested field is malformed

For complete on-arm rows, sum counts and maps, calculate:

```python
mean = None if count == 0 else round(total / count, 6)
maximum = None if count == 0 else max(non_null_row_maxima)
```

Add `"coordination_metrics": _aggregate_coordination_metrics(arm, original_rows, trials)` to every normal aggregate. Add `"coordination_metrics": None` to mixed-provenance aggregates so the schema remains stable.

Do not add coordination fields to the compact console table; `results.json` and `summary.json` are the authoritative detailed surfaces.

- [ ] **Step 7: Run the focused runner tests and verify GREEN**

Run the Step 3 command again.

Expected: phase-delta, off-arm-null, decreasing-count, map-sum, weighted-mean, maximum, and incomplete-trial tests pass.

- [ ] **Step 8: Run the complete touched Python test files**

Run:

```sh
stateful sandbox run --fs build --network disabled --write-dir statefulbench-coordination-python-tests --command "python3 -m unittest discover -s crates/stateful-bench/scripts/tests -t crates/stateful-bench/scripts -p 'test_statefulbench_lite.py' -v" --command "python3 -m unittest discover -s crates/stateful-bench/scripts/tests -t crates/stateful-bench/scripts -p 'test_statefulbench_docker.py' -v" --command "python3 -m unittest discover -s crates/stateful-bench/scripts/tests -t crates/stateful-bench/scripts -p 'test_statefulbench_realworld.py' -v"
```

Expected: all three files pass. If the build profile rejects repeated `--command`, run the three commands in three sequential sandbox invocations with the same dedicated write directory.

- [ ] **Step 9: Commit row and summary metrics**

```sh
git add crates/stateful-bench/scripts/statefulbench_realworld.py crates/stateful-bench/scripts/tests/test_statefulbench_realworld.py
git commit -m "feat: report StatefulBench coordination metrics"
```

---

### Task 5: Verify the integrated measurement contract

**Files:**
- Verify only; no planned production-file changes.

**Interfaces:**
- Consumes all Task 1-4 contracts.
- Produces evidence that the behavior works across server emission, diagnostic collection, OMP parsing, row assembly, and summary aggregation.

- [ ] **Step 1: Run all touched Python suites from a clean process**

Run the three commands from Task 4 Step 8 in fresh sequential sandbox invocations.

Expected: every test in `test_statefulbench_lite.py`, `test_statefulbench_docker.py`, and `test_statefulbench_realworld.py` passes with zero failures and errors.

- [ ] **Step 2: Run the focused Stateful server tests**

Run:

```sh
stateful sandbox run --fs build --network disabled --write-dir statefulbench-server-metrics-tests --command 'cargo test -p stateful-server context_render -- --nocapture'
```

Expected: all context-render route tests and both metric tests pass. Successful test requests emit the fixed marker; invalid and poisoned-store requests do not increment the tested counter.

- [ ] **Step 3: Run the credit-free Docker E2E when the qualified image is available**

Use the currently inspected `linux/arm64` image only if it still matches the local qualification boundary:

```sh
STATEFULBENCH_DOCKER_TEST_IMAGE=statefulbench-realworld:local \
stateful sandbox run --fs build --network disabled --write-dir statefulbench-coordination-docker-e2e --connect-socket "$HOME/.colima/default/docker.sock" --command "python3 -m unittest discover -s crates/stateful-bench/scripts/tests -t crates/stateful-bench/scripts -p 'test_statefulbench_docker.py' -v"
```

Expected: the credit-free fake-OMP Docker E2E passes. If the inspected image is unavailable or its identity changed, record that this optional check was not run; do not rebuild or launch a model-backed benchmark as a substitute.

- [ ] **Step 4: Inspect one deterministic fixture result**

Read the test-created or fixture-generated `results.json` and `summary.json` through the test assertions, not by manually editing artifacts. Confirm:

- on row has populated `coordination_metrics`
- off row has `null`
- server and explicit counts are separate
- no IDs, paths, payloads, messages, or timestamps appear inside `coordination_metrics`
- aggregate wait mean is weighted

- [ ] **Step 5: Commit only if verification required a correctness fix**

If no changes were needed, do not create an empty commit. If a focused correctness fix was required, rerun its RED/GREEN test and commit only the exact touched files with a message naming the corrected contract.
