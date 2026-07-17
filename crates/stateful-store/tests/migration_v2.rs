use rusqlite::Connection;
use serde_json::Value;
use stateful_core::migration_seed_event_id;
use stateful_core::{
    ActorType, AgentIdentity, ExplicitHandoff, HandoffStatus, RequestEnvelope, SourceKind,
    SourceRef, WorkspaceIdentity,
};
use stateful_store::{
    ClaimAcquire, ClaimPath, ClaimRelease, FixedClock, PresenceRegistration,
    ReservationDeclaration, Store, WriteFenceAcquire,
};
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::{
    fs,
    path::{Path, PathBuf},
};
use tempfile::TempDir;
use time::macros::datetime;
use uuid::Uuid;

const FIXTURE: &str = include_str!("fixtures/v1_persistent_state.sql");
const CHECKPOINT: &str = "stateful.v2.event-journal";

fn legacy_database(temp: &TempDir, name: &str) -> PathBuf {
    let path = temp.path().join(name);
    let connection = Connection::open(&path).expect("fixture database should open");
    connection
        .execute_batch(FIXTURE)
        .expect("fixture SQL should apply");
    path
}

fn migration_clock() -> FixedClock {
    FixedClock::new(
        time::OffsetDateTime::parse(
            "2026-07-15T11:30:00Z",
            &time::format_description::well_known::Rfc3339,
        )
        .expect("fixed migration clock should parse"),
    )
}

fn open_legacy(path: &Path) -> stateful_store::StoreResult<Store> {
    Store::open_with_clock(path, migration_clock())
}

fn backup_path(path: &Path) -> PathBuf {
    path.with_extension("v1.backup.sqlite")
}

fn table_exists(path: &Path, table: &str) -> bool {
    let connection = Connection::open(path).expect("database should open");
    connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1)",
            [table],
            |row| row.get(0),
        )
        .expect("schema lookup should succeed")
}

fn journal_payloads(path: &Path, event_type: &str) -> Vec<Value> {
    let connection = Connection::open(path).expect("migrated database should open");
    let mut statement = connection
        .prepare("SELECT payload_json FROM journal_events WHERE event_type = ?1 ORDER BY event_seq")
        .expect("journal query should prepare");
    statement
        .query_map([event_type], |row| row.get::<_, String>(0))
        .expect("journal query should run")
        .map(|row| {
            serde_json::from_str::<Value>(&row.expect("payload should load"))
                .expect("payload should be JSON")["event"]
                .clone()
        })
        .collect()
}

fn request<T: serde::Serialize>(agent_id: &str, payload: T) -> RequestEnvelope<T> {
    RequestEnvelope::new(
        Uuid::new_v4(),
        datetime!(2026-07-15 11:30 UTC),
        AgentIdentity {
            agent_id: agent_id.into(),
            turn_id: Some("migration-test".into()),
            actor_id: format!("{agent_id}-actor"),
            actor_type: ActorType::Agent,
            owner_id: None,
            parent_agent_id: None,
            parent_actor_id: None,
        },
        WorkspaceIdentity {
            root: "/repo".into(),
            workspace_id: "workspace-main".into(),
            repo_id: "repo-main".into(),
            worktree_id: "worktree-main".into(),
            branch: "main".into(),
        },
        SourceRef {
            kind: SourceKind::Server,
            event: "migration-test".into(),
            tool_name: None,
            source_ref: "migration-v2-test".into(),
        },
        payload,
    )
    .expect("request should be valid")
}

fn payload_owned_projection_rows(path: &Path, table: &str, agent_id: &str) -> i64 {
    Connection::open(path)
        .expect("migrated database should open")
        .query_row(
            &format!(
                "SELECT COUNT(*) FROM {table}
                 WHERE workspace_id = 'workspace-main'
                   AND json_extract(payload_json, '$.agent_id') = ?1"
            ),
            [agent_id],
            |row| row.get(0),
        )
        .expect("owned projection count should load")
}

