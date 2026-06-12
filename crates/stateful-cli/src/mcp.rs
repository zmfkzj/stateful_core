use std::{
    io::{Read, Write},
    path::Path,
};

use serde_json::Value;
use stateful_mcp::{ToolCall, map_tool_to_http, protocol_tool_name, tool_descriptors};

use crate::runtime::read_current_session_file_for_mcp;
use crate::{
    CurrentSession, GlobalPaths, HttpResponse, IntentCancelArgs, IntentClaimArgs,
    IntentDeclareArgs, IntentRequestArgs, RepoGate, RepoIdentity, ServerRuntime,
    discover_runtime_with_global, effective_workspace_id_for_repo, ensure_server, get_json,
    intent_cancel_protocol_body, intent_claim_protocol_body, intent_declare_protocol_body,
    intent_request_protocol_body, post_json, repo_gate, repo_identity_for_enabled_repo,
    runtime_env_override_is_configured,
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
    if matches!(tool_name.as_str(), "state_file_write" | "state.file.write") {
        return Ok(error_response(
            410,
            "state_file_write was removed; use native Codex edit tools such as apply_patch or Edit after exact intent declaration and a successful same-session file lease.",
        ));
    }
    let protocol_name = protocol_tool_name(&tool_name).map_err(anyhow::Error::msg)?;
    match repo_gate(&paths, start)? {
        RepoGate::Enabled { repo_root } => {
            let current_session = match current_session_for_mcp_tool(protocol_name, &repo_root) {
                Ok(current_session) => current_session,
                Err(response) => return Ok(response),
            };
            if let Some(response) = reject_mismatched_current_session(
                protocol_name,
                &arguments,
                current_session.as_ref(),
            ) {
                return Ok(response);
            }
            if !runtime_env_override_is_configured() {
                ensure_server(&paths)?;
            }
            let runtime = discover_runtime_with_global(&repo_root, &paths)?;
            call_mcp_tool(
                &runtime,
                &repo_root,
                &paths,
                tool_name,
                arguments,
                current_session.as_ref(),
            )
        }
        RepoGate::Disabled | RepoGate::OutsideGitRepo => Ok(HttpResponse {
            status_code: 409,
            body: serde_json::json!({
                "status": "error",
                "message": "repo not enabled"
            })
            .to_string(),
        }),
    }
}

fn call_mcp_tool(
    runtime: &ServerRuntime,
    repo_root: &Path,
    paths: &GlobalPaths,
    tool_name: impl Into<String>,
    arguments: Value,
    current_session: Option<&CurrentSession>,
) -> anyhow::Result<HttpResponse> {
    let tool_name = tool_name.into();
    let protocol_name = protocol_tool_name(&tool_name).map_err(anyhow::Error::msg)?;

    let tool = ToolCall::new(
        protocol_name,
        enrich_arguments(
            protocol_name,
            arguments,
            runtime,
            repo_root,
            paths,
            current_session,
        ),
    );
    let request = map_tool_to_http(tool).map_err(anyhow::Error::msg)?;

    match request.method {
        "GET" => get_json(runtime, request.path),
        "POST" => {
            let body = if protocol_name == "state.intent.declare" {
                intent_declare_mcp_body(runtime, request.body)?
            } else if protocol_name == "state.intent.request" {
                intent_request_mcp_body(runtime, request.body)?
            } else if protocol_name == "state.intent.claim" {
                intent_claim_mcp_body(runtime, request.body)?
            } else if protocol_name == "state.intent.cancel" {
                intent_cancel_mcp_body(runtime, request.body)?
            } else {
                request.body
            };
            post_json(runtime, request.path, &body)
        }
        method => anyhow::bail!("unsupported MCP HTTP method: {method}"),
    }
}

fn current_session_for_mcp_tool(
    protocol_name: &str,
    repo_root: &Path,
) -> Result<Option<CurrentSession>, HttpResponse> {
    if !is_session_bound_mcp_tool(protocol_name) {
        return Ok(None);
    }

    read_current_session_file_for_mcp(repo_root)
        .map(Some)
        .map_err(|error| current_session_resolution_response(protocol_name, error.to_string()))
}

