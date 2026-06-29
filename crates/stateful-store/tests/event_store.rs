use rusqlite::Connection;
use serde_json::json;
use stateful_core::{AuthorizationInput, CurrentEvidenceKind, CurrentItemKind, DecisionKind};
use stateful_store::{Event, OutboxEntry, ReservationRequestInput, Store, StoreError};
use std::fs;
use std::sync::mpsc;
use std::thread;
use std::time::{Duration as StdDuration, SystemTime, UNIX_EPOCH};
use time::{Duration as TimeDuration, OffsetDateTime};

fn acquire_test_lease(store: &Store, session_id: &str, workspace_id: &str, path: &str) {
    let has_matching_reservation = store
        .live_current_state(Some(path))
        .expect("live current state should load")
        .items
        .iter()
        .any(|item| {
            item.kind == CurrentItemKind::Reservation
                && item.session_id.as_deref() == Some(session_id)
        });
    if !has_matching_reservation {
        store
            .append(Event::reservation_declared(
                session_id,
                workspace_id,
                format!("Acquire test claim for {path}."),
                [path],
            ))
            .expect("claim reservation should append");
    }
    store
        .acquire_claim(session_id, workspace_id, path)
        .expect("claim should acquire");
}

fn query_ids(conn: &Connection, sql: &str) -> Vec<String> {
    let mut statement = conn.prepare(sql).expect("query should prepare");
    let mut ids = statement
        .query_map([], |row| row.get::<_, String>(0))
        .expect("query should execute")
        .collect::<Result<Vec<_>, _>>()
        .expect("ids should load");
    ids.sort();
    ids
}

fn test_timestamp(timestamp: OffsetDateTime) -> String {
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z",
        timestamp.year(),
        u8::from(timestamp.month()),
        timestamp.day(),
        timestamp.hour(),
        timestamp.minute(),
        timestamp.second()
    )
}

#[test]
fn append_event_materializes_session_in_same_transaction() {
    let store = Store::open_in_memory().expect("in-memory store should open");

    store
        .append(Event::session_registered("s1", "w1"))
        .expect("session event should append");

    let session = store
        .session("s1")
        .expect("session lookup should succeed")
        .expect("session should be materialized");
    assert_eq!(session.session_id, "s1");
    assert_eq!(session.workspace_id, "w1");
}

#[test]
fn repeated_event_id_is_idempotent() {
    let store = Store::open_in_memory().expect("in-memory store should open");
    let event = Event::session_registered("s1", "w1").with_event_id("event-1");

    store
        .append(event.clone())
        .expect("first event append should succeed");
    store
        .append(event)
        .expect("duplicate event append should be idempotent");

    assert_eq!(store.event_count().expect("event count should load"), 1);
}

#[test]
fn outbox_entries_are_idempotent_by_outbox_id() {
    let store = Store::open_in_memory().expect("in-memory store should open");
    let entry = OutboxEntry::new("outbox-1", "s1", 1);

    store
        .append_outbox(entry.clone())
        .expect("first outbox append should succeed");
    store
        .append_outbox(entry)
        .expect("duplicate outbox append should be idempotent");

    assert_eq!(store.outbox_count().expect("outbox count should load"), 1);
}

#[test]
fn outbox_entry_persists_full_sync_evidence() {
    let store = Store::open_in_memory().expect("in-memory store should open");
    let entry = OutboxEntry::synced("outbox-1", "s1", 1)
        .with_workspace_id("w1")
        .with_event_type("HeartbeatObserved")
        .with_payload(json!({"error":"server unavailable"}));

    store
        .append_outbox(entry)
        .expect("outbox append should succeed");

    let stored = store
        .outbox_entry("outbox-1")
        .expect("outbox entry lookup should succeed")
        .expect("outbox entry should exist");
    assert_eq!(stored.outbox_id, "outbox-1");
    assert_eq!(stored.session_id, "s1");
    assert_eq!(stored.workspace_id, "w1");
    assert_eq!(stored.sequence, 1);
    assert_eq!(stored.event_type, "HeartbeatObserved");
    assert_eq!(stored.payload, json!({"error":"server unavailable"}));
    assert_eq!(stored.sync_status, stateful_store::SyncStatus::Synced);
}

#[test]
fn reservation_declared_materializes_active_policy_state() {
    let store = Store::open_in_memory().expect("in-memory store should open");

    store
        .append(Event::reservation_declared(
            "s1",
            "w1",
            "Fix auth validation behavior.",
            ["src/auth.ts"],
        ))
        .expect("reservation event should append");

    let state = store
        .policy_state_for_session("s1", "w1")
        .expect("policy state should load");
    let decision =
        stateful_core::authorize_action(&state, AuthorizationInput::write_file("src/auth.ts"));

    assert_eq!(decision.decision, DecisionKind::Allow);
}

#[test]
fn reservation_declared_rejects_empty_or_normalized_empty_scopes() {
    for files in [Vec::<&str>::new(), vec!["./"], vec!["../"], vec!["/"]] {
        let store = Store::open_in_memory().expect("in-memory store should open");

        let error = store
            .append(Event::reservation_declared(
                "s1",
                "w1",
                "Reject empty reservation scope.",
                files,
            ))
            .expect_err("empty reservation scopes should reject");

        assert!(matches!(error, StoreError::MissingScope));
        assert_eq!(store.event_count().expect("event count should load"), 0);
        assert_eq!(
            store
                .current_summary()
                .expect("current summary should load")
                .active_reservation_count,
            0
        );
    }
}

#[test]
fn reservation_declared_rejects_raw_normalized_empty_scope_payloads() {
    for (kind, path) in [
        ("file", "./"),
        ("file", "../"),
        ("file", "a/.."),
        ("directory", "/"),
    ] {
        let store = Store::open_in_memory().expect("in-memory store should open");
        let mut event = Event::reservation_declared(
            "s1",
            "w1",
            "Reject raw normalized-empty reservation scope.",
            ["src/auth.ts"],
        );
        event.payload["scopes"] = serde_json::json!([{ "kind": kind, "path": path }]);

        let error = store
            .append(event)
            .expect_err("raw normalized-empty reservation scope should reject");

        assert!(matches!(error, StoreError::MissingScope));
        assert_eq!(store.event_count().expect("event count should load"), 0);
        assert_eq!(
            store
                .current_summary()
                .expect("current summary should load")
                .active_reservation_count,
            0
        );
    }
}

#[test]
fn intent_declarations_preserve_existing_scope_in_same_workspace() {
    let store = Store::open_in_memory().expect("in-memory store should open");

    store
        .append(Event::reservation_declared(
            "s1",
            "w1",
            "Fix auth validation behavior.",
            ["src/auth.ts"],
        ))
        .expect("w1 reservation should append");
    store
        .append(Event::reservation_declared(
            "s1",
            "w2",
            "Update the public guide.",
            ["docs/guide.md"],
        ))
        .expect("w2 reservation should append");

    let w1_state = store
        .policy_state_for_session("s1", "w1")
        .expect("w1 policy state should load");
    let w1_decision =
        stateful_core::authorize_action(&w1_state, AuthorizationInput::write_file("src/auth.ts"));
    assert_eq!(w1_decision.decision, DecisionKind::Allow);

    let w2_state = store
        .policy_state_for_session("s1", "w2")
        .expect("w2 policy state should load");
    let w2_decision =
        stateful_core::authorize_action(&w2_state, AuthorizationInput::write_file("docs/guide.md"));
    assert_eq!(w2_decision.decision, DecisionKind::Allow);

    store
        .append(Event::reservation_declared(
            "s1",
            "w1",
            "Fix session behavior.",
            ["src/session.ts"],
        ))
        .expect("replacement w1 reservation should append");

    let w1_state = store
        .policy_state_for_session("s1", "w1")
        .expect("replacement w1 policy state should load");
    let old_w1_decision =
        stateful_core::authorize_action(&w1_state, AuthorizationInput::write_file("src/auth.ts"));
    let new_w1_decision = stateful_core::authorize_action(
        &w1_state,
        AuthorizationInput::write_file("src/session.ts"),
    );
    assert_eq!(old_w1_decision.decision, DecisionKind::Allow);
    assert_eq!(new_w1_decision.decision, DecisionKind::Allow);

    let w2_state = store
        .policy_state_for_session("s1", "w2")
        .expect("w2 policy state should still load");
    let w2_decision =
        stateful_core::authorize_action(&w2_state, AuthorizationInput::write_file("docs/guide.md"));
    assert_eq!(w2_decision.decision, DecisionKind::Allow);
}

#[test]
fn intent_declarations_allow_edit_and_artifact_scopes_to_coexist() {
    let store = Store::open_in_memory().expect("in-memory store should open");

    store
        .append(Event::reservation_declared(
            "s1",
            "w1",
            "Fix auth validation behavior.",
            ["src/auth.ts"],
        ))
        .expect("edit reservation should append");
    store
        .append(Event::reservation_declared(
            "s1",
            "w1",
            "Run the workspace test suite.",
            ["tmp/test-suite/"],
        ))
        .expect("artifact reservation should append");

    store
        .acquire_claim("s1", "w1", "src/auth.ts")
        .expect("edit claim should still be authorized");
    store
        .acquire_claim("s1", "w1", "tmp/test-suite/")
        .expect("artifact claim should be authorized");

    let live = store
        .live_current_state(None)
        .expect("live current state should load");
    assert!(
        live.items.iter().any(|item| item.resource == "src/auth.ts"
            && item.purpose == "Fix auth validation behavior.")
    );
    assert!(
        live.items
            .iter()
            .any(|item| item.resource == "tmp/test-suite/"
                && item.purpose == "Run the workspace test suite.")
    );
}

#[test]
fn expired_intent_is_not_write_authorizing_or_counted_active() {
    let store = Store::open_in_memory().expect("in-memory store should open");
    let mut stale_reservation = Event::reservation_declared(
        "stale-session",
        "w1",
        "Fix stale auth behavior.",
        ["src/auth.ts"],
    );
    stale_reservation.created_at = "1970-01-01T00:00:00Z".to_string();

    store
        .append(stale_reservation)
        .expect("stale reservation event should append");

    let state = store
        .policy_state_for_session("stale-session", "w1")
        .expect("policy state should load");
    let decision =
        stateful_core::authorize_action(&state, AuthorizationInput::write_file("src/auth.ts"));
    let summary = store.current_summary().expect("summary should load");

    assert_eq!(decision.decision, DecisionKind::Deny);
    assert_eq!(decision.reason_code, "missing_reservation");
    assert_eq!(summary.active_reservation_count, 0);
}

#[test]
fn file_store_persists_events_and_materialized_views_across_reopen() {
    let temp_root =
        std::env::temp_dir().join(format!("stateful-store-file-{}", std::process::id()));
    if temp_root.exists() {
        fs::remove_dir_all(&temp_root).expect("old temp root should be removable");
    }
    fs::create_dir_all(&temp_root).expect("temp root should be creatable");
    let db_path = temp_root.join(".stateful_core").join("state.db");

    {
        let store = Store::open(&db_path).expect("file store should open");
        store
            .append(Event::session_registered("s1", "w1").with_event_id("event-1"))
            .expect("event should append");
    }

    let reopened = Store::open(&db_path).expect("file store should reopen");

    assert_eq!(reopened.event_count().expect("event count should load"), 1);
    let session = reopened
        .session("s1")
        .expect("session lookup should succeed")
        .expect("session should be materialized");
    assert_eq!(session.workspace_id, "w1");

    fs::remove_dir_all(&temp_root).expect("temp root should be removable");
}

#[test]
fn file_store_enables_wal_journal_mode() {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock should be after epoch")
        .as_nanos();
    let temp_root = std::env::temp_dir().join(format!(
        "stateful-store-wal-journal-{}-{unique}",
        std::process::id()
    ));
    fs::create_dir_all(&temp_root).expect("temp root should be creatable");
    let db_path = temp_root.join("state.db");

    Store::open(&db_path).expect("file store should open");

    let conn = rusqlite::Connection::open(&db_path).expect("db should reopen");
    let journal_mode: String = conn
        .query_row("PRAGMA journal_mode", [], |row| row.get(0))
        .expect("journal mode should load");
    assert_eq!(journal_mode.to_ascii_lowercase(), "wal");

    fs::remove_dir_all(&temp_root).expect("temp root should be removable");
}

#[test]
fn file_store_waits_for_short_sqlite_write_locks() {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock should be after epoch")
        .as_nanos();
    let temp_root = std::env::temp_dir().join(format!(
        "stateful-store-busy-timeout-{}-{unique}",
        std::process::id()
    ));
    fs::create_dir_all(&temp_root).expect("temp root should be creatable");
    let db_path = temp_root.join("state.db");
    let store = Store::open(&db_path).expect("initial file store should open");

    let (locked_tx, locked_rx) = mpsc::channel();
    let lock_db_path = db_path.clone();
    let lock_thread = thread::spawn(move || {
        let conn = rusqlite::Connection::open(lock_db_path).expect("lock db should open");
        conn.execute_batch("BEGIN IMMEDIATE")
            .expect("write lock should start");
        locked_tx.send(()).expect("lock signal should send");
        thread::sleep(StdDuration::from_millis(150));
        conn.execute_batch("COMMIT").expect("write lock should end");
    });

    locked_rx.recv().expect("write lock should be active");
    let append_result = store.append(Event::session_registered("s-waiter", "w1"));
    lock_thread.join().expect("lock thread should finish");

    append_result.expect("append should wait for short sqlite write lock");

    fs::remove_dir_all(&temp_root).expect("temp root should be removable");
}

