mod policy_service;
pub mod protocol;

use crate::{
    policy_service::{PolicyService, WriteAuthorizationRequest, policy_outcome_json},
    protocol::{ProtocolRequest, validate_protocol, validate_protocol_body},
};
use axum::{
    Json, Router,
    body::Bytes,
    extract::State,
    http::{HeaderMap, StatusCode},
    routing::{get, post},
};
use serde::Deserialize;
use serde_json::{Value, json};
use stateful_core::{ContextPackage, ReconciliationDecision, RenderMode, render_prompt_text};
use stateful_store::{Event, OutboxEntry, Store};
use stateful_validation::{ValidationResult, ValidationStatus, run_validation_profile};
use std::{
    net::SocketAddr,
    path::PathBuf,
    sync::{Arc, Mutex},
};

pub const CRATE_NAME: &str = "stateful-server";

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
        .route("/v1/lease/acquire", post(lease_acquire))
        .route("/v1/lease/release", post(lease_release))
        .route("/v1/activity/observe", post(activity_observe))
        .route("/v1/activity/finalize", post(activity_finalize))
        .route("/v1/authorize", post(authorize))
        .route("/v1/conflicts/check", post(conflicts_check))
        .route("/v1/context/render", post(context_render))
        .route("/v1/reconcile/ack", post(reconcile_ack))
        .route("/v1/validation/run", post(validation_run))
        .route("/v1/notifications/poll", post(notifications_poll))
        .route("/v1/resume/next", post(resume_next))
        .route("/v1/outbox/sync", post(outbox_sync))
        .with_state(config)
}

pub async fn serve_addr(addr: SocketAddr, config: ServerConfig) -> anyhow::Result<()> {
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, build_router(config)).await?;
    Ok(())
}

async fn health() -> StatusCode {
    StatusCode::OK
}

