# Reservation Id Authorization Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the existing reservation id the write-authorization batch id for reservation, claim, and edit/write operations while keeping `session_id` for owner, audit, notification, heartbeat, and cleanup.

**Architecture:** The store already persists active declaration reservations in `reservations.reservation_id` using `Event::event_id`, and queued/claimable reservations use `wait_queue.wait_id`. Treat both as the public `reservation_id` at their boundaries. Claims persist the `reservation_id` they were acquired under; authorization checks reservation scope and active claims by `reservation_id`, not by `session_id` equality.

**Tech Stack:** Rust workspace (`stateful-store`, `stateful-server`, `stateful-cli`, `stateful-mcp`), Axum HTTP routes, SQLite via `rusqlite`, MCP tool schemas, installed OMP JavaScript template in `crates/stateful-cli/src/install.rs`.

## Global Constraints

- Do not add a new `batch_id` field or concept.
- Keep `session_id` in audit, notification, resume, lifecycle, heartbeat, finalization, and context-rendering records.
- Keep explicit `--session-id` / `session_id` accepted for manual CLI and protocol compatibility.
- Active Codex/OMP/MCP flows should not require agents to pass `session_id` for write authorization once a `reservation_id` is available.
- Preserve workspace identity semantics.
- Do not implement multi-resource atomic queueing beyond the existing planned model.
- Use TDD: write failing targeted tests first, implement the smallest code that passes, run only targeted crate tests.
- Use Stateful sandbox commands for verification, e.g. `stateful sandbox run --fs build --network enabled --write-dir reservation-id-auth --command 'cargo test ...'`.
- Stage and commit only files changed by the current task.

---

## File Structure

- Modify: `crates/stateful-store/src/lib.rs`
  - Add `reservation_id` to `claims` schema and migration support.
  - Add reservation-scoped claim acquisition and lookup helpers.
  - Keep session-scoped helpers as compatibility wrappers where they are still used for lifecycle cleanup.
- Modify: `crates/stateful-store/tests/event_store.rs`
  - Add store-level tests for claim persistence and same-reservation authorization helpers.
- Modify: `crates/stateful-server/src/policy_service.rs`
  - Add `reservation_id` to authorization inputs.
  - Load scope and claim authority by reservation id.
  - Keep `session_id` checks for reservation owner/notification only.
- Modify: `crates/stateful-server/src/lib.rs`
  - Return `reservation_id` from reservation declare/request/claim/authorize JSON.
  - Accept optional `reservation_id` in claim acquire and authorize payloads.
- Modify: `crates/stateful-server/tests/routes.rs`
  - Add route tests for same-reservation allow and different-reservation deny under the same session.
  - Update response assertions from `wait_id`-only to `reservation_id` where applicable.
- Modify: `crates/stateful-cli/src/runtime.rs`
  - Add optional `reservation_id` to reservation/authorize protocol body builders.
- Modify: `crates/stateful-cli/src/mcp.rs`
  - Add `reservation_id` argument handling for reservation claim, claim acquire, authorize-shaped flows, and mismatch checks.
- Modify: `crates/stateful-cli/src/hook.rs`
  - Thread reservation ids through OMP pre-tool authorize payloads when available.
  - Update denial strings from same-session to same-reservation language.
- Modify: `crates/stateful-cli/src/sandbox.rs`
  - Thread reservation ids through sandbox write authorization when the sandbox call has a reservation id.
- Modify: `crates/stateful-cli/src/install.rs`
  - Store/replay `reservation_id` in lazy edit/write operations and notification payloads.
- Modify: `crates/stateful-mcp/src/lib.rs`
  - Add `reservation_id` to tool schemas/descriptions for claim acquire/release and reservation claim where needed.
- Modify docs and agent-facing assets listed in Task 4.

---

### Task 1: Store Reservation-Scoped Claims

**Files:**
- Modify: `crates/stateful-store/src/lib.rs`
- Modify: `crates/stateful-store/tests/event_store.rs`

**Interfaces:**
- Consumes: existing `reservations.reservation_id`, existing queued `WaitRecord.wait_id`.
- Produces:
  - `claims.reservation_id TEXT` persisted for new claims.
  - `Store::reservation_id_for_active_scope(session_id: &str, workspace_id: &str, relative_path: &str, lease_is_directory: bool) -> StoreResult<Option<String>>`
  - `Store::acquire_claim_for_reservation(reservation_id: impl AsRef<str>, session_id: impl AsRef<str>, workspace_id: impl AsRef<str>, relative_path: impl AsRef<str>) -> StoreResult<()>`
  - `Store::active_exact_file_lease_by_reservation(workspace_id: impl AsRef<str>, relative_path: impl AsRef<str>, reservation_id: impl AsRef<str>) -> StoreResult<bool>`
  - `Store::active_claim_covers_path_by_reservation(workspace_id: impl AsRef<str>, relative_path: impl AsRef<str>, reservation_id: impl AsRef<str>) -> StoreResult<bool>`

- [ ] **Step 1: Add failing store test for reservation id on acquired claim**

