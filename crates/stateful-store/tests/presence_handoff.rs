use stateful_core::{
    ActorType, AgentIdentity, ExplicitHandoff, HandoffStatus, PresencePhase, PresenceResourceRelation,
    PresenceUpdate, RequestEnvelope, SourceKind, SourceRef, WorkspaceIdentity,
};
use stateful_store::{
    Clock, FixedClock, PresenceRegistration, PresenceResourceUpdate, PresenceToolResult,
    PresenceToolStart, Store,
};
use std::sync::{Arc, Mutex};
use time::{Duration, OffsetDateTime, macros::datetime};
use uuid::Uuid;

const NOW: OffsetDateTime = datetime!(2026-07-15 12:00 UTC);

#[derive(Clone)]
struct MutableClock(Arc<Mutex<OffsetDateTime>>);

impl MutableClock {
    fn new(now: OffsetDateTime) -> Self {
        Self(Arc::new(Mutex::new(now)))
    }

    fn advance(&self, duration: Duration) {
        let mut now = self.0.lock().expect("clock lock should not be poisoned");
        *now += duration;
    }
}

impl Clock for MutableClock {
    fn now(&self) -> OffsetDateTime {
        *self.0.lock().expect("clock lock should not be poisoned")
    }
}

fn request<T: serde::Serialize>(
    request_id: Uuid,
    agent_id: &str,
    actor_id: &str,
    actor_type: ActorType,
    payload: T,
) -> RequestEnvelope<T> {
    RequestEnvelope::new(
        request_id,
        NOW,
        AgentIdentity {
            agent_id: agent_id.into(),
            turn_id: Some("turn-1".into()),
            actor_id: actor_id.into(),
            actor_type,
            owner_id: Some("owner-1".into()),
            parent_agent_id: Some("parent-agent".into()),
            parent_actor_id: Some("parent-actor".into()),
        },
        WorkspaceIdentity {
            root: "/repo".into(),
            workspace_id: "workspace-1".into(),
            repo_id: "repo-1".into(),
            worktree_id: "worktree-1".into(),
            branch: "main".into(),
        },
        SourceRef {
            kind: SourceKind::Cli,
            event: "test".into(),
            tool_name: None,
            source_ref: "presence-handoff-test".into(),
        },
        payload,
    )
    .expect("test request should be valid")
}

fn register_request(
    request_id: Uuid,
    agent_id: &str,
    actor_id: &str,
    actor_type: ActorType,
    first_prompt: Option<&str>,
) -> RequestEnvelope<PresenceRegistration> {
    request(
        request_id,
        agent_id,
        actor_id,
        actor_type,
        PresenceRegistration {
            first_prompt: first_prompt.map(str::to_owned),
        },
    )
}

fn store() -> Store {
    Store::open_in_memory_with_clock(FixedClock::new(NOW)).expect("store should open")
}

#[test]
fn registration_upserts_one_presence_per_workspace_and_agent() {
    let mut store = store();
    store
        .register_presence(&register_request(Uuid::new_v4(), "agent-1", "actor-1", ActorType::Agent, Some("first goal")))
        .expect("first registration should succeed");
    store
        .resume_presence(&register_request(Uuid::new_v4(), "agent-1", "actor-1", ActorType::Agent, None))
        .expect("resume should succeed");

    assert_eq!(store.presence_count("workspace-1").expect("presence count should load"), 1);
    let presence = store.presence_record("workspace-1", "agent-1").expect("presence should load").expect("presence should remain live");
    assert_eq!(presence.goal_excerpt.as_deref(), Some("first goal"));
    assert_eq!(presence.actor_id, "actor-1");
}

