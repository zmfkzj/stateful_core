use serde_json::json;
use stateful_core::{
    EventData, EventPayload, MigrationEvent, NewEvent, PresenceEvent, StoredEvent,
    migration_seed_event_id,
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

    let error = serde_json::from_value::<StoredEvent>(invalid)
        .expect_err("direct event deserialization must reject mismatched metadata");
    assert!(error.to_string().contains("event_metadata_mismatch"));
}

#[test]
fn migration_seed_identifiers_use_a_fixed_dedicated_namespace() {
    let first = migration_seed_event_id("reservation", "legacy-42").expect("valid seed");
    let repeated = migration_seed_event_id("reservation", "legacy-42").expect("valid seed");
    let distinct = migration_seed_event_id("claim", "legacy-42").expect("valid seed");

    assert_eq!(first, repeated);
    assert_ne!(first, distinct);
}

#[test]
fn migration_snapshot_seeds_require_the_fixed_legacy_entity_identifier() {
    let mut data = EventData::new("claim-active");
    data.data = json!({
        "legacy_entity_kind": "claim",
        "legacy_primary_key": "claim-active",
    });
    let event = NewEvent::new(
        Uuid::parse_str("8d5ddf45-9ce3-44ac-953e-3b776cd1783d").expect("valid UUID"),
        3,
        OffsetDateTime::parse(
            "2026-05-31T12:00:00Z",
            &time::format_description::well_known::Rfc3339,
        )
        .expect("valid RFC3339 timestamp"),
        EventPayload::Migration(MigrationEvent::ClaimSnapshotSeeded(data)),
    )
    .expect("seed event is valid");
    assert_eq!(
        event.event_id,
        migration_seed_event_id("claim", "claim-active").expect("seed ID should derive")
    );

    let wrong_identifier = json!({
        "event_seq": 1,
        "event_id": Uuid::new_v4(),
        "request_id": event.request_id,
        "event_ordinal": event.event_ordinal,
        "aggregate_kind": event.aggregate_kind,
        "aggregate_id": event.aggregate_id,
        "event_type": event.event_type,
        "observed_at": event.observed_at.format(&time::format_description::well_known::Rfc3339).expect("serializable timestamp"),
        "affects_context": event.affects_context,
        "payload": event.payload,
    });
    assert!(
        serde_json::from_value::<StoredEvent>(wrong_identifier).is_err(),
        "wrong seed ID must be rejected"
    );
}

#[test]
fn non_snapshot_migration_events_keep_request_ordinal_type_identifiers() {
    let request_id = Uuid::parse_str("8d5ddf45-9ce3-44ac-953e-3b776cd1783d").expect("valid UUID");
    let event = NewEvent::new(
        request_id,
        3,
        OffsetDateTime::parse(
            "2026-05-31T12:00:00Z",
            &time::format_description::well_known::Rfc3339,
        )
        .expect("valid RFC3339 timestamp"),
        EventPayload::Migration(MigrationEvent::Started(EventData::new(
            "stateful.v2.event-journal",
        ))),
    )
    .expect("migration lifecycle event is valid");

    assert_eq!(
        event.event_id,
        Uuid::new_v5(&request_id, b"3:migration.started")
    );
}

#[test]
fn migration_snapshot_seed_rejects_aggregate_key_mismatch() {
    let mut data = EventData::new("claim-other");
    data.data = json!({
        "legacy_entity_kind": "claim",
        "legacy_primary_key": "claim-active",
    });
    assert!(
        NewEvent::new(
            Uuid::parse_str("8d5ddf45-9ce3-44ac-953e-3b776cd1783d").expect("valid UUID"),
            3,
            OffsetDateTime::parse(
                "2026-05-31T12:00:00Z",
                &time::format_description::well_known::Rfc3339
            )
            .expect("valid timestamp"),
            EventPayload::Migration(MigrationEvent::ClaimSnapshotSeeded(data)),
        )
        .is_err()
    );
}
