use stateful_mcp::{ToolCall, map_tool_to_http, protocol_tool_name, tool_descriptors};

fn descriptor(name: &str) -> stateful_mcp::ToolDescriptor {
    tool_descriptors()
        .into_iter()
        .find(|tool| tool.name == name)
        .unwrap_or_else(|| panic!("{name} tool descriptor should exist"))
}

fn assert_injected_session_fields_are_hidden(schema: &serde_json::Value) {
    let properties = schema["properties"]
        .as_object()
        .expect("schema properties should be an object");
    assert!(!properties.contains_key("session_id"));
    assert!(!properties.contains_key("workspace_id"));

    let required = schema["required"]
        .as_array()
        .expect("schema required should be an array");
    assert!(
        !required
            .iter()
            .any(|value| value.as_str() == Some("session_id"))
    );
    assert!(
        !required
            .iter()
            .any(|value| value.as_str() == Some("workspace_id"))
    );
}

#[test]
fn context_render_tool_maps_to_http_endpoint() {
    let tool = ToolCall::new("state.context.render", serde_json::json!({"mode": "brief"}));

    let request = map_tool_to_http(tool).expect("context tool should map");

    assert_eq!(request.method, "POST");
    assert_eq!(request.path, "/v1/context/render");
}

#[test]
fn current_read_tool_maps_to_get_endpoint() {
    let tool = ToolCall::new("state.current.read", serde_json::json!({}));

    let request = map_tool_to_http(tool).expect("current tool should map");

    assert_eq!(request.method, "GET");
    assert_eq!(request.path, "/v1/current");
}

#[test]
fn bash_write_tool_is_removed_from_mcp_surface() {
    assert!(protocol_tool_name("state_bash_write").is_err());
    assert!(protocol_tool_name("state.bash.write").is_err());

    let names = tool_descriptors()
        .into_iter()
        .map(|tool| tool.name)
        .collect::<Vec<_>>();
    assert!(!names.contains(&"state_bash_write"));
}

#[test]
fn all_v1_mcp_tools_map_to_http_endpoints() {
    let cases = [
        ("state.session.register", "POST", "/v1/session/register"),
        ("state.session.heartbeat", "POST", "/v1/session/heartbeat"),
        ("state.intent.declare", "POST", "/v1/intent/declare"),
        ("state.intent.request", "POST", "/v1/intent/request"),
        ("state.intent.claim", "POST", "/v1/intent/claim"),
        ("state.intent.cancel", "POST", "/v1/intent/cancel"),
        ("state.lease.acquire", "POST", "/v1/lease/acquire"),
        ("state.lease.release", "POST", "/v1/lease/release"),
        ("state.activity.observe", "POST", "/v1/activity/observe"),
        ("state.activity.finalize", "POST", "/v1/activity/finalize"),
        ("state.conflicts.check", "POST", "/v1/conflicts/check"),
        ("state.current.read", "GET", "/v1/current"),
        ("state.events.read", "GET", "/v1/events"),
        ("state.context.render", "POST", "/v1/context/render"),
        ("state.reconcile.ack", "POST", "/v1/reconcile/ack"),
        ("state.notifications.poll", "POST", "/v1/notifications/poll"),
        ("state.resume.next", "POST", "/v1/resume/next"),
    ];

    for (tool_name, method, path) in cases {
        let request = map_tool_to_http(ToolCall::new(tool_name, serde_json::json!({})))
            .unwrap_or_else(|error| panic!("{tool_name} should map: {error}"));

        assert_eq!(request.method, method, "{tool_name} method");
        assert_eq!(request.path, path, "{tool_name} path");
    }
}

#[test]
fn codex_tool_names_map_to_stateful_protocol_names() {
    assert_eq!(
        protocol_tool_name("state_intent_declare").expect("tool should map"),
        "state.intent.declare"
    );
    assert_eq!(
        protocol_tool_name("state_intent_claim").expect("tool should map"),
        "state.intent.claim"
    );
    assert_eq!(
        protocol_tool_name("state_intent_request").expect("tool should map"),
        "state.intent.request"
    );
    assert_eq!(
        protocol_tool_name("state_intent_cancel").expect("tool should map"),
        "state.intent.cancel"
    );
    assert_eq!(
        protocol_tool_name("state_current_read").expect("tool should map"),
        "state.current.read"
    );
}

