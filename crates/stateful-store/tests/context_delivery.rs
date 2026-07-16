use stateful_core::{
    AGENT_CONTEXT_SCOPE_SOURCE_REF, ActorType, AgentIdentity, ContextDelta, CurrentItemKind,
    CurrentSeverity, EventData, EventPayload, ExplicitHandoff, HandoffStatus,
    HumanObservationEvent, MigrationEvent, NewEvent, PresenceResourceRelation, RenderMode,
    RequestEnvelope, ReservationEvent, SourceKind, SourceRef, WorkspaceIdentity, WriteIntentStart,
    WriteTarget,
};
use stateful_store::{
    ClaimAcquire, ClaimPath, Clock, CommandPlan, ContextAcknowledgement, ContextRender,
    DeliveryAttempt, FixedClock, HumanObservationConfidence, HumanObservationInput,
    HumanObservationKind, NotificationCreate, NotificationDelivery, PresenceRegistration,
    PresenceResourceUpdate, ReservationDeclaration, ReservationRelease, Store, WaitRequest,
};
use tempfile::TempDir;
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
        *self.0.lock().expect("clock lock") += duration;
    }
}

impl Clock for MutableClock {
    fn now(&self) -> OffsetDateTime {
        *self.0.lock().expect("clock lock")
    }
}

fn request<T: serde::Serialize>(agent_id: &str, request_id: Uuid, payload: T) -> RequestEnvelope<T> {
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
        SourceRef {
            kind: SourceKind::Cli,
            event: "test".into(),
            tool_name: None,
            source_ref: "context-delivery-test".into(),
        },
        payload,
    )
    .expect("test request is valid")
}

fn request_in_workspace<T: serde::Serialize>(
    workspace_id: &str,
    agent_id: &str,
    request_id: Uuid,
    payload: T,
) -> RequestEnvelope<T> {
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
            workspace_id: workspace_id.into(),
            repo_id: "repo-1".into(),
            worktree_id: format!("worktree-{workspace_id}"),
            branch: "main".into(),
        },
        SourceRef {
            kind: SourceKind::Cli,
            event: "test".into(),
            tool_name: None,
            source_ref: "context-delivery-test".into(),
        },
        payload,
    )
    .expect("test request is valid")
}

#[test]
fn render_redelivers_until_matching_cumulative_ack() {
    let store = Store::open_in_memory_with_clock(FixedClock::new(NOW)).expect("store opens");
    store
        .declare_reservation(&request(
            "agent-1",
            Uuid::new_v4(),
            ReservationDeclaration { scopes: vec![stateful_core::ReservationScope::file("src/lib.rs")], action: "write_file".into(), purpose: "Update the library.".into() },
        ))
        .expect("state change succeeds");

    let first = store
        .render_context(&request(
            "agent-2",
            Uuid::new_v4(),
            ContextRender { mode: RenderMode::Brief, resource: None },
        ))
        .expect("first render succeeds")
        .response;
    let second = store
        .render_context(&request(
            "agent-2",
            Uuid::new_v4(),
            ContextRender { mode: RenderMode::Brief, resource: None },
        ))
        .expect("unacknowledged render succeeds")
        .response;

    assert_eq!(first.workspace_version, 1);
    assert!(first.changed);
    assert_eq!(second, first, "replay must preserve the sent payload exactly");

    let acknowledgement = ContextAcknowledgement {
        delivery_id: first.delivery_id.clone().expect("delivery id"),
        sequence: first.sequence.expect("delivery sequence"),
        workspace_version: first.workspace_version,
    };
    store
        .acknowledge_context(&request("agent-2", Uuid::new_v4(), acknowledgement))
        .expect("matching acknowledgement succeeds");

    let after_ack: ContextDelta = store
        .render_context(&request(
            "agent-2",
            Uuid::new_v4(),
            ContextRender { mode: RenderMode::Brief, resource: None },
        ))
        .expect("render after acknowledgement succeeds")
        .response;
    assert!(!after_ack.changed);
    assert!(after_ack.delivery_id.is_none());
}

#[test]
fn acknowledgements_are_cumulative_and_isolated_per_agent() {
    let store = Store::open_in_memory_with_clock(FixedClock::new(NOW)).expect("store opens");
    for relative_path in ["src/one.rs", "src/two.rs"] {
        store
            .declare_reservation(&request(
                "agent-1",
                Uuid::new_v4(),
                ReservationDeclaration { scopes: vec![stateful_core::ReservationScope::file(relative_path)], action: "write_file".into(), purpose: "Update source.".into() },
            ))
            .expect("state change succeeds");
        let _ = store
            .render_context(&request(
                "agent-2",
                Uuid::new_v4(),
                ContextRender { mode: RenderMode::Brief, resource: None },
            ))
            .expect("render succeeds");
    }

    let deliveries = store
        .pending_context_deliveries("agent-2", "workspace-1")
        .expect("pending deliveries load");
    assert_eq!(deliveries.len(), 2);
    let first = &deliveries[0];
    let second = &deliveries[1];

    store
        .acknowledge_context(&request(
            "agent-2",
            Uuid::new_v4(),
            ContextAcknowledgement {
                delivery_id: first.delivery_id.clone(),
                sequence: first.sequence,
                workspace_version: first.workspace_version,
            },
        ))
        .expect("older acknowledgement succeeds");
    assert_eq!(store.context_cursor("workspace-1", "agent-2").expect("cursor loads"), first.workspace_version);
    assert_eq!(
        store
            .pending_context_deliveries("agent-2", "workspace-1")
            .expect("newer delivery remains")
            .iter()
            .map(|delivery| delivery.workspace_version)
            .collect::<Vec<_>>(),
        vec![second.workspace_version],
    );
    assert!(
        store
            .acknowledge_context(&request(
                "agent-3",
                Uuid::new_v4(),
                ContextAcknowledgement {
                    delivery_id: second.delivery_id.clone(),
                    sequence: second.sequence,
                    workspace_version: second.workspace_version,
                },
            ))
            .is_err(),
        "a different agent must not acknowledge this delivery",
    );

    store
        .acknowledge_context(&request(
            "agent-2",
            Uuid::new_v4(),
            ContextAcknowledgement {
                delivery_id: second.delivery_id.clone(),
                sequence: second.sequence,
                workspace_version: second.workspace_version,
            },
        ))
        .expect("latest acknowledgement succeeds");
    assert_eq!(store.context_cursor("workspace-1", "agent-2").expect("cursor loads"), second.workspace_version);
    assert!(store
        .pending_context_deliveries("agent-2", "workspace-1")
        .expect("deliveries load")
        .is_empty());
}

