use std::{
    fs,
    io::{Read, Write},
    net::{IpAddr, TcpStream},
    path::{Path, PathBuf},
    process::Command,
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

#[cfg(unix)]
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};

use crate::global_paths::GlobalPaths;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use stateful_core::{
    AgentIdentity, ContractRevision, DecisionKind, ProtocolVersion, RequestEnvelope, SourceRef,
    WorkspaceIdentity,
};

static SECRET_JSON_TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServerRuntime {
    pub base_url: String,
    pub token: String,
    pub workspace_id: String,
    pub pid: u32,
    pub process_start_identity: String,
}

impl std::fmt::Debug for ServerRuntime {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ServerRuntime")
            .field("base_url", &self.base_url)
            .field("token", &"<redacted>")
            .field("workspace_id", &self.workspace_id)
            .field("pid", &self.pid)
            .field("process_start_identity", &self.process_start_identity)
            .finish()
    }
}

impl ServerRuntime {
    pub fn new(
        base_url: impl Into<String>,
        token: impl Into<String>,
        workspace_id: impl Into<String>,
        pid: u32,
        process_start_identity: impl Into<String>,
    ) -> Self {
        Self {
            base_url: base_url.into(),
            token: token.into(),
            workspace_id: workspace_id.into(),
            pid,
            process_start_identity: process_start_identity.into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandIdentity {
    pub task_id: String,
    pub request_id: String,
    pub observed_at: String,
    pub agent: AgentIdentity,
    pub workspace: WorkspaceIdentity,
    pub source: SourceRef,
}

impl CommandIdentity {
    pub fn new(
        task_id: impl Into<String>,
        request_id: impl Into<String>,
        observed_at: impl Into<String>,
        agent: AgentIdentity,
        workspace: WorkspaceIdentity,
        source: SourceRef,
    ) -> Self {
        Self {
            task_id: task_id.into(),
            request_id: request_id.into(),
            observed_at: observed_at.into(),
            agent,
            workspace,
            source,
        }
    }

    pub fn new_now(
        task_id: impl Into<String>,
        request_id: impl Into<String>,
        agent: AgentIdentity,
        workspace: WorkspaceIdentity,
        source: SourceRef,
    ) -> Self {
        Self::new(task_id, request_id, now_rfc3339(), agent, workspace, source)
    }

    fn envelope(&self) -> RequestEnvelope {
        RequestEnvelope {
            protocol_version: ProtocolVersion::V2,
            contract_revision: ContractRevision::Lease1,
            task_id: self.task_id.clone(),
            request_id: self.request_id.clone(),
            observed_at: self.observed_at.clone(),
            agent: self.agent.clone(),
            workspace: self.workspace.clone(),
            source: self.source.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandError {
    pub status_code: Option<u16>,
    pub reason_code: String,
    pub message: String,
    pub request_id: Option<String>,
}

impl CommandError {
    fn transport(error: impl std::fmt::Display) -> Self {
        Self {
            status_code: None,
            reason_code: "transport_error".to_string(),
            message: error.to_string(),
            request_id: None,
        }
    }

    fn protocol(status_code: Option<u16>, message: impl Into<String>) -> Self {
        Self {
            status_code,
            reason_code: "protocol_mismatch".to_string(),
            message: message.into(),
            request_id: None,
        }
    }
}

impl std::fmt::Display for CommandError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.status_code {
            Some(status_code) => write!(
                formatter,
                "stateful command failed with HTTP {status_code} {}: {}",
                self.reason_code, self.message
            ),
            None => write!(
                formatter,
                "stateful command failed {}: {}",
                self.reason_code, self.message
            ),
        }
    }
}

impl std::error::Error for CommandError {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct HttpResponse {
    pub(crate) status_code: u16,
    pub(crate) body: String,
}

#[derive(Serialize)]
struct CommandRequest<'a, P> {
    #[serde(flatten)]
    envelope: &'a RequestEnvelope,
    payload: &'a P,
}

#[derive(Deserialize)]
struct CommandResponse<R> {
    protocol_version: ProtocolVersion,
    contract_revision: ContractRevision,
    #[serde(default)]
    request_id: Option<String>,
    payload: R,
}

#[derive(Deserialize)]
struct ErrorResponse {
    protocol_version: ProtocolVersion,
    contract_revision: ContractRevision,
    decision: DecisionKind,
    reason_code: String,
    message: String,
    #[serde(default)]
    request_id: Option<String>,
}

pub fn post_command<P, R>(
    runtime: &ServerRuntime,
    path: &str,
    identity: &CommandIdentity,
    payload: &P,
) -> Result<R, CommandError>
where
    P: Serialize,
    R: DeserializeOwned,
{
    validate_v2_path(path)?;
    let envelope = identity.envelope();
    let body = serde_json::to_value(CommandRequest {
        envelope: &envelope,
        payload,
    })
    .map_err(CommandError::transport)?;
    let response = post_json_value(runtime, path, &body).map_err(CommandError::transport)?;
    let payload = decode_payload(response)?;
    if payload.request_id.as_deref() != Some(identity.request_id.as_str()) {
        return Err(CommandError::protocol(
            Some(200),
            "command response request_id does not match the request",
        ));
    }
    Ok(payload.payload)
}

pub fn get_payload<R>(runtime: &ServerRuntime, path: &str) -> Result<R, CommandError>
where
    R: DeserializeOwned,
{
    validate_v2_path(path)?;
    let response = get_response(runtime, path).map_err(CommandError::transport)?;
    Ok(decode_payload(response)?.payload)
}

pub fn now_rfc3339() -> String {
    time::OffsetDateTime::now_utc()
        .format(&time::format_description::well_known::Rfc3339)
        .expect("UTC timestamp should format as RFC3339")
}

fn runtime_file_path(repo_root: impl AsRef<Path>) -> PathBuf {
    repo_root
        .as_ref()
        .join(".stateful_core")
        .join("runtime")
        .join("server.json")
}

pub fn write_runtime_file(
    repo_root: impl AsRef<Path>,
    runtime: &ServerRuntime,
) -> anyhow::Result<()> {
    validate_runtime_file(runtime)?;
    let path = runtime_file_path(repo_root);
    let Some(parent) = path.parent() else {
        anyhow::bail!("runtime file path has no parent");
    };

    fs::create_dir_all(parent)?;
    write_secret_json_file(&path, &serde_json::to_string_pretty(runtime)?)
}

pub fn write_global_runtime_file(
    paths: &GlobalPaths,
    runtime: &ServerRuntime,
) -> anyhow::Result<()> {
    validate_runtime_file(runtime)?;
    let Some(parent) = paths.server_json.parent() else {
        anyhow::bail!("global runtime file path has no parent");
    };

    fs::create_dir_all(parent)?;
    write_secret_json_file(&paths.server_json, &serde_json::to_string_pretty(runtime)?)
}

fn write_secret_json_file(path: &Path, contents: &str) -> anyhow::Result<()> {
    let temp_path = secret_json_temp_path(path)?;
    let result = write_secret_json_temp_file(&temp_path, contents).and_then(|_| {
        fs::rename(&temp_path, path)?;
        #[cfg(unix)]
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
        sync_parent_dir(path)
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
pub(crate) fn sync_parent_dir(path: &Path) -> anyhow::Result<()> {
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
pub(crate) fn sync_parent_dir(_path: &Path) -> anyhow::Result<()> {
    Ok(())
}

pub fn global_state_db_path(paths: &GlobalPaths) -> PathBuf {
    paths.state_db.clone()
}

pub fn discover_runtime(repo_root: impl AsRef<Path>) -> anyhow::Result<ServerRuntime> {
    let contents = fs::read_to_string(runtime_file_path(repo_root))?;
    parse_runtime_file(&contents)
}

pub fn discover_runtime_with_global(
    repo_root: impl AsRef<Path>,
    paths: &GlobalPaths,
) -> anyhow::Result<ServerRuntime> {
    match fs::read_to_string(&paths.server_json) {
        Ok(contents) => parse_runtime_file(&contents),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => discover_runtime(repo_root),
        Err(error) => Err(error.into()),
    }
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

fn parse_runtime_file(contents: &str) -> anyhow::Result<ServerRuntime> {
    let runtime: ServerRuntime = serde_json::from_str(contents)?;
    validate_runtime_file(&runtime)?;
    Ok(runtime)
}

pub fn process_start_identity_for_pid(pid: u32) -> anyhow::Result<String> {
    let output = process_ps_output(pid, &["lstart=", "uid=", "comm="])?;
    let canonical = output.split_whitespace().collect::<Vec<_>>().join(" ");
    if canonical.is_empty() {
        anyhow::bail!("process {pid} does not have a start identity");
    }
    Ok(stateful_core::digest_bytes(canonical.as_bytes()).value)
}

pub fn process_parent_pid(pid: u32) -> anyhow::Result<Option<u32>> {
    let parent = process_ps_output(pid, &["ppid="])?.trim().parse::<u32>()?;
    Ok((parent > 1).then_some(parent))
}

fn process_ps_output(pid: u32, columns: &[&str]) -> anyhow::Result<String> {
    if pid == 0 {
        anyhow::bail!("process id must be nonzero");
    }
    let mut args = vec!["-p".to_string(), pid.to_string()];
    for column in columns {
        args.extend(["-o".to_string(), (*column).to_string()]);
    }
    let output = Command::new("ps").args(args).output()?;
    if !output.status.success() {
        anyhow::bail!("process {pid} does not exist");
    }
    let output = String::from_utf8(output.stdout)?;
    if output.trim().is_empty() {
        anyhow::bail!("process {pid} does not exist");
    }
    Ok(output)
}

pub fn validate_runtime_process_identity(runtime: &ServerRuntime) -> anyhow::Result<()> {
    if runtime.pid == 0 || runtime.process_start_identity.trim().is_empty() {
        anyhow::bail!("stateful runtime process identity is incomplete");
    }

    let observed = process_start_identity_for_pid(runtime.pid)
        .map_err(|_| anyhow::anyhow!("stateful runtime process {} is unavailable", runtime.pid))?;
    if observed != runtime.process_start_identity {
        anyhow::bail!(
            "stateful runtime process identity does not match pid {}",
            runtime.pid
        );
    }
    Ok(())
}

pub fn runtime_base_url_is_loopback(base_url: &str) -> bool {
    parse_http_base_url(base_url).is_ok()
}

pub(crate) fn get_response(runtime: &ServerRuntime, path: &str) -> anyhow::Result<HttpResponse> {
    validate_runtime_process_identity(runtime)?;
    validate_http_path(path)?;
    let endpoint = parse_http_base_url(&runtime.base_url)?;
    if runtime.token.is_empty() {
        anyhow::bail!("stateful runtime token must not be empty");
    }
    let request = format!(
        "GET {path} HTTP/1.1\r\nHost: {}\r\nAuthorization: Bearer {}\r\nAccept: application/json\r\nConnection: close\r\n\r\n",
        endpoint.host_header, runtime.token,
    );
    send_request(endpoint, request)
}

fn post_json_value(
    runtime: &ServerRuntime,
    path: &str,
    body: &serde_json::Value,
) -> anyhow::Result<HttpResponse> {
    validate_runtime_process_identity(runtime)?;
    validate_http_path(path)?;
    let endpoint = parse_http_base_url(&runtime.base_url)?;
    if runtime.token.is_empty() {
        anyhow::bail!("stateful runtime token must not be empty");
    }
    let body = body.to_string();
    let request = format!(
        "POST {path} HTTP/1.1\r\nHost: {}\r\nAuthorization: Bearer {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        endpoint.host_header,
        runtime.token,
        body.len(),
        body,
    );
    send_request(endpoint, request)
}

fn send_request(endpoint: ParsedBaseUrl, request: String) -> anyhow::Result<HttpResponse> {
    let mut stream = TcpStream::connect(endpoint.socket_addr)?;
    stream.write_all(request.as_bytes())?;
    stream.set_read_timeout(Some(Duration::from_secs(5)))?;
    parse_http_response(&read_http_response(&mut stream)?)
}

fn decode_payload<R>(response: HttpResponse) -> Result<CommandResponse<R>, CommandError>
where
    R: DeserializeOwned,
{
    if (200..300).contains(&response.status_code) {
        let response: CommandResponse<R> =
            serde_json::from_str(&response.body).map_err(|error| {
                CommandError::protocol(
                    Some(response.status_code),
                    format!("invalid stateful.v2 lease-1 response: {error}"),
                )
            })?;
        if response.protocol_version != ProtocolVersion::V2
            || response.contract_revision != ContractRevision::Lease1
        {
            return Err(CommandError::protocol(
                Some(200),
                "response does not use the stateful.v2 lease-1 contract",
            ));
        }
        return Ok(response);
    }

    let error: ErrorResponse = serde_json::from_str(&response.body).map_err(|parse_error| {
        CommandError::protocol(
            Some(response.status_code),
            format!("invalid stateful.v2 lease-1 error response: {parse_error}"),
        )
    })?;
    if error.protocol_version != ProtocolVersion::V2
        || error.contract_revision != ContractRevision::Lease1
        || error.decision != DecisionKind::Error
    {
        return Err(CommandError::protocol(
            Some(response.status_code),
            "non-success response does not use the stateful.v2 lease-1 error contract",
        ));
    }
    Err(CommandError {
        status_code: Some(response.status_code),
        reason_code: error.reason_code,
        message: error.message,
        request_id: error.request_id,
    })
}

fn validate_v2_path(path: &str) -> Result<(), CommandError> {
    if !path.starts_with("/v2/") {
        return Err(CommandError::protocol(
            None,
            "stateful commands must use a /v2/ route",
        ));
    }
    validate_http_path(path).map_err(CommandError::transport)
}

fn validate_http_path(path: &str) -> anyhow::Result<()> {
    if path.is_empty() || path.contains(['\r', '\n']) {
        anyhow::bail!("invalid stateful server request path");
    }
    Ok(())
}

fn validate_runtime_file(runtime: &ServerRuntime) -> anyhow::Result<()> {
    parse_http_base_url(&runtime.base_url)?;
    if runtime.token.is_empty() {
        anyhow::bail!("stateful runtime token must not be empty");
    }
    if runtime.workspace_id.is_empty() {
        anyhow::bail!("stateful runtime workspace id must not be empty");
    }
    if runtime.pid == 0 {
        anyhow::bail!("stateful runtime process id must be nonzero");
    }
    if runtime.process_start_identity.trim().is_empty() {
        anyhow::bail!("stateful runtime process start identity must not be empty");
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ParsedBaseUrl {
    socket_addr: String,
    host_header: String,
}

fn parse_http_base_url(base_url: &str) -> anyhow::Result<ParsedBaseUrl> {
    let without_scheme = base_url
        .strip_prefix("http://")
        .ok_or_else(|| anyhow::anyhow!("only http:// loopback state server URLs are supported"))?;
    let (authority, suffix) = without_scheme
        .split_once('/')
        .unwrap_or((without_scheme, ""));
    if !suffix.is_empty() {
        anyhow::bail!("stateful runtime base URL must not contain a path");
    }
    if authority.is_empty() {
        anyhow::bail!("missing stateful server authority");
    }
    let (host, port) = split_authority(authority)?;
    if !host_is_loopback(host) {
        anyhow::bail!("stateful runtime must use a loopback HTTP base URL");
    }
    let _ = port.parse::<u16>()?;

    Ok(ParsedBaseUrl {
        socket_addr: authority.to_string(),
        host_header: authority.to_string(),
    })
}

fn split_authority(authority: &str) -> anyhow::Result<(&str, &str)> {
    if let Some(rest) = authority.strip_prefix('[') {
        let (host, port) = rest
            .split_once("]:")
            .ok_or_else(|| anyhow::anyhow!("invalid bracketed stateful server authority"))?;
        if host.is_empty() || port.is_empty() || port.contains(']') {
            anyhow::bail!("invalid bracketed stateful server authority");
        }
        return Ok((host, port));
    }
    let (host, port) = authority
        .rsplit_once(':')
        .ok_or_else(|| anyhow::anyhow!("missing stateful server port"))?;
    if host.is_empty() || port.is_empty() || host.contains(':') {
        anyhow::bail!("invalid stateful server authority");
    }
    Ok((host, port))
}

fn host_is_loopback(host: &str) -> bool {
    host.eq_ignore_ascii_case("localhost")
        || host
            .parse::<IpAddr>()
            .is_ok_and(|address| address.is_loopback())
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