#[test]
fn tool_descriptors_expose_codex_friendly_names() {
    let tools = tool_descriptors();
    let names = tools.iter().map(|tool| tool.name).collect::<Vec<_>>();

    assert!(names.contains(&"state_intent_declare"));
    assert!(names.contains(&"state_intent_request"));
    assert!(names.contains(&"state_intent_claim"));
    assert!(names.contains(&"state_intent_cancel"));
    assert!(names.contains(&"state_current_read"));
    assert!(names.contains(&"state_events_read"));
    assert!(!names.contains(&"state_file_write"));
    assert!(!names.contains(&"state_bash_write"));
    assert!(!names.contains(&"state_validation_run"));
    assert!(names.contains(&"state_notifications_poll"));
    assert!(names.contains(&"state_resume_next"));
}

#[test]
fn validation_tool_is_removed_from_mcp_surface() {
    assert!(protocol_tool_name("state_validation_run").is_err());
    assert!(protocol_tool_name("state.validation.run").is_err());

    let tool = ToolCall::new(
        "state.validation.run",
        serde_json::json!({"profile": "unit"}),
    );
    assert!(map_tool_to_http(tool).is_err());
}

#[test]
fn file_write_tool_is_removed_from_mcp_surface() {
    assert!(protocol_tool_name("state_file_write").is_err());
    assert!(protocol_tool_name("state.file.write").is_err());

    let names = tool_descriptors()
        .into_iter()
        .map(|tool| tool.name)
        .collect::<Vec<_>>();
    assert!(
        !names.contains(&"state_file_write"),
        "state_file_write should be replaced by native Codex edit tools"
    );

    let tool = ToolCall::new(
        "state.file.write",
        serde_json::json!({"path": "src/auth.ts", "contents": ""}),
    );
    assert!(map_tool_to_http(tool).is_err());
}

#[test]
fn session_bound_descriptor_schemas_hide_injected_session_fields() {
    for name in [
        "state_session_register",
        "state_session_heartbeat",
        "state_intent_declare",
        "state_intent_request",
        "state_intent_claim",
        "state_intent_cancel",
        "state_lease_acquire",
        "state_lease_release",
        "state_activity_observe",
        "state_activity_finalize",
        "state_conflicts_check",
        "state_reconcile_ack",
        "state_notifications_poll",
        "state_resume_next",
    ] {
        let tool = descriptor(name);

        assert_injected_session_fields_are_hidden(&tool.input_schema);
    }
}

#[test]
fn run_bound_descriptor_schemas_are_empty() {
    let empty_schema = serde_json::json!({
        "type": "object",
        "properties": {},
        "required": [],
        "additionalProperties": false
    });

    for name in [
        "state_session_register",
        "state_session_heartbeat",
        "state_activity_observe",
        "state_activity_finalize",
        "state_notifications_poll",
        "state_resume_next",
    ] {
        let tool = descriptor(name);

        assert_eq!(tool.input_schema, empty_schema, "{name} schema");
    }
}

#[test]
fn intent_declare_descriptor_exposes_required_input_schema() {
    let tool = descriptor("state_intent_declare");

    assert_eq!(tool.input_schema["type"], "object");
    assert_injected_session_fields_are_hidden(&tool.input_schema);
    assert_eq!(tool.input_schema["properties"]["purpose"]["type"], "string");
    assert_eq!(tool.input_schema["properties"]["purpose"]["minLength"], 1);
    assert_eq!(
        tool.input_schema["properties"]["files_planned"]["type"],
        "array"
    );
    assert_eq!(
        tool.input_schema["properties"]["files_planned"]["items"]["type"],
        "string"
    );
    assert_eq!(
        tool.input_schema["properties"]["files_planned"]["items"]["minLength"],
        1
    );
    assert_eq!(
        tool.input_schema["properties"]["files_planned"]["minItems"],
        1
    );
    assert_eq!(
        tool.input_schema["required"],
        serde_json::json!(["purpose", "files_planned"])
    );
    assert_eq!(tool.input_schema["additionalProperties"], false);
}

