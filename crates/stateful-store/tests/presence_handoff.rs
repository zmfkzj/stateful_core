use stateful_core::{
    ActorType, AgentIdentity, ExplicitHandoff, HandoffStatus, PresencePhase,
    PresenceResourceRelation, PresenceUpdate, ReadClassification, ReadCompletion,
    ReadObservationStart, RequestEnvelope, SourceKind, SourceRef, WorkspaceIdentity,
    WriteIntentStart, WriteIntentStatus, WriteTarget,
};
use stateful_store::{
    ActivityFinalization, ActivityStart, Clock, FixedClock, PresenceRegistration,
    PresenceResourceUpdate, PresenceToolResult, PresenceToolStart, ReservationDeclaration, Store,
    StoreError, WaitRequest, WriteFenceAcquire,
};
use std::sync::{Arc, Mutex};
use tempfile::TempDir;
use time::{Duration, OffsetDateTime, format_description::well_known::Rfc3339, macros::datetime};
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

fn unknown_request<T: serde::Serialize>(
    request_id: Uuid,
    agent_id: &str,
    actor_id: &str,
    payload: T,
) -> RequestEnvelope<T> {
    let mut request = request(request_id, agent_id, actor_id, ActorType::Unknown, payload);
    request.agent.owner_id = None;
    request.agent.parent_agent_id = None;
    request.agent.parent_actor_id = None;
    request
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

fn presence(store: &mut Store, agent_id: &str) -> Option<stateful_core::PresenceRecord> {
    store
        .presence_for_request(
            &request(
                Uuid::new_v4(),
                agent_id,
                &format!("{agent_id}-actor"),
                ActorType::Agent,
                (),
            ),
            agent_id,
        )
        .expect("presence query should succeed")
}

fn handoff(store: &mut Store, agent_id: &str) -> Option<stateful_core::HandoffRecord> {
    store
        .handoff_for_request(
            &request(
                Uuid::new_v4(),
                agent_id,
                &format!("{agent_id}-actor"),
                ActorType::Agent,
                (),
            ),
            agent_id,
        )
        .expect("handoff query should succeed")
}

fn presence_count(store: &mut Store) -> u64 {
    store
        .presence_count_for_request(&request(
            Uuid::new_v4(),
            "agent-1",
            "actor-1",
            ActorType::Agent,
            (),
        ))
        .expect("presence count query should succeed")
}

fn resources(store: &mut Store, agent_id: &str) -> Vec<stateful_core::PresenceResource> {
    store
        .presence_resources_for_request(
            &request(
                Uuid::new_v4(),
                agent_id,
                &format!("{agent_id}-actor"),
                ActorType::Agent,
                (),
            ),
            agent_id,
        )
        .expect("resource query should succeed")
}
#[test]
fn registration_upserts_one_presence_per_workspace_and_agent() {
    let mut store = store();
    store
        .register_presence(&register_request(
            Uuid::new_v4(),
            "agent-1",
            "actor-1",
            ActorType::Agent,
            Some("first goal"),
        ))
        .expect("first registration should succeed");
    store
        .resume_presence(&register_request(
            Uuid::new_v4(),
            "agent-1",
            "actor-1",
            ActorType::Agent,
            None,
        ))
        .expect("resume should succeed");

    assert_eq!(presence_count(&mut store), 1);
    let presence = presence(&mut store, "agent-1").expect("presence should remain live");
    assert_eq!(presence.goal_excerpt.as_deref(), Some("first goal"));
    assert_eq!(presence.actor_id, "actor-1");
}

#[test]
fn registration_preserves_root_subagent_human_system_and_unknown_attribution() {
    let mut store = store();
    for (agent_id, actor_id, actor_type) in [
        ("root", "root-actor", ActorType::Agent),
        ("subagent", "subagent-actor", ActorType::Subagent),
        ("human", "human-actor", ActorType::Human),
        ("system", "system-actor", ActorType::System),
        ("unknown", "unknown-actor", ActorType::Unknown),
    ] {
        store
            .register_presence(&register_request(
                Uuid::new_v4(),
                agent_id,
                actor_id,
                actor_type.clone(),
                None,
            ))
            .expect("registration should succeed");
        let presence = presence(&mut store, agent_id).expect("presence should exist");
        assert_eq!(presence.actor_id, actor_id);
        assert_eq!(presence.actor_type, actor_type);
        assert_eq!(presence.owner_id.as_deref(), Some("owner-1"));
        assert_eq!(presence.parent_agent_id.as_deref(), Some("parent-agent"));
        assert_eq!(presence.parent_actor_id.as_deref(), Some("parent-actor"));
    }
}

#[test]
fn normal_unknown_presence_rejects_a_different_same_agent_identity() {
    let mut store = store();
    let owner = unknown_request(
        Uuid::new_v4(),
        "agent-unknown",
        "unknown",
        PresenceRegistration { first_prompt: None },
    );
    store
        .register_presence(&owner)
        .expect("unknown owner should register");
    store
        .resume_presence(&unknown_request(
            Uuid::new_v4(),
            "agent-unknown",
            "unknown",
            PresenceRegistration { first_prompt: None },
        ))
        .expect("exact unknown owner should resume");

    assert!(matches!(
        store.resume_presence(&unknown_request(
            Uuid::new_v4(),
            "agent-unknown",
            "different-actor",
            PresenceRegistration { first_prompt: None },
        )),
        Err(StoreError::ReservationOwnerMismatch)
    ));
}

#[test]
fn normal_unknown_handoff_rejects_a_different_same_agent_identity() {
    let mut store = store();
    store
        .register_presence(&unknown_request(
            Uuid::new_v4(),
            "agent-unknown",
            "unknown",
            PresenceRegistration { first_prompt: None },
        ))
        .expect("unknown owner should register");
    store
        .stop_presence(&unknown_request(
            Uuid::new_v4(),
            "agent-unknown",
            "unknown",
            (),
        ))
        .expect("exact unknown owner should stop");

    assert!(matches!(
        store.finalize_handoff(&unknown_request(
            Uuid::new_v4(),
            "agent-unknown",
            "different-actor",
            ExplicitHandoff {
                status: HandoffStatus::Done,
                summary: "done".into(),
                files_changed: vec![],
                tests_run: vec![],
                remaining_work: vec![],
                next_plan: None,
            },
        )),
        Err(StoreError::ReservationOwnerMismatch)
    ));
}

#[test]
fn first_prompt_captures_normalized_goal_and_explicit_update_replaces_it() {
    let mut store = store();
    store
        .register_presence(&register_request(
            Uuid::new_v4(),
            "agent-1",
            "actor-1",
            ActorType::Agent,
            Some("fix  auth\n flow"),
        ))
        .expect("registration should succeed");
    store
        .update_presence(&request(
            Uuid::new_v4(),
            "agent-1",
            "actor-1",
            ActorType::Agent,
            PresenceUpdate {
                goal_excerpt: Some("ship  final\t version".into()),
                ..Default::default()
            },
        ))
        .expect("goal update should succeed");

    assert_eq!(
        presence(&mut store, "agent-1")
            .expect("presence should exist")
            .goal_excerpt
            .as_deref(),
        Some("ship final version"),
    );
}

#[test]
fn resource_relations_are_idempotent_and_semantic_changes_version_once() {
    let mut store = store();
    store
        .register_presence(&register_request(
            Uuid::new_v4(),
            "agent-1",
            "actor-1",
            ActorType::Agent,
            None,
        ))
        .expect("registration should succeed");
    let initial_version = store
        .workspace_version("workspace-1")
        .expect("version should load");
    let planned = PresenceResourceUpdate {
        relative_path: "src/lib.rs".into(),
        relation: PresenceResourceRelation::Planned,
    };
    store
        .update_presence_resource(&request(
            Uuid::new_v4(),
            "agent-1",
            "actor-1",
            ActorType::Agent,
            planned.clone(),
        ))
        .expect("first relation should succeed");
    let changed_version = store
        .workspace_version("workspace-1")
        .expect("version should load");
    store
        .update_presence_resource(&request(
            Uuid::new_v4(),
            "agent-1",
            "actor-1",
            ActorType::Agent,
            planned,
        ))
        .expect("identical relation should refresh");
    assert_eq!(
        store
            .workspace_version("workspace-1")
            .expect("version should load"),
        changed_version
    );
    assert_eq!(changed_version, initial_version + 1);

    let changed_request = request(
        Uuid::new_v4(),
        "agent-1",
        "actor-1",
        ActorType::Agent,
        PresenceResourceUpdate {
            relative_path: "src/lib.rs".into(),
            relation: PresenceResourceRelation::Changed,
        },
    );
    let changed = store
        .update_presence_resource(&changed_request)
        .expect("semantic relation should succeed");
    assert_eq!(
        store
            .journal_event_types_for_request(changed_request.request_id)
            .expect("journal should load"),
        vec!["presence.resources_updated"],
    );
    assert_eq!(
        store
            .workspace_version("workspace-1")
            .expect("version should load"),
        changed
            .last_event_seq
            .expect("semantic change event sequence"),
    );
    assert_eq!(resources(&mut store, "agent-1").len(), 2);
}

#[test]
fn busy_tool_defers_expiry_but_never_beyond_sixty_minutes() {
    let clock = MutableClock::new(NOW);
    let mut store = Store::open_in_memory_with_clock(clock.clone()).expect("store should open");
    store
        .register_presence(&register_request(
            Uuid::new_v4(),
            "agent-1",
            "actor-1",
            ActorType::Agent,
            None,
        ))
        .expect("registration should succeed");
    store
        .start_presence_tool(&request(
            Uuid::new_v4(),
            "agent-1",
            "actor-1",
            ActorType::Agent,
            PresenceToolStart {
                tool_name: "cargo test".into(),
                deadline: Some(NOW + Duration::hours(2)),
            },
        ))
        .expect("tool start should succeed");
    let record = presence(&mut store, "agent-1").expect("presence should exist");
    assert_eq!(record.phase, Some(PresencePhase::Testing));
    assert_eq!(record.busy_until, Some(NOW + Duration::minutes(60)));

    clock.advance(Duration::minutes(16));
    store
        .expire_stale_presences(&request(
            Uuid::new_v4(),
            "agent-1",
            "actor-1",
            ActorType::Agent,
            (),
        ))
        .expect("maintenance should succeed");
    assert!(presence(&mut store, "agent-1").is_some());
    clock.advance(Duration::minutes(45));
    store
        .expire_stale_presences(&request(
            Uuid::new_v4(),
            "agent-1",
            "actor-1",
            ActorType::Agent,
            (),
        ))
        .expect("maintenance should succeed");
    assert!(presence(&mut store, "agent-1").is_none());
}

#[test]
fn stale_heartbeat_revives_only_after_recording_fallback_in_its_request() {
    let clock = MutableClock::new(NOW);
    let mut store = Store::open_in_memory_with_clock(clock.clone()).expect("store should open");
    store
        .register_presence(&register_request(
            Uuid::new_v4(),
            "agent-1",
            "actor-1",
            ActorType::Agent,
            None,
        ))
        .expect("registration should succeed");
    clock.advance(Duration::minutes(61));
    let heartbeat = request(Uuid::new_v4(), "agent-1", "actor-1", ActorType::Agent, ());

    store
        .heartbeat_presence(&heartbeat)
        .expect("stale heartbeat should revive only after fallback cleanup");

    assert!(
        handoff(&mut store, "agent-1").is_some(),
        "expiry must commit the fallback handoff"
    );
    assert!(
        presence(&mut store, "agent-1").is_some(),
        "heartbeat should restore the presence"
    );
    assert_eq!(
        store
            .journal_event_types_for_request(heartbeat.request_id)
            .expect("journal should load"),
        vec![
            "handoff.finalized",
            "presence.finalized",
            "reservation.released",
            "claim.released",
            "wait.cancelled",
            "write_fence.released",
            "presence.heartbeat",
        ],
    );
}

#[test]
fn stale_register_resume_and_update_record_fallback_before_reviving_presence() {
    for (kind, presence_event) in [
        ("register", "presence.registered"),
        ("resume", "presence.heartbeat"),
        ("update", "presence.phase_updated"),
    ] {
        let clock = MutableClock::new(NOW);
        let mut store = Store::open_in_memory_with_clock(clock.clone()).expect("store should open");
        store
            .register_presence(&register_request(
                Uuid::new_v4(),
                "agent-1",
                "actor-1",
                ActorType::Agent,
                None,
            ))
            .expect("registration should succeed");
        clock.advance(Duration::minutes(61));
        let request_id = Uuid::new_v4();

        match kind {
            "register" => store
                .register_presence(&register_request(
                    request_id,
                    "agent-1",
                    "actor-1",
                    ActorType::Agent,
                    None,
                ))
                .expect("register should revive only after fallback cleanup"),
            "resume" => store
                .resume_presence(&register_request(
                    request_id,
                    "agent-1",
                    "actor-1",
                    ActorType::Agent,
                    None,
                ))
                .expect("resume should revive only after fallback cleanup"),
            "update" => store
                .update_presence(&request(
                    request_id,
                    "agent-1",
                    "actor-1",
                    ActorType::Agent,
                    PresenceUpdate {
                        phase: Some(PresencePhase::Editing),
                        ..Default::default()
                    },
                ))
                .expect("update should revive only after fallback cleanup"),
            _ => unreachable!(),
        };

        assert!(
            handoff(&mut store, "agent-1").is_some(),
            "{kind} must leave a fallback handoff"
        );
        assert!(
            presence(&mut store, "agent-1").is_some(),
            "{kind} should revive presence after cleanup"
        );
        let event_types = store
            .journal_event_types_for_request(request_id)
            .expect("journal should load");
        assert_eq!(
            &event_types[..6],
            [
                "handoff.finalized",
                "presence.finalized",
                "reservation.released",
                "claim.released",
                "wait.cancelled",
                "write_fence.released",
            ]
        );
        assert_eq!(event_types[6], presence_event);
    }
}

#[test]
fn duplicate_heartbeat_response_remains_frozen_after_ttl() {
    let clock = MutableClock::new(NOW);
    let mut store = Store::open_in_memory_with_clock(clock.clone()).expect("store should open");
    store
        .register_presence(&register_request(
            Uuid::new_v4(),
            "agent-1",
            "actor-1",
            ActorType::Agent,
            None,
        ))
        .expect("registration should succeed");
    let heartbeat = request(Uuid::new_v4(), "agent-1", "actor-1", ActorType::Agent, ());
    let first = store
        .heartbeat_presence(&heartbeat)
        .expect("initial heartbeat should succeed");
    clock.advance(Duration::minutes(16));

    let repeated = store
        .heartbeat_presence(&heartbeat)
        .expect("a duplicate request returns its frozen original response");

    assert_eq!(repeated.response, first.response);
    assert_eq!(store.journal_event_count().expect("journal count"), 2);
}

#[test]
fn fallback_events_persist_stop_and_ttl_causes_independent_of_source_ref() {
    let temp = TempDir::new().expect("temp directory should create");
    let path = temp.path().join("state.db");
    let clock = MutableClock::new(NOW);
    let mut store =
        Store::open_with_clock(&path, clock.clone()).expect("persistent store should open");

    store
        .register_presence(&register_request(
            Uuid::new_v4(),
            "stop-agent",
            "stop-actor",
            ActorType::Agent,
            None,
        ))
        .expect("stop presence should register");
    let mut stop = request(
        Uuid::new_v4(),
        "stop-agent",
        "stop-actor",
        ActorType::Agent,
        (),
    );
    stop.source.source_ref = "presence.expire".into();
    store
        .stop_presence(&stop)
        .expect("stop fallback should persist");

    store
        .register_presence(&register_request(
            Uuid::new_v4(),
            "ttl-agent",
            "ttl-actor",
            ActorType::Agent,
            None,
        ))
        .expect("ttl presence should register");
    clock.advance(Duration::minutes(16));
    let mut expire = request(
        Uuid::new_v4(),
        "ttl-agent",
        "ttl-actor",
        ActorType::Agent,
        (),
    );
    expire.source.source_ref = "presence.stop".into();
    store
        .expire_stale_presences(&expire)
        .expect("ttl fallback should persist");
    drop(store);

    let connection = rusqlite::Connection::open(path).expect("journal should reopen");
    let mut statement = connection
        .prepare(
            "SELECT aggregate_id, json_extract(payload_json, '$.event.data.data.fallback_cause')
             FROM journal_events
             WHERE event_type = 'handoff.finalized'
             ORDER BY aggregate_id",
        )
        .expect("fallback query should prepare");
    let causes = statement
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?))
        })
        .expect("fallback query should run")
        .collect::<Result<Vec<_>, _>>()
        .expect("fallback causes should load");

    assert_eq!(
        causes,
        vec![
            ("stop-agent".into(), Some("stop".into())),
            ("ttl-agent".into(), Some("ttl".into())),
        ],
    );
}

