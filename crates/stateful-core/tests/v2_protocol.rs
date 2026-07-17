use serde::{Deserialize, Serialize};
use stateful_core::{
    ActorType, AgentIdentity, PresenceUpdate, ProtocolVersion, QueryEnvelope, RequestEnvelope,
    SourceKind, SourceRef, WorkspaceIdentity,
};
use time::OffsetDateTime;
use uuid::Uuid;

fn agent() -> AgentIdentity {
    AgentIdentity {
        agent_id: "agent-7".into(),
        turn_id: Some("turn-3".into()),
        actor_id: "actor-9".into(),
        actor_type: ActorType::Agent,
        owner_id: Some("owner-2".into()),
        parent_agent_id: Some("parent-agent-1".into()),
        parent_actor_id: Some("parent-actor-1".into()),
    }
}

fn workspace() -> WorkspaceIdentity {
    WorkspaceIdentity {
        root: "/repo".into(),
        workspace_id: "workspace-4".into(),
        repo_id: "repo-8".into(),
        worktree_id: "worktree-5".into(),
        branch: "presence-v2".into(),
    }
}

fn source() -> SourceRef {
    SourceRef {
        kind: SourceKind::Hook,
        event: "PreToolUse".into(),
        tool_name: Some("apply_patch".into()),
        source_ref: "hook:request-1".into(),
    }
}

#[test]
fn v2_post_envelope_round_trips_full_identity_and_payload() {
    let payload = PresenceUpdate {
        goal_excerpt: Some("fix  auth\n flow".into()),
        ..Default::default()
    };
    let envelope = RequestEnvelope::new(
        Uuid::parse_str("8d5ddf45-9ce3-44ac-953e-3b776cd1783d").expect("valid UUID"),
        OffsetDateTime::parse(
            "2026-05-31T12:00:00Z",
            &time::format_description::well_known::Rfc3339,
        )
        .expect("valid RFC3339 timestamp"),
        agent(),
        workspace(),
        source(),
        payload,
    )
    .expect("valid v2 envelope");

    let json = serde_json::to_value(&envelope).expect("envelope should serialize");
    assert_eq!(json["protocol_version"], "stateful.v2");
    assert_eq!(json["agent"]["agent_id"], "agent-7");
    assert_eq!(json["agent"]["turn_id"], "turn-3");
    assert_eq!(json["agent"]["actor_id"], "actor-9");
    assert_eq!(json["agent"]["actor_type"], "agent");
    assert_eq!(json["agent"]["owner_id"], "owner-2");
    assert_eq!(json["agent"]["parent_agent_id"], "parent-agent-1");
    assert_eq!(json["agent"]["parent_actor_id"], "parent-actor-1");
    assert_eq!(json["workspace"]["root"], "/repo");
    assert_eq!(json["workspace"]["workspace_id"], "workspace-4");
    assert_eq!(json["workspace"]["repo_id"], "repo-8");
    assert_eq!(json["workspace"]["worktree_id"], "worktree-5");
    assert_eq!(json["workspace"]["branch"], "presence-v2");
    assert_eq!(json["payload"]["goal_excerpt"], "fix  auth\n flow");

    let round_trip: RequestEnvelope<PresenceUpdate> =
        serde_json::from_value(json).expect("v2 envelope should deserialize");
    assert_eq!(round_trip, envelope);
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct CurrentQuery {
    resource: String,
}

#[test]
fn query_envelope_requires_explicit_agent_and_workspace_identity() {
    let missing_agent = r#"{
        "protocol_version":"stateful.v2",
        "request_id":"8d5ddf45-9ce3-44ac-953e-3b776cd1783d",
        "observed_at":"2026-05-31T12:00:00Z",
        "actor_id":"actor-9",
        "actor_type":"agent",
        "root":"/repo",
        "workspace_id":"workspace-4",
        "repo_id":"repo-8",
        "worktree_id":"worktree-5",
        "branch":"presence-v2",
        "kind":"hook",
        "event":"PreToolUse",
        "source_ref":"hook:request-1",
        "resource":"src/lib.rs"
    }"#;
    assert!(serde_json::from_str::<QueryEnvelope<CurrentQuery>>(missing_agent).is_err());

    let missing_workspace = r#"{
        "protocol_version":"stateful.v2",
        "request_id":"8d5ddf45-9ce3-44ac-953e-3b776cd1783d",
        "observed_at":"2026-05-31T12:00:00Z",
        "agent_id":"agent-7",
        "actor_id":"actor-9",
        "actor_type":"agent",
        "root":"/repo",
        "repo_id":"repo-8",
        "worktree_id":"worktree-5",
        "branch":"presence-v2",
        "kind":"hook",
        "event":"PreToolUse",
        "source_ref":"hook:request-1",
        "resource":"src/lib.rs"
    }"#;
    assert!(serde_json::from_str::<QueryEnvelope<CurrentQuery>>(missing_workspace).is_err());
}

#[test]
fn v1_protocol_value_is_rejected() {
    let error =
        RequestEnvelope::<PresenceUpdate>::from_json(r#"{"protocol_version":"stateful.v1"}"#)
            .expect_err("v1 must be rejected");

    assert_eq!(error.code, "unsupported_protocol");
    let response = serde_json::to_value(
        error
            .envelope(Uuid::parse_str("8d5ddf45-9ce3-44ac-953e-3b776cd1783d").expect("valid UUID")),
    )
    .expect("error envelope should serialize");
    assert_eq!(response["protocol_version"], "stateful.v2");
    assert_eq!(
        response["request_id"],
        "8d5ddf45-9ce3-44ac-953e-3b776cd1783d"
    );
    assert_eq!(response["error"]["code"], "unsupported_protocol");
}

#[test]
fn migrated_actor_type_accepts_unknown() {
    let actor_type: ActorType = serde_json::from_str(r#""unknown""#).expect("unknown actor");
    assert_eq!(actor_type, ActorType::Unknown);
    assert_eq!(ProtocolVersion::V2.to_string(), "stateful.v2");
}
