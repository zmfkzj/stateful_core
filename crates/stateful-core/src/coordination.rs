use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskStatus {
    Active,
    Draining,
    Completed,
    Failed,
    Cancelled,
}

impl TaskStatus {
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Completed | Self::Failed | Self::Cancelled)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskRecord {
    pub task_id: String,
    pub agent_id: String,
    pub workspace_id: String,
    pub status: TaskStatus,
    pub next_action: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub handoff: Option<String>,
    pub heartbeat_at: String,
    pub expires_at: String,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct CoordinationSettings {
    pub heartbeat_interval_seconds: u64,
    pub inactivity_timeout_seconds: u64,
    pub lease_expiry_seconds: u64,
    pub offer_ttl_seconds: u64,
}

impl Default for CoordinationSettings {
    fn default() -> Self {
        Self {
            heartbeat_interval_seconds: 1,
            inactivity_timeout_seconds: 5,
            lease_expiry_seconds: 60,
            offer_ttl_seconds: 120,
        }
    }
}

impl CoordinationSettings {
    pub fn validate(self) -> Result<(), String> {
        if self.heartbeat_interval_seconds == 0
            || self.inactivity_timeout_seconds == 0
            || self.lease_expiry_seconds == 0
            || self.offer_ttl_seconds == 0
        {
            return Err("coordination settings must be positive".to_string());
        }
        let inactivity_third = self.inactivity_timeout_seconds / 3;
        if self.heartbeat_interval_seconds > inactivity_third
            || inactivity_third >= self.lease_expiry_seconds
        {
            return Err(
                "heartbeat must be at most one third of inactivity timeout, below lease expiry"
                    .to_string(),
            );
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResourceKind {
    Object,
    Entry,
    DirectoryTree,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct ResourceKey {
    pub workspace_id: String,
    pub kind: ResourceKind,
    pub resource_id: String,
    pub canonical_path: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DigestAlgorithm {
    Blake3,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContentDigest {
    pub algorithm: DigestAlgorithm,
    pub value: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ObjectKind {
    RegularFile,
    Directory,
    Symlink,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum ObjectState {
    Absent,
    Present {
        kind: ObjectKind,
        blake3: ContentDigest,
        byte_len: u64,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum EntryState {
    Absent,
    Present {
        kind: ObjectKind,
        device: u64,
        inode: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        empty: Option<bool>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum DirectoryTreeState {
    Absent,
    Present {
        device: u64,
        inode: u64,
        snapshot: ContentDigest,
        entry_count: u64,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "resource_kind", rename_all = "snake_case")]
pub enum ResourceObservation {
    Object {
        resource: ResourceKey,
        observed: ObjectState,
        generation: u64,
    },
    Entry {
        resource: ResourceKey,
        observed: EntryState,
        generation: u64,
    },
    DirectoryTree {
        resource: ResourceKey,
        observed: DirectoryTreeState,
        generation: u64,
    },
}

impl ResourceObservation {
    pub fn resource(&self) -> &ResourceKey {
        match self {
            Self::Object { resource, .. }
            | Self::Entry { resource, .. }
            | Self::DirectoryTree { resource, .. } => resource,
        }
    }

    pub fn generation(&self) -> u64 {
        match self {
            Self::Object { generation, .. }
            | Self::Entry { generation, .. }
            | Self::DirectoryTree { generation, .. } => *generation,
        }
    }

    pub fn has_matching_resource_kind(&self) -> bool {
        matches!(
            self,
            Self::Object { resource, .. } if resource.kind == ResourceKind::Object
        ) || matches!(
            self,
            Self::Entry { resource, .. } if resource.kind == ResourceKind::Entry
        ) || matches!(
            self,
            Self::DirectoryTree { resource, .. }
                if resource.kind == ResourceKind::DirectoryTree
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LeaseMode {
    ReadIntent,
    ExclusiveWrite,
    ExclusiveDirectory,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LeaseState {
    Queued,
    Offered,
    Active,
    Draining,
    Released,
    Expired,
    Cancelled,
    Superseded,
}

impl LeaseState {
    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Released | Self::Expired | Self::Cancelled | Self::Superseded
        )
    }

    pub fn can_transition_to(self, next: Self) -> bool {
        match self {
            Self::Queued => matches!(
                next,
                Self::Offered | Self::Cancelled | Self::Expired | Self::Superseded
            ),
            Self::Offered => matches!(
                next,
                Self::Active | Self::Queued | Self::Cancelled | Self::Expired | Self::Superseded
            ),
            Self::Active => matches!(
                next,
                Self::Draining | Self::Released | Self::Expired | Self::Cancelled
            ),
            Self::Draining => matches!(next, Self::Released | Self::Expired),
            Self::Released | Self::Expired | Self::Cancelled | Self::Superseded => false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LeaseBatch {
    pub batch_id: String,
    pub task_id: String,
    pub mode: LeaseMode,
    pub state: LeaseState,
    pub version: u64,
    pub queue_sequence: u64,
    pub resources: Vec<ResourceKey>,
    pub in_flight_writes: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub offered_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub offer_expires_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub activated_at: Option<String>,
    pub expires_at: String,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReadAttemptStatus {
    Started,
    Completed,
    Failed,
    Expired,
}

impl ReadAttemptStatus {
    pub fn can_transition_to(self, next: Self) -> bool {
        self == Self::Started && matches!(next, Self::Completed | Self::Failed | Self::Expired)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReadAttempt {
    pub read_id: String,
    pub task_id: String,
    pub invocation_id: String,
    pub resources: Vec<ResourceObservation>,
    pub status: ReadAttemptStatus,
    pub started_at: String,
    pub expires_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completed_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReadEvidence {
    pub evidence_id: String,
    pub read_id: String,
    pub task_id: String,
    pub resources: Vec<ResourceObservation>,
    pub complete: bool,
    pub stable: bool,
    pub exact: bool,
    pub valid: bool,
    pub recorded_at: String,
    pub expires_at: String,
}

impl ReadEvidence {
    pub fn authorizes_write(&self) -> bool {
        self.complete && self.stable && self.exact && self.valid
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum MutationOperation {
    Create {
        path: String,
    },
    Update {
        path: String,
    },
    Delete {
        path: String,
        entry_only: bool,
    },
    Rename {
        old_path: String,
        new_path: String,
        entry_only: bool,
    },
    Move {
        old_path: String,
        new_path: String,
        entry_only: bool,
    },
    Hardlink {
        old_path: String,
        new_path: String,
    },
    Mkdir {
        path: String,
    },
    Rmdir {
        path: String,
    },
    WriteDirectory {
        path: String,
    },
    StructuredCommit {
        tracked_paths: Vec<String>,
    },
}

impl MutationOperation {
    pub fn kind_name(&self) -> &'static str {
        match self {
            Self::Create { .. } => "create",
            Self::Update { .. } => "update",
            Self::Delete { .. } => "delete",
            Self::Rename { .. } => "rename",
            Self::Move { .. } => "move",
            Self::Hardlink { .. } => "hardlink",
            Self::Mkdir { .. } => "mkdir",
            Self::Rmdir { .. } => "rmdir",
            Self::WriteDirectory { .. } => "write_directory",
            Self::StructuredCommit { .. } => "structured_commit",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WriteAttemptStatus {
    Prepared,
    InFlight,
    Succeeded,
    FailedClean,
    FailedUnknown,
    Partial,
}

impl WriteAttemptStatus {
    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Succeeded | Self::FailedClean | Self::FailedUnknown | Self::Partial
        )
    }

    pub fn can_transition_to(self, next: Self) -> bool {
        match self {
            Self::Prepared => matches!(next, Self::InFlight | Self::FailedClean),
            Self::InFlight => next.is_terminal(),
            Self::Succeeded | Self::FailedClean | Self::FailedUnknown | Self::Partial => false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WriteAttempt {
    pub attempt_id: String,
    pub command_id: String,
    pub task_id: String,
    pub lease_batch_id: String,
    pub invocation_id: String,
    pub operation: MutationOperation,
    pub resources: Vec<ResourceObservation>,
    pub status: WriteAttemptStatus,
    pub started_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completed_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommandReceipt {
    pub command_id: String,
    pub command_type: String,
    pub payload_digest: ContentDigest,
    pub response_json: String,
    pub recorded_at: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lease_and_attempt_terminals_cannot_be_reopened() {
        assert!(LeaseState::Queued.can_transition_to(LeaseState::Offered));
        assert!(LeaseState::Queued.can_transition_to(LeaseState::Superseded));
        assert!(LeaseState::Offered.can_transition_to(LeaseState::Active));
        assert!(LeaseState::Active.can_transition_to(LeaseState::Draining));
        assert!(LeaseState::Draining.can_transition_to(LeaseState::Released));
        assert!(!LeaseState::Released.can_transition_to(LeaseState::Active));

        assert!(ReadAttemptStatus::Started.can_transition_to(ReadAttemptStatus::Completed));
        assert!(!ReadAttemptStatus::Completed.can_transition_to(ReadAttemptStatus::Started));
        assert!(WriteAttemptStatus::Prepared.can_transition_to(WriteAttemptStatus::InFlight));
        assert!(WriteAttemptStatus::InFlight.can_transition_to(WriteAttemptStatus::Partial));
        assert!(!WriteAttemptStatus::Partial.can_transition_to(WriteAttemptStatus::InFlight));
    }

    #[test]
    fn mutation_operations_have_tagged_exact_target_payloads() {
        let rename = MutationOperation::Rename {
            old_path: "old.txt".to_string(),
            new_path: "new.txt".to_string(),
            entry_only: false,
        };
        assert_eq!(rename.kind_name(), "rename");
        assert_eq!(
            serde_json::to_value(rename).expect("operation should serialize"),
            serde_json::json!({
                "kind": "rename",
                "old_path": "old.txt",
                "new_path": "new.txt",
                "entry_only": false,
            })
        );
        assert_eq!(
            MutationOperation::StructuredCommit {
                tracked_paths: vec!["tracked.txt".to_string()],
            }
            .kind_name(),
            "structured_commit"
        );
    }

    #[test]
    fn coordination_settings_enforce_lease_timing_order() {
        assert_eq!(
            CoordinationSettings::default(),
            CoordinationSettings {
                heartbeat_interval_seconds: 1,
                inactivity_timeout_seconds: 5,
                lease_expiry_seconds: 60,
                offer_ttl_seconds: 120,
            }
        );
        assert!(CoordinationSettings::default().validate().is_ok());
        assert!(
            CoordinationSettings {
                heartbeat_interval_seconds: 0,
                ..CoordinationSettings::default()
            }
            .validate()
            .is_err()
        );
        assert!(
            CoordinationSettings {
                heartbeat_interval_seconds: 2,
                inactivity_timeout_seconds: 5,
                ..CoordinationSettings::default()
            }
            .validate()
            .is_err()
        );
        assert!(
            CoordinationSettings {
                inactivity_timeout_seconds: 6,
                lease_expiry_seconds: 2,
                ..CoordinationSettings::default()
            }
            .validate()
            .is_err()
        );
    }
}