#[test]
fn stale_heartbeat_reopens_beside_an_existing_live_handoff() {
    let clock = MutableClock::new(NOW);
    let mut store = Store::open_in_memory_with_clock(clock.clone()).expect("store should open");
    store
        .register_presence(&register_request(
            Uuid::new_v4(),
            "agent-1",
            "actor-1",
            ActorType::Agent,
            None,
        ))
        .expect("registration should succeed");
    store
        .stop_presence(&request(
            Uuid::new_v4(),
            "agent-1",
            "actor-1",
            ActorType::Agent,
            (),
        ))
        .expect("fallback handoff succeeds");
    store
        .register_presence(&register_request(
            Uuid::new_v4(),
            "agent-1",
            "actor-1",
            ActorType::Agent,
            None,
        ))
        .expect("registration resumes beside the live handoff");
    clock.advance(Duration::minutes(16));

    store
        .heartbeat_presence(&request(
            Uuid::new_v4(),
            "agent-1",
            "actor-1",
            ActorType::Agent,
            (),
        ))
        .expect("stale heartbeat should reopen beside an existing handoff");

    assert!(presence(&mut store, "agent-1").is_some());
    assert!(handoff(&mut store, "agent-1").is_some());
}

#[test]
fn maintenance_leaves_started_write_intents_unclassified_without_post_fingerprints() {
    let clock = MutableClock::new(NOW);
    let mut store = Store::open_in_memory_with_clock(clock.clone()).expect("store should open");
    let before = stateful_core::fingerprint_reader(std::io::Cursor::new(b"before"))
        .expect("test fingerprint");
    let intent = store
        .start_write_intent(&request(
            Uuid::new_v4(),
            "agent-1",
            "actor-1",
            ActorType::Agent,
            WriteIntentStart {
                operation_id: "write-1".into(),
                action: "write_file".into(),
                targets: vec![WriteTarget {
                    path: "src/lib.rs".into(),
                    before,
                }],
            },
        ))
        .expect("intent starts")
        .response;
    clock.advance(Duration::minutes(6));

    store.run_maintenance().expect("maintenance succeeds");

    let active = store
        .active_write_intent("workspace-1", "src/lib.rs")
        .expect("intent reads")
        .expect("maintenance cannot infer a write outcome");
    assert_eq!(active.intent_id, intent.intent_id);
    assert_eq!(active.status, WriteIntentStatus::Started);
}