#[test]
fn reconcile_ack_descriptor_does_not_require_non_empty_files_reread() {
    let tool = descriptor("state_reconcile_ack");

    assert_injected_session_fields_are_hidden(&tool.input_schema);
    assert_eq!(
        tool.input_schema["properties"]["decision"]["enum"],
        serde_json::json!(["adopt", "reapply", "ask_user", "abandon"])
    );
    assert_eq!(
        tool.input_schema["properties"]["files_reread"]["type"],
        "array"
    );
    assert_eq!(
        tool.input_schema["properties"]["files_reread"]["items"]["type"],
        "string"
    );
    assert!(
        tool.input_schema["properties"]["files_reread"]
            .get("minItems")
            .is_none()
    );
    assert_eq!(
        tool.input_schema["properties"]["human_change_summary"]["type"],
        "string"
    );
    assert_eq!(
        tool.input_schema["required"],
        serde_json::json!(["decision", "files_reread", "human_change_summary"])
    );
    assert_eq!(tool.input_schema["additionalProperties"], false);
}

#[test]
fn intent_claim_descriptor_exposes_required_input_schema() {
    let tool = descriptor("state_intent_claim");

    assert_eq!(tool.input_schema["type"], "object");
    assert_injected_session_fields_are_hidden(&tool.input_schema);
    assert_eq!(tool.input_schema["properties"]["wait_id"]["type"], "string");
    assert_eq!(
        tool.input_schema["required"],
        serde_json::json!(["wait_id"])
    );
    assert_eq!(tool.input_schema["additionalProperties"], false);
}

#[test]
fn intent_request_descriptor_exposes_required_input_schema() {
    let tool = descriptor("state_intent_request");

    assert_eq!(tool.input_schema["type"], "object");
    assert_injected_session_fields_are_hidden(&tool.input_schema);
    assert_eq!(
        tool.input_schema["properties"]["request_id"]["type"],
        "string"
    );
    assert_eq!(
        tool.input_schema["properties"]["action"]["enum"],
        serde_json::json!(["write_file", "write_directory"])
    );
    assert_eq!(tool.input_schema["properties"]["path"]["type"], "string");
    assert_eq!(tool.input_schema["properties"]["path"]["minLength"], 1);
    assert_eq!(tool.input_schema["properties"]["purpose"]["type"], "string");
    assert_eq!(tool.input_schema["properties"]["purpose"]["minLength"], 1);
    assert_eq!(
        tool.input_schema["required"],
        serde_json::json!(["request_id", "action", "path", "purpose"])
    );
    assert_eq!(tool.input_schema["additionalProperties"], false);
}

#[test]
fn intent_cancel_descriptor_exposes_required_input_schema() {
    let tool = descriptor("state_intent_cancel");

    assert_eq!(tool.input_schema["type"], "object");
    assert_injected_session_fields_are_hidden(&tool.input_schema);
    assert_eq!(
        tool.input_schema["properties"]["request_id"]["type"],
        "string"
    );
    assert_eq!(
        tool.input_schema["required"],
        serde_json::json!(["request_id"])
    );
    assert_eq!(tool.input_schema["additionalProperties"], false);
}

#[test]
fn lease_descriptors_expose_path_only_schema() {
    for name in ["state_lease_acquire", "state_lease_release"] {
        let tool = descriptor(name);

        assert_eq!(tool.input_schema["type"], "object");
        assert_injected_session_fields_are_hidden(&tool.input_schema);
        assert_eq!(tool.input_schema["properties"]["path"]["type"], "string");
        assert_eq!(
            tool.input_schema["required"],
            serde_json::json!(["path"]),
            "{name} required"
        );
        assert_eq!(tool.input_schema["additionalProperties"], false);
    }
}

#[test]
fn conflicts_check_descriptor_exposes_required_input_schema() {
    let tool = descriptor("state_conflicts_check");

    assert_eq!(tool.input_schema["type"], "object");
    assert_injected_session_fields_are_hidden(&tool.input_schema);
    assert_eq!(
        tool.input_schema["properties"]["action"]["enum"],
        serde_json::json!([
            "write_file",
            "write_directory",
            "delete_file",
            "rename_file",
            "move_file"
        ])
    );
    assert_eq!(tool.input_schema["properties"]["path"]["type"], "string");
    assert_eq!(
        tool.input_schema["properties"]["old_path"]["type"],
        "string"
    );
    assert_eq!(
        tool.input_schema["properties"]["new_path"]["type"],
        "string"
    );
    assert_eq!(
        tool.input_schema["required"],
        serde_json::json!(["action", "path"])
    );
    assert_eq!(tool.input_schema["additionalProperties"], false);
}
