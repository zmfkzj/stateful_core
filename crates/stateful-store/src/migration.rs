use crate::{
    StoreError, StoreResult,
    clock::Clock,
    journal::{
        JournalEvent, MigrationJournalMetadata, append_migration_event, load_journal_events,
        projection_snapshot,
    },
    projector::Projector,
    schema,
};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, backup::Backup, params};
use serde_json::{Map, Value, json};
use stateful_core::{
    EventData, EventPayload, LEGACY_MIGRATION_NAMESPACE, MigrationEvent, NewEvent, ReservationScope,
};
use std::{
    collections::BTreeMap,
    fs::{self, File},
    path::{Path, PathBuf},
    time::Duration,
};
use time::{OffsetDateTime, format_description::well_known::Rfc3339};
use uuid::Uuid;

use fs2::FileExt;
#[cfg(test)]
use std::sync::{Arc, LazyLock, Mutex};

const CHECKPOINT: &str = "stateful.v2.event-journal";
const SHADOW_PREFIX: &str = "_v2_shadow_";
const REPAIR_PREFIX: &str = "_v2_repair_";
const REPLAY_PREFIX: &str = "_v2_replay_";
const LEGACY_TABLES: &[&str] = &[
    "events",
    "agents",
    "activities",
    "reservations",
    "claims",
    "write_fences",
    "human_observations",
    "wait_queue",
    "notifications",
    "outbox",
];
const REQUIRED_LEGACY_COLUMNS: &[(&str, &[&str])] = &[
    ("schema_migrations", &["version", "applied_at"]),
    (
        "events",
        &[
            "event_id",
            "event_type",
            "agent_id",
            "workspace_id",
            "payload_json",
            "created_at",
        ],
    ),
    ("agents", &["agent_id", "workspace_id", "updated_at"]),
    (
        "activities",
        &[
            "activity_id",
            "agent_id",
            "workspace_id",
            "phase",
            "expires_at",
        ],
    ),
    (
        "reservations",
        &[
            "reservation_id",
            "agent_id",
            "workspace_id",
            "purpose",
            "scopes_json",
            "status",
            "declared_at",
            "expires_at",
        ],
    ),
    (
        "claims",
        &[
            "claim_id",
            "reservation_id",
            "agent_id",
            "workspace_id",
            "relative_path",
            "action",
            "status",
            "expires_at",
            "observed_exists",
            "observed_content_hash",
        ],
    ),
    (
        "write_fences",
        &[
            "fence_id",
            "agent_id",
            "workspace_id",
            "relative_path",
            "action",
            "acquired_at",
            "expires_at",
            "released_at",
        ],
    ),
    (
        "human_observations",
        &[
            "observation_id",
            "workspace_id",
            "relative_path",
            "kind",
            "source",
            "confidence",
            "observed_exists",
            "observed_content_hash",
            "observed_at",
            "summary",
            "expires_at",
            "reconciled_at",
            "reconcile_decision",
            "reconciled_by_agent_id",
        ],
    ),
    (
        "wait_queue",
        &[
            "wait_id",
            "request_id",
            "agent_id",
            "workspace_id",
            "relative_path",
            "action",
            "status",
            "requested_at",
            "reservation_expires_at",
            "blocking_agent_id",
            "purpose",
        ],
    ),
    (
        "notifications",
        &[
            "notification_id",
            "sequence",
            "target_agent_id",
            "workspace_id",
            "kind",
            "payload_json",
            "status",
            "created_at",
            "expires_at",
        ],
    ),
    (
        "outbox",
        &[
            "outbox_id",
            "agent_id",
            "workspace_id",
            "sequence",
            "event_type",
            "payload_json",
            "sync_status",
        ],
    ),
];
#[cfg(test)]
type MigrationTestHook = Arc<dyn Fn() + Send + Sync>;
#[cfg(test)]
static MIGRATION_TEST_HOOK: LazyLock<Mutex<Option<MigrationTestHook>>> =
    LazyLock::new(|| Mutex::new(None));
#[cfg(test)]
static MIGRATION_GUARD_TEST_HOOK: LazyLock<Mutex<Option<MigrationTestHook>>> =
    LazyLock::new(|| Mutex::new(None));

#[derive(Clone)]
struct Metadata {
    agent_id: String,
    workspace_id: String,
    repo_id: String,
    worktree_id: String,
    root: String,
    branch: String,
}

impl Metadata {
    fn journal(&self) -> MigrationJournalMetadata<'_> {
        MigrationJournalMetadata {
            agent_id: &self.agent_id,
            workspace_id: &self.workspace_id,
            repo_id: &self.repo_id,
            worktree_id: &self.worktree_id,
            root: &self.root,
            branch: &self.branch,
        }
    }
}

struct PendingEvent {
    payload: EventPayload,
    occurred_at: String,
    metadata: Metadata,
}

pub(crate) struct MigrationGuard(File);

impl MigrationGuard {
    pub(crate) fn acquire(database_path: &Path) -> StoreResult<Self> {
        let file = File::options()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(database_path.with_extension("v2.migration.lock"))?;
        #[cfg(test)]
        match file.try_lock_exclusive() {
            Ok(()) => return Ok(Self(file)),
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                if let Some(hook) = MIGRATION_GUARD_TEST_HOOK
                    .lock()
                    .expect("migration guard hook lock should not poison")
                    .clone()
                {
                    hook();
                }
            }
            Err(error) => {
                return Err(StoreError::MigrationValidation(format!(
                    "could not acquire migration guard: {error}"
                )));
            }
        }
        file.lock_exclusive().map_err(|error| {
            StoreError::MigrationValidation(format!("could not acquire migration guard: {error}"))
        })?;
        Ok(Self(file))
    }
}

impl Drop for MigrationGuard {
    fn drop(&mut self) {
        let _ = FileExt::unlock(&self.0);
    }
}

pub(crate) fn migrate_persistent_v1(
    connection: &Connection,
    database_path: &Path,
    clock: &dyn Clock,
) -> StoreResult<()> {
    loop {
        if has_checkpoint(connection)? {
            validate_ready(connection)?;
            return Ok(());
        }
        if !has_legacy_tables(connection)? {
            return Ok(());
        }

        preflight(connection)?;
        let source_data_version = data_version(connection)?;
        let backup = create_backup(connection, database_path)?;
        #[cfg(test)]
        if let Some(hook) = MIGRATION_TEST_HOOK
            .lock()
            .expect("migration hook lock should not poison")
            .clone()
        {
            hook();
        }
        connection.execute_batch("BEGIN EXCLUSIVE")?;
        if data_version(connection)? != source_data_version {
            connection.execute_batch("ROLLBACK")?;
            fs::remove_file(&backup)?;
            continue;
        }
        let result = (|| {
            preflight(connection)?;
            let now = format_timestamp(clock.now())?;
            let pending = collect_pending_events(connection, &now)?;

            schema::create_v2_schema(connection)?;
            schema::create_projection_tables_with_prefix(connection, SHADOW_PREFIX)?;
            let mut projector = Projector::new(connection, SHADOW_PREFIX, None);
            let request_id =
                Uuid::new_v5(&LEGACY_MIGRATION_NAMESPACE, b"stateful.v1-to-v2-migration");
            for (ordinal, pending) in pending.into_iter().enumerate() {
                let event = NewEvent::new(
                    request_id,
                    ordinal as u32,
                    parse_timestamp(&pending.occurred_at)?,
                    pending.payload,
                )?;
                let event = append_migration_event(connection, &event, pending.metadata.journal())?;
                projector.apply(&event)?;
            }

            replay_and_compare(connection)?;
            append_lifecycle_event(connection, &mut projector, request_id, "validated", &now)?;
            append_lifecycle_event(connection, &mut projector, request_id, "completed", &now)?;
            replay_and_compare(connection)?;

            schema::replace_projections_from_prefix(connection, SHADOW_PREFIX)?;
            schema::drop_projection_tables_with_prefix(connection, REPLAY_PREFIX)?;
            connection.execute(
                "INSERT INTO schema_migrations (version, applied_at) VALUES (?1, ?2)",
                params![CHECKPOINT, now],
            )?;
            for table in LEGACY_TABLES {
                connection.execute_batch(&format!("DROP TABLE {table};"))?;
            }
            if !backup.exists() {
                return Err(StoreError::MigrationValidation(
                    "SQLite backup disappeared before cutover".into(),
                ));
            }
            Ok(())
        })();
        if let Err(error) = result {
            let _ = connection.execute_batch("ROLLBACK");
            return Err(error);
        }
        connection.execute_batch("COMMIT")?;
        return validate_ready(connection);
    }
}

