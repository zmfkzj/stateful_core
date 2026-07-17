use serde::Serialize;
use stateful_core::{
    ActorType, AgentIdentity, ContentFingerprint, ReadClassification, ReadCompletion,
    ReadObservationStart, ReconciliationDecision, RequestEnvelope, ReservationScope, SourceKind,
    SourceRef, WorkspaceIdentity,
};
use stateful_store::{
    ClaimAcquire, ClaimPath, Clock, FixedClock, HumanObservationConfidence, HumanObservationInput,
    HumanObservationKind, ReconciliationAckInput, ReservationDeclaration, ReservationRelease,
    Store, WaitRequest,
};
use std::sync::{Arc, Mutex};
use time::{Duration, OffsetDateTime, macros::datetime};
use uuid::Uuid;

const NOW: OffsetDateTime = datetime!(2026-07-15 12:00 UTC);

#[derive(Clone)]
struct TestClock(Arc<Mutex<OffsetDateTime>>);

impl TestClock {
    fn new(now: OffsetDateTime) -> Self {
        Self(Arc::new(Mutex::new(now)))
    }

    fn advance(&self, duration: Duration) {
        *self.0.lock().expect("clock lock") += duration;
    }
}

impl Clock for TestClock {
    fn now(&self) -> OffsetDateTime {
        *self.0.lock().expect("clock lock")
    }
}

fn request<T: Serialize>(agent_id: &str, payload: T) -> RequestEnvelope<T> {
    RequestEnvelope::new(
        Uuid::new_v4(),
        NOW,
        AgentIdentity {
            agent_id: agent_id.into(),
            turn_id: Some("task9".into()),
            actor_id: format!("{agent_id}-actor"),
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
            kind: SourceKind::Cli,
            event: "test".into(),
            tool_name: None,
            source_ref: "task9-authority".into(),
        },
        payload,
    )
    .expect("request should be valid")
}

fn declaration(scopes: Vec<ReservationScope>) -> ReservationDeclaration {
    ReservationDeclaration {
        scopes,
        action: "write_file".into(),
        purpose: "Preserve task authority.".into(),
    }
}

fn fingerprint(bytes: &[u8]) -> ContentFingerprint {
    stateful_core::fingerprint_reader(std::io::Cursor::new(bytes)).expect("fingerprint")
}

fn reconcile(
    reservation_id: Option<String>,
    decision: ReconciliationDecision,
    files_reread: Vec<&str>,
) -> ReconciliationAckInput {
    ReconciliationAckInput {
        reservation_id,
        decision,
        files_reread: files_reread.into_iter().map(str::to_owned).collect(),
        human_change_summary: "Reviewed the human change.".into(),
    }
}

fn record_human_write(store: &Store) {
    store
        .record_human_observation(&request(
            "human",
            HumanObservationInput {
                relative_path: "src/a.rs".into(),
                kind: HumanObservationKind::Change,
                confidence: HumanObservationConfidence::High,
                source: "watcher".into(),
                summary: "human edit".into(),
                observed_at: None,
            },
        ))
        .expect("human write records");
}

fn exact_reread(store: &Store, agent_id: &str) {
    let content = fingerprint(b"current file");
    store
        .start_read_observation(&request(
            agent_id,
            ReadObservationStart {
                operation_id: "reread-a".into(),
                path: "src/a.rs".into(),
                before: content.clone(),
            },
        ))
        .expect("reread starts");
    store
        .complete_read_observation(&request(
            agent_id,
            ReadCompletion {
                operation_id: "reread-a".into(),
                path: "src/a.rs".into(),
                classification: ReadClassification::Exact,
                after: Some(content),
                semantic_marker: None,
            },
        ))
        .expect("reread completes");
}

#[test]
fn two_scope_reservation_replays_and_authorizes_each_declared_file() {
    let mut store = Store::open_in_memory_with_clock(FixedClock::new(NOW)).expect("store opens");
    let reservation = store
        .declare_reservation(&request(
            "agent-1",
            declaration(vec![
                ReservationScope::file("src/a.rs"),
                ReservationScope::file("src/b.rs"),
            ]),
        ))
        .expect("reservation declares")
        .response;

    assert_eq!(reservation.scopes.len(), 2);
    let claims = store
        .acquire_claim(&request(
            "agent-1",
            ClaimAcquire {
                reservation_id: reservation.reservation_id.clone(),
                paths: vec![
                    ClaimPath {
                        relative_path: "src/a.rs".into(),
                        observation: None,
                    },
                    ClaimPath {
                        relative_path: "src/b.rs".into(),
                        observation: None,
                    },
                ],
            },
        ))
        .expect("both declared files authorize claims");
    assert_eq!(claims.response.acquired, 2);

    store
        .rebuild_projections()
        .expect("reservation replay succeeds");
    assert_eq!(
        store
            .reservation("workspace-1", &reservation.reservation_id)
            .expect("reservation reads")
            .expect("reservation exists")
            .scopes,
        vec![
            ReservationScope::file("src/a.rs"),
            ReservationScope::file("src/b.rs")
        ],
    );
}

