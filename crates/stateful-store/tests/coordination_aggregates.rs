use serde::Serialize;
use serde_json::json;
use stateful_core::{
    ActorType, AgentIdentity, ReconciliationDecision, RequestEnvelope, ReservationScope, SourceKind,
    SourceRef, WorkspaceIdentity,
};
use stateful_store::{
    ActivityFinalization, ActivityStart, ClaimAcquire, ClaimPath, Clock, DeliveryAttempt, FixedClock,
    HumanObservationConfidence, HumanObservationInput, HumanObservationKind, NotificationAcknowledgement,
    NotificationCreate, NotificationDelivery, OutboxDelivery, OutboxEntry, ReconciliationAckInput,
    ReservationDeclaration, ReservationHeartbeat, ReservationRelease, Store, SyncStatus, WaitCancellation,
    WaitGrant, WaitRequest, WriteFenceAcquire, WriteFenceRelease,
};
use std::sync::{Arc, Mutex};
use time::{Duration, OffsetDateTime, macros::datetime};
use uuid::Uuid;

const NOW: OffsetDateTime = datetime!(2026-07-15 12:00 UTC);

#[derive(Clone)]
struct MutableClock(Arc<Mutex<OffsetDateTime>>);

impl MutableClock {
    fn new(now: OffsetDateTime) -> Self { Self(Arc::new(Mutex::new(now))) }
    fn advance(&self, duration: Duration) {
        *self.0.lock().expect("clock lock should not poison") += duration;
    }
}

impl Clock for MutableClock {
    fn now(&self) -> OffsetDateTime { *self.0.lock().expect("clock lock should not poison") }
}

