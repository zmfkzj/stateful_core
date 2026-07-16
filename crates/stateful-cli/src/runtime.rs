use std::{
    fs,
    io::{Read, Write},
    net::{IpAddr, TcpStream},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

#[cfg(unix)]
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};

use crate::global_paths::GlobalPaths;
use crate::repo_registry::RepoIdentity;
use serde::{Deserialize, Serialize};
use stateful_core::{
    ActorType, AgentIdentity, Decision, ProtocolVersion, QueryEnvelope, RequestEnvelope,
    ReservationScope, SourceKind, SourceRef, V2ErrorEnvelope, WorkspaceIdentity,
    normalize_relative_path,
};

const REQUIRED_RUNTIME_CAPABILITIES: &[&str] = &["presence"];
static SECRET_JSON_TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReservationDeclareArgs {
    pub request_id: uuid::Uuid,
    pub agent_id: String,
    pub workspace_id: String,
    pub purpose: String,
    pub files_planned: Vec<String>,
    pub identity: Option<RepoIdentity>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReservationClaimArgs {
    pub request_id: uuid::Uuid,
    pub agent_id: String,
    pub workspace_id: String,
    pub wait_id: String,
    pub relative_path: String,
    pub reservation_id: Option<String>,
    pub identity: Option<RepoIdentity>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReservationRequestArgs {
    pub request_id: uuid::Uuid,
    pub agent_id: String,
    pub workspace_id: String,
    pub reservation_id: Option<String>,
    pub action: String,
    pub path: String,
    pub purpose: String,
    pub identity: Option<RepoIdentity>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReservationCancelArgs {
    pub request_id: uuid::Uuid,
    pub agent_id: String,
    pub workspace_id: String,
    pub wait_id: String,
    pub identity: Option<RepoIdentity>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServerRuntime {
    pub base_url: String,
    pub token: String,
    pub pid: u32,
    pub workspace_id: String,
    #[serde(default = "default_coordination_mode")]
    pub coordination_mode: String,
    pub protocol_version: String,
    pub started_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentContext {
    pub agent_id: String,
    pub workspace_id: String,
}

impl AgentContext {
    pub fn new(agent_id: impl Into<String>, workspace_id: impl Into<String>) -> Self {
        Self {
            agent_id: agent_id.into(),
            workspace_id: workspace_id.into(),
        }
    }
}

fn default_coordination_mode() -> String {
    "awareness".to_string()
}

impl ServerRuntime {
    pub fn new(
        base_url: impl Into<String>,
        token: impl Into<String>,
        workspace_id: impl Into<String>,
        pid: u32,
    ) -> Self {
        Self {
            base_url: base_url.into(),
            token: token.into(),
            pid,
            workspace_id: workspace_id.into(),
            coordination_mode: default_coordination_mode(),
            protocol_version: "stateful.v2".to_string(),
            started_at: "2026-05-31T00:00:00Z".to_string(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpResponse {
    pub status_code: u16,
    pub body: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RuntimeStatus {
    pub protocol_version: String,
    pub journal_schema_version: u64,
    pub coordination_mode: String,
    pub workspace_id: String,
    pub workspace_version: u64,
    pub capabilities: Vec<String>,
}

pub fn write_runtime_file(
    repo_root: impl AsRef<Path>,
    runtime: &ServerRuntime,
) -> anyhow::Result<()> {
    let path = runtime_file_path(repo_root);
    let Some(parent) = path.parent() else {
        anyhow::bail!("runtime file path has no parent");
    };

    fs::create_dir_all(parent)?;
    write_secret_json_file(&path, &serde_json::to_string_pretty(runtime)?)?;
    Ok(())
}

pub fn write_global_runtime_file(
    paths: &GlobalPaths,
    runtime: &ServerRuntime,
) -> anyhow::Result<()> {
    let Some(parent) = paths.server_json.parent() else {
        anyhow::bail!("global runtime file path has no parent");
    };

    fs::create_dir_all(parent)?;
    write_secret_json_file(&paths.server_json, &serde_json::to_string_pretty(runtime)?)?;
    Ok(())
}

fn write_secret_json_file(path: &Path, contents: &str) -> anyhow::Result<()> {
    let temp_path = secret_json_temp_path(path)?;
    let result = write_secret_json_temp_file(&temp_path, contents).and_then(|_| {
        fs::rename(&temp_path, path)?;
        #[cfg(unix)]
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
        sync_parent_dir(path)?;
        Ok(())
    });

    if result.is_err() {
        let _ = fs::remove_file(&temp_path);
    }

    result
}

fn write_secret_json_temp_file(path: &Path, contents: &str) -> anyhow::Result<()> {
    #[cfg(unix)]
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)?;

    #[cfg(not(unix))]
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)?;

    file.write_all(contents.as_bytes())?;
    file.sync_all()?;
    #[cfg(unix)]
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    Ok(())
}

fn secret_json_temp_path(path: &Path) -> anyhow::Result<PathBuf> {
    let Some(parent) = path.parent() else {
        anyhow::bail!("runtime file path has no parent");
    };
    let Some(file_name) = path.file_name().and_then(|name| name.to_str()) else {
        anyhow::bail!("runtime file path has no file name");
    };
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let counter = SECRET_JSON_TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);

    Ok(parent.join(format!(
        ".{file_name}.{}.{}.{}.tmp",
        std::process::id(),
        nanos,
        counter
    )))
}

#[cfg(unix)]
fn sync_parent_dir(path: &Path) -> anyhow::Result<()> {
    let Some(parent) = path.parent() else {
        anyhow::bail!("runtime file path has no parent");
    };
    let directory = fs::File::open(parent)?;
    if let Err(error) = directory.sync_all() {
        if parent_dir_sync_is_unsupported(&error) {
            return Ok(());
        }
        return Err(error.into());
    }
    Ok(())
}

#[cfg(unix)]
fn parent_dir_sync_is_unsupported(error: &std::io::Error) -> bool {
    error.kind() == std::io::ErrorKind::Unsupported || matches!(error.raw_os_error(), Some(45 | 95))
}

#[cfg(not(unix))]
fn sync_parent_dir(_path: &Path) -> anyhow::Result<()> {
    Ok(())
}

pub fn global_state_db_path(paths: &GlobalPaths) -> std::path::PathBuf {
    paths.state_db.clone()
}

pub fn discover_runtime(repo_root: impl AsRef<Path>) -> anyhow::Result<ServerRuntime> {
    if let Some(runtime) = runtime_from_env()? {
        return Ok(runtime);
    }

    let contents = fs::read_to_string(runtime_file_path(repo_root))?;
    Ok(serde_json::from_str(&contents)?)
}

pub fn discover_runtime_with_global(
    repo_root: impl AsRef<Path>,
    paths: &GlobalPaths,
) -> anyhow::Result<ServerRuntime> {
    if let Some(runtime) = runtime_from_env()? {
        return Ok(runtime);
    }

    match fs::read_to_string(&paths.server_json) {
        Ok(contents) => return Ok(serde_json::from_str(&contents)?),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }

    discover_runtime(repo_root)
}

pub fn discover_runtime_with_optional_global(
    repo_root: impl AsRef<Path>,
) -> anyhow::Result<ServerRuntime> {
    let repo_root = repo_root.as_ref();
    match GlobalPaths::from_env() {
        Ok(paths) => discover_runtime_with_global(repo_root, &paths),
        Err(_) => discover_runtime(repo_root),
    }
}

pub fn post_json(
    runtime: &ServerRuntime,
    path: &str,
    body: &serde_json::Value,
) -> anyhow::Result<HttpResponse> {
    post_serialized_json(runtime, path, &body.to_string())
}

fn post_serialized_json(
    runtime: &ServerRuntime,
    path: &str,
    body: &str,
) -> anyhow::Result<HttpResponse> {
    let endpoint = parse_http_base_url(&runtime.base_url)?;
    let request = format!(
        "POST {path} HTTP/1.1\r\nHost: {}\r\nAuthorization: Bearer {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        endpoint.host_header,
        runtime.token,
        body.len(),
        body,
    );

    let mut stream = TcpStream::connect(endpoint.socket_addr)?;
    stream.write_all(request.as_bytes())?;
    stream.set_read_timeout(Some(Duration::from_secs(5)))?;

    let response = read_http_response(&mut stream)?;
    parse_http_response(&response)
}

pub fn get_json(runtime: &ServerRuntime, path: &str) -> anyhow::Result<HttpResponse> {
    let endpoint = parse_http_base_url(&runtime.base_url)?;
    let request = format!(
        "GET {path} HTTP/1.1\r\nHost: {}\r\nAuthorization: Bearer {}\r\nAccept: application/json\r\nConnection: close\r\n\r\n",
        endpoint.host_header, runtime.token,
    );

    let mut stream = TcpStream::connect(endpoint.socket_addr)?;
    stream.write_all(request.as_bytes())?;
    stream.set_read_timeout(Some(Duration::from_secs(5)))?;

    let response = read_http_response(&mut stream)?;
    parse_http_response(&response)
}

pub fn get_v2<Q: Serialize>(
    runtime: &ServerRuntime,
    path: &str,
    request: &QueryEnvelope<Q>,
) -> anyhow::Result<HttpResponse> {
    ensure_v2_path(path)?;
    request.validate().map_err(anyhow::Error::msg)?;
    let query = flattened_query(request)?;
    let response = get_json(runtime, &format!("{path}?{query}"))?;
    decode_v2_response(response, request.request_id)
}

pub fn post_v2<T: Serialize>(
    runtime: &ServerRuntime,
    path: &str,
    request: &RequestEnvelope<T>,
) -> anyhow::Result<HttpResponse> {
    let response = post_v2_raw(runtime, path, request)?;
    decode_v2_response(response, request.request_id)
}

pub fn post_v2_raw<T: Serialize>(
    runtime: &ServerRuntime,
    path: &str,
    request: &RequestEnvelope<T>,
) -> anyhow::Result<HttpResponse> {
    ensure_v2_path(path)?;
    request.validate().map_err(anyhow::Error::msg)?;
    ensure_runtime_protocol(runtime, &request.agent, &request.workspace, &request.source)?;
    post_json(runtime, path, &serde_json::to_value(request)?)
}

pub fn replay_v2_request(
    runtime: &ServerRuntime,
    path: &str,
    serialized_request: &str,
) -> anyhow::Result<HttpResponse> {
    ensure_v2_path(path)?;
    let request = RequestEnvelope::<serde_json::Value>::from_json(serialized_request)
        .map_err(anyhow::Error::msg)?;
    ensure_runtime_protocol(runtime, &request.agent, &request.workspace, &request.source)?;
    let response = post_serialized_json(runtime, path, serialized_request)?;
    decode_v2_response(response, request.request_id)
}

#[expect(
    clippy::too_many_arguments,
    reason = "the V2 envelope exposes its complete protocol identity"
)]
pub fn v2_request_envelope<T>(
    request_id: uuid::Uuid,
    agent_id: String,
    workspace_id: String,
    identity: Option<RepoIdentity>,
    actor_type: ActorType,
    source_kind: SourceKind,
    event: impl Into<String>,
    source_ref: impl Into<String>,
    tool_name: Option<String>,
    payload: T,
) -> anyhow::Result<RequestEnvelope<T>> {
    let (repo_id, worktree_id, root, branch) = identity
        .map(|identity| {
            (
                identity.repo_id,
                identity.worktree_id,
                identity.root,
                identity.branch,
            )
        })
        .unwrap_or_else(|| {
            (
                "unknown".to_string(),
                "unknown".to_string(),
                "unknown".to_string(),
                "unknown".to_string(),
            )
        });
    RequestEnvelope::new(
        request_id,
        time::OffsetDateTime::now_utc(),
        AgentIdentity {
            actor_id: agent_id.clone(),
            agent_id,
            actor_type,
            turn_id: None,
            owner_id: None,
            parent_agent_id: None,
            parent_actor_id: None,
        },
        WorkspaceIdentity {
            root,
            workspace_id,
            repo_id,
            worktree_id,
            branch,
        },
        SourceRef {
            kind: source_kind,
            event: event.into(),
            tool_name,
            source_ref: source_ref.into(),
        },
        payload,
    )
    .map_err(anyhow::Error::msg)
}

pub fn v2_query_envelope<Q>(
    request_id: uuid::Uuid,
    agent: AgentIdentity,
    workspace: WorkspaceIdentity,
    source: SourceRef,
    query: Q,
) -> anyhow::Result<QueryEnvelope<Q>> {
    QueryEnvelope::new(
        request_id,
        time::OffsetDateTime::now_utc(),
        agent,
        workspace,
        source,
        query,
    )
    .map_err(anyhow::Error::msg)
}

#[expect(
    clippy::too_many_arguments,
    reason = "the V2 query exposes its complete protocol identity"
)]
pub fn v2_query_for_runtime<Q>(
    request_id: uuid::Uuid,
    agent_id: String,
    workspace_id: String,
    identity: Option<RepoIdentity>,
    source_kind: SourceKind,
    event: impl Into<String>,
    source_ref: impl Into<String>,
    tool_name: Option<String>,
    query: Q,
) -> anyhow::Result<QueryEnvelope<Q>> {
    let request = v2_request_envelope(
        request_id,
        agent_id,
        workspace_id,
        identity,
        ActorType::Agent,
        source_kind,
        event,
        source_ref,
        tool_name,
        (),
    )?;
    v2_query_envelope(
        request.request_id,
        request.agent,
        request.workspace,
        request.source,
        query,
    )
}
fn ensure_runtime_protocol(
    runtime: &ServerRuntime,
    agent: &AgentIdentity,
    workspace: &WorkspaceIdentity,
    source: &SourceRef,
) -> anyhow::Result<()> {
    let identity =
        fetch_runtime_identity_for(runtime, agent.clone(), workspace.clone(), source.clone())?
            .ok_or_else(|| {
                anyhow::anyhow!("runtime handshake did not return a successful response")
            })?;
    if identity.protocol_version != "stateful.v2" {
        anyhow::bail!(
            "runtime protocol {} is unsupported; stateful.v2 is required before mutation",
            identity.protocol_version
        );
    }
    if identity.journal_schema_version != 2 {
        anyhow::bail!(
            "runtime journal schema {} is unsupported; schema 2 is required before mutation",
            identity.journal_schema_version
        );
    }
    if !runtime_identity_has_required_capabilities(runtime, &identity) {
        anyhow::bail!(
            "runtime does not support required capabilities: {}",
            REQUIRED_RUNTIME_CAPABILITIES.join(", ")
        );
    }
    Ok(())
}

fn ensure_v2_path(path: &str) -> anyhow::Result<()> {
    if path.starts_with("/v2/") {
        Ok(())
    } else {
        anyhow::bail!("stateful.v2 clients require a /v2/ route, got {path}")
    }
}

fn flattened_query<Q: Serialize>(request: &QueryEnvelope<Q>) -> anyhow::Result<String> {
    let serde_json::Value::Object(values) = serde_json::to_value(request)? else {
        anyhow::bail!("query envelope did not serialize as an object");
    };
    let mut pairs = values
        .into_iter()
        .filter_map(|(key, value)| match value {
            serde_json::Value::Null => None,
            serde_json::Value::String(value) => Some(Ok((key, value))),
            serde_json::Value::Bool(value) => Some(Ok((key, value.to_string()))),
            serde_json::Value::Number(value) => Some(Ok((key, value.to_string()))),
            _ => Some(Err(anyhow::anyhow!(
                "query envelope field {key} must be scalar"
            ))),
        })
        .collect::<anyhow::Result<Vec<_>>>()?;
    pairs.sort_by(|left, right| left.0.cmp(&right.0));
    Ok(pairs
        .into_iter()
        .map(|(key, value)| format!("{}={}", percent_encode(&key), percent_encode(&value)))
        .collect::<Vec<_>>()
        .join("&"))
}

fn percent_encode(value: &str) -> String {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    let mut encoded = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                encoded.push(byte as char);
            }
            byte => {
                encoded.push('%');
                encoded.push(HEX[(byte >> 4) as usize] as char);
                encoded.push(HEX[(byte & 0x0f) as usize] as char);
            }
        }
    }
    encoded
}

