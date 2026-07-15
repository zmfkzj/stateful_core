use crate::{
    Store, StoreError, StoreResult,
    projector::Projector,
    schema::PROJECTION_TABLES,
};
use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params, types::ValueRef};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use sha2::{Digest, Sha256};
use stateful_core::{
    ActorType, AgentIdentity, EventPayload, HandoffRecord, NewEvent, PresenceRecord,
    PresenceResource, PresenceResourceRelation, RequestEnvelope, SourceKind, SourceRef,
    StoredEvent, WorkspaceIdentity,
};
use std::collections::BTreeMap;
use time::{OffsetDateTime, format_description::well_known::Rfc3339};
use uuid::Uuid;

pub trait ProjectionReader {
    fn workspace_version(&self, workspace_id: &str) -> StoreResult<u64>;
    fn presence(&self, workspace_id: &str, agent_id: &str) -> StoreResult<Option<PresenceRecord>>;
    fn presence_resource(&self, workspace_id: &str, agent_id: &str, path: &str, relation: PresenceResourceRelation) -> StoreResult<Option<PresenceResource>>;
    fn presence_resources(&self, workspace_id: &str, agent_id: &str) -> StoreResult<Vec<PresenceResource>>;
    fn live_presences(&self, workspace_id: &str) -> StoreResult<Vec<PresenceRecord>>;
    fn handoff(&self, workspace_id: &str, agent_id: &str) -> StoreResult<Option<HandoffRecord>>;
    fn handoffs(&self, workspace_id: &str) -> StoreResult<Vec<HandoffRecord>>;
    fn active_claims_for_path(&self, workspace_id: &str, path: &str) -> StoreResult<Vec<ClaimRecord>>;
    fn active_fence_for_path(&self, workspace_id: &str, path: &str) -> StoreResult<Option<WriteFenceRecord>>;
    fn stable_observation(&self, workspace_id: &str, agent_id: &str, path: &str) -> StoreResult<Option<ReadObservationRecord>>;
    fn aggregate_records(
        &self,
        kind: CurrentAggregate,
        workspace_id: &str,
    ) -> StoreResult<Vec<CurrentRecord>>;
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CurrentAggregate {
    Reservation,
    Claim,
    Wait,
    WriteFence,
    HumanObservation,
    Notification,
    Delivery,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CurrentRecord {
    pub aggregate_id: String,
    pub payload: serde_json::Value,
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


pub(crate) struct MigrationJournalMetadata<'a> {
    pub(crate) agent_id: &'a str,
    pub(crate) workspace_id: &'a str,
    pub(crate) repo_id: &'a str,
    pub(crate) worktree_id: &'a str,
    pub(crate) root: &'a str,
    pub(crate) branch: &'a str,
}

pub(crate) fn append_migration_event(
    connection: &Connection,
    event: &NewEvent,
    metadata: MigrationJournalMetadata<'_>,
) -> StoreResult<JournalEvent> {
    let payload_json = serde_json::to_string(&event.payload)?;
    let occurred_at = format_timestamp(event.observed_at);
    let event_seq = connection.query_row(
        "INSERT INTO journal_events (event_id, request_id, event_ordinal, agent_id, turn_id, workspace_id, repo_id, worktree_id, root, branch, aggregate_kind, aggregate_id, event_type, event_schema_version, actor_id, actor_type, owner_id, parent_agent_id, parent_actor_id, source_kind, source_ref, causation_id, correlation_id, occurred_at, affects_context, payload_json) VALUES (?1, ?2, ?3, ?4, NULL, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, 1, 'unknown', 'unknown', NULL, NULL, NULL, 'server', 'stateful.v1-migration', NULL, NULL, ?13, ?14, ?15) RETURNING event_seq",
        params![
            event.event_id.to_string(),
            event.request_id.to_string(),
            event.event_ordinal,
            metadata.agent_id,
            metadata.workspace_id,
            metadata.repo_id,
            metadata.worktree_id,
            metadata.root,
            metadata.branch,
            event.aggregate_kind,
            event.aggregate_id,
            event.event_type,
            occurred_at,
            event.affects_context as i64,
            payload_json,
        ],
        |row| row.get(0),
    )?;
    load_journal_event(connection, event_seq, None)
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
        self.transaction.query_row(
            "SELECT payload_json, origin_event_seq FROM presence_current WHERE workspace_id = ?1 AND agent_id = ?2",
            params![workspace_id, agent_id],
            |row| presence_from_row(row),
        ).optional().map_err(StoreError::from)
    }

    fn presence_resource(
        &self,
        workspace_id: &str,
        agent_id: &str,
        path: &str,
        relation: PresenceResourceRelation,
    ) -> StoreResult<Option<PresenceResource>> {
        let relation = relation_name(relation);
        self.transaction.query_row(
            "SELECT payload_json, origin_event_seq FROM presence_resource_current WHERE workspace_id = ?1 AND agent_id = ?2 AND relative_path = ?3 AND relation = ?4",
            params![workspace_id, agent_id, path, relation],
            |row| presence_resource_from_row(row),
        ).optional().map_err(StoreError::from)
    }

    fn presence_resources(&self, workspace_id: &str, agent_id: &str) -> StoreResult<Vec<PresenceResource>> {
        let mut statement = self.transaction.prepare(
            "SELECT payload_json, origin_event_seq FROM presence_resource_current WHERE workspace_id = ?1 AND agent_id = ?2 ORDER BY relative_path, relation",
        )?;
        statement.query_map(params![workspace_id, agent_id], presence_resource_from_row)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(StoreError::from)
    }

    fn live_presences(&self, workspace_id: &str) -> StoreResult<Vec<PresenceRecord>> {
        let mut statement = self.transaction.prepare(
            "SELECT payload_json, origin_event_seq FROM presence_current WHERE workspace_id = ?1 ORDER BY agent_id",
        )?;
        statement.query_map([workspace_id], presence_from_row)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(StoreError::from)
    }

    fn handoff(&self, workspace_id: &str, agent_id: &str) -> StoreResult<Option<HandoffRecord>> {
        self.transaction.query_row(
            "SELECT payload_json, origin_event_seq FROM handoff_current WHERE workspace_id = ?1 AND aggregate_id = ?2",
            params![workspace_id, agent_id],
            |row| handoff_from_row(row),
        ).optional().map_err(StoreError::from)
    }

    fn handoffs(&self, workspace_id: &str) -> StoreResult<Vec<HandoffRecord>> {
        let mut statement = self.transaction.prepare(
            "SELECT payload_json, origin_event_seq FROM handoff_current WHERE workspace_id = ?1 ORDER BY aggregate_id",
        )?;
        statement.query_map([workspace_id], handoff_from_row)?
            .collect::<Result<Vec<_>, _>>()
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

    fn aggregate_records(
        &self,
        kind: CurrentAggregate,
        workspace_id: &str,
    ) -> StoreResult<Vec<CurrentRecord>> {
        let table = match kind {
            CurrentAggregate::Reservation => "reservation_current",
            CurrentAggregate::Claim => "claim_current",
            CurrentAggregate::Wait => "wait_current",
            CurrentAggregate::WriteFence => "write_fence_current",
            CurrentAggregate::HumanObservation => "human_observation_current",
            CurrentAggregate::Notification => "notification_current",
            CurrentAggregate::Delivery => "delivery_current",
        };
        let mut statement = self.transaction.prepare(&format!(
            "SELECT aggregate_id, payload_json, origin_event_seq FROM {table}
             WHERE workspace_id = ?1 ORDER BY aggregate_id"
        ))?;
        statement
            .query_map([workspace_id], |row| {
                let payload: String = row.get(1)?;
                let payload = serde_json::from_str(&payload).map_err(|error| {
                    rusqlite::Error::FromSqlConversionFailure(
                        1,
                        rusqlite::types::Type::Text,
                        Box::new(error),
                    )
                })?;
                Ok(CurrentRecord {
                    aggregate_id: row.get(0)?,
                    payload,
                    origin_event_seq: row.get(2)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()
            .map_err(StoreError::from)
    }
}

fn presence_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<PresenceRecord> {
    let payload: String = row.get(0)?;
    let origin_event_seq = row.get(1)?;
    let mut record: PresenceRecord = serde_json::from_str(&payload)
        .map_err(|error| rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(error)))?;
    record.origin_event_seq = origin_event_seq;
    Ok(record)
}

fn presence_resource_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<PresenceResource> {
    let payload: String = row.get(0)?;
    let origin_event_seq = row.get(1)?;
    let mut resource: PresenceResource = serde_json::from_str(&payload)
        .map_err(|error| rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(error)))?;
    resource.origin_event_seq = origin_event_seq;
    Ok(resource)
}

fn handoff_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<HandoffRecord> {
    let payload: String = row.get(0)?;
    let origin_event_seq = row.get(1)?;
    let mut handoff: HandoffRecord = serde_json::from_str(&payload)
        .map_err(|error| rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(error)))?;
    handoff.origin_event_seq = origin_event_seq;
    Ok(handoff)
}

fn relation_name(relation: PresenceResourceRelation) -> &'static str {
    match relation {
        PresenceResourceRelation::Read => "read",
        PresenceResourceRelation::Planned => "planned",
        PresenceResourceRelation::Touched => "touched",
        PresenceResourceRelation::Changed => "changed",
    }
}

impl Store {
    pub fn execute_command<R>(
        &self,
        request: &RequestEnvelope<impl Serialize>,
        route_kind: &'static str,
        build: impl FnOnce(&dyn ProjectionReader) -> StoreResult<CommandPlan<R>>,
    ) -> StoreResult<CommandOutcome<R>>
    where
        R: Serialize + DeserializeOwned + Clone,
    {
        request.validate().map_err(StoreError::V2)?;
        let request_sha256 = normalized_request_sha256(request)?;
        let transaction = self.conn.unchecked_transaction()?;
        if let Some(receipt) = load_receipt(&transaction, request.request_id)? {
            if receipt.route_kind != route_kind || receipt.request_sha256 != request_sha256 || receipt.agent_id != request.agent.agent_id || receipt.workspace_id != request.workspace.workspace_id || receipt.actor_id != request.agent.actor_id {
                return Err(StoreError::IdempotencyKeyReused);
            }
            let response = serde_json::from_str(&receipt.response_json)?;
            return Ok(CommandOutcome { response, http_status: receipt.http_status, first_event_seq: receipt.first_event_seq, last_event_seq: receipt.last_event_seq, duplicate: true });
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
            if let Some((column, value)) = self.corrupt_next_journal_metadata_for_tests.borrow_mut().take() {
                corrupt_journal_field(&transaction, event_seq, &column, &value)?;
            }
            let expected = ExpectedJournalEnvelope::from_request(request);
            let journal_event = load_journal_event(&transaction, event_seq, Some(&expected))?;
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

    pub fn journal_event_types_for_request(&self, request_id: Uuid) -> StoreResult<Vec<String>> {
        let mut statement = self.conn.prepare(
            "SELECT event_type FROM journal_events WHERE request_id = ?1 ORDER BY event_seq",
        )?;
        statement.query_map([request_id.to_string()], |row| row.get(0))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(StoreError::from)
    }

    pub fn projection_row_count(&self) -> StoreResult<u64> {
        Ok(projection_snapshot(&self.conn, "")?.values().map(|rows| rows.len() as u64).sum())
    }

    pub fn projection_snapshot(&self) -> StoreResult<BTreeMap<String, Vec<Vec<String>>>> {
        projection_snapshot(&self.conn, "")
    }

    #[doc(hidden)]
    pub fn projection_schema_snapshot(&self) -> StoreResult<BTreeMap<String, Vec<Vec<String>>>> {
        projection_schema_snapshot(&self.conn)
    }

    #[doc(hidden)]
    pub fn corrupt_journal_metadata_for_tests(&mut self, column: &str, value: &str) -> StoreResult<()> {
        corrupt_journal_field(&self.conn, 0, column, value)
    }

    #[doc(hidden)]
    pub fn corrupt_next_journal_metadata_for_tests(&mut self, column: &str, value: &str) {
        *self.corrupt_next_journal_metadata_for_tests.borrow_mut() = Some((column.into(), value.into()));
    }

    pub fn workspace_version(&self, workspace_id: &str) -> StoreResult<u64> {
        self.conn.query_row("SELECT version FROM workspace_version WHERE workspace_id = ?1", [workspace_id], |row| row.get(0)).optional().map(|value: Option<u64>| value.unwrap_or(0)).map_err(StoreError::from)
    }

    #[doc(hidden)]
    pub fn fail_projector_on_event_for_tests(&mut self, event_number: u32) {
        self.projector_fail_on_event = Some(event_number);
    }
}

struct Receipt {
    route_kind: String,
    request_sha256: String,
    agent_id: String,
    actor_id: String,
    workspace_id: String,
    response_json: String,
    http_status: u16,
    first_event_seq: Option<u64>,
    last_event_seq: Option<u64>,
}

fn load_receipt(transaction: &Transaction<'_>, request_id: Uuid) -> StoreResult<Option<Receipt>> {
    transaction.query_row(
        "SELECT route_kind, request_sha256, agent_id, actor_id, workspace_id, response_json, http_status, first_event_seq, last_event_seq FROM command_receipts WHERE request_id = ?1",
        [request_id.to_string()],
        |row| Ok(Receipt {
            route_kind: row.get(0)?,
            request_sha256: row.get(1)?,
            agent_id: row.get(2)?,
            actor_id: row.get(3)?,
            workspace_id: row.get(4)?,
            response_json: row.get(5)?,
            http_status: row.get(6)?,
            first_event_seq: row.get(7)?,
            last_event_seq: row.get(8)?,
        }),
    ).optional().map_err(StoreError::from)
}

fn insert_journal_event(transaction: &Transaction<'_>, event: &NewEvent, request: &RequestEnvelope<impl Serialize>, occurred_at: &str) -> StoreResult<u64> {
    let payload_json = serde_json::to_string(&event.payload)?;
    transaction.query_row(
        "INSERT INTO journal_events (event_id, request_id, event_ordinal, agent_id, turn_id, workspace_id, repo_id, worktree_id, root, branch, aggregate_kind, aggregate_id, event_type, event_schema_version, actor_id, actor_type, owner_id, parent_agent_id, parent_actor_id, source_kind, source_ref, causation_id, correlation_id, occurred_at, affects_context, payload_json) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, 1, ?14, ?15, ?16, ?17, ?18, ?19, ?20, NULL, ?21, ?22, ?23, ?24) RETURNING event_seq",
        params![event.event_id.to_string(), request.request_id.to_string(), event.event_ordinal, request.agent.agent_id, request.agent.turn_id, request.workspace.workspace_id, request.workspace.repo_id, request.workspace.worktree_id, request.workspace.root, request.workspace.branch, event.aggregate_kind, event.aggregate_id, event.event_type, request.agent.actor_id, actor_type_name(&request.agent.actor_type), request.agent.owner_id.as_deref(), request.agent.parent_agent_id.as_deref(), request.agent.parent_actor_id.as_deref(), source_kind_name(&request.source.kind), request.source.source_ref, request.request_id.to_string(), occurred_at, event.affects_context as i64, payload_json],
        |row| row.get(0),
    ).map_err(StoreError::from)
}

const JOURNAL_EVENT_COLUMNS: &str = "event_seq, event_id, request_id, event_ordinal, agent_id, turn_id, workspace_id, repo_id, worktree_id, root, branch, aggregate_kind, aggregate_id, event_type, event_schema_version, actor_id, actor_type, owner_id, parent_agent_id, parent_actor_id, source_kind, source_ref, causation_id, correlation_id, occurred_at, affects_context, payload_json";

pub(crate) fn load_journal_events(connection: &Connection) -> StoreResult<Vec<JournalEvent>> {
    let mut statement = connection.prepare(&format!(
        "SELECT {JOURNAL_EVENT_COLUMNS} FROM journal_events ORDER BY event_seq"
    ))?;
    statement
        .query_map([], persisted_journal_event_from_row)?
        .collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .map(|event| event.validate(None))
        .collect()
}

fn load_journal_event(
    connection: &Connection,
    event_seq: u64,
    expected: Option<&ExpectedJournalEnvelope>,
) -> StoreResult<JournalEvent> {
    connection
        .query_row(
            &format!("SELECT {JOURNAL_EVENT_COLUMNS} FROM journal_events WHERE event_seq = ?1"),
            [event_seq],
            persisted_journal_event_from_row,
        )?
        .validate(expected)
}

fn persisted_journal_event_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<PersistedJournalEvent> {
    Ok(PersistedJournalEvent {
        event_seq: row.get(0)?,
        event_id: row.get(1)?,
        request_id: row.get(2)?,
        event_ordinal: row.get(3)?,
        agent_id: row.get(4)?,
        turn_id: row.get(5)?,
        workspace_id: row.get(6)?,
        repo_id: row.get(7)?,
        worktree_id: row.get(8)?,
        root: row.get(9)?,
        branch: row.get(10)?,
        aggregate_kind: row.get(11)?,
        aggregate_id: row.get(12)?,
        event_type: row.get(13)?,
        event_schema_version: row.get(14)?,
        actor_id: row.get(15)?,
        actor_type: row.get(16)?,
        owner_id: row.get(17)?,
        parent_agent_id: row.get(18)?,
        parent_actor_id: row.get(19)?,
        source_kind: row.get(20)?,
        source_ref: row.get(21)?,
        causation_id: row.get(22)?,
        correlation_id: row.get(23)?,
        occurred_at: row.get(24)?,
        affects_context: row.get(25)?,
        payload_json: row.get(26)?,
    })
}

struct ExpectedJournalEnvelope {
    request_id: String,
    agent: AgentIdentity,
    workspace: WorkspaceIdentity,
    source: SourceRef,
}

impl ExpectedJournalEnvelope {
    fn from_request<T>(request: &RequestEnvelope<T>) -> Self {
        Self {
            request_id: request.request_id.to_string(),
            agent: request.agent.clone(),
            workspace: request.workspace.clone(),
            source: request.source.clone(),
        }
    }
}

struct PersistedJournalEvent {
    event_seq: u64,
    event_id: String,
    request_id: String,
    event_ordinal: u32,
    agent_id: String,
    turn_id: Option<String>,
    workspace_id: String,
    repo_id: Option<String>,
    worktree_id: Option<String>,
    root: Option<String>,
    branch: Option<String>,
    aggregate_kind: String,
    aggregate_id: String,
    event_type: String,
    event_schema_version: u64,
    actor_id: String,
    actor_type: String,
    owner_id: Option<String>,
    parent_agent_id: Option<String>,
    parent_actor_id: Option<String>,
    source_kind: String,
    source_ref: String,
    causation_id: Option<String>,
    correlation_id: Option<String>,
    occurred_at: String,
    affects_context: i64,
    payload_json: String,
}

impl PersistedJournalEvent {
    fn validate(self, expected: Option<&ExpectedJournalEnvelope>) -> StoreResult<JournalEvent> {
        if self.event_schema_version != 1 || !(0..=1).contains(&self.affects_context) {
            return Err(StoreError::InvalidJournalEvent);
        }
        let actor_type = serde_json::from_value(serde_json::Value::String(self.actor_type.clone()))
            .map_err(|_| StoreError::InvalidJournalEvent)?;
        let agent = AgentIdentity {
            agent_id: self.agent_id.clone(),
            turn_id: self.turn_id.clone(),
            actor_id: self.actor_id.clone(),
            actor_type,
            owner_id: self.owner_id.clone(),
            parent_agent_id: self.parent_agent_id.clone(),
            parent_actor_id: self.parent_actor_id.clone(),
        };
        agent.validate().map_err(StoreError::V2)?;
        let workspace = WorkspaceIdentity {
            root: required_identity_field(self.root.as_deref())?,
            workspace_id: self.workspace_id.clone(),
            repo_id: required_identity_field(self.repo_id.as_deref())?,
            worktree_id: required_identity_field(self.worktree_id.as_deref())?,
            branch: required_identity_field(self.branch.as_deref())?,
        };
        workspace.validate().map_err(StoreError::V2)?;
        let source_kind: SourceKind = serde_json::from_value(serde_json::Value::String(self.source_kind.clone()))
            .map_err(|_| StoreError::InvalidJournalEvent)?;
        SourceRef {
            kind: source_kind.clone(),
            event: "journal".into(),
            tool_name: None,
            source_ref: self.source_ref.clone(),
        }.validate().map_err(StoreError::V2)?;
        for identifier in [&self.causation_id, &self.correlation_id] {
            if let Some(identifier) = identifier {
                Uuid::parse_str(identifier).map_err(|_| StoreError::InvalidJournalEvent)?;
            }
        }
        let payload: EventPayload = serde_json::from_str(&self.payload_json)?;
        let mut event = NewEvent::new(
            Uuid::parse_str(&self.request_id).map_err(|_| StoreError::InvalidJournalEvent)?,
            self.event_ordinal,
            OffsetDateTime::parse(&self.occurred_at, &Rfc3339).map_err(|_| StoreError::InvalidJournalEvent)?,
            payload,
        ).map_err(StoreError::V2)?;
        event.event_id = Uuid::parse_str(&self.event_id).map_err(|_| StoreError::InvalidJournalEvent)?;
        let stored = event.into_stored(self.event_seq).map_err(StoreError::V2)?;
        if stored.aggregate_kind() != self.aggregate_kind
            || stored.aggregate_id() != self.aggregate_id
            || stored.event_type() != self.event_type
            || stored.affects_context() != (self.affects_context == 1)
        {
            return Err(StoreError::InvalidJournalEvent);
        }
        if let Some(expected) = expected {
            if self.request_id != expected.request_id
                || agent.agent_id != expected.agent.agent_id
                || agent.turn_id != expected.agent.turn_id
                || agent.actor_id != expected.agent.actor_id
                || actor_type_name(&agent.actor_type) != actor_type_name(&expected.agent.actor_type)
                || agent.owner_id != expected.agent.owner_id
                || agent.parent_agent_id != expected.agent.parent_agent_id
                || agent.parent_actor_id != expected.agent.parent_actor_id
                || workspace.workspace_id != expected.workspace.workspace_id
                || workspace.repo_id != expected.workspace.repo_id
                || workspace.worktree_id != expected.workspace.worktree_id
                || workspace.root != expected.workspace.root
                || workspace.branch != expected.workspace.branch
                || source_kind_name(&source_kind) != source_kind_name(&expected.source.kind)
                || self.source_ref != expected.source.source_ref
                || self.causation_id.is_some()
                || self.correlation_id.as_deref() != Some(expected.request_id.as_str())
            {
                return Err(StoreError::InvalidJournalEvent);
            }
        }
        Ok(JournalEvent {
            stored,
            agent_id: self.agent_id,
            workspace_id: self.workspace_id,
            actor_id: self.actor_id,
            actor_type: self.actor_type,
            occurred_at: self.occurred_at,
        })
    }
}

fn required_identity_field(value: Option<&str>) -> StoreResult<String> {
    value
        .filter(|value| !value.trim().is_empty())
        .map(str::to_owned)
        .ok_or(StoreError::InvalidJournalEvent)
}

fn corrupt_journal_field(
    connection: &Connection,
    event_seq: u64,
    column: &str,
    value: &str,
) -> StoreResult<()> {
    if !matches!(column, "event_type" | "actor_type" | "affects_context" | "agent_id" | "source_ref") {
        return Err(StoreError::InvalidJournalEvent);
    }
    let query = if event_seq == 0 {
        format!("UPDATE journal_events SET {column} = ?1")
    } else {
        format!("UPDATE journal_events SET {column} = ?1 WHERE event_seq = ?2")
    };
    if event_seq == 0 {
        connection.execute(&query, [value])?;
    } else {
        connection.execute(&query, params![value, event_seq])?;
    }
    Ok(())
}

pub(crate) fn projection_snapshot(connection: &Connection, prefix: &str) -> StoreResult<BTreeMap<String, Vec<Vec<String>>>> {
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

fn projection_schema_snapshot(connection: &Connection) -> StoreResult<BTreeMap<String, Vec<Vec<String>>>> {
    let mut snapshots = BTreeMap::new();
    for table in PROJECTION_TABLES {
        let mut statement = connection.prepare(&format!("PRAGMA table_info({table})"))?;
        let columns = statement.query_map([], |row| {
            Ok(vec![
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, i64>(3)?.to_string(),
                row.get::<_, Option<String>>(4)?.unwrap_or_default(),
                row.get::<_, i64>(5)?.to_string(),
            ])
        })?.collect::<Result<Vec<_>, _>>()?;
        snapshots.insert((*table).into(), columns);
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
