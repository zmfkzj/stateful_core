use std::{
    fs,
    io::{Read, Write},
    net::{IpAddr, TcpStream, ToSocketAddrs},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

#[cfg(unix)]
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};

use crate::global_paths::GlobalPaths;
use crate::repo_registry::RepoIdentity;
use serde::{Deserialize, Serialize};

const REQUIRED_RUNTIME_CAPABILITIES: &[&str] = &["authorize.write_directory"];
static SECRET_JSON_TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);
const HTTP_CONNECT_TIMEOUT: Duration = Duration::from_secs(1);
const HTTP_READ_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReservationDeclareArgs {
    pub agent_id: String,
    pub workspace_id: String,
    pub purpose: String,
    pub files_planned: Vec<String>,
    pub identity: Option<RepoIdentity>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReservationClaimArgs {
    pub agent_id: String,
    pub workspace_id: String,
    pub wait_id: String,
    pub reservation_id: Option<String>,
    pub identity: Option<RepoIdentity>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReservationRequestArgs {
    pub agent_id: String,
    pub workspace_id: String,
    pub request_id: String,
    pub reservation_id: Option<String>,
    pub action: String,
    pub path: String,
    pub purpose: String,
    pub identity: Option<RepoIdentity>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReservationCancelArgs {
    pub agent_id: String,
    pub workspace_id: String,
    pub request_id: String,
    pub identity: Option<RepoIdentity>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServerRuntime {
    pub base_url: String,
    pub token: String,
    pub pid: u32,
    pub workspace_id: String,
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
            protocol_version: "stateful.v1".to_string(),
            started_at: "2026-05-31T00:00:00Z".to_string(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpResponse {
    pub status_code: u16,
    pub body: String,
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
    let endpoint = parse_http_base_url(&runtime.base_url)?;
    let body = body.to_string();
    let request = format!(
        "POST {path} HTTP/1.1\r\nHost: {}\r\nAuthorization: Bearer {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        endpoint.host_header,
        runtime.token,
        body.len(),
        body,
    );

    let mut stream = connect_http_stream(&endpoint)?;
    stream.write_all(request.as_bytes())?;

    let response = read_http_response(&mut stream)?;
    parse_http_response(&response)
}

pub fn get_json(runtime: &ServerRuntime, path: &str) -> anyhow::Result<HttpResponse> {
    let endpoint = parse_http_base_url(&runtime.base_url)?;
    let request = format!(
        "GET {path} HTTP/1.1\r\nHost: {}\r\nAuthorization: Bearer {}\r\nAccept: application/json\r\nConnection: close\r\n\r\n",
        endpoint.host_header, runtime.token,
    );

    let mut stream = connect_http_stream(&endpoint)?;
    stream.write_all(request.as_bytes())?;

    let response = read_http_response(&mut stream)?;
    parse_http_response(&response)
}

pub fn declare_reservation_via_http(
    runtime: &ServerRuntime,
    args: ReservationDeclareArgs,
) -> anyhow::Result<HttpResponse> {
    let body = reservation_declare_protocol_body(runtime, args, "cli", "stateful-cli");

    let response = post_json(runtime, "/v1/reservation/declare", &body)?;

    if !(200..300).contains(&response.status_code) {
        anyhow::bail!(
            "reservation declaration failed with HTTP {}: {}",
            response.status_code,
            response.body
        );
    }

    Ok(response)
}

pub fn claim_reservation_via_http(
    runtime: &ServerRuntime,
    args: ReservationClaimArgs,
) -> anyhow::Result<()> {
    let body = reservation_claim_protocol_body(runtime, args, "cli", "stateful-cli");

    let response = post_json(runtime, "/v1/reservation/claim", &body)?;

    if !(200..300).contains(&response.status_code) {
        anyhow::bail!(
            "reservation claim failed with HTTP {}: {}",
            response.status_code,
            response.body
        );
    }

    Ok(())
}

pub fn request_reservation_via_http(
    runtime: &ServerRuntime,
    args: ReservationRequestArgs,
) -> anyhow::Result<HttpResponse> {
    let body = reservation_request_protocol_body(runtime, args, "cli", "stateful-cli");

    let response = post_json(runtime, "/v1/reservation/request", &body)?;

    if !(200..300).contains(&response.status_code) {
        anyhow::bail!(
            "reservation request failed with HTTP {}: {}",
            response.status_code,
            response.body
        );
    }

    Ok(response)
}

pub fn cancel_reservation_via_http(
    runtime: &ServerRuntime,
    args: ReservationCancelArgs,
) -> anyhow::Result<()> {
    let body = reservation_cancel_protocol_body(runtime, args, "cli", "stateful-cli");

    let response = post_json(runtime, "/v1/reservation/cancel", &body)?;

    if !(200..300).contains(&response.status_code) {
        anyhow::bail!(
            "reservation cancel failed with HTTP {}: {}",
            response.status_code,
            response.body
        );
    }

    Ok(())
}

pub fn reservation_declare_protocol_body(
    runtime: &ServerRuntime,
    args: ReservationDeclareArgs,
    source_kind: &str,
    source_ref: &str,
) -> serde_json::Value {
    let ReservationDeclareArgs {
        agent_id,
        workspace_id,
        purpose,
        files_planned,
        identity,
    } = args;
    protocol_envelope(ProtocolEnvelopeArgs {
        runtime,
        request_id: uuid::Uuid::new_v4().to_string(),
        agent_id,
        workspace_id,
        identity,
        source_kind,
        event: "reservation_declare",
        source_ref,
        source_tool_name: None,
        payload: serde_json::json!({
            "purpose": purpose,
            "files_planned": files_planned
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
        agent_id,
        workspace_id,
        wait_id,
        reservation_id,
        identity,
    } = args;
    let mut payload = serde_json::json!({
        "wait_id": wait_id
    });
    if let Some(reservation_id) = reservation_id {
        payload["reservation_id"] = serde_json::json!(reservation_id);
    }
    protocol_envelope(ProtocolEnvelopeArgs {
        runtime,
        request_id: uuid::Uuid::new_v4().to_string(),
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
        agent_id,
        workspace_id,
        request_id,
        reservation_id,
        action,
        path,
        purpose,
        identity,
    } = args;
    let mut payload = serde_json::json!({
        "request_id": request_id,
        "action": action,
        "path": path,
        "purpose": purpose
    });
    if let Some(reservation_id) = reservation_id {
        payload["reservation_id"] = serde_json::json!(reservation_id);
    }
    protocol_envelope(ProtocolEnvelopeArgs {
        runtime,
        request_id: uuid::Uuid::new_v4().to_string(),
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
        agent_id,
        workspace_id,
        request_id,
        identity,
    } = args;
    protocol_envelope(ProtocolEnvelopeArgs {
        runtime,
        request_id: uuid::Uuid::new_v4().to_string(),
        agent_id,
        workspace_id,
        identity,
        source_kind,
        event: "reservation_cancel",
        source_ref,
        source_tool_name: None,
        payload: serde_json::json!({
            "request_id": request_id
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
        runtime,
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
    let (repo_id, worktree_id, root, branch) = match identity {
        Some(identity) => (
            identity.repo_id,
            identity.worktree_id,
            identity.root,
            identity.branch,
        ),
        None => (String::new(), String::new(), String::new(), String::new()),
    };

    let mut source = serde_json::json!({
        "kind": source_kind,
        "event": event,
        "source_ref": source_ref
    });
    if let Some(tool_name) = source_tool_name {
        source["tool_name"] = serde_json::json!(tool_name);
    }

    serde_json::json!({
        "protocol_version": runtime.protocol_version.as_str(),
        "request_id": request_id,
        "observed_at": now_rfc3339_timestamp(),
        "agent": {
            "agent_id": agent_id,
            "actor_id": agent_id,
            "actor_type": "agent"
        },
        "workspace": {
            "root": root,
            "workspace_id": workspace_id,
            "repo_id": repo_id,
            "worktree_id": worktree_id,
            "branch": branch
        },
        "source": source,
        "payload": payload
    })
}

fn now_rfc3339_timestamp() -> String {
    time::OffsetDateTime::now_utc()
        .format(&time::format_description::well_known::Rfc3339)
        .expect("UTC timestamp should format as RFC3339")
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

pub fn runtime_identity_matches_pid(runtime: &ServerRuntime) -> anyhow::Result<bool> {
    let Some(identity) = fetch_runtime_identity(runtime)? else {
        return Ok(false);
    };
    Ok(identity.status == "ok"
        && identity.protocol_version == runtime.protocol_version
        && identity.pid == runtime.pid)
}

fn runtime_from_env() -> anyhow::Result<Option<ServerRuntime>> {
    let (Some(base_url), Some(token)) = (
        std::env::var("STATEFUL_SERVER_URL").ok(),
        std::env::var("STATEFUL_SERVER_TOKEN").ok(),
    ) else {
        return Ok(None);
    };

    let mut runtime = ServerRuntime::new(base_url, token, "unknown", 0);
    let Some(identity) = fetch_runtime_identity(&runtime)? else {
        anyhow::bail!(
            "STATEFUL_SERVER_URL points to a server that did not return a valid stateful runtime identity"
        );
    };
    if !runtime_identity_has_required_capabilities(&runtime, &identity) {
        anyhow::bail!(
            "STATEFUL_SERVER_URL points to a stateful server that does not support required runtime capabilities: {}",
            REQUIRED_RUNTIME_CAPABILITIES.join(", ")
        );
    }
    runtime.pid = identity.pid;
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
    let response = get_json(runtime, "/v1/runtime/identity")?;
    if response.status_code != 200 {
        return Ok(None);
    }

    Ok(Some(serde_json::from_str(&response.body)?))
}

fn runtime_identity_matches_runtime(runtime: &ServerRuntime, identity: &RuntimeIdentity) -> bool {
    identity.status == "ok"
        && identity.protocol_version == runtime.protocol_version
        && (runtime.pid == 0 || identity.pid == runtime.pid)
}

fn runtime_identity_has_required_capabilities(
    runtime: &ServerRuntime,
    identity: &RuntimeIdentity,
) -> bool {
    identity.status == "ok"
        && identity.protocol_version == runtime.protocol_version
        && REQUIRED_RUNTIME_CAPABILITIES.iter().all(|required| {
            identity
                .capabilities
                .iter()
                .any(|capability| capability.as_str() == *required)
        })
}

#[derive(Debug, serde::Deserialize)]
struct RuntimeIdentity {
    status: String,
    pid: u32,
    protocol_version: String,
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

fn connect_http_stream(endpoint: &ParsedBaseUrl) -> anyhow::Result<TcpStream> {
    let mut last_error = None;
    let mut resolved_any = false;
    for address in endpoint.socket_addr.to_socket_addrs()? {
        resolved_any = true;
        match TcpStream::connect_timeout(&address, HTTP_CONNECT_TIMEOUT) {
            Ok(stream) => {
                stream.set_read_timeout(Some(HTTP_READ_TIMEOUT))?;
                stream.set_write_timeout(Some(HTTP_READ_TIMEOUT))?;
                return Ok(stream);
            }
            Err(error) => last_error = Some(error),
        }
    }

    if !resolved_any {
        anyhow::bail!(
            "stateful runtime address {} resolved to no socket addresses",
            endpoint.host_header
        );
    }

    let Some(error) = last_error else {
        anyhow::bail!(
            "stateful runtime address {} resolved to no socket addresses",
            endpoint.host_header
        );
    };
    anyhow::bail!(
        "failed to connect to stateful runtime {} within {}s: {}",
        endpoint.host_header,
        HTTP_CONNECT_TIMEOUT.as_secs(),
        error
    )
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
}
