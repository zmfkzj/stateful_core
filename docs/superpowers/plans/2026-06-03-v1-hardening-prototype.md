# V1 Hardening Prototype Implementation Plan

> Historical note: validation profile references in this plan are obsolete. Current test/build execution uses `stateful sandbox run --fs write-targets` with explicit write targets or `--write-dir target`.

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build the v1 hardening prototype: explicit scheduling APIs, protocol metadata enforcement, a shared policy service, agent-agent conflict completion, portable hook wrappers, minimal context rendering, lazy expiration, and aligned docs.

**Architecture:** Keep pure scope matching in `stateful-core`, persistence in `stateful-store`, and HTTP orchestration in `stateful-server`. Introduce a server-side policy service as the single decision entry point over store-backed state, then migrate CLI/MCP/hook adapters to send protocol metadata and call the new scheduling path.

**Tech Stack:** Rust 2024, Axum, rusqlite, serde, time, clap, tower tests, existing `stateful` CLI and hook adapter modules.

---

## Scope Check

The design spans protocol, policy, scheduling, expiration, hooks, context, and docs. These are kept in one plan because protocol metadata and policy routing must be shared before scheduling and hook changes can be correct. Each task below produces a testable slice and a commit.

## File Map

- Modify: `crates/stateful-core/src/types.rs`
  - Add reusable protocol/session/workspace/source metadata helpers only when they are independent of store state.
- Create: `crates/stateful-server/src/protocol.rs`
  - Parse and validate protocol metadata for side-effecting JSON requests.
- Create: `crates/stateful-server/src/policy_service.rs`
  - Own final allow/warn/deny/error decisions for write, scheduling, lease, validation, reconciliation, and conflict operations.
- Modify: `crates/stateful-server/src/lib.rs`
  - Route registration and handler migration to protocol and policy service helpers.
- Modify: `crates/stateful-store/src/lib.rs`
  - Add scheduling request persistence, request id idempotency, explicit claim/cancel helpers, real timestamps, lazy expiration, and context query helpers.
- Create: `crates/stateful-store/src/clock.rs`
  - Provide system and fixed test clocks.
- Modify: `crates/stateful-store/Cargo.toml`
  - No new dependency is expected; `time` already exists.
- Modify: `Cargo.toml`
  - Enable `time` parsing and formatting features for deterministic clock tests.
- Modify: `crates/stateful-server/tests/routes.rs`
  - Add route tests for protocol enforcement, scheduling APIs, claim flow, context rendering, and server-level expiration behavior.
- Modify: `crates/stateful-store/tests/event_store.rs`
  - Add store tests for idempotent requests, explicit claim/cancel, and lazy expiration.
- Modify: `crates/stateful-cli/src/runtime.rs`
  - Add envelope construction for CLI/hook/MCP HTTP POST requests.
- Modify: `crates/stateful-cli/src/hook.rs`
  - Split Codex parsing from normalized hook handling.
- Modify: `crates/stateful-cli/src/lib.rs`
  - Add `stateful hook codex <event>` and `stateful hook run <event>` while preserving legacy aliases.
- Modify: `crates/stateful-cli/tests/hook.rs`
  - Add normalized hook tests and Codex compatibility tests.
- Modify: `crates/stateful-mcp/src/lib.rs`
  - Keep `state.*` MCP surface; route state tool calls through the same protocol body shape.
- Modify: `README.md`
  - Align quick start and remove `intent wait`.
- Modify: `docs/implementation-contract.md`
  - Align official claim flow, protocol enforcement, and future work boundaries.
- Modify: `docs/architecture.md`
  - Align hook wrapper and future MCP filesystem/git scope.
- Modify: `docs/current-state-coordination.md`
  - Align scheduling and validation profile deferral wording.
- Modify: `docs/state-model.md`
  - Align explicit claim and expiration minimum behavior.

---

### Task 1: Protocol Envelope Helper

**Files:**
- Create: `crates/stateful-server/src/protocol.rs`
- Modify: `crates/stateful-server/src/lib.rs`
- Modify: `crates/stateful-server/tests/routes.rs`

- [ ] **Step 1: Write failing protocol tests**

Append these tests near the existing auth tests in `crates/stateful-server/tests/routes.rs`:

```rust
#[tokio::test]
async fn side_effecting_routes_fail_closed_without_protocol_metadata() {
    let app = build_router(ServerConfig::new("secret-token"));

    let response = app
        .oneshot(json_request(
            "/v1/intent/declare",
            serde_json::json!({
                "session_id": "s1",
                "workspace_id": "w1",
                "files_planned": ["src/auth.ts"]
            }),
        ))
        .await
        .expect("intent declaration should complete");

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let body = to_bytes(response.into_body(), 2048)
        .await
        .expect("body should read");
    let json: serde_json::Value = serde_json::from_slice(&body).expect("body should be json");
    assert_eq!(json["decision"], "error");
    assert_eq!(json["reason_code"], "protocol_mismatch");
}

#[tokio::test]
async fn side_effecting_routes_accept_v1_protocol_metadata() {
    let app = build_router(ServerConfig::new("secret-token"));

    let response = app
        .oneshot(protocol_request(
            "/v1/intent/declare",
            "req-declare-1",
            "s1",
            "w1",
            serde_json::json!({
                "files_planned": ["src/auth.ts"]
            }),
        ))
        .await
        .expect("intent declaration should complete");

    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn side_effecting_routes_fail_closed_on_major_protocol_mismatch() {
    let app = build_router(ServerConfig::new("secret-token"));

    let mut body = protocol_body("req-declare-2", "s1", "w1", serde_json::json!({
        "files_planned": ["src/auth.ts"]
    }));
    body["protocol_version"] = serde_json::json!("stateful.v2");

    let response = app
        .oneshot(json_request("/v1/intent/declare", body))
        .await
        .expect("intent declaration should complete");

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let body = to_bytes(response.into_body(), 2048)
        .await
        .expect("body should read");
    let json: serde_json::Value = serde_json::from_slice(&body).expect("body should be json");
    assert_eq!(json["reason_code"], "protocol_mismatch");
}
```

Add these helpers near `json_request` in the same file:

```rust
fn protocol_body(
    request_id: &str,
    session_id: &str,
    workspace_id: &str,
    payload: serde_json::Value,
) -> serde_json::Value {
    let mut body = serde_json::json!({
        "protocol_version": "stateful.v1",
        "request_id": request_id,
        "session": {
            "session_id": session_id,
            "actor_id": session_id,
            "actor_type": "agent"
        },
        "workspace": {
            "workspace_id": workspace_id,
            "root": "/repo",
            "repo_id": "repo-1",
            "worktree_id": "worktree-1",
            "branch": "main"
        },
        "source": {
            "kind": "cli",
            "event": "test",
            "source_ref": "routes.rs"
        }
    });
    let object = body.as_object_mut().expect("body should be object");
    for (key, value) in payload.as_object().expect("payload should be object") {
        object.insert(key.clone(), value.clone());
    }
    body
}

fn protocol_request(
    path: &str,
    request_id: &str,
    session_id: &str,
    workspace_id: &str,
    payload: serde_json::Value,
) -> Request<Body> {
    json_request(path, protocol_body(request_id, session_id, workspace_id, payload))
}
```

- [ ] **Step 2: Run protocol tests and verify failure**

Run:

```bash
cargo test -p stateful-server side_effecting_routes_fail_closed --test routes
cargo test -p stateful-server side_effecting_routes_accept_v1_protocol_metadata --test routes
```

Expected: FAIL because side-effecting handlers still deserialize legacy request bodies and do not validate protocol metadata.

- [ ] **Step 3: Add protocol parsing module**

Create `crates/stateful-server/src/protocol.rs` with:

```rust
use axum::{Json, http::StatusCode};
use serde::Deserialize;
use serde_json::{Value, json};

pub const V1_PROTOCOL: &str = "stateful.v1";

#[derive(Debug, Clone, Deserialize)]
pub struct ProtocolRequest<T> {
    pub protocol_version: String,
    pub request_id: String,
    pub session: SessionMetadata,
    pub workspace: WorkspaceMetadata,
    pub source: SourceMetadata,
    #[serde(flatten)]
    pub payload: T,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SessionMetadata {
    pub session_id: String,
    #[serde(default)]
    pub turn_id: Option<String>,
    pub actor_id: String,
    pub actor_type: String,
    #[serde(default)]
    pub owner_id: Option<String>,
    #[serde(default)]
    pub parent_session_id: Option<String>,
    #[serde(default)]
    pub parent_actor_id: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct WorkspaceMetadata {
    pub workspace_id: String,
    pub root: String,
    pub repo_id: String,
    pub worktree_id: String,
    pub branch: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SourceMetadata {
    pub kind: String,
    pub event: String,
    #[serde(default)]
    pub tool_name: Option<String>,
    pub source_ref: String,
}

#[derive(Debug, Clone)]
pub struct ValidatedRequest<T> {
    pub request_id: String,
    pub session: SessionMetadata,
    pub workspace: WorkspaceMetadata,
    pub source: SourceMetadata,
    pub payload: T,
}

pub fn validate_protocol<T>(
    input: ProtocolRequest<T>,
) -> Result<ValidatedRequest<T>, (StatusCode, Json<Value>)> {
    if input.protocol_version != V1_PROTOCOL {
        return Err(protocol_error());
    }

    Ok(ValidatedRequest {
        request_id: input.request_id,
        session: input.session,
        workspace: input.workspace,
        source: input.source,
        payload: input.payload,
    })
}

pub fn protocol_error() -> (StatusCode, Json<Value>) {
    (
        StatusCode::BAD_REQUEST,
        Json(json!({
            "decision": "error",
            "reason_code": "protocol_mismatch",
            "message": "Side-effecting requests require protocol_version stateful.v1.",
            "required_next_action": "Retry with protocol_version stateful.v1 and request_id."
        })),
    )
}
```

