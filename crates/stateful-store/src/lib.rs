mod activity;
mod claims;
mod clock;
mod handoff;
mod human;
mod journal;
mod migration;
mod notifications;
mod outbox;
mod presence;
mod projector;
mod reservations;
mod schema;
mod write_fences;
pub use activity::{ActivityFinalization, ActivityStart};
pub use claims::{ClaimAcquire, ClaimBatchAcquireResult, ClaimObservation, ClaimPath, ClaimRelease, ClaimRecord};
pub use clock::{Clock, FixedClock, SystemClock};
pub use human::{
    HumanObservationConfidence, HumanObservationInput, HumanObservationKind,
    HumanObservationRecord, ReconciliationAckInput,
    HumanReconciliationAcknowledgementRecord,
};
pub use journal::{
    CommandOutcome, CommandPlan, CurrentAggregate, CurrentRecord, ProjectionReader,
    ReadObservationRecord, ReplayReport,
};
pub use notifications::{
    DeliveryAttempt, DeliveryRecord, NotificationCreate, NotificationDelivery, NotificationRecord,
};
pub use outbox::{OutboxDelivery, OutboxEntry, OutboxRecord, SyncStatus};
pub use presence::{
    PresenceRegistration, PresenceResourceUpdate, PresenceToolResult, PresenceToolStart,
};
pub use reservations::{
    ReservationDeclaration, ReservationHeartbeat, ReservationRecord, ReservationRelease,
    WaitCancellation, WaitGrant, WaitRecord, WaitRequest,
};
pub use stateful_core::PresenceRecord;
pub use write_fences::{WriteFenceAcquire, WriteFenceRecord, WriteFenceRelease};

use rusqlite::Connection;
use serde_json::Value;
use std::cell::RefCell;
use std::fs;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration as StdDuration;
use thiserror::Error;

pub const CRATE_NAME: &str = "stateful-store";
const SQLITE_BUSY_TIMEOUT_MS: u64 = 5_000;

#[derive(Debug, Error)]
pub enum StoreError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("sqlite error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("serialization error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("v2 protocol error: {0}")]
    V2(#[from] stateful_core::V2Error),
    #[error("idempotency key was reused with a different command")]
    IdempotencyKeyReused,
    #[error("command contains an invalid event")]
    InvalidCommandEvent,
    #[error("persisted journal event metadata is invalid")]
    InvalidJournalEvent,
    #[error("legacy v1 migration validation failed: {0}")]
    MigrationValidation(String),
    #[error("projector failure injected for test")]
    ProjectorFailure,
    #[error("replayed projection differs from canonical projection")]
    ReplayMismatch,
    #[error("reservation owner mismatch")]
    ReservationOwnerMismatch,
    #[error("reservation request not found")]
    ReservationRequestNotFound,
    #[error("reservation request owner mismatch")]
    ReservationRequestOwnerMismatch,
    #[error("reservation request is not cancelable")]
    ReservationRequestNotCancelable,
    #[error("active claim conflict")]
    ClaimConflict,
    #[error("claim is already held by this session")]
    ClaimAlreadyHeld,
    #[error("claim not found")]
    ClaimNotFound,
    #[error("claim owner mismatch")]
    ClaimOwnerMismatch,
    #[error("matching active reservation is required")]
    MissingReservation,
    #[error("write fence conflict on `{path}` held by `{owner_agent_id}`")]
    WriteFenceConflict { path: String, owner_agent_id: String },
    #[error("invalid claim path `{0}`: direct tmp claims are not allowed; claim a file or subdirectory under tmp instead")]
    InvalidClaimPath(String),
    #[error("purpose is required")]
    MissingPurpose,
    #[error("reservation scope is required")]
    MissingScope,
    #[error("invalid timestamp: {0}")]
    InvalidTimestamp(String),
}

pub type StoreResult<T> = Result<T, StoreError>;

impl StoreError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::IdempotencyKeyReused => "idempotency_key_reused",
            Self::MigrationValidation(_) => "migration_validation",
            _ => "store_error",
        }
    }
}

pub struct Store {
    conn: Connection,
    clock: clock::SharedClock,
    projector_fail_on_event: Option<u32>,
    corrupt_next_journal_metadata_for_tests: RefCell<Option<(String, String)>>,
}

impl Store {
    pub fn open(path: impl AsRef<Path>) -> StoreResult<Self> {
        Self::open_persistent(path)
    }

    pub fn open_persistent(path: impl AsRef<Path>) -> StoreResult<Self> {
        Self::open_with_clock(path, SystemClock)
    }