#[test]
fn explicit_handoff_finalizes_presence_and_cleans_coordination_in_one_transaction() {
    let mut store = store();
    store
        .register_presence(&register_request(
            Uuid::new_v4(),
            "agent-1",
            "actor-1",
            ActorType::Agent,
            None,
        ))
        .expect("registration should succeed");
    let finalize_request = request(
        Uuid::new_v4(),
        "agent-1",
        "actor-1",
        ActorType::Agent,
        ExplicitHandoff {
            status: HandoffStatus::Done,
            summary: "completed the work".into(),
            files_changed: vec!["src/lib.rs".into()],
            tests_run: vec!["cargo test -p stateful-store".into()],
            remaining_work: vec![],
            next_plan: None,
        },
    );
    let outcome = store
        .finalize_handoff(&finalize_request)
        .expect("explicit finalization should succeed");
    assert!(!outcome.duplicate);
    assert!(presence(&mut store, "agent-1").is_none());
    let handoff = handoff(&mut store, "agent-1").expect("handoff should exist");
    assert!(handoff.explicit);
    assert_eq!(handoff.status, HandoffStatus::Done);
    assert_eq!(
        store
            .journal_event_types_for_request(finalize_request.request_id)
            .expect("journal should load"),
        vec![
            "handoff.finalized",
            "presence.finalized",
            "reservation.released",
            "claim.released",
            "wait.cancelled",
            "write_fence.released",
        ]
    );

    let mut failing_store =
        Store::open_in_memory_with_clock(FixedClock::new(NOW)).expect("store should open");
    failing_store
        .register_presence(&register_request(
            Uuid::new_v4(),
            "agent-2",
            "actor-2",
            ActorType::Agent,
            None,
        ))
        .expect("registration should succeed");
    let before = failing_store
        .journal_event_count()
        .expect("journal count should load");
    failing_store.fail_projector_on_event_for_tests(3);
    assert!(
        failing_store
            .finalize_handoff(&request(
                Uuid::new_v4(),
                "agent-2",
                "actor-2",
                ActorType::Agent,
                ExplicitHandoff {
                    status: HandoffStatus::Done,
                    summary: "done".into(),
                    files_changed: vec![],
                    tests_run: vec![],
                    remaining_work: vec![],
                    next_plan: None,
                }
            ))
            .is_err()
    );
    assert_eq!(
        failing_store
            .journal_event_count()
            .expect("journal count should load"),
        before
    );
    assert!(presence(&mut failing_store, "agent-2").is_some());
}