In `crates/stateful-server/src/lib.rs`, add:

```rust
mod protocol;
use protocol::{ProtocolRequest, validate_protocol};
```

Change `intent_declare` to accept `Json(input): Json<ProtocolRequest<IntentDeclarePayload>>`. Define:

```rust
#[derive(Debug, Deserialize)]
struct IntentDeclarePayload {
    files_planned: Vec<String>,
    #[serde(flatten)]
    identity: WorkspaceIdentityRequest,
}
```

Inside the handler, validate first and use metadata for session/workspace:

```rust
let input = match validate_protocol(input) {
    Ok(input) => input,
    Err(error) => return error,
};
let event = Event::intent_declared(
    input.session.session_id,
    input.workspace.workspace_id,
    input.payload.files_planned,
);
```

Set identity from protocol workspace first, then allow payload identity to override only if needed:

```rust
let identity = WorkspaceIdentityRequest {
    repo_id: Some(input.workspace.repo_id),
    worktree_id: Some(input.workspace.worktree_id),
    root: Some(input.workspace.root),
    branch: Some(input.workspace.branch),
};
```

- [ ] **Step 4: Run protocol tests and focused route tests**

Run:

```bash
cargo test -p stateful-server side_effecting_routes_fail_closed --test routes
cargo test -p stateful-server side_effecting_routes_accept_v1_protocol_metadata --test routes
cargo test -p stateful-server declared_intent_allows_matching_authorize_request --test routes
```

Expected: the new protocol tests PASS. The legacy `declared_intent_allows_matching_authorize_request` test FAILS because it still uses `json_request` for `/v1/intent/declare`.

- [ ] **Step 5: Update route tests for `/v1/intent/declare`**

Replace `/v1/intent/declare` calls in `crates/stateful-server/tests/routes.rs` with `protocol_request`. Example replacement:

```rust
let declare = app
    .clone()
    .oneshot(protocol_request(
        "/v1/intent/declare",
        "req-declare-s1-auth",
        "s1",
        "w1",
        serde_json::json!({
            "files_planned": ["src/auth.ts"]
        }),
    ))
    .await
    .expect("intent declaration should complete");
```

Use unique request ids in each test, such as `req-declare-s1-auth`, `req-declare-s1-session`, and `req-declare-s2-queue`.

- [ ] **Step 6: Run all server route tests**

Run:

```bash
cargo test -p stateful-server --test routes
```

Expected: PASS after all intent declaration tests use protocol bodies.

- [ ] **Step 7: Commit protocol helper**

```bash
git add crates/stateful-server/src/protocol.rs crates/stateful-server/src/lib.rs crates/stateful-server/tests/routes.rs
git commit -m "feat: require protocol metadata for intent declarations"
```

---

### Task 2: Store Request Id and Scheduling Persistence

**Files:**
- Modify: `crates/stateful-store/src/lib.rs`
- Modify: `crates/stateful-store/tests/event_store.rs`

- [ ] **Step 1: Write failing store tests**

Append these tests in `crates/stateful-store/tests/event_store.rs` after the existing reservation tests:

```rust
#[test]
fn intent_requests_are_idempotent_by_request_id() {
    let store = Store::open_in_memory().expect("store should open");

    let first = store
        .create_intent_request(
            "req-1",
            "s1",
            "w1",
            &["src/auth.ts".to_string()],
            "write_file",
        )
        .expect("first request should create");
    let second = store
        .create_intent_request(
            "req-1",
            "s1",
            "w1",
            &["src/auth.ts".to_string()],
            "write_file",
        )
        .expect("duplicate request should return existing");

    assert_eq!(first.request_id, "req-1");
    assert_eq!(second.request_id, "req-1");
    assert_eq!(
        store
            .intent_request_count()
            .expect("request count should load"),
        1
    );
}

#[test]
fn cancelling_request_cancels_owned_waiters_only() {
    let store = Store::open_in_memory().expect("store should open");
    store
        .create_intent_request(
            "req-1",
            "s1",
            "w1",
            &["src/auth.ts".to_string()],
            "write_file",
        )
        .expect("request should create");
    let waiter = store
        .enqueue_waiter_for_request("req-1", "s1", "w1", "src/auth.ts", "write_file", Some("s0"))
        .expect("waiter should enqueue");

    store
        .cancel_intent_request("req-1", "s2")
        .expect_err("different session cannot cancel");
    assert_eq!(
        store
            .waiter_status(&waiter.wait_id)
            .expect("waiter status should load"),
        Some("queued".to_string())
    );

    store
        .cancel_intent_request("req-1", "s1")
        .expect("owner should cancel");
    assert_eq!(
        store
            .waiter_status(&waiter.wait_id)
            .expect("waiter status should load"),
        Some("cancelled".to_string())
    );
}
```

- [ ] **Step 2: Run store tests and verify failure**

Run:

```bash
cargo test -p stateful-store intent_requests_are_idempotent_by_request_id cancelling_request_cancels_owned_waiters_only
```

Expected: FAIL because `create_intent_request`, `intent_request_count`, `enqueue_waiter_for_request`, and `cancel_intent_request` do not exist.

- [ ] **Step 3: Add scheduling schema**

In `migrate()` in `crates/stateful-store/src/lib.rs`, add this table before `wait_queue`:

```rust
CREATE TABLE IF NOT EXISTS intent_requests (
    request_id TEXT PRIMARY KEY,
    session_id TEXT NOT NULL,
    workspace_id TEXT NOT NULL,
    resources_json TEXT NOT NULL,
    action TEXT NOT NULL,
    status TEXT NOT NULL,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_intent_requests_session_status
    ON intent_requests(session_id, status);
```

Add `request_id TEXT` to `wait_queue` using `add_column_if_missing` after migration:

```rust
self.add_column_if_missing("wait_queue", "request_id", "TEXT")?;
```

- [ ] **Step 4: Add store record and methods**

Add this record near `WaitRecord`:

```rust
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IntentRequestRecord {
    pub request_id: String,
    pub session_id: String,
    pub workspace_id: String,
    pub resources: Vec<String>,
    pub action: String,
    pub status: String,
    pub created_at: String,
    pub updated_at: String,
}
```

Add methods on `Store`:

```rust
pub fn create_intent_request(
    &self,
    request_id: impl AsRef<str>,
    session_id: impl AsRef<str>,
    workspace_id: impl AsRef<str>,
    resources: &[String],
    action: impl AsRef<str>,
) -> StoreResult<IntentRequestRecord> {
    let resources_json = serde_json::to_string(resources)
        .map_err(|err| rusqlite::Error::ToSqlConversionFailure(Box::new(err)))?;
    self.conn.execute(
        "INSERT OR IGNORE INTO intent_requests (
            request_id, session_id, workspace_id, resources_json, action, status, created_at, updated_at
         ) VALUES (?1, ?2, ?3, ?4, ?5, 'requested', ?6, ?6)",
        params![
            request_id.as_ref(),
            session_id.as_ref(),
            workspace_id.as_ref(),
            resources_json,
            action.as_ref(),
            "2026-05-31T00:00:00Z"
        ],
    )?;
    self.intent_request(request_id.as_ref())?
        .ok_or_else(|| StoreError::Sqlite(rusqlite::Error::QueryReturnedNoRows))
}

pub fn intent_request(
    &self,
    request_id: impl AsRef<str>,
) -> StoreResult<Option<IntentRequestRecord>> {
    self.conn
        .query_row(
            "SELECT request_id, session_id, workspace_id, resources_json, action, status, created_at, updated_at
             FROM intent_requests WHERE request_id = ?1",
            [request_id.as_ref()],
            intent_request_from_row,
        )
        .optional()
        .map_err(StoreError::from)
}

pub fn intent_request_count(&self) -> StoreResult<u64> {
    self.conn
        .query_row("SELECT COUNT(*) FROM intent_requests", [], |row| row.get::<_, u64>(0))
        .map_err(StoreError::from)
}
```

Add `intent_request_from_row` near the existing row mappers:

```rust
fn intent_request_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<IntentRequestRecord> {
    let resources_json: String = row.get(3)?;
    let resources = serde_json::from_str(&resources_json).map_err(|err| {
        rusqlite::Error::FromSqlConversionFailure(
            3,
            rusqlite::types::Type::Text,
            Box::new(err),
        )
    })?;
    Ok(IntentRequestRecord {
        request_id: row.get(0)?,
        session_id: row.get(1)?,
        workspace_id: row.get(2)?,
        resources,
        action: row.get(4)?,
        status: row.get(5)?,
        created_at: row.get(6)?,
        updated_at: row.get(7)?,
    })
}
```

- [ ] **Step 5: Add request-aware queue and cancel methods**

Change `enqueue_waiter` to call a private method that accepts `request_id: Option<&str>`, then add:

```rust
pub fn enqueue_waiter_for_request(
    &self,
    request_id: impl AsRef<str>,
    session_id: impl AsRef<str>,
    workspace_id: impl AsRef<str>,
    relative_path: impl AsRef<str>,
    action: impl AsRef<str>,
    blocking_session_id: Option<&str>,
) -> StoreResult<WaitRecord> {
    self.enqueue_waiter_inner(
        Some(request_id.as_ref()),
        session_id.as_ref(),
        workspace_id.as_ref(),
        relative_path.as_ref(),
        action.as_ref(),
        blocking_session_id,
    )
}

pub fn cancel_intent_request(
    &self,
    request_id: impl AsRef<str>,
    session_id: impl AsRef<str>,
) -> StoreResult<()> {
    let request = self.intent_request(request_id.as_ref())?;
    let Some(request) = request else {
        return Err(StoreError::ReservationOwnerMismatch);
    };
    if request.session_id != session_id.as_ref() {
        return Err(StoreError::ReservationOwnerMismatch);
    }
    self.conn.execute(
        "UPDATE intent_requests SET status = 'cancelled', updated_at = ?1
         WHERE request_id = ?2 AND session_id = ?3",
        params!["2026-05-31T00:00:00Z", request_id.as_ref(), session_id.as_ref()],
    )?;
    self.conn.execute(
        "UPDATE wait_queue SET status = 'cancelled'
         WHERE request_id = ?1 AND session_id = ?2 AND status IN ('queued', 'reserved')",
        params![request_id.as_ref(), session_id.as_ref()],
    )?;
    Ok(())
}
```