fn decode_v2_response(
    response: HttpResponse,
    request_id: uuid::Uuid,
) -> anyhow::Result<HttpResponse> {
    if (200..300).contains(&response.status_code) {
        return Ok(response);
    }
    if let Ok(error) = serde_json::from_str::<V2ErrorEnvelope>(&response.body) {
        if error.protocol_version == ProtocolVersion::V2 && error.request_id == request_id {
            anyhow::bail!("{}: {}", error.error.code, error.error.message);
        }
    }
    if let Ok(decision) = serde_json::from_str::<Decision>(&response.body) {
        anyhow::bail!("{}: {}", decision.reason_code, decision.message);
    }
    anyhow::bail!(
        "stateful.v2 request {request_id} failed with HTTP {}",
        response.status_code
    )
}

fn reservation_scope_for_planned_path(path: String) -> ReservationScope {
    if path.ends_with('/') || path.ends_with('\\') {
        ReservationScope::directory(path)
    } else {
        ReservationScope::file(path)
    }
}

pub fn declare_reservation_via_http(
    runtime: &ServerRuntime,
    args: ReservationDeclareArgs,
) -> anyhow::Result<HttpResponse> {
    let ReservationDeclareArgs {
        request_id,
        agent_id,
        workspace_id,
        purpose,
        files_planned,
        identity,
    } = args;
    if files_planned.is_empty() {
        anyhow::bail!("at least one planned file is required");
    }
    let request = v2_request_envelope(
        request_id,
        agent_id,
        workspace_id,
        identity,
        ActorType::Agent,
        SourceKind::Cli,
        "reservation_declare",
        "stateful-cli",
        None,
        serde_json::json!({
            "scopes": files_planned
                .into_iter()
                .map(reservation_scope_for_planned_path)
                .collect::<Vec<_>>(),
            "action": "write",
            "purpose": purpose,
        }),
    )?;
    post_v2(runtime, "/v2/reservation/declare", &request)
}

