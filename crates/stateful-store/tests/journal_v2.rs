use rusqlite::Connection;
use serde_json::json;
use stateful_core::{
    ActorType, AgentIdentity, AuthorizationEvent, EventData, EventPayload, NewEvent, RequestEnvelope,
    SourceKind, SourceRef, WorkspaceIdentity,
};
use stateful_store::{CommandPlan, FixedClock, Store, StoreError};
use tempfile::TempDir;
use time::{macros::datetime, OffsetDateTime};
use uuid::Uuid;

const NOW: OffsetDateTime = datetime!(2026-07-15 12:00 UTC);

fn request(request_id: Uuid, payload: serde_json::Value) -> RequestEnvelope<serde_json::Value> {
    RequestEnvelope::new(
        request_id,
        NOW,
        AgentIdentity {
            agent_id: "agent-1".into(),
            turn_id: Some("turn-1".into()),
            actor_id: "actor-1".into(),
            actor_type: ActorType::Agent,
            owner_id: None,
            parent_agent_id: None,
            parent_actor_id: None,
        },
        WorkspaceIdentity {
            root: "/repo".into(),
            workspace_id: "workspace-1".into(),
            repo_id: "repo-1".into(),
            worktree_id: "worktree-1".into(),
            branch: "main".into(),
        },
        SourceRef {
            kind: SourceKind::Cli,
            event: "test".into(),
            tool_name: None,
            source_ref: "journal-v2-test".into(),
        },
        payload,
    )
    .expect("test request should be valid")
}

fn event(request_id: Uuid, ordinal: u32, event_type: &str) -> NewEvent {
    let payload = match event_type {
        "authorization.allowed" => EventPayload::Authorization(AuthorizationEvent::Allowed(EventData::new("audit-1"))),
        _ => EventPayload::Authorization(AuthorizationEvent::Denied(EventData::new("decision-1"))),
    };
    NewEvent::new(request_id, ordinal, NOW, payload).expect("test event should be valid")
}

fn command_plan(request_id: Uuid, events: Vec<NewEvent>) -> CommandPlan<serde_json::Value> {
    CommandPlan {
        events,
        response: json!({"request_id": request_id}),
        http_status: 201,
    }
}

fn store() -> Store {
    Store::open_in_memory_with_clock(FixedClock::new(NOW)).expect("store should open")
}

fn corrupt_replay_fails_safely(column: &str, value: &str) {
    let mut store = store();
    let request = request(Uuid::new_v4(), json!({"intent":"deny"}));
    store
        .execute_command(&request, "test.command", |_| Ok(command_plan(request.request_id, vec![event(request.request_id, 0, "authorization.denied")])))
        .expect("command should commit");
    let before = store.projection_snapshot().expect("snapshot should load");
    store.corrupt_journal_metadata_for_tests(column, value).expect("corruption should apply");
    assert!(store.rebuild_projections().is_err(), "{column} corruption must fail replay");
    assert_eq!(store.projection_snapshot().expect("snapshot should load"), before);
}

#[test]
fn command_appends_projects_versions_receipts_and_commits_atomically() {
    let store = store();
    let request = request(Uuid::new_v4(), json!({"intent":"deny"}));

    let outcome = store
        .execute_command(&request, "test.command", |_| Ok(command_plan(request.request_id, vec![event(request.request_id, 0, "authorization.denied")])))
        .expect("command should commit");

    assert_eq!(outcome.first_event_seq, Some(1));
    assert_eq!(outcome.last_event_seq, Some(1));
    assert!(!outcome.duplicate);
    assert_eq!(store.journal_event_count().expect("journal count should load"), 1);
    assert_eq!(store.command_receipt_count().expect("receipt count should load"), 1);
    assert_eq!(store.workspace_version("workspace-1").expect("version should load"), 1);
    assert_eq!(store.projection_row_count().expect("projections should load"), 2);
    for table in [
        "journal_events", "command_receipts", "presence_current", "presence_resource_current",
        "reservation_current", "claim_current", "wait_current", "write_fence_current",
        "read_observation_current", "write_intent_current", "human_observation_current",
        "handoff_current", "handoff_resource_current", "notification_current",
        "workspace_version", "agent_context_cursor", "resource_write_current", "migration_current",
    ] {
        assert!(store.has_table(table).expect("schema lookup should work"), "{table} must exist");
    }
    for index in [
        "idx_journal_events_workspace_sequence", "idx_journal_events_workspace_type",
        "idx_journal_events_aggregate", "idx_claim_current_active_expiry",
        "idx_write_fence_current_active_expiry", "idx_wait_current_operation",
        "idx_write_intent_current_operation", "idx_resource_write_current_path",
        "idx_notification_current_target_version",
    ] {
        assert!(store.has_index(index).expect("schema lookup should work"), "{index} must exist");
    }
}