fn data_version(connection: &Connection) -> StoreResult<i64> {
    connection
        .query_row("PRAGMA data_version", [], |row| row.get(0))
        .map_err(StoreError::from)
}

fn has_checkpoint(connection: &Connection) -> StoreResult<bool> {
    if !has_table(connection, "schema_migrations")? {
        return Ok(false);
    }
    connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM schema_migrations WHERE version = ?1)",
            [CHECKPOINT],
            |row| row.get(0),
        )
        .map_err(StoreError::from)
}

pub(crate) fn repair_v2_terminal_seed_projections(connection: &Connection) -> StoreResult<()> {
    if !has_checkpoint(connection)? {
        return Ok(());
    }
    let transaction =
        rusqlite::Transaction::new_unchecked(connection, TransactionBehavior::Immediate)?;
    let events = load_journal_events(&transaction)?;
    let terminal_events: Vec<_> = events
        .iter()
        .filter(|event| migration_snapshot_is_terminal(event))
        .collect();
    if terminal_events.is_empty() {
        transaction.commit()?;
        return Ok(());
    }

    let mut canonical_projector = Projector::new(&transaction, "", None);
    for event in terminal_events {
        canonical_projector.apply(event)?;
    }
    let canonical = projection_snapshot(&transaction, "")?;
    schema::create_projection_tables_with_prefix(&transaction, REPAIR_PREFIX)?;
    let mut replay_projector = Projector::new(&transaction, REPAIR_PREFIX, None);
    for event in &events {
        replay_projector.apply(event)?;
    }
    let replay = projection_snapshot(&transaction, REPAIR_PREFIX)?;
    const DERIVED_TABLES: [&str; 2] = ["workspace_version", "agent_context_cursor"];
    for table in schema::PROJECTION_TABLES {
        if !DERIVED_TABLES.contains(table) && canonical.get(*table) != replay.get(*table) {
            return Err(StoreError::ReplayMismatch);
        }
    }
    let derived_to_replace: Vec<_> = DERIVED_TABLES
        .into_iter()
        .filter(|table| canonical.get(*table) != replay.get(*table))
        .collect();
    if !derived_to_replace.is_empty() {
        schema::replace_projection_tables_from_prefix(
            &transaction,
            REPAIR_PREFIX,
            &derived_to_replace,
        )?;
    }
    schema::drop_projection_tables_with_prefix(&transaction, REPAIR_PREFIX)?;
    if projection_snapshot(&transaction, "")? != replay {
        return Err(StoreError::ReplayMismatch);
    }
    transaction.commit()?;
    Ok(())
}

fn migration_snapshot_is_terminal(event: &JournalEvent) -> bool {
    let status = match event.stored.payload() {
        EventPayload::Migration(MigrationEvent::ClaimSnapshotSeeded(data))
        | EventPayload::Migration(MigrationEvent::WriteFenceSnapshotSeeded(data)) => {
            data.data.get("status").and_then(Value::as_str)
        }
        _ => return false,
    };
    status.is_some_and(|status| matches!(status, "released" | "expired" | "cancelled"))
}

fn has_legacy_tables(connection: &Connection) -> StoreResult<bool> {
    LEGACY_TABLES.iter().try_fold(false, |found, table| {
        has_table(connection, table).map(|exists| found || exists)
    })
}

fn has_table(connection: &Connection, table: &str) -> StoreResult<bool> {
    connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1)",
            [table],
            |row| row.get(0),
        )
        .map_err(StoreError::from)
}

fn preflight(connection: &Connection) -> StoreResult<()> {
    let quick_check: String = connection.query_row("PRAGMA quick_check", [], |row| row.get(0))?;
    if quick_check != "ok" {
        return invalid(format!("SQLite quick_check failed: {quick_check}"));
    }
    if connection
        .query_row("PRAGMA foreign_key_check", [], |_| Ok(()))
        .optional()?
        .is_some()
    {
        return invalid("SQLite foreign_key_check reported a violation");
    }
    for (table, columns) in REQUIRED_LEGACY_COLUMNS {
        let available = table_columns(connection, table)?;
        for column in *columns {
            if !available.iter().any(|available| available == column) {
                return invalid(format!(
                    "unsupported v1 schema: {table}.{column} is missing"
                ));
            }
        }
    }
    let v1_checkpoint: bool = connection.query_row(
        "SELECT EXISTS(SELECT 1 FROM schema_migrations WHERE version = 'stateful.v1.initial')",
        [],
        |row| row.get(0),
    )?;
    if !v1_checkpoint {
        return invalid("unsupported v1 schema checkpoint");
    }
    for (table, primary_key) in [
        ("events", "event_id"),
        ("agents", "agent_id"),
        ("activities", "activity_id"),
        ("reservations", "reservation_id"),
        ("claims", "claim_id"),
        ("write_fences", "fence_id"),
        ("human_observations", "observation_id"),
        ("wait_queue", "wait_id"),
        ("notifications", "notification_id"),
        ("outbox", "outbox_id"),
    ] {
        let bad: bool = connection.query_row(
            &format!("SELECT EXISTS(SELECT 1 FROM {table} WHERE {primary_key} IS NULL OR trim({primary_key}) = '')"),
            [],
            |row| row.get(0),
        )?;
        if bad {
            return invalid(format!("legacy {table} has an empty primary key"));
        }
    }
    for (table, column) in [
        ("events", "payload_json"),
        ("reservations", "scopes_json"),
        ("notifications", "payload_json"),
        ("outbox", "payload_json"),
    ] {
        validate_json_column(connection, table, column)?;
    }
    for (table, column) in [
        ("events", "created_at"),
        ("agents", "updated_at"),
        ("reservations", "declared_at"),
        ("activities", "expires_at"),
        ("reservations", "expires_at"),
        ("claims", "expires_at"),
        ("write_fences", "acquired_at"),
        ("write_fences", "expires_at"),
        ("write_fences", "released_at"),
        ("human_observations", "observed_at"),
        ("human_observations", "expires_at"),
        ("human_observations", "reconciled_at"),
        ("wait_queue", "requested_at"),
        ("wait_queue", "reservation_expires_at"),
        ("notifications", "created_at"),
        ("notifications", "expires_at"),
    ] {
        validate_timestamp_column(connection, table, column)?;
    }
    Ok(())
}