pub fn claim_reservation_via_http(
    runtime: &ServerRuntime,
    args: ReservationClaimArgs,
) -> anyhow::Result<()> {
    let ReservationClaimArgs {
        request_id,
        agent_id,
        workspace_id,
        wait_id,
        relative_path,
        reservation_id: _,
        identity,
    } = args;
    let normalized_path = normalize_relative_path(&relative_path);
    if normalized_path.is_empty() || normalized_path != relative_path {
        anyhow::bail!("reservation claim path must be a normalized nonempty relative path");
    }
    let request = v2_request_envelope(
        request_id,
        agent_id,
        workspace_id,
        identity,
        ActorType::Agent,
        SourceKind::Cli,
        "reservation_claim",
        "stateful-cli",
        None,
        serde_json::json!({ "relative_path": normalized_path }),
    )?;
    let response = post_v2(runtime, "/v2/reservation/claim", &request)?;
    validate_claimed_wait(&response.body, &wait_id, &normalized_path)?;
    Ok(())
}

fn validate_claimed_wait(
    response_body: &str,
    expected_wait_id: &str,
    expected_relative_path: &str,
) -> anyhow::Result<()> {
    let body: serde_json::Value = serde_json::from_str(response_body)
        .map_err(|error| anyhow::anyhow!("reservation claim returned invalid JSON: {error}"))?;
    if body.get("wait_id").and_then(serde_json::Value::as_str) != Some(expected_wait_id)
        || body
            .get("relative_path")
            .and_then(serde_json::Value::as_str)
            != Some(expected_relative_path)
    {
        anyhow::bail!("reservation claim response did not match queued wait identity and path");
    }
    Ok(())
}