- [ ] **Step 6: Run store tests**

Run:

```bash
cargo test -p stateful-store intent_requests_are_idempotent_by_request_id cancelling_request_cancels_owned_waiters_only
cargo test -p stateful-store migrations_create_contract_tables_and_indexes
```

Expected: PASS.

- [ ] **Step 7: Commit scheduling persistence**

```bash
git add crates/stateful-store/src/lib.rs crates/stateful-store/tests/event_store.rs
git commit -m "feat: persist idempotent intent requests"
```

---

### Task 3: Policy Service Extraction

**Files:**
- Create: `crates/stateful-server/src/policy_service.rs`
- Modify: `crates/stateful-server/src/lib.rs`
- Modify: `crates/stateful-server/tests/routes.rs`

- [ ] **Step 1: Write route test that preserves authorize behavior through service**

Append this test in `crates/stateful-server/tests/routes.rs`:

```rust
#[tokio::test]
async fn authorize_uses_policy_service_and_preserves_scope_decision() {
    let app = build_router(ServerConfig::new("secret-token"));

    let declare = app
        .clone()
        .oneshot(protocol_request(
            "/v1/intent/declare",
            "req-policy-declare",
            "s1",
            "w1",
            serde_json::json!({
                "files_planned": ["src/auth.ts"]
            }),
        ))
        .await
        .expect("intent declaration should complete");
    assert_eq!(declare.status(), StatusCode::OK);

    let response = app
        .oneshot(protocol_request(
            "/v1/authorize",
            "req-policy-authorize",
            "s1",
            "w1",
            serde_json::json!({
                "action": "write_file",
                "path": "src/session.ts"
            }),
        ))
        .await
        .expect("authorize should complete");

    assert_eq!(response.status(), StatusCode::OK);
    let body = to_bytes(response.into_body(), 2048)
        .await
        .expect("body should read");
    let json: serde_json::Value = serde_json::from_slice(&body).expect("body should be json");
    assert_eq!(json["decision"], "deny");
    assert_eq!(json["reason_code"], "scope_mismatch");
}
```

- [ ] **Step 2: Run the test and verify failure**

Run:

```bash
cargo test -p stateful-server authorize_uses_policy_service_and_preserves_scope_decision --test routes
```

Expected: FAIL because `/v1/authorize` still expects legacy request shape and policy logic lives directly in the handler helper.

- [ ] **Step 3: Create policy service module**

Create `crates/stateful-server/src/policy_service.rs`:

```rust
use serde_json::{Value, json};
use stateful_core::{AuthorizationInput, Decision, DecisionKind};
use stateful_store::{Event, Store, WaitRecord};

#[derive(Debug, Clone)]
pub struct WriteAuthorizationRequest {
    pub request_id: String,
    pub session_id: String,
    pub workspace_id: Option<String>,
    pub action: String,
    pub path: String,
    pub old_path: Option<String>,
    pub new_path: Option<String>,
    pub queue_on_conflict: bool,
    pub allow_queue_side_effects: bool,
}

#[derive(Debug, Clone)]
pub struct PolicyOutcome {
    pub decision: Decision,
    pub wait: Option<WaitQueueOutcome>,
    pub reservation: Option<WaitRecord>,
}

#[derive(Debug, Clone)]
pub struct WaitQueueOutcome {
    pub record: WaitRecord,
    pub queue_position: Option<u64>,
}

pub struct PolicyService<'a> {
    store: &'a Store,
}

impl<'a> PolicyService<'a> {
    pub fn new(store: &'a Store) -> Self {
        Self { store }
    }

    pub fn authorize_write(
        &self,
        input: WriteAuthorizationRequest,
    ) -> Result<PolicyOutcome, String> {
        let authorization_input = authorization_input(&input)?;
        let mut active_reservation = None;
        if let Some(workspace_id) = &input.workspace_id {
            if let Some(reservation) = self
                .store
                .active_reservation(workspace_id, &input.path)
                .map_err(|error| error.to_string())?
            {
                if reservation.session_id != input.session_id {
                    return Ok(PolicyOutcome {
                        decision: Decision::deny(
                            "reservation_conflict",
                            "Write target is reserved for the next waiting session.",
                            "Call state.intent.claim for the active reservation before writing.",
                        ),
                        wait: None,
                        reservation: Some(reservation),
                    });
                }
                return Ok(PolicyOutcome {
                    decision: Decision::deny(
                        "reservation_requires_claim",
                        "Reserved writes require an explicit claim before authorization.",
                        "Reread the target, then call state.intent.claim for the reservation.",
                    ),
                    wait: None,
                    reservation: Some(reservation),
                });
            }

            if let Some(owner) = self
                .store
                .active_lease_owner(workspace_id, &input.path)
                .map_err(|error| error.to_string())?
                && owner != input.session_id
            {
                let wait = if input.allow_queue_side_effects && input.queue_on_conflict {
                    let waiter = self
                        .store
                        .enqueue_waiter(
                            &input.session_id,
                            workspace_id,
                            &input.path,
                            &input.action,
                            Some(&owner),
                        )
                        .map_err(|error| error.to_string())?;
                    let queue_position = self
                        .store
                        .queue_position(&waiter.wait_id)
                        .map_err(|error| error.to_string())?;
                    Some(WaitQueueOutcome {
                        record: waiter,
                        queue_position,
                    })
                } else {
                    None
                };
                return Ok(PolicyOutcome {
                    decision: Decision::deny(
                        "active_lease_conflict",
                        "Write target is covered by another active session lease.",
                        "Refresh current state, coordinate with the lease owner, or wait for the lease to release.",
                    ),
                    wait,
                    reservation: None,
                });
            }
        }

        let policy_state = self
            .store
            .policy_state_for_session(&input.session_id)
            .map_err(|error| error.to_string())?;
        let decision = stateful_core::authorize_action(&policy_state, authorization_input);
        Ok(PolicyOutcome {
            decision,
            wait: None,
            reservation: None,
        })
    }
}

fn authorization_input(input: &WriteAuthorizationRequest) -> Result<AuthorizationInput, String> {
    match input.action.as_str() {
        "write_file" => Ok(AuthorizationInput::write_file(&input.path)),
        "delete_file" => Ok(AuthorizationInput::delete_file(&input.path)),
        "rename_file" => Ok(AuthorizationInput::rename_file(
            input.old_path.as_deref().unwrap_or(input.path.as_str()),
            input.new_path.as_deref().unwrap_or(input.path.as_str()),
        )),
        "move_file" => Ok(AuthorizationInput::move_file(
            input.old_path.as_deref().unwrap_or(input.path.as_str()),
            input.new_path.as_deref().unwrap_or(input.path.as_str()),
        )),
        _ => Err("unsupported_action".to_string()),
    }
}

pub fn policy_outcome_json(outcome: PolicyOutcome) -> Value {
    let decision = outcome.decision;
    let mut value = json!({
        "decision": match decision.decision {
            DecisionKind::Allow => "allow",
            DecisionKind::Warn => "warn",
            DecisionKind::Deny => "deny",
            DecisionKind::Error => "error",
        },
        "reason_code": decision.reason_code,
        "message": decision.message,
        "required_next_action": decision.required_next_action,
    });
    if let Some(wait) = outcome.wait {
        value["wait"] = json!({
            "wait_id": wait.record.wait_id,
            "session_id": wait.record.session_id,
            "workspace_id": wait.record.workspace_id,
            "relative_path": wait.record.relative_path,
            "action": wait.record.action,
            "status": wait.record.status,
            "queue_position": wait.queue_position,
            "blocking_session_id": wait.record.blocking_session_id,
        });
    }
    if let Some(reservation) = outcome.reservation {
        value["reservation"] = json!({
            "wait_id": reservation.wait_id,
            "session_id": reservation.session_id,
            "workspace_id": reservation.workspace_id,
            "relative_path": reservation.relative_path,
            "action": reservation.action,
            "status": reservation.status,
            "reservation_expires_at": reservation.reservation_expires_at,
        });
    }
    value
}
```

- [ ] **Step 4: Route `/v1/authorize` through protocol and policy service**

In `crates/stateful-server/src/lib.rs`, add:

```rust
mod policy_service;
use policy_service::{PolicyService, WriteAuthorizationRequest, policy_outcome_json};
```

Change `authorize` and `conflicts_check` to accept `ProtocolRequest<AuthorizePayload>`. Rename the old `AuthorizeRequest` body to:

```rust
#[derive(Debug, Deserialize)]
struct AuthorizePayload {
    #[serde(default)]
    queue_on_conflict: bool,
    action: String,
    #[serde(default)]
    old_path: Option<String>,
    #[serde(default)]
    new_path: Option<String>,
    path: String,
}
```

In `authorize`, after token and protocol validation:

```rust
let input = match validate_protocol(input) {
    Ok(input) => input,
    Err(error) => return error,
};
let result = config
    .store
    .lock()
    .map_err(|_| "store lock poisoned".to_string())
    .and_then(|store| {
        PolicyService::new(&store).authorize_write(WriteAuthorizationRequest {
            request_id: input.request_id,
            session_id: input.session.session_id,
            workspace_id: Some(input.workspace.workspace_id),
            action: input.payload.action,
            path: input.payload.path,
            old_path: input.payload.old_path,
            new_path: input.payload.new_path,
            queue_on_conflict: input.payload.queue_on_conflict,
            allow_queue_side_effects: true,
        })
    });
```