#[test]
fn newer_delivery_keeps_sent_payload_immutable_and_coalesces_one_unread_notification() {
    let store = Store::open_in_memory_with_clock(FixedClock::new(NOW)).expect("store opens");
    let _first_state = store
        .declare_reservation(&request(
            "agent-1",
            Uuid::new_v4(),
            ReservationDeclaration { scopes: vec![stateful_core::ReservationScope::file("src/one.rs")], action: "write_file".into(), purpose: "First change.".into() },
        ))
        .expect("first state succeeds");
    let first = store
        .render_context(&request(
            "agent-2",
            Uuid::new_v4(),
            ContextRender { mode: RenderMode::Brief, resource: None },
        ))
        .expect("first delivery succeeds")
        .response;
    store
        .declare_reservation(&request(
            "agent-1",
            Uuid::new_v4(),
            ReservationDeclaration { scopes: vec![stateful_core::ReservationScope::file("src/two.rs")], action: "write_file".into(), purpose: "Second change.".into() },
        ))
        .expect("second state succeeds");
    let second = store
        .render_context(&request(
            "agent-2",
            Uuid::new_v4(),
            ContextRender { mode: RenderMode::Brief, resource: None },
        ))
        .expect("second delivery succeeds")
        .response;

    assert_ne!(first.delivery_id, second.delivery_id);
    assert_eq!(first.workspace_version, 1);
    assert_eq!(second.workspace_version, 5);
    let persisted_first = store
        .context_delivery(
            "workspace-1",
            first.delivery_id.as_deref().expect("first delivery id"),
        )
        .expect("first delivery loads")
        .expect("first delivery remains replayable");
    assert_eq!(persisted_first.items, first.items);
    assert_eq!(persisted_first.prompt_text, first.prompt_text);
    let notifications = store
        .pending_notifications("agent-2", "workspace-1")
        .expect("notification loads");
    assert_eq!(notifications.len(), 1);
    assert_eq!(notifications[0].payload["target_version"], second.workspace_version);
}

#[test]
fn irrelevant_final_state_still_delivers_an_empty_delta_until_acknowledged() {
    let store = Store::open_in_memory_with_clock(FixedClock::new(NOW)).expect("store opens");
    let reservation = store
        .declare_reservation(&request(
            "agent-1",
            Uuid::new_v4(),
            ReservationDeclaration { scopes: vec![stateful_core::ReservationScope::file("src/transient.rs")], action: "write_file".into(), purpose: "Temporary work.".into() },
        ))
        .expect("reservation succeeds")
        .response;
    store
        .release_reservation(&request(
            "agent-1",
            Uuid::new_v4(),
            ReservationRelease { reservation_id: reservation.reservation_id },
        ))
        .expect("release succeeds");

    let delta = store
        .render_context(&request(
            "agent-2",
            Uuid::new_v4(),
            ContextRender { mode: RenderMode::Brief, resource: None },
        ))
        .expect("empty delta renders")
        .response;
    assert!(delta.changed);
    assert!(delta.items.is_empty());
    assert!(delta.prompt_text.is_empty());
    store
        .acknowledge_context(&request(
            "agent-2",
            Uuid::new_v4(),
            ContextAcknowledgement {
                delivery_id: delta.delivery_id.clone().expect("delivery id"),
                sequence: delta.sequence.expect("sequence"),
                workspace_version: delta.workspace_version,
            },
        ))
        .expect("empty delta acknowledgement succeeds");
    assert!(!store
        .render_context(&request(
            "agent-2",
            Uuid::new_v4(),
            ContextRender { mode: RenderMode::Brief, resource: None },
        ))
        .expect("post-ack render succeeds")
        .response
        .changed);
}

#[test]
fn acknowledgements_require_bound_sequence_and_are_idempotent() {
    let store = Store::open_in_memory_with_clock(FixedClock::new(NOW)).expect("store opens");
    store
        .declare_reservation(&request(
            "agent-1",
            Uuid::new_v4(),
            ReservationDeclaration { scopes: vec![stateful_core::ReservationScope::file("src/ack.rs")], action: "write_file".into(), purpose: "Ack test.".into() },
        ))
        .expect("state change succeeds");
    let delta = store
        .render_context(&request(
            "agent-2",
            Uuid::new_v4(),
            ContextRender { mode: RenderMode::Brief, resource: None },
        ))
        .expect("render succeeds")
        .response;
    assert!(store
        .acknowledge_context(&request(
            "agent-2",
            Uuid::new_v4(),
            ContextAcknowledgement {
                delivery_id: delta.delivery_id.clone().expect("delivery id"),
                sequence: delta.sequence.expect("sequence") + 1,
                workspace_version: delta.workspace_version,
            },
        ))
        .is_err());
    let acknowledgement = ContextAcknowledgement {
        delivery_id: delta.delivery_id.expect("delivery id"),
        sequence: delta.sequence.expect("sequence"),
        workspace_version: delta.workspace_version,
    };
    store
        .acknowledge_context(&request("agent-2", Uuid::new_v4(), acknowledgement.clone()))
        .expect("acknowledgement succeeds");
    let journal_count = store.journal_event_count().expect("journal count");
    store
        .acknowledge_context(&request("agent-2", Uuid::new_v4(), acknowledgement))
        .expect("repeat acknowledgement succeeds");
    assert_eq!(store.journal_event_count().expect("journal count"), journal_count);
}

