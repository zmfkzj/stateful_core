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
        "state_intent_request",
        "state.intent.request",
        "Request a write reservation explicitly, returning queued or reserved state.",
    ),
    (
        "state_intent_claim",
        "state.intent.claim",
        "Claim an active reservation and turn it into write-authorizing intent.",
    ),
    (
        "state_intent_cancel",
        "state.intent.cancel",
        "Cancel a queued or reserved write reservation request owned by the session.",
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
        "state_activity_observe",
        "state.activity.observe",
        "Record observed session activity.",
    ),
    (
        "state_activity_finalize",
        "state.activity.finalize",
        "Finalize observed session activity.",
    ),
    (
        "state_conflicts_check",
        "state.conflicts.check",
        "Dry-run an authorization or conflict check.",
    ),
    (
        "state_current_read",
        "state.current.read",
        "Read the materialized current state summary.",
    ),
    (
        "state_events_read",
        "state.events.read",
        "Read recent stateful audit events.",
    ),
    (
        "state_context_render",
        "state.context.render",
        "Render current-state context for an agent prompt.",
    ),
    (
        "state_reconcile_ack",
        "state.reconcile.ack",
        "Acknowledge reconciliation after a human write conflict.",
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
        | "state.activity.observe"
        | "state.activity.finalize"
        | "state.notifications.poll"
        | "state.resume.next" => empty_object_schema(),
        "state.intent.declare" => object_schema(
            [
                (
                    "purpose",
                    string_schema_with_description(
                        "Required purpose inferred from the user or agent instruction when it is not explicit.",
                    ),
                ),
                ("files_planned", non_empty_string_array_schema()),
            ],
            ["purpose", "files_planned"],
        ),
        "state.intent.request" => object_schema(
            [
                ("request_id", string_schema()),
                (
                    "action",
                    serde_json::json!({
                        "type": "string",
                        "enum": ["write_file", "write_directory"]
                    }),
                ),
                ("path", non_empty_string_schema()),
                (
                    "purpose",
                    string_schema_with_description(
                        "Required purpose inferred from the user or agent instruction when it is not explicit.",
                    ),
                ),
            ],
            ["request_id", "action", "path", "purpose"],
        ),
        "state.intent.claim" => object_schema([("wait_id", string_schema())], ["wait_id"]),
        "state.intent.cancel" => object_schema([("request_id", string_schema())], ["request_id"]),
        "state.lease.acquire" | "state.lease.release" => {
            object_schema([("path", string_schema())], ["path"])
        }
        "state.conflicts.check" => object_schema(
            [
                (
                    "action",
                    serde_json::json!({
                        "type": "string",
                        "enum": [
                            "write_file",
                            "write_directory",
                            "delete_file",
                            "rename_file",
                            "move_file"
                        ]
                    }),
                ),
                ("path", string_schema()),
                ("old_path", string_schema()),
                ("new_path", string_schema()),
            ],
            ["action", "path"],
        ),
        "state.current.read" | "state.events.read" => empty_object_schema(),
        "state.context.render" => object_schema(
            [
                (
                    "mode",
                    serde_json::json!({
                        "type": "string",
                        "enum": ["brief", "detailed"]
                    }),
                ),
                ("resource", string_schema()),
            ],
            [],
        ),
        "state.reconcile.ack" => object_schema(
            [
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
            ["decision", "files_reread", "human_change_summary"],
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

fn string_schema_with_description(description: &str) -> Value {
    serde_json::json!({
        "type": "string",
        "minLength": 1,
        "description": description
    })
}

fn non_empty_string_schema() -> Value {
    serde_json::json!({ "type": "string", "minLength": 1 })
}

fn string_array_schema() -> Value {
    serde_json::json!({
        "type": "array",
        "items": { "type": "string" }
    })
}

fn non_empty_string_array_schema() -> Value {
    serde_json::json!({
        "type": "array",
        "minItems": 1,
        "items": { "type": "string", "minLength": 1 }
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
        "state.intent.request" => ("POST", "/v1/intent/request"),
        "state.intent.claim" => ("POST", "/v1/intent/claim"),
        "state.intent.cancel" => ("POST", "/v1/intent/cancel"),
        "state.lease.acquire" => ("POST", "/v1/lease/acquire"),
        "state.lease.release" => ("POST", "/v1/lease/release"),
        "state.activity.observe" => ("POST", "/v1/activity/observe"),
        "state.activity.finalize" => ("POST", "/v1/activity/finalize"),
        "state.conflicts.check" => ("POST", "/v1/conflicts/check"),
        "state.current.read" => ("GET", "/v1/current"),
        "state.events.read" => ("GET", "/v1/events"),
        "state.context.render" => ("POST", "/v1/context/render"),
        "state.reconcile.ack" => ("POST", "/v1/reconcile/ack"),
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
