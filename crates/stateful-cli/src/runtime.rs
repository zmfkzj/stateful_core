use std::{
    fs,
    io::{Read, Write},
    net::TcpStream,
    path::Path,
    time::Duration,
};

use crate::global_paths::GlobalPaths;
use crate::repo_registry::RepoIdentity;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IntentDeclareArgs {
    pub session_id: String,
    pub workspace_id: String,
    pub files_planned: Vec<String>,
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
pub struct CurrentSession {
    pub session_id: String,
    pub workspace_id: String,
}

impl CurrentSession {
    pub fn new(session_id: impl Into<String>, workspace_id: impl Into<String>) -> Self {
        Self {
            session_id: session_id.into(),
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
    fs::write(path, serde_json::to_string_pretty(runtime)?)?;
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
    fs::write(&paths.server_json, serde_json::to_string_pretty(runtime)?)?;
    Ok(())
}

pub fn global_state_db_path(paths: &GlobalPaths) -> std::path::PathBuf {
    paths.state_db.clone()
}

pub fn discover_runtime(repo_root: impl AsRef<Path>) -> anyhow::Result<ServerRuntime> {
    if let Some(runtime) = runtime_from_env() {
        return Ok(runtime);
    }

    let contents = fs::read_to_string(runtime_file_path(repo_root))?;
    Ok(serde_json::from_str(&contents)?)
}

pub fn discover_runtime_with_global(
    repo_root: impl AsRef<Path>,
    paths: &GlobalPaths,
) -> anyhow::Result<ServerRuntime> {
    if let Some(runtime) = runtime_from_env() {
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

pub fn write_current_session_file(
    repo_root: impl AsRef<Path>,
    session: &CurrentSession,
) -> anyhow::Result<()> {
    let path = current_session_file_path(repo_root);
    let Some(parent) = path.parent() else {
        anyhow::bail!("current session file path has no parent");
    };

    fs::create_dir_all(parent)?;
    fs::write(path, serde_json::to_string_pretty(session)?)?;
    Ok(())
}

pub fn read_current_session_file(repo_root: impl AsRef<Path>) -> anyhow::Result<CurrentSession> {
    let contents = fs::read_to_string(current_session_file_path(repo_root))?;
    Ok(serde_json::from_str(&contents)?)
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

pub fn declare_intent_via_http(
    runtime: &ServerRuntime,
    args: IntentDeclareArgs,
) -> anyhow::Result<()> {
    let body = intent_declare_protocol_body(runtime, args, "cli", "stateful-cli");

    let response = post_json(runtime, "/v1/intent/declare", &body)?;

    if !(200..300).contains(&response.status_code) {
        anyhow::bail!(
            "intent declaration failed with HTTP {}: {}",
            response.status_code,
            response.body
        );
    }

    Ok(())
}

pub fn intent_declare_protocol_body(
    runtime: &ServerRuntime,
    args: IntentDeclareArgs,
    source_kind: &str,
    source_ref: &str,
) -> serde_json::Value {
    let IntentDeclareArgs {
        session_id,
        workspace_id,
        files_planned,
        identity,
    } = args;
    protocol_envelope(
        runtime,
        uuid::Uuid::new_v4().to_string(),
        session_id,
        workspace_id,
        identity,
        source_kind,
        "intent_declare",
        source_ref,
        serde_json::json!({
            "files_planned": files_planned
        }),
    )
}

pub fn protocol_envelope(
    runtime: &ServerRuntime,
    request_id: impl Into<String>,
    session_id: impl Into<String>,
    workspace_id: impl Into<String>,
    identity: Option<RepoIdentity>,
    source_kind: &str,
    event: &str,
    source_ref: &str,
    payload: serde_json::Value,
) -> serde_json::Value {
    let request_id = request_id.into();
    let session_id = session_id.into();
    let workspace_id = workspace_id.into();
    let (repo_id, worktree_id, root, branch) = match identity {
        Some(identity) => (
            identity.repo_id,
            identity.worktree_id,
            identity.root,
            identity.branch,
        ),
        None => (String::new(), String::new(), String::new(), String::new()),
    };

    serde_json::json!({
        "protocol_version": runtime.protocol_version.as_str(),
        "request_id": request_id,
        "observed_at": runtime.started_at.as_str(),
        "session": {
            "session_id": session_id,
            "actor_id": format!("stateful-cli:{}", runtime.pid),
            "actor_type": "agent"
        },
        "workspace": {
            "root": root,
            "workspace_id": workspace_id,
            "repo_id": repo_id,
            "worktree_id": worktree_id,
            "branch": branch
        },
        "source": {
            "kind": source_kind,
            "event": event,
            "source_ref": source_ref
        },
        "payload": payload
    })
}

fn runtime_file_path(repo_root: impl AsRef<Path>) -> std::path::PathBuf {
    repo_root
        .as_ref()
        .join(".stateful_core")
        .join("runtime")
        .join("server.json")
}

fn current_session_file_path(repo_root: impl AsRef<Path>) -> std::path::PathBuf {
    repo_root
        .as_ref()
        .join(".stateful_core")
        .join("runtime")
        .join("session.json")
}

fn runtime_from_env() -> Option<ServerRuntime> {
    let env_url = std::env::var("STATEFUL_SERVER_URL").ok();
    let env_token = std::env::var("STATEFUL_SERVER_TOKEN").ok();
    if let (Some(base_url), Some(token)) = (env_url, env_token) {
        return Some(ServerRuntime::new(base_url, token, "unknown", 0));
    }

    None
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
