# Presence-First Event Journal V2 Implementation and Benchmark Plan

> **For agentic workers:** REQUIRED SUB-SKILL: use `subagent-driven-development` or `executing-plans` after the user chooses an execution mode. Use `test-driven-development` for every behavior change, `systematic-debugging` for every unexpected failure, `requesting-code-review` before final integration, `verification-before-completion` before any completion claim, and `running-statefulbench` for Docker qualification or model-backed benchmark work.

**Goal:** Replace the shipped enforcement-first mutable-state implementation with the approved presence-first `stateful.v2` event journal; preserve legacy product state transactionally; deliver live presence, exact-read freshness, and structured handoffs automatically; make awareness the default; selectively port the real-world StatefulBench runner; and clear one qualified `requests` trial in all three arms.

**Architecture:** `stateful-core` owns v2 identities, protocol DTOs, typed event families, fingerprints, presence/handoff models, policy primitives, and context rendering. `stateful-store` owns the append-only journal, command receipts, projection-only mutation, replay, migration, TTL transitions, and SQLite transactions. `stateful-server` owns typed v2 routes and orchestration over a transaction-scoped projection reader. `stateful-cli` owns CLI, Codex/OMP/IDE adapters, exact tool-operation correlation, recovery outbox, context delivery, runtime discovery, and diagnostics. `stateful-bench` reads sanitized v2 journal diagnostics and never owns product policy.

**Tech stack:** Rust 2024, Axum, rusqlite/SQLite backup API, serde, UUID v5/v4, SHA-256, time, clap, Tokio, tower, JavaScript ESM/Node test runner, Python 3.14 unittest, Docker Linux/arm64, OMP, StatefulBench real-world corpus.

**Approved spec:** `docs/superpowers/specs/2026-07-15-presence-first-event-journal-v2-design.md`

---

## Scope Check

This is one coordinated cutover rather than independent subprojects. The protocol envelope, event metadata, projection schema, runtime adapters, context cursor, diagnostics, and benchmark metrics share identity and version contracts. Porting the runner before the v2 diagnostics exist would preserve the wrong schema; changing default mode before freshness and fence hard stops exist would weaken safety. Tasks therefore establish shared types first, then journal/projections, migration, product behavior, adapters, smoke proof, runner, documentation, and finally the credit-bearing run.

No dev-branch coordination core, CLI, docs, or policy code is merged. Only the runner/corpus paths named in Task 13 are copied from `dev` at `b5e875e`, then adapted to current v2 contracts.

## Locked Cross-Task Contracts

### Protocol

POST bodies use exactly this generic shape:

```rust
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RequestEnvelope<T> {
    pub protocol_version: ProtocolVersion, // only stateful.v2
    pub request_id: Uuid,
    #[serde(with = "time::serde::rfc3339")]
    pub observed_at: OffsetDateTime,
    pub agent: AgentIdentity,
    pub workspace: WorkspaceIdentity,
    pub source: SourceRef,
    pub payload: T,
}
```

```rust
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QueryEnvelope<Q> {
    pub protocol_version: ProtocolVersion,
    pub request_id: Uuid,
    #[serde(with = "time::serde::rfc3339")]
    pub observed_at: OffsetDateTime,
    #[serde(flatten)]
    pub agent: AgentIdentity,
    #[serde(flatten)]
    pub workspace: WorkspaceIdentity,
    #[serde(flatten)]
    pub source: SourceRef,
    #[serde(flatten)]
    pub query: Q,
}
```

GET routes deserialize the same scalar fields through `#[serde(flatten)]` into `QueryEnvelope<Q>`; no server-side identity fallback is permitted. `request_id` is a client-generated UUID. Mutations store a frozen response in `command_receipts`; duplicate UUIDs return that response without policy or projector execution.

The complete v2 surface is fixed:

```text
POST /v2/session/register
POST /v2/presence/update
POST /v2/read/start
POST /v2/read/complete
POST /v2/write/complete
POST /v2/activity/finalize
POST /v2/reservation/declare
POST /v2/reservation/request
POST /v2/reservation/claim
POST /v2/reservation/cancel
POST /v2/claim/acquire
POST /v2/claim/release
POST /v2/authorize
POST /v2/human/observe
POST /v2/human/save-check
POST /v2/reconcile/ack
POST /v2/context/render
POST /v2/context/ack
POST /v2/notifications/poll
POST /v2/resume/next
POST /v2/outbox/sync
GET  /v2/current
GET  /v2/events
GET  /v2/notifications/stream
GET  /v2/runtime/identity
```

`presence/update` carries a typed update kind for heartbeat, goal, phase, plan, resources, tool start, or tool result. `write/complete`, context ACK, outbox sync, and read lifecycle routes are hook/internal protocol, not extra model-facing tool names.

V2 errors use:

```json
{
  "protocol_version": "stateful.v2",
  "request_id": "8d5ddf45-9ce3-44ac-953e-3b776cd1783d",
  "error": {
    "code": "stale_read_observation",
    "message": "src/lib.rs changed after the completed exact read.",
    "required_next_action": "Reread src/lib.rs, then retry the write."
  }
}
```

### Journal and commands

`journal_events.event_seq INTEGER PRIMARY KEY AUTOINCREMENT` is the workspace-global order. Event IDs are UUID v5 values derived from `(request_id, event ordinal, event type)`; migration seed IDs use a separate fixed namespace and `(legacy entity kind, legacy primary key)`. A command runs once under `BEGIN IMMEDIATE`:

```text
lookup command_receipt
  -> validate through transaction-scoped ProjectionReader
  -> append typed events
  -> apply every event with Projector
  -> update workspace_version only for affects_context events
  -> insert command receipt
  -> COMMIT
```

`command_receipts` is transactional idempotency bookkeeping, not canonical coordination truth. Every table ending in `_current`, plus `workspace_version` and `agent_context_cursor`, is canonical projection state and may be mutated only by `projector.rs`.

Typed event families are exhaustive:

```rust
pub enum EventPayload {
    Migration(MigrationEvent),
    Presence(PresenceEvent),
    Reservation(ReservationEvent),
    Claim(ClaimEvent),
    Wait(WaitEvent),
    WriteFence(WriteFenceEvent),
    ReadObservation(ReadObservationEvent),
    WriteIntent(WriteIntentEvent),
    HumanObservation(HumanObservationEvent),
    Handoff(HandoffEvent),
    Authorization(AuthorizationEvent),
    Context(ContextEvent),
    Notification(NotificationEvent),
    Recovery(RecoveryEvent),
}
```

The family variants are fixed as follows:

```text
Migration: Started, LegacyAuditImported, PresenceSnapshotSeeded,
  ReservationSnapshotSeeded, ClaimSnapshotSeeded, WaitSnapshotSeeded,
  WriteFenceSnapshotSeeded, HumanObservationSnapshotSeeded,
  LegacyHandoffSnapshotSeeded, DeliverySnapshotSeeded, Validated, Completed
Presence: Registered, Heartbeat, GoalUpdated, PhaseUpdated, PlanUpdated,
  ResourcesUpdated, ToolStarted, ToolCompleted, Finalized, Expired
Reservation: Declared, Refreshed, Released, Expired
Claim: Acquired, ObservationRefreshed, Released, Expired
Wait: Requested, BecameClaimable, Claimed, Cancelled, Expired
WriteFence: Acquired, ConflictObserved, Released, Expired
ReadObservation: Started, Stabilized, Unstable, Aborted, Invalidated, Expired
WriteIntent: Started, Committed, Failed, OutcomeUnknown, Reconciled
HumanObservation: Observed, Reconciled, Expired
Handoff: Finalized, Expired
Authorization: Allowed, Warned, Denied, OverrideGranted
Context: Rendered, DeliveryCreated, DeliveryAcknowledged, DeliverySuperseded
Notification: Created, Delivered, Expired, Coalesced
Recovery: Queued, Delivered, Failed
```