#[test]
fn deterministic_rejection_is_frozen_without_events() {
    let mut store = store();
    let frozen_request = request(Uuid::new_v4(), json!({"intent":"deny"}));

    assert!(matches!(
        store.execute_command(
            &frozen_request,
            "test.command",
            |_| -> stateful_store::StoreResult<CommandPlan<serde_json::Value>> {
                Err(StoreError::ClaimConflict)
            },
        ),
        Err(StoreError::ClaimConflict)
    ));
    assert_eq!(store.command_receipt_count().expect("receipt count loads"), 1);
    assert_eq!(store.journal_event_count().expect("journal count loads"), 0);

    let state_change = request(Uuid::new_v4(), json!({"intent":"allow"}));
    store
        .execute_command(&state_change, "test.state_change", |_| {
            Ok(command_plan(
                state_change.request_id,
                vec![event(
                    state_change.request_id,
                    0,
                    "authorization.denied",
                )],
            ))
        })
        .expect("unrelated state change commits");
    let event_count = store.journal_event_count().expect("journal count loads");

    assert!(matches!(
        store.execute_command(
            &frozen_request,
            "test.command",
            |_| -> stateful_store::StoreResult<CommandPlan<serde_json::Value>> {
                panic!("duplicate must not rerun policy")
            },
        ),
        Err(StoreError::ClaimConflict)
    ));
    assert_eq!(store.journal_event_count().expect("journal count loads"), event_count);

    let mut changed_actor = frozen_request.clone();
    changed_actor.agent.actor_id = "other-actor".into();
    assert!(matches!(
        store.execute_command(
            &changed_actor,
            "test.command",
            |_| -> stateful_store::StoreResult<CommandPlan<serde_json::Value>> {
                panic!("mismatched duplicate must fail before policy")
            },
        ),
        Err(StoreError::IdempotencyKeyReused)
    ));
    store.rebuild_projections().expect("eventless rejection receipts replay");
}

#[test]
fn unexpected_protocol_errors_remain_retryable() {
    let store = store();
    let request = request(Uuid::new_v4(), json!({"intent":"deny"}));

    let error = store
        .execute_command(
            &request,
            "test.command",
            |_| -> stateful_store::StoreResult<CommandPlan<serde_json::Value>> {
                Err(StoreError::V2(stateful_core::V2Error::new(
                    "internal_test_failure",
                    "simulated internal defect",
                )))
            },
        )
        .expect_err("unexpected protocol errors must not freeze a receipt");
    assert_eq!(error.code(), "internal_test_failure");
    assert_eq!(store.command_receipt_count().expect("receipt count loads"), 0);
    assert_eq!(store.journal_event_count().expect("journal count loads"), 0);

    let retry = store
        .execute_command(&request, "test.command", |_| {
            Ok(command_plan(request.request_id, vec![]))
        })
        .expect("the repaired command retries");
    assert!(!retry.duplicate);
}