#[test]
fn stop_without_explicit_handoff_creates_unknown_fallback() {
    let mut store = store();
    store
        .register_presence(&register_request(
            Uuid::new_v4(),
            "agent-1",
            "actor-1",
            ActorType::Agent,
            None,
        ))
        .expect("registration should succeed");
    store
        .update_presence(&request(
            Uuid::new_v4(),
            "agent-1",
            "actor-1",
            ActorType::Agent,
            PresenceUpdate {
                next_plan: Some("run focused tests".into()),
                ..Default::default()
            },
        ))
        .expect("plan update should succeed");
    store
        .update_presence_resource(&request(
            Uuid::new_v4(),
            "agent-1",
            "actor-1",
            ActorType::Agent,
            PresenceResourceUpdate {
                relative_path: "src/lib.rs".into(),
                relation: PresenceResourceRelation::Changed,
            },
        ))
        .expect("changed relation should succeed");
    store
        .complete_presence_tool(&request(
            Uuid::new_v4(),
            "agent-1",
            "actor-1",
            ActorType::Agent,
            PresenceToolResult {
                tool_name: "cargo test".into(),
                outcome: "passed".into(),
                summary: Some("all focused tests passed".into()),
            },
        ))
        .expect("tool result should succeed");
    store
        .stop_presence(&request(
            Uuid::new_v4(),
            "agent-1",
            "actor-1",
            ActorType::Agent,
            (),
        ))
        .expect("stop should succeed");

    let handoff = handoff(&mut store, "agent-1").expect("fallback should exist");
    assert!(!handoff.explicit);
    assert_eq!(handoff.status, HandoffStatus::Unknown);
    assert!(handoff.summary.contains("no explicit handoff"));
    assert_eq!(handoff.files_changed, vec!["src/lib.rs"]);
    assert_eq!(handoff.tests_run, Vec::<String>::new());
    assert_eq!(handoff.remaining_work, vec!["run focused tests"]);
    assert!(
        handoff
            .last_result
            .as_deref()
            .expect("result should remain")
            .contains("cargo test")
    );
}

#[test]
fn ttl_expiry_lazily_creates_the_same_fallback_once() {
    let clock = MutableClock::new(NOW);
    let mut store = Store::open_in_memory_with_clock(clock.clone()).expect("store should open");
    store
        .register_presence(&register_request(
            Uuid::new_v4(),
            "agent-1",
            "actor-1",
            ActorType::Agent,
            None,
        ))
        .expect("registration should succeed");
    clock.advance(Duration::minutes(16));
    store
        .expire_stale_presences(&request(
            Uuid::new_v4(),
            "agent-1",
            "actor-1",
            ActorType::Agent,
            (),
        ))
        .expect("lazy expiry should succeed");
    let fallback = handoff(&mut store, "agent-1").expect("fallback should exist");
    let event_count = store
        .journal_event_count()
        .expect("journal count should load");
    store
        .expire_stale_presences(&request(
            Uuid::new_v4(),
            "agent-1",
            "actor-1",
            ActorType::Agent,
            (),
        ))
        .expect("repeat expiry should succeed");
    assert_eq!(handoff(&mut store, "agent-1"), Some(fallback));
    assert_eq!(
        store
            .journal_event_count()
            .expect("journal count should load"),
        event_count
    );
}

#[test]
fn handoff_validation_rejects_over_limit_lists_and_summary() {
    let mut store = store();
    store
        .register_presence(&register_request(
            Uuid::new_v4(),
            "agent-1",
            "actor-1",
            ActorType::Agent,
            None,
        ))
        .expect("registration should succeed");
    let too_long_summary = ExplicitHandoff {
        status: HandoffStatus::Done,
        summary: "x".repeat(2_001),
        files_changed: vec![],
        tests_run: vec![],
        remaining_work: vec![],
        next_plan: None,
    };
    assert!(
        store
            .finalize_handoff(&request(
                Uuid::new_v4(),
                "agent-1",
                "actor-1",
                ActorType::Agent,
                too_long_summary
            ))
            .is_err()
    );
    let too_many_tests = ExplicitHandoff {
        status: HandoffStatus::Done,
        summary: "done".into(),
        files_changed: vec![],
        tests_run: vec!["test".into(); 101],
        remaining_work: vec![],
        next_plan: None,
    };
    assert!(
        store
            .finalize_handoff(&request(
                Uuid::new_v4(),
                "agent-1",
                "actor-1",
                ActorType::Agent,
                too_many_tests
            ))
            .is_err()
    );
}