pub fn request_reservation_via_http(
    runtime: &ServerRuntime,
    args: ReservationRequestArgs,
) -> anyhow::Result<HttpResponse> {
    let request = v2_request_envelope(
        args.request_id,
        args.agent_id,
        args.workspace_id,
        args.identity,
        ActorType::Agent,
        SourceKind::Cli,
        "reservation_request",
        "stateful-cli",
        None,
        serde_json::json!({
            "relative_path": args.path,
            "action": args.action,
            "purpose": args.purpose,
            "blocking_agent_id": args.reservation_id,
        }),
    )?;
    post_v2(runtime, "/v2/reservation/request", &request)
}

pub fn cancel_reservation_via_http(
    runtime: &ServerRuntime,
    args: ReservationCancelArgs,
) -> anyhow::Result<()> {
    let request = v2_request_envelope(
        args.request_id,
        args.agent_id,
        args.workspace_id,
        args.identity,
        ActorType::Agent,
        SourceKind::Cli,
        "reservation_cancel",
        "stateful-cli",
        None,
        serde_json::json!({ "wait_id": args.wait_id }),
    )?;
    post_v2(runtime, "/v2/reservation/cancel", &request)?;
    Ok(())
}

pub fn reservation_declare_protocol_body(
    runtime: &ServerRuntime,
    args: ReservationDeclareArgs,
    source_kind: &str,
    source_ref: &str,
) -> serde_json::Value {
    let ReservationDeclareArgs {
        request_id,
        agent_id,
        workspace_id,
        purpose,
        files_planned,
        identity,
    } = args;
    protocol_envelope(ProtocolEnvelopeArgs {
        runtime,
        request_id: request_id.to_string(),
        agent_id,
        workspace_id,
        identity,
        source_kind,
        event: "reservation_declare",
        source_ref,
        source_tool_name: None,
        payload: serde_json::json!({
            "scopes": files_planned
                .into_iter()
                .map(reservation_scope_for_planned_path)
                .collect::<Vec<_>>(),
            "action": "write",
            "purpose": purpose,
        }),
    })
}

