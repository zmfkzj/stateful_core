use stateful_core::{
    ActorType, AgentIdentity, ContentFingerprint, Decision, DecisionKind, PresencePhase,
    PresenceResourceRelation, ReadClassification, ReadCompletion, ReadObservationStart,
    RequestEnvelope, SourceKind, SourceRef, WorkspaceIdentity,
    WriteIntentCompletion, WriteIntentStart, WriteIntentStatus, WriteTarget,
};
use stateful_store::{ActivityFinalization, ActivityStart, FixedClock, Store, WriteFenceRelease};
use time::{Duration, macros::datetime, OffsetDateTime};
use uuid::Uuid;

const NOW: OffsetDateTime = datetime!(2026-07-15 12:00 UTC);

fn fingerprint(bytes: &[u8]) -> ContentFingerprint {
    stateful_core::fingerprint_reader(std::io::Cursor::new(bytes))
        .expect("test content fingerprints")
}

fn request<T: serde::Serialize>(agent_id: &str, payload: T) -> RequestEnvelope<T> {
    request_as(agent_id, &format!("actor-{agent_id}"), payload)
}

fn request_as<T: serde::Serialize>(agent_id: &str, actor_id: &str, payload: T) -> RequestEnvelope<T> {
    RequestEnvelope::new(
        Uuid::new_v4(),
        NOW,
        AgentIdentity {
            agent_id: agent_id.into(),
            turn_id: Some("turn-1".into()),
            actor_id: actor_id.into(),
            actor_type: ActorType::Agent,
            owner_id: None,
            parent_agent_id: None,
            parent_actor_id: None,
        },
        WorkspaceIdentity {
            root: "/repo".into(),
            workspace_id: "workspace-1".into(),
            repo_id: "repo-1".into(),
            worktree_id: "worktree-1".into(),
            branch: "main".into(),
        },
        SourceRef {
            kind: SourceKind::Hook,
            event: "test".into(),
            tool_name: Some("cat".into()),
            source_ref: "freshness-test".into(),
        },
        payload,
    )
    .expect("test request should be valid")
}

fn start_read(store: &Store, agent_id: &str, operation_id: &str, path: &str, before: ContentFingerprint) {
    store
        .start_read_observation(&request(
            agent_id,
            ReadObservationStart {
                operation_id: operation_id.into(),
                path: path.into(),
                before,
            },
        ))
        .expect("read start should journal");
}

fn complete_read(
    store: &Store,
    agent_id: &str,
    operation_id: &str,
    path: &str,
    classification: ReadClassification,
    after: Option<ContentFingerprint>,
    semantic_marker: Option<&str>,
) {
    store
        .complete_read_observation(&request(
            agent_id,
            ReadCompletion {
                operation_id: operation_id.into(),
                path: path.into(),
                classification,
                after,
                semantic_marker: semantic_marker.map(str::to_owned),
            },
        ))
        .expect("read completion should journal");
}

#[test]
fn exact_read_completion_projects_a_read_presence_relation_from_its_event() {
    let mut store = Store::open_in_memory_with_clock(FixedClock::new(NOW)).expect("store opens");
    let content = fingerprint(b"same bytes");

    start_read(&store, "agent-1", "read-1", "src/lib.rs", content.clone());
    let completion = request(
        "agent-1",
        ReadCompletion {
            operation_id: "read-1".into(),
            path: "src/lib.rs".into(),
            classification: ReadClassification::Exact,
            after: Some(content),
            semantic_marker: None,
        },
    );
    let outcome = store
        .complete_read_observation(&completion)
        .expect("exact read completion succeeds");

    let observation = store
        .read_observation("workspace-1", "agent-1", "src/lib.rs")
        .expect("observation reads")
        .expect("observation exists");
    assert!(observation.is_stable());
    assert_eq!(observation.operation_id, "read-1");
    assert_eq!(observation.resource_version, 0);
    assert_eq!(observation.expires_at, Some(NOW + Duration::minutes(60)));
    let resource = store
        .presence_resources_for_request(&request("agent-1", ()), "agent-1")
        .expect("presence resources load")
        .into_iter()
        .find(|resource| {
            resource.relative_path == "src/lib.rs"
                && resource.relation == PresenceResourceRelation::Read
        })
        .expect("exact read projects its resource relation");
    assert_eq!(resource.origin_event_seq, outcome.last_event_seq.expect("resource event sequence"));
    assert_eq!(
        store
            .journal_event_types_for_request(completion.request_id)
            .expect("event types load"),
        vec!["read_observation.stabilized", "presence.resources_updated"],
    );
    store.rebuild_projections().expect("read observation must replay");
    assert!(observation.is_fresh_at(NOW + Duration::minutes(60) - Duration::seconds(1)));
    assert!(!observation.is_fresh_at(NOW + Duration::minutes(60)));
}

