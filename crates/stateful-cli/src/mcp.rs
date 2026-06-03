use std::{
    io::{Read, Write},
    path::Path,
};

use serde_json::Value;
use stateful_mcp::{ToolCall, map_tool_to_http, protocol_tool_name, tool_descriptors};

use crate::{
    GlobalPaths, HttpResponse, ProtocolPostContext, RepoGate, ServerRuntime,
    discover_runtime_with_global, ensure_server, get_json, post_json, post_protocol_json,
    read_current_session_file, repo_gate, repo_identity_for_enabled_repo,
};

pub fn call_mcp_tool_in_repo(
    repo_root: impl AsRef<Path>,
    tool_name: impl Into<String>,
    arguments: Value,
) -> anyhow::Result<HttpResponse> {
    let start = repo_root.as_ref();
    let paths = GlobalPaths::from_env()?;
    let repo_root = match repo_gate(&paths, start)? {
        RepoGate::Enabled { repo_root } => {
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
    let tool = ToolCall::new(
        protocol_name,
        enrich_arguments(protocol_name, arguments, runtime, repo_root, paths),
    );
    let request = map_tool_to_http(tool).map_err(anyhow::Error::msg)?;

    match request.method {
        "GET" => get_json(runtime, request.path),
        "POST" if protocol_required_http_path(request.path) => {
            let session_id = request
                .body
                .get("session_id")
                .and_then(Value::as_str)
                .unwrap_or("unknown")
                .to_string();
            let workspace_id = request
                .body
                .get("workspace_id")
                .and_then(Value::as_str)
                .unwrap_or(runtime.workspace_id.as_str())
                .to_string();
            let identity = repo_identity_for_enabled_repo(paths, repo_root).ok();
            let payload = protocol_payload(&request.body);
            post_protocol_json(
                runtime,
                request.path,
                ProtocolPostContext {
                    session_id: &session_id,
                    workspace_id: &workspace_id,
                    source_kind: "mcp",
                    source_event: protocol_name,
                    source_ref: "stateful mcp",
                    tool_name: Some(protocol_name),
                    identity: identity.as_ref(),
                },
                &payload,
            )
        }
        "POST" => post_json(runtime, request.path, &request.body),
        method => anyhow::bail!("unsupported MCP HTTP method: {method}"),
    }
}

fn protocol_required_http_path(path: &str) -> bool {
    matches!(
        path,
        "/v1/session/register"
            | "/v1/session/heartbeat"
            | "/v1/intent/declare"
            | "/v1/lease/acquire"
            | "/v1/lease/release"
            | "/v1/activity/observe"
            | "/v1/activity/finalize"
            | "/v1/conflicts/check"
            | "/v1/context/render"
            | "/v1/reconcile/ack"
            | "/v1/validation/run"
    )
}

fn protocol_payload(body: &Value) -> Value {
    let Value::Object(mut object) = body.clone() else {
        return body.clone();
    };

    for key in [
        "session_id",
        "workspace_id",
        "repo_id",
        "worktree_id",
        "root",
        "branch",
    ] {
        object.remove(key);
    }

    Value::Object(object)
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
    if tool_name == "state.context.render"
        && let Ok(session) = read_current_session_file(repo_root)
    {
        object
            .entry("session_id")
            .or_insert_with(|| Value::String(session.session_id));
        object
            .entry("workspace_id")
            .or_insert_with(|| Value::String(session.workspace_id));
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
