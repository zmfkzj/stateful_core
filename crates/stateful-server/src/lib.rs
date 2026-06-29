mod policy_service;
mod protocol;

use axum::{
    Json, Router,
    extract::{Query, State},
    http::{HeaderMap, StatusCode},
    response::{
        IntoResponse, Response,
        sse::{Event as SseEvent, KeepAlive, Sse},
    },
    routing::{get, post},
};
use policy_service::{
    AuthorizationOutcome, AuthorizeWriteInput, BaseObservation, CancelReservationInput,
    CancelReservationOutcome, ClaimReservationInput, ClaimReservationOutcome, PolicyService,
    RequestReservationInput, RequestReservationOutcome, WaitQueueInfo, claim_observation_for_path,
};
use serde::Deserialize;
use serde_json::{Value, json};
use stateful_core::{
    ActivityPhase, ContextPackage, ReconciliationDecision, RenderMode,
    normalized_relative_path_is_empty, render_prompt_text,
};
use stateful_store::{
    ClaimBatchAcquireResult, CurrentStateIdentityFilter, Event, NotificationRecord, OutboxEntry,
    Store, StoreError, WaitRecord,
};
use std::{
    collections::VecDeque,
    convert::Infallible,
    net::SocketAddr,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio_stream::{Stream, StreamExt, wrappers::IntervalStream};

pub const CRATE_NAME: &str = "stateful-server";
const RUNTIME_CAPABILITIES: &[&str] = &["authorize.write_directory"];
const DEFAULT_MAINTENANCE_INTERVAL: Duration = Duration::from_secs(30);

#[derive(Debug, Clone)]
pub struct ServerConfig {
    bearer_token: String,
    store: SharedStore,
    maintenance_interval: Duration,
}

impl ServerConfig {
    pub fn new(bearer_token: impl Into<String>) -> Self {
        Self::with_store(
            bearer_token,
            Store::open_in_memory().expect("server in-memory store should open"),
        )
    }

    pub fn with_store(bearer_token: impl Into<String>, store: Store) -> Self {
        Self {
            bearer_token: bearer_token.into(),
            store: Arc::new(Mutex::new(store)),
            maintenance_interval: DEFAULT_MAINTENANCE_INTERVAL,
        }
    }

    pub fn with_maintenance_interval(mut self, interval: Duration) -> Self {
        self.maintenance_interval = interval;
        self
    }
}

type SharedStore = Arc<Mutex<Store>>;

pub fn build_router(config: ServerConfig) -> Router {
    Router::new()
        .route("/health", get(health))
        .route("/v1/current", get(current))
        .route("/v1/events", get(events))
        .route("/v1/runtime/identity", get(runtime_identity))
        .route("/v1/session/register", post(session_register))
        .route("/v1/session/heartbeat", post(session_heartbeat))
        .route("/v1/reservation/declare", post(reservation_declare))
        .route("/v1/reservation/request", post(reservation_request))
        .route("/v1/reservation/claim", post(reservation_claim))
        .route("/v1/reservation/cancel", post(reservation_cancel))
        .route("/v1/claim/acquire", post(lease_acquire))
        .route(
            "/v1/claim/refresh-observation",
            post(lease_refresh_observation),
        )
        .route("/v1/claim/release", post(lease_release))
        .route("/v1/activity/observe", post(activity_observe))
        .route("/v1/activity/finalize", post(activity_finalize))
        .route("/v1/authorize", post(authorize))
        .route("/v1/conflicts/check", post(conflicts_check))
        .route("/v1/context/render", post(context_render))
        .route("/v1/reconcile/ack", post(reconcile_ack))
        .route("/v1/notifications/poll", post(notifications_poll))
        .route("/v1/notifications/stream", get(notifications_stream))
        .route("/v1/resume/next", post(resume_next))
        .route("/v1/outbox/sync", post(outbox_sync))
        .with_state(config)
}

pub async fn serve_addr(addr: SocketAddr, config: ServerConfig) -> anyhow::Result<()> {
    let listener = tokio::net::TcpListener::bind(addr).await?;
    serve_listener(listener, config).await
}

pub async fn serve_listener(
    listener: tokio::net::TcpListener,
    config: ServerConfig,
) -> anyhow::Result<()> {
    let maintenance = run_maintenance_loop(config.store.clone(), config.maintenance_interval);
    tokio::select! {
        result = axum::serve(listener, build_router(config)) => {
            result?;
        }
        () = maintenance => {}
    }
    Ok(())
}

async fn run_maintenance_loop(store: SharedStore, interval: Duration) {
    let mut ticker = tokio::time::interval(interval);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        ticker.tick().await;
        let Ok(store) = store.lock() else {
            continue;
        };
        let _ = store.expire_stale();
        let _ = store.prune_retention();
    }
}

async fn health() -> StatusCode {
    StatusCode::OK
}

async fn current(
    State(config): State<ServerConfig>,
    Query(input): Query<CurrentQuery>,
    headers: HeaderMap,
) -> (StatusCode, Json<Value>) {
    if !has_valid_bearer_token(&headers, &config.bearer_token) {
        return unauthorized();
    }

    let result = config
        .store
        .lock()
        .map_err(|_| "store lock poisoned".to_string())
        .and_then(|store| {
            store
                .live_current_state(input.resource.as_deref())
                .map_err(|error| error.to_string())
        });

    match result {
        Ok(live) => (
            StatusCode::OK,
            Json(json!({
                "status": "ok",
                "current": live.summary,
                "items": live.items
            })),
        ),
        Err(message) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({
                "status": "error",
                "message": message
            })),
        ),
    }
}

async fn events(
    State(config): State<ServerConfig>,
    headers: HeaderMap,
) -> (StatusCode, Json<Value>) {
    if !has_valid_bearer_token(&headers, &config.bearer_token) {
        return unauthorized();
    }

    let result = config
        .store
        .lock()
        .map_err(|_| "store lock poisoned".to_string())
        .and_then(|store| store.recent_events(100).map_err(|error| error.to_string()));

    match result {
        Ok(events) => (
            StatusCode::OK,
            Json(json!({
                "status": "ok",
                "events": events
            })),
        ),
        Err(message) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({
                "status": "error",
                "message": message
            })),
        ),
    }
}

async fn runtime_identity(
    State(config): State<ServerConfig>,
    headers: HeaderMap,
) -> (StatusCode, Json<Value>) {
    if !has_valid_bearer_token(&headers, &config.bearer_token) {
        return unauthorized();
    }

    (
        StatusCode::OK,
        Json(json!({
            "status": "ok",
            "pid": std::process::id(),
            "protocol_version": "stateful.v1",
            "capabilities": RUNTIME_CAPABILITIES
        })),
    )
}