#[test]
fn deliveries_expire_to_replayable_dead_letters_after_twenty_four_hours() {
    let clock = MutableClock::new(NOW);
    let mut store = Store::open_in_memory_with_clock(clock.clone()).expect("store opens");
    store
        .declare_reservation(&request(
            "agent-1",
            Uuid::new_v4(),
            ReservationDeclaration { scopes: vec![stateful_core::ReservationScope::file("src/expiry.rs")], action: "write_file".into(), purpose: "Expiry test.".into() },
        ))
        .expect("state change succeeds");
    let delta = store
        .render_context(&request(
            "agent-2",
            Uuid::new_v4(),
            ContextRender { mode: RenderMode::Brief, resource: None },
        ))
        .expect("render succeeds")
        .response;
    let delivery_id = delta.delivery_id.expect("delivery id");

    clock.advance(Duration::hours(24) + Duration::seconds(1));
    assert_eq!(
        store
            .expire_context_deliveries(&request("agent-2", Uuid::new_v4(), ()))
            .expect("expiry succeeds")
            .response,
        vec![delivery_id.clone()],
    );
    let delivery = store
        .context_delivery("workspace-1", &delivery_id)
        .expect("delivery loads")
        .expect("delivery exists");
    assert_eq!(delivery.status, "dead_letter");
    assert!(delivery.origin_event_seq > 0);
    store.rebuild_projections().expect("dead letter replays");
    assert_eq!(
        store
            .context_delivery("workspace-1", &delivery_id)
            .expect("delivery reloads")
            .expect("delivery remains")
            .status,
        "dead_letter",
    );
}

#[test]
fn overflow_uses_an_exact_summary_after_twenty_and_dead_letters_after_sixty_four() {
    let store = Store::open_in_memory_with_clock(FixedClock::new(NOW)).expect("store opens");
    let mut delivery_ids = Vec::new();
    for index in 0..65 {
        store
            .declare_reservation(&request(
                "agent-1",
                Uuid::new_v4(),
                ReservationDeclaration { scopes: vec![stateful_core::ReservationScope::file(format!("src/{index}.rs"))], action: "write_file".into(), purpose: "Overflow test.".into() },
            ))
            .expect("state change succeeds");
        let delta = store
            .render_context(&request(
                "agent-2",
                Uuid::new_v4(),
                ContextRender { mode: RenderMode::Brief, resource: None },
            ))
            .expect("render succeeds")
            .response;
        delivery_ids.push(delta.delivery_id.expect("delivery id"));
    }

    let summary = store
        .context_delivery("workspace-1", &delivery_ids[20])
        .expect("summary delivery loads")
        .expect("summary delivery exists");
    assert_eq!(
        summary.prompt_text,
        "Context delivery queue: 21 unacknowledged deliveries; this is version 271.\n",
    );
    assert_eq!(
        store
            .pending_context_deliveries("agent-2", "workspace-1")
            .expect("pending deliveries load")
            .len(),
        64,
    );
    assert_eq!(
        store
            .context_delivery("workspace-1", &delivery_ids[64])
            .expect("overflow delivery loads")
            .expect("overflow delivery exists")
            .status,
        "dead_letter",
    );
    assert_eq!(
        store
            .context_delivery("workspace-1", &delivery_ids[64])
            .expect("overflow summary loads")
            .expect("overflow summary exists")
            .prompt_text,
        "Context delivery queue: 64 unacknowledged deliveries; this is version 2273.\n",
    );
}

#[test]
fn dead_letter_acknowledgement_keeps_the_persisted_cursor() {
    let clock = MutableClock::new(NOW);
    let store = Store::open_in_memory_with_clock(clock.clone()).expect("store opens");
    store
        .declare_reservation(&request(
            "agent-1",
            Uuid::new_v4(),
            ReservationDeclaration { scopes: vec![stateful_core::ReservationScope::file("src/dead.rs")], action: "write_file".into(), purpose: "Dead letter test.".into() },
        ))
        .expect("state change succeeds");
    let delta = store
        .render_context(&request(
            "agent-2",
            Uuid::new_v4(),
            ContextRender { mode: RenderMode::Brief, resource: None },
        ))
        .expect("render succeeds")
        .response;
    clock.advance(Duration::hours(24) + Duration::seconds(1));
    store
        .expire_context_deliveries(&request("agent-2", Uuid::new_v4(), ()))
        .expect("expiry succeeds");

    let acknowledgement = store
        .acknowledge_context(&request(
            "agent-2",
            Uuid::new_v4(),
            ContextAcknowledgement {
                delivery_id: delta.delivery_id.expect("delivery id"),
                sequence: delta.sequence.expect("sequence"),
                workspace_version: delta.workspace_version,
            },
        ))
        .expect("dead letter acknowledgement is inert")
        .response;
    assert_eq!(acknowledgement.cursor, 0);
    assert_eq!(store.context_cursor("workspace-1", "agent-2").expect("cursor loads"), 0);
}

#[test]
fn claimable_wait_is_actionable_and_own_coordination_is_active_scope() {
    let store = Store::open_in_memory_with_clock(FixedClock::new(NOW)).expect("store opens");
    let reservation = store
        .declare_reservation(&request(
            "agent-1",
            Uuid::new_v4(),
            ReservationDeclaration { scopes: vec![stateful_core::ReservationScope::file("src/granted.rs")], action: "write_file".into(), purpose: "Current owner.".into() },
        ))
        .expect("reservation succeeds")
        .response;
    store
        .request_wait(&request(
            "agent-2",
            Uuid::new_v4(),
            WaitRequest {
                relative_path: "src/granted.rs".into(),
                action: "write_file".into(),
                purpose: "Need the file.".into(),
                blocking_agent_id: Some("agent-1".into()),
            },
        ))
        .expect("wait succeeds");
    store
        .release_reservation(&request(
            "agent-1",
            Uuid::new_v4(),
            ReservationRelease { reservation_id: reservation.reservation_id },
        ))
        .expect("release grants the wait");

    let delta = store
        .render_context(&request(
            "agent-2",
            Uuid::new_v4(),
            ContextRender { mode: RenderMode::Brief, resource: None },
        ))
        .expect("render succeeds")
        .response;
    let grant = delta
        .items
        .iter()
        .find(|item| item.kind == CurrentItemKind::ClaimableReservation)
        .expect("claimable wait is included");
    assert_eq!(grant.next_action.as_deref(), Some("Claim the granted reservation before it expires."));
    assert!(grant.source_refs.iter().any(|source| source == AGENT_CONTEXT_SCOPE_SOURCE_REF));
    assert!(
        delta
            .items
            .iter()
            .filter(|item| item.agent_id.as_deref() == Some("agent-2"))
            .all(|item| item.source_refs.iter().any(|source| source == AGENT_CONTEXT_SCOPE_SOURCE_REF)),
        "own coordination items are never represented as another agent's blocker",
    );
}

