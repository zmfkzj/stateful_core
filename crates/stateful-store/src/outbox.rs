use crate::{
    CommandOutcome, CommandPlan, CurrentAggregate, Store, StoreResult,
    notifications::{DeliveryAttempt, DeliveryRecord},
    reservations::timestamp,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use stateful_core::{EventData, EventPayload, NewEvent, RecoveryEvent, RequestEnvelope};
use time::OffsetDateTime;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SyncStatus {
    Pending,
    Synced,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OutboxEntry {
    pub outbox_id: String,
    pub sequence: u64,
    pub event_type: String,
    pub payload: Value,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OutboxRecord {
    pub outbox_id: String,
    pub workspace_id: String,
    pub sequence: u64,
    pub event_type: String,
    pub payload: Value,
    pub sync_status: SyncStatus,
    pub attempts: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OutboxDelivery {
    pub outbox_id: String,
    pub outcome: DeliveryAttempt,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl Store {
    pub fn enqueue_outbox(
        &self,
        request: &RequestEnvelope<OutboxEntry>,
    ) -> StoreResult<CommandOutcome<OutboxRecord>> {
        let now = self.clock.now();
        let entry = request.payload.clone();
        self.execute_command(request, "outbox.enqueue", |reader| {
            if let Some(current) = reader
                .aggregate_records(CurrentAggregate::Delivery, &request.workspace.workspace_id)?
                .into_iter()
                .find(|record| record.aggregate_id == outbox_delivery_id(&entry.outbox_id))
            {
                let existing = current
                    .payload
                    .get("outbox")
                    .cloned()
                    .ok_or(crate::StoreError::ReservationRequestNotFound)?;
                return Ok(CommandPlan {
                    events: Vec::new(),
                    response: serde_json::from_value(existing)?,
                    http_status: 200,
                });
            }
            let record = OutboxRecord {
                outbox_id: entry.outbox_id.clone(),
                workspace_id: request.workspace.workspace_id.clone(),
                sequence: entry.sequence,
                event_type: entry.event_type.clone(),
                payload: entry.payload.clone(),
                sync_status: SyncStatus::Pending,
                attempts: 0,
                last_error: None,
            };
            let delivery = DeliveryRecord {
                delivery_id: outbox_delivery_id(&entry.outbox_id),
                notification_id: entry.outbox_id.clone(),
                workspace_id: request.workspace.workspace_id.clone(),
                status: "queued".into(),
                attempts: 0,
                last_error: None,
                retry_at: None,
                delivered_at: None,
                origin_event_seq: 0,
            };
            Ok(CommandPlan {
                events: vec![outbox_event(
                    request,
                    0,
                    now,
                    RecoveryEvent::Queued,
                    &delivery,
                    &record,
                )?],
                response: record,
                http_status: 200,
            })
        })
    }

    pub fn record_outbox_delivery(
        &self,
        request: &RequestEnvelope<OutboxDelivery>,
    ) -> StoreResult<CommandOutcome<OutboxRecord>> {
        let now = self.clock.now();
        let input = request.payload.clone();
        self.execute_command(request, "outbox.delivery", |reader| {
            let current = reader
                .aggregate_records(CurrentAggregate::Delivery, &request.workspace.workspace_id)?
                .into_iter()
                .find(|record| record.aggregate_id == outbox_delivery_id(&input.outbox_id))
                .ok_or(crate::StoreError::ReservationRequestNotFound)?;
            let outbox = current
                .payload
                .get("outbox")
                .cloned()
                .ok_or(crate::StoreError::ReservationRequestNotFound)?;
            let mut record: OutboxRecord = serde_json::from_value(outbox)?;
            let mut delivery: DeliveryRecord = serde_json::from_value(current.payload.clone())?;
            if delivery.status == "delivered" {
                return Ok(CommandPlan {
                    events: Vec::new(),
                    response: record,
                    http_status: 200,
                });
            }
            let (variant, status): (fn(EventData) -> RecoveryEvent, &str) = match input.outcome {
                DeliveryAttempt::Attempted => (RecoveryEvent::Attempted, "attempted"),
                DeliveryAttempt::Delivered => (RecoveryEvent::Delivered, "delivered"),
                DeliveryAttempt::Failed => (RecoveryEvent::Failed, "failed"),
            };
            if delivery.status == status && delivery.last_error == input.error {
                return Ok(CommandPlan {
                    events: Vec::new(),
                    response: record,
                    http_status: 200,
                });
            }
            delivery.status = status.into();
            delivery.attempts += 1;
            delivery.last_error = input.error;
            if matches!(input.outcome, DeliveryAttempt::Delivered) {
                delivery.delivered_at = Some(timestamp(now)?);
                record.sync_status = SyncStatus::Synced;
            }
            record.attempts = delivery.attempts;
            record.last_error = delivery.last_error.clone();
            Ok(CommandPlan {
                events: vec![outbox_event(request, 0, now, variant, &delivery, &record)?],
                response: record,
                http_status: 200,
            })
        })
    }

    pub fn outbox(&self, workspace_id: &str, outbox_id: &str) -> StoreResult<Option<OutboxRecord>> {
        let Some(record) = self
            .current_records(CurrentAggregate::Delivery, workspace_id)?
            .into_iter()
            .find(|record| record.aggregate_id == outbox_delivery_id(outbox_id))
        else {
            return Ok(None);
        };
        record
            .payload
            .get("outbox")
            .cloned()
            .map(serde_json::from_value)
            .transpose()
            .map_err(crate::StoreError::from)
    }
}

fn outbox_event<T>(
    request: &RequestEnvelope<T>,
    ordinal: u32,
    now: OffsetDateTime,
    variant: fn(EventData) -> RecoveryEvent,
    delivery: &DeliveryRecord,
    outbox: &OutboxRecord,
) -> StoreResult<NewEvent> {
    let mut data = EventData::new(&delivery.delivery_id);
    data.data = json!({"delivery": {"delivery_id": delivery.delivery_id, "notification_id": delivery.notification_id,
        "workspace_id": delivery.workspace_id, "status": delivery.status, "attempts": delivery.attempts,
        "last_error": delivery.last_error, "retry_at": delivery.retry_at, "delivered_at": delivery.delivered_at,
        "origin_event_seq": delivery.origin_event_seq, "outbox": outbox}});
    NewEvent::new(
        request.request_id,
        ordinal,
        now,
        EventPayload::Recovery(variant(data)),
    )
    .map_err(crate::StoreError::from)
}

fn outbox_delivery_id(outbox_id: &str) -> String {
    format!("outbox:{outbox_id}")
}