fn table_columns(connection: &Connection, table: &str) -> StoreResult<Vec<String>> {
    let mut statement = connection.prepare(&format!("PRAGMA table_info({table})"))?;
    statement
        .query_map([], |row| row.get(1))?
        .collect::<Result<Vec<String>, _>>()
        .map_err(StoreError::from)
}

fn validate_json_column(connection: &Connection, table: &str, column: &str) -> StoreResult<()> {
    let mut statement = connection.prepare(&format!("SELECT {column} FROM {table}"))?;
    for value in statement.query_map([], |row| row.get::<_, String>(0))? {
        serde_json::from_str::<Value>(&value?).map_err(|error| {
            StoreError::MigrationValidation(format!(
                "legacy {table}.{column} is invalid JSON: {error}"
            ))
        })?;
    }
    Ok(())
}

fn validate_timestamp_column(
    connection: &Connection,
    table: &str,
    column: &str,
) -> StoreResult<()> {
    let mut statement = connection.prepare(&format!(
        "SELECT {column} FROM {table} WHERE {column} IS NOT NULL"
    ))?;
    for value in statement.query_map([], |row| row.get::<_, String>(0))? {
        parse_timestamp(&value?).map_err(|_| {
            StoreError::MigrationValidation(format!("legacy {table}.{column} is not RFC3339"))
        })?;
    }
    Ok(())
}

fn create_backup(connection: &Connection, source: &Path) -> StoreResult<PathBuf> {
    let backup_path = next_backup_path(source);
    let mut destination = Connection::open(&backup_path)?;
    let backup = Backup::new(connection, &mut destination)?;
    backup.run_to_completion(32, Duration::from_millis(1), None)?;
    fs::set_permissions(&backup_path, fs::metadata(source)?.permissions())?;
    Ok(backup_path)
}

fn next_backup_path(source: &Path) -> PathBuf {
    let first = source.with_extension("v1.backup.sqlite");
    if !first.exists() {
        return first;
    }
    for ordinal in 1.. {
        let candidate = source.with_extension(format!("v1.backup.{ordinal}.sqlite"));
        if !candidate.exists() {
            return candidate;
        }
    }
    unreachable!("an unused backup path always exists")
}

fn collect_pending_events(connection: &Connection, now: &str) -> StoreResult<Vec<PendingEvent>> {
    let contexts = workspace_contexts(connection)?;
    let mut pending = vec![PendingEvent {
        payload: EventPayload::Migration(MigrationEvent::Started(EventData {
            aggregate_id: CHECKPOINT.into(),
            repeated: false,
            data: json!({"from": "stateful.v1.initial", "to": CHECKPOINT}),
        })),
        occurred_at: now.into(),
        metadata: default_metadata("unknown", "unknown", &contexts),
    }];

    append_audits(connection, &contexts, &mut pending)?;
    append_presence_seeds(connection, now, &contexts, &mut pending)?;
    append_reservation_seeds(connection, now, &contexts, &mut pending)?;
    append_claim_seeds(connection, now, &contexts, &mut pending)?;
    append_wait_seeds(connection, &contexts, &mut pending)?;
    append_fence_seeds(connection, now, &contexts, &mut pending)?;
    append_human_seeds(connection, &contexts, &mut pending)?;
    append_handoff_seeds(connection, &contexts, &mut pending)?;
    append_delivery_seeds(connection, &contexts, &mut pending)?;
    Ok(pending)
}

fn append_audits(
    connection: &Connection,
    contexts: &BTreeMap<String, Metadata>,
    pending: &mut Vec<PendingEvent>,
) -> StoreResult<()> {
    let mut statement = connection.prepare("SELECT event_id, event_type, agent_id, workspace_id, sequence, repo_id, worktree_id, root, branch, payload_json, created_at FROM events ORDER BY created_at, event_id")?;
    for row in statement.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, String>(3)?,
            row.get::<_, Option<i64>>(4)?,
            row.get::<_, Option<String>>(5)?,
            row.get::<_, Option<String>>(6)?,
            row.get::<_, Option<String>>(7)?,
            row.get::<_, Option<String>>(8)?,
            row.get::<_, String>(9)?,
            row.get::<_, String>(10)?,
        ))
    })? {
        let (
            event_id,
            event_type,
            agent_id,
            workspace_id,
            legacy_sequence,
            repo_id,
            worktree_id,
            root,
            branch,
            payload_json,
            created_at,
        ) = row?;
        pending.push(PendingEvent {
            payload: EventPayload::Migration(MigrationEvent::LegacyAuditImported(EventData {
                aggregate_id: format!("legacy-audit:{event_id}"),
                repeated: false,
                data: json!({"legacy_event_id": event_id, "legacy_event_type": event_type, "legacy_sequence": legacy_sequence, "non_projectable": true, "legacy_payload": serde_json::from_str::<Value>(&payload_json)?}),
            })),
            occurred_at: created_at,
            metadata: metadata(&workspace_id, &agent_id, repo_id, worktree_id, root, branch, contexts),
        });
    }
    Ok(())
}

fn append_presence_seeds(
    connection: &Connection,
    now: &str,
    contexts: &BTreeMap<String, Metadata>,
    pending: &mut Vec<PendingEvent>,
) -> StoreResult<()> {
    let mut agents = connection.prepare(
        "SELECT agent_id, workspace_id, updated_at FROM agents ORDER BY workspace_id, agent_id",
    )?;
    for row in agents.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
        ))
    })? {
        let (agent_id, workspace_id, updated_at) = row?;
        let now = parse_timestamp(now)?;
        let mut activities = connection.prepare("SELECT activity_id, phase, expires_at FROM activities WHERE agent_id = ?1 AND workspace_id = ?2 ORDER BY activity_id")?;
        let rows = activities
            .query_map(params![agent_id, workspace_id], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<String>>(2)?,
                ))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        let mut values = Vec::new();
        for row in rows {
            if row
                .2
                .as_deref()
                .map(parse_timestamp)
                .transpose()?
                .is_none_or(|expires_at| expires_at > now)
            {
                values.push(row);
            }
        }
        let source_activity_ids = values
            .iter()
            .map(|(activity_id, _, _)| activity_id.clone())
            .collect::<Vec<_>>();
        let selected = latest_activity(&values)?;
        let data = json!({
            "agent_id": agent_id,
            "selected_activity_id": selected.map(|value| value.0.clone()),
            "source_activity_ids": source_activity_ids,
            "phase": selected.map(|value| value.1.clone()).unwrap_or_else(|| "unknown".into()),
            "expires_at": selected.and_then(|value| value.2.clone()),
        });
        pending.push(PendingEvent {
            payload: EventPayload::Migration(MigrationEvent::PresenceSnapshotSeeded(seed_data(
                "presence", &agent_id, data,
            ))),
            occurred_at: updated_at,
            metadata: metadata(&workspace_id, &agent_id, None, None, None, None, contexts),
        });
    }
    Ok(())
}