#[test]
fn stale_notification_callback_cannot_deliver_a_coalesced_newer_version() {
    let store = Store::open_in_memory_with_clock(FixedClock::new(NOW)).expect("store opens");
    let first = store
        .create_notification(&request(
            "agent-1",
            Uuid::new_v4(),
            NotificationCreate {
                target_agent_id: "agent-2".into(),
                kind: "context_invalidated".into(),
                payload: serde_json::json!({"target_version": 1}),
                coalesce_key: Some("context-agent-2".into()),
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
                kind: "context_invalidated".into(),
                payload: serde_json::json!({"target_version": 2}),
                coalesce_key: Some("context-agent-2".into()),
            },
        ))
        .expect("newer notification coalesces")
        .response;
    assert_ne!(first.sequence, second.sequence);
    store
        .record_notification_delivery(&request(
            "agent-2",
            Uuid::new_v4(),
            NotificationDelivery {
                notification_id: first.notification_id,
                sequence: first.sequence,
                outcome: DeliveryAttempt::Delivered,
                error: None,
                retry_at: None,
            },
        ))
        .expect("stale callback is accepted inertly");
    let notification = store
        .pending_notifications("agent-2", "workspace-1")
        .expect("notification loads")
        .into_iter()
        .next()
        .expect("newer notification remains queued");
    assert_eq!(notification.sequence, second.sequence);
    assert_eq!(notification.payload["target_version"], 2);
}

#[test]
fn context_invalidated_notification_lifecycle_does_not_advance_context_version() {
    let store = Store::open_in_memory_with_clock(FixedClock::new(NOW)).expect("store opens");
    store
        .create_notification(&request(
            "agent-1",
            Uuid::new_v4(),
            NotificationCreate {
                target_agent_id: "agent-2".into(),
                kind: "context_invalidated".into(),
                payload: serde_json::json!({"target_version": 7}),
                coalesce_key: Some("context-agent-2".into()),
            },
        ))
        .expect("notification queues");
    assert_eq!(
        store.workspace_version("workspace-1").expect("version loads"),
        0,
        "delivery transport is not coordination state",
    );
}

#[test]
fn own_outcome_unknown_remains_a_blocking_safety_state() {
    let store = Store::open_in_memory_with_clock(FixedClock::new(NOW)).expect("store opens");
    let before = stateful_core::fingerprint_reader(std::io::Cursor::new(b"before"))
        .expect("before fingerprint");
    let after = stateful_core::fingerprint_reader(std::io::Cursor::new(b"after"))
        .expect("after fingerprint");
    let intent = store
        .start_write_intent(&request(
            "agent-1",
            Uuid::new_v4(),
            WriteIntentStart {
                operation_id: "write-unknown".into(),
                action: "write_file".into(),
                targets: vec![WriteTarget { path: "src/unknown.rs".into(), before }],
            },
        ))
        .expect("intent starts")
        .response;
    store
        .recover_write_intent(&request(
            "agent-1",
            Uuid::new_v4(),
            (intent.intent_id, vec![("src/unknown.rs".into(), after)]),
        ))
        .expect("different post-state becomes unknown");

    let delta = store
        .render_context(&request(
            "agent-1",
            Uuid::new_v4(),
            ContextRender { mode: RenderMode::Brief, resource: Some("src/unknown.rs".into()) },
        ))
        .expect("render succeeds")
        .response;
    let safety = delta
        .items
        .iter()
        .find(|item| item.summary.contains("write outcome") && item.summary.contains("unknown"))
        .expect("own unknown write is included");
    assert_eq!(safety.severity, CurrentSeverity::Block);
    assert!(
        !safety.source_refs.iter().any(|source| source == AGENT_CONTEXT_SCOPE_SOURCE_REF),
        "safety state must not become an active-scope FYI",
    );
    assert_eq!(
        safety.next_action.as_deref(),
        Some("Perform a fresh exact read and reconcile the unknown write outcome."),
    );
}

#[test]
fn notification_lifecycle_transport_never_advances_context_version() {
    let clock = MutableClock::new(NOW);
    let store = Store::open_in_memory_with_clock(clock.clone()).expect("store opens");
    let first = store
        .create_notification(&request(
            "agent-1",
            Uuid::new_v4(),
            NotificationCreate {
                target_agent_id: "agent-2".into(),
                kind: "context_invalidated".into(),
                payload: serde_json::json!({"target_version": 1}),
                coalesce_key: Some("context-agent-2".into()),
            },
        ))
        .expect("notification queues")
        .response;
    assert_eq!(store.workspace_version("workspace-1").expect("version loads"), 0);
    let coalesced = store
        .create_notification(&request(
            "agent-1",
            Uuid::new_v4(),
            NotificationCreate {
                target_agent_id: "agent-2".into(),
                kind: "context_invalidated".into(),
                payload: serde_json::json!({"target_version": 2}),
                coalesce_key: Some("context-agent-2".into()),
            },
        ))
        .expect("notification coalesces")
        .response;
    assert_eq!(first.notification_id, coalesced.notification_id);
    assert_eq!(store.workspace_version("workspace-1").expect("version loads"), 0);
    for outcome in [DeliveryAttempt::Attempted, DeliveryAttempt::Failed, DeliveryAttempt::Delivered] {
        store
            .record_notification_delivery(&request(
                "agent-2",
                Uuid::new_v4(),
                NotificationDelivery {
                    notification_id: coalesced.notification_id.clone(),
                    sequence: coalesced.sequence,
                    outcome,
                    error: Some("transient".into()),
                    retry_at: None,
                },
            ))
            .expect("current callback succeeds");
        assert_eq!(store.workspace_version("workspace-1").expect("version loads"), 0);
    }
    let queued = store
        .create_notification(&request(
            "agent-1",
            Uuid::new_v4(),
            NotificationCreate {
                target_agent_id: "agent-2".into(),
                kind: "context_invalidated".into(),
                payload: serde_json::json!({"target_version": 3}),
                coalesce_key: Some("context-agent-2".into()),
            },
        ))
        .expect("second notification queues")
        .response;
    clock.advance(Duration::minutes(3));
    store
        .expire_notifications(&request("agent-1", Uuid::new_v4(), ()))
        .expect("queued notification expires");
    assert_eq!(queued.status, "queued");
    assert_eq!(store.workspace_version("workspace-1").expect("version loads"), 0);
}