Return `policy_outcome_json(outcome)`. Keep a legacy compatibility helper only in tests that have not yet migrated; remove `authorize_from_store` after all route tests pass through the service.

- [ ] **Step 5: Migrate route tests for `/v1/authorize` and `/v1/conflicts/check`**

Replace legacy bodies with `protocol_request`. Example:

```rust
let response = app
    .oneshot(protocol_request(
        "/v1/authorize",
        "req-authorize-s1-auth",
        "s1",
        "w1",
        serde_json::json!({
            "action": "write_file",
            "path": "src/auth.ts"
        }),
    ))
    .await
    .expect("authorize should complete");
```

For `/v1/conflicts/check`, use the same body but assert queue side effects do not appear:

```rust
assert!(json.get("wait").is_none());
```

- [ ] **Step 6: Run service migration tests**

Run:

```bash
cargo test -p stateful-server authorize_uses_policy_service_and_preserves_scope_decision --test routes
cargo test -p stateful-server --test routes
```

Expected: PASS.

- [ ] **Step 7: Commit policy service extraction**

```bash
git add crates/stateful-server/src/policy_service.rs crates/stateful-server/src/lib.rs crates/stateful-server/tests/routes.rs
git commit -m "feat: route write authorization through policy service"
```

---

### Task 4: Explicit Scheduling API

**Files:**
- Modify: `crates/stateful-server/src/policy_service.rs`
- Modify: `crates/stateful-server/src/lib.rs`
- Modify: `crates/stateful-server/tests/routes.rs`
- Modify: `crates/stateful-store/src/lib.rs`

- [ ] **Step 1: Write failing scheduling route tests**

Append these tests in `crates/stateful-server/tests/routes.rs`:

```rust
#[tokio::test]
async fn intent_request_grants_available_resource_and_is_idempotent() {
    let store = Store::open_in_memory().expect("store should open");
    let app = build_router(ServerConfig::with_store("secret-token", store));

    let first = app
        .clone()
        .oneshot(protocol_request(
            "/v1/intent/request",
            "req-schedule-1",
            "s1",
            "w1",
            serde_json::json!({
                "action": "write_file",
                "resources": ["src/auth.ts"]
            }),
        ))
        .await
        .expect("intent request should complete");
    assert_eq!(first.status(), StatusCode::OK);
    let body = to_bytes(first.into_body(), 2048)
        .await
        .expect("body should read");
    let json: serde_json::Value = serde_json::from_slice(&body).expect("body should be json");
    assert_eq!(json["request"]["status"], "granted");

    let second = app
        .oneshot(protocol_request(
            "/v1/intent/request",
            "req-schedule-1",
            "s1",
            "w1",
            serde_json::json!({
                "action": "write_file",
                "resources": ["src/auth.ts"]
            }),
        ))
        .await
        .expect("duplicate intent request should complete");
    let body = to_bytes(second.into_body(), 2048)
        .await
        .expect("body should read");
    let json: serde_json::Value = serde_json::from_slice(&body).expect("body should be json");
    assert_eq!(json["request"]["request_id"], "req-schedule-1");
    assert_eq!(json["request"]["status"], "granted");
}

#[tokio::test]
async fn intent_request_queues_blocked_resource_and_claim_creates_lease() {
    let store = Store::open_in_memory().expect("store should open");
    let app = build_router(ServerConfig::with_store("secret-token", store));

    let lease = app
        .clone()
        .oneshot(protocol_request(
            "/v1/lease/acquire",
            "req-lease-owner",
            "s1",
            "w1",
            serde_json::json!({
                "path": "src/auth.ts"
            }),
        ))
        .await
        .expect("lease acquire should complete");
    assert_eq!(lease.status(), StatusCode::OK);

    let queued = app
        .clone()
        .oneshot(protocol_request(
            "/v1/intent/request",
            "req-schedule-2",
            "s2",
            "w1",
            serde_json::json!({
                "action": "write_file",
                "resources": ["src/auth.ts"]
            }),
        ))
        .await
        .expect("intent request should complete");
    let body = to_bytes(queued.into_body(), 2048)
        .await
        .expect("body should read");
    let json: serde_json::Value = serde_json::from_slice(&body).expect("body should be json");
    assert_eq!(json["request"]["status"], "queued");
    let wait_id = json["wait"]["wait_id"].as_str().expect("wait id should be string").to_string();

    let release = app
        .clone()
        .oneshot(protocol_request(
            "/v1/lease/release",
            "req-release-owner",
            "s1",
            "w1",
            serde_json::json!({
                "path": "src/auth.ts"
            }),
        ))
        .await
        .expect("lease release should complete");
    assert_eq!(release.status(), StatusCode::OK);

    let claim = app
        .clone()
        .oneshot(protocol_request(
            "/v1/intent/claim",
            "req-claim-s2",
            "s2",
            "w1",
            serde_json::json!({
                "wait_id": wait_id,
                "resources": ["src/auth.ts"]
            }),
        ))
        .await
        .expect("claim should complete");
    assert_eq!(claim.status(), StatusCode::OK);
    let body = to_bytes(claim.into_body(), 2048)
        .await
        .expect("body should read");
    let json: serde_json::Value = serde_json::from_slice(&body).expect("body should be json");
    assert_eq!(json["request"]["status"], "claimed");

    let write = app
        .oneshot(protocol_request(
            "/v1/authorize",
            "req-write-after-claim",
            "s2",
            "w1",
            serde_json::json!({
                "action": "write_file",
                "path": "src/auth.ts"
            }),
        ))
        .await
        .expect("write authorize should complete");
    let body = to_bytes(write.into_body(), 2048)
        .await
        .expect("body should read");
    let json: serde_json::Value = serde_json::from_slice(&body).expect("body should be json");
    assert_eq!(json["decision"], "allow");
}

#[tokio::test]
async fn reserved_session_write_before_claim_is_denied() {
    let store = Store::open_in_memory().expect("store should open");
    let app = build_router(ServerConfig::with_store("secret-token", store));

    let lease = app
        .clone()
        .oneshot(protocol_request(
            "/v1/lease/acquire",
            "req-lease-owner-deny-before-claim",
            "s1",
            "w1",
            serde_json::json!({ "path": "src/auth.ts" }),
        ))
        .await
        .expect("lease acquire should complete");
    assert_eq!(lease.status(), StatusCode::OK);

    let queued = app
        .clone()
        .oneshot(protocol_request(
            "/v1/intent/request",
            "req-schedule-deny-before-claim",
            "s2",
            "w1",
            serde_json::json!({
                "action": "write_file",
                "resources": ["src/auth.ts"]
            }),
        ))
        .await
        .expect("intent request should complete");
    assert_eq!(queued.status(), StatusCode::OK);

    let release = app
        .clone()
        .oneshot(protocol_request(
            "/v1/lease/release",
            "req-release-deny-before-claim",
            "s1",
            "w1",
            serde_json::json!({ "path": "src/auth.ts" }),
        ))
        .await
        .expect("lease release should complete");
    assert_eq!(release.status(), StatusCode::OK);

    let write = app
        .oneshot(protocol_request(
            "/v1/authorize",
            "req-write-deny-before-claim",
            "s2",
            "w1",
            serde_json::json!({
                "action": "write_file",
                "path": "src/auth.ts"
            }),
        ))
        .await
        .expect("authorize should complete");
    let body = to_bytes(write.into_body(), 2048)
        .await
        .expect("body should read");
    let json: serde_json::Value = serde_json::from_slice(&body).expect("body should be json");
    assert_eq!(json["decision"], "deny");
    assert_eq!(json["reason_code"], "reservation_requires_claim");
}
```

- [ ] **Step 2: Run scheduling route tests and verify failure**

Run:

```bash
cargo test -p stateful-server intent_request_grants_available_resource_and_is_idempotent --test routes
cargo test -p stateful-server intent_request_queues_blocked_resource_and_claim_creates_lease --test routes
cargo test -p stateful-server reserved_session_write_before_claim_is_denied --test routes
```

Expected: FAIL because `/v1/intent/request`, `/v1/intent/claim`, and protocol-wrapped lease routes do not exist yet.

- [ ] **Step 3: Add scheduling payloads and routes**

In `build_router`, add:

```rust
.route("/v1/intent/request", post(intent_request))
.route("/v1/intent/claim", post(intent_claim))
.route("/v1/intent/cancel", post(intent_cancel))
```

Add payload structs:

```rust
#[derive(Debug, Deserialize)]
struct IntentRequestPayload {
    action: String,
    resources: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct IntentClaimPayload {
    wait_id: String,
    resources: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct IntentCancelPayload {
    target_request_id: String,
}
```

- [ ] **Step 4: Add policy service scheduling methods**

Add methods to `PolicyService`:

```rust
pub fn request_intent(
    &self,
    request_id: &str,
    session_id: &str,
    workspace_id: &str,
    action: &str,
    resources: &[String],
) -> Result<serde_json::Value, String> {
    let request = self
        .store
        .create_intent_request(request_id, session_id, workspace_id, resources, action)
        .map_err(|error| error.to_string())?;
    if request.status == "granted" || request.status == "queued" {
        return Ok(json!({ "status": "ok", "request": request }));
    }

    for resource in resources {
        if let Some(owner) = self
            .store
            .active_lease_owner(workspace_id, resource)
            .map_err(|error| error.to_string())?
        {
            let waiter = self
                .store
                .enqueue_waiter_for_request(
                    request_id,
                    session_id,
                    workspace_id,
                    resource,
                    action,
                    Some(&owner),
                )
                .map_err(|error| error.to_string())?;
            self.store
                .mark_intent_request_status(request_id, "queued")
                .map_err(|error| error.to_string())?;
            return Ok(json!({
                "status": "ok",
                "request": self.store.intent_request(request_id).map_err(|error| error.to_string())?,
                "wait": waiter,
                "required_next_action": "Wait for a reservation, then reread the resource and call state.intent.claim."
            }));
        }
    }

    for resource in resources {
        self.store
            .acquire_lease(session_id, workspace_id, resource)
            .map_err(|error| error.to_string())?;
    }
    self.store
        .append(Event::intent_declared(session_id, workspace_id, resources.iter().map(String::as_str)))
        .map_err(|error| error.to_string())?;
    self.store
        .mark_intent_request_status(request_id, "granted")
        .map_err(|error| error.to_string())?;
    Ok(json!({
        "status": "ok",
        "request": self.store.intent_request(request_id).map_err(|error| error.to_string())?,
        "required_next_action": null
    }))
}
```

