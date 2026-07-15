use stateful_core::{
    ActorType, AgentIdentity, ContentFingerprint, PresencePhase, ReadClassification, ReadCompletion,
    ReadObservationStart, RequestEnvelope, SourceKind, SourceRef, WorkspaceIdentity,
    WriteIntentCompletion, WriteIntentStart, WriteTarget,
};
use stateful_store::{ActivityFinalization, ActivityStart, FixedClock, Store};
use time::{Duration, macros::datetime, OffsetDateTime};
use uuid::Uuid;

const NOW: OffsetDateTime = datetime!(2026-07-15 12:00 UTC);

fn fingerprint(bytes: &[u8]) -> ContentFingerprint {
    stateful_core::fingerprint_reader(std::io::Cursor::new(bytes))
        .expect("test content fingerprints")
}

fn request<T: serde::Serialize>(agent_id: &str, payload: T) -> RequestEnvelope<T> {
    RequestEnvelope::new(
        Uuid::new_v4(),
        NOW,
        AgentIdentity {
            agent_id: agent_id.into(),
            turn_id: Some("turn-1".into()),
            actor_id: format!("actor-{agent_id}"),
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
fn exact_successful_unchanged_read_stabilizes_observation() {
    let mut store = Store::open_in_memory_with_clock(FixedClock::new(NOW)).expect("store opens");
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

    let observation = store
        .read_observation("workspace-1", "agent-1", "src/lib.rs")
        .expect("observation reads")
        .expect("observation exists");
    assert!(observation.is_stable());
    assert_eq!(observation.operation_id, "read-1");
    assert_eq!(observation.resource_version, 0);
    assert_eq!(observation.expires_at, Some(NOW + Duration::minutes(30)));
    store.rebuild_projections().expect("read observation must replay");
}

#[test]
fn partial_truncated_structural_failed_and_ambiguous_reads_never_stabilize() {
    let store = Store::open_in_memory_with_clock(FixedClock::new(NOW)).expect("store opens");
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
    }
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
fn complete_structural_summary_requires_a_semantic_marker() {
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

    assert!(store
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
fn committed_write_updates_resource_version_and_invalidates_other_observations() {
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
    store
        .complete_write_intent(&request(
            "agent-1",
            WriteIntentCompletion::committed(
                intent.intent_id.clone(),
                vec![("src/lib.rs".into(), fingerprint(b"after"))],
            ),
        ))
        .expect("write commits");

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
    assert!(store
        .read_observation("workspace-1", "agent-1", "src/lib.rs")
        .expect("writer observation reads")
        .expect("writer becomes stable")
        .is_stable());
    store.rebuild_projections().expect("write version must replay");
    assert_eq!(
        store
            .resource_version("workspace-1", "src/lib.rs")
            .expect("resource version reads")
            .expect("resource version survives replay")
            .version,
        1
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
fn missing_post_hook_with_changed_file_blocks_until_matching_reconcile_ack() {
    let store = Store::open_in_memory_with_clock(FixedClock::new(NOW)).expect("store opens");
    let before = fingerprint(b"before");
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
    store
        .recover_write_intent(&request(
            "agent-1",
            (intent.intent_id.clone(), vec![("src/lib.rs".into(), fingerprint(b"changed"))]),
        ))
        .expect("changed recovery journals unknown outcome");
    assert!(store
        .active_write_intent("workspace-1", "src/lib.rs")
        .expect("intent reads")
        .is_some());
    start_read(&store, "agent-1", "reconcile-read", "src/lib.rs", fingerprint(b"changed"));
    complete_read(
        &store,
        "agent-1",
        "reconcile-read",
        "src/lib.rs",
        ReadClassification::Exact,
        Some(fingerprint(b"changed")),
        None,
    );
    store
        .reconcile_write_intent(&request("agent-1", intent.intent_id))
        .expect("exact reread reconciles matching intent");
    assert!(store
        .active_write_intent("workspace-1", "src/lib.rs")
        .expect("intent reads")
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