#[test]
fn same_agent_context_deliveries_and_cursors_are_isolated_by_workspace() {
    let store = Store::open_in_memory_with_clock(FixedClock::new(NOW)).expect("store opens");
    for workspace_id in ["workspace-a", "workspace-b"] {
        store
            .declare_reservation(&request_in_workspace(
                workspace_id,
                "owner",
                Uuid::new_v4(),
                ReservationDeclaration { scopes: vec![stateful_core::ReservationScope::file("src/shared.rs")], action: "write_file".into(), purpose: "State change.".into() },
            ))
            .expect("state change succeeds");
    }
    let delivery_a = store
        .render_context(&request_in_workspace(
            "workspace-a",
            "same-agent",
            Uuid::new_v4(),
            ContextRender { mode: RenderMode::Brief, resource: None },
        ))
        .expect("workspace a renders")
        .response;
    let delivery_b = store
        .render_context(&request_in_workspace(
            "workspace-b",
            "same-agent",
            Uuid::new_v4(),
            ContextRender { mode: RenderMode::Brief, resource: None },
        ))
        .expect("workspace b renders")
        .response;
    assert_ne!(delivery_a.delivery_id, delivery_b.delivery_id);
    assert!(
        store
            .acknowledge_context(&request_in_workspace(
                "workspace-b",
                "same-agent",
                Uuid::new_v4(),
                ContextAcknowledgement {
                    delivery_id: delivery_a.delivery_id.clone().expect("delivery id"),
                    sequence: delivery_a.sequence.expect("sequence"),
                    workspace_version: delivery_a.workspace_version,
                },
            ))
            .is_err(),
        "a delivery identifier is never valid outside its workspace",
    );
    store
        .acknowledge_context(&request_in_workspace(
            "workspace-a",
            "same-agent",
            Uuid::new_v4(),
            ContextAcknowledgement {
                delivery_id: delivery_a.delivery_id.expect("delivery id"),
                sequence: delivery_a.sequence.expect("sequence"),
                workspace_version: delivery_a.workspace_version,
            },
        ))
        .expect("workspace a acknowledgement succeeds");
    assert_eq!(store.context_cursor("workspace-a", "same-agent").expect("cursor loads"), 1);
    assert_eq!(store.context_cursor("workspace-b", "same-agent").expect("cursor loads"), 0);
    assert_eq!(
        store
            .pending_context_deliveries("same-agent", "workspace-b")
            .expect("workspace b deliveries load")
            .len(),
        1,
    );
}

#[test]
fn resource_filter_keeps_only_relevant_presence_handoff_and_coordination_state() {
    let mut store = Store::open_in_memory_with_clock(FixedClock::new(NOW)).expect("store opens");
    for (agent_id, path) in [("present", "src/relevant.rs"), ("other-present", "src/other.rs")] {
        store
            .register_presence(&request(
                agent_id,
                Uuid::new_v4(),
                PresenceRegistration { first_prompt: None },
            ))
            .expect("presence registers");
        store
            .update_presence_resource(&request(
                agent_id,
                Uuid::new_v4(),
                PresenceResourceUpdate {
                    relative_path: path.into(),
                    relation: PresenceResourceRelation::Planned,
                },
            ))
            .expect("resource updates");
    }
    for (agent_id, path) in [("handoff", "src/relevant.rs"), ("other-handoff", "src/other.rs")] {
        store
            .register_presence(&request(
                agent_id,
                Uuid::new_v4(),
                PresenceRegistration { first_prompt: None },
            ))
            .expect("presence registers");
        store
            .finalize_handoff(&request(
                agent_id,
                Uuid::new_v4(),
                ExplicitHandoff {
                    status: HandoffStatus::Done,
                    summary: "Finished work.".into(),
                    files_changed: vec![path.into()],
                    tests_run: Vec::new(),
                    remaining_work: Vec::new(),
                    next_plan: None,
                },
            ))
            .expect("handoff finalizes");
    }
    for path in ["src/relevant.rs", "src/other.rs"] {
        store
            .declare_reservation(&request(
                "owner",
                Uuid::new_v4(),
                ReservationDeclaration { scopes: vec![stateful_core::ReservationScope::file(path)], action: "write_file".into(), purpose: "Coordinate state.".into() },
            ))
            .expect("reservation declares");
    }
    let delta = store
        .render_context(&request(
            "reader",
            Uuid::new_v4(),
            ContextRender { mode: RenderMode::Detailed, resource: Some("src/relevant.rs".into()) },
        ))
        .expect("filtered render succeeds")
        .response;
    assert!(delta.items.iter().any(|item| item.kind == CurrentItemKind::Agent));
    assert!(delta.items.iter().any(|item| item.kind == CurrentItemKind::Finalization));
    assert!(delta.items.iter().any(|item| item.kind == CurrentItemKind::Reservation));
    assert!(delta.items.iter().all(|item| item.resource == "src/relevant.rs"));
}