    pub fn open_with_clock(path: impl AsRef<Path>, clock: impl Clock + 'static) -> StoreResult<Self> {
        let path = path.as_ref();
        prepare_database_path(path)?;
        let _migration_guard = migration::MigrationGuard::acquire(path)?;
        let conn = Connection::open(path)?;
        configure_file_connection(&conn)?;
        let mut store = Self {
            conn,
            clock: Arc::new(clock),
            projector_fail_on_event: None,
            corrupt_next_journal_metadata_for_tests: RefCell::new(None),
        };
        migration::migrate_persistent_v1(&store.conn, path, store.clock.as_ref())?;
        schema::create_v2_schema(&store.conn)?;
        store.startup_housekeeping()?;
        Ok(store)
    }

    pub fn open_in_memory() -> StoreResult<Self> {
        Self::open_in_memory_with_clock(SystemClock)
    }

    pub fn open_in_memory_with_clock(clock: impl Clock + 'static) -> StoreResult<Self> {
        let conn = Connection::open_in_memory()?;
        configure_connection(&conn)?;
        let mut store = Self {
            conn,
            clock: Arc::new(clock),
            projector_fail_on_event: None,
            corrupt_next_journal_metadata_for_tests: RefCell::new(None),
        };
        schema::create_v2_schema(&store.conn)?;
        store.startup_housekeeping()?;
        Ok(store)
    }

    pub(crate) fn current_records(
        &self,
        aggregate: CurrentAggregate,
        workspace_id: &str,
    ) -> StoreResult<Vec<CurrentRecord>> {
        let table = match aggregate {
            CurrentAggregate::Reservation => "reservation_current",
            CurrentAggregate::Claim => "claim_current",
            CurrentAggregate::Wait => "wait_current",
            CurrentAggregate::WriteFence => "write_fence_current",
            CurrentAggregate::HumanObservation => "human_observation_current",
            CurrentAggregate::HumanAcknowledgement => "human_acknowledgement_current",
            CurrentAggregate::Notification => "notification_current",
            CurrentAggregate::Delivery => "delivery_current",
        };
        let mut statement = self.conn.prepare(&format!(
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

    pub fn has_table(&self, table_name: &str) -> StoreResult<bool> {
        self.schema_object_exists("table", table_name)
    }

    pub fn has_index(&self, index_name: &str) -> StoreResult<bool> {
        self.schema_object_exists("index", index_name)
    }

    fn schema_object_exists(&self, object_type: &str, object_name: &str) -> StoreResult<bool> {
        self.conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = ?1 AND name = ?2)",
                [object_type, object_name],
                |row| row.get(0),
            )
            .map_err(StoreError::from)
    }

    pub fn recent_events(&self, limit: u64) -> StoreResult<Vec<EventRecord>> {
        let mut statement = self.conn.prepare(
            "SELECT event_id, event_type, agent_id, workspace_id, repo_id, worktree_id, root, branch,
                    payload_json, occurred_at
             FROM journal_events ORDER BY event_seq DESC LIMIT ?1",
        )?;
        statement
            .query_map([limit], |row| {
                let payload: String = row.get(8)?;
                Ok(EventRecord {
                    event_id: row.get(0)?,
                    event_type: row.get(1)?,
                    agent_id: row.get(2)?,
                    workspace_id: row.get(3)?,
                    repo_id: row.get(4)?,
                    worktree_id: row.get(5)?,
                    root: row.get(6)?,
                    branch: row.get(7)?,
                    payload: serde_json::from_str(&payload).map_err(|error| {
                        rusqlite::Error::FromSqlConversionFailure(8, rusqlite::types::Type::Text, Box::new(error))
                    })?,
                    created_at: row.get(9)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()
            .map_err(StoreError::from)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct EventRecord {
    pub event_id: String,
    pub event_type: String,
    pub agent_id: String,
    pub workspace_id: String,
    pub repo_id: Option<String>,
    pub worktree_id: Option<String>,
    pub root: Option<String>,
    pub branch: Option<String>,
    pub payload: Value,
    pub created_at: String,
}

fn prepare_database_path(path: &Path) -> StoreResult<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    Ok(())
}

fn configure_connection(conn: &Connection) -> StoreResult<()> {
    conn.busy_timeout(StdDuration::from_millis(SQLITE_BUSY_TIMEOUT_MS))?;
    Ok(())
}

fn configure_file_connection(conn: &Connection) -> StoreResult<()> {
    configure_connection(conn)?;
    conn.pragma_update(None, "journal_mode", "WAL")?;
    Ok(())
}