async fn session_register(
    State(config): State<ServerConfig>,
    headers: HeaderMap,
    Json(input): Json<SessionRequest>,
) -> (StatusCode, Json<Value>) {
    if !has_valid_bearer_token(&headers, &config.bearer_token) {
        return unauthorized();
    }

    append_event_response(
        &config.store,
        with_request_identity(
            Event::session_registered(input.session_id, input.workspace_id),
            input.identity,
        ),
    )
}

async fn session_heartbeat(
    State(config): State<ServerConfig>,
    headers: HeaderMap,
    Json(input): Json<SessionRequest>,
) -> (StatusCode, Json<Value>) {
    if !has_valid_bearer_token(&headers, &config.bearer_token) {
        return unauthorized();
    }

    append_event_response(
        &config.store,
        with_request_identity(
            Event::session_heartbeat(input.session_id, input.workspace_id),
            input.identity,
        ),
    )
}

async fn authorize(
    State(config): State<ServerConfig>,
    headers: HeaderMap,
    Json(input): Json<Value>,
) -> (StatusCode, Json<serde_json::Value>) {
    if !has_valid_bearer_token(&headers, &config.bearer_token) {
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({
                "error": "unauthorized"
            })),
        );
    }

    let envelope = match protocol::require_v1_envelope(input) {
        Ok(envelope) => envelope,
        Err(error) => return error.response(),
    };
    let payload: AuthorizePayload = match serde_json::from_value(envelope.payload) {
        Ok(payload) => payload,
        Err(_) => return protocol::protocol_mismatch_response(),
    };
    let stateful_core::RequestEnvelope {
        session,
        workspace,
        source,
        ..
    } = envelope.request;
    let queue_purpose = if payload.queue_on_conflict {
        let Some(purpose) = payload.purpose else {
            return missing_purpose_response();
        };
        match require_purpose(purpose) {
            Ok(purpose) => Some(purpose),
            Err(response) => return response,
        }
    } else {
        None
    };

    let input = AuthorizeWriteInput {
        session_id: session.session_id,
        reservation_id: payload.reservation_id,
        workspace_id: Some(workspace.workspace_id),
        repo_id: non_empty_identity(workspace.repo_id),
        worktree_id: non_empty_identity(workspace.worktree_id),
        root: non_empty_identity(workspace.root),
        branch: non_empty_identity(workspace.branch),
        source_kind: Some(source.kind),
        source_event: Some(source.event),
        queue_on_conflict: payload.queue_on_conflict,
        queue_purpose,
        action: payload.action,
        old_path: payload.old_path,
        new_path: payload.new_path,
        path: payload.path,
        base_observations: payload
            .base_observations
            .into_iter()
            .map(BaseObservation::from)
            .collect(),
    };

    let outcome = match authorize_with_policy_and_audit(&config.store, input) {
        Ok(outcome) => outcome,
        Err(message) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({
                    "decision": "error",
                    "reason_code": "state_error",
                    "message": message
                })),
            );
        }
    };

    (StatusCode::OK, Json(authorization_json(outcome)))
}

async fn reservation_declare(
    State(config): State<ServerConfig>,
    headers: HeaderMap,
    Json(input): Json<Value>,
) -> (StatusCode, Json<Value>) {
    if !has_valid_bearer_token(&headers, &config.bearer_token) {
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({
                "error": "unauthorized"
            })),
        );
    }

    let envelope = match protocol::require_v1_envelope(input) {
        Ok(envelope) => envelope,
        Err(error) => return error.response(),
    };
    let payload: ReservationDeclarePayload = match serde_json::from_value(envelope.payload) {
        Ok(payload) => payload,
        Err(_) => return protocol::protocol_mismatch_response(),
    };
    let purpose = match require_purpose(payload.purpose) {
        Ok(purpose) => purpose,
        Err(response) => return response,
    };
    let files_planned = match require_files_planned(payload.files_planned) {
        Ok(files_planned) => files_planned,
        Err(response) => return response,
    };
    let identity = WorkspaceIdentityRequest {
        repo_id: non_empty_identity(envelope.request.workspace.repo_id),
        worktree_id: non_empty_identity(envelope.request.workspace.worktree_id),
        root: non_empty_identity(envelope.request.workspace.root),
        branch: non_empty_identity(envelope.request.workspace.branch),
    };

    let event = with_request_identity(
        Event::reservation_declared(
            envelope.request.session.session_id,
            envelope.request.workspace.workspace_id,
            purpose,
            files_planned,
        ),
        identity,
    );
    let reservation_id = event.event_id.clone();
    match append_event(&config.store, event) {
        Ok(()) => (
            StatusCode::OK,
            Json(json!({
                "status": "ok",
                "reservation_id": reservation_id
            })),
        ),
        Err(message) => status_response(Err(message)),
    }
}

async fn reservation_request(
    State(config): State<ServerConfig>,
    headers: HeaderMap,
    Json(input): Json<Value>,
) -> (StatusCode, Json<Value>) {
    if !has_valid_bearer_token(&headers, &config.bearer_token) {
        return unauthorized();
    }

    let envelope = match protocol::require_v1_envelope(input) {
        Ok(envelope) => envelope,
        Err(error) => return error.response(),
    };
    let payload: ReservationRequestPayload = match serde_json::from_value(envelope.payload) {
        Ok(payload) => payload,
        Err(_) => return protocol::protocol_mismatch_response(),
    };
    let purpose = match require_purpose(payload.purpose) {
        Ok(purpose) => purpose,
        Err(response) => return response,
    };
    let path = match require_scope_path(payload.path) {
        Ok(path) => path,
        Err(response) => return response,
    };

    let input = RequestReservationInput {
        session_id: envelope.request.session.session_id,
        workspace_id: envelope.request.workspace.workspace_id,
        request_id: payload.request_id,
        repo_id: non_empty_identity(envelope.request.workspace.repo_id),
        worktree_id: non_empty_identity(envelope.request.workspace.worktree_id),
        root: non_empty_identity(envelope.request.workspace.root),
        branch: non_empty_identity(envelope.request.workspace.branch),
        action: payload.action,
        path,
        purpose,
    };

    match request_intent_with_policy(&config.store, input) {
        Ok(outcome) => (StatusCode::OK, Json(request_intent_json(outcome))),
        Err(RequestReservationError::RequestFailed(message)) => (
            StatusCode::CONFLICT,
            Json(json!({
                "status": "error",
                "reason_code": "request_failed",
                "message": message
            })),
        ),
        Err(RequestReservationError::State(message)) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({
                "status": "error",
                "reason_code": "state_error",
                "message": message
            })),
        ),
    }
}