#[test]
fn partial_truncated_structural_failed_and_ambiguous_reads_never_stabilize() {
    let mut store = Store::open_in_memory_with_clock(FixedClock::new(NOW)).expect("store opens");
    let content = fingerprint(b"same bytes");
    for (index, classification, marker) in [
        ("partial", ReadClassification::Partial, None),
        ("truncated", ReadClassification::Truncated, None),
        ("structural", ReadClassification::StructuralSummary, None),
        ("failed", ReadClassification::Failed, None),
        ("ambiguous", ReadClassification::Ambiguous, None),
    ] {
        let path = format!("src/{index}.rs");
        start_read(&store, "agent-1", index, &path, content.clone());
        complete_read(
            &store,
            "agent-1",
            index,
            &path,
            classification,
            Some(content.clone()),
            marker,
        );
        assert!(
            !store
                .read_observation("workspace-1", "agent-1", &path)
                .expect("observation reads")
                .expect("observation exists")
                .is_stable(),
            "{index} must never become authority"
        );
        assert!(
            !store
                .presence_resources_for_request(&request("agent-1", ()), "agent-1")
                .expect("presence resources load")
                .iter()
                .any(|resource| {
                    resource.relative_path == path
                        && resource.relation == PresenceResourceRelation::Read
                }),
            "{index} must not claim an exact read relation"
        );
    }
}
#[test]
fn write_intent_start_projects_planned_resources_for_every_target_in_order() {
    let mut store = Store::open_in_memory_with_clock(FixedClock::new(NOW)).expect("store opens");
    let start = request(
        "agent-1",
        WriteIntentStart {
            operation_id: "write-1".into(),
            action: "write_file".into(),
            targets: vec![
                WriteTarget { path: "src/a.rs".into(), before: fingerprint(b"a") },
                WriteTarget { path: "src/b.rs".into(), before: fingerprint(b"b") },
            ],
        },
    );

    let first = store.start_write_intent(&start).expect("intent starts");
    let duplicate = store.start_write_intent(&start).expect("duplicate receipt returns");
    assert!(duplicate.duplicate);
    assert_eq!(duplicate.response, first.response);
    assert_eq!(
        store
            .journal_event_types_for_request(start.request_id)
            .expect("event types load"),
        vec![
            "write_intent.started",
            "presence.resources_updated",
            "presence.resources_updated",
            "write_fence.acquired",
            "write_fence.acquired",
        ],
    );
    let planned = store
        .presence_resources_for_request(&request("agent-1", ()), "agent-1")
        .expect("presence resources load")
        .into_iter()
        .filter(|resource| resource.relation == PresenceResourceRelation::Planned)
        .map(|resource| resource.relative_path)
        .collect::<Vec<_>>();
    assert_eq!(planned, vec!["src/a.rs", "src/b.rs"]);
    let completion = request(
        "agent-1",
        WriteIntentCompletion::committed(
            first.response.intent_id.clone(),
            vec![
                ("src/a.rs".into(), fingerprint(b"a changed")),
                ("src/b.rs".into(), fingerprint(b"b changed")),
            ],
        ),
    );
    store
        .complete_write_intent(&completion)
        .expect("multi-target write commits");
    assert_eq!(
        store
            .journal_event_types_for_request(completion.request_id)
            .expect("event types load"),
        vec![
            "write_intent.committed",
            "presence.resources_updated",
            "presence.resources_updated",
            "presence.tool_completed",
            "write_fence.released",
            "write_fence.released",
        ],
    );
    let changed = store
        .presence_resources_for_request(&request("agent-1", ()), "agent-1")
        .expect("presence resources load")
        .into_iter()
        .filter(|resource| resource.relation == PresenceResourceRelation::Changed)
        .map(|resource| resource.relative_path)
        .collect::<Vec<_>>();
    assert_eq!(changed, vec!["src/a.rs", "src/b.rs"]);
}

#[test]
fn file_change_during_read_records_unstable_observation() {
    let store = Store::open_in_memory_with_clock(FixedClock::new(NOW)).expect("store opens");
    start_read(&store, "agent-1", "read-1", "src/lib.rs", fingerprint(b"before"));
    complete_read(
        &store,
        "agent-1",
        "read-1",
        "src/lib.rs",
        ReadClassification::Exact,
        Some(fingerprint(b"after")),
        None,
    );

    assert!(!store
        .read_observation("workspace-1", "agent-1", "src/lib.rs")
        .expect("observation reads")
        .expect("observation exists")
        .is_stable());
}