#[test]
fn registration_preserves_root_subagent_human_system_and_unknown_attribution() {
    let mut store = store();
    for (agent_id, actor_id, actor_type) in [
        ("root", "root-actor", ActorType::Agent),
        ("subagent", "subagent-actor", ActorType::Agent),
        ("human", "human-actor", ActorType::Human),
        ("system", "system-actor", ActorType::System),
        ("unknown", "unknown-actor", ActorType::Unknown),
    ] {
        store
            .register_presence(&register_request(Uuid::new_v4(), agent_id, actor_id, actor_type.clone(), None))
            .expect("registration should succeed");
        let presence = store.presence_record("workspace-1", agent_id).expect("presence should load").expect("presence should exist");
        assert_eq!(presence.actor_id, actor_id);
        assert_eq!(presence.actor_type, actor_type);
        assert_eq!(presence.owner_id.as_deref(), Some("owner-1"));
        assert_eq!(presence.parent_agent_id.as_deref(), Some("parent-agent"));
        assert_eq!(presence.parent_actor_id.as_deref(), Some("parent-actor"));
    }
}

#[test]
fn first_prompt_captures_normalized_goal_and_explicit_update_replaces_it() {
    let mut store = store();
    store
        .register_presence(&register_request(Uuid::new_v4(), "agent-1", "actor-1", ActorType::Agent, Some("fix  auth\n flow")))
        .expect("registration should succeed");
    store
        .update_presence(&request(Uuid::new_v4(), "agent-1", "actor-1", ActorType::Agent, PresenceUpdate {
            goal_excerpt: Some("ship  final\t version".into()),
            ..Default::default()
        }))
        .expect("goal update should succeed");

    assert_eq!(
        store.presence_record("workspace-1", "agent-1").expect("presence should load").expect("presence should exist").goal_excerpt.as_deref(),
        Some("ship final version"),
    );
}

#[test]
fn resource_relations_are_idempotent_and_semantic_changes_version_once() {
    let mut store = store();
    store
        .register_presence(&register_request(Uuid::new_v4(), "agent-1", "actor-1", ActorType::Agent, None))
        .expect("registration should succeed");
    let initial_version = store.workspace_version("workspace-1").expect("version should load");
    let planned = PresenceResourceUpdate { relative_path: "src/lib.rs".into(), relation: PresenceResourceRelation::Planned };
    store
        .update_presence_resource(&request(Uuid::new_v4(), "agent-1", "actor-1", ActorType::Agent, planned.clone()))
        .expect("first relation should succeed");
    let changed_version = store.workspace_version("workspace-1").expect("version should load");
    store
        .update_presence_resource(&request(Uuid::new_v4(), "agent-1", "actor-1", ActorType::Agent, planned))
        .expect("identical relation should refresh");
    assert_eq!(store.workspace_version("workspace-1").expect("version should load"), changed_version);
    assert_eq!(changed_version, initial_version + 1);

    store
        .update_presence_resource(&request(Uuid::new_v4(), "agent-1", "actor-1", ActorType::Agent, PresenceResourceUpdate {
            relative_path: "src/lib.rs".into(), relation: PresenceResourceRelation::Changed,
        }))
        .expect("semantic relation should succeed");
    assert_eq!(store.workspace_version("workspace-1").expect("version should load"), changed_version + 1);
    assert_eq!(store.presence_resources("workspace-1", "agent-1").expect("resources should load").len(), 2);
}

#[test]
fn busy_tool_defers_expiry_but_never_beyond_sixty_minutes() {
    let clock = MutableClock::new(NOW);
    let mut store = Store::open_in_memory_with_clock(clock.clone()).expect("store should open");
    store
        .register_presence(&register_request(Uuid::new_v4(), "agent-1", "actor-1", ActorType::Agent, None))
        .expect("registration should succeed");
    store
        .start_presence_tool(&request(Uuid::new_v4(), "agent-1", "actor-1", ActorType::Agent, PresenceToolStart {
            tool_name: "cargo test".into(),
            deadline: Some(NOW + Duration::hours(2)),
        }))
        .expect("tool start should succeed");
    let presence = store.presence_record("workspace-1", "agent-1").expect("presence should load").expect("presence should exist");
    assert_eq!(presence.phase, Some(PresencePhase::Testing));
    assert_eq!(presence.busy_until, Some(NOW + Duration::minutes(60)));

    clock.advance(Duration::minutes(16));
    store.expire_stale_presences(&request(Uuid::new_v4(), "agent-1", "actor-1", ActorType::Agent, ())).expect("maintenance should succeed");
    assert!(store.presence_record("workspace-1", "agent-1").expect("presence should load").is_some());
    clock.advance(Duration::minutes(45));
    store.expire_stale_presences(&request(Uuid::new_v4(), "agent-1", "actor-1", ActorType::Agent, ())).expect("maintenance should succeed");
    assert!(store.presence_record("workspace-1", "agent-1").expect("presence should load").is_none());
}

