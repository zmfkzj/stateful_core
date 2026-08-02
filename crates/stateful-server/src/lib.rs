mod protocol;

use axum::{
    Json, Router,
    extract::{Path, Query, Request, State},
    http::{HeaderMap, StatusCode},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::{Value, json};
use stateful_core::RequestEnvelope;
use stateful_store::{CommandContext, Store, StoreError, StoreResult};
use std::{
    net::SocketAddr,
    sync::{Arc, Mutex},
    time::Duration,
};
use time::{OffsetDateTime, format_description::well_known::Rfc3339};

pub const CRATE_NAME: &str = "stateful-server";
const DEFAULT_MAINTENANCE_INTERVAL: Duration = Duration::from_secs(1);

type SharedStore = Arc<Mutex<Store>>;

#[derive(Clone)]
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

pub fn build_router(config: ServerConfig) -> Router {
    let protected = Router::new()
        .route("/v2/tasks/start", post(task_start))
        .route("/v2/tasks/heartbeat", post(task_heartbeat))
        .route("/v2/tasks/finalize", post(task_finalize))
        .route("/v2/tasks/cancel", post(task_cancel))
        .route("/v2/reads/start", post(read_start))
        .route("/v2/reads/complete", post(read_complete))
        .route("/v2/writes/prepare", post(write_prepare))
        .route("/v2/writes/complete", post(write_complete))
        .route("/v2/commits/prepare", post(write_prepare))
        .route("/v2/commits/complete", post(write_complete))
        .route("/v2/lease-requests/{batch_id}", get(lease_request_status))
        .route("/v2/leases/activate", post(lease_activate))
        .route("/v2/leases/release", post(lease_release))
        .route("/v2/status", get(status))
        .route("/v2/audit", get(audit))
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
    if !addr.ip().is_loopback() {
        anyhow::bail!("stateful server only accepts loopback addresses");
    }
    let listener = tokio::net::TcpListener::bind(addr).await?;
    serve_listener(listener, config).await
}

pub async fn serve_listener(
    listener: tokio::net::TcpListener,
    config: ServerConfig,
) -> anyhow::Result<()> {
    if !listener.local_addr()?.ip().is_loopback() {
        anyhow::bail!("stateful server only accepts loopback listeners");
    }
    let maintenance = run_maintenance_loop(config.store.clone(), config.maintenance_interval);
    tokio::select! {
        result = axum::serve(listener, build_router(config)) => result?,
        result = maintenance => result?,
    }
    Ok(())
}

async fn run_maintenance_loop(store: SharedStore, interval: Duration) -> anyhow::Result<()> {
    let mut ticker = tokio::time::interval(interval);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        ticker.tick().await;
        let now = match OffsetDateTime::now_utc().format(&Rfc3339) {
            Ok(now) => now,
            Err(error) => {
                eprintln!("stateful maintenance timestamp failed: {error}");
                continue;
            }
        };
        let mut store = store
            .lock()
            .map_err(|_| anyhow::anyhow!("state store lock is poisoned"))?;
        if let Err(error) = store.maintain(&now) {
            eprintln!("stateful maintenance failed: {error}");
        }
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
        return error_response(
            StatusCode::UNAUTHORIZED,
            "unauthorized",
            "valid bearer token required",
            None,
        );
    }
    next.run(request).await
}

async fn task_start(State(config): State<ServerConfig>, Json(body): Json<Value>) -> Response {
    execute_command(&config, body, Store::task_start)
}

async fn task_heartbeat(State(config): State<ServerConfig>, Json(body): Json<Value>) -> Response {
    execute_command(&config, body, Store::task_heartbeat)
}

async fn task_finalize(State(config): State<ServerConfig>, Json(body): Json<Value>) -> Response {
    execute_command(&config, body, Store::task_finalize)
}

async fn task_cancel(State(config): State<ServerConfig>, Json(body): Json<Value>) -> Response {
    execute_command(&config, body, Store::task_cancel)
}

async fn read_start(State(config): State<ServerConfig>, Json(body): Json<Value>) -> Response {
    execute_command(&config, body, Store::read_start)
}

async fn read_complete(State(config): State<ServerConfig>, Json(body): Json<Value>) -> Response {
    execute_command(&config, body, Store::read_complete)
}

async fn write_prepare(State(config): State<ServerConfig>, Json(body): Json<Value>) -> Response {
    execute_command(&config, body, Store::write_prepare)
}

async fn write_complete(State(config): State<ServerConfig>, Json(body): Json<Value>) -> Response {
    execute_command(&config, body, Store::write_complete)
}

async fn lease_activate(State(config): State<ServerConfig>, Json(body): Json<Value>) -> Response {
    execute_command(&config, body, Store::lease_activate)
}