#[test]
fn heartbeat_and_identical_resource_refresh_do_not_churn_delivery_versions() {
    let mut store = Store::open_in_memory_with_clock(FixedClock::new(NOW)).expect("store opens");
    store
        .register_presence(&request(
            "owner",
            Uuid::new_v4(),
            PresenceRegistration { first_prompt: None },
        ))
        .expect("presence registers");
    let relation = PresenceResourceUpdate {
        relative_path: "src/steady.rs".into(),
        relation: PresenceResourceRelation::Planned,
    };
    store
        .update_presence_resource(&request("owner", Uuid::new_v4(), relation.clone()))
        .expect("semantic resource update succeeds");
    let delivery = store
        .render_context(&request(
            "reader",
            Uuid::new_v4(),
            ContextRender { mode: RenderMode::Brief, resource: None },
        ))
        .expect("initial render succeeds")
        .response;
    store
        .acknowledge_context(&request(
            "reader",
            Uuid::new_v4(),
            ContextAcknowledgement {
                delivery_id: delivery.delivery_id.expect("delivery id"),
                sequence: delivery.sequence.expect("sequence"),
                workspace_version: delivery.workspace_version,
            },
        ))
        .expect("acknowledgement succeeds");
    let version = store.workspace_version("workspace-1").expect("version loads");
    store
        .heartbeat_presence(&request("owner", Uuid::new_v4(), ()))
        .expect("heartbeat succeeds");
    assert_eq!(store.workspace_version("workspace-1").expect("version loads"), version);
    store
        .update_presence_resource(&request("owner", Uuid::new_v4(), relation))
        .expect("identical resource refresh succeeds");
    assert_eq!(store.workspace_version("workspace-1").expect("version loads"), version);
    let unchanged = store
        .render_context(&request(
            "reader",
            Uuid::new_v4(),
            ContextRender { mode: RenderMode::Brief, resource: None },
        ))
        .expect("render after no-op transitions succeeds")
        .response;
    assert!(!unchanged.changed);
    assert!(unchanged.delivery_id.is_none());
}

#[test]
fn reconnect_replays_pending_delivery_then_persists_the_ack_cursor() {
    let directory = TempDir::new().expect("temporary directory creates");
    let path = directory.path().join("stateful.sqlite");
    let clock = MutableClock::new(NOW);
    let first = {
        let store = Store::open_with_clock(&path, clock.clone()).expect("persistent store opens");
        store
            .declare_reservation(&request(
                "owner",
                Uuid::new_v4(),
                ReservationDeclaration { scopes: vec![stateful_core::ReservationScope::file("src/reconnect.rs")], action: "write_file".into(), purpose: "Reconnect state.".into() },
            ))
            .expect("state change succeeds");
        store
            .render_context(&request(
                "reader",
                Uuid::new_v4(),
                ContextRender { mode: RenderMode::Brief, resource: None },
            ))
            .expect("initial render succeeds")
            .response
    };
    let acknowledged_version = first.workspace_version;
    {
        let store = Store::open_with_clock(&path, clock.clone()).expect("store reopens");
        let replayed = store
            .render_context(&request(
                "reader",
                Uuid::new_v4(),
                ContextRender { mode: RenderMode::Brief, resource: None },
            ))
            .expect("reconnect render succeeds")
            .response;
        assert_eq!(replayed, first, "pending delivery survives reconnect exactly");
        store
            .acknowledge_context(&request(
                "reader",
                Uuid::new_v4(),
                ContextAcknowledgement {
                    delivery_id: replayed.delivery_id.expect("delivery id"),
                    sequence: replayed.sequence.expect("sequence"),
                    workspace_version: replayed.workspace_version,
                },
            ))
            .expect("replayed delivery acknowledges");
    }
    let store = Store::open_with_clock(&path, clock).expect("store reopens after acknowledgement");
    assert_eq!(store.context_cursor("workspace-1", "reader").expect("cursor loads"), acknowledged_version);
    assert!(
        !store
            .render_context(&request(
                "reader",
                Uuid::new_v4(),
                ContextRender { mode: RenderMode::Brief, resource: None },
            ))
            .expect("post-ack reconnect render succeeds")
            .response
            .changed,
    );
}

fn reservation_context_event(
    request_id: Uuid,
    ordinal: u32,
    reservation_id: &str,
    relative_path: &str,
) -> NewEvent {
    let mut data = EventData::new(reservation_id);
    data.data = serde_json::json!({"reservation": {
        "reservation_id": reservation_id,
        "agent_id": "owner",
        "workspace_id": "workspace-1",
        "scopes": [{"kind": "file", "path": relative_path}],
        "purpose": "Coordinate source changes.",
        "status": "active"
    }});
    NewEvent::new(
        request_id,
        ordinal,
        NOW,
        EventPayload::Reservation(ReservationEvent::Declared(data)),
    )
    .expect("reservation event builds")
}

#[test]
fn context_cursor_uses_event_sequence_across_legacy_audit_gaps() {
    let store = Store::open_in_memory_with_clock(FixedClock::new(NOW)).expect("store opens");
    let request_id = Uuid::new_v4();
    let owner = request("owner", request_id, ());
    let mut audit = EventData::new("legacy-audit");
    audit.data = serde_json::json!({"source_sequence": 42});
    store
        .execute_command(&owner, "test.audit_gap", |_| {
            Ok(CommandPlan {
                events: vec![
                    reservation_context_event(request_id, 0, "reservation-1", "src/one.rs"),
                    NewEvent::new(
                        request_id,
                        1,
                        NOW,
                        EventPayload::Migration(MigrationEvent::LegacyAuditImported(audit)),
                    )
                    .expect("audit event builds"),
                    reservation_context_event(request_id, 2, "reservation-2", "src/two.rs"),
                ],
                response: (),
                http_status: 200,
            })
        })
        .expect("events commit");

    let first = store
        .render_context(&request(
            "reader",
            Uuid::new_v4(),
            ContextRender { mode: RenderMode::Brief, resource: None },
        ))
        .expect("first context renders")
        .response;
    assert_eq!(first.workspace_version, 3);
    assert_eq!(first.items.len(), 2);
    store
        .acknowledge_context(&request(
            "reader",
            Uuid::new_v4(),
            ContextAcknowledgement {
                delivery_id: first.delivery_id.expect("delivery id"),
                sequence: first.sequence.expect("delivery sequence"),
                workspace_version: first.workspace_version,
            },
        ))
        .expect("context acknowledges");
    assert_eq!(store.context_cursor("workspace-1", "reader").expect("cursor loads"), 3);

    let later_request_id = Uuid::new_v4();
    let later_request = request("owner", later_request_id, ());
    store
        .execute_command(&later_request, "test.later_change", |_| {
            Ok(CommandPlan {
                events: vec![reservation_context_event(later_request_id, 0, "reservation-3", "src/three.rs")],
                response: (),
                http_status: 200,
            })
        })
        .expect("later event commits");
    let next = store
        .render_context(&request(
            "reader",
            Uuid::new_v4(),
            ContextRender { mode: RenderMode::Brief, resource: None },
        ))
        .expect("later context renders")
        .response;
    assert_eq!(next.workspace_version, 8);
    assert_eq!(
        next.items.iter().map(|item| item.resource.as_str()).collect::<Vec<_>>(),
        vec!["src/three.rs"],
        "the audit gap and already acknowledged events must not be redelivered",
    );
}