fn reject_mismatched_current_session(
    protocol_name: &str,
    arguments: &Value,
    current_session: Option<&CurrentSession>,
) -> Option<HttpResponse> {
    if !is_session_bound_mcp_tool(protocol_name) {
        return None;
    }
    let object = arguments.as_object()?;
    reject_argument_session_mismatch(
        protocol_name,
        object.get("session_id").and_then(Value::as_str),
        object.get("workspace_id").and_then(Value::as_str),
        current_session,
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

fn current_session_resolution_response(tool_name: &str, error: String) -> HttpResponse {
    error_response(
        403,
        format!("{tool_name} cannot resolve current stateful session: {error}"),
    )
}

fn is_session_bound_mcp_tool(protocol_name: &str) -> bool {
    matches!(
        protocol_name,
        "state.session.register"
            | "state.session.heartbeat"
            | "state.intent.declare"
            | "state.intent.request"
            | "state.intent.claim"
            | "state.intent.cancel"
            | "state.lease.acquire"
            | "state.lease.release"
            | "state.activity.observe"
            | "state.activity.finalize"
            | "state.context.render"
            | "state.conflicts.check"
            | "state.reconcile.ack"
            | "state.notifications.poll"
            | "state.resume.next"
    )
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

fn intent_declare_mcp_body(runtime: &ServerRuntime, body: Value) -> anyhow::Result<Value> {
    let Value::Object(mut object) = body else {
        anyhow::bail!("state.intent.declare arguments must be an object");
    };

    let files_planned = object
        .remove("files_planned")
        .ok_or_else(|| anyhow::anyhow!("state.intent.declare requires files_planned"))
        .and_then(|value| serde_json::from_value::<Vec<String>>(value).map_err(Into::into))?;
    let purpose = take_string(&mut object, "purpose")
        .ok_or_else(|| anyhow::anyhow!("state.intent.declare requires purpose"))?;
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
            purpose,
            files_planned,
            identity,
        },
        "mcp",
        "state.intent.declare",
    ))
}

fn intent_claim_mcp_body(runtime: &ServerRuntime, body: Value) -> anyhow::Result<Value> {
    let Value::Object(mut object) = body else {
        anyhow::bail!("state.intent.claim arguments must be an object");
    };

    let wait_id = take_string(&mut object, "wait_id")
        .ok_or_else(|| anyhow::anyhow!("state.intent.claim requires wait_id"))?;
    let session_id = take_string(&mut object, "session_id")
        .unwrap_or_else(|| format!("stateful-mcp:{}", runtime.pid));
    let workspace_id =
        take_string(&mut object, "workspace_id").unwrap_or_else(|| runtime.workspace_id.clone());
    let identity = repo_identity_from_object(&mut object);

    Ok(intent_claim_protocol_body(
        runtime,
        IntentClaimArgs {
            session_id,
            workspace_id,
            wait_id,
            identity,
        },
        "mcp",
        "state.intent.claim",
    ))
}

fn intent_request_mcp_body(runtime: &ServerRuntime, body: Value) -> anyhow::Result<Value> {
    let Value::Object(mut object) = body else {
        anyhow::bail!("state.intent.request arguments must be an object");
    };

    let request_id = take_string(&mut object, "request_id")
        .ok_or_else(|| anyhow::anyhow!("state.intent.request requires request_id"))?;
    let action = take_string(&mut object, "action")
        .ok_or_else(|| anyhow::anyhow!("state.intent.request requires action"))?;
    let path = take_string(&mut object, "path")
        .ok_or_else(|| anyhow::anyhow!("state.intent.request requires path"))?;
    let purpose = take_string(&mut object, "purpose")
        .ok_or_else(|| anyhow::anyhow!("state.intent.request requires purpose"))?;
    let session_id = take_string(&mut object, "session_id")
        .unwrap_or_else(|| format!("stateful-mcp:{}", runtime.pid));
    let workspace_id =
        take_string(&mut object, "workspace_id").unwrap_or_else(|| runtime.workspace_id.clone());
    let identity = repo_identity_from_object(&mut object);

    Ok(intent_request_protocol_body(
        runtime,
        IntentRequestArgs {
            session_id,
            workspace_id,
            request_id,
            action,
            path,
            purpose,
            identity,
        },
        "mcp",
        "state.intent.request",
    ))
}