fn latest_activity(
    values: &[(String, String, Option<String>)],
) -> StoreResult<Option<&(String, String, Option<String>)>> {
    type LatestActivity<'a> = (
        Option<OffsetDateTime>,
        &'a str,
        &'a (String, String, Option<String>),
    );

    let mut selected: Option<LatestActivity<'_>> = None;
    for value in values {
        let expires_at = value.2.as_deref().map(parse_timestamp).transpose()?;
        let replaces = match selected.as_ref() {
            None => true,
            Some((current_expires_at, current_id, _)) => {
                expires_at > *current_expires_at
                    || (expires_at == *current_expires_at && value.0.as_str() > *current_id)
            }
        };
        if replaces {
            selected = Some((expires_at, value.0.as_str(), value));
        }
    }
    Ok(selected.map(|(_, _, value)| value))
}

fn append_reservation_seeds(
    connection: &Connection,
    now: &str,
    contexts: &BTreeMap<String, Metadata>,
    pending: &mut Vec<PendingEvent>,
) -> StoreResult<()> {
    let now = parse_timestamp(now)?;
    let mut claim_scopes = BTreeMap::<(String, String), Vec<ReservationScope>>::new();
    let mut claims = connection.prepare(
        "SELECT workspace_id, reservation_id, relative_path, action, expires_at FROM claims
         WHERE reservation_id IS NOT NULL AND relative_path IS NOT NULL AND status = 'active'
         ORDER BY workspace_id, reservation_id, claim_id",
    )?;
    for row in claims.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, String>(3)?,
            row.get::<_, Option<String>>(4)?,
        ))
    })? {
        let (workspace_id, reservation_id, relative_path, action, expires_at) = row?;
        if expires_at
            .as_deref()
            .map(parse_timestamp)
            .transpose()?
            .is_some_and(|expires_at| expires_at <= now)
        {
            continue;
        }
        let scope = if action == "write_directory" || relative_path.ends_with('/') {
            ReservationScope::directory(relative_path)
        } else {
            ReservationScope::file(relative_path)
        };
        claim_scopes
            .entry((workspace_id, reservation_id))
            .or_default()
            .push(scope);
    }

    let mut statement = connection.prepare("SELECT reservation_id, agent_id, workspace_id, purpose, scopes_json, status, declared_at, expires_at FROM reservations ORDER BY workspace_id, reservation_id")?;
    for row in statement.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, String>(3)?,
            row.get::<_, String>(4)?,
            row.get::<_, String>(5)?,
            row.get::<_, String>(6)?,
            row.get::<_, Option<String>>(7)?,
        ))
    })? {
        let (
            reservation_id,
            agent_id,
            workspace_id,
            purpose,
            scopes_json,
            status,
            declared_at,
            expires_at,
        ) = row?;
        let mut scopes =
            serde_json::from_str::<Vec<ReservationScope>>(&scopes_json).map_err(|error| {
                StoreError::MigrationValidation(format!(
                    "legacy reservation {reservation_id} scopes are invalid: {error}"
                ))
            })?;
        scopes.extend(
            claim_scopes
                .remove(&(workspace_id.clone(), reservation_id.clone()))
                .unwrap_or_default(),
        );
        scopes.sort_by_key(migration_scope_key);
        scopes.dedup_by(|left, right| migration_scope_key(left) == migration_scope_key(right));
        if scopes.is_empty() {
            return Err(StoreError::MigrationValidation(format!(
                "legacy reservation {reservation_id} has no scopes"
            )));
        }
        let action = if matches!(scopes[0], ReservationScope::Directory(_)) {
            "write_directory"
        } else {
            "write_file"
        };
        pending.push(PendingEvent {
            payload: EventPayload::Migration(MigrationEvent::ReservationSnapshotSeeded(seed_data(
                "reservation",
                &reservation_id,
                json!({
                    "reservation_id": reservation_id,
                    "agent_id": agent_id,
                    "workspace_id": workspace_id,
                    "scopes": scopes,
                    "action": action,
                    "purpose": purpose,
                    "status": status,
                    "declared_at": declared_at,
                    "expires_at": expires_at,
                    "max_expires_at": expires_at,
                    "wait_id": null,
                }),
            ))),
            occurred_at: declared_at,
            metadata: metadata(&workspace_id, &agent_id, None, None, None, None, contexts),
        });
    }
    Ok(())
}

fn migration_scope_key(scope: &ReservationScope) -> String {
    match scope {
        ReservationScope::File(path) => format!("file:{path}"),
        ReservationScope::Directory(path) => format!("directory:{path}"),
    }
}

fn append_claim_seeds(
    connection: &Connection,
    now: &str,
    contexts: &BTreeMap<String, Metadata>,
    pending: &mut Vec<PendingEvent>,
) -> StoreResult<()> {
    let mut statement = connection.prepare("SELECT claim_id, reservation_id, agent_id, workspace_id, repo_id, relative_path, absolute_path, purpose, action, status, expires_at, observed_exists, observed_content_hash FROM claims ORDER BY workspace_id, claim_id")?;
    for row in statement.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, Option<String>>(1)?,
            row.get::<_, Option<String>>(2)?,
            row.get::<_, String>(3)?,
            row.get::<_, Option<String>>(4)?,
            row.get::<_, Option<String>>(5)?,
            row.get::<_, Option<String>>(6)?,
            row.get::<_, Option<String>>(7)?,
            row.get::<_, String>(8)?,
            row.get::<_, String>(9)?,
            row.get::<_, Option<String>>(10)?,
            row.get::<_, Option<i64>>(11)?,
            row.get::<_, Option<String>>(12)?,
        ))
    })? {
        let (
            claim_id,
            reservation_id,
            agent_id,
            workspace_id,
            repo_id,
            relative_path,
            _absolute_path,
            _purpose,
            action,
            status,
            expires_at,
            observed_exists,
            observed_content_hash,
        ) = row?;
        let agent_id = agent_id.unwrap_or_else(|| "unknown".into());
        let status = if status == "active"
            && expires_at
                .as_deref()
                .map(parse_timestamp)
                .transpose()?
                .is_some_and(|expires_at| {
                    expires_at <= parse_timestamp(now).expect("preflight validated migration clock")
                }) {
            "expired"
        } else {
            status.as_str()
        };
        let legacy_base_observation =
            json!({"exists": observed_exists, "content_hash": observed_content_hash.clone()});
        let occurred_at = expires_at.clone().unwrap_or_else(|| now.into());
        let reservation_id =
            reservation_id.unwrap_or_else(|| format!("legacy-reservation-{claim_id}"));
        let relative_path = relative_path.unwrap_or_else(|| "unknown".into());
        let observation = observed_exists.map(|exists| {
            json!({
                "exists": exists != 0,
                "content_hash": observed_content_hash,
            })
        });
        pending.push(PendingEvent {
            payload: EventPayload::Migration(MigrationEvent::ClaimSnapshotSeeded(seed_data(
                "claim",
                &claim_id,
                json!({
                    "claim_id": claim_id,
                    "reservation_id": reservation_id,
                    "agent_id": agent_id,
                    "workspace_id": workspace_id,
                    "relative_path": relative_path,
                    "action": action,
                    "status": status,
                    "acquired_at": now,
                    "expires_at": expires_at,
                    "observation": observation,
                    "legacy_base_observation": legacy_base_observation,
                }),
            ))),
            occurred_at,
            metadata: metadata(
                &workspace_id,
                &agent_id,
                repo_id,
                None,
                None,
                None,
                contexts,
            ),
        });
    }
    Ok(())
}