#[test]
fn explicit_and_fallback_handoffs_expire_after_their_distinct_windows() {
    let clock = MutableClock::new(NOW);
    let mut store = Store::open_in_memory_with_clock(clock.clone()).expect("store should open");
    for agent_id in ["explicit", "fallback"] {
        store
            .register_presence(&register_request(
                Uuid::new_v4(),
                agent_id,
                &format!("{agent_id}-actor"),
                ActorType::Agent,
                None,
            ))
            .expect("registration should succeed");
    }
    store
        .finalize_handoff(&request(
            Uuid::new_v4(),
            "explicit",
            "explicit-actor",
            ActorType::Agent,
            ExplicitHandoff {
                status: HandoffStatus::Done,
                summary: "complete".into(),
                files_changed: vec![],
                tests_run: vec![],
                remaining_work: vec![],
                next_plan: None,
            },
        ))
        .expect("explicit handoff should succeed");
    store
        .stop_presence(&request(
            Uuid::new_v4(),
            "fallback",
            "fallback-actor",
            ActorType::Agent,
            (),
        ))
        .expect("fallback should succeed");

    clock.advance(Duration::hours(25));
    store
        .expire_stale_handoffs(&request(
            Uuid::new_v4(),
            "explicit",
            "explicit-actor",
            ActorType::Agent,
            (),
        ))
        .expect("maintenance should succeed");
    assert!(handoff(&mut store, "fallback").is_none());
    assert!(handoff(&mut store, "explicit").is_some());
    clock.advance(Duration::days(7));
    store
        .expire_stale_handoffs(&request(
            Uuid::new_v4(),
            "explicit",
            "explicit-actor",
            ActorType::Agent,
            (),
        ))
        .expect("maintenance should succeed");
    assert!(handoff(&mut store, "explicit").is_none());
}

#[test]
fn generic_presence_update_rejects_tool_only_fields_without_mutation() {
    let clock = MutableClock::new(NOW);
    let mut store = Store::open_in_memory_with_clock(clock.clone()).expect("store should open");
    store
        .register_presence(&register_request(
            Uuid::new_v4(),
            "agent-1",
            "actor-1",
            ActorType::Agent,
            None,
        ))
        .expect("registration should succeed");
    let before = presence(&mut store, "agent-1").expect("presence should exist");

    assert!(
        store
            .update_presence(&request(
                Uuid::new_v4(),
                "agent-1",
                "actor-1",
                ActorType::Agent,
                PresenceUpdate {
                    last_result: Some("unbounded tool stdout must not be accepted".into()),
                    ..Default::default()
                }
            ))
            .is_err()
    );
    assert!(
        store
            .update_presence(&request(
                Uuid::new_v4(),
                "agent-1",
                "actor-1",
                ActorType::Agent,
                PresenceUpdate {
                    busy_until: Some(NOW + Duration::minutes(60)),
                    ..Default::default()
                }
            ))
            .is_err()
    );
    assert_eq!(
        presence(&mut store, "agent-1").expect("presence should exist"),
        before,
    );

    store
        .start_presence_tool(&request(
            Uuid::new_v4(),
            "agent-1",
            "actor-1",
            ActorType::Agent,
            PresenceToolStart {
                tool_name: "cargo test".into(),
                deadline: Some(NOW + Duration::minutes(30)),
            },
        ))
        .expect("first tool start should succeed");
    clock.advance(Duration::minutes(20));
    store
        .start_presence_tool(&request(
            Uuid::new_v4(),
            "agent-1",
            "actor-1",
            ActorType::Agent,
            PresenceToolStart {
                tool_name: "cargo test".into(),
                deadline: Some(NOW + Duration::minutes(80)),
            },
        ))
        .expect("repeated tool start should refresh without extending its cap");
    assert_eq!(
        presence(&mut store, "agent-1")
            .expect("presence should exist")
            .busy_until,
        Some(NOW + Duration::minutes(30)),
    );
}

#[test]
fn relevant_presence_query_lazily_finalizes_expired_presence_once() {
    let temp = TempDir::new().expect("temporary directory should exist");
    let path = temp.path().join("presence.sqlite");
    let clock = MutableClock::new(NOW);
    {
        let mut store = Store::open_with_clock(&path, clock.clone()).expect("store should open");
        store
            .register_presence(&register_request(
                Uuid::new_v4(),
                "agent-1",
                "actor-1",
                ActorType::Agent,
                None,
            ))
            .expect("registration should succeed");
    }
    clock.advance(Duration::minutes(16));
    let mut store =
        Store::open_with_clock(&path, clock.clone()).expect("reopened store should open");
    assert_eq!(
        store
            .journal_event_count()
            .expect("startup journal should load"),
        7,
        "reopening must finalize the expired presence before a query runs",
    );
    let query = request(Uuid::new_v4(), "agent-1", "actor-1", ActorType::Agent, ());
    assert!(
        store
            .presence_for_request(&query, "agent-1")
            .expect("lazy query should succeed")
            .is_none()
    );
    let event_count = store
        .journal_event_count()
        .expect("journal count should load");
    assert!(handoff(&mut store, "agent-1").is_some());
    assert!(
        store
            .presence_for_request(
                &request(Uuid::new_v4(), "agent-1", "actor-1", ActorType::Agent, ()),
                "agent-1"
            )
            .expect("repeat lazy query should succeed")
            .is_none()
    );
    assert_eq!(
        store
            .journal_event_count()
            .expect("journal count should load"),
        event_count
    );
}

