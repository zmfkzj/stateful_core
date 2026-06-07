use std::{
    fs,
    io::{Read, Write},
    net::TcpStream,
    path::Path,
    time::Duration,
};

#[cfg(unix)]
use std::os::unix::fs::MetadataExt;

use crate::global_paths::GlobalPaths;
use crate::repo_registry::RepoIdentity;
use serde::{Deserialize, Serialize};

pub const STATEFUL_CODEX_RUN_ID_ENV: &str = "STATEFUL_CODEX_RUN_ID";
const REQUIRED_RUNTIME_CAPABILITIES: &[&str] = &["authorize.write_directory"];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IntentDeclareArgs {
    pub session_id: String,
    pub workspace_id: String,
    pub purpose: String,
    pub files_planned: Vec<String>,
    pub identity: Option<RepoIdentity>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IntentClaimArgs {
    pub session_id: String,
    pub workspace_id: String,
    pub wait_id: String,
    pub identity: Option<RepoIdentity>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IntentRequestArgs {
    pub session_id: String,
    pub workspace_id: String,
    pub request_id: String,
    pub action: String,
    pub path: String,
    pub purpose: String,
    pub identity: Option<RepoIdentity>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IntentCancelArgs {
    pub session_id: String,
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

pub fn write_current_session_file(
    repo_root: impl AsRef<Path>,
    session: &CurrentSession,
) -> anyhow::Result<()> {
    let repo_root = repo_root.as_ref();
    if let Some(codex_run_id) = current_codex_run_id()? {
        write_current_session_file_for_codex_run(repo_root, &codex_run_id, session)?;
        if current_session_file_is_untrusted(&current_session_file_path(repo_root))? {
            return Ok(());
        }
    }

    write_legacy_current_session_file(repo_root, session)?;
    Ok(())
}

fn write_legacy_current_session_file(
    repo_root: &Path,
    session: &CurrentSession,
) -> anyhow::Result<()> {
    let runtime_dir = ensure_runtime_dir(repo_root)?;
    let path = runtime_dir.join("session.json");
    write_plain_file(
        &path,
        "current session file",
        &serde_json::to_string_pretty(session)?,
    )
}

pub fn read_current_session_file(repo_root: impl AsRef<Path>) -> anyhow::Result<CurrentSession> {
    let repo_root = repo_root.as_ref();
    if let Some(codex_run_id) = current_codex_run_id()? {
        return read_current_session_file_for_codex_run(repo_root, &codex_run_id);
    }

    reject_untrusted_runtime_dirs(repo_root, false)?;
    let contents = read_plain_file_to_string(
        &current_session_file_path(repo_root),
        "current session file",
    )?;
    Ok(serde_json::from_str(&contents)?)
}

pub fn read_codex_run_bound_current_session(
    repo_root: impl AsRef<Path>,
) -> anyhow::Result<CurrentSession> {
    let repo_root = repo_root.as_ref();
    let codex_run_id = current_codex_run_id()?.ok_or_else(|| {
        anyhow::anyhow!("{STATEFUL_CODEX_RUN_ID_ENV} is required for session-bound MCP tools")
    })?;
    read_current_session_file_for_codex_run(repo_root, &codex_run_id)
}

pub fn write_current_session_file_for_codex_run(
    repo_root: impl AsRef<Path>,
    codex_run_id: &str,
    session: &CurrentSession,
) -> anyhow::Result<()> {
    let repo_root = repo_root.as_ref();
    let path = current_session_file_path_for_codex_run(repo_root, codex_run_id)?;
    let runtime_dir = ensure_runtime_dir(repo_root)?;
    ensure_plain_directory(&runtime_dir.join("sessions"), "current session directory")?;
    match read_plain_file_to_string(&path, "current session file") {
        Ok(contents) => {
            let existing: CurrentSession = serde_json::from_str(&contents)?;
            if existing != *session {
                anyhow::bail!(
                    "Codex run `{codex_run_id}` is already bound to session `{}`",
                    existing.session_id
                );
            }
        }
        Err(error) if is_not_found_error(&error) => {}
        Err(error) => return Err(error),
    }

    write_plain_file(
        &path,
        "current session file",
        &serde_json::to_string_pretty(session)?,
    )?;
    Ok(())
}

pub fn read_current_session_file_for_codex_run(
    repo_root: impl AsRef<Path>,
    codex_run_id: &str,
) -> anyhow::Result<CurrentSession> {
    let repo_root = repo_root.as_ref();
    reject_untrusted_runtime_dirs(repo_root, true)?;
    let contents = read_plain_file_to_string(
        &current_session_file_path_for_codex_run(repo_root, codex_run_id)?,
        "current session file",
    )?;
    Ok(serde_json::from_str(&contents)?)
}

fn read_plain_file_to_string(path: &Path, label: &str) -> anyhow::Result<String> {
    let metadata = fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() {
        anyhow::bail!("stateful refuses symlinked {label}");
    }
    if !metadata.is_file() {
        anyhow::bail!("stateful {label} is not a regular file");
    }
    if is_hard_linked_file(&metadata) {
        anyhow::bail!("stateful refuses hard-linked {label}");
    }
    Ok(fs::read_to_string(path)?)
}

fn is_not_found_error(error: &anyhow::Error) -> bool {
    error
        .downcast_ref::<std::io::Error>()
        .is_some_and(|error| error.kind() == std::io::ErrorKind::NotFound)
}

fn write_plain_file(path: &Path, label: &str, contents: &str) -> anyhow::Result<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            anyhow::bail!("stateful refuses symlinked {label}");
        }
        Ok(metadata) if !metadata.is_file() => {
            anyhow::bail!("stateful {label} is not a regular file");
        }
        Ok(metadata) if is_hard_linked_file(&metadata) => {
            anyhow::bail!("stateful refuses hard-linked {label}");
        }
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    fs::write(path, contents)?;
    Ok(())
}

fn current_session_file_is_untrusted(path: &Path) -> anyhow::Result<bool> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => Ok(metadata.file_type().is_symlink()
            || !metadata.is_file()
            || is_hard_linked_file(&metadata)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error.into()),
    }
}

