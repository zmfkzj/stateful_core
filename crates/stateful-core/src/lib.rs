mod bash;
mod context;
mod policy;
mod reconcile;
mod types;

pub use bash::{BashClassification, BashKind, classify_bash};
pub use context::{ContextPackage, RenderMode, render_prompt_text};
pub use policy::{
    AuthorizationInput, IntentPhase, IntentScope, PolicyState, ScopeSet, authorize_action,
};
pub use reconcile::ReconciliationDecision;
pub use types::{
    ActionKind, ActorType, Decision, DecisionKind, ProtocolVersion, RequestEnvelope, ResourceType,
    SessionIdentity, SourceKind, SourceRef, Target, TargetOperation, WorkspaceIdentity,
};

pub const CRATE_NAME: &str = "stateful-core";