async fn reservation_claim(
    State(config): State<ServerConfig>,
    headers: HeaderMap,
    Json(input): Json<Value>,
) -> (StatusCode, Json<Value>) {
    if !has_valid_bearer_token(&headers, &config.bearer_token) {
        return unauthorized();
    }

    let envelope = match protocol::require_v1_envelope(input) {
        Ok(envelope) => envelope,
        Err(error) => return error.response(),
    };
    let payload: IntentClaimPayload = match serde_json::from_value(envelope.payload) {
        Ok(payload) => payload,
        Err(_) => return protocol::protocol_mismatch_response(),
    };

    let input = ClaimReservationInput {
        session_id: envelope.request.session.session_id,
        workspace_id: envelope.request.workspace.workspace_id,
        wait_id: payload.wait_id,
        repo_id: non_empty_identity(envelope.request.workspace.repo_id),
        worktree_id: non_empty_identity(envelope.request.workspace.worktree_id),
        root: non_empty_identity(envelope.request.workspace.root),
        branch: non_empty_identity(envelope.request.workspace.branch),
    };

    match claim_intent_with_policy_and_audit(&config.store, input) {
        Ok(outcome) => (StatusCode::OK, Json(claim_intent_json(outcome))),
        Err(message) => (
            StatusCode::CONFLICT,
            Json(json!({
                "status": "error",
                "reason_code": "claim_failed",
                "message": message
            })),
        ),
    }
}

async fn reservation_cancel(
    State(config): State<ServerConfig>,
    headers: HeaderMap,
    Json(input): Json<Value>,
) -> (StatusCode, Json<Value>) {
    if !has_valid_bearer_token(&headers, &config.bearer_token) {
        return unauthorized();
    }

    let envelope = match protocol::require_v1_envelope(input) {
        Ok(envelope) => envelope,
        Err(error) => return error.response(),
    };
    let payload: IntentCancelPayload = match serde_json::from_value(envelope.payload) {
        Ok(payload) => payload,
        Err(_) => return protocol::protocol_mismatch_response(),
    };

    let input = CancelReservationInput {
        session_id: envelope.request.session.session_id,
        workspace_id: envelope.request.workspace.workspace_id,
        request_id: payload.request_id,
    };

    match cancel_intent_with_policy_and_audit(&config.store, input) {
        Ok(outcome) => (StatusCode::OK, Json(cancel_intent_json(outcome))),
        Err(message) => (
            StatusCode::CONFLICT,
            Json(json!({
                "status": "error",
                "reason_code": "cancel_failed",
                "message": message
            })),
        ),
    }
}

fn non_empty_identity(value: String) -> Option<String> {
    (!value.is_empty()).then_some(value)
}

fn non_empty_str(value: &str) -> Option<&str> {
    (!value.is_empty()).then_some(value)
}

fn missing_purpose_response() -> (StatusCode, Json<Value>) {
    (
        StatusCode::BAD_REQUEST,
        Json(json!({
            "status": "error",
            "reason_code": "missing_purpose",
            "message": "Reservation purpose is required and must be inferred from the user or agent instruction when it is not explicit."
        })),
    )
}

fn missing_reservation_response() -> (StatusCode, Json<Value>) {
    (
        StatusCode::BAD_REQUEST,
        Json(json!({
            "status": "error",
            "reason_code": "missing_reservation",
            "message": "Claim acquisition requires an active reservation covering the requested path."
        })),
    )
}

fn invalid_claim_path_response(path: String) -> (StatusCode, Json<Value>) {
    (
        StatusCode::BAD_REQUEST,
        Json(json!({
            "status": "error",
            "reason_code": "invalid_claim_path",
            "message": format!("Invalid claim path `{path}`: direct tmp claims are not allowed; claim a file or subdirectory under tmp instead.")
        })),
    )
}

fn claim_conflict_response() -> (StatusCode, Json<Value>) {
    (
        StatusCode::CONFLICT,
        Json(json!({
            "status": "error",
            "reason_code": "claim_conflict",
            "message": "Requested claim conflicts with an active claim or reserved request.",
            "required_next_action": "To wait for this path, call state.reservation.request with action, path, purpose, and request_id. Then poll state.notifications.poll or state.resume.next; when reserved, reread the target and call state.reservation.claim with the wait_id before retrying the write."
        })),
    )
}

fn lease_already_held_response() -> (StatusCode, Json<Value>) {
    (
        StatusCode::OK,
        Json(json!({
            "status": "ok",
            "claim_state": "already_held",
            "message": "Session already holds an active claim for this path."
        })),
    )
}

fn batch_acquire_success_response(success: BatchAcquireSuccess) -> (StatusCode, Json<Value>) {
    let claim_state = if success.result.acquired == 0 {
        "already_held"
    } else if success.result.already_held == 0 {
        "acquired"
    } else {
        "partially_already_held"
    };
    (
        StatusCode::OK,
        Json(json!({
            "status": "ok",
            "claim_state": claim_state,
            "paths": success.paths,
            "acquired": success.result.acquired,
            "already_held": success.result.already_held
        })),
    )
}

fn first_claimable_reservation(
    store: &Store,
    session_id: &str,
    workspace_id: &str,
    paths: &[String],
) -> Result<Option<WaitRecord>, StoreError> {
    for path in paths {
        let reservation = if path.ends_with('/') {
            store.active_reservation_for_directory_by_session(workspace_id, path, session_id)?
        } else {
            store.active_reservation_for_path_by_session(workspace_id, path, session_id)?
        };
        if reservation.is_some() {
            return Ok(reservation);
        }
    }
    Ok(None)
}

fn reservation_claim_required_response(reservation: WaitRecord) -> (StatusCode, Json<Value>) {
    (
        StatusCode::CONFLICT,
        Json(json!({
            "status": "error",
            "reason_code": "reservation_claim_required",
            "message": "A reservation for this session must be claimed before acquiring the claim.",
            "reservation": reservation_json(reservation),
            "required_next_action": "Reread the target, then call state.reservation.claim with the reservation_id before retrying the write."
        })),
    )
}

fn claim_owner_mismatch_response() -> (StatusCode, Json<Value>) {
    (
        StatusCode::CONFLICT,
        Json(json!({
            "status": "error",
            "reason_code": "claim_owner_mismatch",
            "message": "Cannot release a claim owned by another session; wait for the claim to release, or coordinate with the claim owner."
        })),
    )
}

fn claim_not_found_response() -> (StatusCode, Json<Value>) {
    (
        StatusCode::NOT_FOUND,
        Json(json!({
            "status": "error",
            "reason_code": "claim_not_found",
            "message": "No active same-session claim matched the requested path, workspace, and claim type."
        })),
    )
}

fn missing_scope_response() -> (StatusCode, Json<Value>) {
    (
        StatusCode::BAD_REQUEST,
        Json(json!({
            "status": "error",
            "reason_code": "missing_scope",
            "message": "Reservation scope paths must be non-empty after normalization."
        })),
    )
}

fn require_files_planned(
    files_planned: Vec<String>,
) -> Result<Vec<String>, (StatusCode, Json<Value>)> {
    if files_planned.is_empty()
        || files_planned
            .iter()
            .map(String::as_str)
            .any(normalized_relative_path_is_empty)
    {
        return Err(missing_scope_response());
    }
    Ok(files_planned)
}

