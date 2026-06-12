use stateful_core::{ActorType, ProtocolVersion, RequestEnvelope};

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