Add this test near the existing claim acquisition tests in `crates/stateful-store/tests/event_store.rs`:

```rust
#[test]
fn acquired_claim_persists_reservation_id() {
    let store = Store::open_in_memory().expect("in-memory store should open");
    let reservation = Event::reservation_declared(
        "s1",
        "w1",
        "Acquire auth file.",
        ["src/auth.ts"],
    )
    .with_event_id("reservation-a");
    store.append(reservation).expect("reservation should append");

    store
        .acquire_claim_for_reservation("reservation-a", "s1", "w1", "src/auth.ts")
        .expect("claim should acquire under reservation");

    assert!(
        store
            .active_exact_file_lease_by_reservation("w1", "src/auth.ts", "reservation-a")
            .expect("reservation claim should load")
    );
    assert!(
        !store
            .active_exact_file_lease_by_reservation("w1", "src/auth.ts", "reservation-b")
            .expect("other reservation should not match")
    );
}
```

- [ ] **Step 2: Run the failing store test**

Run:

```bash
stateful sandbox run --fs build --network enabled --write-dir reservation-id-auth --command 'cargo test -p stateful-store acquired_claim_persists_reservation_id -- --exact'
```

Expected: FAIL because `acquire_claim_for_reservation` and `active_exact_file_lease_by_reservation` do not exist.

- [ ] **Step 3: Add store schema and migration support**

In `Store::ensure_schema`, add `reservation_id` to the `claims` table and a supporting index:

```rust
CREATE TABLE IF NOT EXISTS claims (
    claim_id TEXT PRIMARY KEY,
    reservation_id TEXT,
    session_id TEXT,
    workspace_id TEXT NOT NULL,
    repo_id TEXT,
    relative_path TEXT,
    absolute_path TEXT,
    purpose TEXT,
    action TEXT NOT NULL DEFAULT 'write_file',
    status TEXT NOT NULL,
    expires_at TEXT,
    observed_exists INTEGER,
    observed_content_hash TEXT
);

CREATE INDEX IF NOT EXISTS idx_claims_reservation_path_status
    ON claims(reservation_id, workspace_id, relative_path, status);
```

Extend `add_column_if_missing` support for `claims.reservation_id`:

```rust
|| (table == "claims"
    && matches!(
        column,
        "reservation_id" | "purpose" | "action" | "observed_exists" | "observed_content_hash"
    ))
```

Call it from schema migration setup with:

```rust
self.add_column_if_missing(
    "claims",
    "reservation_id",
    "ALTER TABLE claims ADD COLUMN reservation_id TEXT;",
)?;
```

- [ ] **Step 4: Add reservation id lookup for active declared reservations**

Add a helper near `active_reservation_purpose_for_lease`:

```rust
pub fn reservation_id_for_active_scope(
    &self,
    session_id: &str,
    workspace_id: &str,
    relative_path: &str,
    lease_is_directory: bool,
) -> StoreResult<Option<String>> {
    self.expire_stale()?;
    let relative_path = normalize_relative_path(relative_path);
    let mut statement = self.conn.prepare(
        "SELECT reservation_id, scopes_json
         FROM reservations
         WHERE session_id = ?1
           AND workspace_id = ?2
           AND status = 'active'
         ORDER BY rowid DESC",
    )?;
    let rows = statement.query_map(params![session_id, workspace_id], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    })?;

    for row in rows {
        let (reservation_id, scopes_json) = row?;
        let scopes: Vec<ReservationScope> = serde_json::from_str(&scopes_json).map_err(|err| {
            rusqlite::Error::FromSqlConversionFailure(
                0,
                rusqlite::types::Type::Text,
                Box::new(err),
            )
        })?;
        let matches_scope = scopes.iter().any(|scope| match scope {
            ReservationScope::File(path) => !lease_is_directory && path == &relative_path,
            ReservationScope::Directory(path) => lease_is_directory && path.trim_end_matches('/') == relative_path,
        });
        if matches_scope {
            return Ok(Some(reservation_id));
        }
    }

    Ok(None)
}
```

- [ ] **Step 5: Add reservation-scoped claim acquisition**

Keep the existing `acquire_claim` method for compatibility. Add this wrapper and update the private insert path to accept `reservation_id: Option<&str>`:

```rust
pub fn acquire_claim_for_reservation(
    &self,
    reservation_id: impl AsRef<str>,
    session_id: impl AsRef<str>,
    workspace_id: impl AsRef<str>,
    relative_path: impl AsRef<str>,
) -> StoreResult<()> {
    self.acquire_claim_with_reservation_observation_and_event(
        Some(reservation_id.as_ref()),
        session_id,
        workspace_id,
        relative_path,
        None,
    )
}
```

Change the claim insert SQL to write `reservation_id`:

```rust
"INSERT INTO claims (
    claim_id,
    reservation_id,
    session_id,
    workspace_id,
    repo_id,
    relative_path,
    absolute_path,
    purpose,
    action,
    status,
    expires_at,
    observed_exists,
    observed_content_hash
) VALUES (?1, ?2, ?3, ?4, NULL, ?5, NULL, ?6, ?7, 'active', ?8, ?9, ?10)"
```