#[test]
fn retention_pruning_removes_old_history_but_preserves_live_notification_state() {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock should be after epoch")
        .as_nanos();
    let temp_root = std::env::temp_dir().join(format!(
        "stateful-store-retention-pruning-{}-{unique}",
        std::process::id()
    ));
    fs::create_dir_all(&temp_root).expect("temp root should be creatable");
    let db_path = temp_root.join("state.db");
    let store = Store::open(&db_path).expect("file store should open");

    let mut old_event = Event::session_registered("old-session", "w1").with_event_id("old-event");
    old_event.created_at = "2026-05-01T00:00:00Z".to_string();
    store.append(old_event).expect("old event should append");
    let mut recent_event =
        Event::session_registered("recent-session", "w1").with_event_id("recent-event");
    recent_event.created_at = "2026-05-20T00:00:00Z".to_string();
    store
        .append(recent_event)
        .expect("recent event should append");

    let conn = Connection::open(&db_path).expect("db should reopen");
    conn.execute_batch(
        "
        INSERT INTO reconciliations (reconciliation_id, session_id, created_at)
        VALUES
            ('old-reconciliation', 's1', '2026-05-01T00:00:00Z'),
            ('recent-reconciliation', 's1', '2026-05-20T00:00:00Z');

        INSERT INTO notifications (
            notification_id,
            target_session_id,
            workspace_id,
            kind,
            payload_json,
            status,
            created_at,
            expires_at
        ) VALUES
            ('old-expired-notification', 's1', 'w1', 'reservation_granted', '{}', 'expired', '2026-05-01T00:00:00Z', '2026-05-02T00:00:00Z'),
            ('old-pending-notification', 's1', 'w1', 'reservation_granted', '{}', 'pending', '2026-05-01T00:00:00Z', '2026-05-30T00:00:00Z'),
            ('recent-expired-notification', 's1', 'w1', 'reservation_granted', '{}', 'expired', '2026-05-20T00:00:00Z', '2026-05-21T00:00:00Z');
        ",
    )
    .expect("history fixtures should insert");

    store
        .prune_retention_before("2026-05-15T00:00:00Z")
        .expect("old history should prune");

    let events = store.recent_events(10).expect("events should load");
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].event_id, "recent-event");

    let reconciliation_ids = query_ids(&conn, "SELECT reconciliation_id FROM reconciliations");
    assert_eq!(reconciliation_ids, vec!["recent-reconciliation"]);

    let notification_ids = query_ids(&conn, "SELECT notification_id FROM notifications");
    assert_eq!(
        notification_ids,
        vec!["old-pending-notification", "recent-expired-notification"]
    );

    fs::remove_dir_all(&temp_root).expect("temp root should be removable");
}

#[test]
fn migration_adds_repo_identity_columns_to_existing_events_table() {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock should be after epoch")
        .as_nanos();
    let temp_root = std::env::temp_dir().join(format!(
        "stateful-store-old-events-{}-{unique}",
        std::process::id()
    ));
    fs::create_dir_all(&temp_root).expect("temp root should be creatable");
    let db_path = temp_root.join("state.db");

    {
        let conn = rusqlite::Connection::open(&db_path).expect("old db should open");
        conn.execute_batch(
            "
            CREATE TABLE events (
                event_id TEXT PRIMARY KEY,
                event_type TEXT NOT NULL,
                session_id TEXT NOT NULL,
                workspace_id TEXT NOT NULL,
                sequence INTEGER,
                payload_json TEXT NOT NULL,
                created_at TEXT NOT NULL
            );

            INSERT INTO events (
                event_id,
                event_type,
                session_id,
                workspace_id,
                sequence,
                payload_json,
                created_at
            ) VALUES (
                'legacy-event-1',
                'SessionRegistered',
                'legacy-session',
                'legacy-workspace',
                NULL,
                '{}',
                '2026-05-31T00:00:00Z'
            );
            ",
        )
        .expect("old events table should be created");
    }

    let store = Store::open(&db_path).expect("store should migrate old db");
    store
        .append(
            Event::session_registered("s1", "w1").with_workspace_identity(
                "repo-1",
                "worktree-1",
                "/repo",
                "main",
            ),
        )
        .expect("event should append after migration");

    let events = store.recent_events(10).expect("events should read");

    assert_eq!(events.len(), 2);
    assert_eq!(events[0].repo_id.as_deref(), Some("repo-1"));
    assert_eq!(events[0].worktree_id.as_deref(), Some("worktree-1"));
    assert_eq!(events[0].root.as_deref(), Some("/repo"));
    assert_eq!(events[0].branch.as_deref(), Some("main"));
    assert_eq!(events[1].event_id, "legacy-event-1");
    assert_eq!(events[1].repo_id, None);
    assert_eq!(events[1].worktree_id, None);
    assert_eq!(events[1].root, None);
    assert_eq!(events[1].branch, None);

    fs::remove_dir_all(&temp_root).expect("temp root should be removable");
}

#[test]
fn migration_removes_legacy_coordination_rows_without_required_purpose() {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock should be after epoch")
        .as_nanos();
    let temp_root = std::env::temp_dir().join(format!(
        "stateful-store-legacy-purpose-cleanup-{}-{unique}",
        std::process::id()
    ));
    fs::create_dir_all(&temp_root).expect("temp root should be creatable");
    let db_path = temp_root.join("state.db");

    {
        let conn = rusqlite::Connection::open(&db_path).expect("old db should open");
        conn.execute_batch(
            "
            CREATE TABLE reservations (
                reservation_id TEXT PRIMARY KEY,
                session_id TEXT NOT NULL,
                workspace_id TEXT NOT NULL,
                scopes_json TEXT NOT NULL,
                status TEXT NOT NULL,
                declared_at TEXT NOT NULL,
                expires_at TEXT
            );

            INSERT INTO reservations (
                reservation_id,
                session_id,
                workspace_id,
                scopes_json,
                status,
                declared_at,
                expires_at
            ) VALUES (
                'legacy-reservation-1',
                'legacy-session',
                'legacy-workspace',
                '[]',
                'active',
                '2026-05-31T00:00:00Z',
                '2999-01-01T00:00:00Z'
            );

            CREATE TABLE wait_queue (
                wait_id TEXT PRIMARY KEY,
                session_id TEXT NOT NULL,
                workspace_id TEXT NOT NULL,
                relative_path TEXT NOT NULL,
                action TEXT NOT NULL,
                status TEXT NOT NULL,
                requested_at TEXT NOT NULL,
                reservation_expires_at TEXT,
                blocking_session_id TEXT
            );

            INSERT INTO wait_queue (
                wait_id,
                session_id,
                workspace_id,
                relative_path,
                action,
                status,
                requested_at,
                reservation_expires_at,
                blocking_session_id
            ) VALUES (
                'legacy-waiter-1',
                'legacy-waiter-session',
                'legacy-workspace',
                'src/auth.ts',
                'write_file',
                'queued',
                '2026-05-31T00:00:00Z',
                NULL,
                'legacy-session'
            );

            CREATE TABLE claims (
                claim_id TEXT PRIMARY KEY,
                session_id TEXT,
                workspace_id TEXT NOT NULL,
                repo_id TEXT,
                relative_path TEXT,
                absolute_path TEXT,
                status TEXT NOT NULL,
                expires_at TEXT
            );

            INSERT INTO claims (
                claim_id,
                session_id,
                workspace_id,
                repo_id,
                relative_path,
                absolute_path,
                status,
                expires_at
            ) VALUES (
                'legacy-claim-1',
                'legacy-session',
                'legacy-workspace',
                NULL,
                'src/auth.ts',
                NULL,
                'active',
                '2999-01-01T00:00:00Z'
            );
            ",
        )
        .expect("legacy coordination tables should be created");
    }

    let store = Store::open(&db_path).expect("store should migrate old coordination db");
    let live = store
        .live_current_state(None)
        .expect("legacy purpose-less rows should not break live current state");

    assert_eq!(live.summary.active_reservation_count, 0);
    assert!(live.items.is_empty());

    drop(store);
    let conn = rusqlite::Connection::open(&db_path).expect("migrated db should reopen");
    let intent_count: u64 = conn
        .query_row("SELECT COUNT(*) FROM reservations", [], |row| row.get(0))
        .expect("reservation count should load");
    let waiter_count: u64 = conn
        .query_row("SELECT COUNT(*) FROM wait_queue", [], |row| row.get(0))
        .expect("waiter count should load");
    let active_claim_count: u64 = conn
        .query_row(
            "SELECT COUNT(*) FROM claims WHERE status = 'active'",
            [],
            |row| row.get(0),
        )
        .expect("active claim count should load");
    assert_eq!(intent_count, 0);
    assert_eq!(waiter_count, 0);
    assert_eq!(active_claim_count, 0);

    fs::remove_dir_all(&temp_root).expect("temp root should be removable");
}

#[test]
fn failed_materialization_rolls_back_event_insert_and_allows_future_appends() {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock should be after epoch")
        .as_nanos();
    let temp_root = std::env::temp_dir().join(format!(
        "stateful-store-failed-materialize-{}-{unique}",
        std::process::id()
    ));
    fs::create_dir_all(&temp_root).expect("temp root should be creatable");
    let db_path = temp_root.join("state.db");

    {
        let conn = rusqlite::Connection::open(&db_path).expect("db should open");
        conn.execute_batch(
            "
            CREATE TABLE reservations (
                reservation_id TEXT PRIMARY KEY,
                session_id TEXT NOT NULL,
                workspace_id TEXT NOT NULL,
                scopes_json TEXT NOT NULL,
                status TEXT NOT NULL,
                declared_at TEXT NOT NULL,
                expires_at TEXT
            );

            CREATE TRIGGER fail_intent_materialization
            BEFORE INSERT ON reservations
            BEGIN
                SELECT RAISE(FAIL, 'forced reservation materialization failure');
            END;
            ",
        )
        .expect("failing reservation trigger should be created");
    }

    let store = Store::open(&db_path).expect("store should open");

    let error = store
        .append(Event::reservation_declared(
            "s1",
            "w1",
            "Fix auth validation behavior.",
            ["src/auth.ts"],
        ))
        .expect_err("reservation materialization should fail");
    assert!(
        error
            .to_string()
            .contains("forced reservation materialization failure"),
        "unexpected error: {error}"
    );
    assert_eq!(store.event_count().expect("event count should load"), 0);

    store
        .append(Event::session_registered("s2", "w1"))
        .expect("subsequent append should start a new transaction");
    assert_eq!(store.event_count().expect("event count should load"), 1);

    fs::remove_dir_all(&temp_root).expect("temp root should be removable");
}

#[test]
fn current_summary_counts_sessions_events_and_active_reservations() {
    let store = Store::open_in_memory().expect("in-memory store should open");

    store
        .append(Event::session_registered("s1", "w1"))
        .expect("session event should append");
    store
        .append(Event::reservation_declared(
            "s1",
            "w1",
            "Fix auth validation behavior.",
            ["src/auth.ts"],
        ))
        .expect("reservation event should append");

    let summary = store.current_summary().expect("summary should load");

    assert_eq!(summary.session_count, 1);
    assert_eq!(summary.active_reservation_count, 1);
    assert_eq!(summary.event_count, 2);
}

#[test]
fn live_current_state_reports_active_items_with_purpose() {
    let store = Store::open_in_memory().expect("in-memory store should open");

    store
        .append(Event::reservation_declared(
            "s1",
            "w1",
            "Fix auth validation behavior.",
            ["src/auth.ts"],
        ))
        .expect("reservation should append");
    acquire_test_lease(&store, "s1", "w1", "src/auth.ts");
    store
        .enqueue_reservation_request(ReservationRequestInput {
            request_id: "request-1",
            session_id: "s2",
            workspace_id: "w1",
            relative_path: "src/auth.ts",
            action: "write_file",
            purpose: "Update the same auth file after the active claim clears.",
            blocking_session_id: Some("s1"),
        })
        .expect("waiter should enqueue");

    let live = store
        .live_current_state(Some("src/auth.ts"))
        .expect("live current state should load");

    assert_eq!(live.summary.active_reservation_count, 1);
    let reservation = live
        .items
        .iter()
        .find(|item| item.kind == CurrentItemKind::Reservation)
        .expect("reservation item should exist");
    assert_eq!(reservation.resource, "src/auth.ts");
    assert_eq!(reservation.purpose, "Fix auth validation behavior.");
    assert_eq!(
        reservation.evidence_kind,
        Some(CurrentEvidenceKind::DeclaredReservation)
    );

    let claim = live
        .items
        .iter()
        .find(|item| item.kind == CurrentItemKind::Claim)
        .expect("claim item should exist");
    assert_eq!(claim.purpose, "Fix auth validation behavior.");
    assert_eq!(claim.evidence_kind, Some(CurrentEvidenceKind::ClaimOnly));

    let waiter = live
        .items
        .iter()
        .find(|item| item.kind == CurrentItemKind::WaitQueue)
        .expect("wait queue item should exist");
    assert_eq!(
        waiter.purpose,
        "Update the same auth file after the active claim clears."
    );
    assert_eq!(waiter.evidence_kind, Some(CurrentEvidenceKind::WaitQueue));
}

