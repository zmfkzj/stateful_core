use std::{
    fs,
    io::{Read, Write},
    path::Path,
};

use serde_json::Value;
use stateful_mcp::{ToolCall, map_tool_to_http, protocol_tool_name, tool_descriptors};

use crate::{
    CurrentSession, GlobalPaths, HttpResponse, IntentDeclareArgs, ProtocolEnvelopeArgs, RepoGate,
    RepoIdentity, ServerRuntime, discover_runtime_with_global, ensure_server, get_json,
    intent_declare_protocol_body, post_json, protocol_envelope, read_current_session_file,
    repo_gate, repo_identity_for_enabled_repo,
};

pub fn call_mcp_tool_in_repo(
    repo_root: impl AsRef<Path>,
    tool_name: impl Into<String>,
    arguments: Value,
) -> anyhow::Result<HttpResponse> {
    let start = repo_root.as_ref();
    let paths = GlobalPaths::from_env()?;
    let tool_name = tool_name.into();
    if matches!(tool_name.as_str(), "state_bash_write" | "state.bash.write") {
        return Ok(error_response(
            410,
            "state_bash_write was removed; use stateful sandbox run ... --command ...",
        ));
    }
    let protocol_name = protocol_tool_name(&tool_name).map_err(anyhow::Error::msg)?;
    let repo_root = match repo_gate(&paths, start)? {
        RepoGate::Enabled { repo_root } => {
            if let Some(response) =
                reject_mismatched_current_session(protocol_name, &arguments, &repo_root)
            {
                return Ok(response);
            }
            ensure_server(&paths)?;
            repo_root
        }
        RepoGate::Disabled | RepoGate::OutsideGitRepo => {
            return Ok(HttpResponse {
                status_code: 409,
                body: serde_json::json!({
                    "status": "error",
                    "message": "repo not enabled"
                })
                .to_string(),
            });
        }
    };
    let runtime = discover_runtime_with_global(&repo_root, &paths)?;
    call_mcp_tool(&runtime, &repo_root, &paths, tool_name, arguments)
}

fn call_mcp_tool(
    runtime: &ServerRuntime,
    repo_root: &Path,
    paths: &GlobalPaths,
    tool_name: impl Into<String>,
    arguments: Value,
) -> anyhow::Result<HttpResponse> {
    let tool_name = tool_name.into();
    let protocol_name = protocol_tool_name(&tool_name).map_err(anyhow::Error::msg)?;
    if protocol_name == "state.file.write" {
        return call_file_write_tool(runtime, repo_root, paths, arguments);
    }
    if let Some(response) = reject_mismatched_current_session(protocol_name, &arguments, repo_root)
    {
        return Ok(response);
    }

    let tool = ToolCall::new(
        protocol_name,
        enrich_arguments(protocol_name, arguments, runtime, repo_root, paths),
    );
    let request = map_tool_to_http(tool).map_err(anyhow::Error::msg)?;

    match request.method {
        "GET" => get_json(runtime, request.path),
        "POST" => {
            let body = if protocol_name == "state.intent.declare" {
                intent_declare_mcp_body(runtime, request.body)?
            } else {
                request.body
            };
            post_json(runtime, request.path, &body)
        }
        method => anyhow::bail!("unsupported MCP HTTP method: {method}"),
    }
}