Each event constructor derives `aggregate_kind`, `aggregate_id`, `event_type`, and `affects_context`; callers cannot supply mismatched strings. Heartbeats, repeated identical resource reads, renders, delivery, ACKs, and allow-only audits have `affects_context=false`.

### Fingerprints and exact reads

All v2 fingerprints are streaming SHA-256 records:

```rust
pub struct ContentFingerprint {
    pub exists: bool,
    pub byte_len: u64,
    pub sha256: Option<String>,
}
```

Legacy FNV hashes are retained only as `legacy_base_observation` migration provenance and never stabilize a v2 read. A read stabilizes only when one runtime operation ID has: full-file/no-range/no-summary/no-truncation classification, successful tool result, and identical before/after fingerprints. Search, glob, directory reads, structural summaries, selected ranges, truncated output, failed tools, and ambiguous adapter payloads update presence only.

A pending post-hook write is recovered deterministically: unchanged filesystem state records `OutcomeUnknown(no_change)` and releases the safety block; changed or missing/created state records `OutcomeUnknown(changed)` and remains a hard stop until an explicit reconcile ACK references that write intent.

### Presence and handoff limits

```text
presence TTL: 15 minutes
busy_until cap: 60 minutes from tool start
read observation TTL: 60 minutes from stabilization
explicit handoff relevance: 7 days
fallback handoff relevance: 24 hours
goal excerpt: 240 Unicode scalar values after whitespace normalization
last-result summary: 240 Unicode scalar values
handoff summary: 2,000 Unicode scalar values
handoff list: at most 100 entries per list
brief context: at most 8 items and 1,200 Unicode scalar values
```

### Context cursor

`workspace_version` is the highest context-affecting event sequence. Render selects aggregates changed after `after_version`, then renders each aggregate's current final projection. Render never advances a cursor. A matching ACK advances `agent_context_cursor`; an unacknowledged delivery is redelivered. `after_version > workspace_version` returns `invalid_context_cursor`. The first v2 release never reports compaction reset.

### Benchmark metrics

`sequential` and `parallel-off` rows use `coordination_metrics: null`. A cleared `parallel-on` row must emit this value-free shape; missing fields make the row uncleared:

```json
{
  "protocol_version": "stateful.v2",
  "journal": {"events": 0, "bytes_start": 0, "bytes_end": 0, "bytes_growth": 0, "by_event_type": {}},
  "presence": {"registered": 0, "expired": 0, "finalized": 0, "peak_active": 0},
  "handoffs": {"explicit": 0, "fallback_stop": 0, "fallback_ttl": 0, "by_status": {}},
  "read_observations": {"started": 0, "stable": 0, "unstable": 0, "aborted": 0, "invalidated": 0},
  "context": {"versions": 0, "renders": 0, "deliveries": 0, "acks": 0, "redeliveries": 0, "coalesced": 0, "prompt_utf8_bytes": 0, "prompt_unicode_scalars": 0, "prompt_items": 0},
  "authorization": {"warned_by_reason": {}, "denied_by_reason": {}},
  "write_safety": {"fence_conflicts": 0, "unknown_outcomes": 0, "same_path_overlaps": 0, "cross_agent_overwrites": 0},
  "notifications": {"by_kind": {}},
  "waits": {"by_final_status": {}, "grant_wait_time_s": {"count": 0, "total": 0.0, "mean": null, "max": null}, "unmeasured_grants": 0}
}
```

`same_path_overlaps` counts distinct operation IDs rejected by an active same-path fence. `cross_agent_overwrites` counts committed writes whose immediately preceding committed writer for that path is another agent; it is descriptive, not semantic-loss proof. Published output contains no IDs, paths, payloads, timestamps, messages, or raw rows.

---

## File Map

### Core

- Modify: `Cargo.toml`, `crates/stateful-core/Cargo.toml`
- Modify: `crates/stateful-core/src/lib.rs`, `types.rs`, `policy.rs`, `context.rs`
- Create: `crates/stateful-core/src/fingerprint.rs`, `journal.rs`, `presence.rs`, `protocol.rs`
- Create: `crates/stateful-core/tests/v2_protocol.rs`, `journal.rs`, `fingerprint.rs`, `presence.rs`

### Store

- Modify: `crates/stateful-store/Cargo.toml`, `src/lib.rs`
- Modify then remove obsolete mutation paths from: `src/activity.rs`, `claims.rs`, `human.rs`, `notifications.rs`, `reservations.rs`, `write_fences.rs`
- Create: `crates/stateful-store/src/clock.rs`, `schema.rs`, `journal.rs`, `projector.rs`, `migration.rs`, `presence.rs`, `handoff.rs`, `observations.rs`, `write_intents.rs`, `context_delivery.rs`
- Split coverage from `crates/stateful-store/tests/event_store.rs` into `journal_v2.rs`, `migration_v2.rs`, `presence_handoff.rs`, `coordination_projections.rs`, `freshness.rs`, and `context_delivery.rs`
- Create: `crates/stateful-store/tests/fixtures/v1_persistent_state.sql`

### Server

- Modify: `crates/stateful-server/src/lib.rs`, `policy_service.rs`, `protocol.rs`
- Create: `crates/stateful-server/src/routes_v2.rs`, `commands.rs`, `context_service.rs`, `diagnostics.rs`
- Split v2 coverage from `crates/stateful-server/tests/routes.rs` into `v2_protocol.rs`, `presence.rs`, `freshness.rs`, `context_delivery.rs`, and `diagnostics.rs`

### CLI and integrations

- Modify: `crates/stateful-cli/src/lib.rs`, `runtime.rs`, `hook.rs`, `install.rs`, `outbox.rs`, `watch.rs`, `server_lifecycle.rs`
- Create: `crates/stateful-cli/src/hook/input.rs`, `observation.rs`, `write_lifecycle.rs`, `delivery.rs`
- Modify: `crates/stateful-cli/assets/stateful-omp-extension.js`, `omp-stateful-required-rule.md`, and `stateful-command-policy/**`
- Create: `crates/stateful-cli/assets/stateful-omp-extension.test.mjs`
- Modify: `crates/stateful-cli/tests/hook.rs`, `install_global.rs`, `runtime.rs`, `outbox.rs`, `server_lifecycle.rs`, `cli.rs`
- Create: `crates/stateful-cli/tests/v2_two_session.rs`
- Modify: `integrations/vscode/extension.js`, `lib/core.js`, `test/core.test.js`

### StatefulBench selective port

- Create from `dev@b5e875e`: `crates/stateful-bench/docker/statefulbench-realworld.Dockerfile`
- Create from `dev@b5e875e`: `crates/stateful-bench/scripts/statefulbench_container_diagnostics.py`, `statefulbench_container_entry.py`, `statefulbench_docker.py`, `statefulbench_lite.py`, `statefulbench_realworld.py`
- Create/modify from `dev@b5e875e`: `crates/stateful-bench/scripts/tests/{__init__.py,conftest.py,test_programbench_agents.py,test_statefulbench_docker.py,test_statefulbench_lite.py,test_statefulbench_realworld.py,test_reports.py}`
- Modify only where the imported runner requires it: `crates/stateful-bench/scripts/denovo_codex_agent.py`, `denovo_progress_report.py`, `programbench_codex_agent.py`, `programbench_omp_agent.py`, `src/denovo.rs`, `src/programbench.rs`, `tests/cli.rs`, `tests/denovo.rs`, `tests/programbench.rs`
- Create unchanged frozen tree from `dev@b5e875e`: `datasets/statefulbench-realworld/**`

### Documentation and operator skill

- Modify: `README.md`, `CHANGELOG.md`
- Modify: `docs/core-concept.md`, `architecture.md`, `state-model.md`, `implementation-contract.md`, `usage-reference.md`, `current-state-coordination.md`, `current-state-example.svg`, `adr/0001-state-first-not-memory-first.md`, `adr/0002-presence-first-not-lock-first.md`
- Create from dev then update: `docs/statefulbench-realworld-design.md`, `docs/superpowers/plans/2026-07-12-statefulbench-docker-runtime.md`
- Create: `docs/statefulbench-realworld.md`
- Modify operator-local installed skill: `skill://running-statefulbench`

