use serde_json::Value;

pub const CRATE_NAME: &str = "stateful-mcp";

const TOOLS: &[(&str, &str, &str)] = &[
    (
        "state_session_register",
        "state.session.register",
        "Register the active coding session with the state server.",
    ),
    (
        "state_session_heartbeat",
        "state.session.heartbeat",
        "Record a heartbeat for the active coding session.",
    ),
    (
        "state_intent_declare",
        "state.intent.declare",
        "Declare file or directory intent before write actions.",
    ),
    (
        "state_lease_acquire",
        "state.lease.acquire",
        "Acquire an advisory lease on a file or resource.",
    ),
    (
        "state_lease_release",
        "state.lease.release",
        "Release an advisory lease on a file or resource.",
    ),
    (
        "state_conflicts_check",
        "state.conflicts.check",
        "Dry-run an authorization or conflict check.",
    ),
    (
        "state_reconcile_ack",
        "state.reconcile.ack",
        "Acknowledge reconciliation after a human write conflict.",
    ),
    (
        "state_bash_write",
        "state.bash.write",
        "Run a write-capable Bash command in an OS sandbox after target authorization.",
    ),
    (
        "state_notifications_poll",
        "state.notifications.poll",
        "Poll pending coordination notifications for the active session.",
    ),
    (
        "state_resume_next",
        "state.resume.next",
        "Read the next reservation that can resume a blocked session.",
    ),
];

#[derive(Debug, Clone, PartialEq)]
pub struct ToolCall {
    pub name: String,
    pub arguments: Value,
}

impl ToolCall {
    pub fn new(name: impl Into<String>, arguments: Value) -> Self {
        Self {
            name: name.into(),
            arguments,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct HttpToolRequest {
    pub method: &'static str,
    pub path: &'static str,
    pub body: Value,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ToolDescriptor {
    pub name: &'static str,
    pub protocol_name: &'static str,
    pub description: &'static str,
    pub input_schema: Value,
}

pub fn tool_descriptors() -> Vec<ToolDescriptor> {
    TOOLS
        .iter()
        .map(|&(name, protocol_name, description)| ToolDescriptor {
            name,
            protocol_name,
            description,
            input_schema: input_schema_for(protocol_name),
        })
        .collect()
}

fn input_schema_for(protocol_name: &str) -> Value {
    match protocol_name {
        "state.session.register"
        | "state.session.heartbeat"
        | "state.notifications.poll"
        | "state.resume.next" => object_schema(
            [
                ("session_id", string_schema()),
                ("workspace_id", string_schema()),
            ],
            ["session_id", "workspace_id"],
        ),
        "state.intent.declare" => object_schema(
            [
                ("session_id", string_schema()),
                ("workspace_id", string_schema()),
                ("files_planned", string_array_schema()),
            ],
            ["files_planned"],
        ),
        "state.lease.acquire" | "state.lease.release" => object_schema(
            [
                ("session_id", string_schema()),
                ("workspace_id", string_schema()),
                ("path", string_schema()),
            ],
            ["session_id", "workspace_id", "path"],
        ),
        "state.conflicts.check" => object_schema(
            [
                ("session_id", string_schema()),
                ("workspace_id", string_schema()),
                (
                    "action",
                    serde_json::json!({
                        "type": "string",
                        "enum": ["write_file", "delete_file", "rename_file", "move_file"]
                    }),
                ),
                ("path", string_schema()),
                ("old_path", string_schema()),
                ("new_path", string_schema()),
            ],
            ["session_id", "action", "path"],
        ),
        "state.reconcile.ack" => object_schema(
            [
                ("session_id", string_schema()),
                ("workspace_id", string_schema()),
                (
                    "decision",
                    serde_json::json!({
                        "type": "string",
                        "enum": ["adopt", "reapply", "ask_user", "abandon"]
                    }),
                ),
                ("files_reread", string_array_schema()),
                ("human_change_summary", string_schema()),
            ],
            [
                "session_id",
                "workspace_id",
                "decision",
                "files_reread",
                "human_change_summary",
            ],
        ),
        "state.bash.write" => object_schema(
            [
                ("session_id", string_schema()),
                ("workspace_id", string_schema()),
                ("command", string_schema()),
                ("write_targets", string_array_schema()),
                ("create_targets", string_array_schema()),
                ("cwd", string_schema()),
                ("timeout_seconds", integer_schema()),
            ],
            ["command", "write_targets"],
        ),
        _ => empty_object_schema(),
    }
}

fn empty_object_schema() -> Value {
    serde_json::json!({
        "type": "object",
        "properties": {},
        "required": [],
        "additionalProperties": false
    })
}

fn object_schema<const P: usize, const R: usize>(
    properties: [(&'static str, Value); P],
    required: [&'static str; R],
) -> Value {
    let properties = properties
        .into_iter()
        .map(|(name, schema)| (name.to_string(), schema))
        .collect::<serde_json::Map<String, Value>>();
    let required = required.iter().copied().collect::<Vec<_>>();

    serde_json::json!({
        "type": "object",
        "properties": properties,
        "required": required,
        "additionalProperties": false
    })
}

fn string_schema() -> Value {
    serde_json::json!({ "type": "string" })
}

fn integer_schema() -> Value {
    serde_json::json!({ "type": "integer", "minimum": 1, "maximum": 600 })
}

fn string_array_schema() -> Value {
    serde_json::json!({
        "type": "array",
        "items": { "type": "string" }
    })
}

pub fn protocol_tool_name(name: &str) -> Result<&'static str, String> {
    TOOLS
        .iter()
        .find_map(|(tool_name, protocol_name, _)| {
            (*tool_name == name || *protocol_name == name).then_some(*protocol_name)
        })
        .ok_or_else(|| format!("unknown stateful MCP tool: {name}"))
}

pub fn map_tool_to_http(tool: ToolCall) -> Result<HttpToolRequest, String> {
    let protocol_name = protocol_tool_name(&tool.name)?;
    let (method, path) = match protocol_name {
        "state.session.register" => ("POST", "/v1/session/register"),
        "state.session.heartbeat" => ("POST", "/v1/session/heartbeat"),
        "state.intent.declare" => ("POST", "/v1/intent/declare"),
        "state.lease.acquire" => ("POST", "/v1/lease/acquire"),
        "state.lease.release" => ("POST", "/v1/lease/release"),
        "state.conflicts.check" => ("POST", "/v1/conflicts/check"),
        "state.reconcile.ack" => ("POST", "/v1/reconcile/ack"),
        "state.bash.write" => {
            return Err(
                "state.bash.write is handled locally by the stateful CLI MCP bridge".to_string(),
            );
        }
        "state.notifications.poll" => ("POST", "/v1/notifications/poll"),
        "state.resume.next" => ("POST", "/v1/resume/next"),
        unknown => return Err(format!("unknown stateful MCP tool: {unknown}")),
    };

    Ok(HttpToolRequest {
        method,
        path,
        body: tool.arguments,
    })
}
