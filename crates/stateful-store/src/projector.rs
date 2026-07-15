use crate::{StoreError, StoreResult, journal::JournalEvent, schema};
use rusqlite::{Connection, params};
use stateful_core::{EventPayload, MigrationEvent};

pub(crate) struct Projector<'a> {
    connection: &'a Connection,
    prefix: &'static str,
    applied_events: u32,
    fail_on_event: Option<u32>,
}

impl<'a> Projector<'a> {
    pub(crate) fn new(
        connection: &'a Connection,
        prefix: &'static str,
        fail_on_event: Option<u32>,
    ) -> Self {
        Self {
            connection,
            prefix,
            applied_events: 0,
            fail_on_event,
        }
    }

    pub(crate) fn apply(&mut self, event: &JournalEvent) -> StoreResult<()> {
        self.applied_events += 1;
        if self.fail_on_event == Some(self.applied_events) {
            return Err(StoreError::ProjectorFailure);
        }

        let table = migration_seed_projection_table(event).or_else(|| match event.stored.aggregate_kind() {
            "presence" => Some("presence_current"),
            "reservation" => Some("reservation_current"),
            "claim" => Some("claim_current"),
            "wait" => Some("wait_current"),
            "write_fence" => Some("write_fence_current"),
            "read_observation" => Some("read_observation_current"),
            "write_intent" => Some("write_intent_current"),
            "human_observation" => Some("human_observation_current"),
            "handoff" => Some("handoff_current"),
            "notification" => Some("notification_current"),
            "migration" => Some("migration_current"),
            _ => None,
        });
        if let Some(table) = table {
            self.apply_aggregate(table, event)?;
        }
        if event.stored.affects_context() {
            self.apply_workspace_version(event)?;
        }
        Ok(())
    }

    fn apply_aggregate(&self, table: &str, event: &JournalEvent) -> StoreResult<()> {
        let table = format!("{}{}", self.prefix, table);
        let payload_json = serde_json::to_string(event.stored.payload())?;
        match table.strip_prefix(self.prefix).unwrap_or(&table) {
            "presence_current" => {
                self.connection.execute(
                    &format!(
                        "INSERT INTO {table} (workspace_id, agent_id, actor_id, actor_type, payload_json, occurred_at, origin_event_seq)
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
                         ON CONFLICT(workspace_id, agent_id) DO UPDATE SET actor_id=excluded.actor_id, actor_type=excluded.actor_type, payload_json=excluded.payload_json, occurred_at=excluded.occurred_at, origin_event_seq=excluded.origin_event_seq"
                    ),
                    params![event.workspace_id, event.agent_id, event.actor_id, event.actor_type, payload_json, event.occurred_at, event.stored.event_seq()],
                )?;
            }
            "read_observation_current" => {
                self.connection.execute(
                    &format!(
                        "INSERT INTO {table} (workspace_id, agent_id, path, payload_json, origin_event_seq)
                         VALUES (?1, ?2, ?3, ?4, ?5)
                         ON CONFLICT(workspace_id, agent_id, path) DO UPDATE SET payload_json=excluded.payload_json, origin_event_seq=excluded.origin_event_seq"
                    ),
                    params![event.workspace_id, event.agent_id, event.stored.aggregate_id(), payload_json, event.stored.event_seq()],
                )?;
            }
            "claim_current" | "write_fence_current" => {
                let data = migration_seed_data(event);
                let path = data.and_then(|data| data.get("relative_path")).and_then(serde_json::Value::as_str);
                let expires_at = data.and_then(|data| data.get("expires_at")).and_then(serde_json::Value::as_str);
                self.connection.execute(
                    &format!(
                        "INSERT INTO {table} (workspace_id, aggregate_id, path, expires_at, payload_json, origin_event_seq)
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                         ON CONFLICT(workspace_id, aggregate_id) DO UPDATE SET path=excluded.path, expires_at=excluded.expires_at, payload_json=excluded.payload_json, origin_event_seq=excluded.origin_event_seq"
                    ),
                    params![event.workspace_id, event.stored.aggregate_id(), path, expires_at, payload_json, event.stored.event_seq()],
                )?;
            }
            "wait_current" | "write_intent_current" => {
                let operation_id = migration_seed_data(event)
                    .and_then(|data| data.get("request_id"))
                    .and_then(serde_json::Value::as_str);
                self.connection.execute(
                    &format!(
                        "INSERT INTO {table} (workspace_id, aggregate_id, operation_id, payload_json, origin_event_seq)
                         VALUES (?1, ?2, ?3, ?4, ?5)
                         ON CONFLICT(workspace_id, aggregate_id) DO UPDATE SET operation_id=excluded.operation_id, payload_json=excluded.payload_json, origin_event_seq=excluded.origin_event_seq"
                    ),
                    params![event.workspace_id, event.stored.aggregate_id(), operation_id, payload_json, event.stored.event_seq()],
                )?;
            }
            "notification_current" => {
                let data = migration_seed_data(event);
                let target_agent_id = data.and_then(|data| data.get("target_agent_id")).and_then(serde_json::Value::as_str);
                let version = data.and_then(|data| data.get("sequence")).and_then(serde_json::Value::as_i64).unwrap_or(0);
                self.connection.execute(
                    &format!(
                        "INSERT INTO {table} (workspace_id, aggregate_id, target_agent_id, version, payload_json, origin_event_seq)
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                         ON CONFLICT(workspace_id, aggregate_id) DO UPDATE SET target_agent_id=excluded.target_agent_id, version=excluded.version, payload_json=excluded.payload_json, origin_event_seq=excluded.origin_event_seq"
                    ),
                    params![event.workspace_id, event.stored.aggregate_id(), target_agent_id, version, payload_json, event.stored.event_seq()],
                )?;
            }
            _ => {
                self.connection.execute(
                    &format!(
                        "INSERT INTO {table} (workspace_id, aggregate_id, payload_json, origin_event_seq)
                         VALUES (?1, ?2, ?3, ?4)
                         ON CONFLICT(workspace_id, aggregate_id) DO UPDATE SET payload_json=excluded.payload_json, origin_event_seq=excluded.origin_event_seq"
                    ),
                    params![event.workspace_id, event.stored.aggregate_id(), payload_json, event.stored.event_seq()],
                )?;
            }
        }
        Ok(())
    }

