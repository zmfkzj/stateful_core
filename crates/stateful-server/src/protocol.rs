use axum::{
    Json,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde::{Deserialize, de::DeserializeOwned};
use serde_json::{Value, json};
use stateful_core::RequestEnvelope;

#[derive(Debug, Deserialize)]
struct ProtocolEnvelope {
    #[serde(flatten)]
    request: RequestEnvelope,
    payload: Value,
}

pub fn parse_command<P>(body: Value) -> Result<(RequestEnvelope, P), ProtocolError>
where
    P: DeserializeOwned,
{
    let envelope: ProtocolEnvelope =
        serde_json::from_value(body).map_err(|_| ProtocolError::Mismatch)?;
    let payload =
        serde_json::from_value(envelope.payload).map_err(|_| ProtocolError::InvalidPayload)?;
    Ok((envelope.request, payload))
}

#[derive(Debug)]
pub enum ProtocolError {
    Mismatch,
    InvalidPayload,
}

impl ProtocolError {
    pub fn response(self) -> Response {
        let (reason_code, message) = match self {
            Self::Mismatch => ("protocol_mismatch", "stateful.v2 lease-1 envelope required"),
            Self::InvalidPayload => ("invalid_payload", "payload does not match the command"),
        };
        (
            StatusCode::BAD_REQUEST,
            Json(json!({
                "protocol_version": "stateful.v2",
                "contract_revision": "lease-1",
                "decision": "error",
                "reason_code": reason_code,
                "message": message,
            })),
        )
            .into_response()
    }
}
