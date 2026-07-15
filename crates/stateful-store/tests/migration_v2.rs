use rusqlite::Connection;
use serde_json::Value;
use stateful_core::migration_seed_event_id;
use stateful_store::{FixedClock, Store};
use std::{fs, path::{Path, PathBuf}};
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use tempfile::TempDir;

const FIXTURE: &str = include_str!("fixtures/v1_persistent_state.sql");
const CHECKPOINT: &str = "stateful.v2.event-journal";

fn legacy_database(temp: &TempDir, name: &str) -> PathBuf {
    let path = temp.path().join(name);
    let connection = Connection::open(&path).expect("fixture database should open");
    connection.execute_batch(FIXTURE).expect("fixture SQL should apply");
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
        .map(|row| serde_json::from_str::<Value>(&row.expect("payload should load")).expect("payload should be JSON")["event"].clone())
        .collect()
}


#[test]
fn persistent_v1_db_is_backed_up_seeded_replayed_and_cut_over() {
    let temp = TempDir::new().expect("temporary directory should exist");
    let path = legacy_database(&temp, "legacy.sqlite");

    let mut store = open_legacy(&path).expect("persistent v1 database should migrate");
    let backup = backup_path(&path);
    assert!(backup.exists(), "migration must retain a versioned SQLite backup");
    assert!(table_exists(&backup, "agents"), "backup must open as shipped v1");
    assert!(!table_exists(&backup, "journal_events"), "backup must not be a raw post-cutover copy");
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
    assert!(!store.has_table("agents").expect("schema lookup should work"), "legacy authority must be removed only after validation");
    assert!(store.journal_event_count().expect("journal should load") > 0);
    let before_replay = store.projection_snapshot().expect("projection snapshot should load");
    store.rebuild_projections().expect("migrated journal should replay identically");
    assert_eq!(store.projection_snapshot().expect("projection snapshot should load"), before_replay);

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
fn tied_activities_choose_latest_expiry_then_activity_id() {
    let temp = TempDir::new().expect("temporary directory should exist");
    let path = legacy_database(&temp, "tied.sqlite");
    open_legacy(&path).expect("legacy database should migrate");

    let payloads = journal_payloads(&path, "migration.presence_snapshot_seeded");
    let alpha = payloads
        .iter()
        .find(|payload| payload["data"]["aggregate_id"] == "agent-alpha")
        .expect("agent alpha seed should exist");
    assert_eq!(alpha["data"]["data"]["selected_activity_id"], "activity-alpha-02");
    assert_eq!(alpha["data"]["data"]["source_activity_ids"], serde_json::json!(["activity-alpha-01", "activity-alpha-02"]));
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
    assert_eq!(active["data"]["data"]["legacy_base_observation"]["content_hash"], "legacy-claim-sha");
    assert!(active["data"]["data"].get("read_provenance").is_none());
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
    assert!(table_exists(&path, "agents"), "legacy source must remain authoritative");
    assert!(!table_exists(&path, "journal_events"), "failed preflight must not create journal tables");
    assert!(!backup_path(&path).exists(), "preflight failure must not create a backup");
}

#[test]
fn migration_rerun_after_checkpoint_is_a_no_op() {
    let temp = TempDir::new().expect("temporary directory should exist");
    let path = legacy_database(&temp, "rerun.sqlite");
    let first = open_legacy(&path).expect("first open should migrate");
    let event_count = first.journal_event_count().expect("journal count should load");
    drop(first);
    let backup = backup_path(&path);
    let backup_count = fs::read_dir(temp.path()).expect("temporary directory should open").filter_map(Result::ok).filter(|entry| entry.path().extension().and_then(|value| value.to_str()) == Some("sqlite")).count();

    let second = open_legacy(&path).expect("second open should validate checkpoint");
    assert_eq!(second.journal_event_count().expect("journal count should load"), event_count);
    assert!(backup.exists());
    assert_eq!(fs::read_dir(temp.path()).expect("temporary directory should open").filter_map(Result::ok).filter(|entry| entry.path().extension().and_then(|value| value.to_str()) == Some("sqlite")).count(), backup_count, "second open must not create another backup");
    assert!(Connection::open(&path)
        .expect("migrated database should open")
        .query_row("SELECT EXISTS(SELECT 1 FROM schema_migrations WHERE version = ?1)", [CHECKPOINT], |row| row.get::<_, bool>(0))
        .expect("checkpoint query should work"));
}


#[test]
fn new_and_in_memory_databases_skip_legacy_migration() {
    let temp = TempDir::new().expect("temporary directory should exist");
    let path = temp.path().join("new.sqlite");
    let persistent = Store::open(&path).expect("new persistent database should initialize v2 directly");
    assert_eq!(persistent.journal_event_count().expect("journal count should load"), 0);
    assert!(!persistent.has_table("agents").expect("schema lookup should work"));
    assert!(!backup_path(&path).exists());

    let memory = Store::open_in_memory().expect("in-memory database should initialize v2 directly");
    assert_eq!(memory.journal_event_count().expect("journal count should load"), 0);
    assert!(!memory.has_table("agents").expect("schema lookup should work"));
}
