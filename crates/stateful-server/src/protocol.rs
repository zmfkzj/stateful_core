use axum::{Json, http::StatusCode};
use serde::Deserialize;
use serde_json::{Value, json};
use stateful_core::{ProtocolVersion, RequestEnvelope};

#[derive(Debug, Deserialize)]
pub struct ProtocolEnvelope {
    #[serde(flatten)]
    pub request: RequestEnvelope,
    pub payload: Value,
}

pub fn require_v1_envelope(body: Value) -> Result<ProtocolEnvelope, ProtocolError> {
    let envelope: ProtocolEnvelope = serde_json::from_value(body).map_err(|_| ProtocolError)?;
    match envelope.request.protocol_version {
        ProtocolVersion::V1 => Ok(envelope),
    }
}

#[derive(Debug)]
pub struct ProtocolError;

impl ProtocolError {
    pub fn response(self) -> (StatusCode, Json<Value>) {
        protocol_mismatch_response()
    }
}

pub fn protocol_mismatch_response() -> (StatusCode, Json<Value>) {
    (
        StatusCode::BAD_REQUEST,
        Json(json!({
            "decision": "error",
            "reason_code": "protocol_mismatch"
        })),
    )
}