#[test]
fn live_current_state_uses_lease_acquisition_purpose_after_intent_redeclare() {
    let store = Store::open_in_memory().expect("in-memory store should open");

    store
        .append(Event::reservation_declared(
            "s1",
            "w1",
            "Fix auth validation behavior.",
            ["src/auth.ts"],
        ))
        .expect("first reservation should append");
    acquire_test_lease(&store, "s1", "w1", "src/auth.ts");
    store
        .append(Event::reservation_declared(
            "s1",
            "w1",
            "Run the workspace test suite.",
            ["target/"],
        ))
        .expect("second reservation should append");

    let live = store
        .live_current_state(Some("src/auth.ts"))
        .expect("live current state should load");
    let claim = live
        .items
        .iter()
        .find(|item| item.kind == CurrentItemKind::Claim)
        .expect("claim item should exist");

    assert_eq!(claim.purpose, "Fix auth validation behavior.");
}

#[test]
fn live_current_state_preserves_directory_lease_resource_shape() {
    let store = Store::open_in_memory().expect("in-memory store should open");

    store
        .append(Event::reservation_declared(
            "s1",
            "w1",
            "Run build artifacts under target.",
            ["target/"],
        ))
        .expect("directory reservation should append");
    store
        .acquire_claim("s1", "w1", "target/")
        .expect("directory claim should acquire");

    let live = store
        .live_current_state(Some("target/"))
        .expect("live current state should load");
    let claim = live
        .items
        .iter()
        .find(|item| item.kind == CurrentItemKind::Claim)
        .expect("claim item should exist");

    assert_eq!(claim.resource, "target/");
    assert_eq!(claim.summary, "s1 has an active write claim on target/.");
}

#[test]
fn event_records_return_recent_audit_events() {
    let store = Store::open_in_memory().expect("in-memory store should open");

    store
        .append(Event::session_registered("s1", "w1").with_event_id("event-1"))
        .expect("first event should append");
    store
        .append(
            Event::reservation_declared(
                "s1",
                "w1",
                "Fix auth validation behavior.",
                ["src/auth.ts"],
            )
            .with_event_id("event-2"),
        )
        .expect("second event should append");

    let events = store.recent_events(10).expect("events should load");

    assert_eq!(events.len(), 2);
    assert_eq!(events[0].event_id, "event-2");
    assert_eq!(events[0].event_type, "ReservationDeclared");
    assert_eq!(events[1].event_id, "event-1");
}

#[test]
fn event_records_preserve_repo_identity_when_present() {
    let store = Store::open_in_memory().expect("store should open");
    let event = Event::session_registered("s1", "w1").with_workspace_identity(
        "repo-1",
        "worktree-1",
        "/repo",
        "main",
    );

    store.append(event).expect("event should append");
    let events = store.recent_events(10).expect("events should read");

    assert_eq!(events[0].repo_id.as_deref(), Some("repo-1"));
    assert_eq!(events[0].worktree_id.as_deref(), Some("worktree-1"));
    assert_eq!(events[0].root.as_deref(), Some("/repo"));
    assert_eq!(events[0].branch.as_deref(), Some("main"));
}

#[test]
fn session_heartbeat_does_not_revive_expired_intent() {
    let store = Store::open_in_memory().expect("in-memory store should open");
    let mut reservation =
        Event::reservation_declared("s1", "w1", "Fix auth validation behavior.", ["src/auth.ts"]);
    reservation.created_at = "2999-01-01T00:00:00Z".to_string();
    store
        .append(reservation)
        .expect("reservation should append");

    let mut heartbeat = Event::session_heartbeat("s1", "w1");
    heartbeat.created_at = "2999-01-01T00:16:00Z".to_string();
    store
        .append(heartbeat)
        .expect("heartbeat should append without reviving expired reservation");

    let live = store
        .live_current_state(Some("src/auth.ts"))
        .expect("live current state should load");
    assert!(
        live.items
            .iter()
            .all(|item| item.kind != CurrentItemKind::Reservation),
        "expired reservation should not remain live: {:?}",
        live.items
    );
    assert_eq!(live.summary.active_reservation_count, 0);
}

#[test]
fn session_heartbeat_expires_stale_lease_and_promotes_waiter() {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock should be after unix epoch")
        .as_nanos();
    let temp_root =
        std::env::temp_dir().join(format!("stateful-store-heartbeat-expired-claim-{unique}"));
    fs::create_dir_all(&temp_root).expect("temp root should be creatable");
    let db_path = temp_root.join("state.db");
    let store = Store::open(&db_path).expect("file store should open");
    acquire_test_lease(&store, "s1", "w1", "src/auth.ts");
    let waiter = store
        .enqueue_waiter(
            "s2",
            "w1",
            "src/auth.ts",
            "write_file",
            "Queue requested file write after blocker expires.",
            Some("s1"),
        )
        .expect("waiter should enqueue");
    drop(store);

    let conn = rusqlite::Connection::open(&db_path).expect("db should reopen");
    conn.execute(
        "UPDATE claims SET expires_at = ?1 WHERE session_id = ?2",
        ["2999-01-01T00:05:00Z", "s1"],
    )
    .expect("claim expiry should update");
    drop(conn);

    let store = Store::open(&db_path).expect("file store should reopen");
    let mut heartbeat = Event::session_heartbeat("s1", "w1");
    heartbeat.created_at = "2999-01-01T00:10:00Z".to_string();
    store
        .append(heartbeat)
        .expect("heartbeat should expire stale claim and promote waiter");

    assert_eq!(
        store
            .active_claim_owner("w1", "src/auth.ts")
            .expect("claim owner should load"),
        None
    );
    let reservation = store
        .active_reservation("w1", "src/auth.ts")
        .expect("reservation lookup should succeed")
        .expect("expired claim should promote waiting session");
    assert_eq!(reservation.wait_id, waiter.wait_id);
    assert_eq!(reservation.session_id, "s2");

    fs::remove_dir_all(&temp_root).expect("temp root should be removable");
}

#[test]
fn session_heartbeat_extends_active_reservation_expiry() {
    let store = Store::open_in_memory().expect("in-memory store should open");
    let mut reservation =
        Event::reservation_declared("s1", "w1", "Fix auth validation behavior.", ["src/auth.ts"]);
    reservation.created_at = "2999-01-01T00:00:00Z".to_string();
    store
        .append(reservation)
        .expect("reservation should append");

    let before = store
        .live_current_state(Some("src/auth.ts"))
        .expect("live current state should load")
        .items
        .into_iter()
        .find(|item| item.kind == CurrentItemKind::Reservation)
        .expect("reservation item should exist");
    assert_eq!(before.expires_at.as_deref(), Some("2999-01-01T00:15:00Z"));

    let mut heartbeat = Event::session_heartbeat("s1", "w1");
    heartbeat.created_at = "2999-01-01T00:10:00Z".to_string();
    store
        .append(heartbeat)
        .expect("heartbeat should append and refresh reservation");

    let after = store
        .live_current_state(Some("src/auth.ts"))
        .expect("live current state should load")
        .items
        .into_iter()
        .find(|item| item.kind == CurrentItemKind::Reservation)
        .expect("reservation item should exist");
    assert_eq!(after.expires_at.as_deref(), Some("2999-01-01T00:25:00Z"));
}

#[test]
fn session_heartbeat_caps_active_reservation_expiry_at_max_lifetime() {
    let store = Store::open_in_memory().expect("in-memory store should open");
    let mut reservation =
        Event::reservation_declared("s1", "w1", "Fix auth validation behavior.", ["src/auth.ts"]);
    reservation.created_at = "2999-01-01T00:00:00Z".to_string();
    store
        .append(reservation)
        .expect("reservation should append");

    for heartbeat_at in [
        "2999-01-01T00:10:00Z",
        "2999-01-01T00:24:00Z",
        "2999-01-01T00:38:00Z",
        "2999-01-01T00:52:00Z",
    ] {
        let mut heartbeat = Event::session_heartbeat("s1", "w1");
        heartbeat.created_at = heartbeat_at.to_string();
        store
            .append(heartbeat)
            .expect("heartbeat should append and refresh reservation");
    }

    let reservation = store
        .live_current_state(Some("src/auth.ts"))
        .expect("live current state should load")
        .items
        .into_iter()
        .find(|item| item.kind == CurrentItemKind::Reservation)
        .expect("reservation item should exist");
    assert_eq!(
        reservation.expires_at.as_deref(),
        Some("2999-01-01T01:00:00Z")
    );
}

#[test]
fn session_heartbeat_fails_closed_for_malformed_reservation_declared_at() {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock should be after unix epoch")
        .as_nanos();
    let temp_root = std::env::temp_dir().join(format!(
        "stateful-store-heartbeat-malformed-reservation-{unique}"
    ));
    fs::create_dir_all(&temp_root).expect("temp root should be creatable");
    let db_path = temp_root.join("state.db");
    let store = Store::open(&db_path).expect("file store should open");
    let mut reservation =
        Event::reservation_declared("s1", "w1", "Fix auth validation behavior.", ["src/auth.ts"]);
    let now = OffsetDateTime::now_utc();
    let heartbeat_at = test_timestamp(now - TimeDuration::minutes(5));
    let current_expires_at = test_timestamp(now + TimeDuration::minutes(5));
    reservation.created_at = test_timestamp(now - TimeDuration::minutes(10));
    store
        .append(reservation)
        .expect("reservation should append");
    drop(store);

    let conn = rusqlite::Connection::open(&db_path).expect("db should reopen");
    conn.execute(
        "UPDATE reservations SET declared_at = ?1, expires_at = ?2 WHERE session_id = ?3",
        ["not-a-timestamp", current_expires_at.as_str(), "s1"],
    )
    .expect("reservation timestamp should corrupt");
    drop(conn);

    let store = Store::open(&db_path).expect("file store should reopen");
    let mut heartbeat = Event::session_heartbeat("s1", "w1");
    heartbeat.created_at = heartbeat_at;
    let error = store
        .append(heartbeat)
        .expect_err("heartbeat should fail closed for malformed reservation timestamp");
    assert!(
        error.to_string().contains("invalid timestamp"),
        "unexpected error: {error}"
    );
    drop(store);

    let conn = rusqlite::Connection::open(&db_path).expect("db should reopen");
    let expires_at: String = conn
        .query_row(
            "SELECT expires_at FROM reservations WHERE session_id = ?1",
            ["s1"],
            |row| row.get(0),
        )
        .expect("reservation expiry should load");
    assert_eq!(expires_at, current_expires_at);
    let event_count: u64 = conn
        .query_row("SELECT COUNT(*) FROM events", [], |row| row.get(0))
        .expect("event count should load");
    assert_eq!(event_count, 1);

    fs::remove_dir_all(&temp_root).expect("temp root should be removable");
}

#[test]
fn reservation_declared_fails_closed_for_malformed_created_at() {
    let store = Store::open_in_memory().expect("in-memory store should open");
    let mut reservation =
        Event::reservation_declared("s1", "w1", "Fix auth validation behavior.", ["src/auth.ts"]);
    reservation.created_at = "not-a-timestamp".to_string();

    let error = store
        .append(reservation)
        .expect_err("malformed reservation timestamp should fail closed");

    assert!(
        error.to_string().contains("invalid timestamp"),
        "unexpected error: {error}"
    );
    assert_eq!(store.event_count().expect("event count should load"), 0);
    assert!(
        store
            .live_current_state(Some("src/auth.ts"))
            .expect("current state should load")
            .items
            .is_empty()
    );
}

#[test]
fn session_heartbeat_extends_active_claim_expiry() {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock should be after unix epoch")
        .as_nanos();
    let temp_root =
        std::env::temp_dir().join(format!("stateful-store-heartbeat-live-claim-{unique}"));
    fs::create_dir_all(&temp_root).expect("temp root should be creatable");
    let db_path = temp_root.join("state.db");
    let store = Store::open(&db_path).expect("file store should open");
    acquire_test_lease(&store, "s1", "w1", "src/auth.ts");
    drop(store);

    let conn = rusqlite::Connection::open(&db_path).expect("db should reopen");
    conn.execute(
        "UPDATE claims SET expires_at = ?1 WHERE session_id = ?2",
        ["2999-01-01T00:11:00Z", "s1"],
    )
    .expect("claim expiry should update");
    conn.execute(
        "UPDATE reservations SET declared_at = ?1, expires_at = ?2 WHERE session_id = ?3",
        ["2999-01-01T00:00:00Z", "2999-01-01T00:20:00Z", "s1"],
    )
    .expect("reservation expiry should update");
    drop(conn);

    let store = Store::open(&db_path).expect("file store should reopen");
    let mut heartbeat = Event::session_heartbeat("s1", "w1");
    heartbeat.created_at = "2999-01-01T00:10:00Z".to_string();
    store
        .append(heartbeat)
        .expect("heartbeat should append and refresh claim");

    let claim = store
        .live_current_state(Some("src/auth.ts"))
        .expect("live current state should load")
        .items
        .into_iter()
        .find(|item| item.kind == CurrentItemKind::Claim)
        .expect("claim item should exist");
    assert_eq!(claim.expires_at.as_deref(), Some("2999-01-01T00:15:00Z"));

    fs::remove_dir_all(&temp_root).expect("temp root should be removable");
}