#[test]
fn read_completion_rejects_another_actor_in_the_same_agent_session() {
    let store = Store::open_in_memory_with_clock(FixedClock::new(NOW)).expect("store opens");
    let content = fingerprint(b"same bytes");
    store
        .start_read_observation(&request_as(
            "agent-1",
            "actor-started",
            ReadObservationStart {
                operation_id: "read-1".into(),
                path: "src/lib.rs".into(),
                before: content.clone(),
            },
        ))
        .expect("read starts");

    let error = store
        .complete_read_observation(&request_as(
            "agent-1",
            "actor-completing",
            ReadCompletion {
                operation_id: "read-1".into(),
                path: "src/lib.rs".into(),
                classification: ReadClassification::Exact,
                after: Some(content),
                semantic_marker: None,
            },
        ))
        .expect_err("a different actor must not complete the read");

    assert!(matches!(error, stateful_store::StoreError::ReadOperationNotFound));
}

#[test]
fn same_content_write_between_read_start_and_completion_keeps_the_read_unstable() {
    let store = Store::open_in_memory_with_clock(FixedClock::new(NOW)).expect("store opens");
    let content = fingerprint(b"same bytes");
    start_read(&store, "agent-1", "read-1", "src/lib.rs", content.clone());
    let intent = store
        .start_write_intent(&request(
            "agent-2",
            WriteIntentStart {
                operation_id: "write-1".into(),
                action: "write_file".into(),
                targets: vec![WriteTarget { path: "src/lib.rs".into(), before: content.clone() }],
            },
        ))
        .expect("intervening write starts")
        .response;
    store
        .complete_write_intent(&request(
            "agent-2",
            WriteIntentCompletion::committed(
                intent.intent_id,
                vec![("src/lib.rs".into(), content.clone())],
            ),
        ))
        .expect("same-content write commits and increments its resource version");
    complete_read(
        &store,
        "agent-1",
        "read-1",
        "src/lib.rs",
        ReadClassification::Exact,
        Some(content),
        None,
    );

    let observation = store
        .read_observation("workspace-1", "agent-1", "src/lib.rs")
        .expect("observation reads")
        .expect("observation exists");
    assert_eq!(observation.status, stateful_core::ReadObservationStatus::Unstable);
    assert_eq!(observation.resource_version, 0, "the persisted version belongs to read start");
}

#[test]
fn incomplete_present_fingerprint_cannot_stabilize_an_exact_read() {
    let store = Store::open_in_memory_with_clock(FixedClock::new(NOW)).expect("store opens");
    let incomplete = ContentFingerprint {
        exists: true,
        byte_len: 4,
        sha256: Some("not-a-sha256".into()),
    };
    start_read(&store, "agent-1", "read-1", "src/lib.rs", incomplete.clone());
    complete_read(
        &store,
        "agent-1",
        "read-1",
        "src/lib.rs",
        ReadClassification::Exact,
        Some(incomplete),
        None,
    );

    assert!(!store
        .read_observation("workspace-1", "agent-1", "src/lib.rs")
        .expect("observation reads")
        .expect("observation exists")
        .is_stable());
}

#[test]
fn concurrent_read_operations_pair_by_operation_id_not_latest_path() {
    let store = Store::open_in_memory_with_clock(FixedClock::new(NOW)).expect("store opens");
    let first = fingerprint(b"first");
    let second = fingerprint(b"second");
    start_read(&store, "agent-1", "first-op", "src/lib.rs", first.clone());
    start_read(&store, "agent-1", "second-op", "src/lib.rs", second.clone());

    complete_read(
        &store,
        "agent-1",
        "first-op",
        "src/lib.rs",
        ReadClassification::Exact,
        Some(first),
        None,
    );
    assert!(store
        .read_observation("workspace-1", "agent-1", "src/lib.rs")
        .expect("observation reads")
        .expect("first read exists")
        .is_stable());

    complete_read(
        &store,
        "agent-1",
        "second-op",
        "src/lib.rs",
        ReadClassification::Exact,
        Some(fingerprint(b"changed")),
        None,
    );
    let latest = store
        .read_observation("workspace-1", "agent-1", "src/lib.rs")
        .expect("observation reads")
        .expect("second read exists");
    assert_eq!(latest.operation_id, "second-op");
    assert!(!latest.is_stable());
}