#[test]
fn pending_human_writes_render_while_reconciled_and_expired_writes_do_not() {
    let store = Store::open_in_memory_with_clock(FixedClock::new(NOW)).expect("store opens");
    let pending = store
        .record_human_observation(&request(
            "human",
            Uuid::new_v4(),
            HumanObservationInput {
                relative_path: "src/pending.rs".into(),
                kind: HumanObservationKind::Save,
                confidence: HumanObservationConfidence::High,
                source: "watcher".into(),
                summary: "pending human write".into(),
                observed_at: None,
            },
        ))
        .expect("pending observation records")
        .response;
    let reconciled = store
        .record_human_observation(&request(
            "human",
            Uuid::new_v4(),
            HumanObservationInput {
                relative_path: "src/reconciled.rs".into(),
                kind: HumanObservationKind::Save,
                confidence: HumanObservationConfidence::High,
                source: "watcher".into(),
                summary: "reconciled human write".into(),
                observed_at: None,
            },
        ))
        .expect("reconciled observation records")
        .response;
    let expired = store
        .record_human_observation(&request(
            "human",
            Uuid::new_v4(),
            HumanObservationInput {
                relative_path: "src/expired.rs".into(),
                kind: HumanObservationKind::Save,
                confidence: HumanObservationConfidence::High,
                source: "watcher".into(),
                summary: "expired human write".into(),
                observed_at: None,
            },
        ))
        .expect("expired observation records")
        .response;
    let mut reconciled = reconciled;
    reconciled.status = "reconciled".into();
    let mut expired = expired;
    expired.status = "expired".into();
    let resolution_request = request("maintenance", Uuid::new_v4(), ());
    store
        .execute_command(&resolution_request, "test.resolve_human_writes", |_| {
            let mut reconciled_data = EventData::new(&reconciled.observation_id);
            reconciled_data.data = serde_json::json!({"observation": reconciled});
            let mut expired_data = EventData::new(&expired.observation_id);
            expired_data.data = serde_json::json!({"observation": expired});
            Ok(CommandPlan {
                events: vec![
                    NewEvent::new(
                        resolution_request.request_id,
                        0,
                        NOW,
                        EventPayload::HumanObservation(HumanObservationEvent::Reconciled(reconciled_data)),
                    )
                    .expect("reconciled event builds"),
                    NewEvent::new(
                        resolution_request.request_id,
                        1,
                        NOW,
                        EventPayload::HumanObservation(HumanObservationEvent::Expired(expired_data)),
                    )
                    .expect("expired event builds"),
                ],
                response: (),
                http_status: 200,
            })
        })
        .expect("resolution events commit");

    let delta = store
        .render_context(&request(
            "reader",
            Uuid::new_v4(),
            ContextRender { mode: RenderMode::Brief, resource: None },
        ))
        .expect("context renders")
        .response;
    assert_eq!(
        delta.items.iter().filter(|item| item.kind == CurrentItemKind::Claim).map(|item| item.resource.as_str()).collect::<Vec<_>>(),
        vec![pending.relative_path.as_str()],
    );
}

fn migration_seed(aggregate_id: &str, entity_kind: &str, payload: serde_json::Value) -> EventData {
    let mut data = EventData::new(aggregate_id);
    let mut payload = payload.as_object().expect("seed payload is an object").clone();
    payload.insert("legacy_entity_kind".into(), entity_kind.into());
    payload.insert("legacy_primary_key".into(), aggregate_id.into());
    data.data = payload.into();
    data
}