#[test]
fn persistent_v1_db_is_backed_up_seeded_replayed_and_cut_over() {
    let temp = TempDir::new().expect("temporary directory should exist");
    let path = legacy_database(&temp, "legacy.sqlite");

    let mut store = open_legacy(&path).expect("persistent v1 database should migrate");
    let backup = backup_path(&path);
    assert!(
        backup.exists(),
        "migration must retain a versioned SQLite backup"
    );
    assert!(
        table_exists(&backup, "agents"),
        "backup must open as shipped v1"
    );
    assert!(
        !table_exists(&backup, "journal_events"),
        "backup must not be a raw post-cutover copy"
    );
    #[cfg(unix)]
    assert_eq!(
        fs::metadata(&path)
            .expect("source metadata")
            .permissions()
            .mode(),
        fs::metadata(&backup)
            .expect("backup metadata")
            .permissions()
            .mode(),
        "backup must preserve exact source permission bits",
    );
    #[cfg(not(unix))]
    assert_eq!(
        fs::metadata(&path)
            .expect("source metadata")
            .permissions()
            .readonly(),
        fs::metadata(&backup)
            .expect("backup metadata")
            .permissions()
            .readonly(),
        "backup must preserve the portable readonly permission setting",
    );
    assert!(
        !store
            .has_table("agents")
            .expect("schema lookup should work"),
        "legacy authority must be removed only after validation"
    );
    assert!(store.journal_event_count().expect("journal should load") > 0);
    let before_replay = store
        .projection_snapshot()
        .expect("projection snapshot should load");
    store
        .rebuild_projections()
        .expect("migrated journal should replay identically");
    assert_eq!(
        store
            .projection_snapshot()
            .expect("projection snapshot should load"),
        before_replay
    );

    let audit_ids = Connection::open(&path)
        .expect("migrated database should open")
        .prepare("SELECT json_extract(payload_json, '$.event.data.data.legacy_event_id') FROM journal_events WHERE event_type = 'migration.legacy_audit_imported' ORDER BY event_seq")
        .expect("audit query should prepare")
        .query_map([], |row| row.get::<_, String>(0))
        .expect("audit query should execute")
        .collect::<Result<Vec<_>, _>>()
        .expect("audit ids should load");
    assert_eq!(audit_ids, ["event-a", "event-z", "event-b"]);
}