#[test]
fn frozen_rejection_reconstructs_its_exact_store_error() {
    let store = store();
    let request = request(Uuid::new_v4(), json!({"intent":"deny"}));
    let expected = StoreError::WriteFenceConflict {
        path: "src/lib.rs".into(),
        owner_agent_id: "agent-2".into(),
    };

    assert!(matches!(
        store.execute_command(
            &request,
            "test.command",
            |_| -> stateful_store::StoreResult<CommandPlan<serde_json::Value>> {
                Err(expected)
            },
        ),
        Err(StoreError::WriteFenceConflict { .. })
    ));
    let duplicate = store
        .execute_command(
            &request,
            "test.command",
            |_| -> stateful_store::StoreResult<CommandPlan<serde_json::Value>> {
                panic!("duplicate must replay the persisted rejection")
            },
        )
        .expect_err("frozen rejection replays");
    assert!(matches!(
        duplicate,
        StoreError::WriteFenceConflict {
            path,
            owner_agent_id,
        } if path == "src/lib.rs" && owner_agent_id == "agent-2"
    ));
}

#[test]
fn replay_rejects_corrupt_eventless_rejection_receipts() {
    let temp = TempDir::new().expect("temporary directory exists");
    let path = temp.path().join("eventless-rejection.sqlite");
    let request = request(Uuid::new_v4(), json!({"intent":"deny"}));
    {
        let store = Store::open_with_clock(&path, FixedClock::new(NOW))
            .expect("store opens");
        assert!(matches!(
            store.execute_command(
                &request,
                "test.command",
                |_| -> stateful_store::StoreResult<CommandPlan<serde_json::Value>> {
                    Err(StoreError::ClaimConflict)
                },
            ),
            Err(StoreError::ClaimConflict)
        ));
    }
    Connection::open(&path)
        .expect("database opens")
        .execute(
            "UPDATE command_receipts SET rejection_json = 'not JSON'",
            [],
        )
        .expect("receipt corruption applies");

    let mut store = Store::open_with_clock(&path, FixedClock::new(NOW))
        .expect("store reopens");
    assert!(
        store.rebuild_projections().is_err(),
        "replay must validate eventless rejection receipts"
    );
}

#[test]
fn prechange_v2_receipts_upgrade_and_replay() {
    let temp = TempDir::new().expect("temporary directory exists");
    let path = temp.path().join("prechange-v2.sqlite");
    let frozen_request = request(Uuid::new_v4(), json!({"intent":"deny"}));
    {
        let store = Store::open_with_clock(&path, FixedClock::new(NOW))
            .expect("v2 store opens");
        store
            .execute_command(&frozen_request, "test.command", |_| {
                Ok(command_plan(
                    frozen_request.request_id,
                    vec![event(
                        frozen_request.request_id,
                        0,
                        "authorization.denied",
                    )],
                ))
            })
            .expect("receipt commits");
    }
    let connection = Connection::open(&path).expect("database opens");
    connection
        .execute_batch(
            "
            ALTER TABLE command_receipts RENAME TO command_receipts_prechange;
            CREATE TABLE command_receipts (
                request_id TEXT PRIMARY KEY,
                route_kind TEXT NOT NULL,
                request_sha256 TEXT NOT NULL,
                agent_id TEXT NOT NULL,
                actor_id TEXT NOT NULL,
                workspace_id TEXT NOT NULL,
                http_status INTEGER NOT NULL,
                response_json TEXT NOT NULL,
                first_event_seq INTEGER,
                last_event_seq INTEGER,
                committed_at TEXT NOT NULL
            );
            INSERT INTO command_receipts (
                request_id, route_kind, request_sha256, agent_id, actor_id, workspace_id,
                http_status, response_json, first_event_seq, last_event_seq, committed_at
            )
            SELECT
                request_id, route_kind, request_sha256, agent_id, actor_id, workspace_id,
                http_status, response_json, first_event_seq, last_event_seq, committed_at
            FROM command_receipts_prechange;
            DROP TABLE command_receipts_prechange;
            ",
        )
        .expect("prechange receipt schema restores");
    drop(connection);

    let mut store = Store::open_with_clock(&path, FixedClock::new(NOW))
        .expect("prechange v2 store upgrades");
    let duplicate = store
        .execute_command(
            &frozen_request,
            "test.command",
            |_| -> stateful_store::StoreResult<CommandPlan<serde_json::Value>> {
                panic!("upgraded receipt must replay")
            },
        )
        .expect("upgraded receipt replays");
    assert!(duplicate.duplicate);
    store
        .rebuild_projections()
        .expect("upgraded receipt still validates event metadata during replay");
    drop(store);

    let connection = Connection::open(&path).expect("upgraded database opens");
    let columns = connection
        .prepare("PRAGMA table_info(command_receipts)")
        .expect("schema query prepares")
        .query_map([], |row| row.get::<_, String>(1))
        .expect("schema query runs")
        .collect::<Result<Vec<_>, _>>()
        .expect("schema columns load");
    assert!(columns.iter().any(|column| column == "rejection_json"));
}