#[test]
fn structural_summary_with_semantic_marker_remains_unstable() {
    let store = Store::open_in_memory_with_clock(FixedClock::new(NOW)).expect("store opens");
    start_read(&store, "agent-1", "structural", "src/lib.rs", fingerprint(b"before"));
    complete_read(
        &store,
        "agent-1",
        "structural",
        "src/lib.rs",
        ReadClassification::StructuralSummary,
        Some(fingerprint(b"after")),
        Some("symbols:fn main"),
    );

    assert!(!store
        .read_observation("workspace-1", "agent-1", "src/lib.rs")
        .expect("observation reads")
        .expect("observation exists")
        .is_stable());
}

#[test]
fn authorize_starts_intent_and_fence_in_one_transaction() {
    let mut store = Store::open_in_memory_with_clock(FixedClock::new(NOW)).expect("store opens");
    store.fail_projector_on_event_for_tests(2);
    let failure = store.start_write_intent(&request(
        "agent-1",
        WriteIntentStart {
            operation_id: "write-1".into(),
            action: "write_file".into(),
            targets: vec![WriteTarget {
                path: "src/lib.rs".into(),
                before: fingerprint(b"before"),
            }],
        },
    ));
    assert!(failure.is_err(), "projector failure must roll back both intent and fence");
    assert_eq!(store.journal_event_count().expect("journal counts"), 0);

    store.fail_projector_on_event_for_tests(0);
    let started = store
        .start_write_intent(&request(
            "agent-1",
            WriteIntentStart {
                operation_id: "write-1".into(),
                action: "write_file".into(),
                targets: vec![WriteTarget {
                    path: "src/lib.rs".into(),
                    before: fingerprint(b"before"),
                }],
            },
        ))
        .expect("intent starts");
    assert!(started.response.intent_id.len() > 1);
    assert_eq!(started.response.fence_ids.len(), 1);
    assert!(store
        .active_write_intent("workspace-1", "src/lib.rs")
        .expect("intent reads")
        .is_some());
}

#[test]
fn committed_write_projects_changed_resources_and_never_synthesizes_a_read() {
    let mut store = Store::open_in_memory_with_clock(FixedClock::new(NOW)).expect("store opens");
    let before = fingerprint(b"before");
    start_read(&store, "agent-2", "read-2", "src/lib.rs", before.clone());
    complete_read(
        &store,
        "agent-2",
        "read-2",
        "src/lib.rs",
        ReadClassification::Exact,
        Some(before.clone()),
        None,
    );
    let intent = store
        .start_write_intent(&request(
            "agent-1",
            WriteIntentStart {
                operation_id: "write-1".into(),
                action: "write_file".into(),
                targets: vec![WriteTarget { path: "src/lib.rs".into(), before }],
            },
        ))
        .expect("intent starts")
        .response;
    let completion = request(
        "agent-1",
        WriteIntentCompletion::committed(
            intent.intent_id.clone(),
            vec![("src/lib.rs".into(), fingerprint(b"after"))],
        ),
    );
    let committed = store
        .complete_write_intent(&completion)
        .expect("write commits");
    let duplicate = store
        .complete_write_intent(&completion)
        .expect("duplicate completion returns its frozen receipt");
    assert!(duplicate.duplicate);
    assert_eq!(duplicate.response, committed.response);
    assert_eq!(
        store
            .journal_event_types_for_request(completion.request_id)
            .expect("event types load"),
        vec![
            "write_intent.committed",
            "read_observation.invalidated",
            "presence.resources_updated",
            "presence.tool_completed",
            "write_fence.released",
        ],
    );

    let resource_version = store
        .resource_version("workspace-1", "src/lib.rs")
        .expect("resource version reads")
        .expect("resource version exists");
    assert_eq!(resource_version.version, 1);
    assert!(resource_version.origin_event_seq > 0);
    assert!(!store
        .read_observation("workspace-1", "agent-2", "src/lib.rs")
        .expect("observation reads")
        .expect("observation remains auditable")
        .is_stable());
    assert!(
        store
            .read_observation("workspace-1", "agent-1", "src/lib.rs")
            .expect("writer observation reads")
            .is_none(),
        "a write completion is not an exact read"
    );
    let resource = store
        .presence_resources_for_request(&request("agent-1", ()), "agent-1")
        .expect("presence resources load")
        .into_iter()
        .find(|resource| {
            resource.relative_path == "src/lib.rs"
                && resource.relation == PresenceResourceRelation::Changed
        })
        .expect("changed resource is projected");
    assert_eq!(resource.origin_event_seq, committed.last_event_seq.expect("completion sequence") - 2);
    let presence = store
        .presence_for_request(&request("agent-1", ()), "agent-1")
        .expect("presence loads")
        .expect("writer presence exists");
    let last_result: serde_json::Value = serde_json::from_str(
        presence.last_result.as_deref().expect("committed write records a tool result"),
    )
    .expect("tool result is valid JSON");
    assert_eq!(last_result["tool_name"], "write_file");
    assert_eq!(last_result["outcome"], "committed");
    store.rebuild_projections().expect("write version must replay");
    assert_eq!(
        store
            .resource_version("workspace-1", "src/lib.rs")
            .expect("resource version reads")
            .expect("resource version survives replay")
            .version,
        1
    );
    store
        .finalize_activity(&request("agent-1", ActivityFinalization {}))
        .expect("writer activity finalizes");
    assert_eq!(
        store
            .handoff_for_request(&request("agent-1", ()), "agent-1")
            .expect("fallback handoff loads")
            .expect("fallback handoff exists")
            .files_changed,
        vec!["src/lib.rs"],
    );
}