#[test]
fn session_heartbeat_does_not_extend_lease_without_matching_active_reservation() {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock should be after unix epoch")
        .as_nanos();
    let temp_root =
        std::env::temp_dir().join(format!("stateful-store-heartbeat-uncovered-claim-{unique}"));
    fs::create_dir_all(&temp_root).expect("temp root should be creatable");
    let db_path = temp_root.join("state.db");
    let store = Store::open(&db_path).expect("file store should open");
    acquire_test_lease(&store, "s1", "w1", "src/auth.ts");
    store
        .complete_session_reservations("s1", "w1")
        .expect("reservation should complete");
    drop(store);

    let conn = rusqlite::Connection::open(&db_path).expect("db should reopen");
    conn.execute(
        "UPDATE claims SET expires_at = ?1 WHERE session_id = ?2",
        ["2999-01-01T00:11:00Z", "s1"],
    )
    .expect("claim expiry should update");
    drop(conn);

    let store = Store::open(&db_path).expect("file store should reopen");
    let mut heartbeat = Event::session_heartbeat("s1", "w1");
    heartbeat.created_at = "2999-01-01T00:10:00Z".to_string();
    store
        .append(heartbeat)
        .expect("heartbeat should append without refreshing uncovered claim");

    let claim = store
        .live_current_state(Some("src/auth.ts"))
        .expect("live current state should load")
        .items
        .into_iter()
        .find(|item| item.kind == CurrentItemKind::Claim)
        .expect("claim item should still exist");
    assert_eq!(claim.expires_at.as_deref(), Some("2999-01-01T00:11:00Z"));

    fs::remove_dir_all(&temp_root).expect("temp root should be removable");
}

#[test]
fn acquire_claim_requires_matching_active_reservation() {
    let store = Store::open_in_memory().expect("in-memory store should open");

    let error = store
        .acquire_claim("s1", "w1", "src/auth.ts")
        .expect_err("claim without matching reservation should fail");

    assert!(matches!(error, StoreError::MissingReservation));
    assert_eq!(store.lease_count().expect("claim count should load"), 0);
}

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

#[test]
fn acquire_claim_rejects_direct_tmp_resource_even_with_matching_reservation() {
    for path in ["tmp", "tmp/"] {
        let store = Store::open_in_memory().expect("in-memory store should open");
        store
            .append(Event::reservation_declared(
                "s1",
                "w1",
                "Run test artifacts in a scoped tmp path.",
                [path],
            ))
            .expect("tmp reservation should append");

        let error = store
            .acquire_claim("s1", "w1", path)
            .expect_err("direct tmp claim should be rejected");
        assert!(
            error
                .to_string()
                .contains("direct tmp claims are not allowed"),
            "unexpected error: {error}"
        );
        assert_eq!(store.lease_count().expect("claim count should load"), 0);
    }
}

#[test]
fn acquire_directory_lease_rejects_file_intent_with_same_normalized_path() {
    let store = Store::open_in_memory().expect("in-memory store should open");

    store
        .append(Event::reservation_declared(
            "s1",
            "w1",
            "Acquire file target.",
            ["target"],
        ))
        .expect("file reservation should append");

    let error = store
        .acquire_claim("s1", "w1", "target/")
        .expect_err("directory claim with file reservation should fail");

    assert!(matches!(error, StoreError::MissingReservation));
    assert_eq!(store.lease_count().expect("claim count should load"), 0);
}

#[test]
fn acquire_claim_rejects_existing_active_file_claim_conflict() {
    let store = Store::open_in_memory().expect("in-memory store should open");

    acquire_test_lease(&store, "s1", "w1", "src/auth.ts");
    store
        .append(Event::reservation_declared(
            "s2",
            "w1",
            "Update the same auth file after the active claim clears.",
            ["src/auth.ts"],
        ))
        .expect("second reservation should append");

    let error = store
        .acquire_claim("s2", "w1", "src/auth.ts")
        .expect_err("conflicting active file claim should reject");

    assert!(matches!(error, StoreError::ClaimConflict));
    assert_eq!(store.lease_count().expect("claim count should load"), 1);
}
#[test]
fn acquire_claim_allows_same_path_in_different_workspaces() {
    let store = Store::open_in_memory().expect("in-memory store should open");

    acquire_test_lease(&store, "s1", "w1", "src/auth.ts");
    acquire_test_lease(&store, "s2", "w2", "src/auth.ts");

    assert_eq!(store.lease_count().expect("claim count should load"), 2);
    assert_eq!(
        store
            .active_claim_owner("w1", "src/auth.ts")
            .expect("w1 claim owner should load")
            .as_deref(),
        Some("s1")
    );
    assert_eq!(
        store
            .active_claim_owner("w2", "src/auth.ts")
            .expect("w2 claim owner should load")
            .as_deref(),
        Some("s2")
    );
}

#[test]
fn same_session_can_acquire_exact_file_lease_under_directory_lease() {
    let store = Store::open_in_memory().expect("in-memory store should open");

    store
        .append(Event::reservation_declared(
            "s1",
            "w1",
            "Acquire source directory.",
            ["src/"],
        ))
        .expect("directory reservation should append");
    store
        .acquire_claim("s1", "w1", "src/")
        .expect("directory claim should acquire");
    store
        .append(Event::reservation_declared(
            "s1",
            "w1",
            "Acquire exact auth file for native edit.",
            ["src/auth.ts"],
        ))
        .expect("file reservation should append");

    store
        .acquire_claim("s1", "w1", "src/auth.ts")
        .expect("same-session exact file claim should acquire under directory claim");

    assert!(
        store
            .active_claim_covers_path_by_session("w1", "src/auth.ts", "s1")
            .expect("file coverage should load")
    );
    assert!(
        store
            .active_exact_file_lease_by_session("w1", "src/auth.ts", "s1")
            .expect("exact file claim should load")
    );
    assert_eq!(store.lease_count().expect("claim count should load"), 2);
}

#[test]
fn acquire_claim_reports_already_held_for_same_session_duplicate_exact_file_lease() {
    let store = Store::open_in_memory().expect("in-memory store should open");

    acquire_test_lease(&store, "s1", "w1", "src/auth.ts");

    let error = store
        .acquire_claim("s1", "w1", "src/auth.ts")
        .expect_err("duplicate exact file claim should report already held");

    assert!(matches!(error, StoreError::ClaimAlreadyHeld));
    assert_eq!(store.lease_count().expect("claim count should load"), 1);
}

#[test]
fn acquire_claims_batch_is_atomic_and_idempotent_for_same_session() {
    let store = Store::open_in_memory().expect("in-memory store should open");
    store
        .append(Event::reservation_declared(
            "s1",
            "w1",
            "Acquire claims for a batch edit.",
            ["src/auth.ts", "src/session.ts"],
        ))
        .expect("batch reservation should append");

    let result = store
        .acquire_claims_with_observations_and_events(
            "s1",
            "w1",
            vec![
                ("src/auth.ts".to_string(), None),
                ("src/session.ts".to_string(), None),
            ],
        )
        .expect("batch claims should acquire");

    assert_eq!(result.acquired, 2);
    assert_eq!(result.already_held, 0);
    assert_eq!(store.lease_count().expect("claim count should load"), 2);

    let result = store
        .acquire_claims_with_observations_and_events(
            "s1",
            "w1",
            vec![
                ("src/auth.ts".to_string(), None),
                ("src/session.ts".to_string(), None),
            ],
        )
        .expect("already-held batch claims should be idempotent");

    assert_eq!(result.acquired, 0);
    assert_eq!(result.already_held, 2);
    assert_eq!(store.lease_count().expect("claim count should load"), 2);
}

#[test]
fn acquire_claims_batch_rolls_back_when_one_path_conflicts() {
    let store = Store::open_in_memory().expect("in-memory store should open");
    acquire_test_lease(&store, "s1", "w1", "src/auth.ts");
    store
        .append(Event::reservation_declared(
            "s2",
            "w1",
            "Acquire claims for a competing batch edit.",
            ["src/session.ts", "src/auth.ts"],
        ))
        .expect("batch reservation should append");

    let error = store
        .acquire_claims_with_observations_and_events(
            "s2",
            "w1",
            vec![
                ("src/session.ts".to_string(), None),
                ("src/auth.ts".to_string(), None),
            ],
        )
        .expect_err("conflicting batch claim should fail");

    assert!(matches!(error, StoreError::ClaimConflict));
    assert_eq!(store.lease_count().expect("claim count should load"), 1);
    assert_eq!(
        store
            .active_claim_owner("w1", "src/session.ts")
            .expect("session claim owner should load"),
        None
    );
}

#[test]
fn acquire_directory_lease_rejects_existing_child_file_claim_conflict() {
    let store = Store::open_in_memory().expect("in-memory store should open");

    acquire_test_lease(&store, "s1", "w1", "target/out.txt");
    store
        .append(Event::reservation_declared(
            "s2",
            "w1",
            "Rewrite build artifacts under target.",
            ["target/"],
        ))
        .expect("directory reservation should append");

    let error = store
        .acquire_claim("s2", "w1", "target/")
        .expect_err("conflicting active child file claim should reject");

    assert!(matches!(error, StoreError::ClaimConflict));
    assert_eq!(store.lease_count().expect("claim count should load"), 1);
}

#[test]
fn acquire_claim_rejects_existing_active_file_reservation_conflict() {
    let store = Store::open_in_memory().expect("in-memory store should open");

    let wait = store
        .enqueue_waiter(
            "s1",
            "w1",
            "src/auth.ts",
            "write_file",
            "Queue requested file write after blocker clears.",
            Some("s0"),
        )
        .expect("waiter should enqueue");
    store
        .promote_next_waiter("w1", "src/auth.ts")
        .expect("waiter should promote");
    store
        .append(Event::reservation_declared(
            "s2",
            "w1",
            "Update auth while reservation is active.",
            ["src/auth.ts"],
        ))
        .expect("second reservation should append");

    let error = store
        .acquire_claim("s2", "w1", "src/auth.ts")
        .expect_err("reserved file should reject direct claim acquire");

    assert!(matches!(error, StoreError::ClaimConflict));
    assert_eq!(store.lease_count().expect("claim count should load"), 0);
    store
        .claim_reservation(&wait.wait_id, "s1")
        .expect("owner should claim reservation");
    store
        .append(Event::reservation_declared(
            "s1",
            "w1",
            "Claim reserved auth update.",
            ["src/auth.ts"],
        ))
        .expect("claim owner reservation should append");
    store
        .acquire_claim("s1", "w1", "src/auth.ts")
        .expect("claimed reservation owner should acquire claim");
}

#[test]
fn acquire_directory_lease_rejects_existing_child_file_reservation_conflict() {
    let store = Store::open_in_memory().expect("in-memory store should open");

    store
        .enqueue_waiter(
            "s1",
            "w1",
            "target/out.txt",
            "write_file",
            "Queue requested file write after blocker clears.",
            Some("s0"),
        )
        .expect("waiter should enqueue");
    store
        .promote_next_waiter("w1", "target/out.txt")
        .expect("waiter should promote");
    store
        .append(Event::reservation_declared(
            "s2",
            "w1",
            "Rewrite build artifacts while child reservation is active.",
            ["target/"],
        ))
        .expect("directory reservation should append");

    let error = store
        .acquire_claim("s2", "w1", "target/")
        .expect_err("reserved child file should reject directory claim acquire");

    assert!(matches!(error, StoreError::ClaimConflict));
    assert_eq!(store.lease_count().expect("claim count should load"), 0);
}

#[test]
fn file_lease_with_directory_name_does_not_cover_descendants() {
    let store = Store::open_in_memory().expect("in-memory store should open");

    store
        .append(Event::reservation_declared(
            "s1",
            "w1",
            "Acquire file target.",
            ["target"],
        ))
        .expect("file reservation should append");
    store
        .acquire_claim("s1", "w1", "target")
        .expect("file claim should acquire");

    assert_eq!(
        store
            .active_claim_conflict_owner_for_path("w1", "target/debug/out.txt", "s2")
            .expect("path claim conflict should load"),
        None
    );
    assert!(
        !store
            .active_claim_covers_path_by_session("w1", "target/debug/out.txt", "s1")
            .expect("descendant file coverage should load")
    );
    assert!(
        !store
            .active_claim_covers_directory_by_session("w1", "target/", "s1")
            .expect("directory coverage should load")
    );
    assert!(
        store
            .active_exact_file_lease_by_session("w1", "target", "s1")
            .expect("exact file claim should load")
    );
}

#[test]
fn directory_lease_same_normalized_path_does_not_conflict_with_file_path() {
    let store = Store::open_in_memory().expect("in-memory store should open");

    acquire_test_lease(&store, "s2", "w1", "target/");

    assert_eq!(
        store
            .active_claim_conflict_owner_for_path("w1", "target", "s1")
            .expect("path claim conflict should load"),
        None
    );
    assert_eq!(
        store
            .active_claim_conflict_owner_for_directory("w1", "target/", "s1")
            .expect("directory claim conflict should load"),
        Some("s2".to_string())
    );
}

