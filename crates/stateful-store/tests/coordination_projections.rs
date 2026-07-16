use serde_json::json;
use stateful_core::{
    ActorType, AgentIdentity, EventData, EventPayload, NewEvent, RequestEnvelope, ReservationEvent,
    SourceKind, SourceRef, WorkspaceIdentity,
};
use stateful_store::{CommandPlan, FixedClock, ReservationDeclaration, Store};
use time::{macros::datetime, OffsetDateTime};
use uuid::Uuid;
const NOW: OffsetDateTime = datetime!(2026-07-15 12:00 UTC);

fn request(request_id: Uuid) -> RequestEnvelope<serde_json::Value> {
    RequestEnvelope::new(
        request_id,
        NOW,
        AgentIdentity {
            agent_id: "agent-1".into(),
            turn_id: Some("turn-1".into()),
            actor_id: "actor-1".into(),
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
            source_ref: "coordination-projection-test".into(),
        },
        json!({"aggregate": "reservation"}),
    )
    .expect("test request should be valid")
}

fn reservation_event(request_id: Uuid, ordinal: u32, released: bool) -> NewEvent {
    let mut data = EventData::new("reservation-1");
    data.data = json!({
        "reservation": {
            "reservation_id": "reservation-1",
            "agent_id": "agent-1",
            "workspace_id": "workspace-1",
            "relative_path": "src/lib.rs",
            "purpose": "Refactor the projector.",
            "status": if released { "released" } else { "active" }
        }
    });
    NewEvent::new(
        request_id,
        ordinal,
        NOW,
        EventPayload::Reservation(if released {
            ReservationEvent::Released(data)
        } else {
            ReservationEvent::Declared(data)
        }),
    )
    .expect("reservation event should be valid")
}

#[test]
fn reservation_lifecycle_is_event_sourced_and_replayable() {
    let mut store = Store::open_in_memory_with_clock(FixedClock::new(NOW)).expect("store should open");
    let request = request(Uuid::new_v4());
    store
        .execute_command(&request, "reservation.lifecycle", |_| {
            Ok(CommandPlan {
                events: vec![
                    reservation_event(request.request_id, 0, false),
                    reservation_event(request.request_id, 1, true),
                ],
                response: json!({"reservation_id": "reservation-1"}),
                http_status: 200,
            })
        })
        .expect("reservation lifecycle should commit");

    let snapshot = store.projection_snapshot().expect("projection should load");
    let row = snapshot["reservation_current"]
        .iter()
        .find(|row| row[1] == "t:reservation-1")
        .expect("reservation projection should exist");
    assert_eq!(row[3], "i:2");
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(row[2].strip_prefix("t:").expect("text payload"))
            .expect("reservation payload should be valid JSON"),
        json!({
            "reservation_id": "reservation-1",
            "agent_id": "agent-1",
            "workspace_id": "workspace-1",
            "relative_path": "src/lib.rs",
            "purpose": "Refactor the projector.",
            "status": "released"
        }),
    );
    store.rebuild_projections().expect("reservation projection should replay");
}

fn aggregate_event(
    request_id: Uuid,
    ordinal: u32,
    aggregate_id: &str,
    record_key: &str,
    record: serde_json::Value,
    payload: impl FnOnce(EventData) -> EventPayload,
) -> NewEvent {
    let mut data = EventData::new(aggregate_id);
    data.data = json!({record_key: record});
    NewEvent::new(request_id, ordinal, NOW, payload(data)).expect("aggregate event should be valid")
}

fn assert_projected_lifecycle(
    route: &'static str,
    table: &str,
    aggregate_id: &str,
    first: NewEvent,
    last: NewEvent,
    expected: serde_json::Value,
) {
    let mut store = Store::open_in_memory_with_clock(FixedClock::new(NOW)).expect("store should open");
    let request_id = first.request_id;
    let request = request(request_id);
    store
        .execute_command(&request, route, |_| {
            Ok(CommandPlan {
                events: vec![first, last],
                response: json!({"aggregate_id": aggregate_id}),
                http_status: 200,
            })
        })
        .expect("aggregate lifecycle should commit");
    let snapshot = store.projection_snapshot().expect("projection should load");
    let row = snapshot[table]
        .iter()
        .find(|row| row[1] == format!("t:{aggregate_id}"))
        .expect("aggregate projection should exist");
    assert_eq!(row.last(), Some(&"i:2".to_string()), "projection must retain its origin event sequence");
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(row[row.len() - 2].strip_prefix("t:").expect("text payload"))
            .expect("projected payload should be JSON"),
        expected,
    );
    store.rebuild_projections().expect("aggregate projection should replay");
}