#[test]
fn startup_expires_fallback_and_explicit_handoffs_at_their_relevance_windows() {
    let temp = TempDir::new().expect("temporary directory should exist");
    let path = temp.path().join("handoff.sqlite");
    let clock = MutableClock::new(NOW);
    {
        let mut store = Store::open_with_clock(&path, clock.clone()).expect("store should open");
        for agent_id in ["explicit", "fallback"] {
            store
                .register_presence(&register_request(
                    Uuid::new_v4(),
                    agent_id,
                    &format!("{agent_id}-actor"),
                    ActorType::Agent,
                    None,
                ))
                .expect("registration should succeed");
        }
        store
            .finalize_handoff(&request(
                Uuid::new_v4(),
                "explicit",
                "explicit-actor",
                ActorType::Agent,
                ExplicitHandoff {
                    status: HandoffStatus::Done,
                    summary: "complete".into(),
                    files_changed: vec![],
                    tests_run: vec![],
                    remaining_work: vec![],
                    next_plan: None,
                },
            ))
            .expect("explicit handoff should succeed");
        store
            .stop_presence(&request(
                Uuid::new_v4(),
                "fallback",
                "fallback-actor",
                ActorType::Agent,
                (),
            ))
            .expect("fallback should succeed");
    }
    clock.advance(Duration::hours(25));
    let mut store = Store::open_with_clock(&path, clock.clone())
        .expect("reopened store should expire fallback");
    assert_eq!(
        store
            .journal_event_count()
            .expect("startup journal should load"),
        15
    );
    let query = request(
        Uuid::new_v4(),
        "explicit",
        "explicit-actor",
        ActorType::Agent,
        (),
    );
    assert!(
        store
            .handoff_for_request(&query, "fallback")
            .expect("fallback query should succeed")
            .is_none()
    );
    assert!(
        store
            .handoff_for_request(&query, "explicit")
            .expect("explicit query should succeed")
            .is_some()
    );
    drop(store);

    clock.advance(Duration::days(7));
    let mut store = Store::open_with_clock(&path, clock.clone())
        .expect("reopened store should expire explicit handoff");
    assert_eq!(
        store
            .journal_event_count()
            .expect("startup journal should load"),
        16
    );
    assert!(
        store
            .handoff_for_request(&query, "explicit")
            .expect("explicit query should succeed")
            .is_none()
    );
}

#[test]
fn startup_coalesces_an_expired_handoff_and_presence_into_one_fallback() {
    let temp = TempDir::new().expect("temporary directory should exist");
    let path = temp.path().join("coalesced-expiry.sqlite");
    let clock = MutableClock::new(NOW);
    {
        let mut store = Store::open_with_clock(&path, clock.clone()).expect("store should open");
        store
            .register_presence(&register_request(
                Uuid::new_v4(),
                "agent-1",
                "actor-1",
                ActorType::Agent,
                None,
            ))
            .expect("registration should succeed");
        store
            .stop_presence(&request(
                Uuid::new_v4(),
                "agent-1",
                "actor-1",
                ActorType::Agent,
                (),
            ))
            .expect("fallback should succeed");
        store
            .register_presence(&register_request(
                Uuid::new_v4(),
                "agent-1",
                "actor-1",
                ActorType::Agent,
                None,
            ))
            .expect("replacement presence should succeed");
    }
    clock.advance(Duration::hours(25));
    let mut store = Store::open_with_clock(&path, clock.clone())
        .expect("reopened store should coalesce expiry");

    assert!(presence(&mut store, "agent-1").is_none());
    let fallback = handoff(&mut store, "agent-1").expect("replacement fallback should exist");
    assert!(!fallback.explicit);
    assert!(fallback.summary.contains("no explicit handoff"));
    assert_eq!(
        store
            .journal_event_count()
            .expect("journal count should load"),
        15
    );
}

#[test]
fn non_stale_reads_and_reopens_do_not_create_expiry_receipts() {
    let temp = TempDir::new().expect("temporary directory should exist");
    let path = temp.path().join("no-op-expiry.sqlite");
    let clock = MutableClock::new(NOW);
    {
        let mut store = Store::open_with_clock(&path, clock.clone()).expect("store should open");
        store
            .register_presence(&register_request(
                Uuid::new_v4(),
                "agent-1",
                "actor-1",
                ActorType::Agent,
                None,
            ))
            .expect("registration should succeed");
        let before = store
            .command_receipt_count()
            .expect("receipt count should load");
        assert!(presence(&mut store, "agent-1").is_some());
        assert_eq!(
            store
                .command_receipt_count()
                .expect("receipt count should load"),
            before
        );
    }
    let store = Store::open_with_clock(&path, clock).expect("unchanged reopen should succeed");
    assert_eq!(
        store
            .journal_event_count()
            .expect("journal count should load"),
        1
    );
    assert_eq!(
        store
            .command_receipt_count()
            .expect("receipt count should load"),
        1
    );
}

#[test]
fn expiry_preflight_compares_offset_timestamps_as_instants() {
    let clock = MutableClock::new(NOW);
    let mut store = Store::open_in_memory_with_clock(clock.clone()).expect("store should open");
    store
        .register_presence(&register_request(
            Uuid::new_v4(),
            "expired",
            "expired-actor",
            ActorType::Agent,
            None,
        ))
        .expect("registration should succeed");
    store
        .start_presence_tool(&request(
            Uuid::new_v4(),
            "expired",
            "expired-actor",
            ActorType::Agent,
            PresenceToolStart {
                tool_name: "tool".into(),
                deadline: Some(
                    OffsetDateTime::parse("2026-07-15T15:00:00+02:00", &Rfc3339)
                        .expect("deadline should parse"),
                ),
            },
        ))
        .expect("tool should start");
    clock.advance(Duration::minutes(61));
    assert!(presence(&mut store, "expired").is_none());

    let future_clock = MutableClock::new(NOW);
    let mut future_store =
        Store::open_in_memory_with_clock(future_clock.clone()).expect("store should open");
    future_store
        .register_presence(&register_request(
            Uuid::new_v4(),
            "future",
            "future-actor",
            ActorType::Agent,
            None,
        ))
        .expect("registration should succeed");
    future_store
        .start_presence_tool(&request(
            Uuid::new_v4(),
            "future",
            "future-actor",
            ActorType::Agent,
            PresenceToolStart {
                tool_name: "tool".into(),
                deadline: Some(
                    OffsetDateTime::parse("2026-07-15T10:45:00-02:00", &Rfc3339)
                        .expect("deadline should parse"),
                ),
            },
        ))
        .expect("tool should start");
    future_clock.advance(Duration::minutes(20));
    let receipts = future_store
        .command_receipt_count()
        .expect("receipt count should load");
    assert!(presence(&mut future_store, "future").is_some());
    assert_eq!(
        future_store
            .command_receipt_count()
            .expect("receipt count should load"),
        receipts
    );
}

#[test]
fn activity_finalization_creates_one_replayable_fallback_handoff() {
    let mut store = store();
    store
        .register_presence(&register_request(
            Uuid::new_v4(),
            "agent-1",
            "actor-1",
            ActorType::Agent,
            None,
        ))
        .expect("presence should register");
    let finalization = request(
        Uuid::new_v4(),
        "agent-1",
        "actor-1",
        ActorType::Agent,
        ActivityFinalization {},
    );

    store
        .finalize_activity(&finalization)
        .expect("activity should finalize");

    assert_eq!(
        store
            .journal_event_types_for_request(finalization.request_id)
            .expect("journal should load"),
        vec![
            "handoff.finalized",
            "presence.finalized",
            "reservation.released",
            "claim.released",
            "wait.cancelled",
            "write_fence.released",
        ],
    );
    assert!(
        !handoff(&mut store, "agent-1")
            .expect("fallback should exist")
            .explicit
    );
    assert!(presence(&mut store, "agent-1").is_none());
    store
        .rebuild_projections()
        .expect("fallback finalization should replay");

    let before = store.journal_event_count().expect("journal should count");
    let repeated = store
        .finalize_activity(&finalization)
        .expect("receipt should replay");
    assert!(repeated.duplicate);
    assert_eq!(
        store.journal_event_count().expect("journal should count"),
        before
    );
}