Add the missing store method:

```rust
pub fn mark_intent_request_status(
    &self,
    request_id: impl AsRef<str>,
    status: impl AsRef<str>,
) -> StoreResult<()> {
    self.conn.execute(
        "UPDATE intent_requests SET status = ?1, updated_at = ?2 WHERE request_id = ?3",
        params![status.as_ref(), "2026-05-31T00:00:00Z", request_id.as_ref()],
    )?;
    Ok(())
}
```

Add `claim_intent`:

```rust
pub fn claim_intent(
    &self,
    request_id: &str,
    session_id: &str,
    workspace_id: &str,
    wait_id: &str,
    resources: &[String],
) -> Result<serde_json::Value, String> {
    self.store
        .claim_reservation(wait_id, session_id)
        .map_err(|error| error.to_string())?;
    for resource in resources {
        self.store
            .acquire_lease(session_id, workspace_id, resource)
            .map_err(|error| error.to_string())?;
    }
    self.store
        .append(Event::intent_declared(session_id, workspace_id, resources.iter().map(String::as_str)))
        .map_err(|error| error.to_string())?;
    self.store
        .mark_intent_request_status(request_id, "claimed")
        .map_err(|error| error.to_string())?;
    Ok(json!({
        "status": "ok",
        "request": self.store.intent_request(request_id).map_err(|error| error.to_string())?,
        "wait_id": wait_id,
        "required_next_action": null
    }))
}
```

If `Event::intent_declared` does not accept an iterator in the current code, collect first:

```rust
let files = resources.iter().map(String::as_str).collect::<Vec<_>>();
self.store.append(Event::intent_declared(session_id, workspace_id, files))?;
```

- [ ] **Step 5: Implement scheduling route handlers**

Each handler validates token, validates protocol, locks the store, calls `PolicyService`, and returns JSON. Example for request:

```rust
async fn intent_request(
    State(config): State<ServerConfig>,
    headers: HeaderMap,
    Json(input): Json<ProtocolRequest<IntentRequestPayload>>,
) -> (StatusCode, Json<Value>) {
    if !has_valid_bearer_token(&headers, &config.bearer_token) {
        return unauthorized();
    }
    let input = match validate_protocol(input) {
        Ok(input) => input,
        Err(error) => return error,
    };
    let result = config
        .store
        .lock()
        .map_err(|_| "store lock poisoned".to_string())
        .and_then(|store| {
            PolicyService::new(&store).request_intent(
                &input.request_id,
                &input.session.session_id,
                &input.workspace.workspace_id,
                &input.payload.action,
                &input.payload.resources,
            )
        });
    match result {
        Ok(value) => (StatusCode::OK, Json(value)),
        Err(message) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "decision": "error", "reason_code": "state_error", "message": message })),
        ),
    }
}
```

Use the same pattern for claim and cancel.

- [ ] **Step 6: Migrate lease acquire/release to protocol request bodies**

Change `lease_acquire` and `lease_release` to accept `ProtocolRequest<LeasePayload>`:

```rust
#[derive(Debug, Deserialize)]
struct LeasePayload {
    path: String,
}
```

Use `input.session.session_id` and `input.workspace.workspace_id` when calling store methods.

- [ ] **Step 7: Run scheduling API tests**

Run:

```bash
cargo test -p stateful-server intent_request_grants_available_resource_and_is_idempotent --test routes
cargo test -p stateful-server intent_request_queues_blocked_resource_and_claim_creates_lease --test routes
cargo test -p stateful-server reserved_session_write_before_claim_is_denied --test routes
cargo test -p stateful-server --test routes
```

Expected: PASS.

- [ ] **Step 8: Commit scheduling API**

```bash
git add crates/stateful-server/src/lib.rs crates/stateful-server/src/policy_service.rs crates/stateful-server/tests/routes.rs crates/stateful-store/src/lib.rs
git commit -m "feat: add explicit intent scheduling api"
```

---

### Task 5: Remaining Protocol Routes, CLI, MCP, and Runtime Bodies

**Files:**
- Modify: `crates/stateful-server/src/lib.rs`
- Modify: `crates/stateful-server/tests/routes.rs`
- Modify: `crates/stateful-cli/src/runtime.rs`
- Modify: `crates/stateful-cli/src/lib.rs`
- Modify: `crates/stateful-cli/src/hook.rs`
- Modify: `crates/stateful-mcp/src/lib.rs`
- Modify: `crates/stateful-cli/tests/hook.rs`
- Modify: `crates/stateful-mcp/tests/tools.rs`

- [ ] **Step 1: Write failing remaining-route protocol test**

Append this test in `crates/stateful-server/tests/routes.rs`:

```rust
#[tokio::test]
async fn remaining_side_effecting_routes_require_protocol_metadata() {
    let app = build_router(ServerConfig::new("secret-token"));

    for (path, body) in [
        (
            "/v1/session/register",
            serde_json::json!({
                "session_id": "s1",
                "workspace_id": "w1"
            }),
        ),
        (
            "/v1/session/heartbeat",
            serde_json::json!({
                "session_id": "s1",
                "workspace_id": "w1"
            }),
        ),
        (
            "/v1/activity/observe",
            serde_json::json!({
                "session_id": "s1",
                "workspace_id": "w1"
            }),
        ),
        (
            "/v1/activity/finalize",
            serde_json::json!({
                "session_id": "s1",
                "workspace_id": "w1"
            }),
        ),
        (
            "/v1/reconcile/ack",
            serde_json::json!({
                "session_id": "s1",
                "workspace_id": "w1",
                "decision": "adopt",
                "files_reread": ["src/auth.ts"],
                "human_change_summary": "reviewed"
            }),
        ),
        (
            "/v1/outbox/sync",
            serde_json::json!({
                "outbox_id": "outbox-1",
                "session_id": "s1",
                "workspace_id": "w1",
                "sequence": 1,
                "event_type": "SessionHeartbeatQueued",
                "payload": {}
            }),
        ),
    ] {
        let response = app
            .clone()
            .oneshot(json_request(path, body))
            .await
            .expect("request should complete");
        assert_eq!(response.status(), StatusCode::BAD_REQUEST, "{path}");
    }
}
```

- [ ] **Step 2: Run remaining-route protocol test and verify failure**

Run:

```bash
cargo test -p stateful-server remaining_side_effecting_routes_require_protocol_metadata --test routes
```

Expected: FAIL because those handlers still accept legacy bodies.

- [ ] **Step 3: Migrate remaining side-effecting server handlers**

Change these handlers to accept `ProtocolRequest<T>` and use protocol session/workspace metadata:

```text
session_register
session_heartbeat
activity_observe
activity_finalize
reconcile_ack
validation_run
outbox_sync
```

Add payload structs:

```rust
#[derive(Debug, Default, Deserialize)]
struct EmptyPayload {
    #[serde(flatten)]
    identity: WorkspaceIdentityRequest,
}

#[derive(Debug, Deserialize)]
struct ReconcileAckPayload {
    decision: String,
    files_reread: Vec<String>,
    human_change_summary: String,
}

#[derive(Debug, Deserialize)]
struct OutboxSyncPayload {
    outbox_id: String,
    sequence: u64,
    event_type: String,
    payload: Value,
}

#[derive(Debug, Deserialize)]
struct ValidationRunPayload {
    repo_root: PathBuf,
    profile: String,
}
```

For `session_register`, use:

```rust
let input = match validate_protocol(input) {
    Ok(input) => input,
    Err(error) => return error,
};
let event = Event::session_registered(
    input.session.session_id,
    input.workspace.workspace_id,
);
let identity = WorkspaceIdentityRequest {
    repo_id: Some(input.workspace.repo_id),
    worktree_id: Some(input.workspace.worktree_id),
    root: Some(input.workspace.root),
    branch: Some(input.workspace.branch),
};
append_event_response(&config.store, with_request_identity(event, identity))
```

For `validation_run`, use `input.workspace.workspace_id` and `input.payload.profile`.

- [ ] **Step 4: Run migrated server route tests**

Run:

```bash
cargo test -p stateful-server remaining_side_effecting_routes_require_protocol_metadata --test routes
cargo test -p stateful-server --test routes
```

Expected: PASS after route tests use `protocol_request` for migrated side-effecting endpoints.

- [ ] **Step 5: Write failing runtime envelope unit test**

