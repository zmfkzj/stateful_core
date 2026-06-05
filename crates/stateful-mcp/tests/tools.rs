use stateful_mcp::{ToolCall, map_tool_to_http, protocol_tool_name, tool_descriptors};

#[test]
fn validation_tool_maps_to_http_endpoint() {
    let tool = ToolCall::new(
        "state.validation.run",
        serde_json::json!({"profile": "unit"}),
    );

    let request = map_tool_to_http(tool).expect("validation tool should map");

    assert_eq!(request.method, "POST");
    assert_eq!(request.path, "/v1/validation/run");
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
fn file_write_tool_is_handled_locally_not_mapped_to_http() {
    let tool = ToolCall::new(
        "state.file.write",
        serde_json::json!({"path": "src/auth.ts", "contents": ""}),
    );

    let error = map_tool_to_http(tool).expect_err("file write is CLI-local");

    assert!(error.contains("handled locally"));
}

#[test]
fn all_v1_mcp_tools_map_to_http_endpoints() {
    let cases = [
        ("state.session.register", "POST", "/v1/session/register"),
        ("state.session.heartbeat", "POST", "/v1/session/heartbeat"),
        ("state.intent.declare", "POST", "/v1/intent/declare"),
        ("state.lease.acquire", "POST", "/v1/lease/acquire"),
        ("state.lease.release", "POST", "/v1/lease/release"),
        ("state.activity.observe", "POST", "/v1/activity/observe"),
        ("state.activity.finalize", "POST", "/v1/activity/finalize"),
        ("state.conflicts.check", "POST", "/v1/conflicts/check"),
        ("state.current.read", "GET", "/v1/current"),
        ("state.events.read", "GET", "/v1/events"),
        ("state.context.render", "POST", "/v1/context/render"),
        ("state.reconcile.ack", "POST", "/v1/reconcile/ack"),
        ("state.validation.run", "POST", "/v1/validation/run"),
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
        protocol_tool_name("state_current_read").expect("tool should map"),
        "state.current.read"
    );
}

#[test]
fn tool_descriptors_expose_codex_friendly_names() {
    let tools = tool_descriptors();
    let names = tools.iter().map(|tool| tool.name).collect::<Vec<_>>();

    assert!(names.contains(&"state_intent_declare"));
    assert!(names.contains(&"state_current_read"));
    assert!(names.contains(&"state_events_read"));
    assert!(names.contains(&"state_validation_run"));
    assert!(names.contains(&"state_file_write"));
    assert!(names.contains(&"state_notifications_poll"));
    assert!(names.contains(&"state_resume_next"));
}

#[test]
fn file_write_descriptor_exposes_required_input_schema() {
    let tools = tool_descriptors();
    let tool = tools
        .iter()
        .find(|tool| tool.name == "state_file_write")
        .expect("file write tool descriptor should exist");

    assert_eq!(tool.protocol_name, "state.file.write");
    assert_eq!(tool.input_schema["type"], "object");
    assert_eq!(tool.input_schema["properties"]["path"]["type"], "string");
    assert_eq!(
        tool.input_schema["properties"]["contents"]["type"],
        "string"
    );
    assert_eq!(
        tool.input_schema["required"],
        serde_json::json!(["path", "contents"])
    );
    assert_eq!(tool.input_schema["additionalProperties"], false);
}

#[test]
fn intent_declare_descriptor_exposes_required_input_schema() {
    let tools = tool_descriptors();
    let tool = tools
        .iter()
        .find(|tool| tool.name == "state_intent_declare")
        .expect("intent tool descriptor should exist");

    assert_eq!(tool.input_schema["type"], "object");
    assert_eq!(
        tool.input_schema["properties"]["session_id"]["type"],
        "string"
    );
    assert_eq!(
        tool.input_schema["properties"]["workspace_id"]["type"],
        "string"
    );
    assert_eq!(
        tool.input_schema["properties"]["files_planned"]["type"],
        "array"
    );
    assert_eq!(
        tool.input_schema["properties"]["files_planned"]["items"]["type"],
        "string"
    );
    assert_eq!(
        tool.input_schema["required"],
        serde_json::json!(["files_planned"])
    );
    assert_eq!(tool.input_schema["additionalProperties"], false);
}