#[test]
fn duplicate_request_returns_frozen_response_without_new_events() {
    let store = store();
    let request = request(Uuid::new_v4(), json!({"intent":"deny"}));
    let first = store
        .execute_command(&request, "test.command", |_| Ok(command_plan(request.request_id, vec![event(request.request_id, 0, "authorization.denied")])))
        .expect("first command should commit");
    let duplicate = store
        .execute_command(&request, "test.command", |_| -> stateful_store::StoreResult<CommandPlan<serde_json::Value>> { panic!("duplicates must not rerun policy") })
        .expect("exact duplicate should succeed");

    assert!(!first.duplicate);
    assert!(duplicate.duplicate);
    assert_eq!(duplicate.response, first.response);
    assert_eq!(duplicate.http_status, first.http_status);
    assert_eq!(store.journal_event_count().expect("journal count should load"), 1);
}

#[test]
fn request_id_reuse_with_different_route_identity_or_payload_is_rejected() {
    let store = store();
    let request_id = Uuid::new_v4();
    let original = request(request_id, json!({"intent":"deny"}));
    store
        .execute_command(&original, "test.command", |_| Ok(command_plan(request_id, vec![event(request_id, 0, "authorization.denied")])))
        .expect("first command should commit");

    let changed_payload = request(request_id, json!({"intent":"allow"}));
    let error = store
        .execute_command(&changed_payload, "test.command", |_| Ok(command_plan(request_id, vec![])))
        .expect_err("payload reuse must fail");
    assert_eq!(error.code(), "idempotency_key_reused");
    let incompatible_response_error = store
        .execute_command(&changed_payload, "test.command", |_| {
            Ok(CommandPlan {
                events: vec![],
                response: "wrong response type".to_owned(),
                http_status: 200,
            })
        })
        .expect_err("mismatched reuse must validate identity before response decoding");
    assert_eq!(incompatible_response_error.code(), "idempotency_key_reused");
    let route_error = store
        .execute_command(&original, "other.command", |_| Ok(command_plan(request_id, vec![])))
        .expect_err("route reuse must fail");
    assert_eq!(route_error.code(), "idempotency_key_reused");
    let mut changed_identity = original.clone();
    changed_identity.agent.actor_id = "other-actor".into();
    let identity_error = store
        .execute_command(&changed_identity, "test.command", |_| Ok(command_plan(request_id, vec![])))
        .expect_err("identity reuse must fail");
    assert_eq!(identity_error.code(), "idempotency_key_reused");
}

#[test]
fn projector_failure_rolls_back_journal_projection_version_and_receipt() {
    {
    let mut store = store();
    let request = request(Uuid::new_v4(), json!({"intent":"deny"}));
    store.fail_projector_on_event_for_tests(2);

    assert!(store
        .execute_command(&request, "test.command", |_| Ok(command_plan(request.request_id, vec![
            event(request.request_id, 0, "authorization.denied"),
            event(request.request_id, 1, "authorization.denied"),
        ])))
        .is_err());
    assert_eq!(store.journal_event_count().expect("journal count should load"), 0);
    assert_eq!(store.projection_row_count().expect("projections should load"), 0);
    assert_eq!(store.workspace_version("workspace-1").expect("version should load"), 0);
    assert_eq!(store.command_receipt_count().expect("receipt count should load"), 0);
    }
    let mut corrupted_live_store = store();
    let corrupted_live_request = request(Uuid::new_v4(), json!({"intent":"deny"}));
    corrupted_live_store.corrupt_next_journal_metadata_for_tests("source_ref", "wrong-source");
    assert!(corrupted_live_store
        .execute_command(&corrupted_live_request, "test.command", |_| Ok(command_plan(corrupted_live_request.request_id, vec![event(corrupted_live_request.request_id, 0, "authorization.denied")])))
        .is_err(), "post-insert envelope corruption must roll back live execution");
    assert_eq!(corrupted_live_store.journal_event_count().expect("journal count should load"), 0);
    assert_eq!(corrupted_live_store.projection_row_count().expect("projection count should load"), 0);
}

