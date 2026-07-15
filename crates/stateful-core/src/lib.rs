mod context;
mod fingerprint;
mod journal;
mod path;
mod policy;
mod presence;
mod protocol;
mod reconcile;
mod types;

pub use context::{
    AGENT_CONTEXT_SCOPE_SOURCE_REF, BRIEF_CONTEXT_MAX_ITEMS, BRIEF_CONTEXT_MAX_SCALARS,
    ContextPackage, CurrentEvidenceKind, CurrentFreshness, CurrentItem, CurrentItemKind,
    CurrentSeverity, RenderMode, render_prompt_text,
};
pub use path::{
    normalize_directory_path, normalize_relative_path, normalized_relative_path_is_empty,
};
pub use fingerprint::{ContentFingerprint, fingerprint_path, fingerprint_reader};
pub use journal::{
    AuthorizationEvent, ClaimEvent, ContextEvent, EventData, EventPayload, HandoffEvent,
    HumanObservationEvent, LEGACY_MIGRATION_NAMESPACE, MigrationEvent, NewEvent,
    NotificationEvent, PresenceEvent, ReadObservationEvent, RecoveryEvent, ReservationEvent,
    StoredEvent, WaitEvent, WriteFenceEvent, WriteIntentEvent, migration_seed_event_id,
};
pub use policy::{
    AuthorizationInput, PolicyState, PresencePhase, ReservationScope, ScopeSet, authorize_action,
};
pub use presence::{
    BUSY_UNTIL_MAXIMUM, EXPLICIT_HANDOFF_RELEVANCE, FALLBACK_HANDOFF_RELEVANCE,
    HANDOFF_LIST_MAX_ENTRIES, HANDOFF_SUMMARY_MAX_SCALARS, LAST_RESULT_MAX_SCALARS,
    PRESENCE_GOAL_EXCERPT_MAX_SCALARS, PRESENCE_TTL, READ_OBSERVATION_TTL, ExplicitHandoff,
    HandoffStatus, PresenceRecord, PresenceResource, PresenceResourceRelation, PresenceUpdate,
};
pub use reconcile::ReconciliationDecision;
pub use protocol::{QueryEnvelope, RequestEnvelope, V2Error, V2ErrorEnvelope};
pub use types::{
    ActorType, AgentIdentity, Decision, DecisionKind, ProtocolVersion, SourceKind, SourceRef,
    WorkspaceIdentity,
};

pub const CRATE_NAME: &str = "stateful-core";