pub fn reservation_claim_protocol_body(
    runtime: &ServerRuntime,
    args: ReservationClaimArgs,
    source_kind: &str,
    source_ref: &str,
) -> serde_json::Value {
    let ReservationClaimArgs {
        request_id,
        agent_id,
        workspace_id,
        wait_id,
        relative_path,
        reservation_id,
        identity,
    } = args;
    let mut payload = serde_json::json!({
        "wait_id": wait_id,
        "relative_path": relative_path
    });
    if let Some(reservation_id) = reservation_id {
        payload["reservation_id"] = serde_json::json!(reservation_id);
    }
    protocol_envelope(ProtocolEnvelopeArgs {
        runtime,
        request_id: request_id.to_string(),
        agent_id,
        workspace_id,
        identity,
        source_kind,
        event: "reservation_claim",
        source_ref,
        source_tool_name: None,
        payload,
    })
}

pub fn reservation_request_protocol_body(
    runtime: &ServerRuntime,
    args: ReservationRequestArgs,
    source_kind: &str,
    source_ref: &str,
) -> serde_json::Value {
    let ReservationRequestArgs {
        request_id,
        agent_id,
        workspace_id,
        reservation_id,
        action,
        path,
        purpose,
        identity,
    } = args;
    let mut payload = serde_json::json!({
        "action": action,
        "path": path,
        "purpose": purpose
    });
    if let Some(reservation_id) = reservation_id {
        payload["reservation_id"] = serde_json::json!(reservation_id);
    }
    protocol_envelope(ProtocolEnvelopeArgs {
        runtime,
        request_id: request_id.to_string(),
        agent_id,
        workspace_id,
        identity,
        source_kind,
        event: "reservation_request",
        source_ref,
        source_tool_name: None,
        payload,
    })
}

