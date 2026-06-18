mod context;
mod path;
mod policy;
mod reconcile;
mod types;

pub use context::{
    ContextPackage, CurrentEvidenceKind, CurrentFreshness, CurrentItem, CurrentItemKind,
    CurrentSeverity, RenderMode, render_prompt_text,
};
pub use path::{
    normalize_directory_path, normalize_relative_path, normalized_relative_path_is_empty,
};
pub use policy::{AuthorizationInput, IntentScope, PolicyState, ScopeSet, authorize_action};
pub use reconcile::ReconciliationDecision;
pub use types::{
    ActorType, Decision, DecisionKind, ProtocolVersion, RequestEnvelope, SessionIdentity,
    SourceKind, SourceRef, WorkspaceIdentity,
};

pub const CRATE_NAME: &str = "stateful-core";