Add this test at the bottom of `crates/stateful-cli/src/runtime.rs` under a `#[cfg(test)]` module:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn protocol_body_wraps_payload_with_session_workspace_and_source() {
        let body = protocol_body(
            "req-1",
            "s1",
            "w1",
            serde_json::json!({
                "path": "src/auth.ts"
            }),
            "cli",
            "lease.acquire",
        );

        assert_eq!(body["protocol_version"], "stateful.v1");
        assert_eq!(body["request_id"], "req-1");
        assert_eq!(body["session"]["session_id"], "s1");
        assert_eq!(body["workspace"]["workspace_id"], "w1");
        assert_eq!(body["source"]["kind"], "cli");
        assert_eq!(body["source"]["event"], "lease.acquire");
        assert_eq!(body["path"], "src/auth.ts");
    }
}
```

- [ ] **Step 6: Run the runtime test and verify failure**

Run:

```bash
cargo test -p stateful-cli protocol_body_wraps_payload_with_session_workspace_and_source
```

Expected: FAIL because `protocol_body` does not exist.

- [ ] **Step 7: Add runtime protocol helper**

In `crates/stateful-cli/src/runtime.rs`, add:

```rust
pub fn protocol_body(
    request_id: impl Into<String>,
    session_id: impl Into<String>,
    workspace_id: impl Into<String>,
    payload: serde_json::Value,
    source_kind: impl Into<String>,
    source_event: impl Into<String>,
) -> serde_json::Value {
    let session_id = session_id.into();
    let workspace_id = workspace_id.into();
    let mut body = serde_json::json!({
        "protocol_version": "stateful.v1",
        "request_id": request_id.into(),
        "session": {
            "session_id": session_id,
            "actor_id": "agent",
            "actor_type": "agent"
        },
        "workspace": {
            "workspace_id": workspace_id,
            "root": "",
            "repo_id": "",
            "worktree_id": "",
            "branch": ""
        },
        "source": {
            "kind": source_kind.into(),
            "event": source_event.into(),
            "source_ref": "stateful-cli"
        }
    });
    let object = body.as_object_mut().expect("protocol body should be object");
    for (key, value) in payload.as_object().expect("payload should be object") {
        object.insert(key.clone(), value.clone());
    }
    body
}
```

Later tasks can enrich workspace identity from `RepoIdentity`; this helper gives every side-effecting request protocol metadata immediately.

- [ ] **Step 8: Update CLI commands to use protocol bodies**

Modify `declare_intent_via_http`, `Command::Notifications`, `Command::Resume`, `Command::Validate` HTTP paths, and hook post bodies to use `protocol_body`. Example for intent declaration:

```rust
let body = protocol_body(
    uuid::Uuid::new_v4().to_string(),
    args.session_id,
    args.workspace_id,
    serde_json::json!({
        "files_planned": args.files_planned
    }),
    "cli",
    "intent.declare",
);
```

If `stateful-cli` does not already depend on `uuid`, add to `crates/stateful-cli/Cargo.toml`:

```toml
uuid.workspace = true
```

- [ ] **Step 9: Update MCP mapping to require protocol metadata**

In `crates/stateful-mcp/src/lib.rs`, keep the same `state.*` tools. For tool calls that already include `session_id` and `workspace_id`, transform the HTTP body:

```rust
fn protocol_tool_body(tool_name: &str, mut arguments: Value) -> Value {
    let session_id = arguments
        .get("session_id")
        .and_then(Value::as_str)
        .unwrap_or("unknown")
        .to_string();
    let workspace_id = arguments
        .get("workspace_id")
        .and_then(Value::as_str)
        .unwrap_or("unknown")
        .to_string();
    let request_id = format!("mcp-{tool_name}-{session_id}");
    let mut body = serde_json::json!({
        "protocol_version": "stateful.v1",
        "request_id": request_id,
        "session": {
            "session_id": session_id,
            "actor_id": "agent",
            "actor_type": "agent"
        },
        "workspace": {
            "workspace_id": workspace_id,
            "root": "",
            "repo_id": "",
            "worktree_id": "",
            "branch": ""
        },
        "source": {
            "kind": "mcp",
            "event": tool_name,
            "source_ref": "stateful-mcp"
        }
    });
    let object = body.as_object_mut().expect("body should be object");
    for (key, value) in arguments.as_object_mut().expect("arguments should be object") {
        object.insert(key.clone(), value.clone());
    }
    body
}
```

Call this helper for POST tools in `map_tool_to_http`. Keep GET tools unchanged.

- [ ] **Step 10: Run CLI and MCP tests**

Run:

```bash
cargo test -p stateful-cli protocol_body_wraps_payload_with_session_workspace_and_source
cargo test -p stateful-cli
cargo test -p stateful-mcp
```

Expected: PASS.

- [ ] **Step 11: Commit remaining protocol migration**

```bash
git add crates/stateful-server/src/lib.rs crates/stateful-server/tests/routes.rs crates/stateful-cli/src/runtime.rs crates/stateful-cli/src/lib.rs crates/stateful-cli/src/hook.rs crates/stateful-cli/Cargo.toml crates/stateful-mcp/src/lib.rs crates/stateful-cli/tests/hook.rs crates/stateful-mcp/tests/tools.rs
git commit -m "feat: send protocol metadata across stateful clients"
```

---

### Task 6: Lazy Clock and Expiration

**Files:**
- Modify: `Cargo.toml`
- Create: `crates/stateful-store/src/clock.rs`
- Modify: `crates/stateful-store/src/lib.rs`
- Modify: `crates/stateful-store/tests/event_store.rs`

- [ ] **Step 1: Write failing lazy expiration tests**

Append these tests to `crates/stateful-store/tests/event_store.rs`:

```rust
#[test]
fn expired_lease_no_longer_blocks_active_owner_lookup() {
    let clock = stateful_store::clock::FixedClock::new("2026-05-31T00:00:00Z");
    let store = Store::open_in_memory_with_clock(clock.clone()).expect("store should open");

    store
        .acquire_lease("s1", "w1", "src/auth.ts")
        .expect("lease should acquire");
    clock.set("2026-05-31T00:16:00Z");

    assert_eq!(
        store
            .active_lease_owner("w1", "src/auth.ts")
            .expect("lease owner should load"),
        None
    );
}

#[test]
fn expired_reservation_promotes_next_waiter_lazily() {
    let clock = stateful_store::clock::FixedClock::new("2026-05-31T00:00:00Z");
    let store = Store::open_in_memory_with_clock(clock.clone()).expect("store should open");

    let first = store
        .enqueue_waiter("s2", "w1", "src/auth.ts", "write_file", Some("s1"))
        .expect("first waiter should enqueue");
    let second = store
        .enqueue_waiter("s3", "w1", "src/auth.ts", "write_file", Some("s1"))
        .expect("second waiter should enqueue");
    store
        .promote_next_waiter("w1", "src/auth.ts")
        .expect("first waiter should promote");

    clock.set("2026-05-31T00:03:00Z");
    let reservation = store
        .active_reservation("w1", "src/auth.ts")
        .expect("reservation lookup should succeed")
        .expect("second waiter should become active reservation");

    assert_eq!(
        store
            .waiter_status(&first.wait_id)
            .expect("first waiter status should load"),
        Some("expired".to_string())
    );
    assert_eq!(reservation.wait_id, second.wait_id);
    assert_eq!(reservation.session_id, "s3");
}
```

- [ ] **Step 2: Run expiration tests and verify failure**

Run:

```bash
cargo test -p stateful-store expired_lease_no_longer_blocks_active_owner_lookup expired_reservation_promotes_next_waiter_lazily
```

Expected: FAIL because `clock` module and `open_in_memory_with_clock` do not exist, and store timestamps are fixed strings.

- [ ] **Step 3: Add clock module**

In the workspace `Cargo.toml`, update the `time` dependency features:

```toml
time = { version = "0.3", features = ["serde", "macros", "formatting", "parsing"] }
```

Create `crates/stateful-store/src/clock.rs`:

```rust
use std::sync::{Arc, Mutex};
use time::OffsetDateTime;

pub trait Clock: Send + Sync {
    fn now(&self) -> OffsetDateTime;
}

#[derive(Debug)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> OffsetDateTime {
        OffsetDateTime::now_utc()
    }
}

#[derive(Debug, Clone)]
pub struct FixedClock {
    value: Arc<Mutex<OffsetDateTime>>,
}

impl FixedClock {
    pub fn new(value: &str) -> Self {
        Self {
            value: Arc::new(Mutex::new(
                OffsetDateTime::parse(value, &time::format_description::well_known::Rfc3339)
                    .expect("fixed clock value should parse"),
            )),
        }
    }

    pub fn set(&self, value: &str) {
        *self.value.lock().expect("clock mutex should lock") =
            OffsetDateTime::parse(value, &time::format_description::well_known::Rfc3339)
                .expect("fixed clock value should parse");
    }
}

impl Clock for FixedClock {
    fn now(&self) -> OffsetDateTime {
        *self.value.lock().expect("clock mutex should lock")
    }
}

pub(crate) fn format_time(value: OffsetDateTime) -> String {
    value
        .format(&time::format_description::well_known::Rfc3339)
        .expect("time should format")
}
```

Expose the module at the top of `crates/stateful-store/src/lib.rs`:

```rust
pub mod clock;
use clock::{Clock, SystemClock, format_time};
use std::sync::Arc;
use time::Duration;
```

- [ ] **Step 4: Add clock to Store**

Change `Store`:

```rust
pub struct Store {
    conn: Connection,
    clock: Arc<dyn Clock>,
}
```

Update constructors:

```rust
pub fn open(path: impl AsRef<Path>) -> StoreResult<Self> {
    Self::open_with_clock(path, Arc::new(SystemClock))
}

pub fn open_with_clock(path: impl AsRef<Path>, clock: Arc<dyn Clock>) -> StoreResult<Self> {
    let path = path.as_ref();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let conn = Connection::open(path)?;
    let store = Self { conn, clock };
    store.migrate()?;
    Ok(store)
}

pub fn open_in_memory() -> StoreResult<Self> {
    let conn = Connection::open_in_memory()?;
    let store = Self {
        conn,
        clock: Arc::new(SystemClock),
    };
    store.migrate()?;
    Ok(store)
}

pub fn open_in_memory_with_clock<C>(clock: C) -> StoreResult<Self>
where
    C: Clock + 'static,
{
    let conn = Connection::open_in_memory()?;
    let store = Self {
        conn,
        clock: Arc::new(clock),
    };
    store.migrate()?;
    Ok(store)
}
```

Add helper methods:

```rust
fn now_string(&self) -> String {
    format_time(self.clock.now())
}

fn lease_expires_at(&self) -> String {
    format_time(self.clock.now() + Duration::seconds(300))
}

