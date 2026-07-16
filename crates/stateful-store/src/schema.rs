use crate::{StoreError, StoreResult};
use rusqlite::Connection;

pub(crate) const PROJECTION_TABLES: &[&str] = &[
    "presence_current",
    "presence_resource_current",
    "reservation_current",
    "claim_current",
    "wait_current",
    "write_fence_current",
    "read_observation_current",
    "read_operation_current",
    "write_intent_current",
    "human_observation_current",
    "human_acknowledgement_current",
    "handoff_current",
    "handoff_resource_current",
    "notification_current",
    "delivery_current",
    "context_delivery_current",
    "workspace_version",
    "agent_context_cursor",
    "resource_write_current",
    "migration_current",
];

pub(crate) fn create_v2_schema(connection: &Connection) -> StoreResult<()> {
    connection.execute_batch(
        "
        CREATE TABLE IF NOT EXISTS journal_events (
            event_seq INTEGER PRIMARY KEY AUTOINCREMENT,
            event_id TEXT NOT NULL UNIQUE,
            request_id TEXT NOT NULL,
            event_ordinal INTEGER NOT NULL,
            agent_id TEXT NOT NULL,
            turn_id TEXT,
            workspace_id TEXT NOT NULL,
            repo_id TEXT,
            worktree_id TEXT,
            root TEXT,
            branch TEXT,
            aggregate_kind TEXT NOT NULL,
            aggregate_id TEXT NOT NULL,
            event_type TEXT NOT NULL,
            event_schema_version INTEGER NOT NULL,
            actor_id TEXT NOT NULL,
            actor_type TEXT NOT NULL,
            owner_id TEXT,
            parent_agent_id TEXT,
            parent_actor_id TEXT,
            source_kind TEXT NOT NULL,
            source_ref TEXT NOT NULL,
            causation_id TEXT,
            correlation_id TEXT,
            occurred_at TEXT NOT NULL,
            affects_context INTEGER NOT NULL,
            payload_json TEXT NOT NULL
        );
        CREATE INDEX IF NOT EXISTS idx_journal_events_workspace_sequence
            ON journal_events(workspace_id, event_seq);
        CREATE INDEX IF NOT EXISTS idx_journal_events_workspace_type
            ON journal_events(workspace_id, event_type);
        CREATE INDEX IF NOT EXISTS idx_journal_events_aggregate
            ON journal_events(workspace_id, aggregate_kind, aggregate_id);

        CREATE TABLE IF NOT EXISTS command_receipts (
            request_id TEXT PRIMARY KEY,
            route_kind TEXT NOT NULL,
            request_sha256 TEXT NOT NULL,
            agent_id TEXT NOT NULL,
            actor_id TEXT NOT NULL,
            workspace_id TEXT NOT NULL,
            http_status INTEGER NOT NULL,
            response_json TEXT NOT NULL,
            rejection_json TEXT,
            first_event_seq INTEGER,
            last_event_seq INTEGER,
            committed_at TEXT NOT NULL
        );

        CREATE TABLE IF NOT EXISTS presence_current (
            workspace_id TEXT NOT NULL,
            agent_id TEXT NOT NULL,
            actor_id TEXT NOT NULL,
            actor_type TEXT NOT NULL,
            payload_json TEXT NOT NULL,
            occurred_at TEXT NOT NULL,
            origin_event_seq INTEGER NOT NULL,
            PRIMARY KEY (workspace_id, agent_id)
        );
        CREATE TABLE IF NOT EXISTS presence_resource_current (
            workspace_id TEXT NOT NULL,
            aggregate_id TEXT NOT NULL,
            agent_id TEXT NOT NULL,
            relative_path TEXT NOT NULL,
            relation TEXT NOT NULL,
            observed_at TEXT NOT NULL,
            payload_json TEXT NOT NULL,
            origin_event_seq INTEGER NOT NULL,
            PRIMARY KEY (workspace_id, aggregate_id)
        );
        CREATE TABLE IF NOT EXISTS reservation_current (
            workspace_id TEXT NOT NULL,
            aggregate_id TEXT NOT NULL,
            payload_json TEXT NOT NULL,
            origin_event_seq INTEGER NOT NULL,
            PRIMARY KEY (workspace_id, aggregate_id)
        );
        CREATE TABLE IF NOT EXISTS claim_current (
            workspace_id TEXT NOT NULL,
            aggregate_id TEXT NOT NULL,
            path TEXT,
            expires_at TEXT,
            payload_json TEXT NOT NULL,
            origin_event_seq INTEGER NOT NULL,
            PRIMARY KEY (workspace_id, aggregate_id)
        );
        CREATE TABLE IF NOT EXISTS wait_current (
            workspace_id TEXT NOT NULL,
            aggregate_id TEXT NOT NULL,
            operation_id TEXT,
            payload_json TEXT NOT NULL,
            origin_event_seq INTEGER NOT NULL,
            PRIMARY KEY (workspace_id, aggregate_id)
        );
        CREATE TABLE IF NOT EXISTS write_fence_current (
            workspace_id TEXT NOT NULL,
            aggregate_id TEXT NOT NULL,
            path TEXT,
            expires_at TEXT,
            payload_json TEXT NOT NULL,
            origin_event_seq INTEGER NOT NULL,
            PRIMARY KEY (workspace_id, aggregate_id)
        );
        CREATE TABLE IF NOT EXISTS read_observation_current (
            workspace_id TEXT NOT NULL,
            agent_id TEXT NOT NULL,
            path TEXT NOT NULL,
            payload_json TEXT NOT NULL,
            origin_event_seq INTEGER NOT NULL,
            PRIMARY KEY (workspace_id, agent_id, path)
        );
        CREATE TABLE IF NOT EXISTS read_operation_current (
            workspace_id TEXT NOT NULL,
            agent_id TEXT NOT NULL,
            operation_id TEXT NOT NULL,
            path TEXT NOT NULL,
            payload_json TEXT NOT NULL,
            origin_event_seq INTEGER NOT NULL,
            PRIMARY KEY (workspace_id, agent_id, operation_id)
        );
        CREATE TABLE IF NOT EXISTS write_intent_current (
            workspace_id TEXT NOT NULL,
            aggregate_id TEXT NOT NULL,
            operation_id TEXT,
            payload_json TEXT NOT NULL,
            origin_event_seq INTEGER NOT NULL,
            PRIMARY KEY (workspace_id, aggregate_id)
        );
        CREATE TABLE IF NOT EXISTS human_observation_current (
            workspace_id TEXT NOT NULL,
            aggregate_id TEXT NOT NULL,
            payload_json TEXT NOT NULL,
            origin_event_seq INTEGER NOT NULL,
            PRIMARY KEY (workspace_id, aggregate_id)
        );
        CREATE TABLE IF NOT EXISTS human_acknowledgement_current (
            workspace_id TEXT NOT NULL,
            aggregate_id TEXT NOT NULL,
            payload_json TEXT NOT NULL,
            origin_event_seq INTEGER NOT NULL,
            PRIMARY KEY (workspace_id, aggregate_id)
        );
        CREATE TABLE IF NOT EXISTS handoff_current (
            workspace_id TEXT NOT NULL,
            aggregate_id TEXT NOT NULL,
            payload_json TEXT NOT NULL,
            origin_event_seq INTEGER NOT NULL,
            PRIMARY KEY (workspace_id, aggregate_id)
        );
        CREATE TABLE IF NOT EXISTS handoff_resource_current (
            workspace_id TEXT NOT NULL,
            aggregate_id TEXT NOT NULL,
            payload_json TEXT NOT NULL,
            origin_event_seq INTEGER NOT NULL,
            PRIMARY KEY (workspace_id, aggregate_id)
        );
        CREATE TABLE IF NOT EXISTS notification_current (
            workspace_id TEXT NOT NULL,
            aggregate_id TEXT NOT NULL,
            target_agent_id TEXT,
            version INTEGER NOT NULL DEFAULT 0,
            payload_json TEXT NOT NULL,
            origin_event_seq INTEGER NOT NULL,
            PRIMARY KEY (workspace_id, aggregate_id)
        );
        CREATE TABLE IF NOT EXISTS delivery_current (
            workspace_id TEXT NOT NULL,
            aggregate_id TEXT NOT NULL,
            payload_json TEXT NOT NULL,
            origin_event_seq INTEGER NOT NULL,
            PRIMARY KEY (workspace_id, aggregate_id)
        );
        CREATE TABLE IF NOT EXISTS context_delivery_current (
            workspace_id TEXT NOT NULL,
            aggregate_id TEXT NOT NULL,
            target_agent_id TEXT NOT NULL,
            version INTEGER NOT NULL,
            sequence INTEGER NOT NULL,
            payload_json TEXT NOT NULL,
            origin_event_seq INTEGER NOT NULL,
            PRIMARY KEY (workspace_id, aggregate_id)
        );
        CREATE TABLE IF NOT EXISTS workspace_version (
            workspace_id TEXT PRIMARY KEY,
            version INTEGER NOT NULL,
            origin_event_seq INTEGER NOT NULL
        );
        CREATE TABLE IF NOT EXISTS agent_context_cursor (
            workspace_id TEXT NOT NULL,
            agent_id TEXT NOT NULL,
            version INTEGER NOT NULL,
            origin_event_seq INTEGER NOT NULL,
            PRIMARY KEY (workspace_id, agent_id)
        );
        CREATE TABLE IF NOT EXISTS resource_write_current (
            workspace_id TEXT NOT NULL,
            path TEXT NOT NULL,
            payload_json TEXT NOT NULL,
            origin_event_seq INTEGER NOT NULL,
            PRIMARY KEY (workspace_id, path)
        );
        CREATE TABLE IF NOT EXISTS migration_current (
            workspace_id TEXT NOT NULL,
            aggregate_id TEXT NOT NULL,
            payload_json TEXT NOT NULL,
            origin_event_seq INTEGER NOT NULL,
            PRIMARY KEY (workspace_id, aggregate_id)
        );

        CREATE INDEX IF NOT EXISTS idx_claim_current_active_expiry
            ON claim_current(workspace_id, path, expires_at);
        CREATE INDEX IF NOT EXISTS idx_write_fence_current_active_expiry
            ON write_fence_current(workspace_id, path, expires_at);
        CREATE INDEX IF NOT EXISTS idx_wait_current_operation
            ON wait_current(workspace_id, operation_id);
        CREATE INDEX IF NOT EXISTS idx_read_operation_current_path
            ON read_operation_current(workspace_id, agent_id, path);
        CREATE INDEX IF NOT EXISTS idx_write_intent_current_operation
            ON write_intent_current(workspace_id, operation_id);
        CREATE INDEX IF NOT EXISTS idx_resource_write_current_path
            ON resource_write_current(workspace_id, path);
        CREATE INDEX IF NOT EXISTS idx_notification_current_target_version
            ON notification_current(target_agent_id, version);
        CREATE INDEX IF NOT EXISTS idx_context_delivery_current_target_version
            ON context_delivery_current(workspace_id, target_agent_id, version, sequence);
        ",
    )?;
    ensure_command_receipt_columns(connection)?;
    ensure_presence_resource_columns(connection)?;
    Ok(())
}