#[test]
fn failed_write_releases_intent_and_fence() {
    let store = Store::open_in_memory_with_clock(FixedClock::new(NOW)).expect("store opens");
    let intent = store
        .start_write_intent(&request(
            "agent-1",
            WriteIntentStart {
                operation_id: "write-1".into(),
                action: "write_file".into(),
                targets: vec![WriteTarget { path: "src/lib.rs".into(), before: fingerprint(b"before") }],
            },
        ))
        .expect("intent starts")
        .response;
    store
        .complete_write_intent(&request(
            "agent-1",
            WriteIntentCompletion::failed(intent.intent_id.clone(), "tool_error"),
        ))
        .expect("write failure journals");

    assert!(store
        .active_write_intent("workspace-1", "src/lib.rs")
        .expect("intent reads")
        .is_none());
    assert!(store
        .active_write_fence("workspace-1", "src/lib.rs")
        .expect("fence reads")
        .is_none());
}

#[test]
fn write_intent_and_fence_mutations_require_the_initiating_actor_lineage() {
    let store = Store::open_in_memory_with_clock(FixedClock::new(NOW)).expect("store opens");
    let owner = request_as(
        "agent-1",
        "actor-owner",
        WriteIntentStart {
            operation_id: "write-1".into(),
            action: "write_file".into(),
            targets: vec![WriteTarget { path: "src/lib.rs".into(), before: fingerprint(b"before") }],
        },
    );
    let intent = store
        .start_write_intent(&owner)
        .expect("owner starts intent")
        .response;
    let mut owner_lineage_mismatch = request_as(
        "agent-1",
        "actor-owner",
        WriteIntentCompletion::committed(
            intent.intent_id.clone(),
            vec![("src/lib.rs".into(), fingerprint(b"after"))],
        ),
    );
    owner_lineage_mismatch.agent.owner_id = Some("different-owner".into());
    let lineage_completion = store
        .complete_write_intent(&owner_lineage_mismatch)
        .expect_err("different owner lineage cannot complete the intent");
    assert!(matches!(lineage_completion, stateful_store::StoreError::WriteIntentOwnerMismatch));
    let attacker = "actor-attacker";
    let completion = store
        .complete_write_intent(&request_as(
            "agent-1",
            attacker,
            WriteIntentCompletion::committed(
                intent.intent_id.clone(),
                vec![("src/lib.rs".into(), fingerprint(b"after"))],
            ),
        ))
        .expect_err("different actor cannot complete the intent");
    assert!(matches!(completion, stateful_store::StoreError::WriteIntentOwnerMismatch));
    let recovery = store
        .recover_write_intent(&request_as(
            "agent-1",
            attacker,
            (
                intent.intent_id.clone(),
                vec![("src/lib.rs".into(), fingerprint(b"after"))],
            ),
        ))
        .expect_err("different actor cannot recover the intent");
    assert!(matches!(recovery, stateful_store::StoreError::WriteIntentOwnerMismatch));

    store
        .recover_write_intent(&request_as(
            "agent-1",
            "actor-owner",
            (
                intent.intent_id.clone(),
                vec![("src/lib.rs".into(), fingerprint(b"after"))],
            ),
        ))
        .expect("owner records an unknown changed outcome");
    let reconciliation = store
        .reconcile_write_intent(&request_as("agent-1", attacker, intent.intent_id.clone()))
        .expect_err("different actor cannot reconcile the intent");
    assert!(matches!(reconciliation, stateful_store::StoreError::WriteIntentOwnerMismatch));
    let release = store
        .release_write_fences(&request_as(
            "agent-1",
            attacker,
            WriteFenceRelease {
                fence_ids: intent.fence_ids.clone(),
            },
        ))
        .expect_err("different actor cannot release the intent fence");
    assert!(matches!(release, stateful_store::StoreError::ClaimOwnerMismatch));
    assert!(store
        .active_write_intent("workspace-1", "src/lib.rs")
        .expect("intent reads")
        .is_some());
    assert!(store
        .active_write_fence("workspace-1", "src/lib.rs")
        .expect("fence reads")
        .is_some());
}

