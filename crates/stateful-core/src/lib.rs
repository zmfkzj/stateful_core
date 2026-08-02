mod coordination;
mod path;
mod resource;
mod types;

pub use coordination::{
    CommandReceipt, ContentDigest, CoordinationSettings, DigestAlgorithm, DirectoryTreeState,
    EntryState, LeaseBatch, LeaseMode, LeaseState, MutationOperation, ObjectKind, ObjectState,
    ReadAttempt, ReadAttemptStatus, ReadEvidence, ResourceKey, ResourceKind, ResourceObservation,
    TaskRecord, TaskStatus, WriteAttempt, WriteAttemptStatus,
};
pub use path::{
    normalize_directory_path, normalize_relative_path, normalized_relative_path_is_empty,
};
pub use resource::{
    ResourceError, ResourceResolver, ResourceSet, digest_bytes, digest_canonical_json,
    digest_reader, resource_keys_overlap, validate_operation_start, validate_operation_transition,
};
pub use types::{
    ActorType, AgentIdentity, ContractRevision, DecisionKind, ProtocolVersion, RequestEnvelope,
    SourceKind, SourceRef, WorkspaceIdentity,
};

pub const CRATE_NAME: &str = "stateful-core";
