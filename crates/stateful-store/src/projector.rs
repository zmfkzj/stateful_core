use crate::{StoreError, StoreResult, journal::JournalEvent, schema};
use rusqlite::{Connection, params};
use stateful_core::{
    ActorType, AuthorizationEvent, ClaimEvent, ContextEvent, EventData, EventPayload, HandoffEvent,
    HandoffRecord, HandoffStatus, HumanAcknowledgementEvent, HumanObservationEvent, MigrationEvent,
    NotificationEvent, PresenceEvent, PresenceRecord, PresenceResource, ReadObservationEvent,
    ReadObservationRecord, RecoveryEvent, ReservationEvent, ResourceVersion, WaitEvent,
    WriteFenceEvent, WriteIntentEvent, FALLBACK_HANDOFF_RELEVANCE,
};

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
        if matches!(
            event.stored.payload(),
            EventPayload::Migration(MigrationEvent::LegacyAuditImported(_))
        ) || migration_snapshot_is_terminal(event) {
            return Ok(());
        }
        if self.apply_typed_migration_seed(event)? {
            if event.stored.affects_context() {
                self.apply_workspace_version(event)?;
            }
            return Ok(());
        }

        if self.apply_presence_or_handoff(event)? {
            if event.stored.affects_context() {
                self.apply_workspace_version(event)?;
            }
            return Ok(());
        }
        if let Some(agent_id) = cleanup_agent_id(event) {
            self.apply_coordination_cleanup(event, agent_id)?;
            if event.stored.affects_context() {
                self.apply_workspace_version(event)?;
            }
            return Ok(());
        }
        if self.apply_freshness(event)? {
            if event.stored.affects_context() {
                self.apply_workspace_version(event)?;
            }
            return Ok(());
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
            "human_acknowledgement" => Some("human_acknowledgement_current"),
            "handoff" => Some("handoff_current"),
            "notification" => Some("notification_current"),
            "recovery" => Some("delivery_current"),
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

    fn apply_typed_migration_seed(&self, event: &JournalEvent) -> StoreResult<bool> {
        match event.stored.payload() {
            EventPayload::Migration(MigrationEvent::PresenceSnapshotSeeded(data)) => {
                let now = event.stored.observed_at();
                let phase = data.data.get("phase")
                    .and_then(serde_json::Value::as_str)
                    .and_then(|phase| serde_json::from_value(serde_json::Value::String(phase.into())).ok());
                let expires_at = data.data.get("expires_at")
                    .and_then(serde_json::Value::as_str)
                    .and_then(|value| time::OffsetDateTime::parse(value, &time::format_description::well_known::Rfc3339).ok())
                    .unwrap_or(now);
                let presence = PresenceRecord {
                    workspace_id: event.workspace_id.clone(),
                    agent_id: event.agent_id.clone(),
                    actor_id: "unknown".into(),
                    actor_type: ActorType::Unknown,
                    owner_id: None,
                    parent_agent_id: None,
                    parent_actor_id: None,
                    goal_excerpt: None,
                    phase,
                    next_plan: None,
                    last_result: None,
                    registered_at: now,
                    updated_at: now,
                    expires_at,
                    busy_until: None,
                    origin_event_seq: event.stored.event_seq(),
                };
                let table = format!("{}presence_current", self.prefix);
                self.connection.execute(
                    &format!(
                        "INSERT INTO {table} (workspace_id, agent_id, actor_id, actor_type, payload_json, occurred_at, origin_event_seq)
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
                         ON CONFLICT(workspace_id, agent_id) DO UPDATE SET actor_id=excluded.actor_id, actor_type=excluded.actor_type, payload_json=excluded.payload_json, occurred_at=excluded.occurred_at, origin_event_seq=excluded.origin_event_seq"
                    ),
                    params![
                        &presence.workspace_id,
                        &presence.agent_id,
                        &presence.actor_id,
                        "unknown",
                        serde_json::to_string(&presence)?,
                        &event.occurred_at,
                        event.stored.event_seq(),
                    ],
                )?;
                Ok(true)
            }
            EventPayload::Migration(MigrationEvent::LegacyHandoffSnapshotSeeded(_)) => {
                let now = event.stored.observed_at();
                let handoff = HandoffRecord {
                    workspace_id: event.workspace_id.clone(),
                    agent_id: event.agent_id.clone(),
                    actor_id: "unknown".into(),
                    actor_type: ActorType::Unknown,
                    owner_id: None,
                    parent_agent_id: None,
                    parent_actor_id: None,
                    status: HandoffStatus::Unknown,
                    summary: "Migrated legacy session ended with no explicit handoff supplied.".into(),
                    files_changed: Vec::new(),
                    tests_run: Vec::new(),
                    remaining_work: Vec::new(),
                    next_plan: None,
                    last_result: None,
                    explicit: false,
                    finalized_at: now,
                    expires_at: now + FALLBACK_HANDOFF_RELEVANCE,
                    origin_event_seq: event.stored.event_seq(),
                };
                let table = format!("{}handoff_current", self.prefix);
                self.connection.execute(
                    &format!(
                        "INSERT INTO {table} (workspace_id, aggregate_id, payload_json, origin_event_seq)
                         VALUES (?1, ?2, ?3, ?4)
                         ON CONFLICT(workspace_id, aggregate_id) DO UPDATE SET payload_json=excluded.payload_json, origin_event_seq=excluded.origin_event_seq"
                    ),
                    params![&handoff.workspace_id, &handoff.agent_id, serde_json::to_string(&handoff)?, event.stored.event_seq()],
                )?;
                Ok(true)
            }
            _ => Ok(false),
        }
    }

    fn apply_freshness(&self, event: &JournalEvent) -> StoreResult<bool> {
        match event.stored.payload() {
            EventPayload::ReadObservation(ReadObservationEvent::Started(data)) => {
                let Some(value) = data.data.get("read_observation") else {
                    return Ok(false);
                };
                let record: ReadObservationRecord = serde_json::from_value(value.clone())?;
                let table = format!("{}read_operation_current", self.prefix);
                self.connection.execute(
                    &format!(
                        "INSERT INTO {table} (workspace_id, agent_id, operation_id, path, payload_json, origin_event_seq)
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                         ON CONFLICT(workspace_id, agent_id, operation_id)
                         DO UPDATE SET path=excluded.path, payload_json=excluded.payload_json, origin_event_seq=excluded.origin_event_seq"
                    ),
                    params![
                        &record.workspace_id,
                        &record.agent_id,
                        &record.operation_id,
                        &record.path,
                        serde_json::to_string(&record)?,
                        event.stored.event_seq(),
                    ],
                )?;
                Ok(true)
            }
            EventPayload::ReadObservation(
                ReadObservationEvent::Stabilized(_)
                | ReadObservationEvent::Unstable(_)
                | ReadObservationEvent::Aborted(_)
                | ReadObservationEvent::Invalidated(_)
                | ReadObservationEvent::Expired(_),
            ) => {
                let Some(data) = event_data(event) else {
                    return Ok(false);
                };
                let Some(value) = data.get("read_observation") else {
                    return Ok(false);
                };
                let record: ReadObservationRecord = serde_json::from_value(value.clone())?;
                self.apply_aggregate("read_observation_current", event)?;
                let operations = format!("{}read_operation_current", self.prefix);
                self.connection.execute(
                    &format!(
                        "DELETE FROM {operations}
                         WHERE workspace_id = ?1 AND agent_id = ?2 AND operation_id = ?3"
                    ),
                    params![&record.workspace_id, &record.agent_id, &record.operation_id],
                )?;
                Ok(true)
            }
            EventPayload::WriteIntent(WriteIntentEvent::Committed(data)) => {
                self.apply_aggregate("write_intent_current", event)?;
                let versions = data.data.get("resource_versions")
                    .cloned()
                    .unwrap_or_else(|| serde_json::Value::Array(Vec::new()));
                for value in versions.as_array().into_iter().flatten() {
                    let version: ResourceVersion = serde_json::from_value(value.clone())?;
                    let table = format!("{}resource_write_current", self.prefix);
                    self.connection.execute(
                        &format!(
                            "INSERT INTO {table} (workspace_id, path, payload_json, origin_event_seq)
                             VALUES (?1, ?2, ?3, ?4)
                             ON CONFLICT(workspace_id, path)
                             DO UPDATE SET payload_json=excluded.payload_json, origin_event_seq=excluded.origin_event_seq"
                        ),
                        params![
                            &version.workspace_id,
                            &version.path,
                            serde_json::to_string(&version)?,
                            event.stored.event_seq(),
                        ],
                    )?;
                }
                Ok(true)
            }
            EventPayload::WriteIntent(
                WriteIntentEvent::Started(_)
                | WriteIntentEvent::Failed(_)
                | WriteIntentEvent::OutcomeUnknown(_)
                | WriteIntentEvent::Reconciled(_),
            ) => {
                self.apply_aggregate("write_intent_current", event)?;
                Ok(true)
            }
            _ => Ok(false),
        }
    }

    fn apply_presence_or_handoff(&self, event: &JournalEvent) -> StoreResult<bool> {
        match event.stored.payload() {
            EventPayload::Presence(PresenceEvent::Finalized(_) | PresenceEvent::Expired(_)) => {
                let presence = format!("{}presence_current", self.prefix);
                let resources = format!("{}presence_resource_current", self.prefix);
                self.connection.execute(
                    &format!("DELETE FROM {presence} WHERE workspace_id = ?1 AND agent_id = ?2"),
                    params![event.workspace_id, event.stored.aggregate_id()],
                )?;
                self.connection.execute(
                    &format!("DELETE FROM {resources} WHERE workspace_id = ?1 AND agent_id = ?2"),
                    params![event.workspace_id, event.stored.aggregate_id()],
                )?;
                let observations = format!("{}read_observation_current", self.prefix);
                let operations = format!("{}read_operation_current", self.prefix);
                self.connection.execute(
                    &format!("DELETE FROM {observations} WHERE workspace_id = ?1 AND agent_id = ?2"),
                    params![event.workspace_id, event.stored.aggregate_id()],
                )?;
                self.connection.execute(
                    &format!("DELETE FROM {operations} WHERE workspace_id = ?1 AND agent_id = ?2"),
                    params![event.workspace_id, event.stored.aggregate_id()],
                )?;
                Ok(true)
            }
            EventPayload::Presence(presence_event) => {
                let Some(data) = presence_event_data(presence_event) else {
                    return Ok(false);
                };
                let Some(value) = data.data.get("presence") else {
                    return Ok(false);
                };
                let presence: PresenceRecord = serde_json::from_value(value.clone())?;
                let table = format!("{}presence_current", self.prefix);
                self.connection.execute(
                    &format!(
                        "INSERT INTO {table} (workspace_id, agent_id, actor_id, actor_type, payload_json, occurred_at, origin_event_seq)
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
                         ON CONFLICT(workspace_id, agent_id) DO UPDATE SET actor_id=excluded.actor_id, actor_type=excluded.actor_type, payload_json=excluded.payload_json, occurred_at=excluded.occurred_at, origin_event_seq=excluded.origin_event_seq"
                    ),
                    params![
                        &presence.workspace_id,
                        &presence.agent_id,
                        &presence.actor_id,
                        serde_json::to_value(&presence.actor_type)?.as_str().unwrap_or("unknown"),
                        serde_json::to_string(&presence)?,
                        &event.occurred_at,
                        event.stored.event_seq(),
                    ],
                )?;
                if let Some(value) = data.data.get("resource") {
                    let resource: PresenceResource = serde_json::from_value(value.clone())?;
                    let table = format!("{}presence_resource_current", self.prefix);
                    let aggregate_id = presence_resource_key(&resource);
                    self.connection.execute(
                        &format!(
                            "INSERT INTO {table} (workspace_id, aggregate_id, agent_id, relative_path, relation, observed_at, payload_json, origin_event_seq)
                             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
                             ON CONFLICT(workspace_id, aggregate_id) DO UPDATE SET agent_id=excluded.agent_id, relative_path=excluded.relative_path, relation=excluded.relation, observed_at=excluded.observed_at, payload_json=excluded.payload_json, origin_event_seq=excluded.origin_event_seq"
                        ),
                        params![
                            &resource.workspace_id,
                            aggregate_id,
                            &resource.agent_id,
                            &resource.relative_path,
                            serde_json::to_value(&resource.relation)?.as_str().unwrap_or(""),
                            resource.observed_at.format(&time::format_description::well_known::Rfc3339)
                                .map_err(|error| StoreError::InvalidTimestamp(error.to_string()))?,
                            serde_json::to_string(&resource)?,
                            event.stored.event_seq(),
                        ],
                    )?;
                }
                Ok(true)
            }
            EventPayload::Handoff(HandoffEvent::Finalized(data)) => {
                let Some(value) = data.data.get("handoff") else {
                    return Ok(false);
                };
                let handoff: HandoffRecord = serde_json::from_value(value.clone())?;
                let table = format!("{}handoff_current", self.prefix);
                self.connection.execute(
                    &format!(
                        "INSERT INTO {table} (workspace_id, aggregate_id, payload_json, origin_event_seq)
                         VALUES (?1, ?2, ?3, ?4)
                         ON CONFLICT(workspace_id, aggregate_id) DO UPDATE SET payload_json=excluded.payload_json, origin_event_seq=excluded.origin_event_seq"
                    ),
                    params![&handoff.workspace_id, &handoff.agent_id, serde_json::to_string(&handoff)?, event.stored.event_seq()],
                )?;
                let resources = format!("{}handoff_resource_current", self.prefix);
                self.connection.execute(
                    &format!("DELETE FROM {resources} WHERE workspace_id = ?1 AND json_extract(payload_json, '$.agent_id') = ?2"),
                    params![&handoff.workspace_id, &handoff.agent_id],
                )?;
                for path in &handoff.files_changed {
                    let payload = serde_json::to_string(&serde_json::json!({
                        "agent_id": handoff.agent_id,
                        "relative_path": path,
                    }))?;
                    self.connection.execute(
                        &format!(
                            "INSERT INTO {resources} (workspace_id, aggregate_id, payload_json, origin_event_seq)
                             VALUES (?1, ?2, ?3, ?4)
                             ON CONFLICT(workspace_id, aggregate_id) DO UPDATE SET payload_json=excluded.payload_json, origin_event_seq=excluded.origin_event_seq"
                        ),
                        params![&handoff.workspace_id, handoff_resource_key(&handoff.agent_id, path), payload, event.stored.event_seq()],
                    )?;
                }
                Ok(true)
            }
            EventPayload::Handoff(HandoffEvent::Expired(data)) => {
                let agent_id = data.data.get("handoff")
                    .and_then(|value| value.get("agent_id"))
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or(event.stored.aggregate_id());
                let handoffs = format!("{}handoff_current", self.prefix);
                let resources = format!("{}handoff_resource_current", self.prefix);
                self.connection.execute(
                    &format!("DELETE FROM {handoffs} WHERE workspace_id = ?1 AND aggregate_id = ?2"),
                    params![event.workspace_id, agent_id],
                )?;
                self.connection.execute(
                    &format!("DELETE FROM {resources} WHERE workspace_id = ?1 AND json_extract(payload_json, '$.agent_id') = ?2"),
                    params![event.workspace_id, agent_id],
                )?;
                Ok(true)
            }
            _ => Ok(false),
        }
    }

    fn apply_coordination_cleanup(&self, event: &JournalEvent, agent_id: &str) -> StoreResult<()> {
        for table in [
            "reservation_current",
            "claim_current",
            "wait_current",
            "write_fence_current",
        ] {
            let table = format!("{}{}", self.prefix, table);
            self.connection.execute(
                &format!(
                    "DELETE FROM {table}
                     WHERE workspace_id = ?1
                       AND origin_event_seq IN (
                           SELECT event_seq FROM journal_events
                           WHERE workspace_id = ?1 AND agent_id = ?2
                       )"
                ),
                params![event.workspace_id, agent_id],
            )?;
        }
        Ok(())
    }

    fn apply_aggregate(&self, table: &str, event: &JournalEvent) -> StoreResult<()> {
        let table = format!("{}{}", self.prefix, table);
        let aggregate_table = table.strip_prefix(self.prefix).unwrap_or(&table);
        let data = event_data(event).map(|data| projection_data(aggregate_table, data));
        let payload_json = serde_json::to_string(data.unwrap_or(&serde_json::Value::Null))?;
        match aggregate_table {
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
                let agent_id = data
                    .and_then(|data| data.get("agent_id"))
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or(&event.agent_id);
                let path = data
                    .and_then(|data| data.get("path"))
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or(event.stored.aggregate_id());
                self.connection.execute(
                    &format!(
                        "INSERT INTO {table} (workspace_id, agent_id, path, payload_json, origin_event_seq)
                         VALUES (?1, ?2, ?3, ?4, ?5)
                         ON CONFLICT(workspace_id, agent_id, path) DO UPDATE SET payload_json=excluded.payload_json, origin_event_seq=excluded.origin_event_seq"
                    ),
                    params![event.workspace_id, agent_id, path, payload_json, event.stored.event_seq()],
                )?;
            }
            "claim_current" | "write_fence_current" => {
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
                let operation_id = data
                    .and_then(|data| data.get(if aggregate_table == "wait_current" { "request_id" } else { "operation_id" }))
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

fn projection_data<'a>(table: &str, data: &'a serde_json::Value) -> &'a serde_json::Value {
    let key = match table {
        "reservation_current" => "reservation",
        "claim_current" => "claim",
        "wait_current" => "wait",
        "write_fence_current" => "write_fence",
        "read_observation_current" => "read_observation",
        "write_intent_current" => "write_intent",
        "human_observation_current" => "observation",
        "human_acknowledgement_current" => "acknowledgement",
        "notification_current" => "notification",
        "delivery_current" => "delivery",
        _ => return data,
    };
    data.get(key).unwrap_or(data)
}

fn event_data(event: &JournalEvent) -> Option<&serde_json::Value> {
    let data: &EventData = match event.stored.payload() {
        EventPayload::Migration(event) => match event {
            MigrationEvent::Started(data)
            | MigrationEvent::LegacyAuditImported(data)
            | MigrationEvent::PresenceSnapshotSeeded(data)
            | MigrationEvent::ReservationSnapshotSeeded(data)
            | MigrationEvent::ClaimSnapshotSeeded(data)
            | MigrationEvent::WaitSnapshotSeeded(data)
            | MigrationEvent::WriteFenceSnapshotSeeded(data)
            | MigrationEvent::HumanObservationSnapshotSeeded(data)
            | MigrationEvent::LegacyHandoffSnapshotSeeded(data)
            | MigrationEvent::DeliverySnapshotSeeded(data)
            | MigrationEvent::Validated(data)
            | MigrationEvent::Completed(data) => data,
        },
        EventPayload::Presence(event) => match event {
            PresenceEvent::Registered(data)
            | PresenceEvent::Heartbeat(data)
            | PresenceEvent::GoalUpdated(data)
            | PresenceEvent::PhaseUpdated(data)
            | PresenceEvent::PlanUpdated(data)
            | PresenceEvent::ResourcesUpdated(data)
            | PresenceEvent::ToolStarted(data)
            | PresenceEvent::ToolCompleted(data)
            | PresenceEvent::Finalized(data)
            | PresenceEvent::Expired(data) => data,
        },
        EventPayload::Reservation(event) => match event {
            ReservationEvent::Declared(data)
            | ReservationEvent::Refreshed(data)
            | ReservationEvent::Released(data)
            | ReservationEvent::Expired(data) => data,
        },
        EventPayload::Claim(event) => match event {
            ClaimEvent::Acquired(data)
            | ClaimEvent::ObservationRefreshed(data)
            | ClaimEvent::Released(data)
            | ClaimEvent::Expired(data) => data,
        },
        EventPayload::Wait(event) => match event {
            WaitEvent::Requested(data)
            | WaitEvent::BecameClaimable(data)
            | WaitEvent::Claimed(data)
            | WaitEvent::Cancelled(data)
            | WaitEvent::Expired(data) => data,
        },
        EventPayload::WriteFence(event) => match event {
            WriteFenceEvent::Acquired(data)
            | WriteFenceEvent::ConflictObserved(data)
            | WriteFenceEvent::Released(data)
            | WriteFenceEvent::Expired(data) => data,
        },
        EventPayload::ReadObservation(event) => match event {
            ReadObservationEvent::Started(data)
            | ReadObservationEvent::Stabilized(data)
            | ReadObservationEvent::Unstable(data)
            | ReadObservationEvent::Aborted(data)
            | ReadObservationEvent::Invalidated(data)
            | ReadObservationEvent::Expired(data) => data,
        },
        EventPayload::WriteIntent(event) => match event {
            WriteIntentEvent::Started(data)
            | WriteIntentEvent::Committed(data)
            | WriteIntentEvent::Failed(data)
            | WriteIntentEvent::OutcomeUnknown(data)
            | WriteIntentEvent::Reconciled(data) => data,
        },
        EventPayload::HumanObservation(event) => match event {
            HumanObservationEvent::Observed(data)
            | HumanObservationEvent::Reconciled(data)
            | HumanObservationEvent::Expired(data) => data,
        },
        EventPayload::HumanAcknowledgement(event) => match event {
            HumanAcknowledgementEvent::Recorded(data) => data,
        },
        EventPayload::Handoff(event) => match event {
            HandoffEvent::Finalized(data) | HandoffEvent::Expired(data) => data,
        },
        EventPayload::Authorization(event) => match event {
            AuthorizationEvent::Allowed(data)
            | AuthorizationEvent::Warned(data)
            | AuthorizationEvent::Denied(data)
            | AuthorizationEvent::OverrideGranted(data) => data,
        },
        EventPayload::Context(event) => match event {
            ContextEvent::Rendered(data)
            | ContextEvent::DeliveryCreated(data)
            | ContextEvent::DeliveryAcknowledged(data)
            | ContextEvent::DeliverySuperseded(data) => data,
        },
        EventPayload::Notification(event) => match event {
            NotificationEvent::Created(data)
            | NotificationEvent::Delivered(data)
            | NotificationEvent::Expired(data)
            | NotificationEvent::Coalesced(data) => data,
        },
        EventPayload::Recovery(event) => match event {
            RecoveryEvent::Queued(data)
            | RecoveryEvent::Attempted(data)
            | RecoveryEvent::Delivered(data)
            | RecoveryEvent::Failed(data) => data,
        },
    };
    Some(&data.data)
}

fn presence_event_data(event: &PresenceEvent) -> Option<&EventData> {
    match event {
        PresenceEvent::Registered(data)
        | PresenceEvent::Heartbeat(data)
        | PresenceEvent::GoalUpdated(data)
        | PresenceEvent::PhaseUpdated(data)
        | PresenceEvent::PlanUpdated(data)
        | PresenceEvent::ResourcesUpdated(data)
        | PresenceEvent::ToolStarted(data)
        | PresenceEvent::ToolCompleted(data)
        | PresenceEvent::Finalized(data)
        | PresenceEvent::Expired(data) => Some(data),
    }
}

fn presence_resource_key(resource: &PresenceResource) -> String {
    let relation = serde_json::to_value(resource.relation)
        .ok()
        .and_then(|value| value.as_str().map(str::to_owned))
        .unwrap_or_default();
    format!("{}\u{1}{}\u{1}{relation}", resource.agent_id, resource.relative_path)
}

fn handoff_resource_key(agent_id: &str, relative_path: &str) -> String {
    format!("{agent_id}\u{1}{relative_path}")
}

fn cleanup_agent_id(event: &JournalEvent) -> Option<&str> {
    let data = match event.stored.payload() {
        EventPayload::Reservation(ReservationEvent::Released(data))
        | EventPayload::Claim(ClaimEvent::Released(data))
        | EventPayload::Wait(WaitEvent::Cancelled(data))
        | EventPayload::WriteFence(WriteFenceEvent::Released(data)) => data,
        _ => return None,
    };
    data.data.get("cleanup")
        .and_then(serde_json::Value::as_bool)
        .filter(|cleanup| *cleanup)
        .map(|_| data.aggregate_id.as_str())
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

fn migration_snapshot_is_terminal(event: &JournalEvent) -> bool {
    let terminal: &[&str] = match event.stored.payload() {
        EventPayload::Migration(MigrationEvent::ClaimSnapshotSeeded(_)) => {
            &["released", "expired", "cancelled"]
        }
        EventPayload::Migration(MigrationEvent::WriteFenceSnapshotSeeded(_)) => {
            &["released", "expired"]
        }
        _ => return false,
    };
    migration_seed_data(event)
        .and_then(|data| data.get("status"))
        .and_then(serde_json::Value::as_str)
        .is_some_and(|status| terminal.contains(&status))
}
