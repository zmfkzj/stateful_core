mod policy_service;
mod protocol;
pub use policy_service::CoordinationMode;

use axum::{
    Json, Router,
    extract::{Query, Request, State},
    http::{HeaderMap, StatusCode},
    middleware::{self, Next},
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
    ClaimBatchAcquireResult, CurrentStateIdentityFilter, Event, HumanObservationConfidence,
    HumanObservationInput, HumanObservationKind, NotificationRecord, OutboxEntry,
    ReconciliationAckInput, Store, StoreError, WaitRecord,
};
use std::{
    collections::VecDeque,
    convert::Infallible,
    net::SocketAddr,
    str::FromStr,
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
    coordination_mode: CoordinationMode,
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
            coordination_mode: CoordinationMode::Enforcement,
        }
    }

    pub fn with_maintenance_interval(mut self, interval: Duration) -> Self {
        self.maintenance_interval = interval;
        self
    }

    pub fn with_coordination_mode(mut self, mode: CoordinationMode) -> Self {
        self.coordination_mode = mode;
        self
    }
}

type SharedStore = Arc<Mutex<Store>>;

pub fn build_router(config: ServerConfig) -> Router {
    let protected = Router::new()
        .route("/v1/current", get(current))
        .route("/v1/events", get(events))
        .route("/v1/runtime/identity", get(runtime_identity))
        .route("/v1/session/register", post(session_register))
        .route("/v1/session/heartbeat", post(agent_heartbeat))
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
        .route("/v1/activity/finalize", post(activity_finalize))
        .route("/v1/authorize", post(authorize))
        .route("/v1/human/observe", post(human_observe))
        .route("/v1/human/save-check", post(human_save_check))
        .route("/v1/reconcile/ack", post(reconcile_ack))
        .route("/v1/context/render", post(context_render))
        .route("/v1/notifications/poll", post(notifications_poll))
        .route("/v1/notifications/stream", get(notifications_stream))
        .route("/v1/resume/next", post(resume_next))
        .route("/v1/outbox/sync", post(outbox_sync))
        .route_layer(middleware::from_fn_with_state(
            config.clone(),
            require_bearer,
        ));

    Router::new()
        .route("/health", get(health))
        .merge(protected)
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

async fn require_bearer(
    State(config): State<ServerConfig>,
    request: Request,
    next: Next,
) -> Response {
    if !has_valid_bearer_token(request.headers(), &config.bearer_token) {
        return unauthorized().into_response();
    }

    next.run(request).await
}

async fn current(
    State(config): State<ServerConfig>,
    Query(input): Query<CurrentQuery>,
) -> (StatusCode, Json<Value>) {
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
    Query(input): Query<EventsQuery>,
) -> (StatusCode, Json<Value>) {
    let limit = input.limit.unwrap_or(100).clamp(1, 100);
    let result = config
        .store
        .lock()
        .map_err(|_| "store lock poisoned".to_string())
        .and_then(|store| {
            store
                .recent_events_filtered(
                    input.workspace_id.as_deref(),
                    input.since.as_deref(),
                    limit,
                )
                .map_err(|error| error.to_string())
        });

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

async fn runtime_identity(State(config): State<ServerConfig>) -> (StatusCode, Json<Value>) {
    (
        StatusCode::OK,
        Json(json!({
            "status": "ok",
            "pid": std::process::id(),
            "protocol_version": "stateful.v1",
            "capabilities": RUNTIME_CAPABILITIES,
            "coordination_mode": config.coordination_mode.as_str()
        })),
    )
}

async fn session_register(
    State(config): State<ServerConfig>,
    Json(input): Json<SessionRequest>,
) -> (StatusCode, Json<Value>) {
    if let Err(response) = require_agent_id(&input.agent_id) {
        return response;
    }

    append_event_response(
        &config.store,
        with_request_identity(
            Event::agent_registered(input.agent_id, input.workspace_id),
            input.identity,
        ),
    )
}

async fn agent_heartbeat(
    State(config): State<ServerConfig>,
    Json(input): Json<SessionRequest>,
) -> (StatusCode, Json<Value>) {
    if let Err(response) = require_agent_id(&input.agent_id) {
        return response;
    }

    append_event_response(
        &config.store,
        with_request_identity(
            Event::agent_heartbeat(input.agent_id, input.workspace_id),
            input.identity,
        ),
    )
}

async fn authorize(
    State(config): State<ServerConfig>,
    Json(input): Json<Value>,
) -> (StatusCode, Json<serde_json::Value>) {
    let envelope = match protocol::require_v1_envelope(input) {
        Ok(envelope) => envelope,
        Err(error) => return error.response(),
    };
    let payload: AuthorizePayload = match serde_json::from_value(envelope.payload) {
        Ok(payload) => payload,
        Err(_) => return protocol::protocol_mismatch_response(),
    };
    let stateful_core::RequestEnvelope {
        agent,
        workspace,
        source,
        ..
    } = envelope.request;
    if let Err(response) = require_agent_id(&agent.agent_id) {
        return response;
    }

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
        agent_id: agent.agent_id,
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

    let outcome =
        match authorize_with_policy_and_audit(&config.store, config.coordination_mode, input) {
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

async fn human_observe(
    State(config): State<ServerConfig>,
    Json(input): Json<Value>,
) -> (StatusCode, Json<Value>) {
    let envelope = match protocol::require_v1_envelope(input) {
        Ok(envelope) => envelope,
        Err(error) => return error.response(),
    };
    let payload: HumanObservePayload = match serde_json::from_value(envelope.payload) {
        Ok(payload) => payload,
        Err(_) => return protocol::protocol_mismatch_response(),
    };
    let kind = match HumanObservationKind::from_str(&payload.kind) {
        Ok(kind) => kind,
        Err(message) => return error_response(StatusCode::BAD_REQUEST, message),
    };
    let confidence = match HumanObservationConfidence::from_str(&payload.confidence) {
        Ok(confidence) => confidence,
        Err(message) => return error_response(StatusCode::BAD_REQUEST, message),
    };
    let path = stateful_core::normalize_relative_path(&payload.path);
    if normalized_relative_path_is_empty(&path) {
        return error_response(StatusCode::BAD_REQUEST, "path is required");
    }

    let workspace_id = envelope.request.workspace.workspace_id;
    let attributed_to_agent = match config.store.lock() {
        Ok(store) => store
            .write_fence_owner_for_observation(&workspace_id, &path, &envelope.request.observed_at)
            .map(|owner| owner.is_some()),
        Err(_) => return status_response(Err("store lock poisoned".to_string())),
    };
    let attributed_to_agent = match attributed_to_agent {
        Ok(attributed) => attributed,
        Err(error) => return status_response(Err(error.to_string())),
    };

    let observation = HumanObservationInput {
        workspace_id,
        relative_path: path,
        kind,
        confidence,
        source: payload.source,
        observed_at: envelope.request.observed_at,
        summary: payload.summary,
    };
    let result = match config.store.lock() {
        Ok(store) => store.record_human_observation(observation),
        Err(_) => return status_response(Err("store lock poisoned".to_string())),
    };

    match result {
        Ok(observation_id) => (
            StatusCode::OK,
            Json(json!({
                "status": "ok",
                "attributed": if attributed_to_agent { "agent" } else { "human" },
                "observation_id": observation_id
            })),
        ),
        Err(error) => status_response(Err(error.to_string())),
    }
}

async fn human_save_check(
    State(config): State<ServerConfig>,
    Json(input): Json<HumanSaveCheckRequest>,
) -> (StatusCode, Json<Value>) {
    let paths = match require_files_planned(input.paths) {
        Ok(paths) => paths,
        Err(response) => return response,
    };
    let result = match config.store.lock() {
        Ok(store) => {
            let mut conflicts = Vec::new();
            for path in &paths {
                match store.active_claim_owner(&input.workspace_id, path) {
                    Ok(Some(owner)) => conflicts.push(json!({
                        "path": path,
                        "conflict_kind": "claim",
                        "owner_agent_id": owner
                    })),
                    Ok(None) => {}
                    Err(error) => return status_response(Err(error.to_string())),
                }
                match store.active_write_fence_owner(&input.workspace_id, path) {
                    Ok(Some(owner)) => conflicts.push(json!({
                        "path": path,
                        "conflict_kind": "write_fence",
                        "owner_agent_id": owner
                    })),
                    Ok(None) => {}
                    Err(error) => return status_response(Err(error.to_string())),
                }
            }
            Ok(conflicts)
        }
        Err(_) => return status_response(Err("store lock poisoned".to_string())),
    };

    match result {
        Ok(conflicts) if conflicts.is_empty() => (
            StatusCode::OK,
            Json(json!({
                "status": "ok",
                "decision": "clear",
                "conflicts": []
            })),
        ),
        Ok(conflicts) => (
            StatusCode::OK,
            Json(json!({
                "status": "ok",
                "decision": "warn",
                "reason_code": "human_save_conflict",
                "conflicts": conflicts
            })),
        ),
        Err(error) => status_response(Err(error)),
    }
}

async fn reconcile_ack(
    State(config): State<ServerConfig>,
    Json(input): Json<ReconcileAckRequest>,
) -> (StatusCode, Json<Value>) {
    if input.files_reread.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({
                "status": "error",
                "reason_code": "missing_files_reread"
            })),
        );
    }
    let Some(reservation_id) = input.reservation_id.clone() else {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({
                "status": "error",
                "reason_code": "missing_reservation"
            })),
        );
    };
    let decision = match ReconciliationDecision::from_str(&input.decision) {
        Ok(decision) => decision,
        Err(message) => return error_response(StatusCode::BAD_REQUEST, message),
    };
    let files_reread = match require_files_planned(input.files_reread) {
        Ok(files) => files,
        Err(response) => return response,
    };

    let result = match config.store.lock() {
        Ok(store) => {
            for path in &files_reread {
                match store.active_exact_file_intent_by_reservation(
                    &input.workspace_id,
                    path,
                    &reservation_id,
                ) {
                    Ok(true) => {}
                    Ok(false) => {
                        return (
                            StatusCode::BAD_REQUEST,
                            Json(json!({
                                "status": "error",
                                "reason_code": "missing_reservation"
                            })),
                        );
                    }
                    Err(error) => return status_response(Err(error.to_string())),
                }
            }
            store.acknowledge_human_reconciliation(ReconciliationAckInput {
                agent_id: input.agent_id,
                workspace_id: input.workspace_id,
                reservation_id: Some(reservation_id),
                decision,
                files_reread,
                human_change_summary: input.human_change_summary,
            })
        }
        Err(_) => return status_response(Err("store lock poisoned".to_string())),
    };

    match result {
        Ok(cleared) => (
            StatusCode::OK,
            Json(json!({
                "status": "ok",
                "cleared": cleared
            })),
        ),
        Err(error) => status_response(Err(error.to_string())),
    }
}