fn ensure_runtime_dir(repo_root: &Path) -> anyhow::Result<std::path::PathBuf> {
    let state_dir = repo_root.join(".stateful_core");
    ensure_plain_directory(&state_dir, "stateful directory")?;
    let runtime_dir = state_dir.join("runtime");
    ensure_plain_directory(&runtime_dir, "runtime directory")?;
    Ok(runtime_dir)
}

fn reject_untrusted_runtime_dirs(repo_root: &Path, include_sessions: bool) -> anyhow::Result<()> {
    let state_dir = repo_root.join(".stateful_core");
    reject_untrusted_directory_if_present(&state_dir, "stateful directory")?;
    let runtime_dir = state_dir.join("runtime");
    reject_untrusted_directory_if_present(&runtime_dir, "runtime directory")?;
    if include_sessions {
        reject_untrusted_directory_if_present(
            &runtime_dir.join("sessions"),
            "current session directory",
        )?;
    }
    Ok(())
}

fn reject_untrusted_directory_if_present(path: &Path, label: &str) -> anyhow::Result<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            anyhow::bail!("stateful refuses symlinked {label}");
        }
        Ok(metadata) if metadata.is_dir() => Ok(()),
        Ok(_) => anyhow::bail!("stateful {label} is not a directory"),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

fn ensure_plain_directory(path: &Path, label: &str) -> anyhow::Result<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            anyhow::bail!("stateful refuses symlinked {label}");
        }
        Ok(metadata) if metadata.is_dir() => Ok(()),
        Ok(_) => anyhow::bail!("stateful {label} is not a directory"),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            fs::create_dir(path)?;
            Ok(())
        }
        Err(error) => Err(error.into()),
    }
}

#[cfg(unix)]
fn is_hard_linked_file(metadata: &fs::Metadata) -> bool {
    metadata.is_file() && metadata.nlink() > 1
}