#[test]
fn claim_lifecycle_and_observation_refresh_are_event_sourced() {
    use stateful_core::ClaimEvent;
    let request_id = Uuid::new_v4();
    let active = json!({"claim_id": "claim-1", "relative_path": "src/lib.rs", "expires_at": "2026-07-15T12:05:00Z", "status": "active"});
    let refreshed = json!({"claim_id": "claim-1", "relative_path": "src/lib.rs", "expires_at": "2026-07-15T12:10:00Z", "status": "active", "observation": {"exists": true}});
    assert_projected_lifecycle(
        "claim.lifecycle",
        "claim_current",
        "claim-1",
        aggregate_event(request_id, 0, "claim-1", "claim", active, |data| EventPayload::Claim(ClaimEvent::Acquired(data))),
        aggregate_event(request_id, 1, "claim-1", "claim", refreshed.clone(), |data| EventPayload::Claim(ClaimEvent::ObservationRefreshed(data))),
        refreshed,
    );
}

#[test]
fn wait_fifo_grant_and_cancel_are_event_sourced() {
    use stateful_core::WaitEvent;
    let request_id = Uuid::new_v4();
    let queued = json!({"wait_id": "wait-1", "request_id": "request-1", "requested_at": "2026-07-15T12:00:00Z", "status": "queued"});
    let canceled = json!({"wait_id": "wait-1", "request_id": "request-1", "requested_at": "2026-07-15T12:00:00Z", "status": "canceled"});
    assert_projected_lifecycle(
        "wait.lifecycle",
        "wait_current",
        "wait-1",
        aggregate_event(request_id, 0, "wait-1", "wait", queued, |data| EventPayload::Wait(WaitEvent::Requested(data))),
        aggregate_event(request_id, 1, "wait-1", "wait", canceled.clone(), |data| EventPayload::Wait(WaitEvent::Cancelled(data))),
        canceled,
    );
}

#[test]
fn fence_conflict_release_and_expiry_are_event_sourced() {
    use stateful_core::WriteFenceEvent;
    let request_id = Uuid::new_v4();
    let acquired = json!({"fence_id": "fence-1", "relative_path": "src/lib.rs", "expires_at": "2026-07-15T12:05:00Z", "status": "active"});
    let released = json!({"fence_id": "fence-1", "relative_path": "src/lib.rs", "expires_at": "2026-07-15T12:05:00Z", "status": "released"});
    assert_projected_lifecycle(
        "fence.lifecycle",
        "write_fence_current",
        "fence-1",
        aggregate_event(request_id, 0, "fence-1", "write_fence", acquired, |data| EventPayload::WriteFence(WriteFenceEvent::Acquired(data))),
        aggregate_event(request_id, 1, "fence-1", "write_fence", released.clone(), |data| EventPayload::WriteFence(WriteFenceEvent::Released(data))),
        released,
    );
}

#[test]
fn human_observe_reconcile_and_expire_are_event_sourced() {
    use stateful_core::HumanObservationEvent;
    let request_id = Uuid::new_v4();
    let observed = json!({"observation_id": "human-1", "relative_path": "src/lib.rs", "status": "unreconciled"});
    let reconciled = json!({"observation_id": "human-1", "relative_path": "src/lib.rs", "status": "reconciled", "decision": "adopt"});
    assert_projected_lifecycle(
        "human.lifecycle",
        "human_observation_current",
        "human-1",
        aggregate_event(request_id, 0, "human-1", "observation", observed, |data| EventPayload::HumanObservation(HumanObservationEvent::Observed(data))),
        aggregate_event(request_id, 1, "human-1", "observation", reconciled.clone(), |data| EventPayload::HumanObservation(HumanObservationEvent::Reconciled(data))),
        reconciled,
    );
}

#[test]
fn notification_create_coalesce_deliver_and_expire_are_event_sourced() {
    use stateful_core::NotificationEvent;
    let request_id = Uuid::new_v4();
    let created = json!({"notification_id": "notification-1", "target_agent_id": "agent-1", "sequence": 1, "status": "queued"});
    let delivered = json!({"notification_id": "notification-1", "target_agent_id": "agent-1", "sequence": 1, "status": "delivered", "attempt": 1});
    assert_projected_lifecycle(
        "notification.lifecycle",
        "notification_current",
        "notification-1",
        aggregate_event(request_id, 0, "notification-1", "notification", created, |data| EventPayload::Notification(NotificationEvent::Created(data))),
        aggregate_event(request_id, 1, "notification-1", "notification", delivered.clone(), |data| EventPayload::Notification(NotificationEvent::Delivered(data))),
        delivered,
    );
}