Use these params:

```rust
params![
    Uuid::new_v4().to_string(),
    reservation_id,
    session_id,
    workspace_id,
    relative_path,
    purpose,
    lease_action,
    expires_at,
    observed_exists,
    observed_content_hash,
]
```

For the old session-scoped `acquire_claim`, derive the latest active reservation id with `reservation_id_for_active_scope(...)` and pass it into the same insert path. This keeps compatibility while making all new claims reservation-scoped.

- [ ] **Step 6: Add reservation-scoped lookup helpers**

Add helpers near existing session-scoped lease checks:

```rust
pub fn active_exact_file_lease_by_reservation(
    &self,
    workspace_id: impl AsRef<str>,
    relative_path: impl AsRef<str>,
    reservation_id: impl AsRef<str>,
) -> StoreResult<bool> {
    self.expire_stale()?;
    let relative_path = normalize_relative_path(relative_path.as_ref());
    self.conn
        .query_row(
            "SELECT EXISTS(
                SELECT 1 FROM claims
                WHERE workspace_id = ?1
                   AND reservation_id = ?2
                   AND status = 'active'
                   AND action = 'write_file'
                   AND relative_path = ?3
            )",
            params![workspace_id.as_ref(), reservation_id.as_ref(), relative_path],
            |row| row.get::<_, bool>(0),
        )
        .map_err(StoreError::from)
}

pub fn active_claim_covers_path_by_reservation(
    &self,
    workspace_id: impl AsRef<str>,
    relative_path: impl AsRef<str>,
    reservation_id: impl AsRef<str>,
) -> StoreResult<bool> {
    self.expire_stale()?;
    let relative_path = normalize_relative_path(relative_path.as_ref());
    self.conn
        .query_row(
            "SELECT EXISTS(
                SELECT 1 FROM claims
                WHERE workspace_id = ?1
                   AND reservation_id = ?2
                   AND status = 'active'
                   AND (
                       (action = 'write_file' AND relative_path = ?3)
                       OR (action = 'write_directory'
                           AND substr(?3, 1, length(relative_path) + 1) = relative_path || '/')
                   )
            )",
            params![workspace_id.as_ref(), reservation_id.as_ref(), relative_path],
            |row| row.get::<_, bool>(0),
        )
        .map_err(StoreError::from)
}
```

- [ ] **Step 7: Run the store test until it passes**

Run:

```bash
stateful sandbox run --fs build --network enabled --write-dir reservation-id-auth --command 'cargo test -p stateful-store acquired_claim_persists_reservation_id -- --exact'
```

Expected: PASS.

- [ ] **Step 8: Commit Task 1**

```bash
git add crates/stateful-store/src/lib.rs crates/stateful-store/tests/event_store.rs
git commit -m "feat: store reservation ids on claims"
```

---

### Task 2: Server Authorization By Reservation Id

**Files:**
- Modify: `crates/stateful-server/src/lib.rs`
- Modify: `crates/stateful-server/src/policy_service.rs`
- Modify: `crates/stateful-server/tests/routes.rs`
- Modify: `crates/stateful-store/src/lib.rs` if Task 1 did not add `policy_state_for_reservation`.

**Interfaces:**
- Consumes: Task 1 reservation-scoped claim helpers.
- Produces:
  - `AuthorizePayload { reservation_id: Option<String>, ... }`
  - `LeaseAcquireRequest { reservation_id: Option<String>, ... }`
  - `AuthorizeWriteInput { reservation_id: Option<String>, ... }`
  - `/v1/reservation/declare` response includes `reservation_id`.
  - `/v1/authorize` accepts optional `reservation_id` and enforces same-reservation claims when present.

- [ ] **Step 1: Add failing route test for different reservation denial under same session**

In `crates/stateful-server/tests/routes.rs`, add a test near the active-claim authorization tests:

```rust
#[tokio::test]
async fn same_session_different_reservation_claim_does_not_authorize_write() {
    let app = build_router(ServerConfig::new("secret-token"));

    let declare_a = app
        .clone()
        .oneshot(protocol_request(
            "/v1/reservation/declare",
            "s1",
            "w1",
            serde_json::json!({
                "purpose": "Reservation A.",
                "files_planned": ["src/auth.ts"]
            }),
        ))
        .await
        .expect("reservation A should complete");
    assert_eq!(declare_a.status(), StatusCode::OK);
    let reservation_a = response_json(declare_a, 2048).await["reservation_id"]
        .as_str()
        .expect("reservation A id should be returned")
        .to_string();

    let declare_b = app
        .clone()
        .oneshot(protocol_request(
            "/v1/reservation/declare",
            "s1",
            "w1",
            serde_json::json!({
                "purpose": "Reservation B.",
                "files_planned": ["src/auth.ts"]
            }),
        ))
        .await
        .expect("reservation B should complete");
    assert_eq!(declare_b.status(), StatusCode::OK);
    let reservation_b = response_json(declare_b, 2048).await["reservation_id"]
        .as_str()
        .expect("reservation B id should be returned")
        .to_string();

    let claim = app
        .clone()
        .oneshot(json_request(
            "/v1/claim/acquire",
            serde_json::json!({
                "session_id": "s1",
                "workspace_id": "w1",
                "reservation_id": reservation_a,
                "path": "src/auth.ts"
            }),
        ))
        .await
        .expect("claim should complete");
    assert_eq!(claim.status(), StatusCode::OK);

    let mut body = protocol_body(
        "s1",
        "w1",
        serde_json::json!({
            "action": "write_file",
            "path": "src/auth.ts",
            "reservation_id": reservation_b
        }),
    );
    body["source"]["kind"] = serde_json::json!("hook");
    body["source"]["event"] = serde_json::json!("pre_tool_use");
    body["source"]["tool_name"] = serde_json::json!("apply_patch");

    let authorize = app
        .oneshot(json_request("/v1/authorize", body))
        .await
        .expect("authorize should complete");
    assert_eq!(authorize.status(), StatusCode::OK);
    let json = response_json(authorize, 2048).await;
    assert_eq!(json["decision"], "deny");
    assert_eq!(json["reason_code"], "missing_claim");
    assert!(json["required_next_action"]
        .as_str()
        .unwrap_or_default()
        .contains("same-reservation"));
}
```

- [ ] **Step 2: Add failing route test for same reservation allow**

Add:

```rust
#[tokio::test]
async fn same_reservation_claim_authorizes_write() {
    let app = build_router(ServerConfig::new("secret-token"));

    let declare = app
        .clone()
        .oneshot(protocol_request(
            "/v1/reservation/declare",
            "s1",
            "w1",
            serde_json::json!({
                "purpose": "Update auth file.",
                "files_planned": ["src/auth.ts"]
            }),
        ))
        .await
        .expect("reservation should complete");
    assert_eq!(declare.status(), StatusCode::OK);
    let reservation_id = response_json(declare, 2048).await["reservation_id"]
        .as_str()
        .expect("reservation id should be returned")
        .to_string();

    let claim = app
        .clone()
        .oneshot(json_request(
            "/v1/claim/acquire",
            serde_json::json!({
                "session_id": "s1",
                "workspace_id": "w1",
                "reservation_id": reservation_id,
                "path": "src/auth.ts"
            }),
        ))
        .await
        .expect("claim should complete");
    assert_eq!(claim.status(), StatusCode::OK);

    let mut body = protocol_body(
        "s1",
        "w1",
        serde_json::json!({
            "action": "write_file",
            "path": "src/auth.ts",
            "reservation_id": reservation_id
        }),
    );
    body["source"]["kind"] = serde_json::json!("hook");
    body["source"]["event"] = serde_json::json!("pre_tool_use");
    body["source"]["tool_name"] = serde_json::json!("apply_patch");

    let authorize = app
        .oneshot(json_request("/v1/authorize", body))
        .await
        .expect("authorize should complete");
    assert_eq!(authorize.status(), StatusCode::OK);
    let json = response_json(authorize, 2048).await;
    assert_eq!(json["decision"], "allow");
    assert_eq!(json["reason_code"], "authorized");
}
```

- [ ] **Step 3: Run failing route tests**

Run:

```bash
stateful sandbox run --fs build --network enabled --write-dir reservation-id-auth --command 'cargo test -p stateful-server same_session_different_reservation_claim_does_not_authorize_write same_reservation_claim_authorizes_write'
```

Expected: FAIL because `/v1/reservation/declare` does not return `reservation_id`, claim acquire ignores it, and authorize ignores it.

- [ ] **Step 4: Return reservation id from declaration route**

In `reservation_declare`, create the event before appending and return its `event_id`:

```rust
let event = with_request_identity(
    Event::reservation_declared(
        envelope.request.session.session_id,
        envelope.request.workspace.workspace_id,
        purpose,
        files_planned,
    ),
    identity,
);
let reservation_id = event.event_id.clone();
match config.store.lock().map_err(|_| "store lock poisoned".to_string()) {
    Ok(store) => match store.append(event) {
        Ok(()) => (StatusCode::OK, Json(json!({
            "status": "ok",
            "reservation_id": reservation_id
        }))),
        Err(error) => status_response(Err(error.to_string())),
    },
    Err(message) => status_response(Err(message)),
}
```

Keep the existing protocol envelope requirement.

- [ ] **Step 5: Accept reservation id in claim acquire**

Add to `LeaseAcquireRequest`:

```rust
#[serde(default)]
reservation_id: Option<String>,
```

When acquiring a single path, branch:

```rust
match input.reservation_id.as_deref() {
    Some(reservation_id) => store.acquire_claim_with_reservation_observation_and_event(
        Some(reservation_id),
        &session_id,
        &workspace_id,
        &path,
        observation,
    ),
    None => store.acquire_claim_with_observation_and_event(
        &session_id,
        &workspace_id,
        &path,
        observation,
    ),
}
```