#[test]
fn promoted_wait_uses_its_own_bound_reservation_id_after_another_declaration() {
    let store = Store::open_in_memory_with_clock(FixedClock::new(NOW)).expect("store opens");
    let blocker = store
        .declare_reservation(&request(
            "agent-1",
            declaration(vec![ReservationScope::file("src/a.rs")]),
        ))
        .expect("blocker declares")
        .response;
    let wait = store
        .request_wait(&request(
            "agent-2",
            WaitRequest {
                relative_path: "src/a.rs".into(),
                action: "write_file".into(),
                purpose: "Need a.".into(),
                blocking_agent_id: None,
            },
        ))
        .expect("wait queues")
        .response;
    let unrelated = store
        .declare_reservation(&request(
            "agent-3",
            declaration(vec![ReservationScope::file("src/b.rs")]),
        ))
        .expect("unrelated declaration succeeds")
        .response;

    store
        .release_reservation(&request(
            "agent-1",
            ReservationRelease {
                reservation_id: blocker.reservation_id,
            },
        ))
        .expect("release promotes wait");

    let promoted = store
        .wait("workspace-1", &wait.wait_id)
        .expect("wait reads")
        .expect("wait exists");
    let bound_id = promoted
        .reservation_id
        .expect("wait has a bound reservation");
    assert_ne!(bound_id, unrelated.reservation_id);
    assert_eq!(
        store
            .reservation("workspace-1", &bound_id)
            .expect("reservation reads")
            .expect("bound reservation exists")
            .agent_id,
        "agent-2",
    );
}

#[test]
fn promoted_directory_wait_retains_its_directory_scope() {
    let store = Store::open_in_memory_with_clock(FixedClock::new(NOW)).expect("store opens");
    let blocker = store
        .declare_reservation(&request(
            "agent-1",
            declaration(vec![ReservationScope::directory("src")]),
        ))
        .expect("directory blocker declares")
        .response;
    let wait = store
        .request_wait(&request(
            "agent-2",
            WaitRequest {
                relative_path: "src".into(),
                action: "write_directory".into(),
                purpose: "Need the directory.".into(),
                blocking_agent_id: None,
            },
        ))
        .expect("directory wait queues")
        .response;
    store
        .release_reservation(&request(
            "agent-1",
            ReservationRelease {
                reservation_id: blocker.reservation_id,
            },
        ))
        .expect("release promotes directory wait");

    let promoted = store
        .wait("workspace-1", &wait.wait_id)
        .expect("wait reads")
        .expect("wait exists");
    assert_eq!(
        store
            .reservation(
                "workspace-1",
                promoted
                    .reservation_id
                    .as_deref()
                    .expect("bound reservation"),
            )
            .expect("reservation reads")
            .expect("reservation exists")
            .scopes,
        vec![ReservationScope::directory("src")],
    );
}

#[test]
fn multi_scope_release_does_not_promote_conflicting_waits_across_scopes() {
    let store = Store::open_in_memory_with_clock(FixedClock::new(NOW)).expect("store opens");
    let blocker = store
        .declare_reservation(&request(
            "agent-1",
            declaration(vec![
                ReservationScope::file("src/a.rs"),
                ReservationScope::file("src/b.rs"),
            ]),
        ))
        .expect("blocker declares")
        .response;
    let file_wait = store
        .request_wait(&request(
            "agent-2",
            WaitRequest {
                relative_path: "src/a.rs".into(),
                action: "write_file".into(),
                purpose: "Need a file.".into(),
                blocking_agent_id: None,
            },
        ))
        .expect("file wait queues")
        .response;
    let directory_wait = store
        .request_wait(&request(
            "agent-3",
            WaitRequest {
                relative_path: "src".into(),
                action: "write_directory".into(),
                purpose: "Need the directory.".into(),
                blocking_agent_id: None,
            },
        ))
        .expect("directory wait queues")
        .response;

    store
        .release_reservation(&request(
            "agent-1",
            ReservationRelease {
                reservation_id: blocker.reservation_id,
            },
        ))
        .expect("release succeeds");

    assert_eq!(
        store
            .wait("workspace-1", &file_wait.wait_id)
            .expect("file wait reads")
            .expect("file wait exists")
            .status,
        "claimable",
    );
    assert_eq!(
        store
            .wait("workspace-1", &directory_wait.wait_id)
            .expect("directory wait reads")
            .expect("directory wait exists")
            .status,
        "queued",
    );
}

