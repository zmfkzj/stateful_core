use axum::{Json, http::StatusCode};
use serde::Deserialize;
use serde_json::{Value, json};

const SUPPORTED_PROTOCOL_VERSION: &str = "stateful.v1";

#[derive(Debug, Deserialize)]
pub struct ProtocolRequest<T> {
    #[serde(default)]
    protocol_version: Option<String>,
    #[serde(default)]
    request_id: Option<String>,
    #[serde(default)]
    session: Option<RawSessionMetadata>,
    #[serde(default)]
    workspace: Option<RawWorkspaceMetadata>,
    #[serde(default)]
    source: Option<RawSourceMetadata>,
    #[serde(flatten)]
    payload: T,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionMetadata {
    pub session_id: String,
    pub turn_id: Option<String>,
    pub actor_id: String,
    pub actor_type: String,
    pub owner_id: Option<String>,
    pub parent_session_id: Option<String>,
    pub parent_actor_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceMetadata {
    pub workspace_id: String,
    pub root: String,
    pub repo_id: String,
    pub worktree_id: String,
    pub branch: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceMetadata {
    pub kind: String,
    pub event: String,
    pub tool_name: Option<String>,
    pub source_ref: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidatedRequest<T> {
    pub protocol_version: String,
    pub request_id: String,
    pub session: SessionMetadata,
    pub workspace: WorkspaceMetadata,
    pub source: SourceMetadata,
    pub payload: T,
}

pub fn validate_protocol<T>(
    request: ProtocolRequest<T>,
) -> Result<ValidatedRequest<T>, (StatusCode, Json<Value>)> {
    let protocol_version = match required(request.protocol_version, "protocol_version") {
        Ok(protocol_version) => protocol_version,
        Err(message) => return Err(protocol_error(message)),
    };
    if protocol_version != SUPPORTED_PROTOCOL_VERSION {
        return Err(protocol_error(format!(
            "Unsupported protocol_version: {protocol_version}"
        )));
    }

    let request_id = match required(request.request_id, "request_id") {
        Ok(request_id) => request_id,
        Err(message) => return Err(protocol_error(message)),
    };

    let session = match request.session {
        Some(session) => validate_session(session),
        None => Err("Missing protocol field: session".to_string()),
    }
    .map_err(protocol_error)?;

    let workspace = match request.workspace {
        Some(workspace) => validate_workspace(workspace),
        None => Err("Missing protocol field: workspace".to_string()),
    }
    .map_err(protocol_error)?;

    let source = match request.source {
        Some(source) => validate_source(source),
        None => Err("Missing protocol field: source".to_string()),
    }
    .map_err(protocol_error)?;

    Ok(ValidatedRequest {
        protocol_version,
        request_id,
        session,
        workspace,
        source,
        payload: request.payload,
    })
}

pub fn protocol_error(message: impl Into<String>) -> (StatusCode, Json<Value>) {
    (
        StatusCode::BAD_REQUEST,
        Json(json!({
            "decision": "error",
            "reason_code": "protocol_mismatch",
            "message": message.into()
        })),
    )
}

#[derive(Debug, Deserialize)]
struct RawSessionMetadata {
    #[serde(default)]
    session_id: Option<String>,
    #[serde(default)]
    turn_id: Option<String>,
    #[serde(default)]
    actor_id: Option<String>,
    #[serde(default)]
    actor_type: Option<String>,
    #[serde(default)]
    owner_id: Option<String>,
    #[serde(default)]
    parent_session_id: Option<String>,
    #[serde(default)]
    parent_actor_id: Option<String>,
}

#[derive(Debug, Deserialize)]
struct RawWorkspaceMetadata {
    #[serde(default)]
    workspace_id: Option<String>,
    #[serde(default)]
    root: Option<String>,
    #[serde(default)]
    repo_id: Option<String>,
    #[serde(default)]
    worktree_id: Option<String>,
    #[serde(default)]
    branch: Option<String>,
}

#[derive(Debug, Deserialize)]
struct RawSourceMetadata {
    #[serde(default)]
    kind: Option<String>,
    #[serde(default)]
    event: Option<String>,
    #[serde(default)]
    tool_name: Option<String>,
    #[serde(default)]
    source_ref: Option<String>,
}

fn validate_session(session: RawSessionMetadata) -> Result<SessionMetadata, String> {
    Ok(SessionMetadata {
        session_id: required(session.session_id, "session.session_id")?,
        turn_id: session.turn_id,
        actor_id: required(session.actor_id, "session.actor_id")?,
        actor_type: required(session.actor_type, "session.actor_type")?,
        owner_id: session.owner_id,
        parent_session_id: session.parent_session_id,
        parent_actor_id: session.parent_actor_id,
    })
}

fn validate_workspace(workspace: RawWorkspaceMetadata) -> Result<WorkspaceMetadata, String> {
    Ok(WorkspaceMetadata {
        workspace_id: required(workspace.workspace_id, "workspace.workspace_id")?,
        root: required(workspace.root, "workspace.root")?,
        repo_id: required(workspace.repo_id, "workspace.repo_id")?,
        worktree_id: required(workspace.worktree_id, "workspace.worktree_id")?,
        branch: required(workspace.branch, "workspace.branch")?,
    })
}

fn validate_source(source: RawSourceMetadata) -> Result<SourceMetadata, String> {
    Ok(SourceMetadata {
        kind: required(source.kind, "source.kind")?,
        event: required(source.event, "source.event")?,
        tool_name: source.tool_name,
        source_ref: required(source.source_ref, "source.source_ref")?,
    })
}

fn required(value: Option<String>, field: &str) -> Result<String, String> {
    match value {
        Some(value) if !value.trim().is_empty() => Ok(value),
        _ => Err(format!("Missing protocol field: {field}")),
    }
}