async fn current(
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
        .and_then(|store| store.current_summary().map_err(|error| error.to_string()));

    match result {
        Ok(summary) => (
            StatusCode::OK,
            Json(json!({
                "status": "ok",
                "current": summary
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
            "protocol_version": "stateful.v1"
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
    body: Bytes,
) -> (StatusCode, Json<Value>) {
    if !has_valid_bearer_token(&headers, &config.bearer_token) {
        return unauthorized();
    }

    let input = match validate_protocol_body::<AuthorizePayload>(&body) {
        Ok(input) => input,
        Err(response) => return response,
    };

    let result = config
        .store
        .lock()
        .map_err(|_| "store lock poisoned".to_string())
        .and_then(|store| {
            PolicyService::new(&store).authorize_write(WriteAuthorizationRequest {
                session_id: input.session.session_id,
                workspace_id: Some(input.workspace.workspace_id),
                action: input.payload.action,
                path: input.payload.path,
                old_path: input.payload.old_path,
                new_path: input.payload.new_path,
                queue_on_conflict: input.payload.queue_on_conflict,
                allow_queue_side_effects: true,
            })
        });

    match result {
        Ok(outcome) => (StatusCode::OK, Json(policy_outcome_json(outcome))),
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

async fn intent_declare(
    State(config): State<ServerConfig>,
    headers: HeaderMap,
    Json(input): Json<ProtocolRequest<IntentDeclareRequest>>,
) -> (StatusCode, Json<Value>) {
    if !has_valid_bearer_token(&headers, &config.bearer_token) {
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({
                "error": "unauthorized"
            })),
        );
    }

    let input = match validate_protocol(input) {
        Ok(input) => input,
        Err(response) => return response,
    };
    let identity = WorkspaceIdentityRequest {
        repo_id: Some(input.workspace.repo_id),
        worktree_id: Some(input.workspace.worktree_id),
        root: Some(input.workspace.root),
        branch: Some(input.workspace.branch),
    };

    append_event_response(
        &config.store,
        with_request_identity(
            Event::intent_declared(
                input.session.session_id,
                input.workspace.workspace_id,
                input.payload.files_planned,
            ),
            identity,
        ),
    )
}

async fn lease_acquire(
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
                .acquire_lease(input.session_id, input.workspace_id, input.path)
                .map_err(|error| error.to_string())
        });

    status_response(result)
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
    let _resource = input.resource;
    let package = ContextPackage::empty();
    let prompt_text = render_prompt_text(&package, mode);

    (
        StatusCode::OK,
        Json(json!({
            "status": "ok",
            "mode": match mode {
                RenderMode::Brief => "brief",
                RenderMode::Detailed => "detailed",
            },
            "prompt_text": prompt_text
        })),
    )
}

async fn conflicts_check(
    State(config): State<ServerConfig>,
    headers: HeaderMap,
    body: Bytes,
) -> (StatusCode, Json<Value>) {
    if !has_valid_bearer_token(&headers, &config.bearer_token) {
        return unauthorized();
    }

    let input = match validate_protocol_body::<AuthorizePayload>(&body) {
        Ok(input) => input,
        Err(response) => return response,
    };

    let result = config
        .store
        .lock()
        .map_err(|_| "store lock poisoned".to_string())
        .and_then(|store| {
            PolicyService::new(&store).authorize_write(WriteAuthorizationRequest {
                session_id: input.session.session_id,
                workspace_id: Some(input.workspace.workspace_id),
                action: input.payload.action,
                path: input.payload.path,
                old_path: input.payload.old_path,
                new_path: input.payload.new_path,
                queue_on_conflict: input.payload.queue_on_conflict,
                allow_queue_side_effects: false,
            })
        });

    match result {
        Ok(outcome) => (StatusCode::OK, Json(policy_outcome_json(outcome))),
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
                "required_next_action": "Claim the reservation by retrying the write after rereading the file."
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

async fn validation_run(
    State(config): State<ServerConfig>,
    headers: HeaderMap,
    Json(input): Json<ValidationRunRequest>,
) -> (StatusCode, Json<Value>) {
    if !has_valid_bearer_token(&headers, &config.bearer_token) {
        return unauthorized();
    }

    match run_validation_profile(&input.repo_root, &input.profile) {
        Ok(result) => {
            let record_result =
                record_validation_result(&config.store, &input.workspace_id, &result);
            if let Err(message) = record_result {
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(json!({
                        "workspace_id": input.workspace_id,
                        "profile_id": result.profile_id,
                        "status": "error",
                        "exit_code": result.exit_code,
                        "message": message
                    })),
                );
            }

            (
                StatusCode::OK,
                Json(validation_result_json(result, input.workspace_id)),
            )
        }
        Err(message) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({
                "workspace_id": input.workspace_id,
                "profile_id": input.profile,
                "status": "error",
                "exit_code": null,
                "message": message.to_string()
            })),
        ),
    }
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

fn unauthorized() -> (StatusCode, Json<Value>) {
    (
        StatusCode::UNAUTHORIZED,
        Json(json!({
            "error": "unauthorized"
        })),
    )
}

fn validation_result_json(result: ValidationResult, workspace_id: String) -> Value {
    json!({
        "workspace_id": workspace_id,
        "profile_id": result.profile_id,
        "status": validation_status_str(result.status),
        "exit_code": result.exit_code,
        "message": result.message,
    })
}

fn record_validation_result(
    store: &SharedStore,
    workspace_id: &str,
    result: &ValidationResult,
) -> Result<(), String> {
    store
        .lock()
        .map_err(|_| "store lock poisoned".to_string())?
        .append_validation_result(
            workspace_id,
            &result.profile_id,
            validation_status_str(result.status),
        )
        .map_err(|error| error.to_string())
}

fn validation_status_str(status: ValidationStatus) -> &'static str {
    match status {
        ValidationStatus::Passed => "passed",
        ValidationStatus::Failed => "failed",
        ValidationStatus::FailedPolicy => "failed_policy",
        ValidationStatus::Timeout => "timeout",
        ValidationStatus::Error => "error",
    }
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
struct IntentDeclareRequest {
    files_planned: Vec<String>,
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

#[derive(Debug, Deserialize)]
struct ValidationRunRequest {
    workspace_id: String,
    repo_root: PathBuf,
    profile: String,
}