#[test]
fn releasing_a_multi_scope_reservation_grants_an_overlapping_wait_once() {
    let store = Store::open_in_memory_with_clock(FixedClock::new(NOW)).expect("store opens");
    let reservation = store
        .declare_reservation(&request(
            "agent-1",
            declaration(vec![
                ReservationScope::file("src/a.rs"),
                ReservationScope::file("src/b.rs"),
            ]),
        ))
        .expect("two-scope reservation declares")
        .response;
    store
        .request_wait(&request(
            "agent-2",
            WaitRequest {
                relative_path: "src/".into(),
                action: "write_directory".into(),
                purpose: "Need source directory.".into(),
                blocking_agent_id: None,
            },
        ))
        .expect("overlapping wait queues");
    let release = request(
        "agent-1",
        ReservationRelease {
            reservation_id: reservation.reservation_id,
        },
    );
    store
        .release_reservation(&release)
        .expect("release succeeds");

    assert_eq!(
        store
            .journal_event_types_for_request(release.request_id)
            .expect("release events")
            .into_iter()
            .filter(|event_type| event_type == "wait.became_claimable")
            .count(),
        1,
    );
}

#[test]
fn reconciliation_clears_only_with_owner_exact_scope_and_fresh_exact_reread() {
    let mut store = Store::open_in_memory_with_clock(FixedClock::new(NOW)).expect("store opens");
    record_human_write(&store);

    let missing = store.acknowledge_human_reconciliation(&request(
        "agent-1",
        reconcile(None, ReconciliationDecision::Adopt, vec!["src/a.rs"]),
    ));
    assert_eq!(
        missing.expect_err("missing authority rejects").code(),
        "missing_reservation"
    );

    let wrong_owner = store
        .declare_reservation(&request(
            "agent-2",
            declaration(vec![ReservationScope::file("src/a.rs")]),
        ))
        .expect("other owner declares")
        .response;
    let wrong_owner_result = store.acknowledge_human_reconciliation(&request(
        "agent-1",
        reconcile(
            Some(wrong_owner.reservation_id.clone()),
            ReconciliationDecision::Adopt,
            vec!["src/a.rs"],
        ),
    ));
    assert_eq!(
        wrong_owner_result.expect_err("wrong owner rejects").code(),
        "reservation_owner_mismatch"
    );
    store
        .release_reservation(&request(
            "agent-2",
            ReservationRelease {
                reservation_id: wrong_owner.reservation_id,
            },
        ))
        .expect("other owner releases");

    let non_covering = store
        .declare_reservation(&request(
            "agent-1",
            declaration(vec![ReservationScope::file("src/b.rs")]),
        ))
        .expect("non-covering reservation declares")
        .response;
    let non_covering_result = store.acknowledge_human_reconciliation(&request(
        "agent-1",
        reconcile(
            Some(non_covering.reservation_id),
            ReconciliationDecision::Adopt,
            vec!["src/a.rs"],
        ),
    ));
    assert_eq!(
        non_covering_result
            .expect_err("non-covering scope rejects")
            .code(),
        "scope_mismatch"
    );

    let exact = store
        .declare_reservation(&request(
            "agent-1",
            declaration(vec![ReservationScope::file("src/a.rs")]),
        ))
        .expect("exact reservation declares")
        .response;
    let stale_result = store.acknowledge_human_reconciliation(&request(
        "agent-1",
        reconcile(
            Some(exact.reservation_id.clone()),
            ReconciliationDecision::Adopt,
            vec!["src/a.rs"],
        ),
    ));
    assert_eq!(
        stale_result.expect_err("missing reread rejects").code(),
        "missing_read_provenance"
    );

    let content = fingerprint(b"current file");
    store
        .start_read_observation(&request(
            "agent-1",
            ReadObservationStart {
                operation_id: "partial-reread".into(),
                path: "src/a.rs".into(),
                before: content.clone(),
            },
        ))
        .expect("partial read starts");
    store
        .complete_read_observation(&request(
            "agent-1",
            ReadCompletion {
                operation_id: "partial-reread".into(),
                path: "src/a.rs".into(),
                classification: ReadClassification::Partial,
                after: Some(content),
                semantic_marker: None,
            },
        ))
        .expect("partial read completes");
    let non_exact_result = store.acknowledge_human_reconciliation(&request(
        "agent-1",
        reconcile(
            Some(exact.reservation_id.clone()),
            ReconciliationDecision::Adopt,
            vec!["src/a.rs"],
        ),
    ));
    assert_eq!(
        non_exact_result
            .expect_err("non-exact reread rejects")
            .code(),
        "stale_observation"
    );

    exact_reread(&store, "agent-1");
    let acknowledged = store
        .acknowledge_human_reconciliation(&request(
            "agent-1",
            reconcile(
                Some(exact.reservation_id),
                ReconciliationDecision::Adopt,
                vec!["src/a.rs"],
            ),
        ))
        .expect("fresh exact authority clears");
    assert_eq!(acknowledged.response, 1);
    assert!(
        store
            .unreconciled_human_observations("workspace-1", &["src/a.rs".into()])
            .expect("observations read")
            .is_empty()
    );

    store
        .rebuild_projections()
        .expect("authority transitions replay");
    assert_eq!(
        NOW + Duration::minutes(60),
        store
            .read_observation("workspace-1", "agent-1", "src/a.rs")
            .expect("read observation")
            .expect("reread exists")
            .expires_at
            .expect("reread expires")
    );
}