fn call_file_write_tool(
    runtime: &ServerRuntime,
    repo_root: &Path,
    paths: &GlobalPaths,
    arguments: Value,
) -> anyhow::Result<HttpResponse> {
    let args = match serde_json::from_value::<FileWriteArguments>(arguments) {
        Ok(args) => args,
        Err(error) => {
            return Ok(error_response(
                400,
                format!("invalid state.file.write arguments: {error}"),
            ));
        }
    };
    let path = match normalize_repo_file_path(&args.path) {
        Ok(path) => path,
        Err(error) => return Ok(error_response(400, error.to_string())),
    };
    let current_session = read_current_session_file(repo_root).ok();
    if let Some(response) = reject_argument_session_mismatch(
        "state.file.write",
        args.session_id.as_deref(),
        args.workspace_id.as_deref(),
        current_session.as_ref(),
    ) {
        return Ok(response);
    }
    let session_id = match current_session
        .as_ref()
        .map(|session| session.session_id.clone())
        .or(args.session_id)
    {
        Some(session_id) => session_id,
        None => {
            return Ok(error_response(
                400,
                "state.file.write requires session_id or a current stateful session file",
            ));
        }
    };
    let workspace_id = current_session
        .map(|session| session.workspace_id)
        .or(args.workspace_id)
        .unwrap_or_else(|| runtime.workspace_id.clone());

    let response =
        authorize_file_write(runtime, repo_root, paths, &session_id, &workspace_id, &path)?;
    if !(200..300).contains(&response.status_code) {
        return Ok(response);
    }

    let decision = serde_json::from_str::<FileWriteAuthorizeDecision>(&response.body)?;
    if decision.decision != "allow" {
        return Ok(HttpResponse {
            status_code: 403,
            body: response.body,
        });
    }

    if let Err(error) = write_repo_file(repo_root, &path, &args.contents) {
        return Ok(error_response(500, error.to_string()));
    }

    Ok(HttpResponse {
        status_code: 200,
        body: serde_json::json!({
            "status": "ok",
            "path": path,
            "bytes": args.contents.len(),
        })
        .to_string(),
    })
}

fn reject_mismatched_current_session(
    protocol_name: &str,
    arguments: &Value,
    repo_root: &Path,
) -> Option<HttpResponse> {
    if !is_session_bound_mcp_tool(protocol_name) {
        return None;
    }
    let current_session = read_current_session_file(repo_root).ok()?;
    let object = arguments.as_object()?;
    reject_argument_session_mismatch(
        protocol_name,
        object.get("session_id").and_then(Value::as_str),
        object.get("workspace_id").and_then(Value::as_str),
        Some(&current_session),
    )
}

fn reject_argument_session_mismatch(
    tool_name: &str,
    session_id: Option<&str>,
    workspace_id: Option<&str>,
    current_session: Option<&CurrentSession>,
) -> Option<HttpResponse> {
    let current_session = current_session?;
    if let Some(session_id) = session_id
        && session_id != current_session.session_id
    {
        return Some(current_session_mismatch_response(
            tool_name,
            "session_id",
            session_id,
            &current_session.session_id,
        ));
    }
    if let Some(workspace_id) = workspace_id
        && workspace_id != current_session.workspace_id
    {
        return Some(current_session_mismatch_response(
            tool_name,
            "workspace_id",
            workspace_id,
            &current_session.workspace_id,
        ));
    }
    None
}

fn current_session_mismatch_response(
    tool_name: &str,
    field: &str,
    requested: &str,
    current: &str,
) -> HttpResponse {
    error_response(
        403,
        format!(
            "{tool_name} cannot use {field} `{requested}` while the current stateful session uses `{current}`"
        ),
    )
}

fn is_session_bound_mcp_tool(protocol_name: &str) -> bool {
    matches!(
        protocol_name,
        "state.session.register"
            | "state.session.heartbeat"
            | "state.intent.declare"
            | "state.lease.acquire"
            | "state.lease.release"
            | "state.activity.observe"
            | "state.activity.finalize"
            | "state.conflicts.check"
            | "state.reconcile.ack"
            | "state.file.write"
            | "state.notifications.poll"
            | "state.resume.next"
    )
}

fn authorize_file_write(
    runtime: &ServerRuntime,
    repo_root: &Path,
    paths: &GlobalPaths,
    session_id: &str,
    workspace_id: &str,
    path: &str,
) -> anyhow::Result<HttpResponse> {
    let body = protocol_envelope(ProtocolEnvelopeArgs {
        runtime,
        request_id: uuid::Uuid::new_v4().to_string(),
        session_id: session_id.to_string(),
        workspace_id: workspace_id.to_string(),
        identity: repo_identity_for_enabled_repo(paths, repo_root).ok(),
        source_kind: "mcp",
        event: "file_write",
        source_ref: "state.file.write",
        payload: serde_json::json!({
            "action": "write_file",
            "path": path,
            "queue_on_conflict": true,
        }),
    });

    post_json(runtime, "/v1/authorize", &body)
}