#[test]
fn explicit_handoff_finalizes_presence_and_cleans_coordination_in_one_transaction() {
    let mut store = store();
    store
        .register_presence(&register_request(Uuid::new_v4(), "agent-1", "actor-1", ActorType::Agent, None))
        .expect("registration should succeed");
    let finalize_request = request(Uuid::new_v4(), "agent-1", "actor-1", ActorType::Agent, ExplicitHandoff {
        status: HandoffStatus::Done,
        summary: "completed the work".into(),
        files_changed: vec!["src/lib.rs".into()],
        tests_run: vec!["cargo test -p stateful-store".into()],
        remaining_work: vec![],
        next_plan: None,
    });
    let outcome = store.finalize_handoff(&finalize_request).expect("explicit finalization should succeed");
    assert!(!outcome.duplicate);
    assert!(store.presence_record("workspace-1", "agent-1").expect("presence should load").is_none());
    let handoff = store.handoff_record("workspace-1", "agent-1").expect("handoff should load").expect("handoff should exist");
    assert!(handoff.explicit);
    assert_eq!(handoff.status, HandoffStatus::Done);
    assert_eq!(store.journal_event_types_for_request(finalize_request.request_id).expect("journal should load"), vec![
        "handoff.finalized", "presence.finalized", "reservation.released", "claim.released", "wait.cancelled", "write_fence.released",
    ]);

    let mut failing_store = Store::open_in_memory_with_clock(FixedClock::new(NOW)).expect("store should open");
    failing_store
        .register_presence(&register_request(Uuid::new_v4(), "agent-2", "actor-2", ActorType::Agent, None))
        .expect("registration should succeed");
    let before = failing_store.journal_event_count().expect("journal count should load");
    failing_store.fail_projector_on_event_for_tests(3);
    assert!(failing_store.finalize_handoff(&request(Uuid::new_v4(), "agent-2", "actor-2", ActorType::Agent, ExplicitHandoff {
        status: HandoffStatus::Done, summary: "done".into(), files_changed: vec![], tests_run: vec![], remaining_work: vec![], next_plan: None,
    })).is_err());
    assert_eq!(failing_store.journal_event_count().expect("journal count should load"), before);
    assert!(failing_store.presence_record("workspace-1", "agent-2").expect("presence should load").is_some());
}

#[test]
fn stop_without_explicit_handoff_creates_unknown_fallback() {
    let mut store = store();
    store
        .register_presence(&register_request(Uuid::new_v4(), "agent-1", "actor-1", ActorType::Agent, None))
        .expect("registration should succeed");
    store
        .update_presence(&request(Uuid::new_v4(), "agent-1", "actor-1", ActorType::Agent, PresenceUpdate {
            next_plan: Some("run focused tests".into()),
            ..Default::default()
        }))
        .expect("plan update should succeed");
    store
        .update_presence_resource(&request(Uuid::new_v4(), "agent-1", "actor-1", ActorType::Agent, PresenceResourceUpdate {
            relative_path: "src/lib.rs".into(), relation: PresenceResourceRelation::Changed,
        }))
        .expect("changed relation should succeed");
    store
        .complete_presence_tool(&request(Uuid::new_v4(), "agent-1", "actor-1", ActorType::Agent, PresenceToolResult {
            tool_name: "cargo test".into(), outcome: "passed".into(), summary: Some("all focused tests passed".into()),
        }))
        .expect("tool result should succeed");
    store.stop_presence(&request(Uuid::new_v4(), "agent-1", "actor-1", ActorType::Agent, ())).expect("stop should succeed");

    let handoff = store.handoff_record("workspace-1", "agent-1").expect("handoff should load").expect("fallback should exist");
    assert!(!handoff.explicit);
    assert_eq!(handoff.status, HandoffStatus::Unknown);
    assert!(handoff.summary.contains("no explicit handoff"));
    assert_eq!(handoff.files_changed, vec!["src/lib.rs"]);
    assert_eq!(handoff.tests_run, Vec::<String>::new());
    assert_eq!(handoff.remaining_work, vec!["run focused tests"]);
    assert!(handoff.last_result.as_deref().expect("result should remain").contains("cargo test"));
}

