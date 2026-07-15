use crate::V2Error;
use serde::{Deserialize, Deserializer, Serialize, de::Error as DeError};
use serde_json::Value;
use time::OffsetDateTime;
use uuid::Uuid;

pub const LEGACY_MIGRATION_NAMESPACE: Uuid =
    Uuid::from_u128(0x3e4ea7f1_456b_5d6a_9e94_7c84da8a1fc9);

pub fn migration_seed_event_id(
    legacy_entity_kind: impl AsRef<str>,
    legacy_primary_key: impl AsRef<str>,
) -> Result<Uuid, V2Error> {
    let legacy_entity_kind = legacy_entity_kind.as_ref();
    let legacy_primary_key = legacy_primary_key.as_ref();
    if legacy_entity_kind.trim().is_empty() || legacy_primary_key.trim().is_empty() {
        return Err(V2Error::new(
            "invalid_migration_seed",
            "legacy entity kind and primary key must not be empty.",
        ));
    }
    let name = format!(
        "{}:{legacy_entity_kind}{}:{legacy_primary_key}",
        legacy_entity_kind.len(),
        legacy_primary_key.len()
    );
    Ok(Uuid::new_v5(&LEGACY_MIGRATION_NAMESPACE, name.as_bytes()))
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EventData {
    pub aggregate_id: String,
    #[serde(default)]
    pub repeated: bool,
    #[serde(default)]
    pub data: Value,
}

impl EventData {
    pub fn new(aggregate_id: impl Into<String>) -> Self {
        Self {
            aggregate_id: aggregate_id.into(),
            repeated: false,
            data: Value::Null,
        }
    }

    fn validate(&self) -> Result<(), V2Error> {
        if self.aggregate_id.trim().is_empty() {
            return Err(V2Error::new(
                "invalid_aggregate_id",
                "event aggregate_id must not be empty.",
            ));
        }
        Ok(())
    }
}

macro_rules! event_family {
    ($name:ident { $($variant:ident),+ $(,)? }) => {
        #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
        #[serde(tag = "kind", content = "data", rename_all = "snake_case")]
        pub enum $name {
            $( $variant(EventData), )+
        }

        impl $name {
            fn parts(&self) -> (&'static str, &EventData) {
                match self {
                    $( Self::$variant(data) => (stringify!($variant), data), )+
                }
            }
        }
    };
}

event_family!(MigrationEvent {
    Started,
    LegacyAuditImported,
    PresenceSnapshotSeeded,
    ReservationSnapshotSeeded,
    ClaimSnapshotSeeded,
    WaitSnapshotSeeded,
    WriteFenceSnapshotSeeded,
    HumanObservationSnapshotSeeded,
    LegacyHandoffSnapshotSeeded,
    DeliverySnapshotSeeded,
    Validated,
    Completed,
});

event_family!(PresenceEvent {
    Registered,
    Heartbeat,
    GoalUpdated,
    PhaseUpdated,
    PlanUpdated,
    ResourcesUpdated,
    ToolStarted,
    ToolCompleted,
    Finalized,
    Expired,
});

event_family!(ReservationEvent {
    Declared,
    Refreshed,
    Released,
    Expired,
});

event_family!(ClaimEvent {
    Acquired,
    ObservationRefreshed,
    Released,
    Expired,
});

event_family!(WaitEvent {
    Requested,
    BecameClaimable,
    Claimed,
    Cancelled,
    Expired,
});

event_family!(WriteFenceEvent {
    Acquired,
    ConflictObserved,
    Released,
    Expired,
});

event_family!(ReadObservationEvent {
    Started,
    Stabilized,
    Unstable,
    Aborted,
    Invalidated,
    Expired,
});

event_family!(WriteIntentEvent {
    Started,
    Committed,
    Failed,
    OutcomeUnknown,
    Reconciled,
});

event_family!(HumanObservationEvent {
    Observed,
    Reconciled,
    Expired,
});

event_family!(HumanAcknowledgementEvent {
    Recorded,
});

event_family!(HandoffEvent {
    Finalized,
    Expired,
});

event_family!(AuthorizationEvent {
    Allowed,
    Warned,
    Denied,
    OverrideGranted,
});

event_family!(ContextEvent {
    Rendered,
    DeliveryCreated,
    DeliveryAcknowledged,
    DeliverySuperseded,
});

event_family!(NotificationEvent {
    Created,
    Delivered,
    Expired,
    Coalesced,
});

event_family!(RecoveryEvent {
    Queued,
    Attempted,
    Delivered,
    Failed,
});

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "family", content = "event", rename_all = "snake_case")]
pub enum EventPayload {
    Migration(MigrationEvent),
    Presence(PresenceEvent),
    Reservation(ReservationEvent),
    Claim(ClaimEvent),
    Wait(WaitEvent),
    WriteFence(WriteFenceEvent),
    ReadObservation(ReadObservationEvent),
    WriteIntent(WriteIntentEvent),
    HumanObservation(HumanObservationEvent),
    HumanAcknowledgement(HumanAcknowledgementEvent),
    Handoff(HandoffEvent),
    Authorization(AuthorizationEvent),
    Context(ContextEvent),
    Notification(NotificationEvent),
    Recovery(RecoveryEvent),
}