fn append_wait_seeds(
    connection: &Connection,
    contexts: &BTreeMap<String, Metadata>,
    pending: &mut Vec<PendingEvent>,
) -> StoreResult<()> {
    let mut statement = connection.prepare("SELECT wait_id, request_id, agent_id, workspace_id, repo_id, worktree_id, root, branch, relative_path, action, status, requested_at, reservation_expires_at, blocking_agent_id, purpose FROM wait_queue")?;
    let mut rows = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, Option<String>>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, Option<String>>(4)?,
                row.get::<_, Option<String>>(5)?,
                row.get::<_, Option<String>>(6)?,
                row.get::<_, Option<String>>(7)?,
                row.get::<_, String>(8)?,
                row.get::<_, String>(9)?,
                row.get::<_, String>(10)?,
                row.get::<_, String>(11)?,
                row.get::<_, Option<String>>(12)?,
                row.get::<_, Option<String>>(13)?,
                row.get::<_, String>(14)?,
            ))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    rows.sort_by_key(|row| {
        (
            row.3.clone(),
            parse_timestamp(&row.11).expect("preflight validated wait timestamp"),
            row.0.clone(),
        )
    });
    for row in rows {
        let (
            wait_id,
            request_id,
            agent_id,
            workspace_id,
            repo_id,
            worktree_id,
            root,
            branch,
            relative_path,
            action,
            status,
            requested_at,
            reservation_expires_at,
            blocking_agent_id,
            purpose,
        ) = row;
        let status = if status == "waiting" {
            "queued"
        } else {
            status.as_str()
        };
        pending.push(PendingEvent {
            payload: EventPayload::Migration(MigrationEvent::WaitSnapshotSeeded(seed_data(
                "wait",
                &wait_id,
                json!({
                    "wait_id": wait_id,
                    "request_id": request_id.unwrap_or_else(|| format!("migration-{wait_id}")),
                    "agent_id": agent_id,
                    "workspace_id": workspace_id,
                    "relative_path": relative_path,
                    "action": action,
                    "status": status,
                    "requested_at": requested_at,
                    "reservation_expires_at": reservation_expires_at,
                    "blocking_agent_id": blocking_agent_id,
                    "reservation_id": null,
                    "purpose": purpose,
                }),
            ))),
            occurred_at: requested_at,
            metadata: metadata(
                &workspace_id,
                &agent_id,
                repo_id,
                worktree_id,
                root,
                branch,
                contexts,
            ),
        });
    }
    Ok(())
}

fn append_fence_seeds(
    connection: &Connection,
    now: &str,
    contexts: &BTreeMap<String, Metadata>,
    pending: &mut Vec<PendingEvent>,
) -> StoreResult<()> {
    let mut statement = connection.prepare("SELECT fence_id, agent_id, workspace_id, relative_path, action, acquired_at, expires_at, released_at FROM write_fences ORDER BY workspace_id, fence_id")?;
    for row in statement.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, String>(3)?,
            row.get::<_, String>(4)?,
            row.get::<_, String>(5)?,
            row.get::<_, String>(6)?,
            row.get::<_, Option<String>>(7)?,
        ))
    })? {
        let (
            fence_id,
            agent_id,
            workspace_id,
            relative_path,
            action,
            acquired_at,
            expires_at,
            released_at,
        ) = row?;
        let status = if released_at.is_some() {
            "released"
        } else if parse_timestamp(&expires_at)? <= parse_timestamp(now)? {
            "expired"
        } else {
            "active"
        };
        pending.push(PendingEvent {
            payload: EventPayload::Migration(MigrationEvent::WriteFenceSnapshotSeeded(seed_data(
                "write_fence",
                &fence_id,
                json!({
                    "fence_id": fence_id,
                    "agent_id": agent_id,
                    "workspace_id": workspace_id,
                    "relative_path": relative_path,
                    "action": action,
                    "status": status,
                    "acquired_at": acquired_at,
                    "expires_at": expires_at,
                    "released_at": released_at,
                }),
            ))),
            occurred_at: acquired_at,
            metadata: metadata(&workspace_id, &agent_id, None, None, None, None, contexts),
        });
    }
    Ok(())
}

fn append_human_seeds(
    connection: &Connection,
    contexts: &BTreeMap<String, Metadata>,
    pending: &mut Vec<PendingEvent>,
) -> StoreResult<()> {
    let mut statement = connection.prepare("SELECT observation_id, workspace_id, relative_path, kind, source, confidence, observed_exists, observed_content_hash, observed_at, summary, expires_at, reconciled_at, reconcile_decision, reconciled_by_agent_id FROM human_observations ORDER BY workspace_id, observation_id")?;
    for row in statement.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, String>(3)?,
            row.get::<_, String>(4)?,
            row.get::<_, String>(5)?,
            row.get::<_, i64>(6)?,
            row.get::<_, Option<String>>(7)?,
            row.get::<_, String>(8)?,
            row.get::<_, String>(9)?,
            row.get::<_, Option<String>>(10)?,
            row.get::<_, Option<String>>(11)?,
            row.get::<_, Option<String>>(12)?,
            row.get::<_, Option<String>>(13)?,
        ))
    })? {
        let (
            observation_id,
            workspace_id,
            relative_path,
            kind,
            source,
            confidence,
            observed_exists,
            observed_content_hash,
            observed_at,
            summary,
            expires_at,
            reconciled_at,
            reconcile_decision,
            reconciled_by_agent_id,
        ) = row?;
        let kind = match kind.as_str() {
            "save" | "change" | "delete" | "presence" | "dirty" => kind,
            _ => "change".into(),
        };
        let confidence = if confidence == "high" { "high" } else { "low" };
        let status = if reconciled_at.is_some() {
            "reconciled"
        } else {
            "pending"
        };
        let legacy_observation = json!({
            "exists": observed_exists != 0,
            "content_hash": observed_content_hash,
        });
        pending.push(PendingEvent {
            payload: EventPayload::Migration(MigrationEvent::HumanObservationSnapshotSeeded(
                seed_data(
                    "human_observation",
                    &observation_id,
                    json!({
                        "observation_id": observation_id,
                        "workspace_id": workspace_id,
                        "relative_path": relative_path,
                        "kind": kind,
                        "source": source,
                        "confidence": confidence,
                        "observed_at": observed_at,
                        "summary": summary,
                        "status": status,
                        "legacy_observation": legacy_observation,
                        "expires_at": expires_at,
                        "reconciled_at": reconciled_at,
                        "decision": reconcile_decision,
                        "reconciled_by_agent_id": reconciled_by_agent_id,
                    }),
                ),
            )),
            occurred_at: observed_at,
            metadata: metadata(&workspace_id, "unknown", None, None, None, None, contexts),
        });
    }
    Ok(())
}