fn reservation_expires_at(&self) -> String {
    format_time(self.clock.now() + Duration::seconds(120))
}
```

- [ ] **Step 5: Replace fixed timestamps and lazy-expire reads**

Replace fixed created/updated/expires strings in lease, wait queue, notifications, activities, validations, outbox, and reconciliation methods with `self.now_string()`, `self.lease_expires_at()`, or `self.reservation_expires_at()`.

Before `active_lease_owner`, run:

```rust
self.expire_leases_for_resource(workspace_id.as_ref(), &relative_path)?;
```

Add:

```rust
fn expire_leases_for_resource(&self, workspace_id: &str, relative_path: &str) -> StoreResult<()> {
    self.conn.execute(
        "UPDATE leases
         SET status = 'expired'
         WHERE workspace_id = ?1
           AND relative_path = ?2
           AND status = 'active'
           AND expires_at IS NOT NULL
           AND expires_at <= ?3",
        params![workspace_id, relative_path, self.now_string()],
    )?;
    self.promote_next_waiter(workspace_id, relative_path)?;
    Ok(())
}
```

Before `active_reservation`, run:

```rust
self.expire_reservations_for_resource(workspace_id.as_ref(), &relative_path)?;
```

Add:

```rust
fn expire_reservations_for_resource(
    &self,
    workspace_id: &str,
    relative_path: &str,
) -> StoreResult<()> {
    self.conn.execute(
        "UPDATE wait_queue
         SET status = 'expired'
         WHERE workspace_id = ?1
           AND relative_path = ?2
           AND status = 'reserved'
           AND reservation_expires_at IS NOT NULL
           AND reservation_expires_at <= ?3",
        params![workspace_id, relative_path, self.now_string()],
    )?;
    self.promote_next_waiter(workspace_id, relative_path)?;
    Ok(())
}
```

- [ ] **Step 6: Run expiration and store tests**

Run:

```bash
cargo test -p stateful-store expired_lease_no_longer_blocks_active_owner_lookup expired_reservation_promotes_next_waiter_lazily
cargo test -p stateful-store
```

Expected: PASS.

- [ ] **Step 7: Commit lazy expiration**

```bash
git add Cargo.toml crates/stateful-store/src/clock.rs crates/stateful-store/src/lib.rs crates/stateful-store/tests/event_store.rs
git commit -m "feat: add lazy expiration with injectable clock"
```

---

### Task 7: Normalized Hook Wrapper

**Files:**
- Modify: `crates/stateful-cli/src/lib.rs`
- Modify: `crates/stateful-cli/src/hook.rs`
- Modify: `crates/stateful-cli/tests/hook.rs`

- [ ] **Step 1: Write failing hook tests**

Append this test in `crates/stateful-cli/tests/hook.rs`:

```rust
#[test]
fn normalized_pre_tool_use_uses_same_authorization_path_as_codex_input() {
    let temp_root = std::env::temp_dir().join(format!(
        "stateful-normalized-hook-test-{}",
        std::process::id()
    ));
    if temp_root.exists() {
        fs::remove_dir_all(&temp_root).expect("old temp root should be removable");
    }
    let paths = GlobalPaths::new(temp_root.join("home"));
    let repo_root = temp_root.join("repo");
    fs::create_dir_all(repo_root.join("src")).expect("repo should be creatable");
    enable_test_repo(&paths, &repo_root);
    let (runtime, rx) = spawn_fake_stateful_server(
        r#"{"decision":"allow","reason_code":"authorized","message":"ok","required_next_action":null}"#,
    );
    write_global_runtime_file(&paths, &runtime).expect("runtime should write");

    let input = serde_json::json!({
        "event": "pre_tool_use",
        "session_id": "s-normalized",
        "cwd": repo_root,
        "tool_name": "Write",
        "tool_input": {
            "file_path": "src/auth.ts"
        },
        "source": {
            "kind": "hook",
            "agent": "generic"
        }
    })
    .to_string();

    let output = run_hook_subprocess(&repo_root, &paths, &["hook", "run", "pre-tool-use"], &input);
    assert!(
        output.status.success(),
        "stateful hook failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let request = rx.recv().expect("fake server should receive request");
    assert!(request.contains("/v1/authorize"));
    assert!(request.contains("src/auth.ts"));

    fs::remove_dir_all(&temp_root).expect("temp root should be removable");
}

#[test]
fn codex_hook_subcommand_remains_compatible() {
    let input = r#"{
      "session_id": "s1",
      "cwd": "/repo",
      "hook_event_name": "PreToolUse",
      "tool_name": "Bash",
      "tool_input": {
        "command": "rg auth src"
      }
    }"#;

    let outcome = stateful_cli::handle_codex_pre_tool_use(input)
        .expect("codex hook input should parse");

    assert_eq!(outcome, HookOutcome::Allow);
}
```

- [ ] **Step 2: Run hook tests and verify failure**

Run:

```bash
cargo test -p stateful-cli normalized_pre_tool_use_uses_same_authorization_path_as_codex_input codex_hook_subcommand_remains_compatible
```

Expected: FAIL because `hook run`, `handle_codex_pre_tool_use`, and normalized event parsing do not exist.

- [ ] **Step 3: Add normalized hook event structs**

In `crates/stateful-cli/src/hook.rs`, add:

```rust
#[derive(Debug, Deserialize)]
struct NormalizedHookInput {
    event: String,
    session_id: String,
    cwd: Option<PathBuf>,
    #[serde(default)]
    tool_name: Option<String>,
    #[serde(default)]
    tool_input: serde_json::Value,
    #[serde(default)]
    source: serde_json::Value,
}

impl NormalizedHookInput {
    fn to_pre_tool_use_input(&self) -> serde_json::Value {
        serde_json::json!({
            "session_id": self.session_id,
            "cwd": self.cwd.as_ref().map(|path| path.to_string_lossy().to_string()).unwrap_or_default(),
            "hook_event_name": "PreToolUse",
            "tool_name": self.tool_name.as_deref().unwrap_or(""),
            "tool_input": self.tool_input,
        })
    }
}
```

Export wrapper helpers from `crates/stateful-cli/src/lib.rs`:

```rust
pub use hook::{handle_codex_pre_tool_use, handle_normalized_hook_in_repo};
```

Add:

```rust
pub fn handle_codex_pre_tool_use(input: &str) -> anyhow::Result<HookOutcome> {
    handle_pre_tool_use(input)
}

pub fn handle_normalized_hook_in_repo(
    input: &str,
    repo_root: impl AsRef<Path>,
) -> anyhow::Result<Option<HookOutcome>> {
    let normalized: NormalizedHookInput = serde_json::from_str(input)?;
    match normalized.event.as_str() {
        "pre_tool_use" => {
            let codex_shape = normalized.to_pre_tool_use_input().to_string();
            handle_pre_tool_use_in_repo(&codex_shape, repo_root).map(Some)
        }
        "post_tool_use" => {
            let body = serde_json::json!({
                "session_id": normalized.session_id,
                "cwd": normalized.cwd,
                "hook_event_name": "PostToolUse"
            })
            .to_string();
            handle_post_tool_use_in_repo(&body, repo_root)?;
            Ok(None)
        }
        _ => Ok(None),
    }
}
```

- [ ] **Step 4: Add CLI hook subcommands**

Change hook command enum in `crates/stateful-cli/src/lib.rs`:

```rust
#[derive(Debug, Subcommand)]
pub enum HookCommand {
    #[command(subcommand)]
    Codex(CodexHookCommand),
    Run {
        event: String,
    },
    SessionStart,
    UserPromptSubmit,
    PreToolUse,
    PostToolUse,
    Stop,
}