async fn lease_release(State(config): State<ServerConfig>, Json(body): Json<Value>) -> Response {
    execute_command(&config, body, Store::lease_release)
}

#[derive(Debug, Deserialize)]
struct LeaseRequestQuery {
    workspace_id: String,
    task_id: String,
    now: String,
}

async fn lease_request_status(
    State(config): State<ServerConfig>,
    Path(batch_id): Path<String>,
    Query(query): Query<LeaseRequestQuery>,
) -> Response {
    query_store(&config, |store| {
        store.lease_request_status(&query.workspace_id, &query.task_id, &batch_id, &query.now)
    })
}

async fn status(State(config): State<ServerConfig>) -> Response {
    query_store(&config, |store| store.status())
}

#[derive(Debug, Deserialize)]
struct AuditQuery {
    #[serde(default = "default_audit_limit")]
    limit: usize,
}

fn default_audit_limit() -> usize {
    100
}

async fn audit(State(config): State<ServerConfig>, Query(query): Query<AuditQuery>) -> Response {
    query_store(&config, |store| store.audit_events(query.limit))
}

fn execute_command<P, R>(
    config: &ServerConfig,
    body: Value,
    operation: fn(&mut Store, &CommandContext, &P) -> StoreResult<R>,
) -> Response
where
    P: DeserializeOwned,
    R: Serialize,
{
    let (request, payload) = match protocol::parse_command::<P>(body) {
        Ok(parsed) => parsed,
        Err(error) => return error.response(),
    };
    let context = command_context(&request);
    let mut store = match config.store.lock() {
        Ok(store) => store,
        Err(_) => {
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "store_unavailable",
                "state store lock is poisoned",
                Some(&request.request_id),
            );
        }
    };
    match operation(&mut store, &context, &payload) {
        Ok(result) => success_response(Some(&request.request_id), result),
        Err(error) => store_error_response(error, Some(&request.request_id)),
    }
}

fn query_store<R>(
    config: &ServerConfig,
    operation: impl FnOnce(&Store) -> StoreResult<R>,
) -> Response
where
    R: Serialize,
{
    let store = match config.store.lock() {
        Ok(store) => store,
        Err(_) => {
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "store_unavailable",
                "state store lock is poisoned",
                None,
            );
        }
    };
    match operation(&store) {
        Ok(result) => success_response(None, result),
        Err(error) => store_error_response(error, None),
    }
}

fn command_context(request: &RequestEnvelope) -> CommandContext {
    CommandContext {
        request_id: request.request_id.clone(),
        task_id: request.task_id.clone(),
        agent_id: request.agent.agent_id.clone(),
        workspace_id: request.workspace.workspace_id.clone(),
        observed_at: request.observed_at.clone(),
    }
}

fn success_response(request_id: Option<&str>, payload: impl Serialize) -> Response {
    let payload = match serde_json::to_value(payload) {
        Ok(payload) => payload,
        Err(error) => {
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "serialization_failed",
                &error.to_string(),
                request_id,
            );
        }
    };
    (
        StatusCode::OK,
        Json(json!({
            "protocol_version": "stateful.v2",
            "contract_revision": "lease-1",
            "request_id": request_id,
            "payload": payload,
        })),
    )
        .into_response()
}

fn store_error_response(error: StoreError, request_id: Option<&str>) -> Response {
    let (status, reason_code) = match &error {
        StoreError::InvalidTimestamp(_) | StoreError::InvalidInput(_) => {
            (StatusCode::BAD_REQUEST, "invalid_input")
        }
        StoreError::IdempotencyMismatch => (StatusCode::CONFLICT, "idempotency_mismatch"),
        StoreError::NotFound(_) => (StatusCode::NOT_FOUND, "not_found"),
        StoreError::Ownership(_) => (StatusCode::FORBIDDEN, "ownership_violation"),
        StoreError::InvalidState(_) => (StatusCode::CONFLICT, "invalid_state"),
        StoreError::Io(_)
        | StoreError::Sqlite(_)
        | StoreError::Json(_)
        | StoreError::Corrupt(_) => (StatusCode::INTERNAL_SERVER_ERROR, "store_error"),
    };
    error_response(status, reason_code, &error.to_string(), request_id)
}

fn error_response(
    status: StatusCode,
    reason_code: &str,
    message: &str,
    request_id: Option<&str>,
) -> Response {
    (
        status,
        Json(json!({
            "protocol_version": "stateful.v2",
            "contract_revision": "lease-1",
            "decision": "error",
            "reason_code": reason_code,
            "request_id": request_id,
            "message": message,
        })),
    )
        .into_response()
}

fn has_valid_bearer_token(headers: &HeaderMap, expected: &str) -> bool {
    headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        == Some(expected)
}