pub fn reservation_cancel_protocol_body(
    runtime: &ServerRuntime,
    args: ReservationCancelArgs,
    source_kind: &str,
    source_ref: &str,
) -> serde_json::Value {
    let ReservationCancelArgs {
        request_id,
        agent_id,
        workspace_id,
        wait_id,
        identity,
    } = args;
    protocol_envelope(ProtocolEnvelopeArgs {
        runtime,
        request_id: request_id.to_string(),
        agent_id,
        workspace_id,
        identity,
        source_kind,
        event: "reservation_cancel",
        source_ref,
        source_tool_name: None,
        payload: serde_json::json!({
            "wait_id": wait_id
        }),
    })
}

pub struct ProtocolEnvelopeArgs<'a> {
    pub runtime: &'a ServerRuntime,
    pub request_id: String,
    pub agent_id: String,
    pub workspace_id: String,
    pub identity: Option<RepoIdentity>,
    pub source_kind: &'a str,
    pub event: &'a str,
    pub source_ref: &'a str,
    pub source_tool_name: Option<&'a str>,
    pub payload: serde_json::Value,
}

pub fn protocol_envelope(args: ProtocolEnvelopeArgs<'_>) -> serde_json::Value {
    let ProtocolEnvelopeArgs {
        runtime: _,
        request_id,
        agent_id,
        workspace_id,
        identity,
        source_kind,
        event,
        source_ref,
        source_tool_name,
        payload,
    } = args;
    let source_kind = match source_kind {
        "hook" => SourceKind::Hook,
        "watcher" => SourceKind::Watcher,
        "ide" => SourceKind::Ide,
        "server" => SourceKind::Server,
        _ => SourceKind::Cli,
    };

    let request = v2_request_envelope(
        uuid::Uuid::parse_str(&request_id).expect("protocol envelope request ID must be a UUID"),
        agent_id,
        workspace_id,
        identity,
        ActorType::Agent,
        source_kind,
        event,
        source_ref,
        source_tool_name.map(str::to_owned),
        payload,
    )
    .expect("protocol envelope arguments must form a valid stateful.v2 envelope");
    serde_json::to_value(request).expect("protocol envelope must serialize")
}

fn runtime_file_path(repo_root: impl AsRef<Path>) -> std::path::PathBuf {
    repo_root
        .as_ref()
        .join(".stateful_core")
        .join("runtime")
        .join("server.json")
}

pub fn validate_agent_id(agent_id: &str, label: &str) -> anyhow::Result<()> {
    if agent_id.is_empty() {
        anyhow::bail!("{label} is set but empty");
    }
    if !agent_id
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
    {
        anyhow::bail!("{label} contains unsupported characters");
    }
    Ok(())
}

pub fn runtime_has_required_identity(runtime: &ServerRuntime) -> bool {
    let Ok(Some(identity)) = fetch_runtime_identity(runtime) else {
        return false;
    };

    runtime_identity_matches_runtime(runtime, &identity)
        && runtime_identity_has_required_capabilities(runtime, &identity)
}

pub fn runtime_status(runtime: &ServerRuntime) -> anyhow::Result<RuntimeStatus> {
    let identity = fetch_runtime_identity(runtime)?
        .ok_or_else(|| anyhow::anyhow!("runtime identity request failed"))?;
    Ok(RuntimeStatus {
        protocol_version: identity.protocol_version,
        journal_schema_version: identity.journal_schema_version,
        coordination_mode: identity.coordination_mode,
        workspace_id: identity.workspace_id,
        workspace_version: identity.workspace_version,
        capabilities: identity.capabilities,
    })
}

pub fn runtime_identity_matches_pid(runtime: &ServerRuntime) -> anyhow::Result<bool> {
    let Some(identity) = fetch_runtime_identity(runtime)? else {
        return Ok(false);
    };
    Ok(runtime.pid != 0
        && identity.pid == runtime.pid
        && runtime_identity_matches_runtime(runtime, &identity))
}