#[derive(Debug, Subcommand)]
pub enum CodexHookCommand {
    SessionStart,
    UserPromptSubmit,
    PreToolUse,
    PostToolUse,
    Stop,
}
```

In `run_hook`, handle:

```rust
HookCommand::Codex(command) => run_codex_hook(command),
HookCommand::Run { event: _ } => {
    let mut input = String::new();
    io::stdin().read_to_string(&mut input)?;
    if let Some(outcome) = handle_normalized_hook_in_repo(&input, hook_start_dir(&input)?)?
        && !matches!(outcome, HookOutcome::Allow)
    {
        println!("{}", serde_json::to_string(&outcome.to_stdout_json()?)?);
    }
}
```

Keep legacy variants by mapping each old variant to the same Codex code path.

- [ ] **Step 5: Run hook tests**

Run:

```bash
cargo test -p stateful-cli normalized_pre_tool_use_uses_same_authorization_path_as_codex_input codex_hook_subcommand_remains_compatible
cargo test -p stateful-cli --test hook
```

Expected: PASS.

- [ ] **Step 6: Commit hook wrapper**

```bash
git add crates/stateful-cli/src/lib.rs crates/stateful-cli/src/hook.rs crates/stateful-cli/tests/hook.rs
git commit -m "feat: add normalized hook wrapper"
```

---

### Task 8: Minimal Context Renderer

**Files:**
- Modify: `crates/stateful-core/src/context.rs`
- Modify: `crates/stateful-store/src/lib.rs`
- Modify: `crates/stateful-server/src/lib.rs`
- Modify: `crates/stateful-server/tests/routes.rs`

- [ ] **Step 1: Write failing context route test**

Append this test in `crates/stateful-server/tests/routes.rs`:

```rust
#[tokio::test]
async fn context_render_includes_reserved_and_blocking_state() {
    let store = Store::open_in_memory().expect("store should open");
    let app = build_router(ServerConfig::with_store("secret-token", store));

    let lease = app
        .clone()
        .oneshot(protocol_request(
            "/v1/lease/acquire",
            "req-context-lease",
            "s1",
            "w1",
            serde_json::json!({ "path": "src/auth.ts" }),
        ))
        .await
        .expect("lease acquire should complete");
    assert_eq!(lease.status(), StatusCode::OK);

    let request = app
        .clone()
        .oneshot(protocol_request(
            "/v1/intent/request",
            "req-context-wait",
            "s2",
            "w1",
            serde_json::json!({
                "action": "write_file",
                "resources": ["src/auth.ts"]
            }),
        ))
        .await
        .expect("intent request should complete");
    assert_eq!(request.status(), StatusCode::OK);

    let release = app
        .clone()
        .oneshot(protocol_request(
            "/v1/lease/release",
            "req-context-release",
            "s1",
            "w1",
            serde_json::json!({ "path": "src/auth.ts" }),
        ))
        .await
        .expect("lease release should complete");
    assert_eq!(release.status(), StatusCode::OK);

    let context = app
        .oneshot(protocol_request(
            "/v1/context/render",
            "req-context-render",
            "s2",
            "w1",
            serde_json::json!({
                "mode": "brief",
                "resource": "src/auth.ts"
            }),
        ))
        .await
        .expect("context render should complete");
    assert_eq!(context.status(), StatusCode::OK);
    let body = to_bytes(context.into_body(), 4096)
        .await
        .expect("body should read");
    let json: serde_json::Value = serde_json::from_slice(&body).expect("body should be json");
    let prompt = json["prompt_text"].as_str().expect("prompt text should be string");
    assert!(prompt.contains("src/auth.ts"));
    assert!(prompt.contains("claim"));
}
```

- [ ] **Step 2: Run context test and verify failure**

Run:

```bash
cargo test -p stateful-server context_render_includes_reserved_and_blocking_state --test routes
```

Expected: FAIL because `/v1/context/render` returns empty prompt text and may not accept protocol bodies yet.

- [ ] **Step 3: Add public context item constructor**

In `crates/stateful-core/src/context.rs`, add:

```rust
impl ContextPackage {
    pub fn with_blocking_item(
        mut self,
        resource: impl Into<String>,
        summary: impl Into<String>,
        next_action: impl Into<String>,
    ) -> Self {
        self.status = ContextStatus::Blocked;
        self.items.push(ContextItem {
            severity: ContextSeverity::Block,
            resource: resource.into(),
            summary: summary.into(),
            next_action: Some(next_action.into()),
            evidence: None,
        });
        self
    }
}
```

- [ ] **Step 4: Add store context query**

In `crates/stateful-store/src/lib.rs`, add:

```rust
pub fn context_package_for_session(
    &self,
    session_id: impl AsRef<str>,
    workspace_id: impl AsRef<str>,
    resource: Option<&str>,
) -> StoreResult<stateful_core::ContextPackage> {
    let mut package = stateful_core::ContextPackage::empty();
    if let Some(resource) = resource {
        if let Some(reservation) = self.active_reservation(workspace_id.as_ref(), resource)? {
            if reservation.session_id == session_id.as_ref() {
                package = package.with_blocking_item(
                    reservation.relative_path,
                    "A reservation is ready for this session.",
                    "Reread the resource, then call state.intent.claim before writing.",
                );
            } else {
                package = package.with_warning(
                    reservation.relative_path,
                    "Another session owns the active reservation for this resource.",
                );
            }
        }
        if let Some(owner) = self.active_lease_owner(workspace_id.as_ref(), resource)? {
            if owner != session_id.as_ref() {
                package = package.with_warning(
                    resource.to_string(),
                    format!("Active lease is held by session {owner}"),
                );
            }
        }
    }
    Ok(package)
}
```

- [ ] **Step 5: Route context render through store query**

Change `context_render` to accept `ProtocolRequest<ContextRenderPayload>`:

```rust
#[derive(Debug, Deserialize)]
struct ContextRenderPayload {
    mode: Option<String>,
    resource: Option<String>,
}
```

In the handler:

```rust
let input = match validate_protocol(input) {
    Ok(input) => input,
    Err(error) => return error,
};
let mode = match input.payload.mode.as_deref() {
    Some("detailed") => RenderMode::Detailed,
    _ => RenderMode::Brief,
};
let package = match config.store.lock() {
    Ok(store) => match store.context_package_for_session(
        &input.session.session_id,
        &input.workspace.workspace_id,
        input.payload.resource.as_deref(),
    ) {
        Ok(package) => package,
        Err(message) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "status": "error", "message": message.to_string() })),
            )
        }
    },
    Err(_) => {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "status": "error", "message": "store lock poisoned" })),
        )
    }
};
```

- [ ] **Step 6: Run context tests**

Run:

```bash
cargo test -p stateful-server context_render_includes_reserved_and_blocking_state --test routes
cargo test -p stateful-core context
cargo test -p stateful-server --test routes
```

Expected: PASS.

- [ ] **Step 7: Commit context renderer**

```bash
git add crates/stateful-core/src/context.rs crates/stateful-store/src/lib.rs crates/stateful-server/src/lib.rs crates/stateful-server/tests/routes.rs
git commit -m "feat: render minimal coordination context"
```

---

### Task 9: Documentation Alignment

**Files:**
- Modify: `README.md`
- Modify: `docs/implementation-contract.md`
- Modify: `docs/architecture.md`
- Modify: `docs/current-state-coordination.md`
- Modify: `docs/state-model.md`

- [ ] **Step 1: Write failing docs grep checks**

Run:

```bash
rg -n "intent wait|retrying the write|MCP filesystem write/edit/delete/rename tools: enforce|Raw Bash test commands are not allowlisted" README.md docs
```

Expected before edits: matches exist for removed or conflicting wording.

- [ ] **Step 2: Update README quick start**

Replace the repo-local foreground quick start block with:

````markdown
## Quick Start

Install global Codex hooks and MCP configuration once:

```bash
stateful install --yes
```

Enable enforcement for the current repo:

```bash
stateful enable
```

The installed hooks lazily start the local state server when an enabled repo
uses stateful coordination. Foreground server start remains available for
debugging:

```bash
stateful server start --foreground
```
````

Remove the `stateful intent wait --timeout <seconds>` bullet from the CLI overview.

- [ ] **Step 3: Update reservation wording**

Use this wording in `README.md`, `docs/implementation-contract.md`,
`docs/current-state-coordination.md`, and `docs/state-model.md` wherever the old
implicit claim wording appears:

```markdown
Reservations are not active write authority. The reserved session rereads the
target and calls `state.intent.claim` or `/v1/intent/claim`. A successful claim
creates active write-authorizing intent and active leases. Retrying a write
before claiming is denied with a required next action that points to the claim
API.
```

- [ ] **Step 4: Update MCP filesystem/git future scope**

In `docs/architecture.md` and `docs/implementation-contract.md`, replace
structured MCP filesystem enforcement language with:

```markdown
The prototype enforces Codex `Edit`, `Write`, `apply_patch`, and the structured
`stateful commit` CLI. Structured MCP filesystem and git write tools are future
work because their argument shape is not part of the current stateful MCP
surface.
```

- [ ] **Step 5: Update validation profile deferral**

In `docs/architecture.md`, `docs/current-state-coordination.md`, and
`docs/state-model.md`, replace conflicting raw test language with:

```markdown
The current prototype keeps the existing validation runner and Bash classifier
behavior. Validation profile policy expansion, including `exclusive`
concurrency, `env`, `allowed_writes` semantics, and whether every test command
must run through `state.validation.run`, is deferred until the validation
profile product semantics are clarified.
```

- [ ] **Step 6: Verify docs grep checks**

Run:

```bash
rg -n "intent wait|retrying the write|MCP filesystem write/edit/delete/rename tools: enforce|Raw Bash test commands are not allowlisted" README.md docs
```

Expected: no matches for removed or conflicting wording.

- [ ] **Step 7: Commit docs alignment**

```bash
git add README.md docs/implementation-contract.md docs/architecture.md docs/current-state-coordination.md docs/state-model.md
git commit -m "docs: align v1 hardening prototype scope"
```

---

### Task 10: Final Verification

**Files:**
- Verify: full workspace

- [ ] **Step 1: Run formatting check**

Run:

```bash
cargo fmt --all --check
```

Expected: PASS.

- [ ] **Step 2: Run full tests**

Run:

```bash
cargo test --workspace
```

Expected: PASS.

- [ ] **Step 3: Run lint check**

Run:

```bash
cargo clippy --workspace --all-targets -- -D warnings
```

Expected: PASS.

- [ ] **Step 4: Run local doctor**

Run:

```bash
./target/debug/stateful doctor
```

Expected: JSON output includes global install fields such as `global_config_yml`,
`global_runtime_server_json`, `global_state_db`, and `repo_enabled`.

- [ ] **Step 5: Confirm removed CLI docs**

Run:

```bash
rg -n "stateful intent wait|intent wait --timeout" README.md docs
```

Expected: no output.

- [ ] **Step 6: Commit final verification note if any docs changed**

If verification required docs changes, commit them:

```bash
git add README.md docs/implementation-contract.md docs/architecture.md docs/current-state-coordination.md docs/state-model.md
git commit -m "docs: record v1 hardening verification"
```

If no files changed, do not create an empty commit.

---

## Self-Review Checklist

- Spec coverage:
  - Scheduling request/claim/cancel: Task 2 and Task 4.
  - Full v1 hardening: Task 1, Task 3, and Task 5.
  - Policy service single entry: Task 3 and Task 4.
  - Agent-agent conflict priority: Task 4.
  - Hook wrapper: Task 7.
  - Minimal context rendering: Task 8.
  - Lazy TTL/expiration: Task 6.
  - Docs alignment and removed `intent wait`: Task 9 and Task 10.
  - Validation profile deferral: Task 9.
  - MCP filesystem/git future scope: Task 9.
- Placeholder scan:
  - The plan contains no red-flag labels and no unspecified test command.
- Type consistency:
  - `ProtocolRequest<T>` is introduced before route handlers use it.
  - `PolicyService` is introduced before scheduling methods are added.
  - `protocol_body` is introduced before CLI and MCP body migration uses it.
  - `FixedClock` is introduced before lazy expiration tests use it.