fn append_handoff_seeds(
    connection: &Connection,
    contexts: &BTreeMap<String, Metadata>,
    pending: &mut Vec<PendingEvent>,
) -> StoreResult<()> {
    let mut statement = connection.prepare("SELECT event_id, agent_id, workspace_id, payload_json, created_at FROM events WHERE event_type = 'ActivityFinalized' ORDER BY created_at, event_id")?;
    for row in statement.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, String>(3)?,
            row.get::<_, String>(4)?,
        ))
    })? {
        let (event_id, agent_id, workspace_id, payload_json, created_at) = row?;
        let cleanup_count = serde_json::from_str::<Value>(&payload_json)?
            .get("cleanup_count")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        pending.push(PendingEvent {
            payload: EventPayload::Migration(MigrationEvent::LegacyHandoffSnapshotSeeded(seed_data("handoff", &event_id, json!({"status": "unknown", "actor_id": "unknown", "goal": "", "next_plan": "", "resources": [], "cleanup_count": cleanup_count, "legacy_event_id": event_id})))),
            occurred_at: created_at,
            metadata: metadata(&workspace_id, &agent_id, None, None, None, None, contexts),
        });
    }
    Ok(())
}

fn append_delivery_seeds(
    connection: &Connection,
    contexts: &BTreeMap<String, Metadata>,
    pending: &mut Vec<PendingEvent>,
) -> StoreResult<()> {
    let mut notifications = connection.prepare("SELECT notification_id, sequence, target_agent_id, workspace_id, kind, payload_json, status, created_at, expires_at FROM notifications ORDER BY workspace_id, sequence, notification_id")?;
    for row in notifications.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, i64>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, String>(3)?,
            row.get::<_, String>(4)?,
            row.get::<_, String>(5)?,
            row.get::<_, String>(6)?,
            row.get::<_, String>(7)?,
            row.get::<_, Option<String>>(8)?,
        ))
    })? {
        let (
            notification_id,
            sequence,
            target_agent_id,
            workspace_id,
            kind,
            payload_json,
            status,
            created_at,
            expires_at,
        ) = row?;
        let status = if status == "pending" {
            "queued"
        } else {
            status.as_str()
        };
        let primary_key = format!("notification:{notification_id}");
        pending.push(PendingEvent {
            payload: EventPayload::Migration(MigrationEvent::DeliverySnapshotSeeded(seed_data(
                "delivery",
                &primary_key,
                json!({
                    "delivery_kind": "notification",
                    "notification": {
                        "notification_id": notification_id,
                        "sequence": sequence,
                        "target_agent_id": target_agent_id,
                        "workspace_id": workspace_id,
                        "kind": kind,
                        "payload": serde_json::from_str::<Value>(&payload_json)?,
                        "status": status,
                        "created_at": created_at,
                        "expires_at": expires_at,
                        "coalesce_key": null,
                    },
                }),
            ))),
            occurred_at: created_at,
            metadata: metadata(&workspace_id, "unknown", None, None, None, None, contexts),
        });
    }
    let mut outbox = connection.prepare("SELECT outbox_id, agent_id, workspace_id, sequence, event_type, payload_json, sync_status FROM outbox ORDER BY workspace_id, sequence, outbox_id")?;
    for row in outbox.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, i64>(3)?,
            row.get::<_, String>(4)?,
            row.get::<_, String>(5)?,
            row.get::<_, String>(6)?,
        ))
    })? {
        let (outbox_id, _agent_id, workspace_id, sequence, event_type, payload_json, sync_status) =
            row?;
        let delivery_status = if sync_status == "delivered" {
            "delivered"
        } else {
            "queued"
        };
        let outbox_status = if sync_status == "delivered" {
            "synced"
        } else {
            "pending"
        };
        let primary_key = format!("outbox:{outbox_id}");
        pending.push(PendingEvent {
            payload: EventPayload::Migration(MigrationEvent::DeliverySnapshotSeeded(seed_data(
                "delivery",
                &primary_key,
                json!({
                    "delivery_kind": "outbox",
                    "delivery": {
                        "delivery_id": primary_key,
                        "notification_id": outbox_id,
                        "workspace_id": workspace_id,
                        "status": delivery_status,
                        "attempts": 0,
                        "last_error": null,
                        "retry_at": null,
                        "delivered_at": null,
                        "outbox": {
                            "outbox_id": outbox_id,
                            "workspace_id": workspace_id,
                            "sequence": sequence,
                            "event_type": event_type,
                            "payload": serde_json::from_str::<Value>(&payload_json)?,
                            "sync_status": outbox_status,
                            "attempts": 0,
                            "last_error": null,
                        },
                    },
                }),
            ))),
            occurred_at: "1970-01-01T00:00:00Z".into(),
            metadata: metadata(&workspace_id, "unknown", None, None, None, None, contexts),
        });
    }
    Ok(())
}

fn append_lifecycle_event(
    connection: &Connection,
    projector: &mut Projector<'_>,
    request_id: Uuid,
    lifecycle: &str,
    now: &str,
) -> StoreResult<()> {
    let manifest = json!({"normalizations": [
        "legacy audits are ordered by created_at,event_id and remain non-projectable",
        "multiple live activities select latest expiry instant then activity_id while preserving source_activity_ids",
        "legacy claim content hashes remain legacy_base_observation and never become read provenance",
        "unavailable actor fields are unknown and unavailable handoff fields are empty",
        "claims without a legacy timestamp use expires_at or the Unix epoch as occurred_at",
        "outbox rows use the Unix epoch as occurred_at because v1 stored no outbox timestamp",
    ]});
    let payload = match lifecycle {
        "validated" => EventPayload::Migration(MigrationEvent::Validated(EventData {
            aggregate_id: CHECKPOINT.into(),
            repeated: false,
            data: manifest,
        })),
        "completed" => EventPayload::Migration(MigrationEvent::Completed(EventData {
            aggregate_id: CHECKPOINT.into(),
            repeated: false,
            data: json!({"manifest": manifest}),
        })),
        _ => return invalid("unknown migration lifecycle event"),
    };
    let ordinal: u32 = connection.query_row("SELECT COUNT(*) FROM journal_events", [], |row| {
        row.get::<_, u64>(0)
    })? as u32;
    let event = NewEvent::new(request_id, ordinal, parse_timestamp(now)?, payload)?;
    let metadata = Metadata {
        agent_id: "unknown".into(),
        workspace_id: "unknown".into(),
        repo_id: "unknown".into(),
        worktree_id: "unknown".into(),
        root: "unknown".into(),
        branch: "unknown".into(),
    };
    let event = append_migration_event(connection, &event, metadata.journal())?;
    projector.apply(&event)
}

fn replay_and_compare(connection: &Connection) -> StoreResult<()> {
    schema::create_projection_tables_with_prefix(connection, REPLAY_PREFIX)?;
    let events = load_journal_events(connection)?;
    let mut projector = Projector::new(connection, REPLAY_PREFIX, None);
    for event in &events {
        projector.apply(event)?;
    }
    if projection_snapshot(connection, SHADOW_PREFIX)?
        != projection_snapshot(connection, REPLAY_PREFIX)?
    {
        return Err(StoreError::MigrationValidation(
            "shadow projection differs from replay".into(),
        ));
    }
    Ok(())
}

