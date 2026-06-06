use stateful_mcp::{ToolCall, map_tool_to_http, protocol_tool_name, tool_descriptors};

#[test]
fn replaceable_tools_are_not_recognized() {
    for tool_name in [
        "state_activity_observe",
        "state.activity.observe",
        "state_activity_finalize",
        "state.activity.finalize",
        "state_current_read",
        "state.current.read",
        "state_events_read",
        "state.events.read",
        "state_context_render",
        "state.context.render",
        "state_validation_run",
        "state.validation.run",
        "state_file_write",
        "state.file.write",
    ] {
        assert!(
            protocol_tool_name(tool_name).is_err(),
            "{tool_name} should not be exposed"
        );
        assert!(
            map_tool_to_http(ToolCall::new(tool_name, serde_json::json!({}))).is_err(),
            "{tool_name} should not map to HTTP"
        );
    }
}

#[test]
fn bash_write_tool_is_handled_locally_not_mapped_to_http() {
    let tool = ToolCall::new(
        "state.bash.write",
        serde_json::json!({"command": "true", "write_targets": ["src/auth.ts"]}),
    );

    let error = map_tool_to_http(tool).expect_err("bash write is CLI-local");

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
        ("state.conflicts.check", "POST", "/v1/conflicts/check"),
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
        protocol_tool_name("state_bash_write").expect("tool should map"),
        "state.bash.write"
    );
}

#[test]
fn tool_descriptors_expose_codex_friendly_names() {
    let tools = tool_descriptors();
    let names = tools.iter().map(|tool| tool.name).collect::<Vec<_>>();

    assert!(names.contains(&"state_intent_declare"));
    assert!(names.contains(&"state_conflicts_check"));
    assert!(names.contains(&"state_reconcile_ack"));
    assert!(names.contains(&"state_bash_write"));
    assert!(names.contains(&"state_notifications_poll"));
    assert!(names.contains(&"state_resume_next"));

    for removed in [
        "state_activity_observe",
        "state_activity_finalize",
        "state_current_read",
        "state_events_read",
        "state_context_render",
        "state_validation_run",
        "state_file_write",
    ] {
        assert!(!names.contains(&removed), "{removed} should not be exposed");
    }
}

#[test]
fn bash_write_descriptor_exposes_required_input_schema() {
    let tools = tool_descriptors();
    let tool = tools
        .iter()
        .find(|tool| tool.name == "state_bash_write")
        .expect("bash write tool descriptor should exist");

    assert_eq!(tool.protocol_name, "state.bash.write");
    assert_eq!(tool.input_schema["type"], "object");
    assert_eq!(tool.input_schema["properties"]["command"]["type"], "string");
    assert_eq!(
        tool.input_schema["properties"]["write_targets"]["type"],
        "array"
    );
    assert_eq!(
        tool.input_schema["properties"]["create_targets"]["type"],
        "array"
    );
    assert_eq!(tool.input_schema["properties"]["cwd"]["type"], "string");
    assert_eq!(
        tool.input_schema["properties"]["timeout_seconds"]["type"],
        "integer"
    );
    assert_eq!(
        tool.input_schema["properties"]["timeout_seconds"]["maximum"],
        600
    );
    assert!(tool.input_schema["properties"]["mcp_wait_ms"].is_null());
    assert_eq!(
        tool.input_schema["required"],
        serde_json::json!(["command", "write_targets"])
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