#[test]
fn cursor_zero_context_includes_migrated_items_in_event_order_and_honors_filters() {
    let store = Store::open_in_memory_with_clock(FixedClock::new(NOW)).expect("store opens");
    let request_id = Uuid::new_v4();
    let owner = request("agent-peer", request_id, ());
    store
        .execute_command(&owner, "test.migration_context", |_| {
            Ok(CommandPlan {
                events: vec![
                    NewEvent::new(
                        request_id,
                        0,
                        NOW,
                        EventPayload::Migration(MigrationEvent::PresenceSnapshotSeeded(migration_seed(
                            "agent-peer",
                            "presence",
                            serde_json::json!({
                                "agent_id": "agent-peer",
                                "phase": "working",
                                "expires_at": "2026-07-16T12:00:00Z"
                            }),
                        ))),
                    )
                    .expect("presence seed builds"),
                    NewEvent::new(
                        request_id,
                        1,
                        NOW,
                        EventPayload::Migration(MigrationEvent::ReservationSnapshotSeeded(migration_seed(
                            "reservation-seed",
                            "reservation",
                            serde_json::json!({
                                "reservation_id": "reservation-seed",
                                "agent_id": "agent-peer",
                                "workspace_id": "workspace-1",
                                "scopes": [{"kind": "file", "path": "src/reserved.rs"}],
                                "action": "write_file",
                                "purpose": "Coordinate migrated work.",
                                "status": "active"
                            }),
                        ))),
                    )
                    .expect("reservation seed builds"),
                    NewEvent::new(
                        request_id,
                        2,
                        NOW,
                        EventPayload::Migration(MigrationEvent::ClaimSnapshotSeeded(migration_seed(
                            "claim-seed",
                            "claim",
                            serde_json::json!({
                                "claim_id": "claim-seed",
                                "reservation_id": "reservation-seed",
                                "agent_id": "agent-peer",
                                "workspace_id": "workspace-1",
                                "relative_path": "src/claimed.rs",
                                "action": "write_file",
                                "status": "active"
                            }),
                        ))),
                    )
                    .expect("claim seed builds"),
                    NewEvent::new(
                        request_id,
                        3,
                        NOW,
                        EventPayload::Migration(MigrationEvent::WriteFenceSnapshotSeeded(migration_seed(
                            "fence-seed",
                            "write_fence",
                            serde_json::json!({
                                "fence_id": "fence-seed",
                                "agent_id": "agent-peer",
                                "workspace_id": "workspace-1",
                                "relative_path": "src/fenced.rs",
                                "action": "write_file",
                                "status": "active"
                            }),
                        ))),
                    )
                    .expect("fence seed builds"),
                    NewEvent::new(
                        request_id,
                        4,
                        NOW,
                        EventPayload::Migration(MigrationEvent::HumanObservationSnapshotSeeded(migration_seed(
                            "human-seed",
                            "human_observation",
                            serde_json::json!({
                                "observation_id": "human-seed",
                                "workspace_id": "workspace-1",
                                "relative_path": "src/human.rs",
                                "kind": "save",
                                "confidence": "high",
                                "status": "pending"
                            }),
                        ))),
                    )
                    .expect("human seed builds"),
                    NewEvent::new(
                        request_id,
                        5,
                        NOW,
                        EventPayload::Migration(MigrationEvent::LegacyHandoffSnapshotSeeded(migration_seed(
                            "legacy-handoff",
                            "handoff",
                            serde_json::json!({}),
                        ))),
                    )
                    .expect("handoff seed builds"),
                ],
                response: (),
                http_status: 200,
            })
        })
        .expect("migration seeds commit");

    let all = store
        .render_context(&request(
            "reader",
            Uuid::new_v4(),
            ContextRender { mode: RenderMode::Brief, resource: None },
        ))
        .expect("cursor-zero context renders")
        .response;
    assert_eq!(
        all.items.iter().map(|item| item.resource.as_str()).collect::<Vec<_>>(),
        vec!["presence", "src/reserved.rs", "src/claimed.rs", "src/fenced.rs", "src/human.rs", "handoff"],
    );
    store
        .acknowledge_context(&request(
            "reader",
            Uuid::new_v4(),
            ContextAcknowledgement {
                delivery_id: all.delivery_id.expect("delivery id"),
                sequence: all.sequence.expect("delivery sequence"),
                workspace_version: all.workspace_version,
            },
        ))
        .expect("context acknowledges");
    assert_eq!(
        store.context_cursor("workspace-1", "reader").expect("cursor loads"),
        all.workspace_version,
    );

    let filtered = store
        .render_context(&request(
            "filtered-reader",
            Uuid::new_v4(),
            ContextRender { mode: RenderMode::Brief, resource: Some("src/fenced.rs".into()) },
        ))
        .expect("filtered cursor-zero context renders")
        .response;
    assert_eq!(
        filtered.items.iter().map(|item| item.resource.as_str()).collect::<Vec<_>>(),
        vec!["src/fenced.rs"],
    );
}

#[test]
fn only_high_confidence_write_observations_are_hard_context_blocks() {
    let store = Store::open_in_memory_with_clock(FixedClock::new(NOW)).expect("store opens");
    for (path, kind, confidence) in [
        ("src/low-write.rs", HumanObservationKind::Save, HumanObservationConfidence::Low),
        ("src/presence.rs", HumanObservationKind::Presence, HumanObservationConfidence::High),
        ("src/dirty.rs", HumanObservationKind::Dirty, HumanObservationConfidence::High),
        ("src/high-write.rs", HumanObservationKind::Change, HumanObservationConfidence::High),
    ] {
        store
            .record_human_observation(&request(
                "human",
                Uuid::new_v4(),
                HumanObservationInput {
                    relative_path: path.into(),
                    kind,
                    confidence,
                    source: "watcher".into(),
                    summary: "observation".into(),
                    observed_at: None,
                },
            ))
            .expect("observation records");
    }

    let delta = store
        .render_context(&request(
            "reader",
            Uuid::new_v4(),
            ContextRender { mode: RenderMode::Brief, resource: None },
        ))
        .expect("context renders")
        .response;
    for path in ["src/low-write.rs", "src/presence.rs", "src/dirty.rs"] {
        let item = delta.items.iter().find(|item| item.resource == path).expect("advisory item");
        assert_eq!(item.severity, CurrentSeverity::Warn);
        assert!(item.next_action.is_none());
    }
    let hard_block = delta
        .items
        .iter()
        .find(|item| item.resource == "src/high-write.rs")
        .expect("high-confidence write item");
    assert_eq!(hard_block.severity, CurrentSeverity::Block);
    assert!(hard_block.next_action.is_some());
}

#[test]
fn active_claim_is_advisory_in_default_context() {
    let store = Store::open_in_memory_with_clock(FixedClock::new(NOW)).expect("store opens");
    let reservation = store
        .declare_reservation(&request(
            "agent-1",
            Uuid::new_v4(),
            ReservationDeclaration {
                scopes: vec![stateful_core::ReservationScope::file("src/claimed.rs")],
                action: "write_file".into(),
                purpose: "Coordinate claimed work.".into(),
            },
        ))
        .expect("reservation declares")
        .response;
    store
        .acquire_claim(&request(
            "agent-1",
            Uuid::new_v4(),
            ClaimAcquire {
                reservation_id: reservation.reservation_id,
                paths: vec![ClaimPath { relative_path: "src/claimed.rs".into(), observation: None }],
            },
        ))
        .expect("claim acquires");

    let delta = store
        .render_context(&request(
            "agent-2",
            Uuid::new_v4(),
            ContextRender { mode: RenderMode::Brief, resource: Some("src/claimed.rs".into()) },
        ))
        .expect("context renders")
        .response;
    let claim = delta
        .items
        .iter()
        .find(|item| item.summary.contains("active claim"))
        .expect("active claim item");
    assert_eq!(claim.severity, CurrentSeverity::Warn);
    assert_eq!(
        claim.next_action.as_deref(),
        Some("Coordinate with the claim owner before editing this resource."),
    );
}
