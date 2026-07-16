use axum::{
    Json,
    body::to_bytes,
    extract::{FromRequest, Request},
    http::{StatusCode, header::CONTENT_TYPE},
    response::{IntoResponse, Response},
};
use serde::{Serialize, de::DeserializeOwned};
use serde_json::{Value, json};
use stateful_core::{QueryEnvelope, RequestEnvelope, V2Error};
use stateful_store::{CommandOutcome, StoreError};


pub(crate) struct V2Json(pub Value);

impl<S> FromRequest<S> for V2Json
where
    S: Send + Sync,
{
    type Rejection = Response;

    async fn from_request(request: Request, _state: &S) -> Result<Self, Self::Rejection> {
        let is_json = request
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| value.split(';').next().is_some_and(|kind| kind.trim() == "application/json"));
        if !is_json {
            return Err(error_response(
                StatusCode::UNSUPPORTED_MEDIA_TYPE,
                None,
                V2Error::new("unsupported_media_type", "POST requests require application/json."),
            ));
        }
        let bytes = to_bytes(request.into_body(), 2 * 1024 * 1024).await.map_err(|_| {
            error_response(
                StatusCode::BAD_REQUEST,
                None,
                V2Error::new("invalid_json", "Request body could not be read."),
            )
        })?;
        serde_json::from_slice(&bytes)
            .map(Self)
            .map_err(|_| error_response(
                StatusCode::BAD_REQUEST,
                None,
                V2Error::new("invalid_json", "Request body must be valid JSON."),
            ))
    }
}

pub(crate) fn parse_query<T: DeserializeOwned>(raw: Option<String>) -> Result<QueryEnvelope<T>, Response> {
    let mut parameters = serde_json::Map::new();
    for pair in raw.as_deref().unwrap_or_default().split('&').filter(|pair| !pair.is_empty()) {
        let Some((key, value)) = pair.split_once('=') else {
            return Err(error_response(
                StatusCode::BAD_REQUEST,
                None,
                V2Error::new("invalid_query_envelope", "Query parameters must be key-value pairs."),
            ));
        };
        let (key, value) = match (decode_query_component(key), decode_query_component(value)) {
            (Ok(key), Ok(value)) => (key, value),
            _ => return Err(error_response(
                StatusCode::BAD_REQUEST,
                None,
                V2Error::new("invalid_query_envelope", "Query parameters contain invalid percent encoding."),
            )),
        };
        let value = if key == "limit" {
            value.parse::<u64>()
                .map(serde_json::Number::from)
                .map(Value::Number)
                .unwrap_or_else(|_| Value::String(value))
        } else {
            Value::String(value)
        };
        if parameters.insert(key, value).is_some() {
            return Err(error_response(
                StatusCode::BAD_REQUEST,
                None,
                V2Error::new("invalid_query_envelope", "Query parameters must not repeat a field."),
            ));
        }
    }
    let request_id = parameters.get("request_id").and_then(Value::as_str).map(str::to_owned);
    if parameters.get("protocol_version").and_then(Value::as_str) == Some("stateful.v1") {
        return Err(error_response(
            StatusCode::BAD_REQUEST,
            request_id.as_deref(),
            V2Error::new("unsupported_protocol", "Only protocol_version stateful.v2 is supported."),
        ));
    }
    let request = serde_json::from_value::<QueryEnvelope<T>>(Value::Object(parameters)).map_err(|_| {
        error_response(
            StatusCode::BAD_REQUEST,
            request_id.as_deref(),
            V2Error::new("invalid_query_envelope", "Query parameters must form a valid V2 query envelope."),
        )
    })?;
    request.validate().map_err(|error| {
        error_response(StatusCode::BAD_REQUEST, Some(&request.request_id.to_string()), error)
    })?;
    Ok(request)
}

fn decode_query_component(value: &str) -> Result<String, ()> {
    let bytes = value.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'+' => decoded.push(b' '),
            b'%' if index + 2 < bytes.len() => {
                let hex = |byte| match byte {
                    b'0'..=b'9' => Ok(byte - b'0'),
                    b'a'..=b'f' => Ok(byte - b'a' + 10),
                    b'A'..=b'F' => Ok(byte - b'A' + 10),
                    _ => Err(()),
                };
                decoded.push(hex(bytes[index + 1])? * 16 + hex(bytes[index + 2])?);
                index += 2;
            }
            b'%' => return Err(()),
            byte => decoded.push(byte),
        }
        index += 1;
    }
    String::from_utf8(decoded).map_err(|_| ())
}
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
        StoreError::ClaimConflict | StoreError::ClaimAlreadyHeld | StoreError::WriteFenceConflict { .. } => (
            StatusCode::CONFLICT,
            V2Error::new("coordination_conflict", "The requested coordination state conflicts with an active record."),
        ),
        StoreError::StaleAuthorization => (
            StatusCode::CONFLICT,
            V2Error::new(
                "stale_authorization",
                "Coordination state changed while the write authorization was being recorded.",
            )
            .with_required_next_action("Re-read coordination state and retry authorization."),
        ),
        StoreError::MissingReservation => (
            StatusCode::CONFLICT,
            V2Error::new("missing_reservation", "A matching active reservation is required."),
        ),
        StoreError::ReservationOwnerMismatch
        | StoreError::ReservationRequestOwnerMismatch
        | StoreError::ClaimOwnerMismatch
        | StoreError::WriteIntentOwnerMismatch => (
            StatusCode::FORBIDDEN,
            V2Error::new("owner_mismatch", "The request identity does not own this coordination record."),
        ),
        StoreError::ReservationRequestNotFound
        | StoreError::ClaimNotFound
        | StoreError::ReadOperationNotFound
        | StoreError::WriteIntentNotFound => (
            StatusCode::NOT_FOUND,
            V2Error::new("not_found", "The requested coordination record does not exist."),
        ),
        StoreError::ReservationRequestNotCancelable
        | StoreError::InvalidClaimPath(_)
        | StoreError::MissingPurpose
        | StoreError::MissingScope
        | StoreError::InvalidTimestamp(_)
        | StoreError::InvalidReadOperation
        | StoreError::InvalidWriteIntent => (
            StatusCode::BAD_REQUEST,
            V2Error::new("invalid_request", "The request violates a command validation rule."),
        ),
        StoreError::Io(_)
        | StoreError::Sqlite(_)
        | StoreError::Json(_)
        | StoreError::InvalidCommandEvent
        | StoreError::InvalidJournalEvent
        | StoreError::MigrationValidation(_)
        | StoreError::ProjectorFailure
        | StoreError::ReplayMismatch => (
            StatusCode::INTERNAL_SERVER_ERROR,
            V2Error::new("internal_error", "The server could not complete the request."),
        ),
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
