use stateful_core::{
    ActorType, AgentIdentity, ContextDelta, RenderMode, RequestEnvelope, SourceKind, SourceRef,
    WorkspaceIdentity,
};
use stateful_store::{
    Clock, ContextAcknowledgement, ContextRender, FixedClock, ReservationDeclaration, ReservationRelease,
    Store,
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

#[test]
fn render_redelivers_until_matching_cumulative_ack() {
    let store = Store::open_in_memory_with_clock(FixedClock::new(NOW)).expect("store opens");
    store
        .declare_reservation(&request(
            "agent-1",
            Uuid::new_v4(),
            ReservationDeclaration {
                relative_path: "src/lib.rs".into(),
                action: "write_file".into(),
                purpose: "Update the library.".into(),
            },
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
                ReservationDeclaration {
                    relative_path: relative_path.into(),
                    action: "write_file".into(),
                    purpose: "Update source.".into(),
                },
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
    assert_eq!(store.context_cursor("workspace-1", "agent-2").expect("cursor loads"), 1);
    assert_eq!(
        store
            .pending_context_deliveries("agent-2", "workspace-1")
            .expect("newer delivery remains")
            .iter()
            .map(|delivery| delivery.workspace_version)
            .collect::<Vec<_>>(),
        vec![2],
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
    assert_eq!(store.context_cursor("workspace-1", "agent-2").expect("cursor loads"), 2);
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
            ReservationDeclaration {
                relative_path: "src/one.rs".into(),
                action: "write_file".into(),
                purpose: "First change.".into(),
            },
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
            ReservationDeclaration {
                relative_path: "src/two.rs".into(),
                action: "write_file".into(),
                purpose: "Second change.".into(),
            },
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
    assert_eq!(second.workspace_version, first.workspace_version + 1);
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
            ReservationDeclaration {
                relative_path: "src/transient.rs".into(),
                action: "write_file".into(),
                purpose: "Temporary work.".into(),
            },
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
            ReservationDeclaration {
                relative_path: "src/ack.rs".into(),
                action: "write_file".into(),
                purpose: "Ack test.".into(),
            },
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
            ReservationDeclaration {
                relative_path: "src/expiry.rs".into(),
                action: "write_file".into(),
                purpose: "Expiry test.".into(),
            },
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
                ReservationDeclaration {
                    relative_path: format!("src/{index}.rs"),
                    action: "write_file".into(),
                    purpose: "Overflow test.".into(),
                },
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
        "Context delivery queue: 21 unacknowledged deliveries; this is version 21.\n",
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
}