#[test]
fn re_registration_rejects_every_changed_immutable_identity_without_journal_mutation() {
    let mut store = store();
    store
        .register_presence(&register_request(
            Uuid::new_v4(),
            "agent-1",
            "actor-1",
            ActorType::Agent,
            None,
        ))
        .expect("presence should register");
    let before = store.journal_event_count().expect("journal should count");
    let base = register_request(Uuid::new_v4(), "agent-1", "actor-1", ActorType::Agent, None);

    for mut changed in [
        {
            let mut request = base.clone();
            request.agent.actor_id = "different-actor".into();
            request
        },
        {
            let mut request = base.clone();
            request.agent.actor_type = ActorType::System;
            request
        },
        {
            let mut request = base.clone();
            request.agent.owner_id = Some("different-owner".into());
            request
        },
        {
            let mut request = base.clone();
            request.agent.parent_agent_id = Some("different-parent-agent".into());
            request
        },
        {
            let mut request = base;
            request.agent.parent_actor_id = Some("different-parent-actor".into());
            request
        },
    ] {
        changed.request_id = Uuid::new_v4();
        assert!(
            store.register_presence(&changed).is_err(),
            "changed identity must reject"
        );
        assert_eq!(
            store.journal_event_count().expect("journal should count"),
            before
        );
    }
    assert_eq!(
        presence(&mut store, "agent-1")
            .expect("presence should remain")
            .actor_id,
        "actor-1"
    );
}

#[test]
fn different_actor_cannot_finalize_or_stop_a_live_same_agent_presence() {
    let mut store = store();
    store
        .register_presence(&register_request(
            Uuid::new_v4(),
            "agent-1",
            "actor-1",
            ActorType::Agent,
            None,
        ))
        .expect("presence should register");
    let before = store.journal_event_count().expect("journal should count");
    let explicit_handoff = ExplicitHandoff {
        status: HandoffStatus::Done,
        summary: "done".into(),
        files_changed: vec![],
        tests_run: vec![],
        remaining_work: vec![],
        next_plan: None,
    };

    assert!(
        store
            .finalize_handoff(&request(
                Uuid::new_v4(),
                "agent-1",
                "different-actor",
                ActorType::Agent,
                explicit_handoff,
            ))
            .is_err()
    );
    assert!(
        store
            .stop_presence(&request(
                Uuid::new_v4(),
                "agent-1",
                "different-actor",
                ActorType::Agent,
                (),
            ))
            .is_err()
    );

    assert_eq!(
        store.journal_event_count().expect("journal should count"),
        before
    );
    assert!(presence(&mut store, "agent-1").is_some());
    assert!(handoff(&mut store, "agent-1").is_none());
}

#[test]
fn explicit_stop_and_ttl_finalization_promote_queued_successors() {
    for mode in ["explicit", "stop", "ttl"] {
        let clock = MutableClock::new(NOW);
        let mut store = Store::open_in_memory_with_clock(clock.clone()).expect("store should open");
        store
            .register_presence(&register_request(
                Uuid::new_v4(),
                "owner",
                "owner-actor",
                ActorType::Agent,
                None,
            ))
            .expect("owner should register");
        store
            .declare_reservation(&request(
                Uuid::new_v4(),
                "owner",
                "owner-actor",
                ActorType::Agent,
                ReservationDeclaration {
                    scopes: vec![stateful_core::ReservationScope::file("src/lib.rs")],
                    action: "write_file".into(),
                    purpose: "Own the file.".into(),
                },
            ))
            .expect("owner reservation should declare");
        let wait = store
            .request_wait(&request(
                Uuid::new_v4(),
                "successor",
                "successor-actor",
                ActorType::Agent,
                WaitRequest {
                    relative_path: "src/lib.rs".into(),
                    action: "write_file".into(),
                    purpose: "Need the file.".into(),
                    blocking_agent_id: Some("owner".into()),
                },
            ))
            .expect("successor should queue")
            .response;

        match mode {
            "explicit" => {
                store
                    .finalize_handoff(&request(
                        Uuid::new_v4(),
                        "owner",
                        "owner-actor",
                        ActorType::Agent,
                        ExplicitHandoff {
                            status: HandoffStatus::Done,
                            summary: "done".into(),
                            files_changed: vec![],
                            tests_run: vec![],
                            remaining_work: vec![],
                            next_plan: None,
                        },
                    ))
                    .expect("explicit finalization should succeed");
            }
            "stop" => {
                store
                    .stop_presence(&request(
                        Uuid::new_v4(),
                        "owner",
                        "owner-actor",
                        ActorType::Agent,
                        (),
                    ))
                    .expect("stop should succeed");
            }
            "ttl" => {
                clock.advance(Duration::minutes(16));
                store
                    .expire_stale_presences(&request(
                        Uuid::new_v4(),
                        "owner",
                        "owner-actor",
                        ActorType::Agent,
                        (),
                    ))
                    .expect("ttl expiry should succeed");
            }
            _ => unreachable!(),
        }

        assert_eq!(
            store
                .wait("workspace-1", &wait.wait_id)
                .expect("wait should load")
                .expect("wait should exist")
                .status,
            "claimable",
            "{mode} finalization should promote the successor",
        );
        store
            .rebuild_projections()
            .expect("finalization should replay");
    }
}

#[test]
fn resumed_presence_replaces_an_older_relevant_fallback_at_its_next_stop() {
    let mut store = store();
    store
        .register_presence(&register_request(
            Uuid::new_v4(),
            "agent-1",
            "actor-1",
            ActorType::Agent,
            None,
        ))
        .expect("first presence should register");
    store
        .stop_presence(&request(
            Uuid::new_v4(),
            "agent-1",
            "actor-1",
            ActorType::Agent,
            (),
        ))
        .expect("first stop should finalize");
    let first = handoff(&mut store, "agent-1").expect("first fallback should exist");
    store
        .resume_presence(&register_request(
            Uuid::new_v4(),
            "agent-1",
            "actor-1",
            ActorType::Agent,
            None,
        ))
        .expect("presence should resume beside the relevant handoff");
    let second_stop = request(Uuid::new_v4(), "agent-1", "actor-1", ActorType::Agent, ());

    store
        .stop_presence(&second_stop)
        .expect("resumed presence should finalize");

    assert!(
        store
            .journal_event_types_for_request(second_stop.request_id)
            .expect("journal should load")
            .contains(&"handoff.finalized".into())
    );
    assert!(
        handoff(&mut store, "agent-1")
            .expect("replacement fallback should exist")
            .origin_event_seq
            > first.origin_event_seq,
    );
}

