mod commands;
mod protocol;
mod routes_v2;

use axum::{Router, extract::{Request, State}, http::{HeaderMap, StatusCode}, middleware::{self, Next}, response::{IntoResponse, Response}, routing::get};
use stateful_core::V2Error;
use stateful_store::Store;
use std::{future::Future, net::SocketAddr, str::FromStr, sync::{Arc, Mutex}, time::Duration};

pub const CRATE_NAME: &str = "stateful-server";
pub(crate) const RUNTIME_CAPABILITIES: &[&str] = &[
    "presence",
    "handoff",
    "exact_read",
    "context_cursor",
    "write_intent",
    "enforcement_opt_in",
];
const DEFAULT_MAINTENANCE_INTERVAL: Duration = Duration::from_secs(30);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CoordinationMode {
    #[default]
    Awareness,
    Enforcement,
}

impl CoordinationMode {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Awareness => "awareness",
            Self::Enforcement => "enforcement",
        }
    }
}

impl FromStr for CoordinationMode {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "awareness" => Ok(Self::Awareness),
            "enforcement" => Ok(Self::Enforcement),
            _ => Err("coordination mode must be awareness or enforcement".into()),
        }
    }
}

#[derive(Clone)]
pub struct ServerConfig {
    pub(crate) bearer_token: String,
    pub(crate) store: SharedStore,
    pub(crate) coordination_mode: CoordinationMode,
    maintenance_interval: Duration,
    ready: bool,
}

impl ServerConfig {
    pub fn new(bearer_token: impl Into<String>) -> Self {
        Self::with_store(
            bearer_token,
            Store::open_in_memory().expect("server in-memory store should open"),
        )
    }

    pub fn with_store(bearer_token: impl Into<String>, mut store: Store) -> Self {
        let ready = store.has_table("journal_events").unwrap_or(false)
            && store.has_table("workspace_version").unwrap_or(false)
            && store.rebuild_projections().is_ok();
        Self {
            bearer_token: bearer_token.into(),
            store: Arc::new(Mutex::new(store)),
            coordination_mode: CoordinationMode::Awareness,
            maintenance_interval: DEFAULT_MAINTENANCE_INTERVAL,
            ready,
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

    pub(crate) const fn is_ready(&self) -> bool {
        self.ready
    }
}

pub(crate) type SharedStore = Arc<Mutex<Store>>;

pub fn build_router(config: ServerConfig) -> Router {
    let protected = routes_v2::router()
        .route_layer(middleware::from_fn_with_state(config.clone(), require_bearer));
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
    serve_listener_until(listener, config, std::future::pending()).await
}

pub async fn serve_listener_until(
    listener: tokio::net::TcpListener,
    config: ServerConfig,
    shutdown: impl Future<Output = ()> + Send + 'static,
) -> anyhow::Result<()> {
    let maintenance = run_maintenance_loop(config.store.clone(), config.maintenance_interval);
    tokio::select! {
        result = axum::serve(listener, build_router(config)).with_graceful_shutdown(shutdown) => result?,
        () = maintenance => {},
    }
    Ok(())
}

async fn run_maintenance_loop(store: SharedStore, interval: Duration) {
    let mut ticker = tokio::time::interval(interval);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        ticker.tick().await;
        if let Ok(mut store) = store.lock() {
            let _ = store.run_maintenance();
        }
    }
}

async fn health(State(config): State<ServerConfig>) -> Response {
    if config.is_ready() {
        StatusCode::OK.into_response()
    } else {
        StatusCode::SERVICE_UNAVAILABLE.into_response()
    }
}

async fn require_bearer(
    State(config): State<ServerConfig>,
    request: Request,
    next: Next,
) -> Response {
    if !has_valid_bearer_token(request.headers(), &config.bearer_token) {
        return protocol::error_response(
            StatusCode::UNAUTHORIZED,
            None,
            V2Error::new("unauthorized", "Bearer authentication is required.")
                .with_required_next_action("Send a valid Bearer token."),
        );
    }
    next.run(request).await
}

fn has_valid_bearer_token(headers: &HeaderMap, expected_token: &str) -> bool {
    headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|header| header.to_str().ok())
        .is_some_and(|value| value == format!("Bearer {expected_token}"))
}