#[test]
fn stale_exact_reread_cannot_clear_a_human_write_block() {
    let clock = TestClock::new(NOW);
    let store = Store::open_in_memory_with_clock(clock.clone()).expect("store opens");
    record_human_write(&store);
    store
        .declare_reservation(&request(
            "agent-1",
            declaration(vec![ReservationScope::file("src/a.rs")]),
        ))
        .expect("first reservation declares");
    exact_reread(&store, "agent-1");

    clock.advance(Duration::minutes(61));
    let renewed = store
        .declare_reservation(&request(
            "agent-1",
            declaration(vec![ReservationScope::file("src/a.rs")]),
        ))
        .expect("expired reservation no longer blocks renewal")
        .response;
    let stale = store.acknowledge_human_reconciliation(&request(
        "agent-1",
        reconcile(
            Some(renewed.reservation_id),
            ReconciliationDecision::Adopt,
            vec!["src/a.rs"],
        ),
    ));
    assert_eq!(
        stale.expect_err("stale reread rejects").code(),
        "stale_observation"
    );
    assert_eq!(
        store
            .unreconciled_human_observations("workspace-1", &["src/a.rs".into()])
            .expect("observations read")
            .len(),
        1,
    );
}

#[test]
fn reread_before_a_human_write_cannot_clear_the_later_block() {
    let clock = TestClock::new(NOW);
    let store = Store::open_in_memory_with_clock(clock.clone()).expect("store opens");
    let reservation = store
        .declare_reservation(&request(
            "agent-1",
            declaration(vec![ReservationScope::file("src/a.rs")]),
        ))
        .expect("reservation declares")
        .response;
    exact_reread(&store, "agent-1");
    clock.advance(Duration::minutes(1));
    record_human_write(&store);

    let acknowledgement = store.acknowledge_human_reconciliation(&request(
        "agent-1",
        reconcile(
            Some(reservation.reservation_id),
            ReconciliationDecision::Adopt,
            vec!["src/a.rs"],
        ),
    ));
    assert_eq!(
        acknowledgement
            .expect_err("pre-change reread rejects")
            .code(),
        "stale_observation",
    );
    assert_eq!(
        store
            .unreconciled_human_observations("workspace-1", &["src/a.rs".into()])
            .expect("observations read")
            .len(),
        1,
    );
}

#[test]
fn ask_user_and_abandon_remain_audit_only_without_reservation_authority() {
    let store = Store::open_in_memory_with_clock(FixedClock::new(NOW)).expect("store opens");
    record_human_write(&store);

    for decision in [
        ReconciliationDecision::AskUser,
        ReconciliationDecision::Abandon,
    ] {
        store
            .acknowledge_human_reconciliation(&request(
                "agent-1",
                reconcile(None, decision, vec!["src/a.rs"]),
            ))
            .expect("audit-only acknowledgement journals");
    }

    assert_eq!(
        store
            .unreconciled_human_observations("workspace-1", &["src/a.rs".into()])
            .expect("observations read")
            .len(),
        1,
    );
}
