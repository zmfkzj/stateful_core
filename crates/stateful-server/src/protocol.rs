use axum::{Json, http::StatusCode, response::{IntoResponse, Response}};
use serde::{Serialize, de::DeserializeOwned};
use serde_json::{Value, json};
use stateful_core::{RequestEnvelope, V2Error};
use stateful_store::{CommandOutcome, StoreError};

pub(crate) fn parse_request<T: DeserializeOwned>(body: Value) -> Result<RequestEnvelope<T>, Response> {
    let request_id = body.get("request_id").and_then(Value::as_str).map(str::to_owned);
    let raw = serde_json::to_string(&body).map_err(|error| {
        error_response(
            StatusCode::BAD_REQUEST,
            request_id.as_deref(),
            V2Error::new("invalid_request_envelope", error.to_string()),
        )
    })?;
    RequestEnvelope::from_json(raw).map_err(|error| error_response(StatusCode::BAD_REQUEST, request_id.as_deref(), error))
}

pub(crate) fn command_response<T: Serialize>(
    request_id: &str,
    result: Result<CommandOutcome<T>, StoreError>,
) -> Response {
    match result {
        Ok(outcome) => (status_code(outcome.http_status), Json(outcome.response)).into_response(),
        Err(error) => store_error_response(request_id, error),
    }
}

pub(crate) fn store_error_response(request_id: &str, error: StoreError) -> Response {
    let (status, error) = match error {
        StoreError::V2(error) => (StatusCode::BAD_REQUEST, error),
        StoreError::IdempotencyKeyReused => (
            StatusCode::CONFLICT,
            V2Error::new("idempotency_key_reused", "request_id was already used for a different command."),
        ),
        StoreError::ClaimConflict | StoreError::WriteFenceConflict { .. } => (
            StatusCode::CONFLICT,
            V2Error::new("coordination_conflict", error.to_string()),
        ),
        StoreError::MissingReservation => (
            StatusCode::CONFLICT,
            V2Error::new("missing_reservation", error.to_string()),
        ),
        error => (StatusCode::BAD_REQUEST, V2Error::new(error.code(), error.to_string())),
    };
    error_response(status, Some(request_id), error)
}

pub(crate) fn error_response(
    status: StatusCode,
    request_id: Option<&str>,
    error: V2Error,
) -> Response {
    let mut body = json!({
        "protocol_version": "stateful.v2",
        "error": error,
    });
    if let Some(request_id) = request_id {
        body["request_id"] = json!(request_id);
    }
    (status, Json(body)).into_response()
}

pub(crate) fn status_code(status: u16) -> StatusCode {
    StatusCode::from_u16(status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR)
}