fn require_scope_path(path: String) -> Result<String, (StatusCode, Json<Value>)> {
    if normalized_relative_path_is_empty(&path) {
        return Err(missing_scope_response());
    }
    Ok(path)
}

fn require_purpose(purpose: String) -> Result<String, (StatusCode, Json<Value>)> {
    let purpose = purpose.trim().to_string();
    if purpose.is_empty() {
        return Err(missing_purpose_response());
    }
    Ok(purpose)
}

async fn lease_acquire(
    State(config): State<ServerConfig>,
    headers: HeaderMap,
    Json(input): Json<LeaseAcquireRequest>,
) -> (StatusCode, Json<Value>) {
    if !has_valid_bearer_token(&headers, &config.bearer_token) {
        return unauthorized();
    }

    let result = match config.store.lock() {
        Ok(store) => {
            let paths = match input.paths() {
                Ok(paths) => paths,
                Err(response) => return response,
            };
            let session_id = input.session_id;
            let workspace_id = input.workspace_id;
            let reservation_id = input.reservation_id;
            if paths.len() == 1 {
                let path = paths[0].clone();
                let observation = match input.root.as_deref().filter(|root| !root.is_empty()) {
                    Some(root) => match claim_observation_for_path(root, &path) {
                        Ok(observation) => Some(observation),
                        Err(error) => return status_response(Err(error)),
                    },
                    None => None,
                };
                let acquire_result = match reservation_id.as_deref() {
                    Some(reservation_id) => store.acquire_claim_for_reservation_with_observation_and_event(
                        reservation_id,
                        &session_id,
                        &workspace_id,
                        &path,
                        observation,
                    ),
                    None => store.acquire_claim_with_observation_and_event(
                        &session_id,
                        &workspace_id,
                        &path,
                        observation,
                    ),
                };
                match acquire_result {
                    Ok(()) => Ok(LeaseAcquireOutcome::Acquired),
                    Err(StoreError::ClaimConflict) => {
                        let reservation = if path.ends_with('/') {
                            store.active_reservation_for_directory_by_session(
                                &workspace_id,
                                &path,
                                &session_id,
                            )
                        } else {
                            store.active_reservation_for_path_by_session(
                                &workspace_id,
                                &path,
                                &session_id,
                            )
                        };
                        match reservation {
                            Ok(Some(reservation)) => {
                                Ok(LeaseAcquireOutcome::Reservation(reservation))
                            }
                            Ok(None) => Err(StoreError::ClaimConflict),
                            Err(error) => Err(error),
                        }
                    }
                    Err(error) => Err(error),
                }
            } else {
                let claims =
                    match input
                        .root
                        .as_deref()
                        .filter(|root| !root.is_empty())
                        .map(|root| {
                            paths
                                .iter()
                                .map(|path| {
                                    claim_observation_for_path(root, path)
                                        .map(|observation| (path.clone(), Some(observation)))
                                })
                                .collect::<Result<Vec<_>, _>>()
                        }) {
                        Some(Ok(claims)) => claims,
                        Some(Err(error)) => return status_response(Err(error)),
                        None => paths
                            .iter()
                            .map(|path| (path.clone(), None))
                            .collect::<Vec<_>>(),
                    };
                let acquire_result = match reservation_id.as_deref() {
                    Some(reservation_id) => store.acquire_claims_for_reservation_with_observations_and_events(
                        reservation_id,
                        &session_id,
                        &workspace_id,
                        claims,
                    ),
                    None => store.acquire_claims_with_observations_and_events(
                        &session_id,
                        &workspace_id,
                        claims,
                    ),
                };
                match acquire_result {
                    Ok(result) => Ok(LeaseAcquireOutcome::Batch(BatchAcquireSuccess {
                        paths,
                        result,
                    })),
                    Err(StoreError::ClaimConflict) => {
                        match first_claimable_reservation(
                            &store,
                            &session_id,
                            &workspace_id,
                            &paths,
                        ) {
                            Ok(Some(reservation)) => {
                                Ok(LeaseAcquireOutcome::Reservation(reservation))
                            }
                            Ok(None) => Err(StoreError::ClaimConflict),
                            Err(error) => Err(error),
                        }
                    }
                    Err(error) => Err(error),
                }
            }
        }
        Err(_) => return status_response(Err("store lock poisoned".to_string())),
    };

    match result {
        Ok(LeaseAcquireOutcome::Acquired) => status_response(Ok(())),
        Ok(LeaseAcquireOutcome::Reservation(reservation)) => {
            reservation_claim_required_response(reservation)
        }
        Ok(LeaseAcquireOutcome::Batch(success)) => batch_acquire_success_response(success),
        Err(StoreError::MissingPurpose) => missing_purpose_response(),
        Err(StoreError::MissingReservation) => missing_reservation_response(),
        Err(StoreError::InvalidClaimPath(path)) => invalid_claim_path_response(path),
        Err(StoreError::ClaimAlreadyHeld) => lease_already_held_response(),
        Err(StoreError::ClaimConflict) => claim_conflict_response(),
        Err(error) => status_response(Err(error.to_string())),
    }
}

async fn lease_refresh_observation(
    State(config): State<ServerConfig>,
    headers: HeaderMap,
    Json(input): Json<LeaseRequest>,
) -> (StatusCode, Json<Value>) {
    if !has_valid_bearer_token(&headers, &config.bearer_token) {
        return unauthorized();
    }

    let root = match input.root.as_deref().filter(|root| !root.is_empty()) {
        Some(root) => root,
        None => {
            return status_response(Err(
                "root is required to refresh claim observations".to_string()
            ));
        }
    };
    let observation = match claim_observation_for_path(root, &input.path) {
        Ok(observation) => observation,
        Err(error) => return status_response(Err(error)),
    };

    let result = match config.store.lock() {
        Ok(store) => store.refresh_exact_file_claim_observation(
            input.session_id,
            input.workspace_id,
            input.path,
            observation,
        ),
        Err(_) => return status_response(Err("store lock poisoned".to_string())),
    };

    match result {
        Ok(()) => status_response(Ok(())),
        Err(StoreError::ClaimOwnerMismatch) => claim_owner_mismatch_response(),
        Err(StoreError::ClaimNotFound) => claim_not_found_response(),
        Err(error) => status_response(Err(error.to_string())),
    }
}

async fn lease_release(
    State(config): State<ServerConfig>,
    headers: HeaderMap,
    Json(input): Json<LeaseRequest>,
) -> (StatusCode, Json<Value>) {
    if !has_valid_bearer_token(&headers, &config.bearer_token) {
        return unauthorized();
    }

    let result = match config.store.lock() {
        Ok(store) => store.release_claim(input.session_id, input.workspace_id, input.path),
        Err(_) => return status_response(Err("store lock poisoned".to_string())),
    };

    match result {
        Ok(()) => status_response(Ok(())),
        Err(StoreError::ClaimOwnerMismatch) => claim_owner_mismatch_response(),
        Err(StoreError::ClaimNotFound) => claim_not_found_response(),
        Err(error) => status_response(Err(error.to_string())),
    }
}