#[test]
fn legacy_audit_import_retains_source_sequence_as_payload_provenance() {
    let temp = TempDir::new().expect("temporary directory should exist");
    let path = legacy_database(&temp, "audit-sequence.sqlite");
    open_legacy(&path).expect("legacy database should migrate");

    let provenance = journal_payloads(&path, "migration.legacy_audit_imported")
        .into_iter()
        .map(|payload| {
            (
                payload["data"]["data"]["legacy_event_id"].clone(),
                payload["data"]["data"]["legacy_sequence"].clone(),
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(
        provenance,
        vec![
            (serde_json::json!("event-a"), serde_json::json!(2)),
            (serde_json::json!("event-z"), serde_json::json!(9)),
            (serde_json::json!("event-b"), serde_json::json!(3)),
        ],
        "legacy sequence is payload provenance, while audit order remains created_at/event_id",
    );
}

#[test]
fn tied_activities_choose_latest_expiry_then_activity_id() {
    let temp = TempDir::new().expect("temporary directory should exist");
    let path = legacy_database(&temp, "tied.sqlite");
    open_legacy(&path).expect("legacy database should migrate");

    let payloads = journal_payloads(&path, "migration.presence_snapshot_seeded");
    let alpha = payloads
        .iter()
        .find(|payload| payload["data"]["aggregate_id"] == "agent-alpha")
        .expect("agent alpha seed should exist");
    assert_eq!(
        alpha["data"]["data"]["selected_activity_id"],
        "activity-alpha-02"
    );
    assert_eq!(
        alpha["data"]["data"]["source_activity_ids"],
        serde_json::json!(["activity-alpha-01", "activity-alpha-02"])
    );
    let event_id: String = Connection::open(&path)
        .expect("migrated database should open")
        .query_row(
            "SELECT event_id FROM journal_events WHERE event_type = 'migration.presence_snapshot_seeded' AND aggregate_id = 'agent-alpha'",
            [],
            |row| row.get(0),
        )
        .expect("agent alpha seed ID should load");
    assert_eq!(
        event_id,
        migration_seed_event_id("presence", "agent-alpha")
            .expect("seed ID should derive")
            .to_string(),
    );
}

#[test]
fn wait_snapshot_seeds_sort_offsets_by_instant_then_wait_id() {
    let temp = TempDir::new().expect("temporary directory should exist");
    let path = legacy_database(&temp, "wait-order.sqlite");
    open_legacy(&path).expect("legacy database should migrate");

    let connection = Connection::open(&path).expect("migrated database should open");
    let wait_ids = connection
        .prepare(
            "SELECT aggregate_id
             FROM journal_events
             WHERE event_type = 'migration.wait_snapshot_seeded'
             ORDER BY event_seq",
        )
        .expect("wait seed query should prepare")
        .query_map([], |row| row.get::<_, String>(0))
        .expect("wait seed query should run")
        .collect::<Result<Vec<_>, _>>()
        .expect("wait seed IDs should load");

    assert_eq!(
        wait_ids,
        [
            "wait-offset-early",
            "wait-expired",
            "wait-active",
            "wait-tie-a",
            "wait-tie-b",
            "wait-offset-late",
        ],
        "RFC3339 offsets sort by their instant and tied instants use wait_id",
    );
}

#[test]
fn legacy_claim_hash_is_not_read_provenance() {
    let temp = TempDir::new().expect("temporary directory should exist");
    let path = legacy_database(&temp, "claims.sqlite");
    open_legacy(&path).expect("legacy database should migrate");

    let payloads = journal_payloads(&path, "migration.claim_snapshot_seeded");
    let active = payloads
        .iter()
        .find(|payload| payload["data"]["aggregate_id"] == "claim-active")
        .expect("active claim seed should exist");
    assert_eq!(
        active["data"]["data"]["legacy_base_observation"]["content_hash"],
        "legacy-claim-sha"
    );
    assert!(active["data"]["data"].get("read_provenance").is_none());
}

#[test]
fn migration_keeps_human_fingerprints_and_terminal_coordination_records() {
    let temp = TempDir::new().expect("temporary directory should exist");
    let path = legacy_database(&temp, "terminal-provenance.sqlite");
    Connection::open(&path)
        .expect("legacy database should open")
        .execute(
            "UPDATE reservations SET status = 'released' WHERE reservation_id = 'reservation-active'",
            [],
        )
        .expect("legacy blocker reservation releases");
    let mut store = open_legacy(&path).expect("legacy database should migrate");

    let human = journal_payloads(&path, "migration.human_observation_snapshot_seeded")
        .into_iter()
        .find(|payload| payload["data"]["aggregate_id"] == "human-reconciled")
        .expect("human seed should exist");
    assert_eq!(
        human["data"]["data"]["legacy_observation"],
        serde_json::json!({"exists": true, "content_hash": "human-sha"})
    );
    let before_rebuild = store
        .projection_snapshot()
        .expect("projection snapshot loads");
    store
        .rebuild_projections()
        .expect("migration replay succeeds");
    assert_eq!(
        store
            .projection_snapshot()
            .expect("projection snapshot loads"),
        before_rebuild,
        "replay must retain the exact legacy human fingerprint",
    );

    assert_eq!(
        store
            .claim("workspace-main", "claim-expired")
            .expect("terminal claim reads")
            .expect("terminal claim remains queryable")
            .status,
        "expired",
    );
    let fence_payload: String = Connection::open(&path)
        .expect("migrated database should open")
        .query_row(
            "SELECT payload_json FROM write_fence_current WHERE aggregate_id = 'fence-expired'",
            [],
            |row| row.get(0),
        )
        .expect("terminal fence remains queryable");
    assert_eq!(
        serde_json::from_str::<Value>(&fence_payload).expect("terminal fence payload is JSON")["status"],
        "released",
    );

    let reservation = store
        .declare_reservation(&request(
            "agent-gamma",
            ReservationDeclaration {
                scopes: vec![stateful_core::ReservationScope::file("src/review.rs")],
                action: "write_file".into(),
                purpose: "replace terminal records".into(),
            },
        ))
        .expect("terminal records must not block a new reservation")
        .response;
    let claim = store
        .acquire_claim(&request(
            "agent-gamma",
            ClaimAcquire {
                reservation_id: reservation.reservation_id,
                paths: vec![ClaimPath {
                    relative_path: "src/review.rs".into(),
                    observation: None,
                }],
            },
        ))
        .expect("terminal claim must not conflict")
        .response
        .claims
        .remove(0);
    let fence = store
        .acquire_write_fences(&request(
            "agent-gamma",
            WriteFenceAcquire {
                paths: vec!["src/review.rs".into()],
                action: "write_file".into(),
            },
        ))
        .expect("terminal fence must not conflict")
        .response;
    assert_eq!(fence.fences.len(), 1);
    assert_eq!(fence.conflict, None);
    store
        .release_claim(&request(
            "agent-gamma",
            ClaimRelease {
                claim_id: claim.claim_id,
            },
        ))
        .expect("new claim releases");
}

#[test]
fn migrated_claim_conflict_is_frozen_after_blocker_release() {
    let temp = TempDir::new().expect("temporary directory should exist");
    let path = legacy_database(&temp, "frozen-claim-conflict.sqlite");
    Connection::open(&path)
        .expect("legacy database should open")
        .execute(
            "UPDATE reservations SET status = 'released' WHERE reservation_id = 'reservation-active'",
            [],
        )
        .expect("legacy reservation releases");
    let store = open_legacy(&path).expect("legacy database should migrate");
    let reservation = store
        .declare_reservation(&request(
            "agent-beta",
            ReservationDeclaration {
                scopes: vec![stateful_core::ReservationScope::file("src/state.rs")],
                action: "write_file".into(),
                purpose: "contend for a migrated claim".into(),
            },
        ))
        .expect("contender reservation declares")
        .response;
    let request_id = Uuid::new_v4();
    let contender = RequestEnvelope::new(
        request_id,
        datetime!(2026-07-15 11:30 UTC),
        AgentIdentity {
            agent_id: "agent-beta".into(),
            turn_id: Some("migration-test".into()),
            actor_id: "agent-beta-actor".into(),
            actor_type: ActorType::Agent,
            owner_id: None,
            parent_agent_id: None,
            parent_actor_id: None,
        },
        WorkspaceIdentity {
            root: "/repo".into(),
            workspace_id: "workspace-main".into(),
            repo_id: "repo-main".into(),
            worktree_id: "worktree-main".into(),
            branch: "main".into(),
        },
        SourceRef {
            kind: SourceKind::Server,
            event: "migration-test".into(),
            tool_name: None,
            source_ref: "migration-v2-test".into(),
        },
        ClaimAcquire {
            reservation_id: reservation.reservation_id,
            paths: vec![ClaimPath {
                relative_path: "src/state.rs".into(),
                observation: None,
            }],
        },
    )
    .expect("contender request is valid");

    assert!(matches!(
        store.acquire_claim(&contender),
        Err(stateful_store::StoreError::ClaimConflict)
    ));
    store
        .release_claim(&request(
            "agent-alpha",
            ClaimRelease {
                claim_id: "claim-active".into(),
            },
        ))
        .expect("migrated blocker claim releases");
    let event_count = store.journal_event_count().expect("journal count loads");

    assert!(matches!(
        store.acquire_claim(&contender),
        Err(stateful_store::StoreError::ClaimConflict)
    ));
    assert_eq!(
        store.journal_event_count().expect("journal count loads"),
        event_count,
        "the duplicate must not acquire a now-unblocked claim",
    );
    assert!(
        store
            .active_claims_for_path("workspace-main", "src/state.rs")
            .expect("active claims load")
            .is_empty()
    );

    let mut changed_actor = contender.clone();
    changed_actor.agent.actor_id = "other-actor".into();
    assert!(matches!(
        store.acquire_claim(&changed_actor),
        Err(stateful_store::StoreError::IdempotencyKeyReused)
    ));
    let mut changed_body = contender.clone();
    changed_body.payload.paths[0].relative_path = "src/other.rs".into();
    assert!(matches!(
        store.acquire_claim(&changed_body),
        Err(stateful_store::StoreError::IdempotencyKeyReused)
    ));
}

#[test]
fn existing_v2_upgrade_repairs_omitted_terminal_seed_projections() {
    let temp = TempDir::new().expect("temporary directory should exist");
    let path = legacy_database(&temp, "prechange-terminal-projections.sqlite");
    drop(open_legacy(&path).expect("legacy database should migrate"));
    Connection::open(&path)
        .expect("migrated database opens")
        .execute_batch(
            "
            DELETE FROM claim_current WHERE aggregate_id = 'claim-expired';
            DELETE FROM write_fence_current WHERE aggregate_id = 'fence-expired';
            ",
        )
        .expect("prechange omitted terminal projections simulate");

    let mut store = Store::open_with_clock(&path, migration_clock())
        .expect("existing v2 database repairs terminal projections");
    assert_eq!(
        store
            .claim("workspace-main", "claim-expired")
            .expect("claim reads")
            .expect("terminal claim is repaired")
            .status,
        "expired",
    );
    assert!(
        store
            .active_claims_for_path("workspace-main", "src/review.rs")
            .expect("active claims load")
            .is_empty()
    );
    assert!(
        store
            .active_write_fence("workspace-main", "src/review.rs")
            .expect("active fence loads")
            .is_none()
    );
    store
        .rebuild_projections()
        .expect("repaired projections replay identically");
}

#[test]
fn failed_terminal_projection_repair_rolls_back_canonical_tables() {
    let temp = TempDir::new().expect("temporary directory should exist");
    let path = legacy_database(&temp, "terminal-repair-rollback.sqlite");
    drop(open_legacy(&path).expect("legacy database should migrate"));
    let connection = Connection::open(&path).expect("migrated database opens");
    connection
        .execute_batch(
            "
            DELETE FROM claim_current WHERE aggregate_id = 'claim-expired';
            UPDATE claim_current
            SET payload_json = '{}'
            WHERE aggregate_id = 'claim-active';
            ",
        )
        .expect("repair failure setup applies");
    drop(connection);

    assert!(matches!(
        Store::open_with_clock(&path, migration_clock()),
        Err(stateful_store::StoreError::ReplayMismatch)
    ));
    let missing: u64 = Connection::open(&path)
        .expect("database reopens after failed repair")
        .query_row(
            "SELECT COUNT(*) FROM claim_current WHERE aggregate_id = 'claim-expired'",
            [],
            |row| row.get(0),
        )
        .expect("canonical table reads");
    assert_eq!(
        missing, 0,
        "failed repair must not commit partial canonical rows"
    );
}

#[test]
fn unavailable_actor_and_handoff_fields_remain_unknown_or_empty() {
    let temp = TempDir::new().expect("temporary directory should exist");
    let path = legacy_database(&temp, "handoff.sqlite");
    open_legacy(&path).expect("legacy database should migrate");

    let payload = journal_payloads(&path, "migration.legacy_handoff_snapshot_seeded")
        .into_iter()
        .next()
        .expect("finalized legacy activity should seed handoff");
    assert_eq!(payload["data"]["data"]["status"], "unknown");
    assert_eq!(payload["data"]["data"]["actor_id"], "unknown");
    assert_eq!(payload["data"]["data"]["goal"], "");
    assert_eq!(payload["data"]["data"]["resources"], serde_json::json!([]));
    assert_eq!(payload["data"]["data"]["cleanup_count"], 2);
}

#[test]
fn malformed_legacy_json_rolls_back_and_preserves_original_schema() {
    let temp = TempDir::new().expect("temporary directory should exist");
    let path = legacy_database(&temp, "malformed.sqlite");
    Connection::open(&path)
        .expect("fixture should open")
        .execute("UPDATE notifications SET payload_json = '{' WHERE notification_id = 'notification-pending'", [])
        .expect("fixture should accept malformed legacy JSON");

    let error = match open_legacy(&path) {
        Ok(_) => panic!("malformed legacy JSON must reject migration"),
        Err(error) => error,
    };
    assert_eq!(error.code(), "migration_validation");
    assert!(
        table_exists(&path, "agents"),
        "legacy source must remain authoritative"
    );
    assert!(
        !table_exists(&path, "journal_events"),
        "failed preflight must not create journal tables"
    );
    assert!(
        !backup_path(&path).exists(),
        "preflight failure must not create a backup"
    );
}

#[test]
fn migration_rerun_after_checkpoint_is_a_no_op() {
    let temp = TempDir::new().expect("temporary directory should exist");
    let path = legacy_database(&temp, "rerun.sqlite");
    let first = open_legacy(&path).expect("first open should migrate");
    let event_count = first
        .journal_event_count()
        .expect("journal count should load");
    drop(first);
    let backup = backup_path(&path);
    let backup_count = fs::read_dir(temp.path())
        .expect("temporary directory should open")
        .filter_map(Result::ok)
        .filter(|entry| entry.path().extension().and_then(|value| value.to_str()) == Some("sqlite"))
        .count();

    let second = open_legacy(&path).expect("second open should validate checkpoint");
    assert_eq!(
        second
            .journal_event_count()
            .expect("journal count should load"),
        event_count
    );
    assert!(backup.exists());
    assert_eq!(
        fs::read_dir(temp.path())
            .expect("temporary directory should open")
            .filter_map(Result::ok)
            .filter(
                |entry| entry.path().extension().and_then(|value| value.to_str()) == Some("sqlite")
            )
            .count(),
        backup_count,
        "second open must not create another backup"
    );
    assert!(
        Connection::open(&path)
            .expect("migrated database should open")
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM schema_migrations WHERE version = ?1)",
                [CHECKPOINT],
                |row| row.get::<_, bool>(0)
            )
            .expect("checkpoint query should work")
    );
}

#[test]
fn new_and_in_memory_databases_skip_legacy_migration() {
    let temp = TempDir::new().expect("temporary directory should exist");
    let path = temp.path().join("new.sqlite");
    let persistent =
        Store::open(&path).expect("new persistent database should initialize v2 directly");
    assert_eq!(
        persistent
            .journal_event_count()
            .expect("journal count should load"),
        0
    );
    assert!(
        !persistent
            .has_table("agents")
            .expect("schema lookup should work")
    );
    assert!(!backup_path(&path).exists());

    let memory = Store::open_in_memory().expect("in-memory database should initialize v2 directly");
    assert_eq!(
        memory
            .journal_event_count()
            .expect("journal count should load"),
        0
    );
    assert!(
        !memory
            .has_table("agents")
            .expect("schema lookup should work")
    );
}

#[test]
fn migrated_presence_and_handoff_project_to_typed_records_before_commands() {
    let temp = TempDir::new().expect("temporary directory should exist");
    let path = legacy_database(&temp, "typed-current.sqlite");
    let mut store = open_legacy(&path).expect("legacy database should migrate");

    let presence = store
        .presence_for_request(&request("agent-alpha", ()), "agent-alpha")
        .expect("typed migrated presence should load")
        .expect("migrated agent should remain present");
    assert_eq!(presence.agent_id, "agent-alpha");
    assert_eq!(presence.actor_type, ActorType::Unknown);
    store
        .resume_presence(&request(
            "agent-alpha",
            PresenceRegistration { first_prompt: None },
        ))
        .expect("commands must accept a migrated presence projection");
    let handoff = store
        .handoff_for_request(&request("agent-alpha", ()), "agent-alpha")
        .expect("typed migrated handoff should load")
        .expect("legacy finalized activity should project a handoff");
    assert_eq!(handoff.status, HandoffStatus::Unknown);
    assert!(!handoff.explicit);
}

#[test]
fn migrated_handoff_accepts_a_v2_owner_after_presence_is_absent() {
    let temp = TempDir::new().expect("temporary directory should exist");
    let path = legacy_database(&temp, "handoff-adoption.sqlite");
    let mut store = open_legacy(&path).expect("legacy database should migrate");
    Connection::open(&path)
        .expect("migrated database should open")
        .execute(
            "DELETE FROM presence_current WHERE workspace_id = ?1 AND agent_id = ?2",
            ["workspace-main", "agent-alpha"],
        )
        .expect("migrated presence should be absent");

    let handoff = store
        .finalize_handoff(&request(
            "agent-alpha",
            ExplicitHandoff {
                status: HandoffStatus::Done,
                summary: "finished".into(),
                files_changed: vec![],
                tests_run: vec![],
                remaining_work: vec![],
                next_plan: None,
            },
        ))
        .expect("migrated handoff should accept its V2 owner");
    assert!(handoff.response.explicit);
}

#[test]
fn resumed_migrated_presence_finalization_cleans_coordination_rows_by_payload_owner() {
    let temp = TempDir::new().expect("temporary directory should exist");
    let path = legacy_database(&temp, "owned-cleanup.sqlite");
    Connection::open(&path)
        .expect("legacy database should open")
        .execute_batch(
            "
            INSERT INTO reservations (reservation_id, agent_id, workspace_id, purpose, scopes_json, status, declared_at, expires_at)
            VALUES ('reservation-beta-active', 'agent-beta', 'workspace-main', 'beta work', '[{\"kind\":\"file\",\"path\":\"src/beta.rs\"}]', 'active', '2026-07-15T11:00:00Z', '2026-07-15T12:30:00Z');
            INSERT INTO claims (claim_id, reservation_id, agent_id, workspace_id, repo_id, relative_path, absolute_path, purpose, action, status, expires_at, observed_exists, observed_content_hash)
            VALUES ('claim-beta-active', 'reservation-beta-active', 'agent-beta', 'workspace-main', 'repo-main', 'src/beta.rs', '/repo/src/beta.rs', 'beta work', 'write_file', 'active', '2026-07-15T12:30:00Z', 1, NULL);
            INSERT INTO write_fences (fence_id, agent_id, workspace_id, relative_path, action, acquired_at, expires_at, released_at)
            VALUES ('fence-beta-active', 'agent-beta', 'workspace-main', 'src/beta.rs', 'write_file', '2026-07-15T11:00:00Z', '2026-07-15T12:30:00Z', NULL);
            INSERT INTO wait_queue (wait_id, request_id, agent_id, workspace_id, repo_id, worktree_id, root, branch, relative_path, action, status, requested_at, reservation_expires_at, blocking_agent_id, purpose)
            VALUES ('wait-alpha-active', 'request-alpha-active', 'agent-alpha', 'workspace-main', 'repo-main', 'worktree-main', '/repo', 'main', 'src/beta.rs', 'write_file', 'waiting', '2026-07-15T11:00:00Z', '2026-07-15T12:30:00Z', 'agent-beta', 'alpha wait');
            ",
        )
        .expect("fixture should gain active rows for both agents");
    let mut store = open_legacy(&path).expect("legacy database should migrate");
    for table in [
        "reservation_current",
        "claim_current",
        "wait_current",
        "write_fence_current",
    ] {
        assert!(
            payload_owned_projection_rows(&path, table, "agent-alpha") > 0,
            "alpha must own a {table} row"
        );
        assert!(
            payload_owned_projection_rows(&path, table, "agent-beta") > 0,
            "beta must own a {table} row"
        );
    }

    store
        .resume_presence(&request(
            "agent-alpha",
            PresenceRegistration { first_prompt: None },
        ))
        .expect("migrated presence should resume before finalization");

    store
        .finalize_handoff(&request(
            "agent-alpha",
            ExplicitHandoff {
                status: HandoffStatus::Done,
                summary: "finished".into(),
                files_changed: vec![],
                tests_run: vec![],
                remaining_work: vec![],
                next_plan: None,
            },
        ))
        .expect("alpha finalization should succeed");

    for table in [
        "reservation_current",
        "claim_current",
        "wait_current",
        "write_fence_current",
    ] {
        assert_eq!(
            payload_owned_projection_rows(&path, table, "agent-alpha"),
            0,
            "alpha {table} rows must be cleaned"
        );
        assert!(
            payload_owned_projection_rows(&path, table, "agent-beta") > 0,
            "beta {table} rows must remain"
        );
    }
    store
        .rebuild_projections()
        .expect("resumed migration cleanup should replay");
}