fn runtime_from_env() -> anyhow::Result<Option<ServerRuntime>> {
    let (Some(base_url), Some(token)) = (
        std::env::var("STATEFUL_SERVER_URL").ok(),
        std::env::var("STATEFUL_SERVER_TOKEN").ok(),
    ) else {
        return Ok(None);
    };
    let runtime = ServerRuntime::new(base_url, token, "unknown", 0);
    let Some(identity) = fetch_runtime_identity(&runtime)? else {
        anyhow::bail!(
            "STATEFUL_SERVER_URL points to a server that did not return a valid stateful.v2 runtime identity"
        );
    };
    if !runtime_identity_has_required_capabilities(&runtime, &identity) {
        anyhow::bail!(
            "STATEFUL_SERVER_URL points to a stateful server that does not support required runtime capabilities: {}",
            REQUIRED_RUNTIME_CAPABILITIES.join(", ")
        );
    }
    Ok(Some(runtime))
}

pub fn runtime_env_override_is_configured() -> bool {
    std::env::var_os("STATEFUL_SERVER_URL").is_some()
        && std::env::var_os("STATEFUL_SERVER_TOKEN").is_some()
}

pub fn runtime_from_remote(
    base_url: impl Into<String>,
    token: impl Into<String>,
    workspace_id: impl Into<String>,
) -> anyhow::Result<ServerRuntime> {
    let runtime = ServerRuntime::new(base_url, token, workspace_id, 0);
    let Some(identity) = fetch_runtime_identity(&runtime)? else {
        anyhow::bail!("LAN server did not return a valid stateful runtime identity");
    };
    if !runtime_identity_has_required_capabilities(&runtime, &identity) {
        anyhow::bail!(
            "LAN server does not support required runtime capabilities: {}",
            REQUIRED_RUNTIME_CAPABILITIES.join(", ")
        );
    }
    Ok(runtime)
}

pub fn runtime_base_url_is_localhost(base_url: &str) -> bool {
    let Some(host) = runtime_base_url_host(base_url) else {
        return false;
    };
    if host.eq_ignore_ascii_case("localhost") {
        return true;
    }
    match host.parse::<IpAddr>() {
        Ok(address) => address.is_loopback(),
        Err(_) => false,
    }
}

fn runtime_base_url_host(base_url: &str) -> Option<&str> {
    let authority = base_url.strip_prefix("http://")?.split('/').next()?;
    if authority.is_empty() {
        return None;
    }
    if let Some(rest) = authority.strip_prefix('[') {
        let (host, _) = rest.split_once(']')?;
        return (!host.is_empty()).then_some(host);
    }
    let host = authority
        .rsplit_once(':')
        .map(|(host, _)| host)
        .unwrap_or(authority);
    (!host.is_empty()).then_some(host)
}

fn fetch_runtime_identity(runtime: &ServerRuntime) -> anyhow::Result<Option<RuntimeIdentity>> {
    let agent = AgentIdentity {
        agent_id: "stateful-cli".to_string(),
        actor_id: "stateful-cli".to_string(),
        actor_type: ActorType::Agent,
        turn_id: None,
        owner_id: None,
        parent_agent_id: None,
        parent_actor_id: None,
    };
    let workspace = WorkspaceIdentity {
        root: "unknown".to_string(),
        workspace_id: runtime.workspace_id.clone(),
        repo_id: "unknown".to_string(),
        worktree_id: "unknown".to_string(),
        branch: "unknown".to_string(),
    };
    let source = SourceRef {
        kind: SourceKind::Cli,
        event: "runtime_identity".to_string(),
        tool_name: None,
        source_ref: "stateful-cli".to_string(),
    };
    fetch_runtime_identity_for(runtime, agent, workspace, source)
}

fn fetch_runtime_identity_for(
    runtime: &ServerRuntime,
    agent: AgentIdentity,
    workspace: WorkspaceIdentity,
    source: SourceRef,
) -> anyhow::Result<Option<RuntimeIdentity>> {
    let query = v2_query_envelope(uuid::Uuid::new_v4(), agent, workspace, source, ())?;
    let response = get_v2(runtime, "/v2/runtime/identity", &query)?;
    if response.status_code != 200 {
        return Ok(None);
    }
    Ok(Some(serde_json::from_str(&response.body)?))
}

fn runtime_identity_matches_runtime(runtime: &ServerRuntime, identity: &RuntimeIdentity) -> bool {
    identity.protocol_version == runtime.protocol_version && identity.journal_schema_version == 2
}

fn runtime_identity_has_required_capabilities(
    runtime: &ServerRuntime,
    identity: &RuntimeIdentity,
) -> bool {
    runtime_identity_matches_runtime(runtime, identity)
        && REQUIRED_RUNTIME_CAPABILITIES.iter().all(|required| {
            identity
                .capabilities
                .iter()
                .any(|capability| capability.as_str() == *required)
        })
}