fn workspace_contexts(connection: &Connection) -> StoreResult<BTreeMap<String, Metadata>> {
    let mut contexts = BTreeMap::new();
    let mut statement = connection.prepare("SELECT workspace_id, agent_id, repo_id, worktree_id, root, branch FROM events ORDER BY workspace_id, created_at, event_id")?;
    for row in statement.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, Option<String>>(2)?,
            row.get::<_, Option<String>>(3)?,
            row.get::<_, Option<String>>(4)?,
            row.get::<_, Option<String>>(5)?,
        ))
    })? {
        let (workspace_id, agent_id, repo_id, worktree_id, root, branch) = row?;
        contexts
            .entry(workspace_id.clone())
            .or_insert_with(|| Metadata {
                agent_id,
                workspace_id,
                repo_id: known(repo_id),
                worktree_id: known(worktree_id),
                root: known(root),
                branch: known(branch),
            });
    }
    Ok(contexts)
}

fn metadata(
    workspace_id: &str,
    agent_id: &str,
    repo_id: Option<String>,
    worktree_id: Option<String>,
    root: Option<String>,
    branch: Option<String>,
    _contexts: &BTreeMap<String, Metadata>,
) -> Metadata {
    Metadata {
        agent_id: known(Some(agent_id.into())),
        workspace_id: known(Some(workspace_id.into())),
        repo_id: known(repo_id),
        worktree_id: known(worktree_id),
        root: known(root),
        branch: known(branch),
    }
}

fn default_metadata(
    agent_id: &str,
    workspace_id: &str,
    contexts: &BTreeMap<String, Metadata>,
) -> Metadata {
    metadata(workspace_id, agent_id, None, None, None, None, contexts)
}

fn known(value: Option<String>) -> String {
    value
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| "unknown".into())
}

fn seed_data(kind: &str, primary_key: &str, value: Value) -> EventData {
    let mut object = value.as_object().cloned().unwrap_or_else(Map::new);
    object.insert("legacy_entity_kind".into(), Value::String(kind.into()));
    object.insert(
        "legacy_primary_key".into(),
        Value::String(primary_key.into()),
    );
    EventData {
        aggregate_id: primary_key.into(),
        repeated: false,
        data: Value::Object(object),
    }
}

fn validate_ready(connection: &Connection) -> StoreResult<()> {
    if !has_checkpoint(connection)?
        || has_legacy_tables(connection)?
        || !has_table(connection, "journal_events")?
    {
        return Err(StoreError::MigrationValidation(
            "v2 migration checkpoint is not ready".into(),
        ));
    }
    for table in schema::PROJECTION_TABLES {
        if !has_table(connection, table)? {
            return Err(StoreError::MigrationValidation(format!(
                "v2 projection {table} is missing"
            )));
        }
    }
    Ok(())
}

fn parse_timestamp(timestamp: &str) -> StoreResult<OffsetDateTime> {
    OffsetDateTime::parse(timestamp, &Rfc3339).map_err(|_| {
        StoreError::MigrationValidation(format!("invalid migration timestamp: {timestamp}"))
    })
}

fn format_timestamp(timestamp: OffsetDateTime) -> StoreResult<String> {
    timestamp.format(&Rfc3339).map_err(|error| {
        StoreError::MigrationValidation(format!("could not format migration timestamp: {error}"))
    })
}