For batch paths, use a reservation-aware batch helper from Task 1 or call the single-path helper inside the same transaction.

- [ ] **Step 6: Accept reservation id in authorize payload and input**

Add to `AuthorizePayload`:

```rust
#[serde(default)]
reservation_id: Option<String>,
```

Add to `AuthorizeWriteInput`:

```rust
pub reservation_id: Option<String>,
```

Set it in `authorize`:

```rust
reservation_id: payload.reservation_id,
```

- [ ] **Step 7: Build policy state by reservation id when supplied**

Add store helper:

```rust
pub fn policy_state_for_reservation(
    &self,
    reservation_id: &str,
    workspace_id: &str,
) -> StoreResult<PolicyState> {
    self.expire_stale()?;
    let phase = self
        .reservation_owner(reservation_id, workspace_id)?
        .and_then(|owner| self.active_session_phase(&owner.session_id, workspace_id).transpose())
        .transpose()?;
    let scopes = self.active_reservation_scope_rows_by_id(reservation_id, workspace_id)?;
    if scopes.is_empty() {
        return Ok(PolicyState::default());
    }
    let mut state = PolicyState::default().with_active_reservation_scopes(scopes);
    if let Some(phase) = phase {
        state = state.with_activity_phase(phase);
    }
    Ok(state)
}
```

Use this shape in `PolicyService::authorize_write`:

```rust
let policy_state = if let (Some(workspace_id), Some(reservation_id)) =
    (&input.workspace_id, input.reservation_id.as_deref())
{
    self.store
        .policy_state_for_reservation(reservation_id, workspace_id)
        .map_err(|error| error.to_string())?
} else if let Some(workspace_id) = &input.workspace_id {
    self.store
        .policy_state_for_session(&input.session_id, workspace_id)
        .map_err(|error| error.to_string())?
} else {
    Default::default()
};
```

If adding `reservation_owner` is too much, skip phase lookup for reservation-scoped state in the first pass and keep phase enforcement in the session fallback. Add a `ponytail:` comment naming the ceiling:

```rust
// ponytail: reservation-scoped authorization reuses scope checks; phase remains session-scoped until reservation owner lookup is needed here.
```

- [ ] **Step 8: Check claims by reservation id when supplied**

In `PolicyService::has_required_lease`, route file writes to reservation helpers when `input.reservation_id` is present:

```rust
if let Some(reservation_id) = input.reservation_id.as_deref() {
    return match input.action.as_str() {
        "write_file" | "delete_file" => self
            .store
            .active_exact_file_lease_by_reservation(workspace_id, &input.path, reservation_id)
            .map_err(|error| error.to_string()),
        "write_directory" => self
            .store
            .active_claim_covers_directory_by_reservation(workspace_id, &input.path, reservation_id)
            .map_err(|error| error.to_string()),
        "rename_file" | "move_file" => {
            let Some((old_path, new_path)) = self.rename_or_move_paths(input) else {
                return Ok(false);
            };
            let old_lease = self
                .store
                .active_exact_file_lease_by_reservation(workspace_id, old_path, reservation_id)
                .map_err(|error| error.to_string())?;
            if !old_lease {
                return Ok(false);
            }
            self.store
                .active_exact_file_lease_by_reservation(workspace_id, new_path, reservation_id)
                .map_err(|error| error.to_string())
        }
        _ => Ok(false),
    };
}
```

Update denial text from same-session to same-reservation:

```rust
"Acquire exact same-reservation file claims for file actions, or exact same-reservation directory claims for write-directory actions."
```

- [ ] **Step 9: Return reservation_id in JSON helpers**

For wait-queue reservations, alias `wait_id` as `reservation_id` in output:

```rust
"reservation_id": reservation.wait_id,
"wait_id": reservation.wait_id,
```

Apply to `authorization_json`, `claim_intent_json`, `request_intent_json` via `reservation_json`, and notification payloads where reservation records are emitted.

- [ ] **Step 10: Run targeted server tests**

Run:

```bash
stateful sandbox run --fs build --network enabled --write-dir reservation-id-auth --command 'cargo test -p stateful-server same_session_different_reservation_claim_does_not_authorize_write same_reservation_claim_authorizes_write'
```

Expected: PASS.

- [ ] **Step 11: Commit Task 2**

```bash
git add crates/stateful-store/src/lib.rs crates/stateful-server/src/lib.rs crates/stateful-server/src/policy_service.rs crates/stateful-server/tests/routes.rs
git commit -m "feat: authorize writes by reservation id"
```

---

### Task 3: CLI, MCP, Hook, Sandbox, And Lazy Resume Threading

