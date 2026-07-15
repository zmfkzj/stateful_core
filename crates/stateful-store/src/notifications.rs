use crate::{
    CommandOutcome, CommandPlan, CurrentAggregate, Store, StoreError, StoreResult,
    reservations::{expired, record_from_current, timestamp, typed_records},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use stateful_core::{
    EventData, EventPayload, NewEvent, NotificationEvent, RecoveryEvent, RequestEnvelope,
};
use time::{Duration, OffsetDateTime};
use uuid::Uuid;

const NOTIFICATION_TTL: Duration = Duration::minutes(2);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NotificationCreate {
    pub target_agent_id: String,
    pub kind: String,
    pub payload: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub coalesce_key: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeliveryAttempt { Attempted, Delivered, Failed }

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NotificationDelivery {
    pub notification_id: String,
    pub sequence: u64,
    pub outcome: DeliveryAttempt,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none", with = "time::serde::rfc3339::option")]
    pub retry_at: Option<OffsetDateTime>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NotificationRecord {
    pub notification_id: String,
    pub sequence: u64,
    pub target_agent_id: String,
    pub workspace_id: String,
    pub kind: String,
    pub payload: Value,
    pub status: String,
    pub created_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub coalesce_key: Option<String>,
    #[serde(default)]
    pub origin_event_seq: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeliveryRecord {
    pub delivery_id: String,
    pub notification_id: String,
    pub workspace_id: String,
    pub status: String,
    pub attempts: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delivered_at: Option<String>,
    #[serde(default)]
    pub origin_event_seq: u64,
}

impl Store {
    pub fn create_notification(
        &self,
        request: &RequestEnvelope<NotificationCreate>,
    ) -> StoreResult<CommandOutcome<NotificationRecord>> {
        let now = self.clock.now();
        let payload = request.payload.clone();
        self.execute_command(request, "notification.create", |reader| {
            if payload.target_agent_id.trim().is_empty() || payload.kind.trim().is_empty() {
                return Err(StoreError::MissingScope);
            }
            let notifications = typed_records::<NotificationRecord>(reader, CurrentAggregate::Notification, &request.workspace.workspace_id)?;
            let next_sequence = notifications
                .iter()
                .filter(|notification| notification.target_agent_id == payload.target_agent_id)
                .map(|notification| notification.sequence)
                .max()
                .unwrap_or(0) + 1;
            let mut notification = notifications.into_iter().find(|notification| {
                notification.status == "queued"
                    && notification.target_agent_id == payload.target_agent_id
                    && notification.kind == payload.kind
                    && notification.coalesce_key == payload.coalesce_key
                    && payload.coalesce_key.is_some()
            });
            let created = notification.is_none();
            let variant = if let Some(existing) = notification.as_mut() {
                existing.sequence = next_sequence;
                existing.payload = payload.payload.clone();
                existing.expires_at = Some(timestamp(now + NOTIFICATION_TTL)?);
                NotificationEvent::Coalesced
            } else {
                let sequence = next_sequence;
                notification = Some(NotificationRecord {
                    notification_id: Uuid::new_v4().to_string(),
                    sequence,
                    target_agent_id: payload.target_agent_id,
                    workspace_id: request.workspace.workspace_id.clone(),
                    kind: payload.kind,
                    payload: payload.payload,
                    status: "queued".into(),
                    created_at: timestamp(now)?,
                    expires_at: Some(timestamp(now + NOTIFICATION_TTL)?),
                    coalesce_key: payload.coalesce_key,
                    origin_event_seq: 0,
                });
                NotificationEvent::Created
            };
            let notification = notification.expect("notification is always assigned");
            let delivery = DeliveryRecord {
                delivery_id: notification.notification_id.clone(),
                notification_id: notification.notification_id.clone(),
                workspace_id: request.workspace.workspace_id.clone(),
                status: "queued".into(),
                attempts: 0,
                last_error: None,
                retry_at: None,
                delivered_at: None,
                origin_event_seq: 0,
            };
            let mut events = vec![notification_event(request, 0, now, variant, &notification)?];
            if created {
                events.push(notification_delivery_event(request, 1, now, RecoveryEvent::Queued, &delivery, &notification)?);
            }
            Ok(CommandPlan { events, response: notification, http_status: 200 })
        })
    }

    pub fn record_notification_delivery(
        &self,
        request: &RequestEnvelope<NotificationDelivery>,
    ) -> StoreResult<CommandOutcome<DeliveryRecord>> {
        let now = self.clock.now();
        let payload = request.payload.clone();
        self.execute_command(request, "notification.delivery", |reader| {
            let notifications = typed_records::<NotificationRecord>(reader, CurrentAggregate::Notification, &request.workspace.workspace_id)?;
            let notification = notifications.into_iter().find(|notification| notification.notification_id == payload.notification_id)
                .ok_or(StoreError::ReservationRequestNotFound)?;
            let mut delivery = typed_records::<DeliveryRecord>(reader, CurrentAggregate::Delivery, &request.workspace.workspace_id)?
                .into_iter().find(|delivery| delivery.notification_id == payload.notification_id)
                .unwrap_or(DeliveryRecord {
                    delivery_id: payload.notification_id.clone(), notification_id: payload.notification_id.clone(),
                    workspace_id: request.workspace.workspace_id.clone(), status: "queued".into(), attempts: 0,
                    last_error: None, retry_at: None, delivered_at: None, origin_event_seq: 0,
                });
            if notification.sequence != payload.sequence
                || delivery.status == "delivered"
                || notification.status == "expired"
            {
                return Ok(CommandPlan { events: Vec::new(), response: delivery, http_status: 200 });
            }
            let (variant, status): (fn(EventData) -> RecoveryEvent, &str) = match payload.outcome {
                DeliveryAttempt::Attempted => (RecoveryEvent::Attempted, "attempted"),
                DeliveryAttempt::Delivered => (RecoveryEvent::Delivered, "delivered"),
                DeliveryAttempt::Failed => (RecoveryEvent::Failed, "failed"),
            };
            if delivery.status == status
                && delivery.last_error == payload.error
                && delivery.retry_at == payload.retry_at.map(timestamp).transpose()?
            {
                return Ok(CommandPlan { events: Vec::new(), response: delivery, http_status: 200 });
            }
            delivery.status = status.into();
            delivery.attempts += 1;
            delivery.last_error = payload.error;
            delivery.retry_at = payload.retry_at.map(timestamp).transpose()?;
            if matches!(payload.outcome, DeliveryAttempt::Delivered) { delivery.delivered_at = Some(timestamp(now)?); }
            let mut events = vec![notification_delivery_event(request, 0, now, variant, &delivery, &notification)?];
            if matches!(payload.outcome, DeliveryAttempt::Delivered) && notification.status != "delivered" {
                let mut notification = notification;
                notification.status = "delivered".into();
                events.push(notification_event(request, 1, now, NotificationEvent::Delivered, &notification)?);
            }
            Ok(CommandPlan { events, response: delivery, http_status: 200 })
        })
    }

    pub fn expire_notifications(
        &self,
        request: &RequestEnvelope<()>,
    ) -> StoreResult<CommandOutcome<Vec<String>>> {
        let now = self.clock.now();
        self.execute_command(request, "notification.expire", |reader| {
            let mut events = Vec::new();
            let mut expired_ids = Vec::new();
            for mut notification in typed_records::<NotificationRecord>(reader, CurrentAggregate::Notification, &request.workspace.workspace_id)? {
                if notification.status == "queued" && notification.expires_at.as_deref().is_some_and(|value| expired(value, now)) {
                    notification.status = "expired".into();
                    expired_ids.push(notification.notification_id.clone());
                    events.push(notification_event(request, events.len() as u32, now, NotificationEvent::Expired, &notification)?);
                }
            }
            Ok(CommandPlan { events, response: expired_ids, http_status: 200 })
        })
    }

    pub fn pending_notifications(&self, target_agent_id: &str, workspace_id: &str) -> StoreResult<Vec<NotificationRecord>> {
        let mut notifications = self.current_records(CurrentAggregate::Notification, workspace_id)?.into_iter()
            .map(record_from_current::<NotificationRecord>)
            .collect::<StoreResult<Vec<_>>>()?
            .into_iter()
            .filter(|notification| notification.target_agent_id == target_agent_id && notification.status == "queued")
            .collect::<Vec<_>>();
        notifications.sort_by_key(|notification| notification.sequence);
        Ok(notifications)
    }

    pub fn delivery(&self, workspace_id: &str, notification_id: &str) -> StoreResult<Option<DeliveryRecord>> {
        self.current_records(CurrentAggregate::Delivery, workspace_id)?.into_iter()
            .map(record_from_current::<DeliveryRecord>)
            .collect::<StoreResult<Vec<_>>>()?
            .into_iter()
            .find(|delivery| delivery.notification_id == notification_id)
            .map(Ok)
            .transpose()
    }
}

fn notification_event<T>(
    request: &RequestEnvelope<T>, ordinal: u32, now: OffsetDateTime,
    variant: fn(EventData) -> NotificationEvent, notification: &NotificationRecord,
) -> StoreResult<NewEvent> {
    let mut data = EventData::new(&notification.notification_id);
    data.data = json!({"notification": notification});
    NewEvent::new(request.request_id, ordinal, now, EventPayload::Notification(variant(data))).map_err(StoreError::from)
}

pub(crate) fn delivery_event<T>(
    request: &RequestEnvelope<T>, ordinal: u32, now: OffsetDateTime,
    variant: fn(EventData) -> RecoveryEvent, delivery: &DeliveryRecord,
) -> StoreResult<NewEvent> {
    let mut data = EventData::new(&delivery.delivery_id);
    data.data = json!({"delivery": delivery});
    NewEvent::new(request.request_id, ordinal, now, EventPayload::Recovery(variant(data))).map_err(StoreError::from)
}

fn notification_delivery_event<T>(
    request: &RequestEnvelope<T>,
    ordinal: u32,
    now: OffsetDateTime,
    variant: fn(EventData) -> RecoveryEvent,
    delivery: &DeliveryRecord,
    notification: &NotificationRecord,
) -> StoreResult<NewEvent> {
    let mut data = EventData::new(&delivery.delivery_id);
    data.data = json!({
        "delivery": delivery,
        "notification_kind": notification.kind,
    });
    NewEvent::new(request.request_id, ordinal, now, EventPayload::Recovery(variant(data))).map_err(StoreError::from)
}