---

### Task 1: Freeze V2 Domain, Protocol, Event, and Fingerprint Types

**Files:** core files and tests listed under “Core”; `Cargo.toml`; `crates/stateful-core/Cargo.toml`.

- [ ] **Step 1: Write failing core contract tests**

Add tests with these exact names and assertions:

```rust
#[test] fn v2_post_envelope_round_trips_full_identity_and_payload();
#[test] fn query_envelope_requires_explicit_agent_and_workspace_identity();
#[test] fn v1_protocol_value_is_rejected();
#[test] fn migrated_actor_type_accepts_unknown();
#[test] fn event_constructor_derives_kind_aggregate_and_context_effect();
#[test] fn event_payload_rejects_kind_payload_mismatch();
#[test] fn sha256_fingerprint_distinguishes_missing_empty_and_nonempty_files();
#[test] fn goal_excerpt_normalizes_whitespace_and_counts_unicode_scalars();
#[test] fn handoff_limits_reject_instead_of_truncating();
```

Use a payload `PresenceUpdate { goal_excerpt: Some("fix  auth\n flow".into()), ..Default::default() }`; assert JSON contains `"protocol_version":"stateful.v2"`, full actor lineage, full workspace identity, and nested `payload`. Assert parsing `stateful.v1` returns `unsupported_protocol`. Fingerprint expected values are the standard SHA-256 digests for empty bytes and `b"stateful"`.

- [ ] **Step 2: Run the tests and observe RED**

```sh
stateful sandbox run --fs build --network disabled --write-dir v2-core-red \
  --command 'cargo test -p stateful-core --test v2_protocol --test journal --test fingerprint --test presence'
```

Expected: compile failures for missing v2 modules/types and `ProtocolVersion::V2`.

- [ ] **Step 3: Implement the core contract**

Use `lsp references` before changing exported `ActivityPhase`, `ProtocolVersion`, or `RequestEnvelope`. Rename `ActivityPhase` to `PresencePhase` through LSP and migrate every callsite; do not leave an alias. Add `ActorType::Unknown`, `ProtocolVersion::V2`, generic `RequestEnvelope<T>`, flattened `QueryEnvelope<Q>`, `V2Error`, `ContentFingerprint`, the presence/handoff records and limits, `StoredEvent`, `NewEvent`, and all event-family enums from the locked contract.

Use this fingerprint API and stream fixed-size buffers rather than allocating the full file:

```rust
pub fn fingerprint_path(path: &Path) -> io::Result<ContentFingerprint>;
pub fn fingerprint_reader(reader: impl Read) -> io::Result<ContentFingerprint>;
```

Enable `sha2 = "0.10"`, `uuid` features `serde`, `v4`, and `v5`, and `time` serde/formatting/parsing. Constructors validate UUID request IDs, nonempty identity components, RFC3339 timestamps, normalized relative paths, scalar limits, and matching payload/event metadata.

- [ ] **Step 4: Run core tests GREEN**

Run the Step 2 command. Expected: all four test binaries pass. Then run existing core tests:

```sh
stateful sandbox run --fs build --network disabled --write-dir v2-core-green \
  --command 'cargo test -p stateful-core'
```

Expected: pass with no `ActivityPhase` or `stateful.v1` references in core.

- [ ] **Step 5: Commit and push**

Commit only Task 1 files as `feat: define stateful v2 domain contracts`, then push the feature branch.

---

### Task 2: Build the Atomic Journal, Receipt, and Projector Foundation

**Files:** store `clock.rs`, `schema.rs`, `journal.rs`, `projector.rs`, `lib.rs`, Cargo files, and `tests/journal_v2.rs`.

- [ ] **Step 1: Write failing journal transaction tests**

Create these behavioral tests:

```rust
#[test] fn command_appends_projects_versions_receipts_and_commits_atomically();
#[test] fn duplicate_request_returns_frozen_response_without_new_events();
#[test] fn request_id_reuse_with_different_route_identity_or_payload_is_rejected();
#[test] fn projector_failure_rolls_back_journal_projection_version_and_receipt();
#[test] fn audit_only_event_does_not_advance_workspace_version();
#[test] fn replay_into_empty_projection_tables_is_byte_equivalent();
#[test] fn event_sequence_and_id_are_stable_and_unique();
```

The rollback test injects a test-only projector failure on the second event and asserts zero rows in all four affected surfaces. The replay test snapshots every projection table ordered by primary key, clears only projections, calls replay, and compares serialized rows byte-for-byte.

- [ ] **Step 2: Run RED**

```sh
stateful sandbox run --fs build --network disabled --write-dir v2-journal-red \
  --command 'cargo test -p stateful-store --test journal_v2'
```

Expected: missing `Store::execute_command`, `Store::rebuild_projections`, and v2 schema symbols.

- [ ] **Step 3: Create schema and command API**

Enable rusqlite features `bundled` and `backup`. Create `journal_events`, `command_receipts`, every locked projection table, `resource_write_current`, `migration_current`, and indexes on workspace/event sequence, workspace/event type, aggregate identity, active expiry, operation ID, resource path, and notification target/version.

Use these public store interfaces:

```rust
pub trait ProjectionReader {
    fn workspace_version(&self, workspace_id: &str) -> StoreResult<u64>;
    fn presence(&self, workspace_id: &str, agent_id: &str) -> StoreResult<Option<PresenceRecord>>;
    fn active_claims_for_path(&self, workspace_id: &str, path: &str) -> StoreResult<Vec<ClaimRecord>>;
    fn active_fence_for_path(&self, workspace_id: &str, path: &str) -> StoreResult<Option<WriteFenceRecord>>;
    fn stable_observation(&self, workspace_id: &str, agent_id: &str, path: &str) -> StoreResult<Option<ReadObservationRecord>>;
}

pub struct CommandPlan<R> {
    pub events: Vec<NewEvent>,
    pub response: R,
    pub http_status: u16,
}

pub struct CommandOutcome<R> {
    pub response: R,
    pub http_status: u16,
    pub first_event_seq: Option<u64>,
    pub last_event_seq: Option<u64>,
    pub duplicate: bool,
}

pub struct ReplayReport {
    pub projectable_events: u64,
    pub projection_rows: u64,
    pub canonical_sha256: String,
}

impl Store {
    pub fn execute_command<R>(
        &mut self,
        request: &RequestEnvelope<impl Serialize>,
        route_kind: &'static str,
        build: impl FnOnce(&dyn ProjectionReader) -> StoreResult<CommandPlan<R>>,
    ) -> StoreResult<CommandOutcome<R>>
    where R: Serialize + DeserializeOwned + Clone;

    pub fn rebuild_projections(&mut self) -> StoreResult<ReplayReport>;
}
```

`execute_command` uses one `TransactionBehavior::Immediate`, checks the receipt first, rejects any UUID reuse whose route kind, actor/workspace identity, or normalized request SHA-256 differs with `idempotency_key_reused`, assigns deterministic event IDs, inserts each journal row, deserializes the stored row, invokes `Projector::apply`, updates `workspace_version` for context events, inserts the frozen receipt, and commits. An exact duplicate returns the frozen receipt without rerunning policy or projectors. No caller receives a committed response before commit succeeds.

- [ ] **Step 4: Implement deterministic replay and clocks**

Add `Clock`, `SystemClock`, and `FixedClock`; all timestamps and TTL calculations use the injected clock. `rebuild_projections` creates empty replay tables, applies projectable events in `event_seq` order with the production projector, validates origin sequences, swaps only after equality checks, and never touches receipts or the journal.

- [ ] **Step 5: Run GREEN and existing store regression tests**

Run the Step 2 command, then:

```sh
stateful sandbox run --fs build --network disabled --write-dir v2-journal-green \
  --command 'cargo test -p stateful-store'
```

Expected: journal tests and existing store tests pass.

- [ ] **Step 6: Commit and push**

Commit as `feat: add atomic event journal and projectors`, then push.

---

### Task 3: Migrate Persistent Legacy Databases Through Snapshot Seeds