async fn activity_observe(
    State(config): State<ServerConfig>,
    headers: HeaderMap,
    Json(input): Json<ActivityRequest>,
) -> (StatusCode, Json<Value>) {
    if !has_valid_bearer_token(&headers, &config.bearer_token) {
        return unauthorized();
    }

    append_activity_response(&config.store, input)
}

async fn activity_finalize(
    State(config): State<ServerConfig>,
    headers: HeaderMap,
    Json(input): Json<ActivityRequest>,
) -> (StatusCode, Json<Value>) {
    if !has_valid_bearer_token(&headers, &config.bearer_token) {
        return unauthorized();
    }

    let result = config
        .store
        .lock()
        .map_err(|_| "store lock poisoned".to_string())
        .and_then(|store| {
            store
                .finalize_session_activity_with_phase(
                    &input.session_id,
                    &input.workspace_id,
                    input.phase.unwrap_or(ActivityPhase::Done),
                )
                .map_err(|error| error.to_string())
        });

    match result {
        Ok((released_claims, completed_reservations)) => (
            StatusCode::OK,
            Json(json!({
                "status": "ok",
                "released_claims": released_claims,
                "completed_reservations": completed_reservations
            })),
        ),
        Err(message) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({
                "error": message
            })),
        ),
    }
}

async fn context_render(
    State(config): State<ServerConfig>,
    headers: HeaderMap,
    Json(input): Json<ContextRenderRequest>,
) -> (StatusCode, Json<Value>) {
    if !has_valid_bearer_token(&headers, &config.bearer_token) {
        return unauthorized();
    }

    let mode = match input.mode.as_deref() {
        Some("detailed") => RenderMode::Detailed,
        _ => RenderMode::Brief,
    };
    let Some(workspace_id) = input.workspace_id.as_deref() else {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({
                "status": "error",
                "message": "workspace_id is required"
            })),
        );
    };
    let result = config
        .store
        .lock()
        .map_err(|_| "store lock poisoned".to_string())
        .and_then(|store| {
            let identity_filter = CurrentStateIdentityFilter {
                repo_id: input.repo_id.as_deref().and_then(non_empty_str),
                worktree_id: input.worktree_id.as_deref().and_then(non_empty_str),
                root: input.root.as_deref().and_then(non_empty_str),
                exclude_session_id: input.session_id.as_deref().and_then(non_empty_str),
            };
            store
                .live_current_state_for_workspace_identity(
                    workspace_id,
                    identity_filter,
                    input.resource.as_deref().and_then(non_empty_str),
                )
                .map_err(|error| error.to_string())
        });

    let live = match result {
        Ok(live) => live,
        Err(message) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({
                    "status": "error",
                    "message": message
                })),
            );
        }
    };
    let package = ContextPackage::from_items(live.items.clone());
    let prompt_text = render_prompt_text(&package, mode);

    (
        StatusCode::OK,
        Json(json!({
            "status": "ok",
            "mode": match mode {
                RenderMode::Brief => "brief",
                RenderMode::Detailed => "detailed",
            },
            "current": live.summary,
            "items": live.items,
            "prompt_text": prompt_text
        })),
    )
}

async fn conflicts_check(
    State(config): State<ServerConfig>,
    headers: HeaderMap,
    Json(input): Json<AuthorizeRequest>,
) -> (StatusCode, Json<Value>) {
    if !has_valid_bearer_token(&headers, &config.bearer_token) {
        return unauthorized();
    }

    let input = AuthorizeWriteInput {
        session_id: input.session_id,
        reservation_id: input.reservation_id,
        workspace_id: input.workspace_id,
        repo_id: None,
        worktree_id: None,
        root: None,
        branch: None,
        source_kind: None,
        source_event: None,
        queue_on_conflict: input.queue_on_conflict,
        queue_purpose: None,
        action: input.action,
        old_path: input.old_path,
        new_path: input.new_path,
        path: input.path,
        base_observations: Vec::new(),
    };

    match authorize_with_policy(&config.store, input, false) {
        Ok(outcome) => (StatusCode::OK, Json(authorization_json(outcome))),
        Err(message) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({
                "decision": "error",
                "reason_code": "state_error",
                "message": message
            })),
        ),
    }
}

async fn reconcile_ack(
    State(config): State<ServerConfig>,
    headers: HeaderMap,
    Json(input): Json<ReconcileAckRequest>,
) -> (StatusCode, Json<Value>) {
    if !has_valid_bearer_token(&headers, &config.bearer_token) {
        return unauthorized();
    }

    let decision = match input.decision.parse::<ReconciliationDecision>() {
        Ok(decision) => decision,
        Err(message) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({
                    "error": message
                })),
            );
        }
    };

    let result = config
        .store
        .lock()
        .map_err(|_| "store lock poisoned".to_string())
        .and_then(|store| {
            store
                .append_reconciliation_ack(&input.session_id)
                .map_err(|error| error.to_string())
        });
    if let Err(message) = result {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({
                "error": message
            })),
        );
    }

    (
        StatusCode::OK,
        Json(json!({
            "status": "ok",
            "session_id": input.session_id,
            "workspace_id": input.workspace_id,
            "files_reread": input.files_reread,
            "human_change_summary": input.human_change_summary,
            "clears_human_write_block": decision.clears_human_write_block()
        })),
    )
}

async fn outbox_sync(
    State(config): State<ServerConfig>,
    headers: HeaderMap,
    Json(input): Json<OutboxSyncRequest>,
) -> (StatusCode, Json<Value>) {
    if !has_valid_bearer_token(&headers, &config.bearer_token) {
        return unauthorized();
    }

    let result = config
        .store
        .lock()
        .map_err(|_| "store lock poisoned".to_string())
        .and_then(|store| {
            store
                .append_outbox(
                    OutboxEntry::synced(
                        input.outbox_id.clone(),
                        input.session_id.clone(),
                        input.sequence,
                    )
                    .with_workspace_id(input.workspace_id.clone())
                    .with_event_type(input.event_type.clone())
                    .with_payload(input.payload.clone()),
                )
                .map_err(|error| error.to_string())
        });

    match result {
        Ok(()) => (
            StatusCode::OK,
            Json(json!({
                "status": "ok",
                "sync_status": "synced",
                "event_type": input.event_type,
                "workspace_id": input.workspace_id,
                "payload": input.payload
            })),
        ),
        Err(message) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({
                "error": message
            })),
        ),
    }
}