#[test]
fn warned_authorization_audits_its_reason_before_intent_and_fence_events() {
    let store = Store::open_in_memory_with_clock(FixedClock::new(NOW)).expect("store opens");
    let authorization = request("agent-1", ());
    let version = store.workspace_version("workspace-1").expect("workspace version loads");

    store
        .start_write_intent_authorized(
            &authorization,
            WriteIntentStart {
                operation_id: "write-1".into(),
                action: "write_file".into(),
                targets: vec![WriteTarget { path: "src/lib.rs".into(), before: fingerprint(b"before") }],
            },
            Decision {
                decision: DecisionKind::Warn,
                reason_code: "missing_read_provenance".into(),
                message: "A new exact read is required.".into(),
                required_next_action: Some("Read the target exactly before writing.".into()),
            },
            version,
        )
        .expect("warned authorization starts an awareness intent");

    assert_eq!(
        store
            .journal_event_types_for_request(authorization.request_id)
            .expect("event types load"),
        vec![
            "authorization.warned",
            "write_intent.started",
            "presence.resources_updated",
            "write_fence.acquired",
        ],
    );
    let warned = store
        .recent_workspace_events("workspace-1", 4)
        .expect("recent events load")
        .into_iter()
        .find(|event| event.event_type == "authorization.warned")
        .expect("warning event exists");
    assert_eq!(
        warned.payload["event"]["data"]["data"]["decision"]["reason_code"],
        "missing_read_provenance",
    );
    assert_eq!(warned.payload["event"]["data"]["data"]["action"], "write_file");
}

#[test]
fn stale_authorization_snapshot_from_a_second_connection_creates_no_intent_or_fence() {
    let temporary = tempfile::tempdir().expect("temporary database directory creates");
    let database = temporary.path().join("write-intent.sqlite");
    let first = Store::open_with_clock(&database, FixedClock::new(NOW)).expect("first store opens");
    let mut second = Store::open_with_clock(&database, FixedClock::new(NOW)).expect("second store opens");
    let authorization = request("agent-1", ());
    let version = first.workspace_version("workspace-1").expect("workspace version loads");
    second
        .start_activity(&request(
            "agent-2",
            ActivityStart {
                phase: PresencePhase::Editing,
            },
        ))
        .expect("second connection changes the workspace");
    let before = first.journal_event_count().expect("journal count loads");

    let error = first
        .start_write_intent_authorized(
            &authorization,
            WriteIntentStart {
                operation_id: "write-1".into(),
                action: "write_file".into(),
                targets: vec![WriteTarget { path: "src/lib.rs".into(), before: fingerprint(b"before") }],
            },
            Decision::allow("authorized", "Action is authorized."),
            version,
        )
        .expect_err("stale authorization must not start an intent");
    assert!(matches!(error, stateful_store::StoreError::StaleAuthorization));
    assert_eq!(first.journal_event_count().expect("journal count loads"), before);
    assert!(first
        .active_write_intent("workspace-1", "src/lib.rs")
        .expect("intent reads")
        .is_none());
    assert!(first
        .active_write_fence("workspace-1", "src/lib.rs")
        .expect("fence reads")
        .is_none());
}

#[test]
fn write_intent_start_rejects_an_incomplete_present_fingerprint() {
    let store = Store::open_in_memory_with_clock(FixedClock::new(NOW)).expect("store opens");
    let incomplete = ContentFingerprint {
        exists: true,
        byte_len: 4,
        sha256: Some("not-a-sha256".into()),
    };

    assert!(store
        .start_write_intent(&request(
            "agent-1",
            WriteIntentStart {
                operation_id: "write-1".into(),
                action: "write_file".into(),
                targets: vec![WriteTarget { path: "src/lib.rs".into(), before: incomplete }],
            },
        ))
        .is_err());
}