**Files:** `schema.rs`, `migration.rs`, `lib.rs`, `tests/migration_v2.rs`, `tests/fixtures/v1_persistent_state.sql`.

- [ ] **Step 1: Write the legacy fixture and failing migration tests**

The SQL fixture must contain two agents, tied live activities, active and expired reservations/claims/waits/fences, reconciled and unreconciled human observations, pending/delivered notifications, outbox rows, recent `ActivityFinalized`, legacy events with equal timestamps, and a schema checkpoint matching shipped v1.

Add:

```rust
#[test] fn persistent_v1_db_is_backed_up_seeded_replayed_and_cut_over();
#[test] fn tied_activities_choose_latest_expiry_then_activity_id();
#[test] fn legacy_claim_hash_is_not_read_provenance();
#[test] fn unavailable_actor_and_handoff_fields_remain_unknown_or_empty();
#[test] fn malformed_legacy_json_rolls_back_and_preserves_original_schema();
#[test] fn migration_rerun_after_checkpoint_is_a_no_op();
#[test] fn new_and_in_memory_databases_skip_legacy_migration();
```

Verify backup bytes open as the original schema, backup permissions equal the source permissions, imported audits order by `(created_at,event_id)`, seeds have deterministic IDs, and cutover removes obsolete current tables only after replay equality.

- [ ] **Step 2: Run RED**

```sh
stateful sandbox run --fs build --network disabled --write-dir v2-migration-red \
  --command 'cargo test -p stateful-store --test migration_v2'
```

Expected: migration entry points and checkpoint absent.

- [ ] **Step 3: Implement exclusive preflight, backup, shadow replay, and cutover**

`Store::open_persistent` performs: detect legacy/checkpoint; `PRAGMA quick_check` and foreign-key/schema validation; acquire exclusive SQLite migration ownership; create a versioned SQLite backup through `rusqlite::backup::Backup`; copy source permissions; create `_v2_shadow` journal/projection tables; import legacy audits as `MigrationEvent::LegacyAuditImported`; append deterministic seed events; run production projectors; replay into a second empty projection set; compare canonical rows; append `Validated` and `Completed`; rename shadow tables; record `stateful.v2.event-journal`; drop legacy current tables; commit; then run post-commit readiness checks.

Every allowed normalization is serialized in `MigrationCompleted.manifest`; any other row difference returns `StoreError::MigrationValidation` before cutover. A crash/failure before commit leaves legacy tables authoritative and the backup intact.

- [ ] **Step 4: Run GREEN plus reopen tests**

Run Step 2 and open the migrated fixture twice. Expected: first open migrates, second open appends no events and creates no second backup.

- [ ] **Step 5: Commit and push**

Commit as `feat: migrate legacy state into journal seeds`, then push.

---

### Task 4: Implement First-Class Presence and Structured Handoffs

**Files:** core presence/journal/context files; store `presence.rs`, `handoff.rs`, `projector.rs`, `activity.rs`, `lib.rs`; `tests/presence_handoff.rs`.

- [ ] **Step 1: Write failing presence and handoff tests**

```rust
#[test] fn registration_upserts_one_presence_per_workspace_and_agent();
#[test] fn registration_preserves_root_subagent_human_system_and_unknown_attribution();
#[test] fn first_prompt_captures_normalized_goal_and_explicit_update_replaces_it();
#[test] fn resource_relations_are_idempotent_and_semantic_changes_version_once();
#[test] fn busy_tool_defers_expiry_but_never_beyond_sixty_minutes();
#[test] fn explicit_handoff_finalizes_presence_and_cleans_coordination_in_one_transaction();
#[test] fn stop_without_explicit_handoff_creates_unknown_fallback();
#[test] fn ttl_expiry_lazily_creates_the_same_fallback_once();
#[test] fn handoff_validation_rejects_over_limit_lists_and_summary();
#[test] fn explicit_and_fallback_handoffs_expire_after_their_distinct_windows();
```

Assert explicit finalization emits correlated `Handoff::Finalized`, `Presence::Finalized`, and cleanup events; no partial cleanup survives an injected failure.

- [ ] **Step 2: Run RED**

```sh
stateful sandbox run --fs build --network disabled --write-dir v2-presence-red \
  --command 'cargo test -p stateful-store --test presence_handoff'
```

- [ ] **Step 3: Implement presence commands and projectors**

Add commands for register/resume, heartbeat, goal, phase, plan, resources, tool start/result, explicit finalize, Stop fallback, and lazy/maintenance expiry. `PresenceRecord` stores scalar state; `presence_resource_current` stores one normalized `(workspace,agent,path,relation)` row. Repeated heartbeat and repeated identical relation refresh expiry/observation timestamps without context-version advancement.

Tool results store only `{tool_name,outcome,completed_at,summary?}`. Recognized test commands set `PresencePhase::Testing`; other tools do not infer domain plans. Attribution comes only from request identity.

- [ ] **Step 4: Implement handoff cleanup and relevance**

Create explicit/fallback handoff projectors and resource rows. Stop and TTL call one idempotent finalization command. Explicit handoff wins over fallback. Fallback copies changed resources, next plan, and bounded last result without inventing tests or status. Lazy expiry runs on startup and relevant queries in addition to maintenance.

- [ ] **Step 5: Run GREEN and regression tests**

Run Step 2 and `cargo test -p stateful-store`. Expected: pass.

- [ ] **Step 6: Commit and push**

Commit as `feat: persist presence and structured handoffs`, then push.

---

### Task 5: Route Reservations, Claims, Waits, Fences, Human State, and Delivery Through Events

**Files:** existing store aggregate modules, `journal.rs`, `projector.rs`, `lib.rs`; `tests/coordination_projections.rs`; existing `event_store.rs` cases migrated by domain.

- [ ] **Step 1: Write failing aggregate transition tests**

Cover every shipped transition with journal and origin-sequence assertions:

```rust
#[test] fn reservation_lifecycle_is_event_sourced_and_replayable();
#[test] fn claim_lifecycle_and_observation_refresh_are_event_sourced();
#[test] fn wait_fifo_grant_and_cancel_are_event_sourced();
#[test] fn fence_conflict_release_and_expiry_are_event_sourced();
#[test] fn human_observe_reconcile_and_expire_are_event_sourced();
#[test] fn notification_create_coalesce_deliver_and_expire_are_event_sourced();
#[test] fn aggregate_failure_rolls_back_a_multi_event_transition();
```

- [ ] **Step 2: Run RED against the journal**

```sh
stateful sandbox run --fs build --network disabled --write-dir v2-aggregates-red \
  --command 'cargo test -p stateful-store --test coordination_projections'
```

Expected: existing methods mutate legacy tables without v2 journal rows.

- [ ] **Step 3: Convert each public method into command planning plus projection**

Keep externally useful method names only until server cutover, but replace their SQL bodies with `execute_command` plans. Move all `INSERT`, `UPDATE`, and `DELETE` statements for canonical tables into `projector.rs`. Expiration is represented by typed events, never silent SQL deletion. FIFO wait ordering remains `(requested_at,wait_id)`; grant events preserve reservation purpose and linkage.

Awareness overlap creates `Authorization::Warned` plus coalesced context notification, never a wait. Enforcement conflict creates the existing wait/claimable transitions.

- [ ] **Step 4: Split the monolithic store test file while preserving behavior**

Move domain tests from `event_store.rs` into the named test files; delete moved copies. Do not rewrite assertions to source-text checks. Run each new test binary and the remaining `event_store` binary.

- [ ] **Step 5: Verify no aggregate regression**

```sh
stateful sandbox run --fs build --network disabled --write-dir v2-aggregates-green \
  --command 'cargo test -p stateful-store'
```

Expected: pass.

- [ ] **Step 6: Commit and push**

Commit as `refactor: project coordination state from events`, then push.

---

### Task 6: Add Exact-Read Observations, Write Intents, Resource Versions, and Thin Safety

