use crate::{
    Store, StoreError, StoreResult,
    projector::Projector,
    schema::PROJECTION_TABLES,
};
use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params, types::ValueRef};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use sha2::{Digest, Sha256};
use stateful_core::{
    ActorType, EventPayload, NewEvent, PresenceRecord, RequestEnvelope, StoredEvent,
};
use std::collections::BTreeMap;
use time::{OffsetDateTime, format_description::well_known::Rfc3339};
use uuid::Uuid;

pub trait ProjectionReader {
    fn workspace_version(&self, workspace_id: &str) -> StoreResult<u64>;
    fn presence(&self, workspace_id: &str, agent_id: &str) -> StoreResult<Option<PresenceRecord>>;
    fn active_claims_for_path(&self, workspace_id: &str, path: &str) -> StoreResult<Vec<ClaimRecord>>;
    fn active_fence_for_path(&self, workspace_id: &str, path: &str) -> StoreResult<Option<WriteFenceRecord>>;
    fn stable_observation(&self, workspace_id: &str, agent_id: &str, path: &str) -> StoreResult<Option<ReadObservationRecord>>;
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClaimRecord {
    pub workspace_id: String,
    pub claim_id: String,
    pub path: Option<String>,
    pub expires_at: Option<String>,
    pub origin_event_seq: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WriteFenceRecord {
    pub workspace_id: String,
    pub fence_id: String,
    pub path: Option<String>,
    pub expires_at: Option<String>,
    pub origin_event_seq: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReadObservationRecord {
    pub workspace_id: String,
    pub agent_id: String,
    pub path: String,
    pub origin_event_seq: u64,
}

#[derive(Debug, Clone)]
pub struct CommandPlan<R> {
    pub events: Vec<NewEvent>,
    pub response: R,
    pub http_status: u16,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandOutcome<R> {
    pub response: R,
    pub http_status: u16,
    pub first_event_seq: Option<u64>,
    pub last_event_seq: Option<u64>,
    pub duplicate: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReplayReport {
    pub projectable_events: u64,
    pub projection_rows: u64,
    pub canonical_sha256: String,
}

pub(crate) struct JournalEvent {
    pub(crate) stored: StoredEvent,
    pub(crate) agent_id: String,
    pub(crate) workspace_id: String,
    pub(crate) actor_id: String,
    pub(crate) actor_type: String,
    pub(crate) occurred_at: String,
}

struct SqlProjectionReader<'a> {
    transaction: &'a Transaction<'a>,
}

impl ProjectionReader for SqlProjectionReader<'_> {
    fn workspace_version(&self, workspace_id: &str) -> StoreResult<u64> {
        self.transaction
            .query_row(
                "SELECT version FROM workspace_version WHERE workspace_id = ?1",
                [workspace_id],
                |row| row.get(0),
            )
            .optional()
            .map(|value: Option<u64>| value.unwrap_or(0))
            .map_err(StoreError::from)
    }

    fn presence(&self, workspace_id: &str, agent_id: &str) -> StoreResult<Option<PresenceRecord>> {
        self.transaction
            .query_row(
                "SELECT actor_id, actor_type, occurred_at, origin_event_seq FROM presence_current WHERE workspace_id = ?1 AND agent_id = ?2",
                params![workspace_id, agent_id],
                |row| {
                    let actor_type: String = row.get(1)?;
                    let occurred_at: String = row.get(2)?;
                    let timestamp = OffsetDateTime::parse(&occurred_at, &Rfc3339).map_err(|error| rusqlite::Error::FromSqlConversionFailure(2, rusqlite::types::Type::Text, Box::new(error)))?;
                    let actor_type = serde_json::from_value(serde_json::Value::String(actor_type)).map_err(|error| rusqlite::Error::FromSqlConversionFailure(1, rusqlite::types::Type::Text, Box::new(error)))?;
                    Ok(PresenceRecord {
                        workspace_id: workspace_id.into(), agent_id: agent_id.into(), actor_id: row.get(0)?, actor_type,
                        owner_id: None, parent_agent_id: None, parent_actor_id: None, goal_excerpt: None, phase: None,
                        next_plan: None, last_result: None, registered_at: timestamp, updated_at: timestamp,
                        expires_at: timestamp, busy_until: None, origin_event_seq: row.get(3)?,
                    })
                },
            )
            .optional()
            .map_err(StoreError::from)
    }

    fn active_claims_for_path(&self, workspace_id: &str, path: &str) -> StoreResult<Vec<ClaimRecord>> {
        let mut statement = self.transaction.prepare(
            "SELECT aggregate_id, path, expires_at, origin_event_seq FROM claim_current WHERE workspace_id = ?1 AND path = ?2",
        )?;
        statement.query_map(params![workspace_id, path], |row| Ok(ClaimRecord {
            workspace_id: workspace_id.into(), claim_id: row.get(0)?, path: row.get(1)?, expires_at: row.get(2)?, origin_event_seq: row.get(3)?,
        }))?.collect::<Result<Vec<_>, _>>().map_err(StoreError::from)
    }

    fn active_fence_for_path(&self, workspace_id: &str, path: &str) -> StoreResult<Option<WriteFenceRecord>> {
        self.transaction.query_row(
            "SELECT aggregate_id, path, expires_at, origin_event_seq FROM write_fence_current WHERE workspace_id = ?1 AND path = ?2",
            params![workspace_id, path],
            |row| Ok(WriteFenceRecord { workspace_id: workspace_id.into(), fence_id: row.get(0)?, path: row.get(1)?, expires_at: row.get(2)?, origin_event_seq: row.get(3)? }),
        ).optional().map_err(StoreError::from)
    }

    fn stable_observation(&self, workspace_id: &str, agent_id: &str, path: &str) -> StoreResult<Option<ReadObservationRecord>> {
        self.transaction.query_row(
            "SELECT origin_event_seq FROM read_observation_current WHERE workspace_id = ?1 AND agent_id = ?2 AND path = ?3",
            params![workspace_id, agent_id, path],
            |row| Ok(ReadObservationRecord { workspace_id: workspace_id.into(), agent_id: agent_id.into(), path: path.into(), origin_event_seq: row.get(0)? }),
        ).optional().map_err(StoreError::from)
    }
}

impl Store {
    pub fn execute_command<R>(
        &mut self,
        request: &RequestEnvelope<impl Serialize>,
        route_kind: &'static str,
        build: impl FnOnce(&dyn ProjectionReader) -> StoreResult<CommandPlan<R>>,
    ) -> StoreResult<CommandOutcome<R>>
    where
        R: Serialize + DeserializeOwned + Clone,
    {
        request.validate().map_err(StoreError::V2)?;
        let request_sha256 = normalized_request_sha256(request)?;
        let transaction = self.conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        if let Some(receipt) = load_receipt::<R>(&transaction, request.request_id)? {
            if receipt.route_kind != route_kind || receipt.request_sha256 != request_sha256 || receipt.agent_id != request.agent.agent_id || receipt.workspace_id != request.workspace.workspace_id || receipt.actor_id != request.agent.actor_id {
                return Err(StoreError::IdempotencyKeyReused);
            }
            return Ok(CommandOutcome { response: receipt.response, http_status: receipt.http_status, first_event_seq: receipt.first_event_seq, last_event_seq: receipt.last_event_seq, duplicate: true });
        }

        let plan = build(&SqlProjectionReader { transaction: &transaction })?;
        let occurred_at = format_timestamp(self.clock.now());
        let mut projector = Projector::new(&transaction, "", self.projector_fail_on_event);
        let mut first_event_seq = None;
        let mut last_event_seq = None;
        for event in plan.events {
            if event.request_id != request.request_id {
                return Err(StoreError::InvalidCommandEvent);
            }
            let normalized = NewEvent::new(request.request_id, event.event_ordinal, self.clock.now(), event.payload)
                .map_err(StoreError::V2)?;
            let event_seq = insert_journal_event(&transaction, &normalized, request, &occurred_at)?;
            let stored = normalized.into_stored(event_seq).map_err(StoreError::V2)?;
            let journal_event = JournalEvent {
                stored, agent_id: request.agent.agent_id.clone(), workspace_id: request.workspace.workspace_id.clone(),
                actor_id: request.agent.actor_id.clone(), actor_type: actor_type_name(&request.agent.actor_type).into(), occurred_at: occurred_at.clone(),
            };
            projector.apply(&journal_event)?;
            first_event_seq.get_or_insert(event_seq);
            last_event_seq = Some(event_seq);
        }
        let response_json = serde_json::to_string(&plan.response)?;
        transaction.execute(
            "INSERT INTO command_receipts (request_id, route_kind, request_sha256, agent_id, actor_id, workspace_id, http_status, response_json, first_event_seq, last_event_seq, committed_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            params![request.request_id.to_string(), route_kind, request_sha256, request.agent.agent_id, request.agent.actor_id, request.workspace.workspace_id, plan.http_status, response_json, first_event_seq, last_event_seq, occurred_at],
        )?;
        transaction.commit()?;
        Ok(CommandOutcome { response: plan.response, http_status: plan.http_status, first_event_seq, last_event_seq, duplicate: false })
    }

    pub fn rebuild_projections(&mut self) -> StoreResult<ReplayReport> {
        let transaction = self.conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let canonical = projection_snapshot(&transaction, "")?;
        Projector::create_replay_tables(&transaction)?;
        let events = load_journal_events(&transaction)?;
        let mut projector = Projector::new(&transaction, "replay_", None);
        for event in &events { projector.apply(event)?; }
        let replay = projection_snapshot(&transaction, "replay_")?;
        if canonical != replay { return Err(StoreError::ReplayMismatch); }
        let projection_rows = replay.values().map(|rows| rows.len() as u64).sum();
        let canonical_sha256 = sha256_bytes(&serde_json::to_vec(&replay)?);
        Projector::swap_replay_tables(&transaction)?;
        transaction.commit()?;
        Ok(ReplayReport { projectable_events: events.len() as u64, projection_rows, canonical_sha256 })
    }

    pub fn journal_event_count(&self) -> StoreResult<u64> {
        self.conn.query_row("SELECT COUNT(*) FROM journal_events", [], |row| row.get(0)).map_err(StoreError::from)
    }

    pub fn command_receipt_count(&self) -> StoreResult<u64> {
        self.conn.query_row("SELECT COUNT(*) FROM command_receipts", [], |row| row.get(0)).map_err(StoreError::from)
    }

    pub fn journal_event_ids(&self) -> StoreResult<Vec<String>> {
        let mut statement = self.conn.prepare("SELECT event_id FROM journal_events ORDER BY event_seq")?;
        statement.query_map([], |row| row.get(0))?.collect::<Result<Vec<_>, _>>().map_err(StoreError::from)
    }

    pub fn projection_row_count(&self) -> StoreResult<u64> {
        Ok(projection_snapshot(&self.conn, "")?.values().map(|rows| rows.len() as u64).sum())
    }

    pub fn projection_snapshot(&self) -> StoreResult<BTreeMap<String, Vec<Vec<String>>>> {
        projection_snapshot(&self.conn, "")
    }

    pub fn workspace_version(&self, workspace_id: &str) -> StoreResult<u64> {
        self.conn.query_row("SELECT version FROM workspace_version WHERE workspace_id = ?1", [workspace_id], |row| row.get(0)).optional().map(|value: Option<u64>| value.unwrap_or(0)).map_err(StoreError::from)
    }

    #[doc(hidden)]
    pub fn fail_projector_on_event_for_tests(&mut self, event_number: u32) {
        self.projector_fail_on_event = Some(event_number);
    }
}

struct Receipt<R> { route_kind: String, request_sha256: String, agent_id: String, actor_id: String, workspace_id: String, response: R, http_status: u16, first_event_seq: Option<u64>, last_event_seq: Option<u64> }

fn load_receipt<R: DeserializeOwned>(transaction: &Transaction<'_>, request_id: Uuid) -> StoreResult<Option<Receipt<R>>> {
    transaction.query_row(
        "SELECT route_kind, request_sha256, agent_id, actor_id, workspace_id, response_json, http_status, first_event_seq, last_event_seq FROM command_receipts WHERE request_id = ?1",
        [request_id.to_string()],
        |row| {
            let response_json: String = row.get(5)?;
            Ok(Receipt { route_kind: row.get(0)?, request_sha256: row.get(1)?, agent_id: row.get(2)?, actor_id: row.get(3)?, workspace_id: row.get(4)?, response: serde_json::from_str(&response_json).map_err(|error| rusqlite::Error::FromSqlConversionFailure(5, rusqlite::types::Type::Text, Box::new(error)))?, http_status: row.get(6)?, first_event_seq: row.get(7)?, last_event_seq: row.get(8)? })
        },
    ).optional().map_err(StoreError::from)
}

fn insert_journal_event(transaction: &Transaction<'_>, event: &NewEvent, request: &RequestEnvelope<impl Serialize>, occurred_at: &str) -> StoreResult<u64> {
    let payload_json = serde_json::to_string(&event.payload)?;
    transaction.query_row(
        "INSERT INTO journal_events (event_id, request_id, event_ordinal, agent_id, turn_id, workspace_id, repo_id, worktree_id, root, branch, aggregate_kind, aggregate_id, event_type, event_schema_version, actor_id, actor_type, owner_id, parent_agent_id, parent_actor_id, source_kind, source_ref, causation_id, correlation_id, occurred_at, affects_context, payload_json) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, 1, ?14, ?15, ?16, ?17, ?18, ?19, ?20, NULL, NULL, ?21, ?22, ?23) RETURNING event_seq",
        params![event.event_id.to_string(), request.request_id.to_string(), event.event_ordinal, request.agent.agent_id, request.agent.turn_id, request.workspace.workspace_id, request.workspace.repo_id, request.workspace.worktree_id, request.workspace.root, request.workspace.branch, event.aggregate_kind, event.aggregate_id, event.event_type, request.agent.actor_id, actor_type_name(&request.agent.actor_type), request.agent.owner_id.as_deref(), request.agent.parent_agent_id.as_deref(), request.agent.parent_actor_id.as_deref(), source_kind_name(&request.source.kind), request.source.source_ref, occurred_at, event.affects_context as i64, payload_json],
        |row| row.get(0),
    ).map_err(StoreError::from)
}

fn load_journal_events(connection: &Connection) -> StoreResult<Vec<JournalEvent>> {
    let mut statement = connection.prepare("SELECT event_seq, event_id, request_id, event_ordinal, agent_id, workspace_id, actor_id, actor_type, occurred_at, payload_json FROM journal_events ORDER BY event_seq")?;
    statement.query_map([], |row| {
        let event_seq: u64 = row.get(0)?;
        let event_id: String = row.get(1)?;
        let request_id: String = row.get(2)?;
        let event_ordinal: u32 = row.get(3)?;
        let occurred_at: String = row.get(8)?;
        let payload_json: String = row.get(9)?;
        let payload: EventPayload = serde_json::from_str(&payload_json).map_err(|error| rusqlite::Error::FromSqlConversionFailure(9, rusqlite::types::Type::Text, Box::new(error)))?;
        let event = NewEvent::new(Uuid::parse_str(&request_id).map_err(|error| rusqlite::Error::FromSqlConversionFailure(2, rusqlite::types::Type::Text, Box::new(error)))?, event_ordinal, OffsetDateTime::parse(&occurred_at, &Rfc3339).map_err(|error| rusqlite::Error::FromSqlConversionFailure(8, rusqlite::types::Type::Text, Box::new(error)))?, payload).map_err(|error| rusqlite::Error::FromSqlConversionFailure(9, rusqlite::types::Type::Text, Box::new(error)))?;
        let mut event = event;
        event.event_id = Uuid::parse_str(&event_id).map_err(|error| rusqlite::Error::FromSqlConversionFailure(1, rusqlite::types::Type::Text, Box::new(error)))?;
        let stored = event.into_stored(event_seq).map_err(|error| rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Integer, Box::new(error)))?;
        Ok(JournalEvent { stored, agent_id: row.get(4)?, workspace_id: row.get(5)?, actor_id: row.get(6)?, actor_type: row.get(7)?, occurred_at })
    })?.collect::<Result<Vec<_>, _>>().map_err(StoreError::from)
}

fn projection_snapshot(connection: &Connection, prefix: &str) -> StoreResult<BTreeMap<String, Vec<Vec<String>>>> {
    let mut snapshots = BTreeMap::new();
    for table in PROJECTION_TABLES {
        let name = format!("{prefix}{table}");
        let mut statement = connection.prepare(&format!("SELECT * FROM {name} ORDER BY rowid"))?;
        let rows = statement.query_map([], |row| {
            (0..row.as_ref().column_count()).map(|index| match row.get_ref(index)? {
                ValueRef::Null => Ok("null".into()), ValueRef::Integer(value) => Ok(format!("i:{value}")), ValueRef::Real(value) => Ok(format!("r:{value}")), ValueRef::Text(value) => Ok(format!("t:{}", String::from_utf8_lossy(value))), ValueRef::Blob(value) => Ok(format!("b:{value:?}")),
            }).collect::<Result<Vec<_>, rusqlite::Error>>()
        })?.collect::<Result<Vec<_>, _>>()?;
        snapshots.insert((*table).into(), rows);
    }
    Ok(snapshots)
}

fn normalized_request_sha256(payload: &impl Serialize) -> StoreResult<String> {
    let mut value = serde_json::to_value(payload)?;
    normalize_json(&mut value);
    Ok(sha256_bytes(&serde_json::to_vec(&value)?))
}

fn normalize_json(value: &mut serde_json::Value) {
    match value {
        serde_json::Value::Array(values) => for value in values { normalize_json(value); },
        serde_json::Value::Object(values) => {
            let ordered = std::mem::take(values).into_iter().map(|(key, mut value)| { normalize_json(&mut value); (key, value) }).collect::<BTreeMap<_, _>>();
            *values = ordered.into_iter().collect();
        }
        _ => {}
    }
}

fn sha256_bytes(bytes: &[u8]) -> String { format!("{:x}", Sha256::digest(bytes)) }

fn format_timestamp(timestamp: OffsetDateTime) -> String { timestamp.format(&Rfc3339).expect("RFC3339 formatting is valid") }

fn actor_type_name(actor_type: &ActorType) -> &'static str {
    match actor_type {
        ActorType::Agent => "agent",
        ActorType::Subagent => "subagent",
        ActorType::Human => "human",
        ActorType::System => "system",
        ActorType::Unknown => "unknown",
    }
}

fn source_kind_name(source_kind: &stateful_core::SourceKind) -> &'static str {
    match source_kind {
        stateful_core::SourceKind::Hook => "hook",
        stateful_core::SourceKind::Cli => "cli",
        stateful_core::SourceKind::Watcher => "watcher",
        stateful_core::SourceKind::Ide => "ide",
        stateful_core::SourceKind::Server => "server",
    }
}