#[test]
fn write_intent_completion_rejects_an_incomplete_present_fingerprint() {
    let store = Store::open_in_memory_with_clock(FixedClock::new(NOW)).expect("store opens");
    let intent = store
        .start_write_intent(&request(
            "agent-1",
            WriteIntentStart {
                operation_id: "write-1".into(),
                action: "write_file".into(),
                targets: vec![WriteTarget { path: "src/lib.rs".into(), before: fingerprint(b"before") }],
            },
        ))
        .expect("intent starts")
        .response;
    let incomplete = ContentFingerprint {
        exists: true,
        byte_len: 4,
        sha256: Some("not-a-sha256".into()),
    };

    assert!(store
        .complete_write_intent(&request(
            "agent-1",
            WriteIntentCompletion::committed(
                intent.intent_id,
                vec![("src/lib.rs".into(), incomplete)],
            ),
        ))
        .is_err());
}

#[test]
fn write_intent_recovery_rejects_an_incomplete_present_fingerprint() {
    let store = Store::open_in_memory_with_clock(FixedClock::new(NOW)).expect("store opens");
    let intent = store
        .start_write_intent(&request(
            "agent-1",
            WriteIntentStart {
                operation_id: "write-1".into(),
                action: "write_file".into(),
                targets: vec![WriteTarget { path: "src/lib.rs".into(), before: fingerprint(b"before") }],
            },
        ))
        .expect("intent starts")
        .response;
    let incomplete = ContentFingerprint {
        exists: true,
        byte_len: 4,
        sha256: Some("not-a-sha256".into()),
    };

    assert!(store
        .recover_write_intent(&request(
            "agent-1",
            (intent.intent_id, vec![("src/lib.rs".into(), incomplete)]),
        ))
        .is_err());
}

#[test]
fn missing_post_hook_with_unchanged_file_resolves_unknown_no_change() {
    let store = Store::open_in_memory_with_clock(FixedClock::new(NOW)).expect("store opens");
    let before = fingerprint(b"before");
    let intent = store
        .start_write_intent(&request(
            "agent-1",
            WriteIntentStart {
                operation_id: "write-1".into(),
                action: "write_file".into(),
                targets: vec![WriteTarget { path: "src/lib.rs".into(), before: before.clone() }],
            },
        ))
        .expect("intent starts")
        .response;

    store
        .recover_write_intent(&request("agent-1", (intent.intent_id.clone(), vec![("src/lib.rs".into(), before)])))
        .expect("unchanged intent recovers");
    assert!(store
        .active_write_intent("workspace-1", "src/lib.rs")
        .expect("intent reads")
        .is_none());
}

#[test]
fn changed_unknown_requires_an_exact_reread_after_the_unknown_event() {
    let store = Store::open_in_memory_with_clock(FixedClock::new(NOW)).expect("store opens");
    let before = fingerprint(b"before");
    let intent = store
        .start_write_intent(&request(
            "agent-1",
            WriteIntentStart {
                operation_id: "write-1".into(),
                action: "write_file".into(),
                targets: vec![WriteTarget { path: "src/lib.rs".into(), before: before.clone() }],
            },
        ))
        .expect("intent starts")
        .response;
    start_read(&store, "agent-1", "before-unknown", "src/lib.rs", before.clone());
    complete_read(
        &store,
        "agent-1",
        "before-unknown",
        "src/lib.rs",
        ReadClassification::Exact,
        Some(before),
        None,
    );
    let changed = fingerprint(b"changed");
    store
        .recover_write_intent(&request(
            "agent-1",
            (intent.intent_id.clone(), vec![("src/lib.rs".into(), changed.clone())]),
        ))
        .expect("changed recovery journals unknown outcome");

    assert!(store
        .reconcile_write_intent(&request("agent-1", intent.intent_id.clone()))
        .is_err());

    start_read(&store, "agent-1", "after-unknown", "src/lib.rs", changed.clone());
    complete_read(
        &store,
        "agent-1",
        "after-unknown",
        "src/lib.rs",
        ReadClassification::Exact,
        Some(changed),
        None,
    );
    store
        .reconcile_write_intent(&request("agent-1", intent.intent_id))
        .expect("reread after the unknown outcome reconciles");
}