**Files:** core fingerprint/journal/policy; store `observations.rs`, `write_intents.rs`, `projector.rs`; server `policy_service.rs`; store/server freshness tests.

- [ ] **Step 1: Write failing freshness and recovery tests**

```rust
#[test] fn exact_successful_unchanged_read_stabilizes_observation();
#[test] fn partial_truncated_structural_and_failed_reads_never_stabilize();
#[test] fn file_change_during_read_records_unstable_observation();
#[test] fn stable_equal_observation_allows_in_both_modes();
#[test] fn stable_changed_observation_denies_in_both_modes();
#[test] fn missing_or_expired_observation_warns_and_allows_subject_to_hard_stops();
#[test] fn authorize_starts_intent_and_fence_in_one_transaction();
#[test] fn committed_write_updates_resource_version_and_invalidates_other_observations();
#[test] fn failed_write_releases_intent_and_fence();
#[test] fn missing_post_hook_with_unchanged_file_resolves_unknown_no_change();
#[test] fn missing_post_hook_with_changed_file_blocks_until_matching_reconcile_ack();
#[test] fn same_path_fence_human_write_invalid_target_and_server_failure_remain_hard_stops();
```

Use two concurrent operation IDs on one path to prove pairing never uses “latest path” heuristics.

- [ ] **Step 2: Run RED**

```sh
stateful sandbox run --fs build --network disabled --write-dir v2-freshness-store-red \
  --command 'cargo test -p stateful-store --test freshness'
stateful sandbox run --fs build --network disabled --write-dir v2-freshness-server-red \
  --command 'cargo test -p stateful-server --test freshness'
```

- [ ] **Step 3: Implement read start/complete and observation projection**

`ReadStart` records before fingerprint and operation ID. `ReadComplete` requires the same actor/workspace/path/operation ID and an adapter classification. Success plus identical before/after yields `Stabilized`; changed yields `Unstable`; tool error yields `Aborted`. Stabilized observations expire at 60 minutes and are invalidated by another actor's committed write or session finalization.

- [ ] **Step 4: Implement authorization/write lifecycle atomically**

Before a file-changing tool, compute current fingerprint, evaluate transaction-scoped projections, and emit `WriteIntent::Started` plus `WriteFence::Acquired` under one correlation. Return the intent ID to the adapter. Post success sends pre/post fingerprints and emits `Committed`, resource writer/version, presence changed/result, observation invalidations, intent resolution, and fence release in one command. Post failure emits `Failed` and release.

On startup/pre-tool, inspect expired pending intents. Unchanged state resolves no-change. Changed state remains `OutcomeUnknown(changed)`; only reconcile ACK with the exact intent ID and a fresh exact read can emit `Reconciled` and release the block.

- [ ] **Step 5: Refactor policy service over `ProjectionReader`**

Keep mode-independent hard-stop ordering:

```text
invalid target/security -> unknown write outcome -> stale stable observation
-> active fence -> unreconciled high-confidence human write
-> broad reservation/claim/phase/read-missing policy
```

Awareness softens only the final broad group. Enforcement preserves deny/queue/lazy behavior. Authorization allow audits are non-context events; warnings/denials carry reason codes.

- [ ] **Step 6: Run GREEN**

Run Step 2, then all store/server tests. Expected: pass.

- [ ] **Step 7: Commit and push**

Commit as `feat: enforce exact-read freshness and write intents`, then push.

---

### Task 7: Implement Versioned Context Rendering, Delivery, ACK, and Coalescing

**Files:** core `context.rs`; store `context_delivery.rs`, `projector.rs`, notification code; server `context_service.rs`; core/store/server context tests.

- [ ] **Step 1: Write failing version/delivery tests**

```rust
#[test] fn meaningful_state_change_advances_workspace_version();
#[test] fn heartbeat_render_delivery_ack_and_identical_read_do_not_advance_version();
#[test] fn delta_render_returns_current_final_state_not_raw_events();
#[test] fn changed_but_now_irrelevant_entity_returns_empty_delivery_for_ack();
#[test] fn render_without_ack_redelivers_same_delivery();
#[test] fn ack_advances_only_matching_agent_workspace_delivery();
#[test] fn newer_version_supersedes_and_coalesces_old_notification();
#[test] fn duplicate_and_out_of_order_notifications_do_not_regress_cursor();
#[test] fn brief_context_prioritizes_hard_actions_overlap_handoff_presence_then_info();
#[test] fn brief_context_enforces_eight_item_and_twelve_hundred_scalar_limits();
#[test] fn resource_filter_includes_relevant_handoff_and_nearby_activity();
```

- [ ] **Step 2: Run RED**

```sh
stateful sandbox run --fs build --network disabled --write-dir v2-context-core-red \
  --command 'cargo test -p stateful-core --test context'
stateful sandbox run --fs build --network disabled --write-dir v2-context-store-red \
  --command 'cargo test -p stateful-store --test context_delivery'
stateful sandbox run --fs build --network disabled --write-dir v2-context-server-red \
  --command 'cargo test -p stateful-server --test context_delivery'
```

- [ ] **Step 3: Implement delta selection and final-state rendering**

Query distinct `(aggregate_kind,aggregate_id)` from context-affecting events in `(after_version,current_version]`, then read current projection rows and apply identity/resource relevance. Produce `from_version`, `workspace_version`, `changed`, `reset_required=false`, `delivery_id`, structured items, and prompt text. Reject cursors ahead of current version. Count Unicode scalars after priority ordering; never slice UTF-8 bytes.

- [ ] **Step 4: Implement delivery, ACK, and notification projectors**

Render appends audit-only `Context::Rendered`; changed responses create or redeliver `DeliveryCreated`. ACK emits `DeliveryAcknowledged` and advances `agent_context_cursor` monotonically. A newer target version supersedes an unacknowledged older delivery and coalesces one `context_invalidated(target_version)` notification per target actor/workspace. Empty changed deliveries still require ACK but inject no prompt.

- [ ] **Step 5: Run GREEN and context regressions**

Run Step 2 and all core/store/server tests. Expected: pass.

- [ ] **Step 6: Commit and push**

Commit as `feat: deliver versioned coordination context`, then push.

---

### Task 8: Cut the HTTP Server and Runtime Handshake to V2

**Files:** server source modules and split route tests.

- [ ] **Step 1: Write failing v2 route and handshake tests**

Test every route in the locked complete v2 surface, plus:

```rust
#[tokio::test] async fn v1_routes_are_absent();
#[tokio::test] async fn v2_body_with_v1_protocol_returns_unsupported_protocol();
#[tokio::test] async fn get_queries_reject_missing_identity();
#[tokio::test] async fn duplicate_mutation_returns_identical_frozen_response();
#[tokio::test] async fn runtime_identity_reports_v2_schema_mode_version_and_capabilities();
#[tokio::test] async fn health_is_not_ready_until_migration_and_replay_checks_pass();
#[tokio::test] async fn awareness_is_default_and_enforcement_is_explicit();
```

- [ ] **Step 2: Run RED**

```sh
stateful sandbox run --fs build --network disabled --write-dir v2-server-red \
  --command 'cargo test -p stateful-server --test v2_protocol'
```

Expected: 404 for v2 routes and existing v1 routes still present.

- [ ] **Step 3: Split router wiring from handlers and register only v2**

`lib.rs` retains `ServerConfig`, readiness, listener, auth middleware, maintenance, and router assembly. `routes_v2.rs` registers every route in the locked complete v2 surface. `protocol.rs` validates envelopes/query identity and maps all failures to `V2Error`. `commands.rs` holds typed handlers and invokes store command transactions. Remove every `/v1/` registration and v1 request struct; do not retain aliases.

`/v2/runtime/identity` returns:

```json
{
  "protocol_version": "stateful.v2",
  "journal_schema_version": 2,
  "coordination_mode": "awareness",
  "workspace_id": "workspace-1",
  "workspace_version": 42,
  "capabilities": ["presence", "handoff", "exact_read", "context_cursor", "write_intent", "enforcement_opt_in"]
}
```

- [ ] **Step 4: Make maintenance and queries invoke lazy expiry**