fn write_repo_file(repo_root: &Path, relative_path: &str, contents: &str) -> anyhow::Result<()> {
    ensure_repo_file_target(repo_root, relative_path)?;
    let target = repo_root.join(relative_path);
    let Some(parent) = target.parent() else {
        anyhow::bail!("state.file.write target has no parent directory");
    };
    fs::create_dir_all(parent)?;
    fs::write(target, contents)?;
    Ok(())
}

fn ensure_repo_file_target(repo_root: &Path, relative_path: &str) -> anyhow::Result<()> {
    let canonical_repo = repo_root
        .canonicalize()
        .unwrap_or_else(|_| repo_root.to_path_buf());
    let target = repo_root.join(relative_path);
    let Some(parent) = Path::new(relative_path).parent() else {
        anyhow::bail!("state.file.write target has no parent directory");
    };

    let mut cursor = repo_root.to_path_buf();
    for component in parent.components() {
        cursor.push(component);
        if let Ok(metadata) = fs::symlink_metadata(&cursor) {
            if metadata.file_type().is_symlink() {
                anyhow::bail!("state.file.write refuses symlinked parent directories");
            }
            if !metadata.is_dir() {
                anyhow::bail!("state.file.write parent path is not a directory");
            }
        }
    }

    if let Ok(metadata) = fs::symlink_metadata(&target) {
        if metadata.file_type().is_symlink() {
            anyhow::bail!("state.file.write refuses symlink file targets");
        }
        if metadata.is_dir() {
            anyhow::bail!("state.file.write target is a directory");
        }
    }

    if let Some(parent) = target.parent()
        && parent.exists()
    {
        let canonical_parent = parent.canonicalize()?;
        if !canonical_parent.starts_with(canonical_repo) {
            anyhow::bail!("state.file.write parent path escapes the repo");
        }
    }

    Ok(())
}

fn normalize_repo_file_path(path: &str) -> anyhow::Result<String> {
    let trimmed = path.trim();
    if trimmed.is_empty() {
        anyhow::bail!("state.file.write path is required");
    }
    if Path::new(trimmed).is_absolute() {
        anyhow::bail!("state.file.write path must be repo-relative");
    }

    let normalized = trimmed.replace('\\', "/");
    let mut segments = Vec::new();
    for segment in normalized.split('/') {
        if segment.is_empty() || segment == "." {
            continue;
        }
        if segment == ".." {
            anyhow::bail!("state.file.write path must stay inside the repo");
        }
        if is_git_internal_segment(segment) {
            anyhow::bail!("state.file.write refuses to write Git internals");
        }
        if segment.chars().any(char::is_control) {
            anyhow::bail!("state.file.write path must not contain control characters");
        }
        segments.push(segment);
    }

    if segments.is_empty() {
        anyhow::bail!("state.file.write path is required");
    }

    Ok(segments.join("/"))
}

fn error_response(status_code: u16, message: impl Into<String>) -> HttpResponse {
    HttpResponse {
        status_code,
        body: serde_json::json!({
            "status": "error",
            "message": message.into()
        })
        .to_string(),
    }
}

fn is_git_internal_segment(segment: &str) -> bool {
    segment.eq_ignore_ascii_case(".git")
}

fn intent_declare_mcp_body(runtime: &ServerRuntime, body: Value) -> anyhow::Result<Value> {
    let Value::Object(mut object) = body else {
        anyhow::bail!("state.intent.declare arguments must be an object");
    };

    let files_planned = object
        .remove("files_planned")
        .ok_or_else(|| anyhow::anyhow!("state.intent.declare requires files_planned"))
        .and_then(|value| serde_json::from_value::<Vec<String>>(value).map_err(Into::into))?;
    let session_id = take_string(&mut object, "session_id")
        .unwrap_or_else(|| format!("stateful-mcp:{}", runtime.pid));
    let workspace_id =
        take_string(&mut object, "workspace_id").unwrap_or_else(|| runtime.workspace_id.clone());
    let identity = repo_identity_from_object(&mut object);

    Ok(intent_declare_protocol_body(
        runtime,
        IntentDeclareArgs {
            session_id,
            workspace_id,
            files_planned,
            identity,
        },
        "mcp",
        "state.intent.declare",
    ))
}