**Files:**
- Modify: `crates/stateful-cli/src/runtime.rs`
- Modify: `crates/stateful-cli/src/mcp.rs`
- Modify: `crates/stateful-cli/src/lib.rs`
- Modify: `crates/stateful-cli/src/hook.rs`
- Modify: `crates/stateful-cli/src/sandbox.rs`
- Modify: `crates/stateful-cli/src/install.rs`
- Modify: `crates/stateful-mcp/src/lib.rs`
- Modify: `crates/stateful-cli/tests/mcp.rs`
- Modify: `crates/stateful-cli/tests/hook.rs`
- Modify: `crates/stateful-cli/tests/cli.rs`
- Modify: `crates/stateful-mcp/tests/tools.rs`

**Interfaces:**
- Consumes: server accepts and returns `reservation_id`.
- Produces:
  - CLI accepts `--reservation-id` for claim and cancel-like write-authority operations where useful.
  - MCP tool schemas accept `reservation_id` on `state_claim_acquire`, `state_reservation_claim`, and replay-related paths.
  - OMP lazy operations store and replay `reservation_id`.

- [ ] **Step 1: Add failing MCP test for reservation id in claim acquire body**

In `crates/stateful-cli/tests/mcp.rs`, add a test near existing reservation MCP tests:

```rust
#[test]
fn mcp_claim_acquire_passes_reservation_id_without_requiring_session_argument() {
    let temp_root = temp_root("stateful-mcp-claim-reservation-id");
    let paths = GlobalPaths::new(temp_root.join("home"));
    let repo_root = temp_root.join("repo");
    fs::create_dir_all(&repo_root).expect("repo root should be creatable");
    enable_test_repo(&paths, &repo_root);
    write_current_session_file(&repo_root, &CurrentSession::new("s-current", "w1"))
        .expect("current session should write");
    let (runtime, rx) = spawn_fake_stateful_server(r#"{"status":"ok"}"#);
    write_global_runtime_file(&paths, &runtime).expect("global runtime file should write");

    let response = run_mcp_jsonrpc_in_repo(
        &repo_root,
        &paths,
        r#"{
          "jsonrpc":"2.0",
          "id":51,
          "method":"tools/call",
          "params":{
            "name":"state_claim_acquire",
            "arguments":{"reservation_id":"reservation-a","paths":["src/auth.ts"]}
          }
        }"#,
    );

    let request = rx.recv_timeout(Duration::from_secs(1)).expect("claim request should arrive");
    let body = request_json_body(&request);
    assert_eq!(body["session_id"], "s-current");
    assert_eq!(body["workspace_id"], "w1");
    assert_eq!(body["reservation_id"], "reservation-a");
    assert_eq!(body["paths"], serde_json::json!(["src/auth.ts"]));
    let json: serde_json::Value = serde_json::from_str(&response).expect("response should be json");
    assert_eq!(json["result"]["isError"], false);

    fs::remove_dir_all(&temp_root).expect("temp root should be removable");
}
```

- [ ] **Step 2: Run failing MCP test**

Run:

```bash
stateful sandbox run --fs build --network enabled --write-dir reservation-id-auth --command 'cargo test -p stateful-cli mcp_claim_acquire_passes_reservation_id_without_requiring_session_argument -- --exact'
```

Expected: FAIL if the schema rejects `reservation_id` or the body drops it.

- [ ] **Step 3: Add reservation id to runtime protocol args**

In `crates/stateful-cli/src/runtime.rs`, extend args where payloads need it:

```rust
pub struct ReservationClaimArgs {
    pub session_id: String,
    pub workspace_id: String,
    pub wait_id: String,
    pub reservation_id: Option<String>,
    pub identity: Option<RepoIdentity>,
}

pub struct ReservationRequestArgs {
    pub session_id: String,
    pub workspace_id: String,
    pub request_id: String,
    pub reservation_id: Option<String>,
    pub action: String,
    pub path: String,
    pub purpose: String,
    pub identity: Option<RepoIdentity>,
}
```

Add `reservation_id` to payload JSON only when present:

```rust
let mut payload = serde_json::json!({ "wait_id": wait_id });
if let Some(reservation_id) = reservation_id {
    payload["reservation_id"] = serde_json::json!(reservation_id);
}
```

Use the same pattern for request/authorize-shaped payload builders.

- [ ] **Step 4: Add MCP schema fields**

In `crates/stateful-mcp/src/lib.rs`, add optional `reservation_id` property to `state_claim_acquire` and `state_reservation_claim` schemas:

```rust
"reservation_id": {
  "type": "string",
  "description": "Reservation id that owns the claim batch. When supplied, write authorization requires claims under this same reservation."
}
```

Do not add `reservation_id` to lifecycle tools like `state_session_heartbeat`.

- [ ] **Step 5: Preserve reservation id through MCP argument enrichment**

In `crates/stateful-cli/src/mcp.rs`, keep `reservation_id` untouched while injecting current session owner fields:

```rust
if !object.contains_key("session_id") {
    object.insert("session_id".to_string(), Value::String(current_session.session_id.clone()));
}
if !object.contains_key("workspace_id") {
    object.insert("workspace_id".to_string(), Value::String(current_session.workspace_id.clone()));
}
```

Do not reject `reservation_id` in `reject_argument_session_mismatch`; mismatch checks still apply only to explicit `session_id` and `workspace_id`.