#[test]
fn changed_unknown_reconciliation_versions_rereads_and_invalidates_peers() {
    let mut store = Store::open_in_memory_with_clock(FixedClock::new(NOW)).expect("store opens");
    let before = fingerprint(b"before");
    start_read(&store, "agent-2", "peer-read", "src/lib.rs", before.clone());
    complete_read(
        &store,
        "agent-2",
        "peer-read",
        "src/lib.rs",
        ReadClassification::Exact,
        Some(before.clone()),
        None,
    );
    let intent = store
        .start_write_intent(&request(
            "agent-1",
            WriteIntentStart {
                operation_id: "write-1".into(),
                action: "write_file".into(),
                targets: vec![WriteTarget { path: "src/lib.rs".into(), before }],
            },
        ))
        .expect("intent starts")
        .response;
    let changed = fingerprint(b"changed");
    store
        .recover_write_intent(&request(
            "agent-1",
            (intent.intent_id.clone(), vec![("src/lib.rs".into(), changed.clone())]),
        ))
        .expect("changed recovery journals unknown outcome");
    start_read(&store, "agent-1", "reconcile-read", "src/lib.rs", changed.clone());
    complete_read(
        &store,
        "agent-1",
        "reconcile-read",
        "src/lib.rs",
        ReadClassification::Exact,
        Some(changed.clone()),
        None,
    );

    store
        .reconcile_write_intent(&request("agent-1", intent.intent_id))
        .expect("exact reread reconciles matching intent");

    let version = store
        .resource_version("workspace-1", "src/lib.rs")
        .expect("resource version reads")
        .expect("reconciliation versions the changed resource");
    assert_eq!(version.version, 1);
    assert_eq!(version.fingerprint, changed);
    assert!(!store
        .read_observation("workspace-1", "agent-2", "src/lib.rs")
        .expect("peer observation reads")
        .expect("peer observation remains auditable")
        .is_stable());
    assert_eq!(
        store
            .read_observation("workspace-1", "agent-1", "src/lib.rs")
            .expect("writer observation reads")
            .expect("writer observation exists")
            .resource_version,
        1
    );
    assert!(store
        .active_write_intent("workspace-1", "src/lib.rs")
        .expect("intent reads")
        .is_none());
    assert!(store
        .active_write_fence("workspace-1", "src/lib.rs")
        .expect("fence reads")
        .is_none());
    store.rebuild_projections().expect("reconciliation must replay");
}

#[test]
fn unchanged_unknown_reconciliation_releases_fences_without_versions_or_peer_invalidations() {
    let store = Store::open_in_memory_with_clock(FixedClock::new(NOW)).expect("store opens");
    let before = fingerprint(b"before");
    start_read(&store, "agent-2", "peer-read", "src/lib.rs", before.clone());
    complete_read(
        &store,
        "agent-2",
        "peer-read",
        "src/lib.rs",
        ReadClassification::Exact,
        Some(before.clone()),
        None,
    );
    let intent = store
        .start_write_intent(&request(
            "agent-1",
            WriteIntentStart {
                operation_id: "write-1".into(),
                action: "write_file".into(),
                targets: vec![WriteTarget { path: "src/lib.rs".into(), before: before.clone() }],
            },
        ))
        .expect("intent starts")
        .response;
    store
        .recover_write_intent(&request(
            "agent-1",
            (intent.intent_id.clone(), vec![("src/lib.rs".into(), fingerprint(b"changed"))]),
        ))
        .expect("changed recovery journals unknown outcome");
    start_read(&store, "agent-1", "reread", "src/lib.rs", before.clone());
    complete_read(
        &store,
        "agent-1",
        "reread",
        "src/lib.rs",
        ReadClassification::Exact,
        Some(before),
        None,
    );

    let reconciled = store
        .reconcile_write_intent(&request("agent-1", intent.intent_id))
        .expect("unchanged exact reread reconciles")
        .response;

    assert_eq!(reconciled.status, WriteIntentStatus::Reconciled);
    assert!(store
        .resource_version("workspace-1", "src/lib.rs")
        .expect("resource version reads")
        .is_none());
    assert!(store
        .read_observation("workspace-1", "agent-2", "src/lib.rs")
        .expect("peer observation reads")
        .expect("peer observation remains auditable")
        .is_stable());
    assert!(store
        .active_write_intent("workspace-1", "src/lib.rs")
        .expect("intent reads")
        .is_none());
    assert!(store
        .active_write_fence("workspace-1", "src/lib.rs")
        .expect("fence reads")
        .is_none());
}

#[test]
fn session_finalization_invalidates_its_stable_observations() {
    let mut store = Store::open_in_memory_with_clock(FixedClock::new(NOW)).expect("store opens");
    store
        .start_activity(&request(
            "agent-1",
            ActivityStart {
                phase: PresencePhase::Editing,
            },
        ))
        .expect("activity starts");
    let content = fingerprint(b"same bytes");
    start_read(&store, "agent-1", "read-1", "src/lib.rs", content.clone());
    complete_read(
        &store,
        "agent-1",
        "read-1",
        "src/lib.rs",
        ReadClassification::Exact,
        Some(content),
        None,
    );

    store
        .finalize_activity(&request("agent-1", ActivityFinalization {}))
        .expect("activity finalizes");
    assert!(store
        .read_observation("workspace-1", "agent-1", "src/lib.rs")
        .expect("observation reads")
        .is_none());
}