fn intent_cancel_mcp_body(runtime: &ServerRuntime, body: Value) -> anyhow::Result<Value> {
    let Value::Object(mut object) = body else {
        anyhow::bail!("state.intent.cancel arguments must be an object");
    };

    let request_id = take_string(&mut object, "request_id")
        .ok_or_else(|| anyhow::anyhow!("state.intent.cancel requires request_id"))?;
    let session_id = take_string(&mut object, "session_id")
        .unwrap_or_else(|| format!("stateful-mcp:{}", runtime.pid));
    let workspace_id =
        take_string(&mut object, "workspace_id").unwrap_or_else(|| runtime.workspace_id.clone());
    let identity = repo_identity_from_object(&mut object);

    Ok(intent_cancel_protocol_body(
        runtime,
        IntentCancelArgs {
            session_id,
            workspace_id,
            request_id,
            identity,
        },
        "mcp",
        "state.intent.cancel",
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
    current_session: Option<&CurrentSession>,
) -> Value {
    let Value::Object(mut object) = arguments else {
        return arguments;
    };

    if is_session_bound_mcp_tool(tool_name)
        && let Some(session) = current_session
    {
        object.insert(
            "session_id".to_string(),
            Value::String(session.session_id.clone()),
        );
        object.insert(
            "workspace_id".to_string(),
            Value::String(session.workspace_id.clone()),
        );
    }

    if matches!(
        tool_name,
        "state.intent.declare"
            | "state.intent.request"
            | "state.intent.claim"
            | "state.intent.cancel"
            | "state.lease.acquire"
            | "state.context.render"
    ) {
        let identity = repo_identity_for_enabled_repo(paths, repo_root).ok();
        object.entry("workspace_id").or_insert_with(|| {
            Value::String(effective_workspace_id_for_repo(
                &runtime.workspace_id,
                identity.as_ref(),
            ))
        });
        if let Some(identity) = identity {
            add_repo_identity(&mut object, identity);
        }
    }

    Value::Object(object)
}

fn add_repo_identity(object: &mut serde_json::Map<String, Value>, identity: RepoIdentity) {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::enable_repo;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn enrich_arguments_derives_workspace_for_default_shared_runtime() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time should be after unix epoch")
            .as_nanos();
        let temp_root = std::env::temp_dir().join(format!(
            "stateful-mcp-enrich-workspace-{}-{unique}",
            std::process::id()
        ));
        if temp_root.exists() {
            fs::remove_dir_all(&temp_root).expect("old temp root should be removable");
        }
        let repo_root = temp_root.join("repo");
        fs::create_dir_all(repo_root.join(".git")).expect("repo marker should be creatable");
        let paths = GlobalPaths::new(temp_root.join("home"));
        enable_repo(&paths, &repo_root).expect("repo should enable");
        let runtime = ServerRuntime::new("http://127.0.0.1:9", "secret-token", "shared", 42);

        let enriched = enrich_arguments(
            "state.intent.declare",
            serde_json::json!({
                "session_id": "s1",
                "purpose": "Fix auth validation behavior.",
                "files_planned": ["src/auth.ts"]
            }),
            &runtime,
            &repo_root,
            &paths,
            None,
        );

        let workspace_id = enriched["workspace_id"]
            .as_str()
            .expect("workspace_id should be present");
        assert!(workspace_id.starts_with("workspace-"));
        assert_ne!(workspace_id, "shared");
        assert!(
            enriched["repo_id"]
                .as_str()
                .is_some_and(|repo_id| repo_id.starts_with("repo-"))
        );

        fs::remove_dir_all(&temp_root).expect("temp root should be removable");
    }
}