- [ ] **Step 6: Add CLI argument parsing for reservation id where the user needs it**

In `crates/stateful-cli/src/lib.rs`, add `--reservation-id` to `reservation claim` and `reservation request`:

```rust
Claim {
    #[arg(long)]
    session_id: Option<String>,
    #[arg(long)]
    workspace_id: Option<String>,
    #[arg(long)]
    reservation_id: Option<String>,
    #[arg(long)]
    wait_id: String,
},
```

Add a parser test in `crates/stateful-cli/tests/cli.rs`:

```rust
#[test]
fn reservation_claim_command_parses_reservation_id() {
    let cli = Cli::try_parse_from([
        "stateful",
        "reservation",
        "claim",
        "--reservation-id",
        "reservation-a",
        "--wait-id",
        "wait-1",
    ])
    .expect("claim command should parse");

    assert!(matches!(cli.command, Command::Reservation(ReservationCommand::Claim {
        ref reservation_id,
        ref wait_id,
        ..
    }) if reservation_id.as_deref() == Some("reservation-a") && wait_id == "wait-1"));
}
```

- [ ] **Step 7: Thread reservation id through hooks and sandbox payloads**

Add optional fields without changing session lifecycle identity:

```rust
#[derive(Debug, Deserialize)]
struct OmpPreToolUseInput {
    session_id: String,
    #[serde(default)]
    reservation_id: Option<String>,
    // existing fields...
}
```

In `authorize_omp_targets`, add to payload:

```rust
if let Some(reservation_id) = &input.reservation_id {
    payload["reservation_id"] = serde_json::json!(reservation_id);
}
```

In `SandboxAuthorizeContext`, add:

```rust
pub(crate) reservation_id: Option<String>,
```

In `authorize_sandbox_write`, add to payload when present:

```rust
if let Some(reservation_id) = &context.reservation_id {
    payload["reservation_id"] = serde_json::json!(reservation_id);
}
```

- [ ] **Step 8: Store and replay reservation id in OMP lazy operations**

In `crates/stateful-cli/src/install.rs`, when constructing lazy operation records, store both ids:

```javascript
const record = {
  operation_id: operationId,
  session_id: decision.session_id || input.session_id,
  reservation_id: decision?.reservation?.reservation_id || decision?.wait?.reservation_id || decision?.reservation?.wait_id || "",
  workspace_id: input.workspace_id,
  tool_name: input.tool_name,
  payload: input,
};
```

When replaying:

```javascript
session_id: operation.session_id,
reservation_id: operation.reservation_id || undefined,
workspace_id: operation.workspace_id,
```

Update user-facing notification text:

```javascript
lines.push("reservation_id: " + (payload.reservation_id || waitId));
lines.push("Next: reread the target, then call state_reservation_claim with this reservation_id before retrying the write.");
```

- [ ] **Step 9: Run targeted CLI/MCP tests**

Run:

```bash
stateful sandbox run --fs build --network enabled --write-dir reservation-id-auth --command 'cargo test -p stateful-cli mcp_claim_acquire_passes_reservation_id_without_requiring_session_argument reservation_claim_command_parses_reservation_id -- --exact'
```

Expected: PASS.

- [ ] **Step 10: Commit Task 3**

```bash
git add crates/stateful-cli/src/runtime.rs crates/stateful-cli/src/mcp.rs crates/stateful-cli/src/lib.rs crates/stateful-cli/src/hook.rs crates/stateful-cli/src/sandbox.rs crates/stateful-cli/src/install.rs crates/stateful-mcp/src/lib.rs crates/stateful-cli/tests/mcp.rs crates/stateful-cli/tests/hook.rs crates/stateful-cli/tests/cli.rs crates/stateful-mcp/tests/tools.rs
git commit -m "feat: thread reservation ids through clients"
```

---

### Task 4: Documentation, Agent Guidance, And Final Verification

**Files:**
- Modify: `docs/state-model.md`
- Modify: `docs/current-state-coordination.md`
- Modify: `docs/architecture.md`
- Modify: `docs/usage-reference.md`
- Modify: `docs/core-concept.md`
- Modify: `docs/implementation-contract.md`
- Modify: `docs/concurrency-control-spec.md`
- Modify: `docs/v1-hardening-scope-decisions.md`
- Modify: `docs/adr/0001-state-first-not-memory-first.md`
- Modify: `README.md`
- Modify: `crates/stateful-cli/assets/stateful-command-policy/SKILL.md`
- Modify: `crates/stateful-cli/assets/stateful-command-policy/denial-recovery.md`
- Modify: `crates/stateful-cli/assets/stateful-command-policy/sandbox-tools.md`
- Modify: `crates/stateful-cli/assets/stateful-command-policy/subagent-write-recovery.md`
- Modify: `crates/stateful-cli/assets/omp-stateful-required-rule.md`
- Modify: `crates/stateful-server/src/policy_service.rs`
- Modify: `crates/stateful-server/src/lib.rs`
- Modify: `crates/stateful-store/src/lib.rs`
- Modify test names/assertions in `crates/stateful-store/tests/event_store.rs`, `crates/stateful-server/tests/routes.rs`, and `crates/stateful-mcp/tests/tools.rs`.

