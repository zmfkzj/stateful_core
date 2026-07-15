use axum::{Json, http::StatusCode};
use serde::Deserialize;
use serde_json::{Value, json};
use stateful_core::{AgentIdentity, SourceRef, WorkspaceIdentity};

#[derive(Debug, Deserialize)]
enum LegacyProtocolVersion {
    #[serde(rename = "stateful.v1")]
    V1,
}

#[derive(Debug, Deserialize)]
pub(crate) struct LegacyRequestEnvelope {
    #[serde(rename = "protocol_version")]
    _protocol_version: LegacyProtocolVersion,
    #[serde(rename = "request_id")]
    _request_id: String,
    pub(crate) observed_at: String,
    pub(crate) agent: AgentIdentity,
    pub(crate) workspace: WorkspaceIdentity,
    pub(crate) source: SourceRef,
}

#[derive(Debug, Deserialize)]
pub(crate) struct ProtocolEnvelope {
    #[serde(flatten)]
    pub(crate) request: LegacyRequestEnvelope,
    pub(crate) payload: Value,
}

pub(crate) fn require_v1_envelope(body: Value) -> Result<ProtocolEnvelope, ProtocolError> {
    serde_json::from_value(body).map_err(|_| ProtocolError)
}

#[derive(Debug)]
pub(crate) struct ProtocolError;

impl ProtocolError {
    pub(crate) fn response(self) -> (StatusCode, Json<Value>) {
        protocol_mismatch_response()
    }
}

pub(crate) fn protocol_mismatch_response() -> (StatusCode, Json<Value>) {
    (
        StatusCode::BAD_REQUEST,
        Json(json!({
            "decision": "error",
            "reason_code": "protocol_mismatch"
        })),
    )
}
