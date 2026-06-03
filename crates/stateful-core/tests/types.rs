use stateful_core::{
    ActionKind, ActorType, ProtocolVersion, RequestEnvelope, ResourceType, Target, TargetOperation,
};

#[test]
fn request_envelope_round_trips_protocol_version() {
    let json = r#"{
      "protocol_version":"stateful.v1",
      "request_id":"req-1",
      "observed_at":"2026-05-31T12:00:00Z",
      "session":{"session_id":"s1","turn_id":"t1","actor_id":"a1","actor_type":"agent"},
      "workspace":{"root":"/repo","workspace_id":"w1","repo_id":"r1","worktree_id":"wt1","branch":"main"},
      "source":{"kind":"hook","event":"PreToolUse","tool_name":"apply_patch","source_ref":"hook:req-1"}
    }"#;

    let envelope: RequestEnvelope =
        serde_json::from_str(json).expect("request envelope json should deserialize");

    assert_eq!(envelope.protocol_version, ProtocolVersion::V1);
    assert_eq!(envelope.session.actor_type, ActorType::Agent);
    assert_eq!(envelope.workspace.branch, "main");
}

#[test]
fn authorization_target_round_trips_file_write_shape() {
    let json = r#"{
      "operation": "write",
      "resource_type": "file",
      "path": "src/auth.ts"
    }"#;

    let target: Target = serde_json::from_str(json).expect("target json should deserialize");

    assert_eq!(target.operation, TargetOperation::Write);
    assert_eq!(target.resource_type, ResourceType::File);
    assert_eq!(target.path.as_deref(), Some("src/auth.ts"));
}

#[test]
fn action_kind_uses_snake_case_protocol_names() {
    let action: ActionKind =
        serde_json::from_str(r#""write_file""#).expect("action should deserialize");

    assert_eq!(action, ActionKind::WriteFile);
    assert_eq!(
        serde_json::to_string(&ActionKind::ValidationRun).expect("action should serialize"),
        r#""validation_run""#
    );
}