**Interfaces:**
- Consumes: Tasks 1-3 behavior and JSON fields.
- Produces: Consistent agent/user language: “same-reservation claim” for write authorization, “session_id” for owner/audit/notification.

- [ ] **Step 1: Replace write-authorization language only**

Search for same-session authorization text with the built-in grep tool, not shell grep:

```text
pattern: same-session|same session|session claim|session_id
paths: docs README.md crates/stateful-cli/assets crates/stateful-server/src crates/stateful-store/src crates/stateful-mcp/src crates/stateful-server/tests crates/stateful-store/tests crates/stateful-mcp/tests
```

Keep references where `session_id` means lifecycle/session resolution. Replace only write-authorization language:

```text
same-session claim -> same-reservation claim
same-session file claim -> exact same-reservation file claim
Do not change session_id -> Reuse the reservation_id returned by the reservation flow
```

- [ ] **Step 2: Update state model docs with the final hierarchy**

In `docs/state-model.md`, make the hierarchy:

```text
session
  goal
    turn
      reservation_id
        file/directory scopes
          resource claims
            write actions
```

Add this rule:

```markdown
Write authorization is reservation-scoped: a write is allowed only when its `reservation_id` has active scope for the target and an active claim under the same `reservation_id`. `session_id` remains the owner and notification identity for that reservation.
```

- [ ] **Step 3: Update usage examples**

In `docs/usage-reference.md` and `README.md`, show the new flow:

```bash
stateful reservation declare --purpose "Update README content requested by the user." README.md
# returns reservation_id
stateful claim acquire --reservation-id <reservation_id> README.md
# edit/write carries the same reservation_id through the active integration
```

If the current CLI subcommand remains `reservation claim --wait-id`, explain that queued reservations expose the same id as both `wait_id` and `reservation_id` during compatibility.

- [ ] **Step 4: Update denial messages and tests**

Change policy denial strings in `crates/stateful-server/src/policy_service.rs` from same-session to same-reservation. Example final strings:

```rust
"Write target is inside active reservation scope, but no active same-reservation claim matches it."
"Acquire exact same-reservation file claims for file actions, or exact same-reservation directory claims for write-directory actions."
```

Update assertions in route tests that checked `same-session file claims` to check `same-reservation file claims`.

- [ ] **Step 5: Run targeted documentation-sensitive tests**

Run:

```bash
stateful sandbox run --fs build --network enabled --write-dir reservation-id-auth --command 'cargo test -p stateful-server missing_claim_cannot_be_bypassed_by_changing_session_id -- --exact'
```

Expected: PASS after updated assertion text.

Run MCP schema tests if changed:

```bash
stateful sandbox run --fs build --network enabled --write-dir reservation-id-auth --command 'cargo test -p stateful-mcp --test tools'
```

Expected: PASS.

- [ ] **Step 6: Run final targeted verification across changed crates**

Run:

```bash
stateful sandbox run --fs build --network enabled --write-dir reservation-id-auth --command 'cargo test -p stateful-store acquired_claim_persists_reservation_id -- --exact'
```

Run:

```bash
stateful sandbox run --fs build --network enabled --write-dir reservation-id-auth --command 'cargo test -p stateful-server same_session_different_reservation_claim_does_not_authorize_write same_reservation_claim_authorizes_write'
```

Run:

```bash
stateful sandbox run --fs build --network enabled --write-dir reservation-id-auth --command 'cargo test -p stateful-cli mcp_claim_acquire_passes_reservation_id_without_requiring_session_argument reservation_claim_command_parses_reservation_id -- --exact'
```

Expected: all PASS.

- [ ] **Step 7: Commit Task 4**

```bash
git add docs README.md crates/stateful-cli/assets crates/stateful-server/src/policy_service.rs crates/stateful-server/src/lib.rs crates/stateful-store/src/lib.rs crates/stateful-store/tests/event_store.rs crates/stateful-server/tests/routes.rs crates/stateful-mcp/src/lib.rs crates/stateful-mcp/tests/tools.rs
git commit -m "docs: describe same-reservation write authority"
```

---

## Self-Review Notes

- Spec coverage: covered stable reservation id return, claim records with reservation id, authorization by reservation id, session id retained for lifecycle, MCP/CLI helpers, lazy resume replay, and doc terminology updates.
- Placeholder scan: no TBD/TODO/fill-in-later steps. Each task has concrete tests, code shapes, commands, and commit commands.
- Type consistency: plan uses `reservation_id: Option<String>` at protocol/client boundaries and `reservation_id` string at store lookup boundaries. Wait-queue records keep `wait_id`; public JSON aliases it as `reservation_id` for compatibility.
- Ponytail decision: no new `batch_id`; use existing `reservations.reservation_id` and `wait_queue.wait_id` as reservation ids at their respective boundaries.
