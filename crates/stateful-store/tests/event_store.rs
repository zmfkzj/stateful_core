use stateful_core::{AuthorizationInput, DecisionKind};
use stateful_store::{Event, OutboxEntry, Store};
use std::fs;
use std::time::{SystemTime, UNIX_EPOCH};

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
fn intent_declared_materializes_active_policy_state() {
    let store = Store::open_in_memory().expect("in-memory store should open");

    store
        .append(Event::intent_declared("s1", "w1", ["src/auth.ts"]))
        .expect("intent event should append");

    let state = store
        .policy_state_for_session("s1")
        .expect("policy state should load");
    let decision =
        stateful_core::authorize_action(&state, AuthorizationInput::write_file("src/auth.ts"));

    assert_eq!(decision.decision, DecisionKind::Allow);
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
    assert_eq!(events[0].event_id, "legacy-event-1");
    assert_eq!(events[0].repo_id, None);
    assert_eq!(events[0].worktree_id, None);
    assert_eq!(events[0].root, None);
    assert_eq!(events[0].branch, None);
    assert_eq!(events[1].repo_id.as_deref(), Some("repo-1"));
    assert_eq!(events[1].worktree_id.as_deref(), Some("worktree-1"));
    assert_eq!(events[1].root.as_deref(), Some("/repo"));
    assert_eq!(events[1].branch.as_deref(), Some("main"));

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
            CREATE TABLE intents (
                intent_id TEXT PRIMARY KEY,
                session_id TEXT NOT NULL,
                workspace_id TEXT NOT NULL,
                scopes_json TEXT NOT NULL,
                status TEXT NOT NULL,
                declared_at TEXT NOT NULL,
                expires_at TEXT
            );

            CREATE TRIGGER fail_intent_materialization
            BEFORE INSERT ON intents
            BEGIN
                SELECT RAISE(FAIL, 'forced intent materialization failure');
            END;
            ",
        )
        .expect("failing intent trigger should be created");
    }

    let store = Store::open(&db_path).expect("store should open");

    let error = store
        .append(Event::intent_declared("s1", "w1", ["src/auth.ts"]))
        .expect_err("intent materialization should fail");
    assert!(
        error
            .to_string()
            .contains("forced intent materialization failure"),
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
fn current_summary_counts_sessions_events_and_active_intents() {
    let store = Store::open_in_memory().expect("in-memory store should open");

    store
        .append(Event::session_registered("s1", "w1"))
        .expect("session event should append");
    store
        .append(Event::intent_declared("s1", "w1", ["src/auth.ts"]))
        .expect("intent event should append");

    let summary = store.current_summary().expect("summary should load");

    assert_eq!(summary.session_count, 1);
    assert_eq!(summary.active_intent_count, 1);
    assert_eq!(summary.event_count, 2);
}

#[test]
fn event_records_return_recent_audit_events() {
    let store = Store::open_in_memory().expect("in-memory store should open");

    store
        .append(Event::session_registered("s1", "w1").with_event_id("event-1"))
        .expect("first event should append");
    store
        .append(Event::intent_declared("s1", "w1", ["src/auth.ts"]).with_event_id("event-2"))
        .expect("second event should append");

    let events = store.recent_events(10).expect("events should load");

    assert_eq!(events.len(), 2);
    assert_eq!(events[0].event_id, "event-1");
    assert_eq!(events[1].event_id, "event-2");
    assert_eq!(events[1].event_type, "IntentDeclared");
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
fn active_lease_owner_uses_normalized_relative_paths() {
    let store = Store::open_in_memory().expect("in-memory store should open");

    store
        .acquire_lease("s1", "w1", "src/./auth.ts")
        .expect("lease should acquire");

    assert_eq!(
        store
            .active_lease_owner("w1", "src/auth.ts")
            .expect("lease owner should load"),
        Some("s1".to_string())
    );
}

#[test]
fn released_lease_promotes_first_waiter_to_reservation() {
    let store = Store::open_in_memory().expect("in-memory store should open");

    store
        .acquire_lease("s1", "w1", "src/auth.ts")
        .expect("lease should acquire");
    let first = store
        .enqueue_waiter("s2", "w1", "src/auth.ts", "write_file", Some("s1"))
        .expect("first waiter should enqueue");
    let second = store
        .enqueue_waiter("s3", "w1", "src/auth.ts", "write_file", Some("s1"))
        .expect("second waiter should enqueue");

    store
        .release_lease("s1", "w1", "src/auth.ts")
        .expect("lease should release");

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
fn reservation_promotion_creates_pending_notification_for_waiter() {
    let store = Store::open_in_memory().expect("in-memory store should open");

    store
        .enqueue_waiter("s2", "w1", "src/auth.ts", "write_file", Some("s1"))
        .expect("waiter should enqueue");
    store
        .promote_next_waiter("w1", "src/auth.ts")
        .expect("waiter should promote");

    let notifications = store
        .pending_notifications("s2")
        .expect("notifications should load");
    assert_eq!(notifications.len(), 1);
    assert_eq!(notifications[0].target_session_id, "s2");
    assert_eq!(notifications[0].workspace_id, "w1");
    assert_eq!(notifications[0].kind, "reservation_granted");
    assert_eq!(notifications[0].payload["relative_path"], "src/auth.ts");
}

#[test]
fn reservation_blocks_other_sessions_until_claimed_or_expired() {
    let store = Store::open_in_memory().expect("in-memory store should open");

    let wait = store
        .enqueue_waiter("s2", "w1", "src/auth.ts", "write_file", Some("s1"))
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
fn expired_reservation_promotes_next_waiter() {
    let store = Store::open_in_memory().expect("in-memory store should open");

    let first = store
        .enqueue_waiter("s2", "w1", "src/auth.ts", "write_file", Some("s1"))
        .expect("first waiter should enqueue");
    let second = store
        .enqueue_waiter("s3", "w1", "src/auth.ts", "write_file", Some("s1"))
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
            &["src/auth.ts".to_string(), "src/billing.ts".to_string()],
            "write_file",
        )
        .expect("request should create");
    let queued_waiter = store
        .enqueue_waiter_for_request("req-1", "s1", "w1", "src/auth.ts", "write_file", Some("s0"))
        .expect("queued waiter should enqueue");
    let reserved_waiter = store
        .enqueue_waiter_for_request(
            "req-1",
            "s1",
            "w1",
            "src/billing.ts",
            "write_file",
            Some("s0"),
        )
        .expect("reserved waiter should enqueue");
    store
        .promote_next_waiter("w1", "src/billing.ts")
        .expect("waiter should reserve");

    store
        .cancel_intent_request("req-1", "s2")
        .expect_err("different session cannot cancel");
    assert_eq!(
        store
            .waiter_status(&queued_waiter.wait_id)
            .expect("queued waiter status should load"),
        Some("queued".to_string())
    );
    assert_eq!(
        store
            .waiter_status(&reserved_waiter.wait_id)
            .expect("reserved waiter status should load"),
        Some("reserved".to_string())
    );

    store
        .cancel_intent_request("req-1", "s1")
        .expect("owner should cancel");
    assert_eq!(
        store
            .waiter_status(&queued_waiter.wait_id)
            .expect("queued waiter status should load"),
        Some("cancelled".to_string())
    );
    assert_eq!(
        store
            .waiter_status(&reserved_waiter.wait_id)
            .expect("reserved waiter status should load"),
        Some("cancelled".to_string())
    );
}

#[test]
fn cancel_intent_request_rejects_non_cancellable_status() {
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
    store
        .mark_intent_request_status("req-1", "claimed")
        .expect("request status should update");

    store
        .cancel_intent_request("req-1", "s1")
        .expect_err("claimed request should not cancel");

    let request = store
        .intent_request("req-1")
        .expect("request should load")
        .expect("request should exist");
    assert_eq!(request.status, "claimed");
}

#[test]
fn request_aware_enqueue_dedupes_same_resource_only() {
    let store = Store::open_in_memory().expect("store should open");
    store
        .create_intent_request(
            "req-1",
            "s1",
            "w1",
            &["src/auth.ts".to_string(), "src/billing.ts".to_string()],
            "write_file",
        )
        .expect("request should create");

    let first = store
        .enqueue_waiter_for_request("req-1", "s1", "w1", "src/auth.ts", "write_file", Some("s0"))
        .expect("first waiter should enqueue");
    let duplicate = store
        .enqueue_waiter_for_request("req-1", "s1", "w1", "src/auth.ts", "write_file", Some("s0"))
        .expect("duplicate waiter should dedupe");
    let different_resource = store
        .enqueue_waiter_for_request(
            "req-1",
            "s1",
            "w1",
            "src/billing.ts",
            "write_file",
            Some("s0"),
        )
        .expect("different resource should enqueue");

    assert_eq!(first.wait_id, duplicate.wait_id);
    assert_eq!(first.request_id.as_deref(), Some("req-1"));
    assert_eq!(duplicate.request_id.as_deref(), Some("req-1"));
    assert_ne!(first.wait_id, different_resource.wait_id);
    assert_eq!(different_resource.request_id.as_deref(), Some("req-1"));
}

#[test]
fn request_id_survives_reservation_lookups() {
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

    let promoted = store
        .promote_next_waiter("w1", "src/auth.ts")
        .expect("waiter should promote")
        .expect("promoted waiter should load");
    assert_eq!(promoted.wait_id, waiter.wait_id);
    assert_eq!(promoted.request_id.as_deref(), Some("req-1"));

    let active = store
        .active_reservation("w1", "src/auth.ts")
        .expect("reservation should load")
        .expect("active reservation should exist");
    assert_eq!(active.wait_id, waiter.wait_id);
    assert_eq!(active.request_id.as_deref(), Some("req-1"));

    let next = store
        .next_reservation_for_session("s1", "w1")
        .expect("session reservation should load")
        .expect("session reservation should exist");
    assert_eq!(next.wait_id, waiter.wait_id);
    assert_eq!(next.request_id.as_deref(), Some("req-1"));
}

#[test]
fn migrations_create_contract_tables_and_indexes() {
    let store = Store::open_in_memory().expect("in-memory store should open");

    for table in ["schema_migrations", "intent_requests"] {
        assert!(
            store.has_table(table).expect("table check should run"),
            "missing table {table}"
        );
    }

    assert!(
        store
            .has_column("wait_queue", "request_id")
            .expect("column check should run"),
        "missing wait_queue.request_id column"
    );

    for index in [
        "idx_events_workspace_created_at",
        "idx_events_session_sequence",
        "idx_sessions_workspace_session",
        "idx_activities_workspace_expires_at",
        "idx_intents_session_status_expires_at",
        "idx_leases_workspace_absolute_status_expires_at",
        "idx_leases_repo_relative_status_expires_at",
        "idx_intent_requests_session_status",
        "idx_conflicts_session_checked_at",
        "idx_validations_workspace_profile_status",
        "idx_reconciliations_session_created_at",
        "idx_outbox_session_sequence_sync_status",
    ] {
        assert!(
            store.has_index(index).expect("index check should run"),
            "missing index {index}"
        );
    }
}