#[test]
fn release_claim_matches_requested_path_shape_when_file_and_directory_paths_overlap() {
    let store = Store::open_in_memory().expect("in-memory store should open");

    store
        .append(Event::reservation_declared(
            "s1",
            "w1",
            "Acquire file target.",
            ["target"],
        ))
        .expect("file reservation should append");
    store
        .acquire_claim("s1", "w1", "target")
        .expect("file claim should acquire");
    store
        .append(Event::reservation_declared(
            "s1",
            "w1",
            "Acquire directory target.",
            ["target/"],
        ))
        .expect("directory reservation should append");
    store
        .acquire_claim("s1", "w1", "target/")
        .expect("directory claim should acquire");

    store
        .release_claim("s1", "w1", "target")
        .expect("file claim should release");

    assert!(
        !store
            .active_exact_file_lease_by_session("w1", "target", "s1")
            .expect("exact file claim should load")
    );
    assert!(
        store
            .active_claim_covers_directory_by_session("w1", "target/", "s1")
            .expect("directory coverage should load")
    );
}

#[test]
fn release_claim_rejects_other_session_owner() {
    let store = Store::open_in_memory().expect("in-memory store should open");
    acquire_test_lease(&store, "s1", "w1", "target/");

    let error = store
        .release_claim("s2", "w1", "target/")
        .expect_err("other session should not release claim");

    assert!(error.to_string().contains("claim owner mismatch"));
    assert!(
        store
            .active_claim_covers_directory_by_session("w1", "target/", "s1")
            .expect("directory coverage should load")
    );
}

#[test]
fn release_claim_rejects_missing_same_session_lease() {
    let store = Store::open_in_memory().expect("in-memory store should open");

    let error = store
        .release_claim("s1", "w1", "target/")
        .expect_err("missing claim should not report success");

    assert!(error.to_string().contains("claim not found"));
}

#[test]
fn active_claim_owner_uses_normalized_relative_paths() {
    let store = Store::open_in_memory().expect("in-memory store should open");

    acquire_test_lease(&store, "s1", "w1", "src/./auth.ts");

    assert_eq!(
        store
            .active_claim_owner("w1", "src/auth.ts")
            .expect("claim owner should load"),
        Some("s1".to_string())
    );
}

#[test]
fn active_claim_conflict_for_directory_matches_subtree_paths() {
    let store = Store::open_in_memory().expect("in-memory store should open");

    acquire_test_lease(&store, "s2", "w1", "target/debug/out.txt");

    assert_eq!(
        store
            .active_claim_conflict_owner_for_directory("w1", "target/", "s1")
            .expect("directory claim conflict should load"),
        Some("s2".to_string())
    );
    assert_eq!(
        store
            .active_claim_conflict_owner_for_directory("w1", "target/", "s2")
            .expect("same-session directory claim should not conflict"),
        None
    );
    assert_eq!(
        store
            .active_claim_conflict_owner_for_directory("w1", "target-other/", "s1")
            .expect("sibling directory should not conflict"),
        None
    );
}

#[test]
fn active_claim_conflict_for_directory_matches_ancestor_directory_paths() {
    let store = Store::open_in_memory().expect("in-memory store should open");

    acquire_test_lease(&store, "s2", "w1", "target/");

    assert_eq!(
        store
            .active_claim_conflict_owner_for_directory("w1", "target/debug/", "s1")
            .expect("ancestor directory claim conflict should load"),
        Some("s2".to_string())
    );
}

#[test]
fn active_claim_covers_directory_by_same_session_matches_exact_or_ancestor_paths() {
    let store = Store::open_in_memory().expect("in-memory store should open");

    acquire_test_lease(&store, "s1", "w1", "target/");

    assert!(
        store
            .active_claim_covers_directory_by_session("w1", "target/", "s1")
            .expect("directory claim coverage should load")
    );
    assert!(
        store
            .active_claim_covers_directory_by_session("w1", "target/debug/", "s1")
            .expect("ancestor directory claim coverage should load")
    );
    assert!(
        !store
            .active_claim_covers_directory_by_session("w1", "target/", "s2")
            .expect("other session directory claim coverage should load")
    );
}

#[test]
fn active_claim_conflict_for_path_matches_ancestor_directory_paths() {
    let store = Store::open_in_memory().expect("in-memory store should open");

    acquire_test_lease(&store, "s2", "w1", "target/");

    assert_eq!(
        store
            .active_claim_conflict_owner_for_path("w1", "target/debug/out.txt", "s1")
            .expect("path claim conflict should load"),
        Some("s2".to_string())
    );
    assert_eq!(
        store
            .active_claim_conflict_owner_for_path("w1", "target/debug/out.txt", "s2")
            .expect("same-session directory claim should not conflict"),
        None
    );
    assert_eq!(
        store
            .active_claim_conflict_owner_for_path("w1", "target-other/out.txt", "s1")
            .expect("sibling directory should not conflict"),
        None
    );
}

#[test]
fn active_claim_covers_path_by_same_session_matches_exact_or_ancestor_paths() {
    let store = Store::open_in_memory().expect("in-memory store should open");

    acquire_test_lease(&store, "s1", "w1", "target/");
    acquire_test_lease(&store, "s1", "w1", "src/auth.ts");

    assert!(
        store
            .active_claim_covers_path_by_session("w1", "src/auth.ts", "s1")
            .expect("exact file claim coverage should load")
    );
    assert!(
        store
            .active_claim_covers_path_by_session("w1", "target/debug/out.txt", "s1")
            .expect("ancestor directory claim coverage should load")
    );
    assert!(
        !store
            .active_claim_covers_path_by_session("w1", "target-other/out.txt", "s1")
            .expect("sibling path claim coverage should load")
    );
    assert!(
        !store
            .active_claim_covers_path_by_session("w1", "src/auth.ts", "s2")
            .expect("other session file claim coverage should load")
    );
}

#[test]
fn active_exact_file_lease_by_same_session_ignores_ancestor_directory_lease() {
    let store = Store::open_in_memory().expect("in-memory store should open");

    acquire_test_lease(&store, "s1", "w1", "target/");
    acquire_test_lease(&store, "s1", "w1", "src/auth.ts");

    assert!(
        store
            .active_exact_file_lease_by_session("w1", "src/auth.ts", "s1")
            .expect("exact file claim should load")
    );
    assert!(
        !store
            .active_exact_file_lease_by_session("w1", "target/debug/out.txt", "s1")
            .expect("ancestor directory claim should not count as exact file claim")
    );
    assert!(
        !store
            .active_exact_file_lease_by_session("w1", "src/auth.ts", "s2")
            .expect("other session file claim should not count")
    );
}

#[test]
fn active_exact_file_intent_by_same_session_ignores_directory_intent() {
    let store = Store::open_in_memory().expect("in-memory store should open");

    store
        .append(Event::reservation_declared(
            "s1",
            "w1",
            "Edit files under src.",
            ["src/"],
        ))
        .expect("directory reservation should append");
    assert!(
        !store
            .active_exact_file_intent_by_session("w1", "src/auth.ts", "s1")
            .expect("directory scope should not count as exact file scope")
    );

    store
        .append(Event::reservation_declared(
            "s1",
            "w1",
            "Fix auth validation behavior.",
            ["src/auth.ts"],
        ))
        .expect("file reservation should append");
    assert!(
        store
            .active_exact_file_intent_by_session("w1", "src/auth.ts", "s1")
            .expect("task reservation exact file scope should load")
    );
    assert!(
        !store
            .active_exact_file_intent_by_session("w1", "src/auth.ts", "s2")
            .expect("other session task reservation exact file scope should not count")
    );
}

#[test]
fn expired_lease_is_not_returned_as_active_owner() {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock should be after epoch")
        .as_nanos();
    let temp_root = std::env::temp_dir().join(format!(
        "stateful-store-expired-claim-{}-{unique}",
        std::process::id()
    ));
    fs::create_dir_all(&temp_root).expect("temp root should be creatable");
    let db_path = temp_root.join("state.db");
    let store = Store::open(&db_path).expect("file store should open");
    acquire_test_lease(&store, "stale-session", "w1", "src/auth.ts");
    drop(store);

    let conn = rusqlite::Connection::open(&db_path).expect("db should reopen");
    conn.execute("UPDATE claims SET expires_at = '1970-01-01T00:00:00Z'", [])
        .expect("claim should be made stale");
    drop(conn);

    let store = Store::open(&db_path).expect("file store should reopen");
    assert_eq!(
        store
            .active_claim_owner("w1", "src/auth.ts")
            .expect("claim owner should load"),
        None
    );

    fs::remove_dir_all(&temp_root).expect("temp root should be removable");
}

#[test]
fn released_lease_promotes_first_waiter_to_reservation() {
    let store = Store::open_in_memory().expect("in-memory store should open");

    acquire_test_lease(&store, "s1", "w1", "src/auth.ts");
    let first = store
        .enqueue_waiter(
            "s2",
            "w1",
            "src/auth.ts",
            "write_file",
            "Queue requested file write after blocker clears.",
            Some("s1"),
        )
        .expect("first waiter should enqueue");
    let second = store
        .enqueue_waiter(
            "s3",
            "w1",
            "src/auth.ts",
            "write_file",
            "Queue requested file write after blocker clears.",
            Some("s1"),
        )
        .expect("second waiter should enqueue");

    store
        .release_claim("s1", "w1", "src/auth.ts")
        .expect("claim should release");

    let reservation = store
        .active_reservation("w1", "src/auth.ts")
        .expect("reservation lookup should succeed")
        .expect("first waiter should be reserved");
    assert_eq!(reservation.wait_id, first.wait_id);
    assert_eq!(reservation.session_id, "s2");
    assert_eq!(reservation.relative_path, "src/auth.ts");
    assert_eq!(
        store
            .waiter_status(&second.wait_id)
            .expect("second waiter should load"),
        Some("queued".to_string())
    );
}

#[test]
fn reservation_request_rejects_normalized_empty_relative_path() {
    for path in ["./", "../", "/", "a/.."] {
        let store = Store::open_in_memory().expect("in-memory store should open");

        let error = store
            .enqueue_reservation_request(ReservationRequestInput {
                request_id: "request-1",
                session_id: "s1",
                workspace_id: "w1",
                relative_path: path,
                action: "write_file",
                purpose: "Reserve path before writing.",
                blocking_session_id: None,
            })
            .expect_err("normalized-empty reservation request should reject");

        assert!(matches!(error, StoreError::MissingScope));
        assert!(
            store
                .waiter_by_request_id("request-1")
                .expect("waiter lookup should load")
                .is_none()
        );
    }
}

#[test]
fn reservation_request_id_is_idempotent_for_wait_queue_records() {
    let store = Store::open_in_memory().expect("in-memory store should open");

    let first = store
        .enqueue_reservation_request(ReservationRequestInput {
            request_id: "request-1",
            session_id: "s2",
            workspace_id: "w1",
            relative_path: "src/auth.ts",
            action: "write_file",
            purpose: "Fix auth validation behavior.",
            blocking_session_id: Some("s1"),
        })
        .expect("reservation request should enqueue");
    let repeated = store
        .enqueue_reservation_request(ReservationRequestInput {
            request_id: "request-1",
            session_id: "s2",
            workspace_id: "w1",
            relative_path: "src/auth.ts",
            action: "write_file",
            purpose: "A different retry purpose should not replace the original.",
            blocking_session_id: Some("s1"),
        })
        .expect("reservation request retry should load existing waiter");

    assert_eq!(repeated.wait_id, first.wait_id);
    assert_eq!(repeated.status, "queued");
    assert_eq!(repeated.purpose, "Fix auth validation behavior.");
    assert_eq!(
        store
            .queue_position(&repeated.wait_id)
            .expect("queue position should load"),
        Some(1)
    );
}

#[test]
fn expired_reservation_request_retry_requeues_same_waiter_with_original_fifo_position() {
    let store = Store::open_in_memory().expect("in-memory store should open");

    let first = store
        .enqueue_reservation_request(ReservationRequestInput {
            request_id: "request-1",
            session_id: "s2",
            workspace_id: "w1",
            relative_path: "src/auth.ts",
            action: "write_file",
            purpose: "Fix auth validation behavior.",
            blocking_session_id: Some("s1"),
        })
        .expect("first request should enqueue");
    let second = store
        .enqueue_reservation_request(ReservationRequestInput {
            request_id: "request-2",
            session_id: "s3",
            workspace_id: "w1",
            relative_path: "src/auth.ts",
            action: "write_file",
            purpose: "Update session handling.",
            blocking_session_id: Some("s1"),
        })
        .expect("second request should enqueue");
    store
        .promote_next_waiter("w1", "src/auth.ts")
        .expect("first request should reserve");
    store
        .expire_reservation(&first.wait_id)
        .expect("first reservation should expire");
    store
        .promote_next_waiter("w1", "src/auth.ts")
        .expect("second request should reserve");

    let retried = store
        .enqueue_reservation_request(ReservationRequestInput {
            request_id: "request-1",
            session_id: "s2",
            workspace_id: "w1",
            relative_path: "src/auth.ts",
            action: "write_file",
            purpose: "Retry should preserve the original purpose.",
            blocking_session_id: Some("s3"),
        })
        .expect("expired request retry should requeue");
    let third = store
        .enqueue_reservation_request(ReservationRequestInput {
            request_id: "request-3",
            session_id: "s4",
            workspace_id: "w1",
            relative_path: "src/auth.ts",
            action: "write_file",
            purpose: "Update auth docs.",
            blocking_session_id: Some("s3"),
        })
        .expect("third request should enqueue");

    assert_eq!(retried.wait_id, first.wait_id);
    assert_eq!(retried.status, "queued");
    assert_eq!(retried.purpose, "Fix auth validation behavior.");
    assert_eq!(
        store
            .queue_position(&retried.wait_id)
            .expect("retried queue position should load"),
        Some(1)
    );
    assert_eq!(
        store
            .queue_position(&third.wait_id)
            .expect("third queue position should load"),
        Some(2)
    );

    store
        .expire_reservation(&second.wait_id)
        .expect("second reservation should expire");
    let next = store
        .promote_next_waiter("w1", "src/auth.ts")
        .expect("next waiter should promote")
        .expect("retried waiter should reserve");
    assert_eq!(next.wait_id, first.wait_id);
    assert_eq!(next.session_id, "s2");
}