#[test]
fn re_registration_cannot_change_identity_while_a_relevant_handoff_is_retained() {
    let mut store = store();
    store
        .register_presence(&register_request(
            Uuid::new_v4(),
            "agent-1",
            "actor-1",
            ActorType::Agent,
            None,
        ))
        .expect("presence should register");
    store
        .stop_presence(&request(
            Uuid::new_v4(),
            "agent-1",
            "actor-1",
            ActorType::Agent,
            (),
        ))
        .expect("presence should stop");
    let before = store.journal_event_count().expect("journal should count");

    assert!(
        store
            .register_presence(&register_request(
                Uuid::new_v4(),
                "agent-1",
                "different-actor",
                ActorType::Agent,
                None,
            ))
            .is_err()
    );

    assert_eq!(
        store.journal_event_count().expect("journal should count"),
        before
    );
}

#[test]
fn activity_start_rejects_changed_identity_for_live_or_retained_presence() {
    let mut live_store = store();
    live_store
        .register_presence(&register_request(
            Uuid::new_v4(),
            "agent-1",
            "actor-1",
            ActorType::Agent,
            None,
        ))
        .expect("presence should register");
    let before_live = live_store
        .journal_event_count()
        .expect("journal should count");

    assert!(
        live_store
            .start_activity(&request(
                Uuid::new_v4(),
                "agent-1",
                "different-actor",
                ActorType::Agent,
                ActivityStart {
                    phase: PresencePhase::Editing
                },
            ))
            .is_err()
    );

    assert_eq!(
        live_store
            .journal_event_count()
            .expect("journal should count"),
        before_live
    );
    assert_eq!(
        presence(&mut live_store, "agent-1")
            .expect("presence should remain")
            .actor_id,
        "actor-1"
    );

    let mut retained_store = store();
    retained_store
        .register_presence(&register_request(
            Uuid::new_v4(),
            "agent-1",
            "actor-1",
            ActorType::Agent,
            None,
        ))
        .expect("presence should register");
    retained_store
        .stop_presence(&request(
            Uuid::new_v4(),
            "agent-1",
            "actor-1",
            ActorType::Agent,
            (),
        ))
        .expect("presence should stop");
    let before_retained = retained_store
        .journal_event_count()
        .expect("journal should count");

    assert!(
        retained_store
            .start_activity(&request(
                Uuid::new_v4(),
                "agent-1",
                "different-actor",
                ActorType::Agent,
                ActivityStart {
                    phase: PresencePhase::Editing
                },
            ))
            .is_err()
    );

    assert_eq!(
        retained_store
            .journal_event_count()
            .expect("journal should count"),
        before_retained,
    );
    assert_eq!(
        handoff(&mut retained_store, "agent-1")
            .expect("handoff should remain")
            .actor_id,
        "actor-1",
    );
}

#[test]
fn finalization_keeps_same_agent_fence_owned_by_a_different_actor() {
    let mut store = store();
    store
        .register_presence(&register_request(
            Uuid::new_v4(),
            "agent-1",
            "presence-owner",
            ActorType::Agent,
            None,
        ))
        .expect("presence registers");
    store
        .start_activity(&request(
            Uuid::new_v4(),
            "agent-1",
            "presence-owner",
            ActorType::Agent,
            ActivityStart {
                phase: PresencePhase::Editing,
            },
        ))
        .expect("owner activity starts");
    store
        .acquire_write_fences(&request(
            Uuid::new_v4(),
            "agent-1",
            "fence-owner",
            ActorType::Agent,
            WriteFenceAcquire {
                paths: vec!["src/held.rs".into()],
                action: "write_file".into(),
            },
        ))
        .expect("other actor fence acquires");

    store
        .finalize_activity(&request(
            Uuid::new_v4(),
            "agent-1",
            "presence-owner",
            ActorType::Agent,
            ActivityFinalization {},
        ))
        .expect("presence finalizes");

    assert!(
        store
            .active_write_fence("workspace-1", "src/held.rs")
            .expect("fence loads")
            .is_some()
    );
}

#[test]
fn expired_presence_exact_read_finalizes_before_reopening_resource_presence_and_replays() {
    let clock = MutableClock::new(NOW);
    let mut store = Store::open_in_memory_with_clock(clock.clone()).expect("store opens");
    store
        .register_presence(&register_request(
            Uuid::new_v4(),
            "agent-1",
            "actor-1",
            ActorType::Agent,
            None,
        ))
        .expect("presence registers");
    clock.advance(Duration::minutes(61));
    let content = stateful_core::fingerprint_reader(std::io::Cursor::new(b"content"))
        .expect("content fingerprint");
    store
        .start_read_observation(&request(
            Uuid::new_v4(),
            "agent-1",
            "actor-1",
            ActorType::Agent,
            ReadObservationStart {
                operation_id: "expired-read".into(),
                path: "src/expired.rs".into(),
                before: content.clone(),
            },
        ))
        .expect("read starts");
    let complete = request(
        Uuid::new_v4(),
        "agent-1",
        "actor-1",
        ActorType::Agent,
        ReadCompletion {
            operation_id: "expired-read".into(),
            path: "src/expired.rs".into(),
            classification: ReadClassification::Exact,
            after: Some(content),
            semantic_marker: None,
        },
    );
    store
        .complete_read_observation(&complete)
        .expect("exact completion finalizes then reopens lifecycle presence");
    let events = store
        .journal_event_types_for_request(complete.request_id)
        .expect("completion event types load");
    let finalized = events
        .iter()
        .position(|event| event == "presence.finalized")
        .expect("presence finalizes");
    let read = events
        .iter()
        .position(|event| event == "read_observation.stabilized")
        .expect("read stabilizes");
    let resource = events
        .iter()
        .position(|event| event == "presence.resources_updated")
        .expect("read resource updates");
    assert!(finalized < read && read < resource);
    assert!(presence(&mut store, "agent-1").is_some());
    assert!(resources(&mut store, "agent-1").iter().any(|resource| {
        resource.relative_path == "src/expired.rs"
            && resource.relation == PresenceResourceRelation::Read
    }));
    store
        .rebuild_projections()
        .expect("empty replay matches live projections");
}
