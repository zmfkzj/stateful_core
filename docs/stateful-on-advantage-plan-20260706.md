# Stateful-On Advantage Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the `stateful:on` benchmark condition demonstrably beat `stateful:off` — first by proving the coordination *tax* is near-zero at equal quality, then by making coordination produce quality that stateless cannot.

**Architecture:** Four independent levers, sequenced measurement-first. Lever 3 (metrics) lands first because no advantage can be claimed until value and cost are measured cleanly. Lever 2 (cut the largest remaining self-inflicted friction turn) and Lever 4 (a benchmark slice where contention is real) are cheap follow-ups. Lever 1 (cross-agent findings sharing — the only score-upside lever) is built last and gated on Lever 3 evidence that agents actually re-derive knowledge.

**Tech Stack:** Rust workspace (`stateful-cli`, `stateful-server`, `stateful-store`, `stateful-core`, `stateful-bench`) + Python benchmark scripts under `crates/stateful-bench/scripts`. Sandbox for all commands: `stateful sandbox run --fs build --network enabled --write-dir <scratch> --command '<cmd>'`.

## Global Constraints

- Rust tests run via `cargo test -p <crate> --test <file>` inside a `stateful sandbox run --fs build` wrapper; never raw `cargo` outside the sandbox.
- Python tests run via `stateful sandbox run --fs build ... --command 'python -m pytest crates/stateful-bench/scripts/tests/<file> -q'` (or the repo's configured venv).
- No DB schema change (hard constraint inherited from `docs/collab-context-enrichment-spec.md`). Findings sharing reuses the free-form `append_notification` payload.
- Do not change the fixed benchmark run shape (`skill://running-stateful-benchmarks`): 3 trials, 10 instances, `--max-concurrent 4`, one server per host. New comparisons are *additional slices*, not edits to the standard recipe.
- Every behavioral change ships with its runnable check (TDD). Skip formatters/linters/project-wide suites during task work; run them once at the end.
- Commit after each task.

---

## Grounding Summary (verified 2026-07-06)

Decisive fact that reframes the whole effort: in the analyzed DeNovo run, true cross-agent contention was **4 `active_claim_conflict` events across 30 rollouts**, every one with a real `blocking_agent_id` (`docs/denovo-stateful-coordination-analysis-20260704.md` §S1). The other 107 denials were **self-inflicted friction** (write-first `missing_reservation`, `stale_target_observation` 40, `missing_claim` 20), not protection. Observed scores were within noise (~0.58 both conditions). So today `stateful:on` pays a large coordination tax for ~zero score benefit.

Consequences for the plan:
- **Cost side (Levers 2, 3):** the tax is self-inflicted and shrinking. `missing_reservation` was just removed by predeclare-first (`crates/stateful-cli/src/hook.rs:551-572`). `stale_target_observation` is now the largest residual class and is addressable.
- **Score side (Lever 1):** locking cannot raise the score when nothing collides. The only path to `on > off` is coordination producing *reusable knowledge* across subagents. This is speculative here, hence gated on measurement.

Verified current-code anchors:

| Area | Anchor |
|---|---|
| Predeclare-first + authorize loop | `crates/stateful-cli/src/hook.rs:551-675` |
| Auto-claim release | `release_omp_auto_claims` `crates/stateful-cli/src/hook.rs:756-765` |
| Keep-claim-on-stale (Block still returned) | `should_keep_omp_auto_claims` `crates/stateful-cli/src/hook.rs:918-920` |
| missing_claim auto-retry arm (template to mirror) | `crates/stateful-cli/src/hook.rs:621-645` |
| Base observation captured from disk | `base_observation_for_target` `crates/stateful-cli/src/hook.rs:2942-2961` |
| Server stale check + denial strings | `base_observation_decision` `crates/stateful-server/src/policy_service.rs:953-1024`; `stale_target_observation_decision` `:1430-1434` |
| Reservation minted per edit | `declare_and_claim_omp_pre_tool_reservation` `crates/stateful-cli/src/hook.rs:922-988` |
| Trace summary struct + reader | `DeNovoTraceSummary` `crates/stateful-bench/src/denovo.rs:2048-2064`; `orchestration_trace_summary` `:2092-2135` |
| Report fields + assignment | `DeNovoConditionReport` `crates/stateful-bench/src/denovo.rs:1959-1986`; assigned `:2266-2279` |
| Python trace summarizer | `summarize_orchestration_events` `crates/stateful-bench/scripts/denovo_codex_agent.py` |
| Instance selection | `denovo_matrix_instance_ids` `crates/stateful-bench/src/denovo.rs:1096-1120` |
| Per-instance file-count signal | dataset row keys `measured_files_total` / `included_files` (`datasets/denovo/shards/denovoswe_public_shard_b.jsonl`) |
| Notification write path (free-form payload) | `append_notification` `crates/stateful-store/src/notifications.rs:107-148` |
| Overlap producer (pattern to mirror) | `notify_scope_overlap_for_declaration` `crates/stateful-store/src/scope_overlap.rs:9-62` |
| Context renderer | `render_prompt_text` `crates/stateful-core/src/context.rs:280-388`; `CurrentItem` `:75-101` |

## Sequencing

```
Phase 1 (Lever 3, metrics)  ── prerequisite for every quality/cost claim
        │
        ├── Phase 2 (Lever 2, stale auto-refresh)   ) independent, run in parallel
        └── Phase 3 (Lever 4, contention slice)      )
                │
                └── Phase 4 (Lever 1, findings sharing) ── GATED on Phase 1 evidence
```

---

## Phase 1 — Lever 3: Coordination-value metrics

**Why first:** Today the report shows raw counts (reservation/claim/conflict/denial events); `on` looks like pure overhead. Split friction from protection so the comparison can show "same-or-better score, fewer wasted turns, N real collisions prevented."

### Task 1.1: Python summarizer emits value fields

**Files:**
- Modify: `crates/stateful-bench/scripts/denovo_codex_agent.py` (`summarize_orchestration_events`)
- Test: `crates/stateful-bench/scripts/tests/test_reports.py`

**Interfaces:**
- Produces (into each result's `orchestration_trace` object): `true_collisions_prevented: int`, `self_inflicted_denials: int`, `scope_overlap_warnings: int`.
- Definitions:
  - `true_collisions_prevented` = count of events where `event_type` is a claim conflict (`active_claim_conflict`, or `AuthorizationDenied` whose `reason_code == "active_claim_conflict"`) **and** `wait.blocking_agent_id` (or `blocking_agent_id`) is non-null.
  - `self_inflicted_denials` = count of `AuthorizationDenied` events whose `reason_code` ∈ {`missing_reservation`, `missing_claim`, `stale_target_observation`, `missing_base_observation`, `scope_mismatch`} **and** no `blocking_agent_id`.
  - `scope_overlap_warnings` = count of events / notifications with `kind == "scope_overlap"` (advisory awareness delivered before a collision).

- [ ] **Step 1: Write the failing test**

```python
def test_summary_splits_friction_from_true_collision():
    events = [
        {"event_type": "AuthorizationDenied", "reason_code": "stale_target_observation",
         "workspace_id": "w1", "path": "a.py"},
        {"event_type": "AuthorizationDenied", "reason_code": "active_claim_conflict",
         "workspace_id": "w1", "path": "b.py", "wait": {"blocking_agent_id": "s2"}},
        {"event_type": "ScopeOverlap", "kind": "scope_overlap", "workspace_id": "w1", "path": "b.py"},
    ]
    summary = summarize_orchestration_events(events, agent_id=None, workspace_id="w1")
    assert summary["true_collisions_prevented"] == 1
    assert summary["self_inflicted_denials"] == 1
    assert summary["scope_overlap_warnings"] == 1
```

- [ ] **Step 2: Run test to verify it fails**

Run: `... --command 'python -m pytest crates/stateful-bench/scripts/tests/test_reports.py::test_summary_splits_friction_from_true_collision -q'`
Expected: FAIL — `KeyError: 'true_collisions_prevented'`.

- [ ] **Step 3: Implement in `summarize_orchestration_events`**

Add, alongside the existing `conflict_events`/`denial_events` accumulation, three counters using the same `matching` iteration and the existing `event_field(event, ...)` helper:

```python
SELF_INFLICTED = {
    "missing_reservation", "missing_claim", "stale_target_observation",
    "missing_base_observation", "scope_mismatch",
}
true_collisions_prevented = 0
self_inflicted_denials = 0
scope_overlap_warnings = 0
for event in matching:
    etype = event.get("event_type", "")
    reason = event.get("reason_code") or event_field(event, "reason_code")
    blocker = (event.get("wait") or {}).get("blocking_agent_id") or event.get("blocking_agent_id")
    if event.get("kind") == "scope_overlap" or etype == "ScopeOverlap":
        scope_overlap_warnings += 1
    if reason == "active_claim_conflict" and blocker:
        true_collisions_prevented += 1
    elif etype == "AuthorizationDenied" and reason in SELF_INFLICTED and not blocker:
        self_inflicted_denials += 1
```

Add the three keys to the returned summary dict next to `conflict_events`/`denial_events`.

- [ ] **Step 4: Run test to verify it passes**

Run: `... --command 'python -m pytest crates/stateful-bench/scripts/tests/test_reports.py -q'`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/stateful-bench/scripts/denovo_codex_agent.py crates/stateful-bench/scripts/tests/test_reports.py
git commit -m "bench: split self-inflicted friction from true collisions in trace summary"
```

### Task 1.2: Rust report aggregates and renders value fields

**Files:**
- Modify: `crates/stateful-bench/src/denovo.rs` — `DeNovoTraceSummary` (2048-2064), `orchestration_trace_summary` (2092-2135), `DeNovoConditionReport` (1959-1986), `build_denovo_condition_report` (2266-2279), `render_denovo_report_markdown` (1404-1428).
- Test: `crates/stateful-bench/tests/denovo.rs`

**Interfaces:**
- Consumes: `orchestration_trace` object keys `true_collisions_prevented`, `self_inflicted_denials`, `scope_overlap_warnings` (from Task 1.1).
- Produces: `DeNovoConditionReport.orchestration_true_collisions_prevented: usize`, `.orchestration_self_inflicted_denials: usize`, `.orchestration_scope_overlap_warnings: usize` (all `#[serde(default)]`).

- [ ] **Step 1: Write the failing test** (mirror an existing trace-summary test in `crates/stateful-bench/tests/denovo.rs`)

```rust
#[test]
fn condition_report_surfaces_true_collisions_and_friction() {
    let results = vec![official_result_with_trace(serde_json::json!({
        "trace_captured": true,
        "true_collisions_prevented": 2,
        "self_inflicted_denials": 5,
        "scope_overlap_warnings": 3,
    }))];
    let report = build_denovo_condition_report("r1", DeNovoCondition::new(true, true), results, 1000, None);
    assert_eq!(report.orchestration_true_collisions_prevented, 2);
    assert_eq!(report.orchestration_self_inflicted_denials, 5);
    assert_eq!(report.orchestration_scope_overlap_warnings, 3);
}
```

(If no `official_result_with_trace` helper exists, add a small local builder in the test that puts the JSON under `extra["orchestration_trace"]`, matching how `orchestration_trace_summary` reads it at `denovo.rs:2095-2098`.)

- [ ] **Step 2: Run test to verify it fails**

Run: `stateful sandbox run --fs build --network enabled --write-dir cargo-target-metrics --command 'cargo test -p stateful-bench --test denovo condition_report_surfaces_true_collisions_and_friction'`
Expected: FAIL — unknown field / method.

- [ ] **Step 3: Implement**

Add three `usize` fields to `DeNovoTraceSummary`; sum them in `orchestration_trace_summary` with the existing `value_usize(trace.get(...))` pattern; add three `#[serde(default)]` fields to `DeNovoConditionReport`; assign them in `build_denovo_condition_report` next to the existing `orchestration_conflict_events` assignment; add a markdown column/line in `render_denovo_report_markdown` showing `true collisions` and `self-inflicted denials` per condition.

- [ ] **Step 4: Run test to verify it passes**

Run: same as Step 2.
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/stateful-bench/src/denovo.rs crates/stateful-bench/tests/denovo.rs
git commit -m "bench: report true-collisions-prevented and self-inflicted-denial counts"
```

---

## Phase 2 — Lever 2: Stale auto-refresh under held claim

**Why:** `stale_target_observation` is the largest residual self-inflicted denial class (40 in the analyzed run). Today the hook keeps the auto-claim on stale (`should_keep_omp_auto_claims`, hook.rs:918-920) but still returns `Block`, forcing the agent to spend a turn re-reading and retrying. When the agent already **holds the exclusive claim**, no peer can have changed the file, so the hook can re-read the base observation itself and retry authorization once — eliminating the wasted turn. `[INFERENCE]` residual stale under a held claim is the agent's own prior write / a non-agent touch, not contention; safe to auto-refresh because the claim serializes peers.

### Task 2.1: Auto-refresh base observation and retry once when claim is held

**Files:**
- Modify: `crates/stateful-cli/src/hook.rs` — the `'authorize` loop (551-675); add `should_auto_refresh_omp_base_observation` beside `should_auto_claim_omp_tool_reservation` (905-916).
- Test: `crates/stateful-cli/tests/hook.rs`

**Interfaces:**
- Consumes: `AuthorizeDecision.reason_code`; the loop-local `auto_claimed_paths` (non-empty ⇒ claim held).
- Produces: new loop guard `auto_refresh_retried: bool`; helper `fn should_auto_refresh_omp_base_observation(input, decision, targets) -> bool` (edit/write, `write_file` targets, `reason_code == "stale_target_observation"`).
- Behavior: on stale, when `!auto_refresh_retried` and the claim is held (`!auto_claimed_paths.is_empty()`), re-read targets (fresh `base_observation_for_target` is already re-read each authorize call via `authorization_payload_for_target`, so a retry naturally re-captures), set `auto_refresh_retried = true`, and `continue 'authorize;`. Only fall through to `Block` after one refresh retry still fails.

- [ ] **Step 1: Write the failing test** (mirror `omp_write_stale_observation_after_auto_claim_blocks_without_releasing_claim`, hook.rs tests)

```rust
#[test]
fn omp_write_stale_under_held_claim_auto_refreshes_then_allows() {
    // Fake server: declare ok -> claim ok -> authorize STALE -> authorize ALLOW.
    let (runtime, rx) = spawn_fake_stateful_server_sequence(vec![
        r#"{"reservation_id":"auto-reservation"}"#,
        r#"{"status":"ok","claim_state":"acquired","paths":["docs/a.md"]}"#,
        r#"{"decision":"deny","reason_code":"stale_target_observation","message":"changed"}"#,
        r#"{"decision":"allow","message":"ok"}"#,
    ]);
    // ... build write input for docs/a.md, no reservation_id, yolo:false ...
    let outcome = handle_omp_pre_tool_use_with_runtime(&input, Some(&runtime), Some(Path::new("/repo")));
    assert_eq!(outcome, OmpHookOutcome::Allow);
    // declare, claim, first authorize (stale), second authorize (allow) all consumed; NO release posted.
    // assert the four requests arrive and no /v1/claim/release is sent.
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `stateful sandbox run --fs build --network enabled --write-dir cargo-target-stale --command 'cargo test -p stateful-cli --test hook omp_write_stale_under_held_claim_auto_refreshes_then_allows -- --exact'`
Expected: FAIL — currently returns `Block` on the stale decision.

- [ ] **Step 3: Implement**

In the `if decision.decision != "allow"` branch, after the existing `should_auto_claim` arm (hook.rs:621-645) and before computing `keep_auto_claims` (647), add:

```rust
if !auto_refresh_retried
    && !auto_claimed_paths.is_empty()
    && should_auto_refresh_omp_base_observation(input, &decision, &targets)
{
    auto_refresh_retried = true;
    continue 'authorize;
}
```

Declare `let mut auto_refresh_retried = false;` beside `auto_claim_retried` (hook.rs:548). Add the helper mirroring `should_auto_claim_omp_tool_reservation` but matching `reason_code == "stale_target_observation"`. The retry re-runs `post_omp_authorize_target`, which rebuilds the payload via `authorization_payload_for_target` → `base_observation_for_target` (fresh disk read), so the stale hash is refreshed with no agent round-trip.

- [ ] **Step 4: Run test to verify it passes**

Run: same as Step 2, then the full file: `... --command 'cargo test -p stateful-cli --test hook'`
Expected: PASS (target test + all 150 existing).

- [ ] **Step 5: Commit**

```bash
git add crates/stateful-cli/src/hook.rs crates/stateful-cli/tests/hook.rs
git commit -m "hook: auto-refresh base observation and retry once on stale under held claim"
```

### Task 2.2 (guard): stale without a held claim still blocks

**Files:** Test only: `crates/stateful-cli/tests/hook.rs`

- [ ] **Step 1: Write the test** — stale on a caller-supplied `reservation_id` (no auto-claim held, `auto_claimed_paths` empty) must still `Block` with `stale_target_observation` and must not silently refresh (prevents clobbering a peer change when the writer does not hold an exclusive auto-claim).

```rust
#[test]
fn omp_write_stale_without_auto_claim_still_blocks() {
    // Fake server: authorize STALE only. Input supplies reservation_id "r-ext" (no auto-declare/claim).
    // Expect OmpHookOutcome::Block whose reason mentions reread; only one authorize consumed.
}
```

- [ ] **Step 2: Run — expect PASS** (Task 2.1 gated the refresh on `!auto_claimed_paths.is_empty()`; this test proves the negative path). If it fails, tighten the guard.

Run: `... --command 'cargo test -p stateful-cli --test hook omp_write_stale_without_auto_claim_still_blocks -- --exact'`

- [ ] **Step 3: Commit**

```bash
git add crates/stateful-cli/tests/hook.rs
git commit -m "hook: assert stale without held claim still blocks (no silent refresh)"
```

---

## Phase 3 — Lever 4: Contention-likely benchmark slice

**Why:** If instances rarely force subagents onto shared files, `on` cannot show score upside by construction. The dataset already carries a per-instance file-count signal (`measured_files_total`/`included_files`), so a "multi-file" slice needs **no dataset change** — just a filter. Run the standard on/off comparison on this slice and check whether any `on` advantage concentrates there.

### Task 3.1: File-count filter in instance selection

**Files:**
- Modify: `crates/stateful-bench/src/denovo.rs` — `denovo_matrix_instance_ids` (1096-1120); add a `min_measured_files: Option<usize>` field to `DeNovoMatrixRunOptions` (940-978) and a CLI flag `--min-measured-files` on the matrix subcommand (`DeNovoCommand`, 97-222).
- Test: `crates/stateful-bench/tests/denovo.rs`

**Interfaces:**
- Produces: `denovo_matrix_instance_ids(data_file, requested_instance_ids, mode, min_measured_files)` — keeps a row only when `min_measured_files` is `None` **or** the row's `measured_files_total` (fallback `included_files`) `>= threshold`.

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn instance_ids_filter_by_min_measured_files() {
    let file = write_temp_jsonl(&[
        serde_json::json!({"instance_id": "one_file", "measured_files_total": 1}),
        serde_json::json!({"instance_id": "many_files", "measured_files_total": 4}),
    ]);
    let ids = denovo_matrix_instance_ids(file.path(), &[], DeNovoRunMode::Batch, Some(3)).unwrap();
    assert_eq!(ids, vec!["many_files".to_string()]);
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `stateful sandbox run --fs build --network enabled --write-dir cargo-target-slice --command 'cargo test -p stateful-bench --test denovo instance_ids_filter_by_min_measured_files'`
Expected: FAIL — arity mismatch.

- [ ] **Step 3: Implement** — add the param and filter inside the existing row loop (denovo.rs:1103-1114):

```rust
if let Some(min) = min_measured_files {
    let files = row.get("measured_files_total")
        .or_else(|| row.get("included_files"))
        .and_then(Value::as_u64).unwrap_or(0) as usize;
    if files < min { continue; }
}
```

Thread `min_measured_files` through `DeNovoMatrixRunOptions` and the CLI flag (default `None` ⇒ standard behavior unchanged). Update the one existing call site of `denovo_matrix_instance_ids` inside `run_denovo_matrix` (1122-1320).

- [ ] **Step 4: Run to verify it passes**

Run: same as Step 2, then `... --command 'cargo test -p stateful-bench --test denovo'`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/stateful-bench/src/denovo.rs crates/stateful-bench/tests/denovo.rs
git commit -m "bench: optional min-measured-files instance filter for contention slice"
```

### Task 3.2: Document the contention-slice comparison

**Files:** Modify `docs/denovo-benchmark-commands.md` (add a `--min-measured-files 3` slice example running both `stateful:off,subagent:on` and `stateful:on,subagent:on`).

- [ ] **Step 1:** Add a command block and a one-paragraph note: interpret with Phase 1 metrics — an advantage is credible only if `on` shows higher score AND `true_collisions_prevented > 0` on the slice. No code; docs only.
- [ ] **Step 2: Commit**

```bash
git add docs/denovo-benchmark-commands.md
git commit -m "docs: contention-slice comparison recipe and interpretation"
```

---

## Phase 4 — Lever 1: Cross-agent findings sharing (GATED)

**Gate:** Build only after Phase 1 metrics on a real run show agents re-deriving knowledge across subagents (e.g. repeated identical exploration, or repeated same-file failures across sibling subagents). If the data does not show re-derivation, **stop** — this lever is speculative and YAGNI until proven. Record the gate decision in `docs/denovo-stateful-coordination-analysis-*.md`.

**Why:** The only path to `on > off` on *score*. Reuses existing plumbing: `append_notification` (free-form payload, no schema change), the SSE/poll delivery already consumed by the OMP extension, and the context renderer. Net-new surface is intentionally minimal: one notification kind + one write path + one render line.

### Task 4.1: Store — record a resource finding and notify overlapping peers

**Files:**
- Create: `crates/stateful-store/src/findings.rs` (mirror `scope_overlap.rs` structure).
- Modify: `crates/stateful-store/src/lib.rs` (add `mod findings;`).
- Test: `crates/stateful-store/tests/event_store.rs`

**Interfaces:**
- Produces: `Store::record_resource_finding(&self, workspace_id, by_agent_id, relative_path, note: &str) -> StoreResult<()>` — writes a `resource_finding` notification (via `append_notification`, kind `"resource_finding"`, payload `{relative_path, note, by_agent_id, source:"finding"}`) to every *other* agent holding an overlapping active reservation scope, reusing `scope_overlap_candidates` and the 120s dedup pattern from `scope_overlap.rs:64-134`.

- [ ] **Step 1: Write the failing test** — declaring reservations for two agents on `src/auth.ts`, then `record_resource_finding("w","s2","src/auth.ts","validation is in verify()")`, asserts `pending_notifications("s1","w")` contains one `resource_finding` with the note; self (`s2`) excluded; non-overlapping agent not notified.
- [ ] **Step 2: Run to verify it fails.** `... --command 'cargo test -p stateful-store --test event_store resource_finding'`
- [ ] **Step 3: Implement** `findings.rs` mirroring `notify_scope_overlap_for_declaration`; reuse `append_notification`. No schema change.
- [ ] **Step 4: Run to verify it passes** (target + full `event_store`).
- [ ] **Step 5: Commit** `git commit -m "store: record_resource_finding notifies overlapping peers (no schema change)"`

### Task 4.2: Server route + extension write path

**Files:** Modify `crates/stateful-server/src/lib.rs` (add `POST /v1/finding` → `record_resource_finding`); `crates/stateful-cli/assets/stateful-omp-extension.js` (handle `resource_finding` SSE event in `processReservationSseBlock` at :368-376, mirroring `scope_overlap`; add a minimal `state_finding` tool that POSTs `{relative_path, note}`); test `crates/stateful-server/tests/routes.rs` and `crates/stateful-cli/assets/stateful-omp-extension.test.mjs`.

- [ ] **Step 1:** Failing route test (holder poll sees `resource_finding` after POST) + failing extension test (`resource_finding` delivered once, deduped, `triggerTurn:false`).
- [ ] **Step 2:** Run — verify fail.
- [ ] **Step 3:** Implement route + extension handler mirroring the `scope_overlap` consumer (`deliverScopeOverlapNotification`, extension.js:372-375) and `seenScopeOverlaps` dedup.
- [ ] **Step 4:** Run — verify pass.
- [ ] **Step 5:** Commit `git commit -m "server+ext: resource_finding route and OMP delivery"`

### Task 4.3: Renderer — surface peer findings

**Files:** Modify `crates/stateful-core/src/context.rs` (`render_prompt_text` 280-388; add findings as a rendered line under Nearby Activity, reusing `CurrentItem` — no struct field add per the enrichment-spec constraint); test `crates/stateful-core/tests/context.rs`.

- [ ] **Step 1:** Failing test — a `resource_finding` item renders `"finding on src/auth.ts: validation is in verify() (by s2)"` within the item cap.
- [ ] **Step 2:** Run — verify fail.
- [ ] **Step 3:** Implement using the existing evidence/section machinery.
- [ ] **Step 4:** Run — verify pass.
- [ ] **Step 5:** Commit `git commit -m "core: render peer resource findings in Nearby Activity"`

---

## Explicitly Deferred / Skipped

- **Per-edit reservation churn refactor** (reuse a session-scoped reservation instead of minting one per edit in `declare_and_claim_omp_pre_tool_reservation`, hook.rs:922-988). Its main harm is metric noise (inflated lifecycle events) + a per-declare overlap scan — **not** score or agent turns. That noise is better removed in Phase 1 (dedupe/label churn in the metric) than by refactoring the reservation lifecycle, which changes reservation semantics (session-scope vs edit-scope) for little gain. Revisit only if a run shows declare latency on the hot path materially hurting wall-clock. `// ponytail: skip lifecycle refactor; fix the metric, not the engine, unless latency proves otherwise.`
- **Syscall-level enforcement (eBPF/FUSE/ptrace)** and **client-state elimination / event-sourcing rewrite** (from the earlier proposal) are rejected: `hook.rs` is 3,878 lines (not 6,000), the server is already the event-sourced authority (`policy_service.rs`, store event log), and syscall interception is a cross-platform rewrite for a benefit the tool-boundary hook already delivers.

## Self-Review

- **Coverage:** Lever 3 → Phase 1; Lever 2 → Phase 2; Lever 4 → Phase 3; Lever 1 → Phase 4. All four proposals mapped.
- **Sequencing honesty:** measurement (Phase 1) precedes optimization and the gated score lever (Phase 4) — updated from the earlier loose "L2→L1" ordering after grounding showed contention is near-zero.
- **Placeholders:** each code task carries real anchors, real symbol names, and representative test/impl code; line numbers are current-as-of 2026-07-06 and the executor must re-ground with `read`/`grep` before editing (numbers drift).
- **Type consistency:** report field names (`orchestration_true_collisions_prevented`, `orchestration_self_inflicted_denials`, `orchestration_scope_overlap_warnings`) and the `orchestration_trace` JSON keys (`true_collisions_prevented`, `self_inflicted_denials`, `scope_overlap_warnings`) match across Tasks 1.1↔1.2; `min_measured_files` matches across Task 3.1 signature and options struct; `resource_finding` kind matches across Tasks 4.1–4.3.