#[derive(Debug, serde::Deserialize)]
struct RuntimeIdentity {
    protocol_version: String,
    journal_schema_version: u64,
    coordination_mode: String,
    pid: u32,
    #[serde(default)]
    workspace_id: String,
    #[serde(default)]
    workspace_version: u64,
    #[serde(default)]
    capabilities: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ParsedBaseUrl {
    socket_addr: String,
    host_header: String,
}

fn parse_http_base_url(base_url: &str) -> anyhow::Result<ParsedBaseUrl> {
    let without_scheme = base_url
        .strip_prefix("http://")
        .ok_or_else(|| anyhow::anyhow!("only http:// local state server URLs are supported"))?;
    let authority = without_scheme
        .split('/')
        .next()
        .ok_or_else(|| anyhow::anyhow!("missing state server authority"))?;
    if authority.is_empty() {
        anyhow::bail!("missing state server authority");
    }

    Ok(ParsedBaseUrl {
        socket_addr: authority.to_string(),
        host_header: authority.to_string(),
    })
}

fn parse_http_response(response: &str) -> anyhow::Result<HttpResponse> {
    let (headers, body) = response.split_once("\r\n\r\n").unwrap_or((response, ""));
    let status_line = headers
        .lines()
        .next()
        .ok_or_else(|| anyhow::anyhow!("missing HTTP status line"))?;
    let status_code = status_line
        .split_whitespace()
        .nth(1)
        .ok_or_else(|| anyhow::anyhow!("missing HTTP status code"))?
        .parse::<u16>()?;

    Ok(HttpResponse {
        status_code,
        body: body.to_string(),
    })
}

fn read_http_response(stream: &mut TcpStream) -> anyhow::Result<String> {
    let mut buffer = Vec::new();
    let mut byte = [0_u8; 1];
    while !buffer.ends_with(b"\r\n\r\n") {
        let read = stream.read(&mut byte)?;
        if read == 0 {
            break;
        }
        buffer.push(byte[0]);
    }

    let headers = String::from_utf8(buffer.clone())?;
    let content_length = headers
        .lines()
        .find_map(|line| {
            line.to_ascii_lowercase()
                .strip_prefix("content-length: ")
                .map(str::to_string)
        })
        .map(|value| value.trim().parse::<usize>())
        .transpose()?
        .unwrap_or(0);

    let mut body = vec![0_u8; content_length];
    if content_length > 0 {
        stream.read_exact(&mut body)?;
        buffer.extend_from_slice(&body);
    }

    Ok(String::from_utf8(buffer)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn parent_dir_sync_ignores_enotsup() {
        let error = std::io::Error::from_raw_os_error(45);

        assert!(parent_dir_sync_is_unsupported(&error));
    }

    #[test]
    fn v2_authorization_denial_preserves_reason_code() {
        let request_id = uuid::Uuid::new_v4();
        let error = decode_v2_response(
            HttpResponse {
                status_code: 403,
                body: serde_json::json!({
                    "decision": "deny",
                    "reason_code": "missing_reservation",
                    "message": "Declare a reservation.",
                    "required_next_action": "Declare an exact file reservation."
                })
                .to_string(),
            },
            request_id,
        )
        .expect_err("authorization denial should remain an error");

        assert!(error.to_string().contains("missing_reservation"));
        assert!(error.to_string().contains("Declare a reservation."));
    }
    #[test]
    fn claim_validation_rejects_uuid_as_path_or_a_different_queued_wait() {
        let error = validate_claimed_wait(
            r#"{"wait_id":"wait-1","relative_path":"src/auth.ts"}"#,
            "wait-1",
            "00000000-0000-4000-8000-000000000103",
        )
        .expect_err("a UUID must not be accepted as the granted path");
        assert!(
            error.to_string().contains("did not match"),
            "unexpected error: {error}"
        );

        assert!(
            validate_claimed_wait(
                r#"{"wait_id":"wait-other","relative_path":"src/auth.ts"}"#,
                "wait-1",
                "src/auth.ts",
            )
            .is_err(),
            "claim response wait identity must match the queued wait"
        );
    }

    #[test]
    fn native_claim_protocol_preserves_wait_identity_and_granted_path() {
        let runtime = ServerRuntime::new("http://127.0.0.1:43873", "token", "w1", 0);
        let body = reservation_claim_protocol_body(
            &runtime,
            ReservationClaimArgs {
                request_id: uuid::Uuid::new_v4(),
                agent_id: "agent-1".to_string(),
                workspace_id: "w1".to_string(),
                wait_id: "wait-1".to_string(),
                relative_path: "src/auth.ts".to_string(),
                reservation_id: Some("reservation-1".to_string()),
                identity: None,
            },
            "hook",
            "native-tool",
        );

        assert_eq!(body["payload"]["wait_id"], "wait-1");
        assert_eq!(body["payload"]["relative_path"], "src/auth.ts");
        assert_eq!(body["payload"]["reservation_id"], "reservation-1");
    }

}