#[test]
fn audit_only_event_does_not_advance_workspace_version() {
    let store = store();
    let request = request(Uuid::new_v4(), json!({"intent":"audit"}));
    store
        .execute_command(&request, "test.command", |_| Ok(command_plan(request.request_id, vec![event(request.request_id, 0, "authorization.allowed")])))
        .expect("audit command should commit");

    assert_eq!(store.workspace_version("workspace-1").expect("version should load"), 0);
}

#[test]
fn replay_into_empty_projection_tables_is_byte_equivalent() {
    {
    let mut store = store();
    let request = request(Uuid::new_v4(), json!({"intent":"deny"}));
    store
        .execute_command(&request, "test.command", |_| Ok(command_plan(request.request_id, vec![event(request.request_id, 0, "authorization.denied")])))
        .expect("command should commit");
    let before = store.projection_snapshot().expect("snapshot should load");
    let schema_before = store.projection_schema_snapshot().expect("schema should load");
    let journal_count = store.journal_event_count().expect("journal count should load");
    let receipt_count = store.command_receipt_count().expect("receipt count should load");

    let report = store.rebuild_projections().expect("replay should succeed");

    assert_eq!(store.projection_snapshot().expect("snapshot should load"), before);
    assert_eq!(store.journal_event_count().expect("journal count should load"), journal_count);
    assert_eq!(store.command_receipt_count().expect("receipt count should load"), receipt_count);
    assert_eq!(report.projectable_events, 1);
    assert_ne!(report.canonical_sha256, "");
    store.rebuild_projections().expect("replay should remain repeatable");
    assert_eq!(store.projection_snapshot().expect("snapshot should load"), before);
    assert_eq!(store.projection_schema_snapshot().expect("schema should load"), schema_before);
    }
    let mut corrupt_store = store();
    let corrupt_request = request(Uuid::new_v4(), json!({"intent":"deny"}));
    corrupt_store
        .execute_command(&corrupt_request, "test.command", |_| Ok(command_plan(corrupt_request.request_id, vec![event(corrupt_request.request_id, 0, "authorization.denied")])))
        .expect("command should commit");
    let corrupt_before = corrupt_store.projection_snapshot().expect("snapshot should load");
    corrupt_store.corrupt_journal_metadata_for_tests("event_type", "presence.registered").expect("corruption should apply");
    assert!(corrupt_store.rebuild_projections().is_err(), "corrupt persisted metadata must fail replay");
    assert_eq!(corrupt_store.projection_snapshot().expect("snapshot should load"), corrupt_before);
    corrupt_replay_fails_safely("actor_type", "robot");
    corrupt_replay_fails_safely("affects_context", "2");
    corrupt_replay_fails_safely("agent_id", "wrong-agent");
    corrupt_replay_fails_safely("source_ref", "");
}

#[test]
fn event_sequence_and_id_are_stable_and_unique() {
    let store = store();
    let request = request(Uuid::new_v4(), json!({"intent":"deny"}));
    let events = vec![
        event(request.request_id, 0, "authorization.denied"),
        event(request.request_id, 1, "authorization.denied"),
    ];
    let expected_ids = events.iter().map(|event| event.event_id.to_string()).collect::<Vec<_>>();
    let outcome = store
        .execute_command(&request, "test.command", |_| Ok(command_plan(request.request_id, events)))
        .expect("command should commit");

    assert_eq!(outcome.first_event_seq, Some(1));
    assert_eq!(outcome.last_event_seq, Some(2));
    assert_eq!(store.journal_event_ids().expect("journal IDs should load"), expected_ids);
}