fn request<T: Serialize>(agent_id: &str, request_id: Uuid, payload: T) -> RequestEnvelope<T> {
    RequestEnvelope::new(
        request_id,
        NOW,
        AgentIdentity {
            agent_id: agent_id.into(),
            turn_id: Some("turn-1".into()),
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
        SourceRef { kind: SourceKind::Cli, event: "test".into(), tool_name: None, source_ref: "aggregate-test".into() },
        payload,
    ).expect("test request is valid")
}

fn declaration(path: &str) -> ReservationDeclaration {
    ReservationDeclaration {
        scopes: vec![if path.ends_with('/') {
            ReservationScope::directory(path)
        } else {
            ReservationScope::file(path)
        }],
        action: "write_file".into(),
        purpose: "Refactor the projector.".into(),
    }
}

#[test]
fn claim_acquisition_refresh_and_release_are_atomic_journaled_transitions() {
    let mut store = Store::open_in_memory_with_clock(FixedClock::new(NOW)).expect("store opens");
    let reservation = store.declare_reservation(&request("agent-1", Uuid::new_v4(), declaration("src/lib.rs")))
        .expect("reservation declares").response;
    let claim = store.acquire_claim(&request("agent-1", Uuid::new_v4(), ClaimAcquire {
        reservation_id: reservation.reservation_id.clone(),
        paths: vec![ClaimPath { relative_path: "src/lib.rs".into(), observation: None }],
    })).expect("claim acquires").response.claims.remove(0);
    let refreshed = store.acquire_claim(&request("agent-1", Uuid::new_v4(), ClaimAcquire {
        reservation_id: reservation.reservation_id,
        paths: vec![ClaimPath { relative_path: "src/lib.rs".into(), observation: Some(stateful_store::ClaimObservation { exists: true, content_hash: Some("abc".into()) }) }],
    })).expect("claim refreshes");
    assert_eq!(refreshed.response.already_held, 1);
    let released = store.release_claim(&request("agent-1", Uuid::new_v4(), stateful_store::ClaimRelease { claim_id: claim.claim_id.clone() }))
        .expect("claim releases");
    assert_eq!(released.response.status, "released");
    let persisted = store.claim("workspace-1", &claim.claim_id).expect("claim reads").expect("claim exists");
    assert_eq!(persisted.status, "released");
    assert!(persisted.origin_event_seq > 0, "projected claim retains journal origin");
    store.rebuild_projections().expect("claims replay");
}

#[test]
fn wait_grant_uses_submission_order_before_random_wait_id_and_replays() {
    let mut store = Store::open_in_memory_with_clock(FixedClock::new(NOW)).expect("store opens");
    let (first, second) = loop {
        let first_id = store.request_wait(&request("agent-1", Uuid::new_v4(), WaitRequest {
            relative_path: "src/lib.rs".into(), action: "write_file".into(), purpose: "First.".into(), blocking_agent_id: None,
        })).expect("first wait queues").response.wait_id;
        let second_id = store.request_wait(&request("agent-2", Uuid::new_v4(), WaitRequest {
            relative_path: "src/lib.rs".into(), action: "write_file".into(), purpose: "Second.".into(), blocking_agent_id: None,
        })).expect("second wait queues").response.wait_id;
        let first = store.wait("workspace-1", &first_id).expect("first wait reads").expect("first wait exists");
        let second = store.wait("workspace-1", &second_id).expect("second wait reads").expect("second wait exists");
        if first.wait_id > second.wait_id {
            break (first, second);
        }
        store.cancel_wait(&request("agent-1", Uuid::new_v4(), WaitCancellation {
            wait_id: first.wait_id,
        })).expect("first retry wait cancels");
        store.cancel_wait(&request("agent-2", Uuid::new_v4(), WaitCancellation {
            wait_id: second.wait_id,
        })).expect("second retry wait cancels");
    };

    let granted = store.grant_next_wait(&request("agent-3", Uuid::new_v4(), WaitGrant {
        relative_path: "src/lib.rs".into(),
    })).expect("first submitted wait grants").response.expect("waiter grants");

    assert!(first.origin_event_seq < second.origin_event_seq);
    assert_eq!(granted.wait_id, first.wait_id, "submission sequence wins before random wait ID");
    assert_eq!(
        store.pending_notifications(&first.agent_id, "workspace-1").expect("notifications read")[0].notification_id,
        first.wait_id,
    );
    assert_eq!(store.wait("workspace-1", &second.wait_id).expect("wait reads").expect("other remains").status, "queued");
    store.rebuild_projections().expect("FIFO grant replays");
}

#[test]
fn releasing_a_reservation_promotes_the_next_waiter() {
    let store = Store::open_in_memory_with_clock(FixedClock::new(NOW)).expect("store opens");
    let reservation = store.declare_reservation(&request("agent-1", Uuid::new_v4(), declaration("src/lib.rs")))
        .expect("reservation declares").response;
    let wait = store.request_wait(&request("agent-2", Uuid::new_v4(), WaitRequest {
        relative_path: "src/lib.rs".into(), action: "write_file".into(), purpose: "Need the file.".into(), blocking_agent_id: None,
    })).expect("wait queues").response;

    store.release_reservation(&request("agent-1", Uuid::new_v4(), ReservationRelease {
        reservation_id: reservation.reservation_id,
    })).expect("release succeeds");

    let promoted = store.wait("workspace-1", &wait.wait_id).expect("wait reads").expect("wait exists");
    assert_eq!(promoted.status, "claimable");
    let granted = store.reservation("workspace-1", promoted.reservation_id.as_deref().expect("grant reservation"))
        .expect("reservation reads").expect("grant reservation exists");
    assert_eq!(granted.agent_id, "agent-2");
}


#[test]
fn expiring_a_reservation_promotes_the_next_waiter() {
    let clock = MutableClock::new(NOW);
    let store = Store::open_in_memory_with_clock(clock.clone()).expect("store opens");
    store.declare_reservation(&request("agent-1", Uuid::new_v4(), declaration("src/lib.rs")))
        .expect("reservation declares");
    let wait = store.request_wait(&request("agent-2", Uuid::new_v4(), WaitRequest {
        relative_path: "src/lib.rs".into(), action: "write_file".into(), purpose: "Need the file.".into(), blocking_agent_id: None,
    })).expect("wait queues").response;

    clock.advance(Duration::minutes(16));
    store.expire_reservations(&request("agent-1", Uuid::new_v4(), ())).expect("expiry succeeds");

    assert_eq!(
        store.wait("workspace-1", &wait.wait_id).expect("wait reads").expect("wait exists").status,
        "claimable",
    );
}

#[test]
fn maintenance_expires_reservations_in_workspaces_without_presence() {
    let clock = MutableClock::new(NOW);
    let mut store = Store::open_in_memory_with_clock(clock.clone()).expect("store opens");
    let mut request = request("agent-1", Uuid::new_v4(), declaration("src/lib.rs"));
    request.workspace.workspace_id = "workspace-2".into();
    let reservation = store
        .declare_reservation(&request)
        .expect("reservation declares")
        .response;

    clock.advance(Duration::minutes(16));
    store.run_maintenance().expect("maintenance succeeds");
    assert_eq!(
        store
            .reservation("workspace-2", &reservation.reservation_id)
            .expect("reservation reads")
            .expect("reservation exists")
            .status,
        "expired",
    );
    let events_after_expiry = store.journal_event_count().expect("journal count");
    store.run_maintenance().expect("repeated maintenance succeeds");
    assert_eq!(
        store.journal_event_count().expect("journal count"),
        events_after_expiry,
        "repeated maintenance is event-idempotent",
    );
    store.rebuild_projections().expect("maintenance replays");
}

#[test]
fn maintenance_expires_human_writes_so_they_no_longer_block() {
    let clock = MutableClock::new(NOW);
    let mut store = Store::open_in_memory_with_clock(clock.clone()).expect("store opens");
    store
        .record_human_observation(&request(
            "agent-2",
            Uuid::new_v4(),
            HumanObservationInput {
                relative_path: "src/lib.rs".into(),
                kind: HumanObservationKind::Save,
                confidence: HumanObservationConfidence::High,
                source: "watcher".into(),
                summary: "human save".into(),
                observed_at: None,
            },
        ))
        .expect("human write records");
    assert_eq!(
        store
            .unreconciled_human_observations("workspace-1", &["src/lib.rs".into()])
            .expect("human blocks load")
            .len(),
        1,
    );

    clock.advance(Duration::hours(24));
    store.run_maintenance().expect("maintenance succeeds");
    assert!(
        store
            .unreconciled_human_observations("workspace-1", &["src/lib.rs".into()])
            .expect("expired human writes no longer block")
            .is_empty(),
    );
}

#[test]
fn releasing_a_directory_reservation_promotes_nonconflicting_child_waiters() {
    let store = Store::open_in_memory_with_clock(FixedClock::new(NOW)).expect("store opens");
    let reservation = store.declare_reservation(&request("agent-1", Uuid::new_v4(), declaration("src/")))
        .expect("directory reservation declares").response;
    let first = store.request_wait(&request("agent-2", Uuid::new_v4(), WaitRequest {
        relative_path: "src/a.rs".into(), action: "write_file".into(), purpose: "Need a.".into(), blocking_agent_id: None,
    })).expect("first wait queues").response;
    let second = store.request_wait(&request("agent-3", Uuid::new_v4(), WaitRequest {
        relative_path: "src/b.rs".into(), action: "write_file".into(), purpose: "Need b.".into(), blocking_agent_id: None,
    })).expect("second wait queues").response;

    store.release_reservation(&request("agent-1", Uuid::new_v4(), ReservationRelease {
        reservation_id: reservation.reservation_id,
    })).expect("release succeeds");

    for wait in [first, second] {
        assert_eq!(
            store.wait("workspace-1", &wait.wait_id).expect("wait reads").expect("wait exists").status,
            "claimable",
        );
    }
}

#[test]
fn reservation_heartbeat_and_wait_cancellation_are_journaled_terminal_transitions() {
    let clock = MutableClock::new(NOW);
    let mut store = Store::open_in_memory_with_clock(clock.clone()).expect("store opens");
    let reservation = store.declare_reservation(&request("agent-1", Uuid::new_v4(), declaration("src/lib.rs")))
        .expect("reservation declares").response;
    clock.advance(Duration::minutes(10));
    let refreshed = store.heartbeat_reservation(&request("agent-1", Uuid::new_v4(), ReservationHeartbeat {
        reservation_id: reservation.reservation_id.clone(),
    })).expect("heartbeat refreshes").response;
    assert!(refreshed.expires_at > reservation.expires_at);
    let wait = store.request_wait(&request("agent-2", Uuid::new_v4(), WaitRequest {
        relative_path: "src/lib.rs".into(), action: "write_file".into(), purpose: "Need the file.".into(), blocking_agent_id: None,
    })).expect("wait queues").response;
    let canceled = store.cancel_wait(&request("agent-2", Uuid::new_v4(), WaitCancellation {
        wait_id: wait.wait_id.clone(),
    })).expect("wait cancels").response;
    assert_eq!(canceled.status, "canceled");
    store.release_reservation(&request("agent-1", Uuid::new_v4(), ReservationRelease {
        reservation_id: reservation.reservation_id,
    })).expect("release succeeds");
    assert_eq!(
        store.wait("workspace-1", &wait.wait_id).expect("wait reads").expect("wait exists").status,
        "canceled",
    );
    store.rebuild_projections().expect("reservation and wait replay");
}
#[test]
fn fences_human_reconciliation_and_expiry_do_not_regress_terminal_records() {
    let clock = MutableClock::new(NOW);
    let store = Store::open_in_memory_with_clock(clock.clone()).expect("store opens");
    let fence = store.acquire_write_fences(&request("agent-1", Uuid::new_v4(), WriteFenceAcquire { paths: vec!["src/lib.rs".into()], action: "write_file".into() }))
        .expect("fence acquires").response.fences.remove(0);
    let conflict = store.acquire_write_fences(&request("agent-2", Uuid::new_v4(), WriteFenceAcquire { paths: vec!["src/lib.rs".into()], action: "write_file".into() }))
        .expect("conflict is journal-safe response");
    assert_eq!(conflict.http_status, 409);
    assert_eq!(conflict.response.conflict.expect("conflict detail").owner_agent_id, "agent-1");
    let observation = store.record_human_observation(&request("agent-2", Uuid::new_v4(), HumanObservationInput {
        relative_path: "src/lib.rs".into(), kind: HumanObservationKind::Save, confidence: HumanObservationConfidence::High,
        source: "watcher".into(), summary: "human save".into(), observed_at: None,
    })).expect("human observation records").response;
    assert_eq!(observation.status, "reconciled", "active agent fence attributes the write");
    let released = store.release_write_fences(&request("agent-1", Uuid::new_v4(), WriteFenceRelease { fence_ids: vec![fence.fence_id.clone()] }))
        .expect("fence releases");
    assert_eq!(released.response[0].status, "released");
    clock.advance(Duration::minutes(10));
    store.expire_write_fences(&request("agent-1", Uuid::new_v4(), ())).expect("expiry succeeds");
    assert_eq!(store.write_fence("workspace-1", &fence.fence_id).expect("fence reads").expect("terminal fence exists").status, "released");
}

#[test]
fn delivery_callbacks_are_idempotent_and_terminal_activity_emits_no_notification() {
    let mut store = Store::open_in_memory_with_clock(FixedClock::new(NOW)).expect("store opens");
    let notification = store.create_notification(&request("agent-1", Uuid::new_v4(), NotificationCreate {
        target_agent_id: "agent-2".into(), kind: "context".into(), payload: json!({"v": 1}), coalesce_key: Some("context-agent-2".into()),
    })).expect("notification queues").response;
    let delivery = request("agent-2", Uuid::new_v4(), NotificationDelivery { notification_id: notification.notification_id.clone(), sequence: notification.sequence, outcome: DeliveryAttempt::Delivered, error: None, retry_at: None });
    store.record_notification_delivery(&delivery).expect("delivery records");
    let before = store.journal_event_count().expect("journal count");
    store.record_notification_delivery(&request("agent-2", Uuid::new_v4(), NotificationDelivery { notification_id: notification.notification_id.clone(), sequence: notification.sequence, outcome: DeliveryAttempt::Delivered, error: None, retry_at: None }))
        .expect("repeated callback is inert");
    assert_eq!(store.journal_event_count().expect("journal count"), before);
    store.start_activity(&request("agent-1", Uuid::new_v4(), ActivityStart { phase: stateful_core::PresencePhase::Editing })).expect("activity starts");
    let before_finalize = store.journal_event_count().expect("journal count");
    store.finalize_activity(&request("agent-1", Uuid::new_v4(), ActivityFinalization {})).expect("activity finalizes");
    assert_eq!(store.journal_event_count().expect("journal count"), before_finalize + 1);
    store.rebuild_projections().expect("all aggregates replay");
}

#[test]
fn notification_acknowledgements_are_bound_to_target_and_sequence() {
    let mut store = Store::open_in_memory_with_clock(FixedClock::new(NOW)).expect("store opens");
    let first = store
        .create_notification(&request(
            "agent-1",
            Uuid::new_v4(),
            NotificationCreate {
                target_agent_id: "agent-2".into(),
                kind: "reservation".into(),
                payload: json!({"wait_id": "wait-1"}),
                coalesce_key: None,
            },
        ))
        .expect("first notification queues")
        .response;
    let second = store
        .create_notification(&request(
            "agent-1",
            Uuid::new_v4(),
            NotificationCreate {
                target_agent_id: "agent-2".into(),
                kind: "reservation".into(),
                payload: json!({"wait_id": "wait-2"}),
                coalesce_key: None,
            },
        ))
        .expect("second notification queues")
        .response;

    assert!(
        store
            .acknowledge_notifications(&request(
                "agent-1",
                Uuid::new_v4(),
                NotificationAcknowledgement { sequence: first.sequence },
            ))
            .expect("foreign acknowledgement is safe")
            .response
            .is_empty(),
    );
    let acknowledged = store
        .acknowledge_notifications(&request(
            "agent-2",
            Uuid::new_v4(),
            NotificationAcknowledgement { sequence: first.sequence },
        ))
        .expect("target acknowledgement succeeds");
    assert_eq!(acknowledged.response, vec![first.notification_id]);
    assert_eq!(
        store
            .pending_notifications("agent-2", "workspace-1")
            .expect("pending notifications load")
            .into_iter()
            .map(|notification| notification.notification_id)
            .collect::<Vec<_>>(),
        vec![second.notification_id],
    );
    let before = store.journal_event_count().expect("journal count");
    store
        .acknowledge_notifications(&request(
            "agent-2",
            Uuid::new_v4(),
            NotificationAcknowledgement { sequence: first.sequence },
        ))
        .expect("repeated acknowledgement is inert");
    assert_eq!(store.journal_event_count().expect("journal count"), before);
    store.rebuild_projections().expect("acknowledgements replay");
}

#[test]
fn outbox_delivery_is_event_sourced_and_idempotent_by_outbox_identity() {
    let mut store = Store::open_in_memory_with_clock(FixedClock::new(NOW)).expect("store opens");
    let entry = OutboxEntry {
        outbox_id: "outbox-1".into(), sequence: 1, event_type: "heartbeat".into(), payload: json!({"n": 1}),
    };
    let first = store.enqueue_outbox(&request("agent-1", Uuid::new_v4(), entry.clone()))
        .expect("outbox queues");
    let before_duplicate = store.journal_event_count().expect("journal count");
    let duplicate = store.enqueue_outbox(&request("agent-1", Uuid::new_v4(), entry))
        .expect("outbox identity is idempotent");
    assert_eq!(first.response, duplicate.response);
    assert_eq!(store.journal_event_count().expect("journal count"), before_duplicate);

    let failed = request("agent-1", Uuid::new_v4(), OutboxDelivery {
        outbox_id: "outbox-1".into(), outcome: DeliveryAttempt::Failed, error: Some("offline".into()),
    });
    store.record_outbox_delivery(&failed).expect("failure records");
    let before_repeated_failure = store.journal_event_count().expect("journal count");
    store.record_outbox_delivery(&request("agent-1", Uuid::new_v4(), OutboxDelivery {
        outbox_id: "outbox-1".into(), outcome: DeliveryAttempt::Failed, error: Some("offline".into()),
    })).expect("repeated failure is inert");
    assert_eq!(store.journal_event_count().expect("journal count"), before_repeated_failure);

    let delivered = store.record_outbox_delivery(&request("agent-1", Uuid::new_v4(), OutboxDelivery {
        outbox_id: "outbox-1".into(), outcome: DeliveryAttempt::Delivered, error: None,
    })).expect("delivery records");
    assert_eq!(delivered.response.sync_status, SyncStatus::Synced);
    store.rebuild_projections().expect("outbox replays");
}

#[test]
fn notifications_coalesce_and_expire_through_journaled_commands() {
    let clock = MutableClock::new(NOW);
    let mut store = Store::open_in_memory_with_clock(clock.clone()).expect("store opens");
    let first = store.create_notification(&request("agent-1", Uuid::new_v4(), NotificationCreate {
        target_agent_id: "agent-2".into(), kind: "context".into(), payload: json!({"v": 1}),
        coalesce_key: Some("context-agent-2".into()),
    })).expect("notification queues").response;
    let second = store.create_notification(&request("agent-1", Uuid::new_v4(), NotificationCreate {
        target_agent_id: "agent-2".into(), kind: "context".into(), payload: json!({"v": 2}),
        coalesce_key: Some("context-agent-2".into()),
    })).expect("notification coalesces").response;
    assert_eq!(first.notification_id, second.notification_id);
    assert_eq!(second.payload, json!({"v": 2}));
    store.create_notification(&request("agent-1", Uuid::new_v4(), NotificationCreate {
        target_agent_id: "agent-2".into(), kind: "other".into(), payload: json!({"v": 3}),
        coalesce_key: None,
    })).expect("second notification queues");
    assert_eq!(
        store.pending_notifications("agent-2", "workspace-1").expect("pending notifications")
            .into_iter().map(|notification| notification.sequence).collect::<Vec<_>>(),
        vec![2, 3],
    );

    clock.advance(Duration::minutes(3));
    store.expire_notifications(&request("agent-1", Uuid::new_v4(), ())).expect("notification expires");
    assert!(store.pending_notifications("agent-2", "workspace-1").expect("pending notifications").is_empty());
    store.rebuild_projections().expect("notifications replay");
}

#[test]
fn promotion_checks_each_candidate_against_nonreleased_active_reservations() {
    let mut store = Store::open_in_memory_with_clock(FixedClock::new(NOW)).expect("store opens");
    let released = store.declare_reservation(&request("agent-1", Uuid::new_v4(), declaration("src/a.rs")))
        .expect("first reservation declares").response;
    store.declare_reservation(&request("agent-3", Uuid::new_v4(), declaration("src/b.rs")))
        .expect("sibling reservation declares");
    let directory_wait = store.request_wait(&request("agent-2", Uuid::new_v4(), WaitRequest {
        relative_path: "src/".into(), action: "write_directory".into(), purpose: "Need the directory.".into(),
        blocking_agent_id: Some("agent-1".into()),
    })).expect("directory wait queues").response;

    store.release_reservation(&request("agent-1", Uuid::new_v4(), ReservationRelease {
        reservation_id: released.reservation_id,
    })).expect("first reservation releases");

    assert_eq!(
        store.wait("workspace-1", &directory_wait.wait_id).expect("wait reads").expect("wait exists").status,
        "queued",
        "the unrelated active sibling reservation still blocks a directory candidate",
    );
    store.rebuild_projections().expect("candidate promotion replays");
}

#[test]
fn releasing_one_directory_claim_keeps_siblings_and_blocks_directory_waiter() {
    let mut store = Store::open_in_memory_with_clock(FixedClock::new(NOW)).expect("store opens");
    let reservation = store.declare_reservation(&request("agent-1", Uuid::new_v4(), declaration("src/")))
        .expect("directory reservation declares").response;
    let claims = store.acquire_claim(&request("agent-1", Uuid::new_v4(), ClaimAcquire {
        reservation_id: reservation.reservation_id.clone(),
        paths: vec![
            ClaimPath { relative_path: "src/a.rs".into(), observation: None },
            ClaimPath { relative_path: "src/b.rs".into(), observation: None },
        ],
    })).expect("sibling claims acquire").response.claims;
    let released_claim = claims.iter().find(|claim| claim.relative_path == "src/a.rs").expect("first claim").clone();
    let sibling_claim = claims.iter().find(|claim| claim.relative_path == "src/b.rs").expect("second claim").clone();
    let directory_wait = store.request_wait(&request("agent-2", Uuid::new_v4(), WaitRequest {
        relative_path: "src/".into(), action: "write_directory".into(), purpose: "Need the directory.".into(),
        blocking_agent_id: Some("agent-1".into()),
    })).expect("directory wait queues").response;

    store.release_claim(&request("agent-1", Uuid::new_v4(), stateful_store::ClaimRelease {
        claim_id: released_claim.claim_id,
    })).expect("one claim releases");

    assert_eq!(store.claim("workspace-1", &sibling_claim.claim_id).expect("sibling reads").expect("sibling exists").status, "active");
    assert_eq!(
        store.reservation("workspace-1", &reservation.reservation_id).expect("reservation reads").expect("reservation exists").status,
        "active",
    );
    assert_eq!(
        store.wait("workspace-1", &directory_wait.wait_id).expect("wait reads").expect("wait exists").status,
        "queued",
        "the still-held sibling file claim blocks the directory waiter",
    );
    store.rebuild_projections().expect("partial claim release replays");
}

#[test]
fn activity_finalization_promotes_after_all_owner_reservations_are_planned_for_release() {
    let mut store = Store::open_in_memory_with_clock(FixedClock::new(NOW)).expect("store opens");
    store.start_activity(&request("agent-1", Uuid::new_v4(), ActivityStart {
        phase: stateful_core::PresencePhase::Editing,
    })).expect("activity starts");
    store.declare_reservation(&request("agent-1", Uuid::new_v4(), declaration("src/a.rs")))
        .expect("first reservation declares");
    store.declare_reservation(&request("agent-1", Uuid::new_v4(), declaration("src/b.rs")))
        .expect("second reservation declares");
    let directory_wait = store.request_wait(&request("agent-2", Uuid::new_v4(), WaitRequest {
        relative_path: "src/".into(), action: "write_directory".into(), purpose: "Need the directory.".into(),
        blocking_agent_id: Some("agent-1".into()),
    })).expect("directory wait queues").response;

    store.finalize_activity(&request("agent-1", Uuid::new_v4(), ActivityFinalization {}))
        .expect("activity finalizes");

    assert_eq!(
        store.wait("workspace-1", &directory_wait.wait_id).expect("wait reads").expect("wait exists").status,
        "claimable",
        "promotion must see every owner reservation as part of the finalization release set",
    );
    store.rebuild_projections().expect("activity cleanup replays");
}

#[test]
fn activity_finalization_cancels_its_overlapping_wait_before_promoting_successor() {
    let mut store = Store::open_in_memory_with_clock(FixedClock::new(NOW)).expect("store opens");
    store.start_activity(&request("agent-1", Uuid::new_v4(), ActivityStart {
        phase: stateful_core::PresencePhase::Editing,
    })).expect("activity starts");
    store.declare_reservation(&request("agent-1", Uuid::new_v4(), declaration("src/a.rs")))
        .expect("owner reservation declares");
    let owner_wait = store.request_wait(&request("agent-1", Uuid::new_v4(), WaitRequest {
        relative_path: "src/".into(), action: "write_directory".into(), purpose: "Owner wait.".into(),
        blocking_agent_id: Some("agent-2".into()),
    })).expect("owner wait queues").response;
    let successor_wait = store.request_wait(&request("agent-2", Uuid::new_v4(), WaitRequest {
        relative_path: "src/".into(), action: "write_directory".into(), purpose: "Successor wait.".into(),
        blocking_agent_id: Some("agent-1".into()),
    })).expect("successor wait queues").response;

    store.finalize_activity(&request("agent-1", Uuid::new_v4(), ActivityFinalization {}))
        .expect("activity finalizes");

    assert_eq!(store.wait("workspace-1", &owner_wait.wait_id).expect("owner wait reads").expect("wait exists").status, "canceled");
    assert_eq!(store.wait("workspace-1", &successor_wait.wait_id).expect("successor wait reads").expect("wait exists").status, "claimable");
    store.rebuild_projections().expect("finalization cleanup replays");
}

#[test]
fn empty_adopt_and_reapply_acks_are_journaled_without_clearing_observations() {
    let mut store = Store::open_in_memory_with_clock(FixedClock::new(NOW)).expect("store opens");
    store.record_human_observation(&request("agent-2", Uuid::new_v4(), HumanObservationInput {
        relative_path: "src/a.rs".into(), kind: HumanObservationKind::Save,
        confidence: HumanObservationConfidence::High, source: "watcher".into(),
        summary: "human save".into(), observed_at: None,
    })).expect("observation records");
    let reservation = store
        .declare_reservation(&request("agent-1", Uuid::new_v4(), declaration("src/a.rs")))
        .expect("reservation declares")
        .response;
    let before = store.journal_event_count().expect("journal count");

    let acknowledgement = request("agent-1", Uuid::new_v4(), ReconciliationAckInput {
        reservation_id: Some(reservation.reservation_id.clone()),
        decision: ReconciliationDecision::Adopt,
        files_reread: Vec::new(),
        human_change_summary: "No files were reread.".into(),
    });
    let adopted = store.acknowledge_human_reconciliation(&acknowledgement)
        .expect("empty Adopt acknowledgement journals");
    let reapply_acknowledgement = request("agent-1", Uuid::new_v4(), ReconciliationAckInput {
        reservation_id: Some(reservation.reservation_id),
        decision: ReconciliationDecision::Reapply,
        files_reread: Vec::new(),
        human_change_summary: "Reapply has no reread files.".into(),
    });
    let reapplied = store.acknowledge_human_reconciliation(&reapply_acknowledgement)
        .expect("empty Reapply acknowledgement journals");

    assert_eq!(adopted.response, 0);
    assert_eq!(reapplied.response, 0);
    assert_eq!(store.journal_event_count().expect("journal count"), before + 2);
    for acknowledgement_request in [&acknowledgement, &reapply_acknowledgement] {
        assert_eq!(
            store.journal_event_types_for_request(acknowledgement_request.request_id)
                .expect("ack event types"),
            vec!["human_acknowledgement.recorded"],
        );
    }
    assert_eq!(
        store.unreconciled_human_observations("workspace-1", &["src/a.rs".into()])
            .expect("pending observations").len(),
        1,
        "an empty clearing acknowledgement cannot clear every pending observation",
    );
    let acknowledgements = store.human_reconciliation_acknowledgements("workspace-1")
        .expect("acknowledgements read");
    assert_eq!(acknowledgements.len(), 2);
    for decision in [ReconciliationDecision::Adopt, ReconciliationDecision::Reapply] {
        let record = acknowledgements.iter()
            .find(|record| record.decision == decision)
            .expect("clearing acknowledgement persists");
        assert!(record.files_reread.is_empty());
    }
    store.rebuild_projections().expect("empty acknowledgements replay");
}

#[test]
fn nonclearing_and_unmatched_human_acks_are_persistent_and_replayable() {
    let mut store = Store::open_in_memory_with_clock(FixedClock::new(NOW)).expect("store opens");
    store.record_human_observation(&request("agent-2", Uuid::new_v4(), HumanObservationInput {
        relative_path: "src/a.rs".into(), kind: HumanObservationKind::Save,
        confidence: HumanObservationConfidence::High, source: "watcher".into(),
        summary: "human save".into(), observed_at: None,
    })).expect("observation records");
    let reapply_reservation = store
        .declare_reservation(&request("agent-1", Uuid::new_v4(), declaration("src/missing.rs")))
        .expect("reapply reservation declares")
        .response;
    let ask_user = store.acknowledge_human_reconciliation(&request("agent-1", Uuid::new_v4(), ReconciliationAckInput { reservation_id: None, decision: ReconciliationDecision::AskUser, files_reread: vec!["src/a.rs".into()], human_change_summary: "Need confirmation.".into() })).expect("nonclearing acknowledgement journals");
    let abandon = store.acknowledge_human_reconciliation(&request("agent-1", Uuid::new_v4(), ReconciliationAckInput { reservation_id: None, decision: ReconciliationDecision::Abandon, files_reread: vec!["src/a.rs".into()], human_change_summary: "Abandon the change.".into() })).expect("Abandon acknowledgement journals");
    let unmatched = store.acknowledge_human_reconciliation(&request("agent-1", Uuid::new_v4(), ReconciliationAckInput { reservation_id: Some(reapply_reservation.reservation_id), decision: ReconciliationDecision::Reapply, files_reread: Vec::new(), human_change_summary: "No matching observation.".into() })).expect("unmatched acknowledgement journals");

    assert_eq!(ask_user.response, 0);
    assert_eq!(abandon.response, 0);
    assert_eq!(unmatched.response, 0);
    let acknowledgements = store.human_reconciliation_acknowledgements("workspace-1")
        .expect("acknowledgements read");
    assert_eq!(acknowledgements.len(), 3);
    let ask_user_record = acknowledgements.iter()
        .find(|record| record.decision == ReconciliationDecision::AskUser)
        .expect("AskUser acknowledgement persists");
    assert_eq!(ask_user_record.files_reread, vec!["src/a.rs".to_string()]);
    assert_eq!(ask_user_record.human_change_summary, "Need confirmation.");
    let abandon_record = acknowledgements.iter()
        .find(|record| record.decision == ReconciliationDecision::Abandon)
        .expect("Abandon acknowledgement persists");
    assert_eq!(abandon_record.files_reread, vec!["src/a.rs".to_string()]);
    assert_eq!(abandon_record.human_change_summary, "Abandon the change.");
    let unmatched_record = acknowledgements.iter()
        .find(|record| record.decision == ReconciliationDecision::Reapply)
        .expect("unmatched acknowledgement persists");
    assert!(unmatched_record.files_reread.is_empty());
    assert_eq!(unmatched_record.human_change_summary, "No matching observation.");
    assert_eq!(
        store.unreconciled_human_observations("workspace-1", &["src/a.rs".into()])
            .expect("pending observations").len(),
        1,
        "AskUser and Abandon never clear a human-write block",
    );
    store.rebuild_projections().expect("human acknowledgements replay");
}

#[test]
fn claim_batch_rejects_duplicate_and_ancestor_overlaps_instead_of_dropping_them() {
    let store = Store::open_in_memory_with_clock(FixedClock::new(NOW)).expect("store opens");
    let reservation = store.declare_reservation(&request("agent-1", Uuid::new_v4(), declaration("src/")))
        .expect("directory reservation declares").response;

    let duplicate = store.acquire_claim(&request("agent-1", Uuid::new_v4(), ClaimAcquire {
        reservation_id: reservation.reservation_id.clone(),
        paths: vec![
            ClaimPath { relative_path: "src/a.rs".into(), observation: None },
            ClaimPath { relative_path: "src/a.rs".into(), observation: None },
        ],
    }));
    assert!(matches!(duplicate, Err(stateful_store::StoreError::ClaimConflict)));

    let ancestor_overlap = store.acquire_claim(&request("agent-1", Uuid::new_v4(), ClaimAcquire {
        reservation_id: reservation.reservation_id,
        paths: vec![
            ClaimPath { relative_path: "src/a.rs".into(), observation: None },
            ClaimPath { relative_path: "src/".into(), observation: None },
        ],
    }));
    assert!(matches!(ancestor_overlap, Err(stateful_store::StoreError::ClaimConflict)));
}

#[test]
fn two_connections_return_the_frozen_duplicate_outcome() {
    let temporary = tempfile::tempdir().expect("temporary directory creates");
    let database = temporary.path().join("coordination.sqlite");
    let first_store = Store::open_with_clock(&database, FixedClock::new(NOW)).expect("first store opens");
    let second_store = Store::open_with_clock(&database, FixedClock::new(NOW)).expect("second store opens");
    let command = request("agent-1", Uuid::new_v4(), declaration("src/lib.rs"));

    let first = first_store.declare_reservation(&command).expect("first command commits");
    let duplicate = second_store.declare_reservation(&command).expect("second connection reuses receipt");

    assert!(!first.duplicate);
    assert!(duplicate.duplicate);
    assert_eq!(duplicate.response, first.response);
    assert_eq!(second_store.journal_event_count().expect("journal count"), 1);
}

#[test]
fn cancelling_a_claimable_wait_releases_the_grant_and_promotes_the_next_waiter() {
    let store = Store::open_in_memory_with_clock(FixedClock::new(NOW)).expect("store opens");
    let first = store.request_wait(&request("agent-1", Uuid::new_v4(), WaitRequest {
        relative_path: "src/lib.rs".into(), action: "write_file".into(), purpose: "First.".into(), blocking_agent_id: None,
    })).expect("first wait queues").response;
    let second = store.request_wait(&request("agent-2", Uuid::new_v4(), WaitRequest {
        relative_path: "src/lib.rs".into(), action: "write_file".into(), purpose: "Second.".into(), blocking_agent_id: None,
    })).expect("second wait queues").response;
    let granted = store.grant_next_wait(&request("agent-3", Uuid::new_v4(), WaitGrant {
        relative_path: "src/lib.rs".into(),
    })).expect("one wait grants").response.expect("a wait grants");
    let next = if granted.wait_id == first.wait_id { second } else { first };

    store.cancel_wait(&request(&granted.agent_id, Uuid::new_v4(), WaitCancellation {
        wait_id: granted.wait_id.clone(),
    })).expect("claimable wait cancels");

    assert_eq!(store.wait("workspace-1", &granted.wait_id).expect("granted wait reads").expect("wait exists").status, "canceled");
    assert_eq!(store.wait("workspace-1", &next.wait_id).expect("next wait reads").expect("wait exists").status, "claimable");
}

#[test]
fn wait_grant_only_transitions_queued_waits() {
    let store = Store::open_in_memory_with_clock(FixedClock::new(NOW)).expect("store opens");
    let queued = store.request_wait(&request("agent-1", Uuid::new_v4(), WaitRequest {
        relative_path: "src/lib.rs".into(), action: "write_file".into(), purpose: "Need the file.".into(), blocking_agent_id: None,
    })).expect("wait queues").response;
    let grant = request("agent-2", Uuid::new_v4(), WaitGrant { relative_path: "src/lib.rs".into() });
    let granted = store.grant_next_wait(&grant).expect("wait grants").response.expect("wait is queued");
    assert_eq!(granted.wait_id, queued.wait_id);
    let before = store.journal_event_count().expect("journal count");

    assert!(store.grant_next_wait(&request("agent-2", Uuid::new_v4(), WaitGrant {
        relative_path: "src/lib.rs".into(),
    })).expect("claimable wait is ignored").response.is_none());

    assert_eq!(store.journal_event_count().expect("journal count"), before);
}

#[test]
fn outbox_identity_does_not_collide_with_wait_delivery_identity() {
    let store = Store::open_in_memory_with_clock(FixedClock::new(NOW)).expect("store opens");
    let wait = store.request_wait(&request("agent-1", Uuid::new_v4(), WaitRequest {
        relative_path: "src/lib.rs".into(), action: "write_file".into(), purpose: "Need the file.".into(), blocking_agent_id: None,
    })).expect("wait queues").response;
    store.grant_next_wait(&request("agent-2", Uuid::new_v4(), WaitGrant {
        relative_path: "src/lib.rs".into(),
    })).expect("wait grants");

    let outbox = store.enqueue_outbox(&request("agent-1", Uuid::new_v4(), OutboxEntry {
        outbox_id: wait.wait_id.clone(), sequence: 1, event_type: "heartbeat".into(), payload: json!({"n": 1}),
    })).expect("outbox queues despite matching wait delivery identity").response;

    assert_eq!(outbox.outbox_id, wait.wait_id);
    assert!(store.delivery("workspace-1", &wait.wait_id).expect("wait delivery reads").is_some());
    assert!(store.outbox("workspace-1", &outbox.outbox_id).expect("outbox reads").is_some());
}

#[test]
fn duplicate_fence_refreshes_the_same_fence_with_the_new_action() {
    let store = Store::open_in_memory_with_clock(FixedClock::new(NOW)).expect("store opens");
    let original = store.acquire_write_fences(&request("agent-1", Uuid::new_v4(), WriteFenceAcquire {
        paths: vec!["src/lib.rs".into()], action: "write_file".into(),
    })).expect("fence acquires").response.fences.remove(0);
    let refreshed = store.acquire_write_fences(&request("agent-1", Uuid::new_v4(), WriteFenceAcquire {
        paths: vec!["src/lib.rs".into()], action: "write_directory".into(),
    })).expect("same path refreshes").response.fences.remove(0);

    assert_eq!(refreshed.fence_id, original.fence_id);
    assert_eq!(refreshed.action, "write_directory");
    assert_eq!(
        store.write_fence("workspace-1", &original.fence_id).expect("fence reads").expect("fence exists").action,
        "write_directory",
    );
}

#[test]
fn duplicate_fence_release_id_emits_one_transition_and_receipted_result() {
    let store = Store::open_in_memory_with_clock(FixedClock::new(NOW)).expect("store opens");
    let fence = store.acquire_write_fences(&request("agent-1", Uuid::new_v4(), WriteFenceAcquire {
        paths: vec!["src/lib.rs".into()], action: "write_file".into(),
    })).expect("fence acquires").response.fences.remove(0);
    let release = request("agent-1", Uuid::new_v4(), WriteFenceRelease {
        fence_ids: vec![fence.fence_id.clone(), fence.fence_id.clone()],
    });
    let before = store.journal_event_count().expect("journal count");

    let released = store.release_write_fences(&release).expect("duplicate fence IDs release once");

    assert_eq!(released.response.len(), 1);
    assert_eq!(released.response[0].status, "released");
    assert_eq!(store.journal_event_count().expect("journal count"), before + 1);
    assert_eq!(
        store.journal_event_types_for_request(release.request_id).expect("release event types"),
        vec!["write_fence.released"],
    );
    assert!(store.release_write_fences(&release).expect("receipt replays").duplicate);
}

#[test]
fn released_fence_attributes_human_writes_only_during_the_owner_grace_window() {
    let store = Store::open_in_memory_with_clock(FixedClock::new(NOW)).expect("store opens");
    let fence = store.acquire_write_fences(&request("agent-1", Uuid::new_v4(), WriteFenceAcquire {
        paths: vec!["src/lib.rs".into()], action: "write_file".into(),
    })).expect("fence acquires").response.fences.remove(0);
    store.release_write_fences(&request("agent-1", Uuid::new_v4(), WriteFenceRelease {
        fence_ids: vec![fence.fence_id],
    })).expect("fence releases");

    let within_grace = store.record_human_observation(&request("agent-2", Uuid::new_v4(), HumanObservationInput {
        relative_path: "src/lib.rs".into(), kind: HumanObservationKind::Save,
        confidence: HumanObservationConfidence::High, source: "watcher".into(),
        summary: "save during grace".into(), observed_at: Some(NOW + Duration::seconds(1)),
    })).expect("grace observation records").response;
    let after_grace = store.record_human_observation(&request("agent-2", Uuid::new_v4(), HumanObservationInput {
        relative_path: "src/lib.rs".into(), kind: HumanObservationKind::Save,
        confidence: HumanObservationConfidence::High, source: "watcher".into(),
        summary: "save after grace".into(), observed_at: Some(NOW + Duration::seconds(3)),
    })).expect("post grace observation records").response;

    assert_eq!(within_grace.status, "reconciled");
    assert_eq!(within_grace.reconciled_by_agent_id.as_deref(), Some("agent-1"));
    assert_eq!(after_grace.status, "pending");
    assert_eq!(after_grace.reconciled_by_agent_id, None);
}

#[test]
fn expired_notification_callback_is_inert() {
    let clock = MutableClock::new(NOW);
    let store = Store::open_in_memory_with_clock(clock.clone()).expect("store opens");
    let notification = store.create_notification(&request("agent-1", Uuid::new_v4(), NotificationCreate {
        target_agent_id: "agent-2".into(), kind: "context".into(), payload: json!({"v": 1}), coalesce_key: None,
    })).expect("notification queues").response;
    clock.advance(Duration::minutes(3));
    store.expire_notifications(&request("agent-1", Uuid::new_v4(), ())).expect("notification expires");
    let before = store.journal_event_count().expect("journal count");

    let callback = store.record_notification_delivery(&request("agent-2", Uuid::new_v4(), NotificationDelivery {
        notification_id: notification.notification_id.clone(), sequence: notification.sequence, outcome: DeliveryAttempt::Delivered, error: None, retry_at: None,
    })).expect("late callback is accepted inertly");

    assert_eq!(callback.response.status, "queued");
    assert_eq!(store.journal_event_count().expect("journal count"), before);
    assert_eq!(store.pending_notifications("agent-2", "workspace-1").expect("pending notifications").len(), 0);
}
