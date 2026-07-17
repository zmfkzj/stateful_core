mod context;
mod fingerprint;
mod freshness;
mod journal;
mod path;
mod policy;
mod presence;
mod protocol;
mod reconcile;
mod types;

pub use context::{
    AGENT_CONTEXT_SCOPE_SOURCE_REF, BRIEF_CONTEXT_MAX_ITEMS, BRIEF_CONTEXT_MAX_SCALARS,
    ContextDelta, ContextPackage, CurrentEvidenceKind, CurrentFreshness, CurrentItem,
    CurrentItemKind, CurrentSeverity, RenderMode, render_prompt_text,
};
pub use fingerprint::{ContentFingerprint, fingerprint_path, fingerprint_reader};
pub use freshness::{
    FreshnessMode, OBSERVATION_TTL, ObservationFreshness, ReadClassification, ReadCompletion,
    ReadObservationRecord, ReadObservationStart, ReadObservationStatus, ResourceVersion,
    ThinSafetyState, WriteIntentCompletion, WriteIntentOutcome, WriteIntentRecord,
    WriteIntentStart, WriteIntentStatus, WriteTarget, evaluate_thin_safety, observation_status,
};
pub use journal::{
    AuthorizationEvent, ClaimEvent, ContextEvent, EventData, EventPayload, HandoffEvent,
    HumanAcknowledgementEvent, HumanObservationEvent, LEGACY_MIGRATION_NAMESPACE, MigrationEvent,
    NewEvent, NotificationEvent, PresenceEvent, ReadObservationEvent, RecoveryEvent,
    ReservationEvent, StoredEvent, WaitEvent, WriteFenceEvent, WriteIntentEvent,
    migration_seed_event_id,
};
pub use path::{
    normalize_directory_path, normalize_relative_path, normalized_relative_path_is_empty,
};
pub use policy::{
    AuthorizationInput, PolicyState, PresencePhase, ReservationScope, ScopeSet, authorize_action,
};
pub use presence::{
    BUSY_UNTIL_MAXIMUM, EXPLICIT_HANDOFF_RELEVANCE, ExplicitHandoff, FALLBACK_HANDOFF_RELEVANCE,
    HANDOFF_LIST_MAX_ENTRIES, HANDOFF_SUMMARY_MAX_SCALARS, HandoffRecord, HandoffStatus,
    LAST_RESULT_MAX_SCALARS, PRESENCE_GOAL_EXCERPT_MAX_SCALARS, PRESENCE_TTL, PresenceRecord,
    PresenceResource, PresenceResourceRelation, PresenceUpdate, READ_OBSERVATION_TTL,
};
pub use protocol::{QueryEnvelope, RequestEnvelope, V2Error, V2ErrorEnvelope};
pub use reconcile::ReconciliationDecision;
pub use types::{
    ActorType, AgentIdentity, Decision, DecisionKind, ProtocolVersion, SourceKind, SourceRef,
    WorkspaceIdentity,
};

pub const CRATE_NAME: &str = "stateful-core";