impl EventPayload {
    fn metadata(&self) -> EventMetadata<'_> {
        let (aggregate_kind, variant) = match self {
            Self::Migration(event) => ("migration", event.parts()),
            Self::Presence(event) => ("presence", event.parts()),
            Self::Reservation(event) => ("reservation", event.parts()),
            Self::Claim(event) => ("claim", event.parts()),
            Self::Wait(event) => ("wait", event.parts()),
            Self::WriteFence(event) => ("write_fence", event.parts()),
            Self::ReadObservation(event) => ("read_observation", event.parts()),
            Self::WriteIntent(event) => ("write_intent", event.parts()),
            Self::HumanObservation(event) => ("human_observation", event.parts()),
            Self::HumanAcknowledgement(event) => ("human_acknowledgement", event.parts()),
            Self::Handoff(event) => ("handoff", event.parts()),
            Self::Authorization(event) => ("authorization", event.parts()),
            Self::Context(event) => ("context", event.parts()),
            Self::Notification(event) => ("notification", event.parts()),
            Self::Recovery(event) => ("recovery", event.parts()),
        };
        let (variant, data) = variant;
        let event_name = snake_case(variant);
        EventMetadata {
            aggregate_kind,
            aggregate_id: &data.aggregate_id,
            event_type: format!("{aggregate_kind}.{event_name}"),
            affects_context: affects_context(aggregate_kind, &event_name, data),
            data,
        }
    }
}

struct EventMetadata<'a> {
    aggregate_kind: &'static str,
    aggregate_id: &'a str,
    event_type: String,
    affects_context: bool,
    data: &'a EventData,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NewEvent {
    pub event_id: Uuid,
    pub request_id: Uuid,
    pub event_ordinal: u32,
    pub aggregate_kind: String,
    pub aggregate_id: String,
    pub event_type: String,
    #[serde(with = "time::serde::rfc3339")]
    pub observed_at: OffsetDateTime,
    pub affects_context: bool,
    pub payload: EventPayload,
}

impl NewEvent {
    pub fn new(
        request_id: Uuid,
        event_ordinal: u32,
        observed_at: OffsetDateTime,
        payload: EventPayload,
    ) -> Result<Self, V2Error> {
        if request_id.is_nil() {
            return Err(V2Error::new(
                "invalid_request_id",
                "request_id must be a non-nil UUID.",
            ));
        }
        let metadata = payload.metadata();
        metadata.data.validate()?;
        let event_id = migration_snapshot_event_id(&payload)?.unwrap_or_else(|| {
            Uuid::new_v5(
                &request_id,
                format!("{event_ordinal}:{}", metadata.event_type).as_bytes(),
            )
        });
        Ok(Self {
            event_id,
            request_id,
            event_ordinal,
            aggregate_kind: metadata.aggregate_kind.into(),
            aggregate_id: metadata.aggregate_id.into(),
            event_type: metadata.event_type,
            observed_at,
            affects_context: metadata.affects_context,
            payload,
        })
    }

    pub fn into_stored(self, event_seq: u64) -> Result<StoredEvent, V2Error> {
        StoredEvent::new(event_seq, self)
    }
}

fn migration_snapshot_event_id(payload: &EventPayload) -> Result<Option<Uuid>, V2Error> {
    let (expected_kind, data) = match payload {
        EventPayload::Migration(MigrationEvent::PresenceSnapshotSeeded(data)) => ("presence", data),
        EventPayload::Migration(MigrationEvent::ReservationSnapshotSeeded(data)) => ("reservation", data),
        EventPayload::Migration(MigrationEvent::ClaimSnapshotSeeded(data)) => ("claim", data),
        EventPayload::Migration(MigrationEvent::WaitSnapshotSeeded(data)) => ("wait", data),
        EventPayload::Migration(MigrationEvent::WriteFenceSnapshotSeeded(data)) => ("write_fence", data),
        EventPayload::Migration(MigrationEvent::HumanObservationSnapshotSeeded(data)) => ("human_observation", data),
        EventPayload::Migration(MigrationEvent::LegacyHandoffSnapshotSeeded(data)) => ("handoff", data),
        EventPayload::Migration(MigrationEvent::DeliverySnapshotSeeded(data)) => ("delivery", data),
        _ => return Ok(None),
    };
    let object = data.data.as_object().ok_or_else(|| V2Error::new(
        "invalid_migration_seed",
        "migration snapshot seed data must include its legacy entity identity.",
    ))?;
    let entity_kind = object
        .get("legacy_entity_kind")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| V2Error::new(
            "invalid_migration_seed",
            "migration snapshot seed data must include legacy_entity_kind.",
        ))?;
    if entity_kind != expected_kind {
        return Err(V2Error::new(
            "invalid_migration_seed",
            "migration snapshot seed kind must match its event variant.",
        ));
    }
    let primary_key = object
        .get("legacy_primary_key")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| V2Error::new(
            "invalid_migration_seed",
            "migration snapshot seed data must include legacy_primary_key.",
        ))?;
    if data.aggregate_id != primary_key {
        return Err(V2Error::new(
            "invalid_migration_seed",
            "migration snapshot seed aggregate_id must match legacy_primary_key.",
        ));
    }
    migration_seed_event_id(entity_kind, primary_key).map(Some)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct StoredEvent {
    event_seq: u64,
    event_id: Uuid,
    request_id: Uuid,
    event_ordinal: u32,
    aggregate_kind: String,
    aggregate_id: String,
    event_type: String,
    #[serde(with = "time::serde::rfc3339")]
    observed_at: OffsetDateTime,
    affects_context: bool,
    payload: EventPayload,
}