#[test]
fn canceling_reserved_reservation_request_promotes_next_waiter() {
    let store = Store::open_in_memory().expect("in-memory store should open");
    let first = store
        .enqueue_reservation_request(ReservationRequestInput {
            request_id: "request-1",
            session_id: "s2",
            workspace_id: "w1",
            relative_path: "src/auth.ts",
            action: "write_file",
            purpose: "Fix auth validation behavior.",
            blocking_session_id: Some("s1"),
        })
        .expect("first request should enqueue");
    let second = store
        .enqueue_reservation_request(ReservationRequestInput {
            request_id: "request-2",
            session_id: "s3",
            workspace_id: "w1",
            relative_path: "src/auth.ts",
            action: "write_file",
            purpose: "Update session handling.",
            blocking_session_id: Some("s1"),
        })
        .expect("second request should enqueue");
    store
        .promote_next_waiter("w1", "src/auth.ts")
        .expect("first waiter should promote");

    let canceled = store
        .cancel_reservation_request("request-1", "s2", "w1")
        .expect("first request should cancel");

    assert_eq!(canceled.wait_id, first.wait_id);
    assert_eq!(canceled.status, "canceled");
    let reservation = store
        .active_reservation("w1", "src/auth.ts")
        .expect("reservation lookup should succeed")
        .expect("second waiter should be reserved");
    assert_eq!(reservation.wait_id, second.wait_id);
    assert_eq!(reservation.session_id, "s3");
}

#[test]
fn finalizing_session_cancels_queued_and_reserved_waiters_and_promotes_next() {
    let store = Store::open_in_memory().expect("in-memory store should open");
    let reserved = store
        .enqueue_reservation_request(ReservationRequestInput {
            request_id: "request-reserved",
            session_id: "s2",
            workspace_id: "w1",
            relative_path: "src/auth.ts",
            action: "write_file",
            purpose: "Fix auth validation behavior.",
            blocking_session_id: Some("s1"),
        })
        .expect("reserved session request should enqueue");
    let next = store
        .enqueue_reservation_request(ReservationRequestInput {
            request_id: "request-next",
            session_id: "s3",
            workspace_id: "w1",
            relative_path: "src/auth.ts",
            action: "write_file",
            purpose: "Update session handling.",
            blocking_session_id: Some("s1"),
        })
        .expect("next request should enqueue");
    let queued = store
        .enqueue_reservation_request(ReservationRequestInput {
            request_id: "request-queued",
            session_id: "s2",
            workspace_id: "w1",
            relative_path: "docs/notes.md",
            action: "write_file",
            purpose: "Update docs.",
            blocking_session_id: Some("s1"),
        })
        .expect("queued session request should enqueue");
    store
        .promote_next_waiter("w1", "src/auth.ts")
        .expect("reserved waiter should promote");

    store
        .finalize_session_activity("s2", "w1")
        .expect("session finalize should cancel waits");

    assert_eq!(
        store
            .waiter_by_request_id("request-reserved")
            .expect("reserved waiter should load")
            .expect("reserved waiter should exist")
            .status,
        "canceled"
    );
    assert_eq!(
        store
            .waiter_by_request_id("request-queued")
            .expect("queued waiter should load")
            .expect("queued waiter should exist")
            .status,
        "canceled"
    );
    let promoted = store
        .active_reservation("w1", "src/auth.ts")
        .expect("reservation lookup should succeed")
        .expect("next waiter should be reserved");
    assert_eq!(promoted.wait_id, next.wait_id);
    assert_eq!(promoted.session_id, "s3");
    assert_ne!(reserved.wait_id, promoted.wait_id);
    assert_ne!(queued.wait_id, promoted.wait_id);
}

#[test]
fn cancel_reservation_request_rolls_back_when_next_reservation_notification_fails() {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system time should be after unix epoch")
        .as_nanos();
    let temp_root = std::env::temp_dir().join(format!(
        "stateful-store-cancel-rollback-{}-{unique}",
        std::process::id()
    ));
    fs::create_dir_all(&temp_root).expect("temp root should be creatable");
    let db_path = temp_root.join("state.db");
    let store = Store::open(&db_path).expect("file store should open");

    let first = store
        .enqueue_reservation_request(ReservationRequestInput {
            request_id: "request-1",
            session_id: "s2",
            workspace_id: "w1",
            relative_path: "src/auth.ts",
            action: "write_file",
            purpose: "Fix auth validation behavior.",
            blocking_session_id: Some("s1"),
        })
        .expect("first request should enqueue");
    let second = store
        .enqueue_reservation_request(ReservationRequestInput {
            request_id: "request-2",
            session_id: "s3",
            workspace_id: "w1",
            relative_path: "src/auth.ts",
            action: "write_file",
            purpose: "Update session handling.",
            blocking_session_id: Some("s1"),
        })
        .expect("second request should enqueue");
    store
        .promote_next_waiter("w1", "src/auth.ts")
        .expect("first waiter should promote");

    let trigger_conn = Connection::open(&db_path).expect("trigger connection should open");
    trigger_conn
        .execute_batch(
            "CREATE TRIGGER fail_cancel_notification
             BEFORE INSERT ON notifications
             BEGIN
                 SELECT RAISE(ABORT, 'simulated cancel notification failure');
             END;",
        )
        .expect("failure trigger should install");

    let error = store
        .cancel_reservation_request("request-1", "s2", "w1")
        .expect_err("cancel should surface notification failure");
    assert!(
        error
            .to_string()
            .contains("simulated cancel notification failure"),
        "error should report trigger failure: {error}"
    );

    assert_eq!(
        store
            .waiter_status(&first.wait_id)
            .expect("first waiter status should load")
            .as_deref(),
        Some("reserved")
    );
    assert_eq!(
        store
            .waiter_status(&second.wait_id)
            .expect("second waiter status should load")
            .as_deref(),
        Some("queued")
    );
    let reservation = store
        .active_reservation("w1", "src/auth.ts")
        .expect("reservation lookup should succeed")
        .expect("first reservation should remain active");
    assert_eq!(reservation.wait_id, first.wait_id);

    drop(trigger_conn);
    fs::remove_dir_all(&temp_root).expect("temp root should be removable");
}

#[test]
fn promote_next_waiter_for_path_rolls_back_when_notification_fails() {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system time should be after unix epoch")
        .as_nanos();
    let temp_root = std::env::temp_dir().join(format!(
        "stateful-store-promote-rollback-{}-{unique}",
        std::process::id()
    ));
    fs::create_dir_all(&temp_root).expect("temp root should be creatable");
    let db_path = temp_root.join("state.db");
    let store = Store::open(&db_path).expect("file store should open");

    let wait = store
        .enqueue_waiter(
            "s2",
            "w1",
            "src/auth.ts",
            "write_file",
            "Queue requested file write after blocker clears.",
            Some("s1"),
        )
        .expect("waiter should enqueue");

    let trigger_conn = Connection::open(&db_path).expect("trigger connection should open");
    trigger_conn
        .execute_batch(
            "CREATE TRIGGER fail_promote_notification
             BEFORE INSERT ON notifications
             BEGIN
                 SELECT RAISE(ABORT, 'simulated promote notification failure');
             END;",
        )
        .expect("failure trigger should install");

    let error = store
        .promote_next_waiter_for_path("w1", "src/auth.ts")
        .expect_err("promotion should surface notification failure");
    assert!(
        error
            .to_string()
            .contains("simulated promote notification failure"),
        "error should report trigger failure: {error}"
    );

    assert_eq!(
        store
            .waiter_status(&wait.wait_id)
            .expect("waiter status should load")
            .as_deref(),
        Some("queued")
    );
    assert!(
        store
            .active_reservation("w1", "src/auth.ts")
            .expect("reservation lookup should succeed")
            .is_none(),
        "failed promotion should not leave a reservation"
    );

    drop(trigger_conn);
    fs::remove_dir_all(&temp_root).expect("temp root should be removable");
}

#[test]
fn released_child_lease_promotes_directory_waiter_to_reservation() {
    let store = Store::open_in_memory().expect("in-memory store should open");

    acquire_test_lease(&store, "s1", "w1", "target/out.txt");
    let wait = store
        .enqueue_waiter(
            "s2",
            "w1",
            "target",
            "write_directory",
            "Queue requested directory write after blocker clears.",
            Some("s1"),
        )
        .expect("directory waiter should enqueue");

    store
        .release_claim("s1", "w1", "target/out.txt")
        .expect("child claim should release");

    let reservation = store
        .active_reservation("w1", "target")
        .expect("reservation lookup should succeed")
        .expect("directory waiter should be reserved");
    assert_eq!(reservation.wait_id, wait.wait_id);
    assert_eq!(reservation.session_id, "s2");
    assert_eq!(reservation.relative_path, "target");

    let notifications = store
        .pending_notifications("s2", "w1")
        .expect("notifications should load");
    assert_eq!(notifications.len(), 1);
    assert_eq!(notifications[0].kind, "reservation_granted");
    assert_eq!(notifications[0].payload["relative_path"], "target");
    assert_eq!(
        notifications[0].payload["purpose"],
        "Queue requested directory write after blocker clears."
    );
}

#[test]
fn release_claim_rolls_back_when_reservation_notification_fails() {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system time should be after unix epoch")
        .as_nanos();
    let temp_root = std::env::temp_dir().join(format!(
        "stateful-store-release-rollback-{}-{unique}",
        std::process::id()
    ));
    fs::create_dir_all(&temp_root).expect("temp root should be creatable");
    let db_path = temp_root.join("state.db");
    let store = Store::open(&db_path).expect("file store should open");

    acquire_test_lease(&store, "s1", "w1", "target/out.txt");
    let wait = store
        .enqueue_waiter(
            "s2",
            "w1",
            "target/out.txt",
            "write_file",
            "Queue requested file write after blocker clears.",
            Some("s1"),
        )
        .expect("waiter should enqueue");

    let trigger_conn = Connection::open(&db_path).expect("trigger connection should open");
    trigger_conn
        .execute_batch(
            "CREATE TRIGGER fail_reservation_notification
             BEFORE INSERT ON notifications
             BEGIN
                 SELECT RAISE(ABORT, 'simulated notification failure');
             END;",
        )
        .expect("failure trigger should install");

    let error = store
        .release_claim("s1", "w1", "target/out.txt")
        .expect_err("release should surface notification failure");
    assert!(
        error.to_string().contains("simulated notification failure"),
        "error should report trigger failure: {error}"
    );

    let owner = store
        .active_claim_conflict_owner_for_path("w1", "target/out.txt", "s2")
        .expect("active claim owner lookup should succeed");
    assert_eq!(owner.as_deref(), Some("s1"));
    assert_eq!(
        store
            .waiter_status(&wait.wait_id)
            .expect("waiter status should load")
            .as_deref(),
        Some("queued")
    );
    assert!(
        store
            .active_reservation("w1", "target/out.txt")
            .expect("active reservation lookup should succeed")
            .is_none(),
        "failed release should not leave a promoted reservation"
    );

    drop(trigger_conn);
    fs::remove_dir_all(&temp_root).expect("temp root should be removable");
}

#[test]
fn release_session_claims_rolls_back_when_reservation_notification_fails() {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system time should be after unix epoch")
        .as_nanos();
    let temp_root = std::env::temp_dir().join(format!(
        "stateful-store-session-release-rollback-{}-{unique}",
        std::process::id()
    ));
    fs::create_dir_all(&temp_root).expect("temp root should be creatable");
    let db_path = temp_root.join("state.db");
    let store = Store::open(&db_path).expect("file store should open");

    acquire_test_lease(&store, "s1", "w1", "target/out.txt");
    let wait = store
        .enqueue_waiter(
            "s2",
            "w1",
            "target/out.txt",
            "write_file",
            "Queue requested file write after blocker clears.",
            Some("s1"),
        )
        .expect("waiter should enqueue");

    let trigger_conn = Connection::open(&db_path).expect("trigger connection should open");
    trigger_conn
        .execute_batch(
            "CREATE TRIGGER fail_session_release_notification
             BEFORE INSERT ON notifications
             BEGIN
                 SELECT RAISE(ABORT, 'simulated session release notification failure');
             END;",
        )
        .expect("failure trigger should install");

    let error = store
        .release_session_claims("s1", "w1")
        .expect_err("session release should surface notification failure");
    assert!(
        error
            .to_string()
            .contains("simulated session release notification failure"),
        "error should report trigger failure: {error}"
    );

    let owner = store
        .active_claim_conflict_owner_for_path("w1", "target/out.txt", "s2")
        .expect("active claim owner lookup should succeed");
    assert_eq!(owner.as_deref(), Some("s1"));
    assert_eq!(
        store
            .waiter_status(&wait.wait_id)
            .expect("waiter status should load")
            .as_deref(),
        Some("queued")
    );
    assert!(
        store
            .active_reservation("w1", "target/out.txt")
            .expect("active reservation lookup should succeed")
            .is_none(),
        "failed session release should not leave a promoted reservation"
    );

    drop(trigger_conn);
    fs::remove_dir_all(&temp_root).expect("temp root should be removable");
}