#[test]
fn ttl_expiry_lazily_creates_the_same_fallback_once() {
    let clock = MutableClock::new(NOW);
    let mut store = Store::open_in_memory_with_clock(clock.clone()).expect("store should open");
    store
        .register_presence(&register_request(Uuid::new_v4(), "agent-1", "actor-1", ActorType::Agent, None))
        .expect("registration should succeed");
    clock.advance(Duration::minutes(16));
    store.expire_stale_presences(&request(Uuid::new_v4(), "agent-1", "actor-1", ActorType::Agent, ())).expect("lazy expiry should succeed");
    let fallback = store.handoff_record("workspace-1", "agent-1").expect("handoff should load").expect("fallback should exist");
    let event_count = store.journal_event_count().expect("journal count should load");
    store.expire_stale_presences(&request(Uuid::new_v4(), "agent-1", "actor-1", ActorType::Agent, ())).expect("repeat expiry should succeed");
    assert_eq!(store.handoff_record("workspace-1", "agent-1").expect("handoff should load"), Some(fallback));
    assert_eq!(store.journal_event_count().expect("journal count should load"), event_count);
}

#[test]
fn handoff_validation_rejects_over_limit_lists_and_summary() {
    let mut store = store();
    store
        .register_presence(&register_request(Uuid::new_v4(), "agent-1", "actor-1", ActorType::Agent, None))
        .expect("registration should succeed");
    let too_long_summary = ExplicitHandoff {
        status: HandoffStatus::Done, summary: "x".repeat(2_001), files_changed: vec![], tests_run: vec![], remaining_work: vec![], next_plan: None,
    };
    assert!(store.finalize_handoff(&request(Uuid::new_v4(), "agent-1", "actor-1", ActorType::Agent, too_long_summary)).is_err());
    let too_many_tests = ExplicitHandoff {
        status: HandoffStatus::Done, summary: "done".into(), files_changed: vec![], tests_run: vec!["test".into(); 101], remaining_work: vec![], next_plan: None,
    };
    assert!(store.finalize_handoff(&request(Uuid::new_v4(), "agent-1", "actor-1", ActorType::Agent, too_many_tests)).is_err());
}

#[test]
fn explicit_and_fallback_handoffs_expire_after_their_distinct_windows() {
    let clock = MutableClock::new(NOW);
    let mut store = Store::open_in_memory_with_clock(clock.clone()).expect("store should open");
    for agent_id in ["explicit", "fallback"] {
        store
            .register_presence(&register_request(Uuid::new_v4(), agent_id, &format!("{agent_id}-actor"), ActorType::Agent, None))
            .expect("registration should succeed");
    }
    store.finalize_handoff(&request(Uuid::new_v4(), "explicit", "explicit-actor", ActorType::Agent, ExplicitHandoff {
        status: HandoffStatus::Done, summary: "complete".into(), files_changed: vec![], tests_run: vec![], remaining_work: vec![], next_plan: None,
    })).expect("explicit handoff should succeed");
    store.stop_presence(&request(Uuid::new_v4(), "fallback", "fallback-actor", ActorType::Agent, ())).expect("fallback should succeed");

    clock.advance(Duration::hours(25));
    store.expire_stale_handoffs(&request(Uuid::new_v4(), "explicit", "explicit-actor", ActorType::Agent, ())).expect("maintenance should succeed");
    assert!(store.handoff_record("workspace-1", "fallback").expect("handoff should load").is_none());
    assert!(store.handoff_record("workspace-1", "explicit").expect("handoff should load").is_some());
    clock.advance(Duration::days(7));
    store.expire_stale_handoffs(&request(Uuid::new_v4(), "explicit", "explicit-actor", ActorType::Agent, ())).expect("maintenance should succeed");
    assert!(store.handoff_record("workspace-1", "explicit").expect("handoff should load").is_none());
}