Startup and presence/current/context/policy queries finalize expired presence/handoffs/intents idempotently before reading. Maintenance appends expiry events; it never prunes journal events. Readiness becomes true only after schema checkpoint, projector version, and replay metadata checks.

- [ ] **Step 5: Split and run route suites**

Move v2-relevant tests from `routes.rs` into domain files and delete obsolete v1 expectations. Run:

```sh
stateful sandbox run --fs build --network disabled --write-dir v2-server-green \
  --command 'cargo test -p stateful-server'
```

Expected: pass; no route under `/v1` responds.

- [ ] **Step 6: Commit and push**

Commit as `feat: cut server protocol to stateful v2`, then push.

---

### Task 9: Cut CLI, Runtime, Outbox, Watcher, and Doctor to V2

**Files:** CLI source/tests except OMP asset; server diagnostics.

- [ ] **Step 1: Write failing client/runtime/doctor tests**

```rust
#[test] fn runtime_post_wraps_typed_payload_in_v2_envelope();
#[test] fn runtime_get_serializes_full_query_identity();
#[test] fn unsupported_runtime_protocol_fails_before_mutation();
#[test] fn recovery_outbox_preserves_original_request_id_and_retries_idempotently();
#[test] fn watcher_emits_v2_human_observation_and_reconciliation();
#[test] fn server_start_and_install_default_to_awareness();
#[test] fn explicit_enforcement_flag_is_preserved();
#[test] fn doctor_reports_journal_size_rows_types_time_range_growth_and_threshold();
#[test] fn doctor_warns_at_default_five_hundred_twelve_mib_without_pruning();
```

- [ ] **Step 2: Run RED**

```sh
stateful sandbox run --fs build --network disabled --write-dir v2-cli-red \
  --command 'cargo test -p stateful-cli --test runtime --test outbox --test server_lifecycle --test cli'
```

- [ ] **Step 3: Implement one v2 HTTP client path**

Centralize envelope creation, POST, flattened GET query, capability handshake, and structured error parsing in `runtime.rs`. All CLI commands, watcher calls, outbox replay, and hooks use it. Delete hand-built v1 JSON and endpoint strings. Preserve model-facing command/tool names while changing their wire payload atomically.

Outbox entries contain serialized v2 envelope, route, request UUID, attempt metadata, and no duplicate request regeneration. On replay, a frozen duplicate receipt counts as success.

- [ ] **Step 4: Implement defaults and diagnostics**

Set clap/server/install default mode to awareness; enforcement requires `--coordination-mode enforcement`. `stateful doctor` reads sanitized journal diagnostics and configurable byte threshold without VACUUM, deletion, or payload output. Runtime status reports protocol/schema/version/capabilities.

- [ ] **Step 5: Run GREEN and CLI regression suite**

Run Step 2, then `cargo test -p stateful-cli`. Expected: pass.

- [ ] **Step 6: Commit and push**

Commit as `feat: migrate stateful clients to v2`, then push.

---

### Task 10: Implement Codex Presence, Exact-Read, Write, Handoff, and Context Hooks

**Files:** `hook.rs`, new hook submodules, `install.rs`, CLI hook/install tests.

- [ ] **Step 1: Write failing Codex lifecycle tests**

```rust
#[test] fn session_start_registers_renders_injects_and_acks_initial_context();
#[test] fn first_prompt_captures_goal_and_later_prompts_deliver_only_new_versions();
#[test] fn local_once_per_session_marker_is_not_created_or_consulted();
#[test] fn full_successful_read_posts_start_and_complete_with_one_operation_id();
#[test] fn partial_or_truncated_read_updates_presence_without_baseline();
#[test] fn pre_write_returns_intent_and_post_success_commits_it();
#[test] fn post_failure_records_failure_and_releases_fence();
#[test] fn missing_server_fails_closed_only_for_file_changing_tools();
#[test] fn stop_posts_fallback_only_when_explicit_handoff_is_absent();
#[test] fn presence_handoff_and_delivery_failures_enter_recovery_outbox();
```

Fixtures include hook operation aliases `tool_use_id`, `tool_call_id`, and `call_id`; if none is supplied, reject exact-read/write lifecycle classification rather than path-pairing.

- [ ] **Step 2: Run RED**

```sh
stateful sandbox run --fs build --network disabled --write-dir v2-codex-hooks-red \
  --command 'cargo test -p stateful-cli --test hook --test install_global'
```

- [ ] **Step 3: Extract v2 hook concerns without disturbing sandbox policy**

Keep shell/tool classification and sandbox authorization in `hook.rs`. Move payload parsing to `hook/input.rs`, exact-read classification/fingerprints to `observation.rs`, pre/post write and unknown recovery to `write_lifecycle.rs`, and render/queue/ACK logic to `delivery.rs`. Delete the local prompt marker functions and state files.

Update generated Codex hooks so SessionStart, UserPromptSubmit, PreToolUse/PostToolUse Read, PreToolUse/PostToolUse write tools, and Stop include operation ID, prompt, tool outcome, truncation/completeness metadata, and result summary where available. Ambiguous metadata never becomes a stable observation.

- [ ] **Step 4: Implement failure behavior and recovery**

Read-only tools continue when render/presence delivery fails and enqueue recoverable events. File-changing tools fail closed when the v2 handshake/policy cannot run. Post-write failures preserve original request/operation IDs. Stop is idempotent and does not overwrite an explicit handoff.

- [ ] **Step 5: Run GREEN**

Run Step 2 and the full CLI suite. Expected: pass.

- [ ] **Step 6: Commit and push**

Commit as `feat: wire codex hooks to presence and freshness`, then push.

---

### Task 11: Implement OMP and VS Code V2 Delivery

**Files:** OMP asset/test/install tests; VS Code integration/tests; hook delivery/input code.

- [ ] **Step 1: Add failing JavaScript and integration tests**

Export pure helpers from the OMP asset and test:

```javascript
import {
  exactReadCandidate,
  coalesceContextInvalidation,
  shouldDeliverContextVersion,
} from "./stateful-omp-extension.js";
```

Test full raw/no-range/non-truncated reads only; latest-version coalescing; duplicate/out-of-order suppression; initial `nextTurn` delivery then ACK; ACK failure redelivery; SSE loss recovery on `tool_result`; and awareness warnings queued to the next turn. VS Code tests assert v2 envelope/handshake for human observe/save-check/reconcile and no v1 endpoint.

- [ ] **Step 2: Run RED**

```sh
stateful sandbox run --fs read-only --network disabled \
  --command 'node --test crates/stateful-cli/assets/stateful-omp-extension.test.mjs integrations/vscode/test/core.test.js'
```

Also run `cargo test -p stateful-cli --test install_global --test hook`; expected failures identify old reservation-only SSE and v1 IDE payloads.

- [ ] **Step 3: Implement OMP delivery and operation correlation**

Pass `event.toolCallId` from `tool_call` to pre-hook and the same ID plus `event.isError`, completeness, and result metadata from `tool_result`. `session_start` queues initial context with `deliverAs:"nextTurn"`, then ACKs. SSE consumes `context_invalidated(target_version)`, retains one latest pending version, renders/queues/ACKs it, and ignores old versions. Every `tool_result` compares server cursor/version and recovers a missed SSE notification. Keep reservation-granted notifications functional in enforcement mode.

Awareness overlap never stores a lazy operation; it queues warning/context. Enforcement denials retain lazy edit/write replay.

- [ ] **Step 4: Implement VS Code v2 human sensor calls**

Perform runtime identity handshake, build explicit IDE actor/workspace/source identity, and use v2 human observe/save-check/reconcile endpoints. Unsupported protocol fails the save check conservatively with actionable output.

- [ ] **Step 5: Run GREEN**

Run Step 2, the two Rust tests, and `cargo test -p stateful-cli`. Expected: pass.

- [ ] **Step 6: Commit and push**

Commit as `feat: deliver v2 context to omp and vscode`, then push.

---

### Task 12: Prove the Product Path with a Two-Session Runtime Smoke