#[test]
fn released_child_lease_does_not_promote_directory_waiter_while_another_child_lease_is_active() {
    let store = Store::open_in_memory().expect("in-memory store should open");

    acquire_test_lease(&store, "s1", "w1", "target/out.txt");
    acquire_test_lease(&store, "s3", "w1", "target/other.txt");
    let wait = store
        .enqueue_waiter(
            "s2",
            "w1",
            "target",
            "write_directory",
            "Queue requested directory write after blocker clears.",
            Some("s1"),
        )
        .expect("directory waiter should enqueue");

    store
        .release_claim("s1", "w1", "target/out.txt")
        .expect("first child claim should release");

    assert!(
        store
            .active_reservation("w1", "target")
            .expect("reservation lookup should succeed")
            .is_none()
    );
    assert_eq!(
        store
            .waiter_status(&wait.wait_id)
            .expect("waiter status should load"),
        Some("queued".to_string())
    );

    store
        .release_claim("s3", "w1", "target/other.txt")
        .expect("second child claim should release");

    let reservation = store
        .active_reservation("w1", "target")
        .expect("reservation lookup should succeed")
        .expect("directory waiter should be reserved after all child claims release");
    assert_eq!(reservation.wait_id, wait.wait_id);
}

#[test]
fn released_child_lease_promotes_earlier_directory_waiter_before_later_child_waiter() {
    let store = Store::open_in_memory().expect("in-memory store should open");

    acquire_test_lease(&store, "s1", "w1", "target/out.txt");
    let directory_wait = store
        .enqueue_waiter(
            "s2",
            "w1",
            "target",
            "write_directory",
            "Queue requested directory write after blocker clears.",
            Some("s1"),
        )
        .expect("directory waiter should enqueue");
    let child_wait = store
        .enqueue_waiter(
            "s3",
            "w1",
            "target/out.txt",
            "write_file",
            "Queue requested file write after blocker clears.",
            Some("s1"),
        )
        .expect("child waiter should enqueue");

    store
        .release_claim("s1", "w1", "target/out.txt")
        .expect("child claim should release");

    let reservation = store
        .active_reservation("w1", "target")
        .expect("directory reservation lookup should succeed")
        .expect("directory waiter should be reserved first");
    assert_eq!(reservation.wait_id, directory_wait.wait_id);
    assert_eq!(
        store
            .waiter_status(&child_wait.wait_id)
            .expect("child waiter status should load"),
        Some("queued".to_string())
    );
}

#[test]
fn released_directory_lease_promotes_child_file_waiter_to_reservation() {
    let store = Store::open_in_memory().expect("in-memory store should open");

    acquire_test_lease(&store, "s1", "w1", "target/");
    let wait = store
        .enqueue_waiter(
            "s2",
            "w1",
            "target/out.txt",
            "write_file",
            "Queue requested file write after blocker clears.",
            Some("s1"),
        )
        .expect("child file waiter should enqueue");

    store
        .release_claim("s1", "w1", "target/")
        .expect("directory claim should release");

    let reservation = store
        .active_reservation("w1", "target/out.txt")
        .expect("reservation lookup should succeed")
        .expect("child file waiter should be reserved");
    assert_eq!(reservation.wait_id, wait.wait_id);
    assert_eq!(reservation.session_id, "s2");

    let notifications = store
        .pending_notifications("s2", "w1")
        .expect("notifications should load");
    assert_eq!(notifications.len(), 1);
    assert_eq!(notifications[0].payload["relative_path"], "target/out.txt");
}

#[test]
fn released_directory_lease_promotes_all_non_conflicting_child_file_waiters() {
    let store = Store::open_in_memory().expect("in-memory store should open");

    acquire_test_lease(&store, "s1", "w1", "target/");
    let first = store
        .enqueue_waiter(
            "s2",
            "w1",
            "target/a.txt",
            "write_file",
            "Queue first file write after blocker clears.",
            Some("s1"),
        )
        .expect("first child file waiter should enqueue");
    let second = store
        .enqueue_waiter(
            "s3",
            "w1",
            "target/b.txt",
            "write_file",
            "Queue second file write after blocker clears.",
            Some("s1"),
        )
        .expect("second child file waiter should enqueue");

    store
        .release_claim("s1", "w1", "target/")
        .expect("directory claim should release");

    let first_reservation = store
        .active_reservation("w1", "target/a.txt")
        .expect("first reservation lookup should succeed")
        .expect("first child file waiter should be reserved");
    assert_eq!(first_reservation.wait_id, first.wait_id);
    let second_reservation = store
        .active_reservation("w1", "target/b.txt")
        .expect("second reservation lookup should succeed")
        .expect("second child file waiter should be reserved");
    assert_eq!(second_reservation.wait_id, second.wait_id);
}

#[test]
fn released_directory_lease_keeps_same_session_conflicting_waiter_queued() {
    let store = Store::open_in_memory().expect("in-memory store should open");

    acquire_test_lease(&store, "s1", "w1", "target/");
    let first = store
        .enqueue_reservation_request(ReservationRequestInput {
            request_id: "request-1",
            session_id: "s2",
            workspace_id: "w1",
            relative_path: "target/a.txt",
            action: "write_file",
            purpose: "Queue first file write after blocker clears.",
            blocking_session_id: Some("s1"),
        })
        .expect("first child file waiter should enqueue");
    let second = store
        .enqueue_reservation_request(ReservationRequestInput {
            request_id: "request-2",
            session_id: "s2",
            workspace_id: "w1",
            relative_path: "target/a.txt",
            action: "write_file",
            purpose: "Queue second file write after blocker clears.",
            blocking_session_id: Some("s1"),
        })
        .expect("second child file waiter should enqueue");

    store
        .release_claim("s1", "w1", "target/")
        .expect("directory claim should release");

    let reservation = store
        .active_reservation("w1", "target/a.txt")
        .expect("reservation lookup should succeed")
        .expect("first waiter should be reserved");
    assert_eq!(reservation.wait_id, first.wait_id);
    assert_eq!(
        store
            .waiter_status(&second.wait_id)
            .expect("second waiter status should load")
            .as_deref(),
        Some("queued")
    );
}

#[test]
fn live_current_state_rolls_back_unblocked_promotion_when_notification_fails() {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system time should be after unix epoch")
        .as_nanos();
    let temp_root = std::env::temp_dir().join(format!(
        "stateful-store-live-current-promotion-rollback-{}-{unique}",
        std::process::id()
    ));
    fs::create_dir_all(&temp_root).expect("temp root should be creatable");
    let db_path = temp_root.join("state.db");
    let store = Store::open(&db_path).expect("file store should open");
    let wait = store
        .enqueue_waiter(
            "s2",
            "w1",
            "target/a.txt",
            "write_file",
            "Queue file write after a blocker that already cleared.",
            Some("s1"),
        )
        .expect("waiter should enqueue");

    let trigger_conn = Connection::open(&db_path).expect("trigger connection should open");
    trigger_conn
        .execute_batch(
            "CREATE TRIGGER fail_current_notification
             BEFORE INSERT ON notifications
             BEGIN
                 SELECT RAISE(ABORT, 'simulated current notification failure');
             END;",
        )
        .expect("failure trigger should install");

    let error = store
        .live_current_state(Some("target/a.txt"))
        .expect_err("current-state promotion should surface notification failure");
    assert!(
        error
            .to_string()
            .contains("simulated current notification failure"),
        "error should report trigger failure: {error}"
    );
    assert_eq!(
        store
            .waiter_status(&wait.wait_id)
            .expect("waiter status should load")
            .as_deref(),
        Some("queued")
    );
    assert!(
        store
            .active_reservation("w1", "target/a.txt")
            .expect("reservation lookup should succeed")
            .is_none(),
        "failed live current promotion should not leave a reservation"
    );

    drop(trigger_conn);
    fs::remove_dir_all(&temp_root).expect("temp root should be removable");
}

#[test]
fn live_current_state_promotes_queued_waiter_without_active_conflict() {
    let store = Store::open_in_memory().expect("in-memory store should open");
    let wait = store
        .enqueue_waiter(
            "s2",
            "w1",
            "target/a.txt",
            "write_file",
            "Queue file write after a blocker that already cleared.",
            Some("s1"),
        )
        .expect("waiter should enqueue");

    let live = store
        .live_current_state(Some("target/a.txt"))
        .expect("live current state should load");

    assert!(
        live.items
            .iter()
            .all(|item| item.kind != CurrentItemKind::WaitQueue)
    );
    let reservation = store
        .active_reservation("w1", "target/a.txt")
        .expect("reservation lookup should succeed")
        .expect("unblocked waiter should be reserved");
    assert_eq!(reservation.wait_id, wait.wait_id);
}

#[test]
fn released_directory_lease_promotes_child_directory_waiter_to_reservation() {
    let store = Store::open_in_memory().expect("in-memory store should open");

    acquire_test_lease(&store, "s1", "w1", "target/");
    let wait = store
        .enqueue_waiter(
            "s2",
            "w1",
            "target/debug",
            "write_directory",
            "Queue requested directory write after blocker clears.",
            Some("s1"),
        )
        .expect("child directory waiter should enqueue");

    store
        .release_claim("s1", "w1", "target/")
        .expect("directory claim should release");

    let reservation = store
        .active_reservation("w1", "target/debug")
        .expect("reservation lookup should succeed")
        .expect("child directory waiter should be reserved");
    assert_eq!(reservation.wait_id, wait.wait_id);
    assert_eq!(reservation.session_id, "s2");
    assert_eq!(reservation.action, "write_directory");
}

#[test]
fn reservation_promotion_creates_pending_notification_for_waiter() {
    let store = Store::open_in_memory().expect("in-memory store should open");

    store
        .enqueue_waiter(
            "s2",
            "w1",
            "src/auth.ts",
            "write_file",
            "Queue requested file write after blocker clears.",
            Some("s1"),
        )
        .expect("waiter should enqueue");
    store
        .promote_next_waiter("w1", "src/auth.ts")
        .expect("waiter should promote");

    let notifications = store
        .pending_notifications("s2", "w1")
        .expect("notifications should load");
    assert_eq!(notifications.len(), 1);
    assert_eq!(notifications[0].target_session_id, "s2");
    assert_eq!(notifications[0].workspace_id, "w1");
    assert_eq!(notifications[0].kind, "reservation_granted");
    assert_eq!(notifications[0].payload["relative_path"], "src/auth.ts");
    assert_eq!(
        notifications[0].payload["purpose"],
        "Queue requested file write after blocker clears."
    );
}

#[test]
fn pending_notifications_are_scoped_to_workspace() {
    let store = Store::open_in_memory().expect("in-memory store should open");

    for workspace_id in ["w1", "w2"] {
        store
            .enqueue_waiter(
                "s2",
                workspace_id,
                "src/auth.ts",
                "write_file",
                "Queue requested file write after blocker clears.",
                Some("s1"),
            )
            .expect("waiter should enqueue");
        store
            .promote_next_waiter(workspace_id, "src/auth.ts")
            .expect("waiter should promote");
    }

    let w1_notifications = store
        .pending_notifications("s2", "w1")
        .expect("w1 notifications should load");
    let w2_notifications = store
        .pending_notifications("s2", "w2")
        .expect("w2 notifications should load");

    assert_eq!(w1_notifications.len(), 1);
    assert_eq!(w1_notifications[0].workspace_id, "w1");
    assert_eq!(w2_notifications.len(), 1);
    assert_eq!(w2_notifications[0].workspace_id, "w2");
}

#[test]
fn pending_notifications_are_delivered_once() {
    let store = Store::open_in_memory().expect("in-memory store should open");

    store
        .enqueue_waiter(
            "s2",
            "w1",
            "src/auth.ts",
            "write_file",
            "Queue requested file write after blocker clears.",
            Some("s1"),
        )
        .expect("waiter should enqueue");
    store
        .promote_next_waiter("w1", "src/auth.ts")
        .expect("waiter should promote");

    let first_poll = store
        .pending_notifications("s2", "w1")
        .expect("first poll should load notification");
    let second_poll = store
        .pending_notifications("s2", "w1")
        .expect("second poll should not redeliver notification");

    assert_eq!(first_poll.len(), 1);
    assert!(
        second_poll.is_empty(),
        "delivered notifications should not be returned again"
    );
}

