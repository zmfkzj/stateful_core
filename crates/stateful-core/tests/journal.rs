use stateful_core::{
    EventData, EventPayload, NewEvent, PresenceEvent, StoredEvent, migration_seed_event_id,
};
use time::OffsetDateTime;
use uuid::Uuid;

fn registered_payload() -> EventPayload {
    EventPayload::Presence(PresenceEvent::Registered(EventData::new("agent-7")))
}

#[test]
fn event_constructor_derives_kind_aggregate_and_context_effect() {
    let event = NewEvent::new(
        Uuid::parse_str("8d5ddf45-9ce3-44ac-953e-3b776cd1783d").expect("valid UUID"),
        3,
        OffsetDateTime::parse(
            "2026-05-31T12:00:00Z",
            &time::format_description::well_known::Rfc3339,
        )
        .expect("valid RFC3339 timestamp"),
        registered_payload(),
    )
    .expect("registered event is valid");

    assert_eq!(event.aggregate_kind, "presence");
    assert_eq!(event.aggregate_id, "agent-7");
    assert_eq!(event.event_type, "presence.registered");
    assert!(event.affects_context);
    assert_eq!(
        event.event_id,
        Uuid::new_v5(
            &Uuid::parse_str("8d5ddf45-9ce3-44ac-953e-3b776cd1783d").expect("valid UUID"),
            b"3:presence.registered",
        )
    );
}

#[test]
fn event_payload_rejects_kind_payload_mismatch() {
    let event = NewEvent::new(
        Uuid::parse_str("8d5ddf45-9ce3-44ac-953e-3b776cd1783d").expect("valid UUID"),
        0,
        OffsetDateTime::parse(
            "2026-05-31T12:00:00Z",
            &time::format_description::well_known::Rfc3339,
        )
        .expect("valid RFC3339 timestamp"),
        registered_payload(),
    )
    .expect("registered event is valid");
    let invalid = serde_json::json!({
        "event_seq": 1,
        "event_id": event.event_id,
        "request_id": event.request_id,
        "event_ordinal": event.event_ordinal,
        "aggregate_kind": "reservation",
        "aggregate_id": event.aggregate_id,
        "event_type": event.event_type,
        "observed_at": event.observed_at.format(&time::format_description::well_known::Rfc3339).expect("serializable timestamp"),
        "affects_context": event.affects_context,
        "payload": event.payload,
    });

    let error = StoredEvent::from_json(invalid.to_string()).expect_err("mismatch must fail");
    assert_eq!(error.code, "event_metadata_mismatch");
}

#[test]
fn migration_seed_identifiers_use_a_fixed_dedicated_namespace() {
    let first = migration_seed_event_id("reservation", "legacy-42").expect("valid seed");
    let repeated = migration_seed_event_id("reservation", "legacy-42").expect("valid seed");
    let distinct = migration_seed_event_id("claim", "legacy-42").expect("valid seed");

    assert_eq!(first, repeated);
    assert_ne!(first, distinct);
}