fn repo_identity_from_object(object: &mut serde_json::Map<String, Value>) -> Option<RepoIdentity> {
    Some(RepoIdentity {
        repo_id: take_string(object, "repo_id")?,
        worktree_id: take_string(object, "worktree_id")?,
        root: take_string(object, "root")?,
        branch: take_string(object, "branch")?,
    })
}

fn take_string(object: &mut serde_json::Map<String, Value>, key: &str) -> Option<String> {
    object
        .remove(key)
        .and_then(|value| value.as_str().map(ToOwned::to_owned))
}

fn enrich_arguments(
    tool_name: &str,
    arguments: Value,
    runtime: &ServerRuntime,
    repo_root: &Path,
    paths: &GlobalPaths,
) -> Value {
    let Value::Object(mut object) = arguments else {
        return arguments;
    };

    if tool_name == "state.validation.run" {
        object
            .entry("workspace_id")
            .or_insert_with(|| Value::String(runtime.workspace_id.clone()));
        object
            .entry("repo_root")
            .or_insert_with(|| Value::String(repo_root.to_string_lossy().into_owned()));
    }
    if tool_name == "state.intent.declare" {
        if !object.contains_key("session_id")
            && let Ok(session) = read_current_session_file(repo_root)
        {
            object.insert("session_id".to_string(), Value::String(session.session_id));
            object
                .entry("workspace_id")
                .or_insert_with(|| Value::String(session.workspace_id));
        }
        object
            .entry("workspace_id")
            .or_insert_with(|| Value::String(runtime.workspace_id.clone()));
        add_repo_identity(&mut object, paths, repo_root);
    }

    Value::Object(object)
}

fn add_repo_identity(
    object: &mut serde_json::Map<String, Value>,
    paths: &GlobalPaths,
    repo_root: &Path,
) {
    let Ok(identity) = repo_identity_for_enabled_repo(paths, repo_root) else {
        return;
    };
    object
        .entry("repo_id")
        .or_insert_with(|| Value::String(identity.repo_id));
    object
        .entry("worktree_id")
        .or_insert_with(|| Value::String(identity.worktree_id));
    object
        .entry("root")
        .or_insert_with(|| Value::String(identity.root));
    object
        .entry("branch")
        .or_insert_with(|| Value::String(identity.branch));
}

#[derive(Debug, serde::Deserialize)]
struct FileWriteArguments {
    #[serde(default)]
    session_id: Option<String>,
    #[serde(default)]
    workspace_id: Option<String>,
    path: String,
    contents: String,
}

#[derive(Debug, serde::Deserialize)]
struct FileWriteAuthorizeDecision {
    decision: String,
}

pub fn handle_mcp_jsonrpc_in_repo(
    repo_root: impl AsRef<Path>,
    message: &str,
) -> anyhow::Result<Option<String>> {
    let repo_root = repo_root.as_ref();
    let request: Value = serde_json::from_str(message)?;
    let id = request.get("id").cloned().unwrap_or(Value::Null);
    let method = request
        .get("method")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow::anyhow!("MCP request missing method"))?;

    let response = match method {
        "initialize" => jsonrpc_result(
            id,
            serde_json::json!({
                "protocolVersion": "2024-11-05",
                "capabilities": {
                    "tools": {}
                },
                "serverInfo": {
                    "name": "stateful",
                    "version": env!("CARGO_PKG_VERSION")
                }
            }),
        ),
        "notifications/initialized" => return Ok(None),
        "tools/list" => jsonrpc_result(
            id,
            serde_json::json!({
                "tools": tool_descriptors()
                    .into_iter()
                    .map(|tool| serde_json::json!({
                        "name": tool.name,
                        "description": tool.description,
                        "inputSchema": tool.input_schema
                    }))
                    .collect::<Vec<_>>()
            }),
        ),
        "tools/call" => {
            let params = request
                .get("params")
                .cloned()
                .unwrap_or_else(|| serde_json::json!({}));
            let name = params
                .get("name")
                .and_then(Value::as_str)
                .ok_or_else(|| anyhow::anyhow!("MCP tools/call missing params.name"))?;
            let arguments = params
                .get("arguments")
                .cloned()
                .unwrap_or_else(|| serde_json::json!({}));
            let response = call_mcp_tool_in_repo(repo_root, name, arguments)?;
            jsonrpc_result(
                id,
                serde_json::json!({
                    "content": [{
                        "type": "text",
                        "text": response.body
                    }],
                    "isError": !(200..300).contains(&response.status_code)
                }),
            )
        }
        _ => jsonrpc_error(id, -32601, format!("unknown MCP method: {method}")),
    };

    Ok(Some(serde_json::to_string(&response)?))
}