async fn notifications_poll(
    State(config): State<ServerConfig>,
    headers: HeaderMap,
    Json(input): Json<NotificationsPollRequest>,
) -> (StatusCode, Json<Value>) {
    if !has_valid_bearer_token(&headers, &config.bearer_token) {
        return unauthorized();
    }

    let result = config
        .store
        .lock()
        .map_err(|_| "store lock poisoned".to_string())
        .and_then(|store| {
            store
                .pending_notifications(&input.session_id, &input.workspace_id)
                .map_err(|error| error.to_string())
        });

    match result {
        Ok(notifications) => (
            StatusCode::OK,
            Json(json!({
                "status": "ok",
                "workspace_id": input.workspace_id,
                "notifications": notifications
            })),
        ),
        Err(message) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({
                "error": message
            })),
        ),
    }
}

async fn notifications_stream(
    State(config): State<ServerConfig>,
    headers: HeaderMap,
    Query(input): Query<NotificationsPollRequest>,
) -> Response {
    if !has_valid_bearer_token(&headers, &config.bearer_token) {
        return unauthorized().into_response();
    }

    Sse::new(notification_sse_stream(config.store.clone(), input))
        .keep_alive(KeepAlive::default())
        .into_response()
}

fn notification_sse_stream(
    store: SharedStore,
    input: NotificationsPollRequest,
) -> impl Stream<Item = Result<SseEvent, Infallible>> {
    let pending = Arc::new(Mutex::new(VecDeque::<NotificationRecord>::new()));
    let interval = tokio::time::interval(Duration::from_secs(1));
    IntervalStream::new(interval).filter_map(move |_| {
        let store = store.clone();
        let input = input.clone();
        let pending = pending.clone();
        let next = {
            let mut queued = pending
                .lock()
                .expect("notification queue lock should not poison");
            if queued.is_empty() {
                if let Ok(notifications) = store
                    .lock()
                    .map_err(|_| "store lock poisoned".to_string())
                    .and_then(|store| {
                        store
                            .pending_notifications(&input.session_id, &input.workspace_id)
                            .map_err(|error| error.to_string())
                    })
                {
                    queued.extend(notifications);
                }
            }
            queued.pop_front()
        };

        next.map(|notification| Ok(notification_sse_event(notification)))
    })
}

fn notification_sse_event(notification: NotificationRecord) -> SseEvent {
    let required_next_action = if notification.kind == "reservation_granted" {
        Some(
            "Reread the target, then call state.reservation.claim for the reservation before writing.",
        )
    } else {
        None
    };
    SseEvent::default()
        .id(notification.notification_id.clone())
        .event(notification.kind.clone())
        .data(
            json!({
                "status": "ok",
                "notification_id": notification.notification_id,
                "workspace_id": notification.workspace_id,
                "kind": notification.kind,
                "payload": notification.payload,
                "required_next_action": required_next_action
            })
            .to_string(),
        )
}

async fn resume_next(
    State(config): State<ServerConfig>,
    headers: HeaderMap,
    Json(input): Json<ResumeNextRequest>,
) -> (StatusCode, Json<Value>) {
    if !has_valid_bearer_token(&headers, &config.bearer_token) {
        return unauthorized();
    }

    let result = config
        .store
        .lock()
        .map_err(|_| "store lock poisoned".to_string())
        .and_then(|store| {
            store
                .next_reservation_for_session(&input.session_id, &input.workspace_id)
                .map_err(|error| error.to_string())
        });

    match result {
        Ok(Some(reservation)) => (
            StatusCode::OK,
            Json(json!({
                "status": "ok",
                "resume_available": true,
                "reservation": reservation_json(reservation),
                "required_next_action": "Reread the target, then call state.reservation.claim for the reservation before writing."
            })),
        ),
        Ok(None) => (
            StatusCode::OK,
            Json(json!({
                "status": "ok",
                "resume_available": false,
                "reservation": null,
                "required_next_action": null
            })),
        ),
        Err(message) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({
                "error": message
            })),
        ),
    }
}

fn authorize_with_policy(
    store: &SharedStore,
    input: AuthorizeWriteInput,
    allow_queue_side_effects: bool,
) -> Result<AuthorizationOutcome, String> {
    let store = store
        .lock()
        .map_err(|_| "store lock poisoned".to_string())?;
    PolicyService::new(&store).authorize_write(input, allow_queue_side_effects)
}

fn authorize_with_policy_and_audit(
    store: &SharedStore,
    input: AuthorizeWriteInput,
) -> Result<AuthorizationOutcome, String> {
    let audit_input = input.clone();
    let store = store
        .lock()
        .map_err(|_| "store lock poisoned".to_string())?;
    store.transaction(
        |store| {
            if let Some(heartbeat) = authorize_heartbeat_event(&audit_input) {
                store.append(heartbeat).map_err(|error| error.to_string())?;
            }
            let outcome = PolicyService::new(store).authorize_write(input, true)?;
            if matches!(outcome.decision.decision, stateful_core::DecisionKind::Deny) {
                let audit = authorization_denied_audit_event(&audit_input, &outcome);
                store.append(audit).map_err(|error| error.to_string())?;
            }
            Ok(outcome)
        },
        |error| error.to_string(),
    )
}

fn authorize_heartbeat_event(input: &AuthorizeWriteInput) -> Option<Event> {
    let workspace_id = input.workspace_id.as_ref()?;
    Some(with_request_identity(
        Event::session_heartbeat(input.session_id.clone(), workspace_id.clone()),
        WorkspaceIdentityRequest {
            repo_id: input.repo_id.clone(),
            worktree_id: input.worktree_id.clone(),
            root: input.root.clone(),
            branch: input.branch.clone(),
        },
    ))
}

fn claim_intent_with_policy_and_audit(
    store: &SharedStore,
    input: ClaimReservationInput,
) -> Result<ClaimReservationOutcome, String> {
    let store = store
        .lock()
        .map_err(|_| "store lock poisoned".to_string())?;
    store.transaction(
        |store| {
            let outcome = PolicyService::new(store).claim_intent(input)?;
            let audit = reservation_claimed_audit_event(&outcome);
            store.append(audit).map_err(|error| error.to_string())?;
            Ok(outcome)
        },
        |error| error.to_string(),
    )
}

fn request_intent_with_policy(
    store: &SharedStore,
    input: RequestReservationInput,
) -> Result<RequestReservationOutcome, RequestReservationError> {
    let audit_input = input.clone();
    let store = store
        .lock()
        .map_err(|_| RequestReservationError::State("store lock poisoned".to_string()))?;
    store.transaction(
        |store| {
            let outcome = PolicyService::new(store)
                .request_intent(input)
                .map_err(RequestReservationError::RequestFailed)?;
            let audit = reservation_requested_audit_event(&audit_input, &outcome);
            store
                .append(audit)
                .map_err(|error| RequestReservationError::State(error.to_string()))?;
            Ok(outcome)
        },
        |error| RequestReservationError::State(error.to_string()),
    )
}

enum RequestReservationError {
    RequestFailed(String),
    State(String),
}