#[cfg(not(unix))]
fn is_hard_linked_file(_metadata: &fs::Metadata) -> bool {
    false
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

pub fn claim_intent_via_http(runtime: &ServerRuntime, args: IntentClaimArgs) -> anyhow::Result<()> {
    let body = intent_claim_protocol_body(runtime, args, "cli", "stateful-cli");

    let response = post_json(runtime, "/v1/intent/claim", &body)?;

    if !(200..300).contains(&response.status_code) {
        anyhow::bail!(
            "intent claim failed with HTTP {}: {}",
            response.status_code,
            response.body
        );
    }

    Ok(())
}

pub fn request_intent_via_http(
    runtime: &ServerRuntime,
    args: IntentRequestArgs,
) -> anyhow::Result<()> {
    let body = intent_request_protocol_body(runtime, args, "cli", "stateful-cli");

    let response = post_json(runtime, "/v1/intent/request", &body)?;

    if !(200..300).contains(&response.status_code) {
        anyhow::bail!(
            "intent request failed with HTTP {}: {}",
            response.status_code,
            response.body
        );
    }

    Ok(())
}

pub fn cancel_intent_via_http(
    runtime: &ServerRuntime,
    args: IntentCancelArgs,
) -> anyhow::Result<()> {
    let body = intent_cancel_protocol_body(runtime, args, "cli", "stateful-cli");

    let response = post_json(runtime, "/v1/intent/cancel", &body)?;

    if !(200..300).contains(&response.status_code) {
        anyhow::bail!(
            "intent cancel failed with HTTP {}: {}",
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
        purpose,
        files_planned,
        identity,
    } = args;
    protocol_envelope(ProtocolEnvelopeArgs {
        runtime,
        request_id: uuid::Uuid::new_v4().to_string(),
        session_id,
        workspace_id,
        identity,
        source_kind,
        event: "intent_declare",
        source_ref,
        source_tool_name: None,
        payload: serde_json::json!({
            "purpose": purpose,
            "files_planned": files_planned
        }),
    })
}

pub fn intent_claim_protocol_body(
    runtime: &ServerRuntime,
    args: IntentClaimArgs,
    source_kind: &str,
    source_ref: &str,
) -> serde_json::Value {
    let IntentClaimArgs {
        session_id,
        workspace_id,
        wait_id,
        identity,
    } = args;
    protocol_envelope(ProtocolEnvelopeArgs {
        runtime,
        request_id: uuid::Uuid::new_v4().to_string(),
        session_id,
        workspace_id,
        identity,
        source_kind,
        event: "intent_claim",
        source_ref,
        source_tool_name: None,
        payload: serde_json::json!({
            "wait_id": wait_id
        }),
    })
}

pub fn intent_request_protocol_body(
    runtime: &ServerRuntime,
    args: IntentRequestArgs,
    source_kind: &str,
    source_ref: &str,
) -> serde_json::Value {
    let IntentRequestArgs {
        session_id,
        workspace_id,
        request_id,
        action,
        path,
        purpose,
        identity,
    } = args;
    protocol_envelope(ProtocolEnvelopeArgs {
        runtime,
        request_id: uuid::Uuid::new_v4().to_string(),
        session_id,
        workspace_id,
        identity,
        source_kind,
        event: "intent_request",
        source_ref,
        source_tool_name: None,
        payload: serde_json::json!({
            "request_id": request_id,
            "action": action,
            "path": path,
            "purpose": purpose
        }),
    })
}

pub fn intent_cancel_protocol_body(
    runtime: &ServerRuntime,
    args: IntentCancelArgs,
    source_kind: &str,
    source_ref: &str,
) -> serde_json::Value {
    let IntentCancelArgs {
        session_id,
        workspace_id,
        request_id,
        identity,
    } = args;
    protocol_envelope(ProtocolEnvelopeArgs {
        runtime,
        request_id: uuid::Uuid::new_v4().to_string(),
        session_id,
        workspace_id,
        identity,
        source_kind,
        event: "intent_cancel",
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
    pub session_id: String,
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
        session_id,
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
        "source": source,
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

fn current_session_file_path_for_codex_run(
    repo_root: impl AsRef<Path>,
    codex_run_id: &str,
) -> anyhow::Result<std::path::PathBuf> {
    validate_codex_run_id(codex_run_id)?;
    Ok(repo_root
        .as_ref()
        .join(".stateful_core")
        .join("runtime")
        .join("sessions")
        .join(format!("{codex_run_id}.json")))
}

fn current_codex_run_id() -> anyhow::Result<Option<String>> {
    let Some(codex_run_id) = std::env::var_os(STATEFUL_CODEX_RUN_ID_ENV) else {
        return Ok(None);
    };
    let codex_run_id = codex_run_id.to_string_lossy().into_owned();
    validate_codex_run_id(&codex_run_id)?;
    Ok(Some(codex_run_id))
}

fn validate_codex_run_id(codex_run_id: &str) -> anyhow::Result<()> {
    if codex_run_id.is_empty() {
        anyhow::bail!("{STATEFUL_CODEX_RUN_ID_ENV} is set but empty");
    }
    if !codex_run_id
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
    {
        anyhow::bail!("{STATEFUL_CODEX_RUN_ID_ENV} contains unsupported characters");
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
    Ok(runtime_identity_matches_runtime(runtime, &identity))
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
        && identity.pid == runtime.pid
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