pub fn serve_mcp_stdio_in_repo(
    repo_root: impl AsRef<Path>,
    mut input: impl Read,
    mut output: impl Write,
) -> anyhow::Result<()> {
    let repo_root = repo_root.as_ref();
    while let Some(message) = read_mcp_message(&mut input)? {
        if let Some(response) = handle_mcp_jsonrpc_in_repo(repo_root, &message.body)? {
            write_mcp_message(&mut output, &response, message.framing)?;
        }
    }

    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum McpFraming {
    ContentLength,
    JsonLine,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct McpMessage {
    body: String,
    framing: McpFraming,
}

fn jsonrpc_result(id: Value, result: Value) -> Value {
    serde_json::json!({
        "jsonrpc": "2.0",
        "id": id,
        "result": result
    })
}

fn jsonrpc_error(id: Value, code: i64, message: impl Into<String>) -> Value {
    serde_json::json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": {
            "code": code,
            "message": message.into()
        }
    })
}

fn read_mcp_message(input: &mut impl Read) -> anyhow::Result<Option<McpMessage>> {
    let Some(first_line) = read_line(input)? else {
        return Ok(None);
    };
    let first_line_trimmed = first_line.trim_end_matches(&['\r', '\n'][..]);

    if !first_line_trimmed
        .to_ascii_lowercase()
        .starts_with("content-length:")
    {
        return Ok(Some(McpMessage {
            body: first_line_trimmed.to_string(),
            framing: McpFraming::JsonLine,
        }));
    }

    let mut headers = first_line;
    loop {
        let Some(line) = read_line(input)? else {
            anyhow::bail!("unexpected EOF while reading MCP headers");
        };
        let is_blank = line == "\n" || line == "\r\n";
        headers.push_str(&line);
        if is_blank {
            break;
        }
    }

    let content_length = headers
        .lines()
        .find_map(|line| {
            line.to_ascii_lowercase()
                .strip_prefix("content-length:")
                .map(|value| value.trim().to_string())
        })
        .ok_or_else(|| anyhow::anyhow!("missing MCP Content-Length header"))?
        .parse::<usize>()?;
    let mut body = vec![0_u8; content_length];
    input.read_exact(&mut body)?;

    Ok(Some(McpMessage {
        body: String::from_utf8(body)?,
        framing: McpFraming::ContentLength,
    }))
}

fn read_line(input: &mut impl Read) -> anyhow::Result<Option<String>> {
    let mut line = Vec::new();
    let mut byte = [0_u8; 1];
    loop {
        let read = input.read(&mut byte)?;
        if read == 0 {
            return if line.is_empty() {
                Ok(None)
            } else {
                Ok(Some(String::from_utf8(line)?))
            };
        }
        line.push(byte[0]);
        if byte[0] == b'\n' {
            return Ok(Some(String::from_utf8(line)?));
        }
    }
}

fn write_mcp_message(
    output: &mut impl Write,
    message: &str,
    framing: McpFraming,
) -> anyhow::Result<()> {
    match framing {
        McpFraming::ContentLength => {
            write!(
                output,
                "Content-Length: {}\r\n\r\n{}",
                message.len(),
                message
            )?;
        }
        McpFraming::JsonLine => {
            writeln!(output, "{message}")?;
        }
    }
    output.flush()?;
    Ok(())
}