fn cancel_intent_with_policy_and_audit(
    store: &SharedStore,
    input: CancelReservationInput,
) -> Result<CancelReservationOutcome, String> {
    let request_id = input.request_id.clone();
    let store = store
        .lock()
        .map_err(|_| "store lock poisoned".to_string())?;
    store.transaction(
        |store| {
            let outcome = PolicyService::new(store).cancel_intent(input)?;
            let audit = reservation_canceled_audit_event(&request_id, &outcome);
            store.append(audit).map_err(|error| error.to_string())?;
            Ok(outcome)
        },
        |error| error.to_string(),
    )
}

fn append_event(store: &SharedStore, event: Event) -> Result<(), String> {
    store
        .lock()
        .map_err(|_| "store lock poisoned".to_string())
        .and_then(|store| store.append(event).map_err(|error| error.to_string()))
}

fn append_event_response(store: &SharedStore, event: Event) -> (StatusCode, Json<Value>) {
    match append_event(store, event) {
        Ok(()) => (
            StatusCode::OK,
            Json(json!({
                "status": "ok"
            })),
        ),
        Err(message) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({
                "error": message
            })),
        ),
    }
}

fn append_activity_response(
    store: &SharedStore,
    input: ActivityRequest,
) -> (StatusCode, Json<Value>) {
    let result = store
        .lock()
        .map_err(|_| "store lock poisoned".to_string())
        .and_then(|store| {
            store
                .append_activity_with_phase(
                    input.session_id,
                    input.workspace_id,
                    input.phase.unwrap_or(ActivityPhase::Exploring),
                )
                .map_err(|error| error.to_string())
        });

    status_response(result)
}

fn with_request_identity(mut event: Event, identity: WorkspaceIdentityRequest) -> Event {
    event.repo_id = identity.repo_id;
    event.worktree_id = identity.worktree_id;
    event.root = identity.root;
    event.branch = identity.branch;
    event
}

fn with_wait_identity(mut event: Event, wait: &WaitRecord) -> Event {
    event.repo_id = wait.repo_id.clone();
    event.worktree_id = wait.worktree_id.clone();
    event.root = wait.root.clone();
    event.branch = wait.branch.clone();
    event
}

fn reservation_requested_audit_event(
    input: &RequestReservationInput,
    outcome: &RequestReservationOutcome,
) -> Event {
    let wait_id = outcome
        .wait
        .as_ref()
        .map(|wait| wait.record.wait_id.clone())
        .or_else(|| {
            outcome
                .reservation
                .as_ref()
                .map(|reservation| reservation.wait_id.clone())
        });
    let queue_position = outcome.wait.as_ref().and_then(|wait| wait.queue_position);
    let blocking_session_id = outcome
        .wait
        .as_ref()
        .and_then(|wait| wait.record.blocking_session_id.clone())
        .or_else(|| {
            outcome
                .reservation
                .as_ref()
                .and_then(|reservation| reservation.blocking_session_id.clone())
        });

    with_request_identity(
        Event::reservation_requested(
            input.session_id.clone(),
            input.workspace_id.clone(),
            outcome.request_id.clone(),
            input.action.clone(),
            input.path.clone(),
            input.purpose.clone(),
            outcome.request_state.clone(),
            wait_id,
            queue_position,
            blocking_session_id,
        ),
        WorkspaceIdentityRequest {
            repo_id: input.repo_id.clone(),
            worktree_id: input.worktree_id.clone(),
            root: input.root.clone(),
            branch: input.branch.clone(),
        },
    )
}

fn reservation_claimed_audit_event(outcome: &ClaimReservationOutcome) -> Event {
    let reservation = &outcome.reservation;
    with_wait_identity(
        Event::reservation_claimed(
            reservation.session_id.clone(),
            reservation.workspace_id.clone(),
            reservation.wait_id.clone(),
            reservation.action.clone(),
            reservation.relative_path.clone(),
            reservation.purpose.clone(),
        ),
        reservation,
    )
}

fn reservation_canceled_audit_event(request_id: &str, outcome: &CancelReservationOutcome) -> Event {
    let wait = &outcome.wait;
    with_wait_identity(
        Event::reservation_canceled(
            wait.session_id.clone(),
            wait.workspace_id.clone(),
            request_id.to_string(),
            wait.wait_id.clone(),
            wait.action.clone(),
            wait.relative_path.clone(),
            wait.purpose.clone(),
        ),
        wait,
    )
}

fn authorization_denied_audit_event(
    input: &AuthorizeWriteInput,
    outcome: &AuthorizationOutcome,
) -> Event {
    let mut event = with_request_identity(
        Event::authorization_denied(
            input.session_id.clone(),
            input.workspace_id.clone().unwrap_or_default(),
            input.action.clone(),
            input.path.clone(),
            input.old_path.clone(),
            input.new_path.clone(),
            outcome.decision.reason_code.clone(),
            outcome.decision.message.clone(),
        ),
        WorkspaceIdentityRequest {
            repo_id: input.repo_id.clone(),
            worktree_id: input.worktree_id.clone(),
            root: input.root.clone(),
            branch: input.branch.clone(),
        },
    );

    if let Some(wait) = outcome.wait.as_ref() {
        event.payload["wait"] = json!({
            "wait_id": wait.record.wait_id,
            "reservation_id": wait.record.wait_id,
            "session_id": wait.record.session_id,
            "workspace_id": wait.record.workspace_id,
            "relative_path": wait.record.relative_path,
            "action": wait.record.action,
            "status": wait.record.status,
            "purpose": wait.record.purpose,
            "queue_position": wait.queue_position,
            "blocking_session_id": wait.record.blocking_session_id,
        });
    }

    event
}

fn status_response(result: Result<(), String>) -> (StatusCode, Json<Value>) {
    match result {
        Ok(()) => (
            StatusCode::OK,
            Json(json!({
                "status": "ok"
            })),
        ),
        Err(message) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({
                "error": message
            })),
        ),
    }
}

fn authorization_json(outcome: AuthorizationOutcome) -> Value {
    let decision = outcome.decision;
    let mut value = json!({
        "decision": match decision.decision {
            stateful_core::DecisionKind::Allow => "allow",
            stateful_core::DecisionKind::Warn => "warn",
            stateful_core::DecisionKind::Deny => "deny",
            stateful_core::DecisionKind::Error => "error",
        },
        "reason_code": decision.reason_code,
        "message": decision.message,
        "required_next_action": decision.required_next_action,
    });

    if let Some(wait) = outcome.wait {
        value["wait"] = wait_queue_json(wait);
    }

    if let Some(reservation) = outcome.reservation {
        value["reservation"] = reservation_json(reservation);
    }

    value
}

fn claim_intent_json(outcome: ClaimReservationOutcome) -> Value {
    let reservation = outcome.reservation;
    json!({
        "status": "ok",
        "reservation": reservation_json(reservation)
    })
}

