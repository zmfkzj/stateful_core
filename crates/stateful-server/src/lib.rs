use axum::{
    Json, Router,
    extract::State,
    http::{HeaderMap, StatusCode},
    routing::{get, post},
};
use serde::Deserialize;
use serde_json::{Value, json};
use stateful_core::{
    AuthorizationInput, ContextPackage, Decision, ReconciliationDecision, RenderMode,
    render_prompt_text,
};
use stateful_store::{Event, OutboxEntry, Store, WaitRecord};
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
        Event::session_registered(input.session_id, input.workspace_id),
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
        Event::session_heartbeat(input.session_id, input.workspace_id),
    )
}

async fn authorize(
    State(config): State<ServerConfig>,
    headers: HeaderMap,
    Json(input): Json<AuthorizeRequest>,
) -> (StatusCode, Json<serde_json::Value>) {
    if !has_valid_bearer_token(&headers, &config.bearer_token) {
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({
                "error": "unauthorized"
            })),
        );
    }

    let outcome = match authorize_from_store(&config.store, input, true) {
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
    Json(input): Json<IntentDeclareRequest>,
) -> (StatusCode, Json<Value>) {
    if !has_valid_bearer_token(&headers, &config.bearer_token) {
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({
                "error": "unauthorized"
            })),
        );
    }

    append_event_response(
        &config.store,
        Event::intent_declared(input.session_id, input.workspace_id, input.files_planned),
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
    Json(input): Json<AuthorizeRequest>,
) -> (StatusCode, Json<Value>) {
    if !has_valid_bearer_token(&headers, &config.bearer_token) {
        return unauthorized();
    }

    match authorize_from_store(&config.store, input, false) {
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

#[derive(Debug, Clone)]
struct AuthorizationOutcome {
    decision: Decision,
    wait: Option<WaitQueueInfo>,
    reservation: Option<WaitRecord>,
}

#[derive(Debug, Clone)]
struct WaitQueueInfo {
    record: WaitRecord,
    queue_position: Option<u64>,
}

fn authorize_from_store(
    store: &SharedStore,
    input: AuthorizeRequest,
    allow_queue_side_effects: bool,
) -> Result<AuthorizationOutcome, String> {
    let path = input.path.clone();
    let authorization_input = match input.action.as_str() {
        "write_file" => AuthorizationInput::write_file(&path),
        "delete_file" => AuthorizationInput::delete_file(&path),
        "rename_file" => AuthorizationInput::rename_file(
            input.old_path.as_deref().unwrap_or(path.as_str()),
            input.new_path.as_deref().unwrap_or(path.as_str()),
        ),
        "move_file" => AuthorizationInput::move_file(
            input.old_path.as_deref().unwrap_or(path.as_str()),
            input.new_path.as_deref().unwrap_or(path.as_str()),
        ),
        _ => {
            return Ok(AuthorizationOutcome {
                decision: Decision::deny(
                    "unsupported_action",
                    "Action is not supported by the v1 authorization API.",
                    "Use a supported action such as write_file.",
                ),
                wait: None,
                reservation: None,
            });
        }
    };

    let store = store
        .lock()
        .map_err(|_| "store lock poisoned".to_string())?;
    let mut active_reservation = None;
    if let Some(workspace_id) = &input.workspace_id {
        if let Some(reservation) = store
            .active_reservation(workspace_id, &path)
            .map_err(|error| error.to_string())?
        {
            if reservation.session_id != input.session_id {
                return Ok(AuthorizationOutcome {
                    decision: Decision::deny(
                        "reservation_conflict",
                        "Write target is reserved for the next waiting session.",
                        "Wait for the active reservation to be claimed or expire.",
                    ),
                    wait: None,
                    reservation: Some(reservation),
                });
            }
            active_reservation = Some(reservation);
        }

        if let Some(owner) = store
            .active_lease_owner(workspace_id, &path)
            .map_err(|error| error.to_string())?
            && owner != input.session_id
        {
            let wait = if allow_queue_side_effects && input.queue_on_conflict {
                let waiter = store
                    .enqueue_waiter(
                        &input.session_id,
                        workspace_id,
                        &path,
                        &input.action,
                        Some(&owner),
                    )
                    .map_err(|error| error.to_string())?;
                let queue_position = store
                    .queue_position(&waiter.wait_id)
                    .map_err(|error| error.to_string())?;
                Some(WaitQueueInfo {
                    record: waiter,
                    queue_position,
                })
            } else {
                None
            };

            return Ok(AuthorizationOutcome {
                decision: Decision::deny(
                    "active_lease_conflict",
                    "Write target is covered by another active session lease.",
                    "Refresh current state, coordinate with the lease owner, or wait for the lease to release.",
                ),
                wait,
                reservation: None,
            });
        }
    }

    let policy_state = store
        .policy_state_for_session(&input.session_id)
        .map_err(|error| error.to_string())?;

    let decision = stateful_core::authorize_action(&policy_state, authorization_input);
    let mut reservation = active_reservation;
    if matches!(decision.decision, stateful_core::DecisionKind::Allow)
        && let Some(active) = &reservation
    {
        store
            .claim_reservation(&active.wait_id, &input.session_id)
            .map_err(|error| error.to_string())?;
        if let Some(workspace_id) = &input.workspace_id {
            store
                .acquire_lease(&input.session_id, workspace_id, &path)
                .map_err(|error| error.to_string())?;
        }
        if let Some(claimed) = &mut reservation {
            claimed.status = "claimed".to_string();
        }
    }

    Ok(AuthorizationOutcome {
        decision,
        wait: None,
        reservation,
    })
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
    session_id: String,
    workspace_id: String,
    files_planned: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct SessionRequest {
    session_id: String,
    workspace_id: String,
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