**Files:** `crates/stateful-cli/tests/v2_two_session.rs`; implementation fixes only where the smoke exposes a real defect.

- [ ] **Step 1: Write the end-to-end smoke before fixes**

The test starts a real listener with a persistent temp DB and temp Git workspace, then performs exactly:

```text
A register -> A full exact read -> B register -> B write commit
-> A receives versioned context -> A stale write denied
-> A exact reread -> A write commit -> B explicit handoff
-> A receives handoff -> A expires without Stop -> TTL fallback
-> stop server -> replay journal into empty projections -> compare current state
```

Assert awareness warnings do not queue, stale/fence/human hard stops still deny, every delivery is ACKed once, explicit and fallback handoffs differ honestly, and replay equality includes origin event sequences.

- [ ] **Step 2: Run smoke and debug causes, not symptoms**

```sh
stateful sandbox run --fs build --network disabled --write-dir v2-two-session-smoke \
  --command 'cargo test -p stateful-cli --test v2_two_session -- --nocapture'
```

Expected before integration fixes: a concrete failing transition. Use `systematic-debugging`; do not weaken assertions.

- [ ] **Step 3: Make the smoke pass and rerun targeted crates**

After the smoke passes, run core/store/server/CLI tests. Expected: all pass.

- [ ] **Step 4: Commit and push**

Commit the smoke and source fixes as `test: prove v2 two-session coordination`, then push.

---

### Task 13: Selectively Port the Frozen Real-World Runner and Corpus

**Files:** only the StatefulBench/dataset/dev-source paths listed in the File Map; no coordination core, server, CLI, README, or unrelated dev file.

- [ ] **Step 1: Record and review the exact source delta**

Use `git diff --name-status main...dev -- <listed paths>` and compare every imported file to `dev@b5e875e`. Reject any path outside the locked port surface. Preserve the frozen dataset byte-for-byte.

- [ ] **Step 2: Copy the exact runner/corpus paths from dev**

Use a path-limited Git checkout/restore under the active Stateful reservation. Include Dockerfile, five runner/runtime scripts, named test files, required ProgramBench/DeNovo adapter deltas, the entire `datasets/statefulbench-realworld` tree, `docs/statefulbench-realworld-design.md`, and the Docker runtime plan. Do not merge or cherry-pick dev.

- [ ] **Step 3: Run imported unit tests before v2 adaptation**

```sh
stateful sandbox run --fs build --network disabled --write-dir realworld-port-tests \
  --command 'python3 -m unittest discover -s crates/stateful-bench/scripts/tests -t crates/stateful-bench/scripts -p "test_statefulbench_*.py" -v'
```

Run Rust benchmark tests as well. Classify failures as missing imported dependency, stale v1 assumption, or environment gate; do not modify frozen corpus to make tests green.

- [ ] **Step 4: Commit and push the mechanical port**

Commit only the reviewed port as `bench: port real-world runner and corpus`, then push.

---

### Task 14: Adapt StatefulBench to V2 Metrics and Clear Credit-Free Gates

**Files:** imported runner/diagnostics/Docker/tests and benchmark Rust adapter files; no frozen dataset edits.

- [ ] **Step 1: Write failing v2 diagnostics and aggregation tests**

Add tests that construct a sanitized v2 journal/projection snapshot and assert the exact locked `coordination_metrics` shape, weighted wait means, category sorting, null metrics for off arms, uncleared on-arm rows when any metric is missing, no path/ID/message leakage, fresh DB per row, explicit awareness launch, and aggregate null on incomplete scheduled trials.

- [ ] **Step 2: Run RED**

```sh
stateful sandbox run --fs build --network disabled --write-dir realworld-v2-red \
  --command 'python3 -m unittest discover -s crates/stateful-bench/scripts/tests -t crates/stateful-bench/scripts -p "test_statefulbench_realworld.py" -v'
stateful sandbox run --fs build --network disabled --write-dir realworld-v2-docker-red \
  --command 'python3 -m unittest discover -s crates/stateful-bench/scripts/tests -t crates/stateful-bench/scripts -p "test_statefulbench_docker.py" -v'
```

- [ ] **Step 3: Implement sanitized v2 diagnostic extraction**

Read journal counts/types, projection lifecycle counts, context delivery counters, warning/denial reason codes, write-safety counters, notification status, wait durations, and SQLite size at phase snapshots. Never serialize payloads or identity/resource fields. Verify phase counters are monotonic. Parallel-on starts `stateful server --coordination-mode awareness`; each row gets isolated HOME and a fresh DB.

Update report/summary aggregation to sum counters, compute weighted duration means, retain complete row objects, and make incomplete on-arm metrics null plus uncleared.

- [ ] **Step 4: Run Python/Rust/JavaScript benchmark tests GREEN**

Run full script unittest discovery and `cargo test -p stateful-bench`. Expected: pass.

- [ ] **Step 5: Build and inspect one immutable Linux/arm64 image**

Use tag `statefulbench-realworld:presence-v2` and the current checkout. Run through the Stateful external sandbox with the Colima Docker socket and network enabled:

```sh
stateful sandbox run --fs external --purpose 'Build StatefulBench v2 image' \
  --connect-socket "$HOME/.colima/default/docker.sock" --network enabled \
  --command 'docker build --platform linux/arm64 --pull -f crates/stateful-bench/docker/statefulbench-realworld.Dockerfile -t statefulbench-realworld:presence-v2 .'
stateful sandbox run --fs external --purpose 'Inspect StatefulBench v2 image' \
  --connect-socket "$HOME/.colima/default/docker.sock" --network disabled \
  --command 'docker image inspect statefulbench-realworld:presence-v2 --format "{{.Id}} {{.Os}}/{{.Architecture}} {{join .RepoDigests \",\"}}"'
```

Expected: one image ID and `linux/arm64`. Record the ID; rebuilding or retagging invalidates subsequent receipts.

- [ ] **Step 6: Qualify `requests` with that exact image**

Use cache `$HOME/.cache/statefulbench-realworld-presence-v2/cache`:

```sh
stateful sandbox run --fs external --purpose 'Qualify StatefulBench requests corpus' \
  --write-dir "$HOME/.cache/statefulbench-realworld-presence-v2" \
  --connect-socket "$HOME/.colima/default/docker.sock" --network enabled \
  --command 'python3 crates/stateful-bench/scripts/statefulbench_realworld.py qualify --manifest datasets/statefulbench-realworld/manifest.json --cache "$HOME/.cache/statefulbench-realworld-presence-v2/cache" --docker-image statefulbench-realworld:presence-v2 --repo requests'
```

Expected: exit 0 and a receipt bound to manifest, corpus, graded inputs, source archive, image ID/platform/digests, and six-tool map.

- [ ] **Step 7: Run the credit-free Docker E2E with the same image**

```sh
stateful sandbox run --fs external --purpose 'Run StatefulBench credit-free Docker gate' \
  --write-dir "$HOME/.cache/statefulbench-realworld-presence-v2" \
  --connect-socket "$HOME/.colima/default/docker.sock" --network enabled \
  --command 'STATEFULBENCH_DOCKER_TEST_IMAGE=statefulbench-realworld:presence-v2 python3 -m unittest discover -s crates/stateful-bench/scripts/tests -t crates/stateful-bench/scripts -p "test_statefulbench_docker.py" -v'
```

Expected: all tests pass and diagnostics include complete v2 coordination fields. Any fix that changes the image requires rebuild, re-inspection, requalification, and rerun of this gate.

- [ ] **Step 8: Commit and push**

Commit as `bench: adapt real-world runner to stateful v2`, then push.

---

### Task 15: Complete Documentation, Skills, Templates, and Cross-Surface Consistency

**Files:** every documentation/asset/skill target listed in the File Map.

- [ ] **Step 1: Dispatch independent post-change documentation slices**

After Tasks 12 and 14 demonstrably pass, use parallel subagents with non-overlapping write scopes:

```text
README + CHANGELOG
core concept + architecture + state model + implementation contract
usage reference + current-state matrix/SVG + ADRs
bundled command-policy skill + OMP/Codex install templates
real-world benchmark guide/design/runtime plan + installed running-statefulbench skill
read-only implementation/documentation conflict audit
```

Each writer must use current code/tests as truth, skip formatters and project-wide suites, and report exact changed paths. The main agent reviews every result before integration.

- [ ] **Step 2: Enforce the documentation cutover**

All surfaces must say: awareness default; enforcement opt-in; presence/freshness/handoff product center; canonical indefinite journal; migration/backup behavior; exact-read rather than write-time provenance; explicit/fallback handoffs; version/delivery/ACK context; hard-stop boundaries; v2 endpoint/envelope; doctor retention warning; and descriptive one-trial benchmark limits.

Remove current-behavior claims for v1 routes, enforcement default, once-per-session prompt markers, claim hashes as model reads, age-based canonical event pruning, and cleanup counts as handoff. Mark ADR 0001/0002 `Accepted` with implementation evidence. Preserve historical plans as historical records rather than rewriting them.

- [ ] **Step 3: Update benchmark operator guidance**

`docs/statefulbench-realworld.md` and `skill://running-statefulbench` must use the exact image/qualification/run contracts from Task 14, say parallel-on explicitly starts awareness, list v2 metric completeness, require fresh row DBs/output dirs, and forbid claims of causal/statistical superiority. The operator-local skill change is reported separately because it is not a Git artifact.

- [ ] **Step 4: Run focused doc/asset tests and conflict scan**

Run install-global tests, Node tests, link/reference checks already present in the repo, and built-in `grep` searches for current v1/default/pruning/once-marker claims. Review every match in context; historical records may remain only when labeled historical.

- [ ] **Step 5: Commit and push repository documentation**

Commit repo files as `docs: document presence-first stateful v2`, push, and record the separate installed-skill update.

---

### Task 16: Final Review, Full Verification, and the Scoped Three-Arm Run

**Files:** all changed files; fresh external benchmark output only.

- [ ] **Step 1: Run final code review before the expensive gate**

Invoke `requesting-code-review` and dispatch a reviewer against the approved spec and branch diff. Require findings on journal atomicity/replay, migration rollback, hard-stop safety, identity isolation, cursor redelivery, privacy of diagnostics, direct projection writes, v1 leftovers, and benchmark admission. Fix every correctness/security finding, rerun its targeted reproduction, commit, and push.

- [ ] **Step 2: Format, lint, build, and test the full repository**

Run the repository formatter, Clippy with warnings denied, workspace build, workspace tests, Node tests, and Python runner discovery under Stateful sandbox profiles. Expected: every command exits 0. Rerun `v2_two_session` separately with `--nocapture` and record its pass.

- [ ] **Step 3: Verify architectural invariants**

Use built-in code search, not source-text unit tests, to establish:

```text
no /v1/ route or stateful.v1 runtime string
no canonical projection mutation outside projector/migration cutover
no age-based DELETE from journal_events
no local once-per-session context marker
no claim hash accepted as v2 read provenance
no parallel-on implicit default in the runner
```

Inspect journal replay on a persistent smoke DB and verify doctor output contains no payload/identity/resource data.

- [ ] **Step 4: Reconfirm immutable benchmark admission**

Inspect `statefulbench-realworld:presence-v2` and compare its image ID to the `requests` qualification receipt. Rerun qualification and credit-free E2E if any product/runner/Docker input changed after Task 14. Do not start model agents until both pass against the final image.

- [ ] **Step 5: Launch the model-backed run under supervised execution**

Use a fresh output directory `$HOME/.cache/statefulbench-realworld-presence-v2/runs/requests-v2-20260715-t1` and the exact qualified image:

Launch specification:

```yaml
name: statefulbench-requests-v2
application: stateful
args:
  - sandbox
  - run
  - --fs
  - external
  - --purpose
  - Run qualified StatefulBench requests trial
  - --write-dir
  - /Users/arthur/.cache/statefulbench-realworld-presence-v2
  - --connect-socket
  - /Users/arthur/.colima/default/docker.sock
  - --network
  - enabled
  - --command
  - python3 crates/stateful-bench/scripts/statefulbench_realworld.py run --manifest datasets/statefulbench-realworld/manifest.json --cache "$HOME/.cache/statefulbench-realworld-presence-v2/cache" --out "$HOME/.cache/statefulbench-realworld-presence-v2/runs/requests-v2-20260715-t1" --docker-image statefulbench-realworld:presence-v2 --repos requests --arms sequential,parallel-off,parallel-on --trials 1 --model openai-codex/gpt-5.6-terra --thinking high
cwd: /Users/arthur/Code/stateful_core
```

Start it with `launch`, observe progress from supervised logs without tight polling, and wait for the wrapper exit. Never run live OMP agents on the host.

- [ ] **Step 6: Enforce row completion and recovery rules**

Success requires exactly three rows, all `cleared:true`; every task and final agent normal; post-suite/evaluators/upstream suite pass; container cleanup and diagnostics complete; identical image/qualification identity; off-arm metrics null; parallel-on full v2 metrics; no unclassified diagnostic.

If any row is uncleared: retain output, classify the actual failure, use `systematic-debugging`, fix the source/runtime cause, rebuild and requalify when image inputs changed, and rerun all three arms into a new `-retryN` directory. Never edit or reuse a failed output directory and never report an uncleared row as evidence.

- [ ] **Step 7: Produce the descriptive evidence report**

Report exact commit, image ID/platform/digests, receipt identity, model/thinking, repository/arms/trial, row clear states, evaluator outcomes, wall times, tokens, tool calls, complete parallel-on coordination metrics, journal growth, and artifact paths. State explicitly that one trial is scoped integration/efficiency evidence, not causal, safety, behavioral-quality, or statistical-superiority proof.

- [ ] **Step 8: Final commit/push and completion verification**

If verification changed files, stage only those files, commit as `fix: address final v2 verification findings`, and push. Invoke `verification-before-completion`; confirm the remote branch contains every finished commit and every completion criterion below has current evidence.

---

## Completion Criteria and Evidence Map

- Journal append/project/receipt atomicity, idempotency, rollback, and replay: Tasks 2, 5, 16.
- Transactional persistent migration, physical backup, honest unknown attribution, rerun no-op: Task 3.
- Live presence, attribution, goal/phase/plan/resources, bounded tool result: Tasks 1, 4, 10, 11.
- Explicit/Stop/TTL handoff and transactional cleanup: Tasks 4, 8, 10, 12.
- Exact read stabilization, stale denial, missing-read warning, write intent recovery: Tasks 6, 10, 11, 12.
- Awareness default, enforcement opt-in, thin hard stops: Tasks 6, 8, 9, 12.
- Semantic context version, render/delivery/ACK/redelivery/coalescing and budgets: Tasks 7, 10, 11, 12.
- Clean v2 routes/envelopes/capability handshake and v1 removal: Tasks 1, 8, 9, 10, 11, 16.
- Indefinite canonical retention and doctor growth warning: Tasks 2, 3, 8, 9, 15, 16.
- Real-world runner/corpus selective port without dev core: Task 13.
- V2 metrics, identical qualified image, fresh DBs, awareness on-arm, credit-free Docker gate: Task 14.
- README/docs/ADRs/skills/templates/benchmark guide consistency: Task 15.
- Two-session smoke, full repository verification, reviewed code, and three cleared `requests` rows: Tasks 12 and 16.

## Self-Review Checklist

- Every approved spec section maps to at least one task above.
- Every behavioral task starts with a failing observable-contract test and names the expected RED reason.
- Public type/function names are introduced before later tasks use them.
- No compatibility shim, v1 alias, direct projection mutation, event pruning path, or benchmark mock is part of the delivered state.
- The frozen dataset is copied unchanged; only runner/runtime/metrics code is adapted.
- Credit-bearing work occurs only after final image qualification and credit-free E2E.
- Documentation work begins only after product smoke and credit-free benchmark behavior pass.
- Commits stage only task-owned files and are pushed after creation.