#[test]
fn reservation_blocks_other_sessions_until_claimed_or_expired() {
    let store = Store::open_in_memory().expect("in-memory store should open");

    let wait = store
        .enqueue_waiter(
            "s2",
            "w1",
            "src/auth.ts",
            "write_file",
            "Queue requested file write after blocker clears.",
            Some("s1"),
        )
        .expect("waiter should enqueue");
    store
        .promote_next_waiter("w1", "src/auth.ts")
        .expect("next waiter should promote");

    assert_eq!(
        store
            .active_reservation_owner("w1", "src/auth.ts")
            .expect("reservation owner should load"),
        Some("s2".to_string())
    );
    assert!(
        store
            .claim_reservation(&wait.wait_id, "s3")
            .expect_err("non-owner should not claim")
            .to_string()
            .contains("reservation owner mismatch")
    );
    store
        .claim_reservation(&wait.wait_id, "s2")
        .expect("owner should claim reservation");
    assert_eq!(
        store
            .waiter_status(&wait.wait_id)
            .expect("waiter status should load"),
        Some("claimed".to_string())
    );
}

#[test]
fn active_waiter_by_session_matches_queued_and_reserved_standing() {
    let store = Store::open_in_memory().expect("in-memory store should open");

    let file_wait = store
        .enqueue_waiter(
            "s2",
            "w1",
            "target/out.txt",
            "write_file",
            "Queue requested file write after blocker clears.",
            Some("s1"),
        )
        .expect("file waiter should enqueue");
    assert_eq!(
        store
            .active_waiter_for_path_by_session("w1", "target/out.txt", "s2")
            .expect("path waiter should load")
            .expect("queued file waiter should match")
            .wait_id,
        file_wait.wait_id
    );
    assert_eq!(
        store
            .active_waiter_for_directory_by_session("w1", "target", "s2")
            .expect("directory waiter should load")
            .expect("queued child file waiter should match parent directory")
            .wait_id,
        file_wait.wait_id
    );
    assert!(
        store
            .active_waiter_for_path_by_session("w1", "target/out.txt", "s1")
            .expect("other-session waiter lookup should load")
            .is_none()
    );

    store
        .promote_next_waiter("w1", "target/out.txt")
        .expect("file waiter should promote");
    let reserved = store
        .active_waiter_for_path_by_session("w1", "target/out.txt", "s2")
        .expect("reserved waiter should load")
        .expect("reserved file waiter should match");
    assert_eq!(reserved.wait_id, file_wait.wait_id);
    assert_eq!(reserved.status, "reserved");

    let directory_wait = store
        .enqueue_waiter(
            "s3",
            "w1",
            "target",
            "write_directory",
            "Queue requested directory write after blocker clears.",
            Some("s1"),
        )
        .expect("directory waiter should enqueue");
    assert_eq!(
        store
            .active_waiter_for_path_by_session("w1", "target/out.txt", "s3")
            .expect("ancestor directory waiter should load")
            .expect("queued ancestor directory waiter should match child path")
            .wait_id,
        directory_wait.wait_id
    );
}

#[test]
fn active_reservation_conflict_for_directory_matches_subtree_paths() {
    let store = Store::open_in_memory().expect("in-memory store should open");

    store
        .enqueue_waiter(
            "s2",
            "w1",
            "target/out.txt",
            "write_file",
            "Queue requested file write after blocker clears.",
            Some("s1"),
        )
        .expect("waiter should enqueue");
    store
        .promote_next_waiter("w1", "target/out.txt")
        .expect("waiter should promote");

    let conflict = store
        .active_reservation_conflict_for_directory("w1", "target/", "s1")
        .expect("directory reservation conflict should load")
        .expect("reserved subtree path should conflict");
    assert_eq!(conflict.session_id, "s2");
    assert_eq!(conflict.relative_path, "target/out.txt");
    assert!(
        store
            .active_reservation_conflict_for_directory("w1", "target/", "s2")
            .expect("same-session reservation should not conflict")
            .is_none()
    );
    assert!(
        store
            .active_reservation_conflict_for_directory("w1", "target-other/", "s1")
            .expect("sibling directory should not conflict")
            .is_none()
    );
}

#[test]
fn directory_reservation_same_normalized_path_does_not_conflict_with_file_path() {
    let store = Store::open_in_memory().expect("in-memory store should open");

    store
        .enqueue_waiter(
            "s2",
            "w1",
            "target",
            "write_directory",
            "Queue requested directory write after blocker clears.",
            Some("s3"),
        )
        .expect("directory waiter should enqueue");
    store
        .promote_next_waiter("w1", "target")
        .expect("directory waiter should promote");

    assert_eq!(
        store
            .active_reservation_conflict_for_path("w1", "target", "s1")
            .expect("path reservation conflict should load"),
        None
    );
    assert!(
        store
            .active_reservation_for_path_by_session("w1", "target", "s2")
            .expect("same-session path reservation should load")
            .is_none()
    );
    assert!(
        store
            .active_reservation_conflict_for_directory("w1", "target/", "s1")
            .expect("directory reservation conflict should load")
            .is_some()
    );
}

#[test]
fn active_reservation_conflict_for_directory_matches_ancestor_directory_paths() {
    let store = Store::open_in_memory().expect("in-memory store should open");
    store
        .enqueue_waiter(
            "s2",
            "w1",
            "target",
            "write_directory",
            "Queue requested directory write after blocker clears.",
            Some("s3"),
        )
        .expect("ancestor directory waiter should enqueue");
    store
        .promote_next_waiter("w1", "target")
        .expect("ancestor directory waiter should promote");

    let conflict = store
        .active_reservation_conflict_for_directory("w1", "target/debug/", "s1")
        .expect("ancestor directory reservation conflict should load");

    assert_eq!(conflict.expect("conflict should exist").session_id, "s2");
}

#[test]
fn active_reservation_conflict_for_path_matches_ancestor_directory_paths() {
    let store = Store::open_in_memory().expect("in-memory store should open");

    store
        .enqueue_waiter(
            "s2",
            "w1",
            "target",
            "write_directory",
            "Queue requested directory write after blocker clears.",
            Some("s1"),
        )
        .expect("directory waiter should enqueue");
    store
        .promote_next_waiter("w1", "target")
        .expect("waiter should promote");

    let conflict = store
        .active_reservation_conflict_for_path("w1", "target/out.txt", "s1")
        .expect("path reservation conflict should load")
        .expect("reserved ancestor directory should conflict");
    assert_eq!(conflict.session_id, "s2");
    assert_eq!(conflict.relative_path, "target");
    assert!(
        store
            .active_reservation_conflict_for_path("w1", "target/out.txt", "s2")
            .expect("same-session reservation should not conflict")
            .is_none()
    );
    assert!(
        store
            .active_reservation_conflict_for_path("w1", "target-other/out.txt", "s1")
            .expect("sibling directory should not conflict")
            .is_none()
    );
}

#[test]
fn expired_reservation_promotes_next_waiter() {
    let store = Store::open_in_memory().expect("in-memory store should open");

    let first = store
        .enqueue_waiter(
            "s2",
            "w1",
            "src/auth.ts",
            "write_file",
            "Queue requested file write after blocker clears.",
            Some("s1"),
        )
        .expect("first waiter should enqueue");
    let second = store
        .enqueue_waiter(
            "s3",
            "w1",
            "src/auth.ts",
            "write_file",
            "Queue requested file write after blocker clears.",
            Some("s1"),
        )
        .expect("second waiter should enqueue");
    store
        .promote_next_waiter("w1", "src/auth.ts")
        .expect("first waiter should promote");

    store
        .expire_reservation(&first.wait_id)
        .expect("first reservation should expire");
    store
        .promote_next_waiter("w1", "src/auth.ts")
        .expect("second waiter should promote");

    assert_eq!(
        store
            .waiter_status(&first.wait_id)
            .expect("first waiter status should load"),
        Some("expired".to_string())
    );
    let reservation = store
        .active_reservation("w1", "src/auth.ts")
        .expect("reservation lookup should succeed")
        .expect("second waiter should be reserved");
    assert_eq!(reservation.wait_id, second.wait_id);
    assert_eq!(reservation.session_id, "s3");
}

#[test]
fn stale_reservation_expiry_promotes_next_waiter() {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock should be after epoch")
        .as_nanos();
    let temp_root = std::env::temp_dir().join(format!(
        "stateful-store-stale-reservation-{}-{unique}",
        std::process::id()
    ));
    fs::create_dir_all(&temp_root).expect("temp root should be creatable");
    let db_path = temp_root.join("state.db");
    let store = Store::open(&db_path).expect("file store should open");

    let first = store
        .enqueue_waiter(
            "s2",
            "w1",
            "src/auth.ts",
            "write_file",
            "Queue requested file write after blocker clears.",
            Some("s1"),
        )
        .expect("first waiter should enqueue");
    let second = store
        .enqueue_waiter(
            "s3",
            "w1",
            "src/auth.ts",
            "write_file",
            "Queue requested file write after blocker clears.",
            Some("s1"),
        )
        .expect("second waiter should enqueue");
    store
        .promote_next_waiter("w1", "src/auth.ts")
        .expect("first waiter should promote");
    drop(store);

    let conn = rusqlite::Connection::open(&db_path).expect("db should reopen");
    conn.execute(
        "UPDATE wait_queue SET reservation_expires_at = '1970-01-01T00:00:00Z'
         WHERE wait_id = ?1",
        [&first.wait_id],
    )
    .expect("reservation should be made stale");
    drop(conn);

    let store = Store::open(&db_path).expect("file store should reopen");
    store
        .expire_stale_at("1970-01-01T00:00:01Z")
        .expect("stale reservation should expire");

    assert_eq!(
        store
            .waiter_status(&first.wait_id)
            .expect("first waiter status should load"),
        Some("expired".to_string())
    );
    let reservation = store
        .active_reservation("w1", "src/auth.ts")
        .expect("reservation lookup should succeed")
        .expect("second waiter should be reserved");
    assert_eq!(reservation.wait_id, second.wait_id);
    assert_eq!(reservation.session_id, "s3");

    fs::remove_dir_all(&temp_root).expect("temp root should be removable");
}

#[test]
fn expire_stale_rolls_back_reservation_expiry_when_next_notification_fails() {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock should be after epoch")
        .as_nanos();
    let temp_root = std::env::temp_dir().join(format!(
        "stateful-store-expire-reservation-rollback-{}-{unique}",
        std::process::id()
    ));
    fs::create_dir_all(&temp_root).expect("temp root should be creatable");
    let db_path = temp_root.join("state.db");
    let store = Store::open(&db_path).expect("file store should open");

    let first = store
        .enqueue_waiter(
            "s2",
            "w1",
            "src/auth.ts",
            "write_file",
            "Queue requested file write after blocker clears.",
            Some("s1"),
        )
        .expect("first waiter should enqueue");
    let second = store
        .enqueue_waiter(
            "s3",
            "w1",
            "src/auth.ts",
            "write_file",
            "Queue requested file write after blocker clears.",
            Some("s1"),
        )
        .expect("second waiter should enqueue");
    store
        .promote_next_waiter("w1", "src/auth.ts")
        .expect("first waiter should promote");
    drop(store);

    let conn = Connection::open(&db_path).expect("db should reopen");
    conn.execute(
        "UPDATE wait_queue SET reservation_expires_at = '1970-01-01T00:00:00Z'
         WHERE wait_id = ?1",
        [&first.wait_id],
    )
    .expect("reservation should be made stale");
    drop(conn);

    let store = Store::open(&db_path).expect("file store should reopen");
    let trigger_conn = Connection::open(&db_path).expect("trigger connection should open");
    trigger_conn
        .execute_batch(
            "CREATE TRIGGER fail_expire_reservation_notification
             BEFORE INSERT ON notifications
             BEGIN
                 SELECT RAISE(ABORT, 'simulated stale reservation notification failure');
             END;",
        )
        .expect("failure trigger should install");

    let error = store
        .expire_stale_at("1970-01-01T00:00:01Z")
        .expect_err("stale expiration should surface notification failure");
    assert!(
        error
            .to_string()
            .contains("simulated stale reservation notification failure"),
        "error should report trigger failure: {error}"
    );

    assert_eq!(
        store
            .waiter_status(&first.wait_id)
            .expect("first waiter status should load")
            .as_deref(),
        Some("reserved")
    );
    assert_eq!(
        store
            .waiter_status(&second.wait_id)
            .expect("second waiter status should load")
            .as_deref(),
        Some("queued")
    );

    drop(trigger_conn);
    fs::remove_dir_all(&temp_root).expect("temp root should be removable");
}

#[test]
fn migrations_create_contract_tables_and_indexes() {
    let store = Store::open_in_memory().expect("in-memory store should open");

    assert!(
        store
            .has_table("schema_migrations")
            .expect("table check should run")
    );
    for index in [
        "idx_events_workspace_created_at",
        "idx_events_session_sequence",
        "idx_sessions_workspace_session",
        "idx_activities_workspace_expires_at",
        "idx_reservations_session_status_expires_at",
        "idx_claims_workspace_absolute_status_expires_at",
        "idx_claims_repo_relative_status_expires_at",
        "idx_conflicts_session_checked_at",
        "idx_reconciliations_session_created_at",
        "idx_outbox_session_sequence_sync_status",
    ] {
        assert!(
            store.has_index(index).expect("index check should run"),
            "missing index {index}"
        );
    }
}