    fn apply_workspace_version(&self, event: &JournalEvent) -> StoreResult<()> {
        let table = format!("{}workspace_version", self.prefix);
        self.connection.execute(
            &format!(
                "INSERT INTO {table} (workspace_id, version, origin_event_seq) VALUES (?1, 1, ?2)
                 ON CONFLICT(workspace_id) DO UPDATE SET version=version+1, origin_event_seq=excluded.origin_event_seq"
            ),
            params![event.workspace_id, event.stored.event_seq()],
        )?;
        let cursor = format!("{}agent_context_cursor", self.prefix);
        self.connection.execute(
            &format!(
                "INSERT INTO {cursor} (workspace_id, agent_id, version, origin_event_seq) VALUES (?1, ?2, 1, ?3)
                 ON CONFLICT(workspace_id, agent_id) DO UPDATE SET version=version+1, origin_event_seq=excluded.origin_event_seq"
            ),
            params![event.workspace_id, event.agent_id, event.stored.event_seq()],
        )?;
        Ok(())
    }

    pub(crate) fn create_replay_tables(connection: &Connection) -> StoreResult<()> {
        for table in schema::PROJECTION_TABLES {
            let replay = format!("replay_{table}");
            let ddl: String = connection.query_row(
                "SELECT sql FROM sqlite_master WHERE type = 'table' AND name = ?1",
                [*table],
                |row| row.get(0),
            )?;
            let replay_ddl = ddl.replacen(table, &replay, 1);
            connection.execute_batch(&format!("DROP TABLE IF EXISTS {replay}; {replay_ddl};"))?;
        }
        Ok(())
    }

    pub(crate) fn swap_replay_tables(connection: &Connection) -> StoreResult<()> {
        for table in schema::PROJECTION_TABLES {
            let replay = format!("replay_{table}");
            connection.execute_batch(&format!(
                "DROP TABLE {table}; ALTER TABLE {replay} RENAME TO {table};"
            ))?;
        }
        schema::create_v2_schema(connection)
    }
}

fn migration_seed_projection_table(event: &JournalEvent) -> Option<&'static str> {
    match event.stored.payload() {
        EventPayload::Migration(MigrationEvent::PresenceSnapshotSeeded(_)) => Some("presence_current"),
        EventPayload::Migration(MigrationEvent::ReservationSnapshotSeeded(_)) => Some("reservation_current"),
        EventPayload::Migration(MigrationEvent::ClaimSnapshotSeeded(_)) => Some("claim_current"),
        EventPayload::Migration(MigrationEvent::WaitSnapshotSeeded(_)) => Some("wait_current"),
        EventPayload::Migration(MigrationEvent::WriteFenceSnapshotSeeded(_)) => Some("write_fence_current"),
        EventPayload::Migration(MigrationEvent::HumanObservationSnapshotSeeded(_)) => Some("human_observation_current"),
        EventPayload::Migration(MigrationEvent::LegacyHandoffSnapshotSeeded(_)) => Some("handoff_current"),
        EventPayload::Migration(MigrationEvent::DeliverySnapshotSeeded(_)) => Some("notification_current"),
        _ => None,
    }
}

fn migration_seed_data(event: &JournalEvent) -> Option<&serde_json::Value> {
    match event.stored.payload() {
        EventPayload::Migration(MigrationEvent::PresenceSnapshotSeeded(data))
        | EventPayload::Migration(MigrationEvent::ReservationSnapshotSeeded(data))
        | EventPayload::Migration(MigrationEvent::ClaimSnapshotSeeded(data))
        | EventPayload::Migration(MigrationEvent::WaitSnapshotSeeded(data))
        | EventPayload::Migration(MigrationEvent::WriteFenceSnapshotSeeded(data))
        | EventPayload::Migration(MigrationEvent::HumanObservationSnapshotSeeded(data))
        | EventPayload::Migration(MigrationEvent::LegacyHandoffSnapshotSeeded(data))
        | EventPayload::Migration(MigrationEvent::DeliverySnapshotSeeded(data)) => Some(&data.data),
        _ => None,
    }
}