#[derive(Deserialize)]
struct StoredEventWire {
    event_seq: u64,
    event_id: Uuid,
    request_id: Uuid,
    event_ordinal: u32,
    aggregate_kind: String,
    aggregate_id: String,
    event_type: String,
    #[serde(with = "time::serde::rfc3339")]
    observed_at: OffsetDateTime,
    affects_context: bool,
    payload: EventPayload,
}

impl<'de> Deserialize<'de> for StoredEvent {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let wire = StoredEventWire::deserialize(deserializer)?;
        let event = Self {
            event_seq: wire.event_seq,
            event_id: wire.event_id,
            request_id: wire.request_id,
            event_ordinal: wire.event_ordinal,
            aggregate_kind: wire.aggregate_kind,
            aggregate_id: wire.aggregate_id,
            event_type: wire.event_type,
            observed_at: wire.observed_at,
            affects_context: wire.affects_context,
            payload: wire.payload,
        };
        event.validate().map_err(D::Error::custom)?;
        Ok(event)
    }
}

impl StoredEvent {
    pub fn new(event_seq: u64, event: NewEvent) -> Result<Self, V2Error> {
        if event_seq == 0 {
            return Err(V2Error::new(
                "invalid_event_seq",
                "event_seq must be a positive workspace-global sequence.",
            ));
        }
        let stored = Self {
            event_seq,
            event_id: event.event_id,
            request_id: event.request_id,
            event_ordinal: event.event_ordinal,
            aggregate_kind: event.aggregate_kind,
            aggregate_id: event.aggregate_id,
            event_type: event.event_type,
            observed_at: event.observed_at,
            affects_context: event.affects_context,
            payload: event.payload,
        };
        stored.validate()?;
        Ok(stored)
    }

    pub const fn event_seq(&self) -> u64 {
        self.event_seq
    }

    pub const fn event_id(&self) -> Uuid {
        self.event_id
    }

    pub const fn request_id(&self) -> Uuid {
        self.request_id
    }

    pub const fn event_ordinal(&self) -> u32 {
        self.event_ordinal
    }

    pub fn aggregate_kind(&self) -> &str {
        &self.aggregate_kind
    }

    pub fn aggregate_id(&self) -> &str {
        &self.aggregate_id
    }

    pub fn event_type(&self) -> &str {
        &self.event_type
    }

    pub const fn observed_at(&self) -> OffsetDateTime {
        self.observed_at
    }

    pub const fn affects_context(&self) -> bool {
        self.affects_context
    }

    pub fn payload(&self) -> &EventPayload {
        &self.payload
    }

    pub fn from_json(input: impl AsRef<str>) -> Result<Self, V2Error> {
        let event: Self = serde_json::from_str(input.as_ref())
            .map_err(|error| V2Error::new("invalid_event", error.to_string()))?;
        event.validate()?;
        Ok(event)
    }

    pub fn validate(&self) -> Result<(), V2Error> {
        if self.event_seq == 0 {
            return Err(V2Error::new(
                "invalid_event_seq",
                "event_seq must be a positive workspace-global sequence.",
            ));
        }
        let derived = NewEvent::new(
            self.request_id,
            self.event_ordinal,
            self.observed_at,
            self.payload.clone(),
        )?;
        if self.event_id != derived.event_id
            || self.aggregate_kind != derived.aggregate_kind
            || self.aggregate_id != derived.aggregate_id
            || self.event_type != derived.event_type
            || self.affects_context != derived.affects_context
        {
            return Err(V2Error::new(
                "event_metadata_mismatch",
                "Stored event metadata must match its event payload.",
            ));
        }
        Ok(())
    }
}

fn affects_context(family: &str, variant: &str, data: &EventData) -> bool {
    !matches!(
        (family, variant),
        ("presence", "heartbeat")
            | ("authorization", "allowed")
            | ("context", "rendered")
            | ("context", "delivery_created")
            | ("context", "delivery_acknowledged")
            | ("context", "delivery_superseded")
            | ("notification", "delivered")
    ) && !(data.repeated
        && matches!(
            (family, variant),
            ("presence", "resources_updated") | ("read_observation", "stabilized")
        ))
}

fn snake_case(value: &str) -> String {
    let mut output = String::new();
    for (index, character) in value.chars().enumerate() {
        if character.is_ascii_uppercase() {
            if index != 0 {
                output.push('_');
            }
            output.push(character.to_ascii_lowercase());
        } else {
            output.push(character);
        }
    }
    output
}
