use serde::Serialize;
use serde_json::json;
use stateful_core::{
    ActorType, AgentIdentity, RequestEnvelope, SourceKind, SourceRef, WorkspaceIdentity,
};
use stateful_store::{
    ActivityFinalization, ActivityStart, ClaimAcquire, ClaimPath, Clock, DeliveryAttempt, FixedClock,
    HumanObservationConfidence, HumanObservationInput, HumanObservationKind, NotificationCreate,
    NotificationDelivery, OutboxDelivery, OutboxEntry, ReservationDeclaration, ReservationHeartbeat, ReservationRelease,
    Store, SyncStatus, WaitCancellation, WaitGrant, WaitRequest, WriteFenceAcquire, WriteFenceRelease,
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
    ReservationDeclaration { relative_path: path.into(), action: "write_file".into(), purpose: "Refactor the projector.".into() }
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
fn wait_grant_is_fifo_and_uses_wait_id_as_notification_identity() {
    let store = Store::open_in_memory_with_clock(FixedClock::new(NOW)).expect("store opens");
    let first = store.request_wait(&request("agent-1", Uuid::new_v4(), WaitRequest { relative_path: "src/lib.rs".into(), action: "write_file".into(), purpose: "First.".into(), blocking_agent_id: None }))
        .expect("first wait queues").response;
    let second = store.request_wait(&request("agent-2", Uuid::new_v4(), WaitRequest { relative_path: "src/lib.rs".into(), action: "write_file".into(), purpose: "Second.".into(), blocking_agent_id: None }))
        .expect("second wait queues").response;
    let grant = request("agent-1", Uuid::new_v4(), WaitGrant { relative_path: "src/lib.rs".into() });
    let granted = store.grant_next_wait(&grant).expect("first waiter grants").response.expect("waiter granted");
    let expected = if first.wait_id < second.wait_id { &first } else { &second };
    let queued = if first.wait_id < second.wait_id { &second } else { &first };
    assert_eq!(granted.wait_id, expected.wait_id);
    assert_eq!(
        store.pending_notifications(&expected.agent_id, "workspace-1").expect("notifications read")[0].notification_id,
        expected.wait_id,
    );
    assert!(store.grant_next_wait(&request("agent-1", Uuid::new_v4(), WaitGrant { relative_path: "src/lib.rs".into() }))
        .expect("active reservation blocks second grant").response.is_none());
    assert_eq!(store.wait("workspace-1", &queued.wait_id).expect("wait reads").expect("other remains").status, "queued");
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
    let delivery = request("agent-1", Uuid::new_v4(), NotificationDelivery { notification_id: notification.notification_id.clone(), outcome: DeliveryAttempt::Delivered, error: None, retry_at: None });
    store.record_notification_delivery(&delivery).expect("delivery records");
    let before = store.journal_event_count().expect("journal count");
    store.record_notification_delivery(&request("agent-1", Uuid::new_v4(), NotificationDelivery { notification_id: notification.notification_id.clone(), outcome: DeliveryAttempt::Delivered, error: None, retry_at: None }))
        .expect("repeated callback is inert");
    assert_eq!(store.journal_event_count().expect("journal count"), before);
    store.start_activity(&request("agent-1", Uuid::new_v4(), ActivityStart { phase: stateful_core::PresencePhase::Editing })).expect("activity starts");
    let before_finalize = store.journal_event_count().expect("journal count");
    store.finalize_activity(&request("agent-1", Uuid::new_v4(), ActivityFinalization {})).expect("activity finalizes");
    assert_eq!(store.journal_event_count().expect("journal count"), before_finalize + 1);
    store.rebuild_projections().expect("all aggregates replay");
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
        vec![1, 2],
    );

    clock.advance(Duration::minutes(3));
    store.expire_notifications(&request("agent-1", Uuid::new_v4(), ())).expect("notification expires");
    assert!(store.pending_notifications("agent-2", "workspace-1").expect("pending notifications").is_empty());
    store.rebuild_projections().expect("notifications replay");
}