async fn reservation_declare(
    State(config): State<ServerConfig>,
    Json(input): Json<Value>,
) -> (StatusCode, Json<Value>) {
    let envelope = match protocol::require_v1_envelope(input) {
        Ok(envelope) => envelope,
        Err(error) => return error.response(),
    };
    if let Err(response) = require_agent_id(&envelope.request.agent.agent_id) {
        return response;
    }

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
            envelope.request.agent.agent_id,
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
    Json(input): Json<Value>,
) -> (StatusCode, Json<Value>) {
    let envelope = match protocol::require_v1_envelope(input) {
        Ok(envelope) => envelope,
        Err(error) => return error.response(),
    };
    if let Err(response) = require_agent_id(&envelope.request.agent.agent_id) {
        return response;
    }

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
        agent_id: envelope.request.agent.agent_id,
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
    Json(input): Json<Value>,
) -> (StatusCode, Json<Value>) {
    let envelope = match protocol::require_v1_envelope(input) {
        Ok(envelope) => envelope,
        Err(error) => return error.response(),
    };
    if let Err(response) = require_agent_id(&envelope.request.agent.agent_id) {
        return response;
    }

    let payload: IntentClaimPayload = match serde_json::from_value(envelope.payload) {
        Ok(payload) => payload,
        Err(_) => return protocol::protocol_mismatch_response(),
    };

    let input = ClaimReservationInput {
        agent_id: envelope.request.agent.agent_id,
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
    Json(input): Json<Value>,
) -> (StatusCode, Json<Value>) {
    let envelope = match protocol::require_v1_envelope(input) {
        Ok(envelope) => envelope,
        Err(error) => return error.response(),
    };
    if let Err(response) = require_agent_id(&envelope.request.agent.agent_id) {
        return response;
    }

    let payload: IntentCancelPayload = match serde_json::from_value(envelope.payload) {
        Ok(payload) => payload,
        Err(_) => return protocol::protocol_mismatch_response(),
    };

    let input = CancelReservationInput {
        agent_id: envelope.request.agent.agent_id,
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

fn require_agent_id(agent_id: &str) -> Result<(), (StatusCode, Json<Value>)> {
    if agent_id.is_empty() {
        return Err(invalid_agent_id_response("agent_id is required"));
    }
    if !agent_id
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
    {
        return Err(invalid_agent_id_response(
            "agent_id contains unsupported characters",
        ));
    }
    Ok(())
}

fn invalid_agent_id_response(message: &str) -> (StatusCode, Json<Value>) {
    (
        StatusCode::BAD_REQUEST,
        Json(json!({
            "status": "error",
            "reason_code": "invalid_agent_id",
            "message": message
        })),
    )
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
            "message": "Agent already holds an active claim for this path."
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
    agent_id: &str,
    workspace_id: &str,
    paths: &[String],
) -> Result<Option<WaitRecord>, StoreError> {
    for path in paths {
        let reservation = if path.ends_with('/') {
            store.active_reservation_for_directory_by_agent(workspace_id, path, agent_id)?
        } else {
            store.active_reservation_for_path_by_agent(workspace_id, path, agent_id)?
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
            "message": "A reservation for this agent must be claimed before acquiring the claim.",
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
            "message": "Cannot release a claim owned by another agent; wait for the claim to release, or coordinate with the claim owner."
        })),
    )
}

fn claim_not_found_response() -> (StatusCode, Json<Value>) {
    (
        StatusCode::NOT_FOUND,
        Json(json!({
            "status": "error",
            "reason_code": "claim_not_found",
            "message": "No active same-agent claim matched the requested path, workspace, and claim type."
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
    Json(input): Json<LeaseAcquireRequest>,
) -> (StatusCode, Json<Value>) {
    if let Err(response) = require_agent_id(&input.agent_id) {
        return response;
    }

    let result = match config.store.lock() {
        Ok(store) => {
            let paths = match input.paths() {
                Ok(paths) => paths,
                Err(response) => return response,
            };
            let agent_id = input.agent_id;
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
                    Some(reservation_id) => store
                        .acquire_claim_for_reservation_with_observation_and_event(
                            reservation_id,
                            &agent_id,
                            &workspace_id,
                            &path,
                            observation,
                        ),
                    None => store.acquire_claim_with_observation_and_event(
                        &agent_id,
                        &workspace_id,
                        &path,
                        observation,
                    ),
                };
                match acquire_result {
                    Ok(()) => Ok(LeaseAcquireOutcome::Acquired),
                    Err(StoreError::ClaimConflict) => {
                        let reservation = if path.ends_with('/') {
                            store.active_reservation_for_directory_by_agent(
                                &workspace_id,
                                &path,
                                &agent_id,
                            )
                        } else {
                            store.active_reservation_for_path_by_agent(
                                &workspace_id,
                                &path,
                                &agent_id,
                            )
                        };
                        match reservation {
                            Ok(Some(reservation)) => {
                                Ok(LeaseAcquireOutcome::Reservation(Box::new(reservation)))
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
                    Some(reservation_id) => store
                        .acquire_claims_for_reservation_with_observations_and_events(
                            reservation_id,
                            &agent_id,
                            &workspace_id,
                            claims,
                        ),
                    None => store.acquire_claims_with_observations_and_events(
                        &agent_id,
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
                        match first_claimable_reservation(&store, &agent_id, &workspace_id, &paths)
                        {
                            Ok(Some(reservation)) => {
                                Ok(LeaseAcquireOutcome::Reservation(Box::new(reservation)))
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
            reservation_claim_required_response(*reservation)
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
    Json(input): Json<LeaseRequest>,
) -> (StatusCode, Json<Value>) {
    if let Err(response) = require_agent_id(&input.agent_id) {
        return response;
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
            input.agent_id,
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
    Json(input): Json<LeaseRequest>,
) -> (StatusCode, Json<Value>) {
    if let Err(response) = require_agent_id(&input.agent_id) {
        return response;
    }

    let result = match config.store.lock() {
        Ok(store) => {
            let claim_result =
                store.release_claim(&input.agent_id, &input.workspace_id, &input.path);
            let fence_result =
                store.release_write_fences(&input.agent_id, &input.workspace_id, &input.path);
            match (claim_result, fence_result) {
                (Ok(()), Ok(_)) => Ok(()),
                (Err(StoreError::ClaimNotFound), Ok(released_fences)) if released_fences > 0 => {
                    Ok(())
                }
                (Err(error), _) => Err(error),
                (_, Err(error)) => Err(error),
            }
        }
        Err(_) => return status_response(Err("store lock poisoned".to_string())),
    };

    match result {
        Ok(()) => status_response(Ok(())),
        Err(StoreError::ClaimOwnerMismatch) => claim_owner_mismatch_response(),
        Err(StoreError::ClaimNotFound) => claim_not_found_response(),
        Err(error) => status_response(Err(error.to_string())),
    }
}

async fn activity_finalize(
    State(config): State<ServerConfig>,
    Json(input): Json<ActivityRequest>,
) -> (StatusCode, Json<Value>) {
    if let Err(response) = require_agent_id(&input.agent_id) {
        return response;
    }

    let result = config
        .store
        .lock()
        .map_err(|_| "store lock poisoned".to_string())
        .and_then(|store| {
            store
                .finalize_session_activity_with_phase(
                    &input.agent_id,
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
        Err(message) => error_response(StatusCode::INTERNAL_SERVER_ERROR, message),
    }
}

async fn context_render(
    State(config): State<ServerConfig>,
    Json(input): Json<ContextRenderRequest>,
) -> (StatusCode, Json<Value>) {
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
    if let Some(agent_id) = input.agent_id.as_deref() {
        if let Err(response) = require_agent_id(agent_id) {
            return response;
        }
    }

    let result = config
        .store
        .lock()
        .map_err(|_| "store lock poisoned".to_string())
        .and_then(|store| {
            let identity_filter = CurrentStateIdentityFilter {
                repo_id: input.repo_id.as_deref().and_then(non_empty_str),
                worktree_id: input.worktree_id.as_deref().and_then(non_empty_str),
                root: input.root.as_deref().and_then(non_empty_str),
                exclude_agent_id: input.agent_id.as_deref().and_then(non_empty_str),
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
            "coordination_mode": config.coordination_mode.as_str(),
            "current": live.summary,
            "items": live.items,
            "prompt_text": prompt_text
        })),
    )
}

async fn outbox_sync(
    State(config): State<ServerConfig>,
    Json(input): Json<OutboxSyncRequest>,
) -> (StatusCode, Json<Value>) {
    if let Err(response) = require_agent_id(&input.agent_id) {
        return response;
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
                        input.agent_id.clone(),
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
        Err(message) => error_response(StatusCode::INTERNAL_SERVER_ERROR, message),
    }
}

async fn notifications_poll(
    State(config): State<ServerConfig>,
    Json(input): Json<NotificationsPollRequest>,
) -> (StatusCode, Json<Value>) {
    if let Err(response) = require_agent_id(&input.agent_id) {
        return response;
    }

    let result = config
        .store
        .lock()
        .map_err(|_| "store lock poisoned".to_string())
        .and_then(|store| {
            store
                .pending_notifications(&input.agent_id, &input.workspace_id)
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
        Err(message) => error_response(StatusCode::INTERNAL_SERVER_ERROR, message),
    }
}

async fn notifications_stream(
    State(config): State<ServerConfig>,
    headers: HeaderMap,
    Query(input): Query<NotificationsPollRequest>,
) -> Response {
    if let Err(response) = require_agent_id(&input.agent_id) {
        return response.into_response();
    }

    let last_seen_sequence = notification_last_event_sequence(&headers).unwrap_or(0);
    if last_seen_sequence > 0 {
        let result = config
            .store
            .lock()
            .map_err(|_| "store lock poisoned".to_string())
            .and_then(|store| {
                store
                    .mark_notifications_delivered_through(
                        &input.agent_id,
                        &input.workspace_id,
                        last_seen_sequence,
                    )
                    .map_err(|error| error.to_string())
            });
        if let Err(message) = result {
            return error_response(StatusCode::INTERNAL_SERVER_ERROR, message).into_response();
        }
    }

    Sse::new(notification_sse_stream(
        config.store.clone(),
        input,
        last_seen_sequence,
    ))
    .keep_alive(KeepAlive::default())
    .into_response()
}

fn notification_last_event_sequence(headers: &HeaderMap) -> Option<u64> {
    headers
        .get("last-event-id")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.trim().parse::<u64>().ok())
}

fn notification_sse_stream(
    store: SharedStore,
    input: NotificationsPollRequest,
    initial_after_sequence: u64,
) -> impl Stream<Item = Result<SseEvent, Infallible>> {
    let pending = Arc::new(Mutex::new(VecDeque::<NotificationRecord>::new()));
    let last_sent_sequence = Arc::new(Mutex::new(initial_after_sequence));
    let interval = tokio::time::interval(Duration::from_secs(1));
    IntervalStream::new(interval).filter_map(move |_| {
        let store = store.clone();
        let input = input.clone();
        let pending = pending.clone();
        let last_sent_sequence = last_sent_sequence.clone();
        let next = {
            let mut queued = pending
                .lock()
                .expect("notification queue lock should not poison");
            if queued.is_empty() {
                let after_sequence = *last_sent_sequence
                    .lock()
                    .expect("notification sequence lock should not poison");
                if let Ok(notifications) = store
                    .lock()
                    .map_err(|_| "store lock poisoned".to_string())
                    .and_then(|store| {
                        store
                            .pending_notifications_after(
                                &input.agent_id,
                                &input.workspace_id,
                                after_sequence,
                            )
                            .map_err(|error| error.to_string())
                    })
                {
                    queued.extend(notifications);
                }
            }
            let notification = queued.pop_front();
            if let Some(notification) = &notification {
                let mut sequence = last_sent_sequence
                    .lock()
                    .expect("notification sequence lock should not poison");
                *sequence = (*sequence).max(notification.sequence);
            }
            notification
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
        .id(notification.sequence.to_string())
        .event(notification.kind.clone())
        .data(
            json!({
                "status": "ok",
                "notification_id": notification.notification_id,
                "sequence": notification.sequence,
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
    Json(input): Json<ResumeNextRequest>,
) -> (StatusCode, Json<Value>) {
    if let Err(response) = require_agent_id(&input.agent_id) {
        return response;
    }

    let result = config
        .store
        .lock()
        .map_err(|_| "store lock poisoned".to_string())
        .and_then(|store| {
            store
                .next_reservation_for_agent(&input.agent_id, &input.workspace_id)
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
        Err(message) => error_response(StatusCode::INTERNAL_SERVER_ERROR, message),
    }
}

fn authorize_with_policy_and_audit(
    store: &SharedStore,
    coordination_mode: CoordinationMode,
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
            let outcome =
                PolicyService::new(store, coordination_mode).authorize_write(input, true)?;
            match outcome.decision.decision {
                stateful_core::DecisionKind::Deny => {
                    let audit = authorization_denied_audit_event(&audit_input, &outcome);
                    store.append(audit).map_err(|error| error.to_string())?;
                }
                stateful_core::DecisionKind::Warn => {
                    let audit = authorization_warned_audit_event(&audit_input, &outcome);
                    store.append(audit).map_err(|error| error.to_string())?;
                }
                _ => {}
            }
            Ok(outcome)
        },
        |error| error.to_string(),
    )
}

fn authorize_heartbeat_event(input: &AuthorizeWriteInput) -> Option<Event> {
    let workspace_id = input.workspace_id.as_ref()?;
    Some(with_request_identity(
        Event::agent_heartbeat(input.agent_id.clone(), workspace_id.clone()),
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
            let outcome =
                PolicyService::new(store, CoordinationMode::Enforcement).claim_intent(input)?;
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
            let outcome = PolicyService::new(store, CoordinationMode::Enforcement)
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
            let outcome =
                PolicyService::new(store, CoordinationMode::Enforcement).cancel_intent(input)?;
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
    status_response(append_event(store, event))
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
    let blocking_agent_id = outcome
        .wait
        .as_ref()
        .and_then(|wait| wait.record.blocking_agent_id.clone())
        .or_else(|| {
            outcome
                .reservation
                .as_ref()
                .and_then(|reservation| reservation.blocking_agent_id.clone())
        });

    with_request_identity(
        Event::reservation_requested(
            input.agent_id.clone(),
            input.workspace_id.clone(),
            outcome.request_id.clone(),
            input.action.clone(),
            input.path.clone(),
            input.purpose.clone(),
            outcome.request_state.clone(),
            wait_id,
            queue_position,
            blocking_agent_id,
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
            reservation.agent_id.clone(),
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
            wait.agent_id.clone(),
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
            input.agent_id.clone(),
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
            "agent_id": wait.record.agent_id,
            "workspace_id": wait.record.workspace_id,
            "relative_path": wait.record.relative_path,
            "action": wait.record.action,
            "status": wait.record.status,
            "purpose": wait.record.purpose,
            "queue_position": wait.queue_position,
            "blocking_agent_id": wait.record.blocking_agent_id,
        });
    }

    event
}

fn authorization_warned_audit_event(
    input: &AuthorizeWriteInput,
    outcome: &AuthorizationOutcome,
) -> Event {
    with_request_identity(
        Event::authorization_warned(
            input.agent_id.clone(),
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
    )
}

fn error_response(status: StatusCode, message: impl Into<String>) -> (StatusCode, Json<Value>) {
    let message = message.into();
    (
        status,
        Json(json!({
            "status": "error",
            "message": message
        })),
    )
}

fn status_response(result: Result<(), String>) -> (StatusCode, Json<Value>) {
    match result {
        Ok(()) => (
            StatusCode::OK,
            Json(json!({
                "status": "ok"
            })),
        ),
        Err(message) => error_response(StatusCode::INTERNAL_SERVER_ERROR, message),
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
        "agent_id": record.agent_id,
        "workspace_id": record.workspace_id,
        "relative_path": record.relative_path,
        "action": record.action,
        "status": record.status,
        "queue_position": queue_position,
        "blocking_agent_id": record.blocking_agent_id,
        "purpose": record.purpose,
    })
}

fn reservation_json(reservation: WaitRecord) -> Value {
    let reservation_id = reservation.wait_id.clone();
    json!({
        "reservation_id": reservation_id,
        "wait_id": reservation.wait_id,
        "agent_id": reservation.agent_id,
        "workspace_id": reservation.workspace_id,
        "relative_path": reservation.relative_path,
        "action": reservation.action,
        "status": reservation.status,
        "reservation_expires_at": reservation.reservation_expires_at,
        "purpose": reservation.purpose,
    })
}

fn unauthorized() -> (StatusCode, Json<Value>) {
    error_response(StatusCode::UNAUTHORIZED, "unauthorized")
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
struct EventsQuery {
    workspace_id: Option<String>,
    since: Option<String>,
    limit: Option<u64>,
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
    agent_id: String,
    workspace_id: String,
    #[serde(flatten)]
    identity: WorkspaceIdentityRequest,
}

enum LeaseAcquireOutcome {
    Acquired,
    Reservation(Box<WaitRecord>),
    Batch(BatchAcquireSuccess),
}

struct BatchAcquireSuccess {
    paths: Vec<String>,
    result: ClaimBatchAcquireResult,
}

#[derive(Debug, Deserialize)]
struct LeaseAcquireRequest {
    agent_id: String,
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
    agent_id: String,
    workspace_id: String,
    path: String,
    #[serde(default)]
    root: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ActivityRequest {
    agent_id: String,
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
    agent_id: String,
    workspace_id: String,
}

#[derive(Debug, Deserialize)]
struct ResumeNextRequest {
    agent_id: String,
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
struct HumanObservePayload {
    path: String,
    kind: String,
    confidence: String,
    source: String,
    summary: String,
}

#[derive(Debug, Deserialize)]
struct HumanSaveCheckRequest {
    workspace_id: String,
    paths: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct ReconcileAckRequest {
    agent_id: String,
    workspace_id: String,
    #[serde(default)]
    reservation_id: Option<String>,
    decision: String,
    files_reread: Vec<String>,
    human_change_summary: String,
}

#[derive(Debug, Deserialize)]
struct ContextRenderRequest {
    mode: Option<String>,
    resource: Option<String>,
    workspace_id: Option<String>,
    #[serde(default)]
    agent_id: Option<String>,
    #[serde(default)]
    repo_id: Option<String>,
    #[serde(default)]
    worktree_id: Option<String>,
    #[serde(default)]
    root: Option<String>,
}

#[derive(Debug, Deserialize)]
struct OutboxSyncRequest {
    outbox_id: String,
    agent_id: String,
    workspace_id: String,
    sequence: u64,
    event_type: String,
    payload: Value,
}
