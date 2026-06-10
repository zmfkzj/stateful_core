mod policy_service;
mod protocol;

use axum::{
    Json, Router,
    extract::{Query, State},
    http::{HeaderMap, StatusCode},
    routing::{get, post},
};
use policy_service::{
    AuthorizationOutcome, AuthorizeWriteInput, CancelIntentInput, CancelIntentOutcome,
    ClaimIntentInput, ClaimIntentOutcome, PolicyService, RequestIntentInput, RequestIntentOutcome,
    WaitQueueInfo,
};
use serde::Deserialize;
use serde_json::{Value, json};
use stateful_core::{ContextPackage, ReconciliationDecision, RenderMode, render_prompt_text};
use stateful_store::{
    CurrentStateIdentityFilter, Event, OutboxEntry, Store, StoreError, WaitRecord,
};
use std::{
    net::SocketAddr,
    sync::{Arc, Mutex},
};

pub const CRATE_NAME: &str = "stateful-server";
const RUNTIME_CAPABILITIES: &[&str] = &["authorize.write_directory"];

#[derive(Debug, Clone)]
pub struct ServerConfig {
    bearer_token: String,
    store: SharedStore,
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
        }
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
        .route("/v1/intent/declare", post(intent_declare))
        .route("/v1/intent/request", post(intent_request))
        .route("/v1/intent/claim", post(intent_claim))
        .route("/v1/intent/cancel", post(intent_cancel))
        .route("/v1/lease/acquire", post(lease_acquire))
        .route("/v1/lease/release", post(lease_release))
        .route("/v1/activity/observe", post(activity_observe))
        .route("/v1/activity/finalize", post(activity_finalize))
        .route("/v1/authorize", post(authorize))
        .route("/v1/conflicts/check", post(conflicts_check))
        .route("/v1/context/render", post(context_render))
        .route("/v1/reconcile/ack", post(reconcile_ack))
        .route("/v1/notifications/poll", post(notifications_poll))
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
    axum::serve(listener, build_router(config)).await?;
    Ok(())
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
        workspace_id: Some(workspace.workspace_id),
        repo_id: non_empty_identity(workspace.repo_id),
        worktree_id: non_empty_identity(workspace.worktree_id),
        root: non_empty_identity(workspace.root),
        branch: non_empty_identity(workspace.branch),
        source_kind: Some(source.kind),
        source_tool_name: source.tool_name,
        queue_on_conflict: payload.queue_on_conflict,
        queue_purpose,
        action: payload.action,
        old_path: payload.old_path,
        new_path: payload.new_path,
        path: payload.path,
    };

    let outcome = match authorize_with_policy(&config.store, input, true) {
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

async fn intent_declare(
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
    let payload: IntentDeclarePayload = match serde_json::from_value(envelope.payload) {
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

    append_event_response(
        &config.store,
        with_request_identity(
            Event::intent_declared(
                envelope.request.session.session_id,
                envelope.request.workspace.workspace_id,
                purpose,
                files_planned,
            ),
            identity,
        ),
    )
}

async fn intent_request(
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
    let payload: IntentRequestPayload = match serde_json::from_value(envelope.payload) {
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

    let input = RequestIntentInput {
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
        Err(message) => (
            StatusCode::CONFLICT,
            Json(json!({
                "status": "error",
                "reason_code": "request_failed",
                "message": message
            })),
        ),
    }
}

async fn intent_claim(
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

    let input = ClaimIntentInput {
        session_id: envelope.request.session.session_id,
        workspace_id: envelope.request.workspace.workspace_id,
        wait_id: payload.wait_id,
        repo_id: non_empty_identity(envelope.request.workspace.repo_id),
        worktree_id: non_empty_identity(envelope.request.workspace.worktree_id),
        root: non_empty_identity(envelope.request.workspace.root),
        branch: non_empty_identity(envelope.request.workspace.branch),
    };

    match claim_intent_with_policy(&config.store, input) {
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

async fn intent_cancel(
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

    let input = CancelIntentInput {
        session_id: envelope.request.session.session_id,
        workspace_id: envelope.request.workspace.workspace_id,
        request_id: payload.request_id,
    };

    match cancel_intent_with_policy(&config.store, input) {
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
            "message": "Intent purpose is required and must be inferred from the user or agent instruction when it is not explicit."
        })),
    )
}

fn missing_intent_response() -> (StatusCode, Json<Value>) {
    (
        StatusCode::BAD_REQUEST,
        Json(json!({
            "status": "error",
            "reason_code": "missing_intent",
            "message": "Lease acquisition requires an active intent covering the requested path."
        })),
    )
}

fn lease_conflict_response() -> (StatusCode, Json<Value>) {
    (
        StatusCode::CONFLICT,
        Json(json!({
            "status": "error",
            "reason_code": "lease_conflict",
            "message": "Requested lease conflicts with an active lease or reserved request."
        })),
    )
}

fn missing_scope_response() -> (StatusCode, Json<Value>) {
    (
        StatusCode::BAD_REQUEST,
        Json(json!({
            "status": "error",
            "reason_code": "missing_scope",
            "message": "Intent scope paths must be non-empty after normalization."
        })),
    )
}

fn require_files_planned(
    files_planned: Vec<String>,
) -> Result<Vec<String>, (StatusCode, Json<Value>)> {
    if files_planned.is_empty()
        || files_planned
            .iter()
            .any(|path| normalized_scope_is_empty(path))
    {
        return Err(missing_scope_response());
    }
    Ok(files_planned)
}

fn normalized_scope_is_empty(path: &str) -> bool {
    let normalized = path.trim().replace(char::from(92), "/");
    let mut segments = Vec::new();
    for segment in normalized.split(char::from(47)) {
        if segment.is_empty() || segment == "." {
            continue;
        }
        if segment == ".." {
            segments.pop();
        } else {
            segments.push(segment);
        }
    }
    segments.is_empty()
}

fn require_scope_path(path: String) -> Result<String, (StatusCode, Json<Value>)> {
    if normalized_scope_is_empty(&path) {
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
    Json(input): Json<LeaseRequest>,
) -> (StatusCode, Json<Value>) {
    if !has_valid_bearer_token(&headers, &config.bearer_token) {
        return unauthorized();
    }

    let result = match config.store.lock() {
        Ok(store) => store.acquire_lease(input.session_id, input.workspace_id, input.path),
        Err(_) => return status_response(Err("store lock poisoned".to_string())),
    };

    match result {
        Ok(()) => status_response(Ok(())),
        Err(StoreError::MissingPurpose) => missing_purpose_response(),
        Err(StoreError::MissingIntent) => missing_intent_response(),
        Err(StoreError::LeaseConflict) => lease_conflict_response(),
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

    let result = config
        .store
        .lock()
        .map_err(|_| "store lock poisoned".to_string())
        .and_then(|store| {
            store
                .release_lease(input.session_id, input.workspace_id, input.path)
                .map_err(|error| error.to_string())
        });

    status_response(result)
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
                .append_activity(&input.session_id, &input.workspace_id)
                .map_err(|error| error.to_string())?;
            let released = store
                .release_session_leases(&input.session_id, &input.workspace_id)
                .map_err(|error| error.to_string())?;
            Ok(released)
        });

    match result {
        Ok(released_leases) => (
            StatusCode::OK,
            Json(json!({
                "status": "ok",
                "released_leases": released_leases
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
            };
            store
                .live_current_state_for_workspace_identity(
                    workspace_id,
                    identity_filter,
                    input.resource.as_deref(),
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
        workspace_id: input.workspace_id,
        repo_id: None,
        worktree_id: None,
        root: None,
        branch: None,
        source_kind: None,
        source_tool_name: None,
        queue_on_conflict: input.queue_on_conflict,
        queue_purpose: None,
        action: input.action,
        old_path: input.old_path,
        new_path: input.new_path,
        path: input.path,
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
                .append_outbox(OutboxEntry::new(
                    input.outbox_id,
                    input.session_id,
                    input.sequence,
                ))
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
                .pending_notifications(&input.session_id)
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
                "reservation": reservation,
                "required_next_action": "Reread the target, then call state.intent.claim for the reservation before writing."
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

fn claim_intent_with_policy(
    store: &SharedStore,
    input: ClaimIntentInput,
) -> Result<ClaimIntentOutcome, String> {
    let store = store
        .lock()
        .map_err(|_| "store lock poisoned".to_string())?;
    PolicyService::new(&store).claim_intent(input)
}

fn request_intent_with_policy(
    store: &SharedStore,
    input: RequestIntentInput,
) -> Result<RequestIntentOutcome, String> {
    let store = store
        .lock()
        .map_err(|_| "store lock poisoned".to_string())?;
    PolicyService::new(&store).request_intent(input)
}

fn cancel_intent_with_policy(
    store: &SharedStore,
    input: CancelIntentInput,
) -> Result<CancelIntentOutcome, String> {
    let store = store
        .lock()
        .map_err(|_| "store lock poisoned".to_string())?;
    PolicyService::new(&store).cancel_intent(input)
}

fn append_event_response(store: &SharedStore, event: Event) -> (StatusCode, Json<Value>) {
    let result = store
        .lock()
        .map_err(|_| "store lock poisoned".to_string())
        .and_then(|store| store.append(event).map_err(|error| error.to_string()));

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

fn append_activity_response(
    store: &SharedStore,
    input: ActivityRequest,
) -> (StatusCode, Json<Value>) {
    let result = store
        .lock()
        .map_err(|_| "store lock poisoned".to_string())
        .and_then(|store| {
            store
                .append_activity(input.session_id, input.workspace_id)
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
        value["wait"] = json!({
            "wait_id": wait.record.wait_id,
            "session_id": wait.record.session_id,
            "workspace_id": wait.record.workspace_id,
            "relative_path": wait.record.relative_path,
            "action": wait.record.action,
            "status": wait.record.status,
            "queue_position": wait.queue_position,
            "blocking_session_id": wait.record.blocking_session_id,
        });
    }

    if let Some(reservation) = outcome.reservation {
        value["reservation"] = json!({
            "wait_id": reservation.wait_id,
            "session_id": reservation.session_id,
            "workspace_id": reservation.workspace_id,
            "relative_path": reservation.relative_path,
            "action": reservation.action,
            "status": reservation.status,
            "reservation_expires_at": reservation.reservation_expires_at,
        });
    }

    value
}

fn claim_intent_json(outcome: ClaimIntentOutcome) -> Value {
    let reservation = outcome.reservation;
    json!({
        "status": "ok",
        "reservation": {
            "wait_id": reservation.wait_id,
            "session_id": reservation.session_id,
            "workspace_id": reservation.workspace_id,
            "relative_path": reservation.relative_path,
            "action": reservation.action,
            "status": reservation.status,
            "reservation_expires_at": reservation.reservation_expires_at,
        }
    })
}

fn request_intent_json(outcome: RequestIntentOutcome) -> Value {
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

fn cancel_intent_json(outcome: CancelIntentOutcome) -> Value {
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
    json!({
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
    json!({
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
struct IntentDeclarePayload {
    purpose: String,
    files_planned: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct IntentRequestPayload {
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

#[derive(Debug, Deserialize)]
struct LeaseRequest {
    session_id: String,
    workspace_id: String,
    path: String,
}

#[derive(Debug, Deserialize)]
struct ActivityRequest {
    session_id: String,
    workspace_id: String,
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

#[derive(Debug, Deserialize)]
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
    action: String,
    #[serde(default)]
    old_path: Option<String>,
    #[serde(default)]
    new_path: Option<String>,
    path: String,
}

#[derive(Debug, Deserialize)]
struct AuthorizeRequest {
    session_id: String,
    #[serde(default)]
    workspace_id: Option<String>,
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
