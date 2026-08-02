mod migration_v2;
mod models_v2;
mod schema_v2;
mod store_v2;

use std::{io, path::Path, time::Duration as StdDuration};

use rusqlite::Connection;
use thiserror::Error;

pub use models_v2::{
    AuditRecord, CommandContext, LeaseActivateInput, LeaseActivateResult, LeaseReleaseInput,
    LeaseReleaseResult, LeaseReleaseStatus, LeaseRequestState, LeaseRequestStatus,
    ReadCommandResult, ReadCompleteInput, ReadResultStatus, ReadStartInput, RuntimeProcessInput,
    StatusSnapshot, TaskCommandResult, TaskEndInput, TaskHeartbeatInput, TaskStartInput,
    WriteCompleteInput, WriteCompleteResult, WritePrepareInput, WritePrepareResult,
    WriteResultStatus, WriteTerminal,
};

const SQLITE_BUSY_TIMEOUT_MS: u64 = 5_000;

#[derive(Debug, Error)]
pub enum StoreError {
    #[error("io error: {0}")]
    Io(#[from] io::Error),
    #[error("sqlite error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("invalid timestamp: {0}")]
    InvalidTimestamp(String),
    #[error("invalid input: {0}")]
    InvalidInput(String),
    #[error("idempotency key was reused with a different command or payload")]
    IdempotencyMismatch,
    #[error("not found: {0}")]
    NotFound(String),
    #[error("ownership violation: {0}")]
    Ownership(String),
    #[error("invalid state: {0}")]
    InvalidState(String),
    #[error("corrupt state: {0}")]
    Corrupt(String),
}

pub type StoreResult<T> = Result<T, StoreError>;

pub struct Store {
    pub(crate) conn: Connection,
}

impl Store {
    pub fn open(path: impl AsRef<Path>) -> StoreResult<Self> {
        Ok(Self {
            conn: migration_v2::open_file(path.as_ref())?,
        })
    }

    pub fn open_in_memory() -> StoreResult<Self> {
        Ok(Self {
            conn: migration_v2::open_memory()?,
        })
    }
}

pub type SharedStore = std::sync::Arc<std::sync::Mutex<Store>>;

pub(crate) fn create_v2_schema(connection: &Connection) -> StoreResult<()> {
    schema_v2::create_v2_schema(connection)
}

pub(crate) fn configure_file_connection(connection: &Connection) -> StoreResult<()> {
    configure_connection(connection)?;
    connection.pragma_update(None, "journal_mode", "WAL")?;
    Ok(())
}

pub(crate) fn configure_connection(connection: &Connection) -> StoreResult<()> {
    connection.busy_timeout(StdDuration::from_millis(SQLITE_BUSY_TIMEOUT_MS))?;
    connection.pragma_update(None, "foreign_keys", true)?;
    Ok(())
}