#[test]
fn duplicate_grant_request_is_receipted_without_duplicate_notification() {
    use stateful_core::WaitEvent;
    let store = Store::open_in_memory_with_clock(FixedClock::new(NOW)).expect("store should open");
    let request_id = Uuid::new_v4();
    let request = request(request_id);
    let event = aggregate_event(
        request_id,
        0,
        "wait-1",
        "wait",
        json!({"wait_id": "wait-1", "request_id": "request-1", "status": "claimable"}),
        |data| EventPayload::Wait(WaitEvent::BecameClaimable(data)),
    );
    let first = store
        .execute_command(&request, "wait.grant", |_| Ok(CommandPlan { events: vec![event], response: json!({"wait_id": "wait-1"}), http_status: 200 }))
        .expect("first grant should commit");
    let duplicate = store
        .execute_command(&request, "wait.grant", |_| -> stateful_store::StoreResult<CommandPlan<serde_json::Value>> {
            panic!("duplicate must return its receipt without replanning")
        })
        .expect("duplicate grant should be idempotent");
    assert!(!first.duplicate);
    assert!(duplicate.duplicate);
    assert_eq!(store.journal_event_count().expect("journal count should load"), 1);
}

#[test]
fn terminal_activity_does_not_emit_a_warning_event() {
    use stateful_core::PresenceEvent;
    let store = Store::open_in_memory_with_clock(FixedClock::new(NOW)).expect("store should open");
    let request_id = Uuid::new_v4();
    let request = request(request_id);
    let event = aggregate_event(
        request_id,
        0,
        "agent-1",
        "activity",
        json!({"agent_id": "agent-1", "status": "finalized"}),
        |data| EventPayload::Presence(PresenceEvent::Finalized(data)),
    );
    store
        .execute_command(&request, "activity.finalize", |_| Ok(CommandPlan { events: vec![event], response: json!({}), http_status: 200 }))
        .expect("activity should finalize");
    assert_eq!(
        store.journal_event_types_for_request(request_id).expect("journal types should load"),
        vec!["presence.finalized"],
    );
}


#[test]
fn reservation_command_is_journaled_receipted_and_replayable() {
    let mut store = Store::open_in_memory_with_clock(FixedClock::new(NOW)).expect("store should open");
    let request = reservation_request(Uuid::new_v4());
    let first = store
        .declare_reservation(&request)
        .expect("reservation command should commit");
    let duplicate = store
        .declare_reservation(&request)
        .expect("duplicate should return frozen receipt");

    assert!(!first.duplicate);
    assert!(duplicate.duplicate);
    assert_eq!(store.journal_event_count().expect("journal should load"), 1);
    assert_eq!(first.response, duplicate.response);
    store.rebuild_projections().expect("reservation must replay");
}

fn reservation_request(request_id: Uuid) -> RequestEnvelope<ReservationDeclaration> {
    let base = request(request_id);
    RequestEnvelope::new(
        base.request_id,
        base.observed_at,
        base.agent,
        base.workspace,
        base.source,
        ReservationDeclaration { scopes: vec![stateful_core::ReservationScope::file("src/lib.rs")], action: "write_file".into(), purpose: "Refactor the projector.".into() },
    )
    .expect("reservation request should be valid")
}
#[test]
fn aggregate_failure_rolls_back_a_multi_event_transition() {
    let mut store = Store::open_in_memory_with_clock(FixedClock::new(NOW)).expect("store should open");
    store.fail_projector_on_event_for_tests(2);
    let request_id = Uuid::new_v4();
    let request = request(request_id);
    assert!(store
        .execute_command(&request, "reservation.failure", |_| {
            Ok(CommandPlan {
                events: vec![reservation_event(request_id, 0, false), reservation_event(request_id, 1, true)],
                response: json!({}),
                http_status: 200,
            })
        })
        .is_err());
    assert_eq!(store.journal_event_count().expect("journal count should load"), 0);
    assert_eq!(store.command_receipt_count().expect("receipt count should load"), 0);
}
