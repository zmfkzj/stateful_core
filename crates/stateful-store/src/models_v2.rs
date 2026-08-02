use serde::{Deserialize, Serialize};
use stateful_core::{CoordinationSettings, MutationOperation, ResourceObservation, TaskStatus};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CommandContext {
    pub request_id: String,
    pub task_id: String,
    pub agent_id: String,
    pub workspace_id: String,
    pub observed_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RuntimeProcessInput {
    pub pid: u32,
    pub process_start_identity: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TaskStartInput {
    pub next_action: String,
    pub settings: CoordinationSettings,
    pub expires_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime_process: Option<RuntimeProcessInput>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TaskHeartbeatInput {
    pub next_action: String,
    pub expires_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TaskEndInput {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub handoff: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TaskCommandResult {
    pub task_id: String,
    pub status: TaskStatus,
    pub draining: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ReadStartInput {
    pub read_id: String,
    pub invocation_id: String,
    pub resources: Vec<ResourceObservation>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ReadCompleteInput {
    pub read_id: String,
    pub invocation_id: String,
    pub resources: Vec<ResourceObservation>,
    pub terminal_success: bool,
    pub complete: bool,
    pub stable: bool,
    pub exact: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ReadResultStatus {
    Started,
    Completed,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ReadCommandResult {
    pub read_id: String,
    pub status: ReadResultStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub evidence_id: Option<String>,
}

/// Canonical wire contract for `/v2/writes/prepare` and `/v2/commits/prepare`.
///
/// `request_expires_at` bounds queued/offered acquisition; `lease_expires_at`
/// independently bounds an active exclusive lease.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WritePrepareInput {
    pub invocation_id: String,
    pub operation: MutationOperation,
    pub current: Vec<ResourceObservation>,
    pub request_expires_at: String,
    pub lease_expires_at: String,
    pub attempt_deadline: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum WritePrepareResult {
    Ready {
        attempt_id: String,
        permit_id: String,
        lease_batch_ids: Vec<String>,
    },
    Queued {
        batch_id: String,
    },
    RereadRequired {
        lease_batch_ids: Vec<String>,
    },
    Denied {
        reason_code: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LeaseActivateInput {
    pub batch_id: String,
    pub offer_id: String,
    pub version: u64,
    pub lease_expires_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LeaseActivateResult {
    pub batch_id: String,
    pub active: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LeaseReleaseInput {
    pub batch_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum LeaseReleaseStatus {
    Released,
    Deferred,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LeaseReleaseResult {
    pub batch_id: String,
    pub status: LeaseReleaseStatus,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum WriteTerminal {
    Success,
    FailedKnown,
    Uncertain,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WriteCompleteInput {
    pub attempt_id: String,
    pub permit_id: String,
    pub invocation_id: String,
    pub terminal: WriteTerminal,
    pub post_resources: Vec<ResourceObservation>,
    pub expected_post_resources: Vec<ResourceObservation>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum WriteResultStatus {
    Completed,
    Failed,
    Uncertain,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WriteCompleteResult {
    pub attempt_id: String,
    pub status: WriteResultStatus,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum LeaseRequestState {
    Queued,
    Offered,
    Activated,
    Superseded,
    Expired,
    Cancelled,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LeaseRequestStatus {
    pub batch_id: String,
    pub state: LeaseRequestState,
    pub version: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub offer_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub offer_expires_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub superseded_by: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct StatusSnapshot {
    pub active_tasks: u64,
    pub draining_tasks: u64,
    pub active_leases: u64,
    pub draining_leases: u64,
    pub queued_requests: u64,
    pub offered_requests: u64,
    pub executing_writes: u64,
    pub uncertain_writes: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AuditRecord {
    pub event_id: String,
    pub workspace_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_id: Option<String>,
    pub agent_id: String,
    pub event_type: String,
    pub payload: serde_json::Value,
    pub created_at: String,
}