fn request_intent_json(outcome: RequestReservationOutcome) -> Value {
    let mut value = json!({
        "status": "ok",
        "request_id": outcome.request_id,
        "request_state": outcome.request_state,
    });

    if let Some(wait) = outcome.wait {
        value["wait"] = wait_queue_json(wait);
    }
    if let Some(reservation) = outcome.reservation {
        value["reservation"] = reservation_json(reservation);
    }

    value
}

fn cancel_intent_json(outcome: CancelReservationOutcome) -> Value {
    let request_state = outcome.wait.status.clone();
    json!({
        "status": "ok",
        "request_id": outcome.request_id,
        "request_state": request_state,
        "wait": wait_record_json(outcome.wait, None),
    })
}

fn wait_queue_json(wait: WaitQueueInfo) -> Value {
    wait_record_json(wait.record, wait.queue_position)
}

fn wait_record_json(record: WaitRecord, queue_position: Option<u64>) -> Value {
    let reservation_id = record.wait_id.clone();
    json!({
        "reservation_id": reservation_id,
        "wait_id": record.wait_id,
        "session_id": record.session_id,
        "workspace_id": record.workspace_id,
        "relative_path": record.relative_path,
        "action": record.action,
        "status": record.status,
        "queue_position": queue_position,
        "blocking_session_id": record.blocking_session_id,
        "purpose": record.purpose,
    })
}

fn reservation_json(reservation: WaitRecord) -> Value {
    let reservation_id = reservation.wait_id.clone();
    json!({
        "reservation_id": reservation_id,
        "wait_id": reservation.wait_id,
        "session_id": reservation.session_id,
        "workspace_id": reservation.workspace_id,
        "relative_path": reservation.relative_path,
        "action": reservation.action,
        "status": reservation.status,
        "reservation_expires_at": reservation.reservation_expires_at,
        "purpose": reservation.purpose,
    })
}

fn unauthorized() -> (StatusCode, Json<Value>) {
    (
        StatusCode::UNAUTHORIZED,
        Json(json!({
            "error": "unauthorized"
        })),
    )
}

fn has_valid_bearer_token(headers: &HeaderMap, expected_token: &str) -> bool {
    let Some(header) = headers.get(axum::http::header::AUTHORIZATION) else {
        return false;
    };

    let Ok(header) = header.to_str() else {
        return false;
    };

    header == format!("Bearer {expected_token}")
}

#[derive(Debug, Deserialize)]
struct CurrentQuery {
    resource: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ReservationDeclarePayload {
    purpose: String,
    files_planned: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct ReservationRequestPayload {
    request_id: String,
    action: String,
    path: String,
    purpose: String,
}

#[derive(Debug, Deserialize)]
struct IntentClaimPayload {
    wait_id: String,
}

#[derive(Debug, Deserialize)]
struct IntentCancelPayload {
    request_id: String,
}

#[derive(Debug, Deserialize)]
struct SessionRequest {
    session_id: String,
    workspace_id: String,
    #[serde(flatten)]
    identity: WorkspaceIdentityRequest,
}

enum LeaseAcquireOutcome {
    Acquired,
    Reservation(WaitRecord),
    Batch(BatchAcquireSuccess),
}

struct BatchAcquireSuccess {
    paths: Vec<String>,
    result: ClaimBatchAcquireResult,
}

#[derive(Debug, Deserialize)]
struct LeaseAcquireRequest {
    session_id: String,
    workspace_id: String,
    #[serde(default)]
    reservation_id: Option<String>,
    #[serde(default)]
    path: Option<String>,
    #[serde(default)]
    paths: Vec<String>,
    #[serde(default)]
    root: Option<String>,
}

impl LeaseAcquireRequest {
    fn paths(&self) -> Result<Vec<String>, (StatusCode, Json<Value>)> {
        let mut paths = Vec::new();
        if let Some(path) = &self.path {
            paths.push(path.clone());
        }
        paths.extend(self.paths.iter().cloned());
        require_files_planned(paths)
    }
}

#[derive(Debug, Deserialize)]
struct LeaseRequest {
    session_id: String,
    workspace_id: String,
    path: String,
    #[serde(default)]
    root: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ActivityRequest {
    session_id: String,
    workspace_id: String,
    #[serde(default)]
    phase: Option<ActivityPhase>,
}

#[derive(Debug, Default, Deserialize)]
struct WorkspaceIdentityRequest {
    #[serde(default)]
    repo_id: Option<String>,
    #[serde(default)]
    worktree_id: Option<String>,
    #[serde(default)]
    root: Option<String>,
    #[serde(default)]
    branch: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct NotificationsPollRequest {
    session_id: String,
    workspace_id: String,
}

#[derive(Debug, Deserialize)]
struct ResumeNextRequest {
    session_id: String,
    workspace_id: String,
}

#[derive(Debug, Deserialize)]
struct AuthorizePayload {
    #[serde(default)]
    queue_on_conflict: bool,
    #[serde(default)]
    purpose: Option<String>,
    #[serde(default)]
    reservation_id: Option<String>,
    action: String,
    #[serde(default)]
    old_path: Option<String>,
    #[serde(default)]
    new_path: Option<String>,
    path: String,
    #[serde(default)]
    base_observations: Vec<BaseObservationPayload>,
}

#[derive(Debug, Deserialize)]
struct BaseObservationPayload {
    path: String,
    exists: bool,
    #[serde(default)]
    content_hash: Option<String>,
}

impl From<BaseObservationPayload> for BaseObservation {
    fn from(payload: BaseObservationPayload) -> Self {
        Self {
            path: payload.path,
            exists: payload.exists,
            content_hash: payload.content_hash,
        }
    }
}

#[derive(Debug, Deserialize)]
struct AuthorizeRequest {
    session_id: String,
    #[serde(default)]
    workspace_id: Option<String>,
    #[serde(default)]
    reservation_id: Option<String>,
    #[serde(default)]
    queue_on_conflict: bool,
    action: String,
    #[serde(default)]
    old_path: Option<String>,
    #[serde(default)]
    new_path: Option<String>,
    path: String,
}

#[derive(Debug, Deserialize)]
struct ContextRenderRequest {
    mode: Option<String>,
    resource: Option<String>,
    workspace_id: Option<String>,
    #[serde(default)]
    session_id: Option<String>,
    #[serde(default)]
    repo_id: Option<String>,
    #[serde(default)]
    worktree_id: Option<String>,
    #[serde(default)]
    root: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ReconcileAckRequest {
    session_id: String,
    workspace_id: String,
    decision: String,
    files_reread: Vec<String>,
    human_change_summary: String,
}

#[derive(Debug, Deserialize)]
struct OutboxSyncRequest {
    outbox_id: String,
    session_id: String,
    workspace_id: String,
    sequence: u64,
    event_type: String,
    payload: Value,
}