fn ensure_command_receipt_columns(connection: &Connection) -> StoreResult<()> {
    let mut statement = connection.prepare("PRAGMA table_info(command_receipts)")?;
    let columns = statement
        .query_map([], |row| row.get::<_, String>(1))?
        .collect::<Result<Vec<_>, _>>()?;
    if !columns.iter().any(|column| column == "rejection_json") {
        connection.execute_batch("ALTER TABLE command_receipts ADD COLUMN rejection_json TEXT;")?;
    }
    Ok(())
}

fn ensure_presence_resource_columns(connection: &Connection) -> StoreResult<()> {
    let mut statement = connection.prepare("PRAGMA table_info(presence_resource_current)")?;
    let columns = statement
        .query_map([], |row| row.get::<_, String>(1))?
        .collect::<Result<Vec<_>, _>>()?;
    for (column, definition) in [
        ("agent_id", "TEXT NOT NULL DEFAULT ''"),
        ("relative_path", "TEXT NOT NULL DEFAULT ''"),
        ("relation", "TEXT NOT NULL DEFAULT ''"),
        ("observed_at", "TEXT NOT NULL DEFAULT ''"),
    ] {
        if !columns.iter().any(|current| current == column) {
            connection.execute_batch(&format!(
                "ALTER TABLE presence_resource_current ADD COLUMN {column} {definition};"
            ))?;
        }
    }
    Ok(())
}

