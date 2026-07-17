use stateful_core::{ActorType, ProtocolVersion, RequestEnvelope};

#[test]
fn request_envelope_round_trips_protocol_version() {
    let json = r#"{
      "protocol_version":"stateful.v2",
      "request_id":"8d5ddf45-9ce3-44ac-953e-3b776cd1783d",
      "observed_at":"2026-05-31T12:00:00Z",
      "agent":{"agent_id":"s1","turn_id":"t1","actor_id":"a1","actor_type":"agent"},
      "workspace":{"root":"/repo","workspace_id":"w1","repo_id":"r1","worktree_id":"wt1","branch":"main"},
      "source":{"kind":"hook","event":"PreToolUse","tool_name":"apply_patch","source_ref":"hook:req-1"},
      "payload":{}
    }"#;

    let envelope: RequestEnvelope<serde_json::Value> =
        serde_json::from_str(json).expect("request envelope json should deserialize");

    assert_eq!(envelope.protocol_version, ProtocolVersion::V2);
    assert_eq!(envelope.agent.actor_type, ActorType::Agent);
    assert_eq!(envelope.workspace.branch, "main");
}