fn invalid(message: impl Into<String>) -> StoreResult<()> {
    Err(StoreError::MigrationValidation(message.into()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{FixedClock, Store};
    use std::{
        env,
        io::{self, BufRead, BufReader, Write},
        process::{Child, ChildStdout, Command, Stdio},
        sync::{
            Arc, Barrier, Mutex,
            atomic::{AtomicBool, Ordering},
        },
        thread,
    };
    use tempfile::TempDir;

    const MIGRATION_GUARD_SUBPROCESS_PATH: &str = "STATEFUL_MIGRATION_GUARD_SUBPROCESS_PATH";
    const BLOCKED: &str = "STATEFUL_MIGRATION_GUARD_BLOCKED";
    const COUNT_PREFIX: &str = "STATEFUL_MIGRATION_GUARD_COUNT=";

    fn spawn_migration_guard_opener(path: &Path) -> (Child, BufReader<ChildStdout>) {
        let mut child =
            Command::new(env::current_exe().expect("migration test executable should resolve"))
                .args([
                    "--exact",
                    "migration::tests::migration_guard_subprocess_helper",
                    "--nocapture",
                ])
                .env(MIGRATION_GUARD_SUBPROCESS_PATH, path)
                .stdout(Stdio::piped())
                .spawn()
                .expect("migration opener subprocess should start");
        let stdout = child
            .stdout
            .take()
            .expect("migration opener subprocess stdout should pipe");
        (child, BufReader::new(stdout))
    }

    fn read_subprocess_message(output: &mut BufReader<ChildStdout>, marker: &str) -> String {
        let mut line = String::new();
        loop {
            line.clear();
            assert!(
                output
                    .read_line(&mut line)
                    .expect("subprocess output should be readable")
                    > 0,
                "subprocess exited before reporting {marker}",
            );
            if let Some(index) = line.find(marker) {
                return line[index + marker.len()..].trim().into();
            }
        }
    }

    #[test]
    fn migration_guard_subprocess_helper() {
        let Some(path) = env::var_os(MIGRATION_GUARD_SUBPROCESS_PATH) else {
            return;
        };
        *MIGRATION_GUARD_TEST_HOOK
            .lock()
            .expect("migration guard hook lock should not poison") = Some(Arc::new(|| {
            let mut output = io::stdout().lock();
            writeln!(output, "{BLOCKED}")
                .expect("subprocess should report in-path migration guard contention");
            output
                .flush()
                .expect("migration guard contention marker should flush through the pipe");
        }));
        let count = Store::open_with_clock(
            Path::new(&path),
            FixedClock::new(
                OffsetDateTime::parse("2026-07-15T11:30:00Z", &Rfc3339)
                    .expect("clock should parse"),
            ),
        )
        .expect("subprocess opener should migrate or reopen successfully")
        .journal_event_count()
        .expect("subprocess opener should load the journal count");
        let mut output = io::stdout().lock();
        writeln!(output, "{COUNT_PREFIX}{count}")
            .expect("subprocess should report its journal count");
        output
            .flush()
            .expect("subprocess result should flush through the pipe");
    }

    #[test]
    fn migration_guard_blocks_independent_openers_before_sqlite_configuration() {
        let temp = TempDir::new().expect("temporary directory should exist");
        let path = temp.path().join("concurrent.sqlite");
        let fixture = Connection::open(&path).expect("fixture should open");
        fixture
            .execute_batch(include_str!("../tests/fixtures/v1_persistent_state.sql"))
            .expect("fixture should apply");
        drop(fixture);

        let held_migration_lock = File::options()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(path.with_extension("v2.migration.lock"))
            .expect("parent should create the migration lock file");
        held_migration_lock
            .lock_exclusive()
            .expect("parent should hold the migration lock");
        let migration_owner =
            Connection::open(&path).expect("parent migration connection should open");
        migration_owner
            .execute_batch("BEGIN EXCLUSIVE")
            .expect("parent should hold SQLite exclusive while the guard is held");
        let (mut first, mut first_output) = spawn_migration_guard_opener(&path);
        let (mut second, mut second_output) = spawn_migration_guard_opener(&path);

        assert_eq!(
            read_subprocess_message(&mut first_output, BLOCKED),
            "",
            "first opener must observe the production migration guard before SQLite setup",
        );
        assert_eq!(
            read_subprocess_message(&mut second_output, BLOCKED),
            "",
            "second opener must observe the production migration guard before SQLite setup",
        );
        migration_owner
            .execute_batch("ROLLBACK")
            .expect("parent should release its SQLite exclusive transaction");
        drop(migration_owner);
        drop(held_migration_lock);

        let first_count: u64 = read_subprocess_message(&mut first_output, COUNT_PREFIX)
            .parse()
            .expect("first subprocess journal count should be numeric");
        let second_count: u64 = read_subprocess_message(&mut second_output, COUNT_PREFIX)
            .parse()
            .expect("second subprocess journal count should be numeric");
        assert!(
            first
                .wait()
                .expect("first subprocess should join")
                .success(),
            "first subprocess should migrate successfully",
        );
        assert!(
            second
                .wait()
                .expect("second subprocess should join")
                .success(),
            "second subprocess should reopen successfully",
        );
        assert_eq!(
            first_count, second_count,
            "both independent openers must observe the same journal",
        );

        let migrated = Connection::open(&path).expect("migrated database should open");
        let checkpoint_count: u64 = migrated
            .query_row(
                "SELECT COUNT(*) FROM schema_migrations WHERE version = ?1",
                [CHECKPOINT],
                |row| row.get(0),
            )
            .expect("checkpoint count should load");
        let seed_count: u64 = migrated
            .query_row(
                "SELECT COUNT(*)
                 FROM journal_events
                 WHERE event_type LIKE 'migration.%_snapshot_seeded'",
                [],
                |row| row.get(0),
            )
            .expect("seed count should load");
        let distinct_seed_count: u64 = migrated
            .query_row(
                "SELECT COUNT(DISTINCT event_id)
                 FROM journal_events
                 WHERE event_type LIKE 'migration.%_snapshot_seeded'",
                [],
                |row| row.get(0),
            )
            .expect("distinct seed count should load");
        assert_eq!(
            checkpoint_count, 1,
            "only one migration checkpoint may commit"
        );
        assert!(
            path.with_extension("v1.backup.sqlite").exists(),
            "one migration backup must be retained",
        );
        assert!(
            !path.with_extension("v1.backup.1.sqlite").exists(),
            "a second opener must not retain another migration backup",
        );
        assert!(
            seed_count > 0,
            "the legacy fixture must produce snapshot seeds"
        );
        assert_eq!(
            seed_count, distinct_seed_count,
            "each deterministic snapshot seed must appear in one stream only",
        );
        for event_type in ["migration.started", "migration.completed"] {
            let count: u64 = migrated
                .query_row(
                    "SELECT COUNT(*) FROM journal_events WHERE event_type = ?1",
                    [event_type],
                    |row| row.get(0),
                )
                .expect("migration lifecycle count should load");
            assert_eq!(count, 1, "{event_type} must be appended once");
        }
    }

    #[test]
    fn writer_after_backup_retries_without_retaining_a_stale_rollback_backup() {
        let temp = TempDir::new().expect("temporary directory should exist");
        let path = temp.path().join("legacy.sqlite");
        let connection = Connection::open(&path).expect("fixture should open");
        connection
            .execute_batch(include_str!("../tests/fixtures/v1_persistent_state.sql"))
            .expect("fixture should apply");
        drop(connection);

        let backup = path.with_extension("v1.backup.sqlite");
        let backup_created = Arc::new(Barrier::new(2));
        let writer_committed = Arc::new(Barrier::new(2));
        let paused = Arc::new(AtomicBool::new(false));
        let first_backup_updated_at = Arc::new(Mutex::new(None));
        let hook_backup = backup.clone();
        let hook_backup_created = Arc::clone(&backup_created);
        let hook_writer_committed = Arc::clone(&writer_committed);
        let hook_paused = Arc::clone(&paused);
        let hook_first_backup_updated_at = Arc::clone(&first_backup_updated_at);
        *MIGRATION_TEST_HOOK
            .lock()
            .expect("migration hook lock should not poison") = Some(Arc::new(move || {
            if !hook_paused.swap(true, Ordering::SeqCst) {
                *hook_first_backup_updated_at
                    .lock()
                    .expect("backup timestamp lock should not poison") = Some(
                    Connection::open(&hook_backup)
                        .expect("first backup should open")
                        .query_row(
                            "SELECT updated_at FROM agents WHERE agent_id = 'agent-alpha'",
                            [],
                            |row| row.get::<_, String>(0),
                        )
                        .expect("first backup should contain the legacy row"),
                );
                hook_backup_created.wait();
                hook_writer_committed.wait();
            }
        }));

        let migration_path = path.clone();
        let migration = thread::spawn(move || {
            let connection =
                Connection::open(&migration_path).expect("migration connection should open");
            migrate_persistent_v1(
                &connection,
                &migration_path,
                &FixedClock::new(
                    OffsetDateTime::parse("2026-07-15T11:30:00Z", &Rfc3339)
                        .expect("clock should parse"),
                ),
            )
        });

        backup_created.wait();
        let writer_result = {
            let writer = Connection::open(&path).expect("second writer connection should open");
            writer.execute_batch(
                "BEGIN IMMEDIATE;
                 UPDATE agents
                    SET updated_at = '2026-07-15T11:20:00Z'
                  WHERE agent_id = 'agent-alpha';
                 COMMIT;",
            )
        };
        writer_committed.wait();
        writer_result.expect("second writer should commit the authoritative legacy change");
        migration
            .join()
            .expect("migration thread should join")
            .expect("migration should reject the stale backup and retry");
        *MIGRATION_TEST_HOOK
            .lock()
            .expect("migration hook lock should not poison") = None;

        assert_eq!(
            first_backup_updated_at
                .lock()
                .expect("backup timestamp lock should not poison")
                .as_deref(),
            Some("2026-07-15T11:00:00Z"),
            "the first completed backup must be the pre-writer snapshot",
        );
        assert!(
            !path.with_extension("v1.backup.1.sqlite").exists(),
            "a stale rollback backup must not survive beside the retained retry backup",
        );
        let migrated = Connection::open(&path).expect("migrated database should open");
        let journal_updated_at: String = migrated
            .query_row(
                "SELECT occurred_at
                   FROM journal_events
                  WHERE event_type = 'migration.presence_snapshot_seeded'
                    AND aggregate_id = 'agent-alpha'",
                [],
                |row| row.get(0),
            )
            .expect("journal should contain the committed legacy row");
        let canonical_updated_at: String = migrated
            .query_row(
                "SELECT occurred_at
                   FROM presence_current
                  WHERE workspace_id = 'workspace-main'
                    AND agent_id = 'agent-alpha'",
                [],
                |row| row.get(0),
            )
            .expect("canonical projection should contain the committed legacy row");
        assert_eq!(journal_updated_at, "2026-07-15T11:20:00Z");
        assert_eq!(canonical_updated_at, journal_updated_at);

        let retained_backup = Connection::open(&backup).expect("retained retry backup should open");
        assert_eq!(
            retained_backup
                .query_row(
                    "SELECT updated_at FROM agents WHERE agent_id = 'agent-alpha'",
                    [],
                    |row| row.get::<_, String>(0),
                )
                .expect("retained backup should contain the authoritative legacy row"),
            "2026-07-15T11:20:00Z",
        );
    }
}