pub(crate) fn create_projection_tables_with_prefix(
    connection: &Connection,
    prefix: &str,
) -> StoreResult<()> {
    for table in PROJECTION_TABLES {
        let prefixed = format!("{prefix}{table}");
        let ddl: String = connection.query_row(
            "SELECT sql FROM sqlite_master WHERE type = 'table' AND name = ?1",
            [*table],
            |row| row.get(0),
        )?;
        let prefixed_ddl = ddl.replacen(table, &prefixed, 1);
        connection.execute_batch(&format!(
            "DROP TABLE IF EXISTS {prefixed}; {prefixed_ddl};"
        ))?;
    }
    Ok(())
}

pub(crate) fn replace_projections_from_prefix(
    connection: &Connection,
    prefix: &str,
) -> StoreResult<()> {
    for table in PROJECTION_TABLES {
        let prefixed = format!("{prefix}{table}");
        connection.execute_batch(&format!(
            "DROP TABLE {table}; ALTER TABLE {prefixed} RENAME TO {table};"
        ))?;
    }
    create_v2_schema(connection)
}

pub(crate) fn replace_projection_tables_from_prefix(
    connection: &Connection,
    prefix: &str,
    tables: &[&str],
) -> StoreResult<()> {
    for table in tables {
        if !PROJECTION_TABLES.contains(table) {
            return Err(StoreError::InvalidJournalEvent);
        }
        let prefixed = format!("{prefix}{table}");
        connection.execute_batch(&format!(
            "DROP TABLE {table}; ALTER TABLE {prefixed} RENAME TO {table};"
        ))?;
    }
    create_v2_schema(connection)
}

pub(crate) fn drop_projection_tables_with_prefix(
    connection: &Connection,
    prefix: &str,
) -> StoreResult<()> {
    for table in PROJECTION_TABLES {
        let prefixed = format!("{prefix}{table}");
        connection.execute_batch(&format!("DROP TABLE IF EXISTS {prefixed};"))?;
    }
    Ok(())
}
