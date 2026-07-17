use crate::{
    CommandOutcome, CommandPlan, CurrentAggregate, CurrentRecord, ProjectionReader, Store,
    StoreError, StoreResult,
    notifications::NotificationRecord,
    reservations::{expired, record_from_current, timestamp, typed_records},
};
use rusqlite::OptionalExtension;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use stateful_core::{
    ContextDelta, ContextEvent, ContextPackage, CurrentFreshness, CurrentItem, CurrentItemKind,
    CurrentSeverity, EventData, EventPayload, NewEvent, NotificationEvent, RecoveryEvent,
    RenderMode, RequestEnvelope, V2Error,
};
use std::collections::BTreeSet;
use time::{Duration, OffsetDateTime};
use uuid::Uuid;

const CONTEXT_DELIVERY_TTL: Duration = Duration::hours(24);
const CONTEXT_DELIVERY_SUMMARY_THRESHOLD: usize = 20;
const CONTEXT_DELIVERY_DEAD_LETTER_THRESHOLD: usize = 64;
const CONTEXT_INVALIDATED_KIND: &str = "context_invalidated";
const DELIVERY_PENDING: &str = "pending";
const DELIVERY_SUPERSEDED: &str = "superseded";
const DELIVERY_ACKNOWLEDGED: &str = "acknowledged";
const DELIVERY_DEAD_LETTER: &str = "dead_letter";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContextRender {
    pub mode: RenderMode,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resource: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContextAcknowledgement {
    pub delivery_id: String,
    pub sequence: u64,
    pub workspace_version: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContextAcknowledgementResult {
    pub acknowledged_version: u64,
    pub cursor: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContextDeliveryRecord {
    pub delivery_id: String,
    pub target_agent_id: String,
    pub workspace_id: String,
    pub sequence: u64,
    pub from_version: u64,
    pub workspace_version: u64,
    pub status: String,
    pub items: Vec<CurrentItem>,
    pub prompt_text: String,
    pub created_at: String,
    pub expires_at: String,
    #[serde(default)]
    pub origin_event_seq: u64,
}

impl Store {
    pub fn render_context(
        &self,
        request: &RequestEnvelope<ContextRender>,
    ) -> StoreResult<CommandOutcome<ContextDelta>> {
        let now = self.clock.now();
        let input = request.payload.clone();
        self.execute_command(request, "context.render", |reader| {
            let workspace_id = &request.workspace.workspace_id;
            let agent_id = &request.agent.agent_id;
            let current_version = reader.workspace_version(workspace_id)?;
            let cursor = reader.context_cursor(workspace_id, agent_id)?;
            if cursor > current_version {
                return Err(invalid_context(
                    "context cursor is ahead of workspace version.",
                ));
            }

            let deliveries = context_deliveries(reader, workspace_id, agent_id)?;
            let mut events = vec![rendered_event(request, 0, now, cursor, current_version)?];
            if let Some(delivery) = deliveries.iter().find(|delivery| {
                is_unacknowledged(delivery) && delivery.workspace_version == current_version
            }) {
                return Ok(CommandPlan {
                    events,
                    response: delta_from_delivery(delivery),
                    http_status: 200,
                });
            }
            if cursor == current_version {
                return Ok(CommandPlan {
                    events,
                    response: ContextDelta::unchanged(cursor, current_version),
                    http_status: 200,
                });
            }

            let mut items = changed_current_items(
                reader,
                workspace_id,
                agent_id,
                cursor,
                input.resource.as_deref(),
            )?;
            let outstanding = deliveries
                .iter()
                .filter(|delivery| is_unacknowledged(delivery))
                .count();
            let dead_letter = outstanding >= CONTEXT_DELIVERY_DEAD_LETTER_THRESHOLD;
            if outstanding >= CONTEXT_DELIVERY_SUMMARY_THRESHOLD {
                items.clear();
            }
            let sequence = deliveries
                .iter()
                .map(|delivery| delivery.sequence)
                .max()
                .unwrap_or(0)
                + 1;
            let mut delivery = ContextDeliveryRecord {
                delivery_id: Uuid::new_v4().to_string(),
                target_agent_id: agent_id.clone(),
                workspace_id: workspace_id.clone(),
                sequence,
                from_version: cursor,
                workspace_version: current_version,
                status: DELIVERY_PENDING.into(),
                items,
                prompt_text: String::new(),
                created_at: timestamp(now)?,
                expires_at: timestamp(now + CONTEXT_DELIVERY_TTL)?,
                origin_event_seq: 0,
            };
            let package = ContextPackage::from_items(delivery.items.clone());
            delivery.prompt_text = stateful_core::render_prompt_text(&package, input.mode);
            if outstanding >= CONTEXT_DELIVERY_SUMMARY_THRESHOLD {
                delivery.prompt_text = format!(
                    "Context delivery queue: {} unacknowledged deliveries; this is version {}.\n",
                    outstanding + usize::from(!dead_letter),
                    current_version,
                );
            }

            for previous in deliveries
                .iter()
                .filter(|delivery| is_unacknowledged(delivery))
            {
                let mut previous = previous.clone();
                previous.status = DELIVERY_SUPERSEDED.into();
                events.push(context_delivery_event(
                    request,
                    events.len() as u32,
                    now,
                    ContextEvent::DeliverySuperseded,
                    &previous,
                )?);
            }
            events.push(context_delivery_event(
                request,
                events.len() as u32,
                now,
                ContextEvent::DeliveryCreated,
                &delivery,
            )?);

            if dead_letter {
                delivery.status = DELIVERY_DEAD_LETTER.into();
                events.push(context_delivery_recovery_event(
                    request,
                    events.len() as u32,
                    now,
                    &delivery,
                )?);
            } else {
                let (notification, variant) =
                    context_notification(reader, request, now, &delivery)?;
                events.push(notification_event(
                    request,
                    events.len() as u32,
                    now,
                    variant,
                    &notification,
                )?);
            }

            Ok(CommandPlan {
                events,
                response: delta_from_delivery(&delivery),
                http_status: 200,
            })
        })
    }

    pub fn acknowledge_context(
        &self,
        request: &RequestEnvelope<ContextAcknowledgement>,
    ) -> StoreResult<CommandOutcome<ContextAcknowledgementResult>> {
        let now = self.clock.now();
        let input = request.payload.clone();
        self.execute_command(request, "context.acknowledge", |reader| {
            let workspace_id = &request.workspace.workspace_id;
            let agent_id = &request.agent.agent_id;
            let deliveries = context_deliveries(reader, workspace_id, agent_id)?;
            let target = deliveries
                .iter()
                .find(|delivery| delivery.delivery_id == input.delivery_id)
                .ok_or_else(|| {
                    invalid_context("context delivery does not belong to this agent and workspace.")
                })?;
            if target.sequence != input.sequence
                || target.workspace_version != input.workspace_version
            {
                return Err(invalid_context(
                    "context acknowledgement does not match its delivery sequence and version.",
                ));
            }
            let acknowledged_version = target.workspace_version;
            let cursor = reader.context_cursor(workspace_id, agent_id)?;
            if target.status == DELIVERY_DEAD_LETTER || cursor >= acknowledged_version {
                return Ok(CommandPlan {
                    events: Vec::new(),
                    response: ContextAcknowledgementResult {
                        acknowledged_version,
                        cursor,
                    },
                    http_status: 200,
                });
            }

            let mut events = Vec::new();
            for delivery in deliveries.into_iter().filter(|delivery| {
                is_unacknowledged(delivery) && delivery.workspace_version <= acknowledged_version
            }) {
                let mut delivery = delivery;
                delivery.status = DELIVERY_ACKNOWLEDGED.into();
                events.push(context_delivery_event(
                    request,
                    events.len() as u32,
                    now,
                    ContextEvent::DeliveryAcknowledged,
                    &delivery,
                )?);
            }
            Ok(CommandPlan {
                events,
                response: ContextAcknowledgementResult {
                    acknowledged_version,
                    cursor: acknowledged_version,
                },
                http_status: 200,
            })
        })
    }

    pub fn expire_context_deliveries(
        &self,
        request: &RequestEnvelope<()>,
    ) -> StoreResult<CommandOutcome<Vec<String>>> {
        let now = self.clock.now();
        self.execute_command(request, "context.expire_deliveries", |reader| {
            let deliveries = typed_records::<ContextDeliveryRecord>(
                reader,
                CurrentAggregate::ContextDelivery,
                &request.workspace.workspace_id,
            )?;
            let mut events = Vec::new();
            let mut expired_ids = Vec::new();
            for mut delivery in deliveries.into_iter().filter(|delivery| {
                is_unacknowledged(delivery) && expired(&delivery.expires_at, now)
            }) {
                delivery.status = DELIVERY_DEAD_LETTER.into();
                expired_ids.push(delivery.delivery_id.clone());
                events.push(context_delivery_recovery_event(
                    request,
                    events.len() as u32,
                    now,
                    &delivery,
                )?);
            }
            Ok(CommandPlan {
                events,
                response: expired_ids,
                http_status: 200,
            })
        })
    }

    pub fn context_delivery(
        &self,
        workspace_id: &str,
        delivery_id: &str,
    ) -> StoreResult<Option<ContextDeliveryRecord>> {
        self.current_records(CurrentAggregate::ContextDelivery, workspace_id)?
            .into_iter()
            .map(record_from_current::<ContextDeliveryRecord>)
            .collect::<StoreResult<Vec<_>>>()?
            .into_iter()
            .find(|delivery| delivery.delivery_id == delivery_id)
            .map(Ok)
            .transpose()
    }

    pub fn pending_context_deliveries(
        &self,
        target_agent_id: &str,
        workspace_id: &str,
    ) -> StoreResult<Vec<ContextDeliveryRecord>> {
        let mut deliveries = self
            .current_records(CurrentAggregate::ContextDelivery, workspace_id)?
            .into_iter()
            .map(record_from_current::<ContextDeliveryRecord>)
            .collect::<StoreResult<Vec<_>>>()?
            .into_iter()
            .filter(|delivery| {
                delivery.target_agent_id == target_agent_id && is_unacknowledged(delivery)
            })
            .collect::<Vec<_>>();
        deliveries.sort_by_key(|delivery| (delivery.workspace_version, delivery.sequence));
        Ok(deliveries)
    }

    pub fn context_cursor(&self, workspace_id: &str, agent_id: &str) -> StoreResult<u64> {
        self.conn
            .query_row(
                "SELECT version FROM agent_context_cursor WHERE workspace_id = ?1 AND agent_id = ?2",
                rusqlite::params![workspace_id, agent_id],
                |row| row.get(0),
            )
            .optional()
            .map(|value: Option<u64>| value.unwrap_or(0))
            .map_err(StoreError::from)
    }
}

fn context_deliveries(
    reader: &dyn ProjectionReader,
    workspace_id: &str,
    agent_id: &str,
) -> StoreResult<Vec<ContextDeliveryRecord>> {
    Ok(typed_records::<ContextDeliveryRecord>(
        reader,
        CurrentAggregate::ContextDelivery,
        workspace_id,
    )?
    .into_iter()
    .filter(|delivery| delivery.target_agent_id == agent_id)
    .collect())
}

fn delta_from_delivery(delivery: &ContextDeliveryRecord) -> ContextDelta {
    ContextDelta {
        from_version: delivery.from_version,
        workspace_version: delivery.workspace_version,
        changed: true,
        reset_required: false,
        delivery_id: Some(delivery.delivery_id.clone()),
        sequence: Some(delivery.sequence),
        items: delivery.items.clone(),
        prompt_text: delivery.prompt_text.clone(),
    }
}

fn is_unacknowledged(delivery: &ContextDeliveryRecord) -> bool {
    matches!(
        delivery.status.as_str(),
        DELIVERY_PENDING | DELIVERY_SUPERSEDED
    )
}

fn invalid_context(message: &str) -> StoreError {
    StoreError::V2(V2Error::new("invalid_context_delivery", message))
}

fn rendered_event<T>(
    request: &RequestEnvelope<T>,
    ordinal: u32,
    now: OffsetDateTime,
    from_version: u64,
    workspace_version: u64,
) -> StoreResult<NewEvent> {
    let mut data = EventData::new(format!("{}:{workspace_version}", request.agent.agent_id));
    data.data = json!({"from_version": from_version, "workspace_version": workspace_version});
    NewEvent::new(
        request.request_id,
        ordinal,
        now,
        EventPayload::Context(ContextEvent::Rendered(data)),
    )
    .map_err(StoreError::from)
}

fn context_delivery_event<T>(
    request: &RequestEnvelope<T>,
    ordinal: u32,
    now: OffsetDateTime,
    variant: fn(EventData) -> ContextEvent,
    delivery: &ContextDeliveryRecord,
) -> StoreResult<NewEvent> {
    let mut data = EventData::new(&delivery.delivery_id);
    data.data = json!({"context_delivery": delivery});
    NewEvent::new(
        request.request_id,
        ordinal,
        now,
        EventPayload::Context(variant(data)),
    )
    .map_err(StoreError::from)
}

fn context_delivery_recovery_event<T>(
    request: &RequestEnvelope<T>,
    ordinal: u32,
    now: OffsetDateTime,
    delivery: &ContextDeliveryRecord,
) -> StoreResult<NewEvent> {
    let mut data = EventData::new(&delivery.delivery_id);
    data.data = json!({"context_delivery": delivery});
    NewEvent::new(
        request.request_id,
        ordinal,
        now,
        EventPayload::Recovery(RecoveryEvent::Failed(data)),
    )
    .map_err(StoreError::from)
}

type ContextNotification = (NotificationRecord, fn(EventData) -> NotificationEvent);

fn context_notification<T>(
    reader: &dyn ProjectionReader,
    request: &RequestEnvelope<T>,
    now: OffsetDateTime,
    delivery: &ContextDeliveryRecord,
) -> StoreResult<ContextNotification> {
    let notifications = typed_records::<NotificationRecord>(
        reader,
        CurrentAggregate::Notification,
        &request.workspace.workspace_id,
    )?;
    let next_sequence = notifications
        .iter()
        .filter(|notification| notification.target_agent_id == request.agent.agent_id)
        .map(|notification| notification.sequence)
        .max()
        .unwrap_or(0)
        + 1;
    let mut queued = notifications.into_iter().find(|notification| {
        notification.status == "queued"
            && notification.target_agent_id == request.agent.agent_id
            && notification.kind == CONTEXT_INVALIDATED_KIND
    });
    let variant = if let Some(notification) = queued.as_mut() {
        notification.sequence = next_sequence;
        notification.payload = json!({
            "delivery_id": delivery.delivery_id,
            "sequence": delivery.sequence,
            "target_version": delivery.workspace_version,
        });
        notification.expires_at = Some(timestamp(now + CONTEXT_DELIVERY_TTL)?);
        NotificationEvent::Coalesced
    } else {
        let sequence = next_sequence;
        queued = Some(NotificationRecord {
            notification_id: Uuid::new_v4().to_string(),
            sequence,
            target_agent_id: request.agent.agent_id.clone(),
            workspace_id: request.workspace.workspace_id.clone(),
            kind: CONTEXT_INVALIDATED_KIND.into(),
            payload: json!({
                "delivery_id": delivery.delivery_id,
                "sequence": delivery.sequence,
                "target_version": delivery.workspace_version,
            }),
            status: "queued".into(),
            created_at: timestamp(now)?,
            expires_at: Some(timestamp(now + CONTEXT_DELIVERY_TTL)?),
            coalesce_key: Some(format!(
                "{CONTEXT_INVALIDATED_KIND}:{}",
                request.agent.agent_id
            )),
            origin_event_seq: 0,
        });
        NotificationEvent::Created
    };
    Ok((queued.expect("notification is assigned"), variant))
}

fn notification_event<T>(
    request: &RequestEnvelope<T>,
    ordinal: u32,
    now: OffsetDateTime,
    variant: fn(EventData) -> NotificationEvent,
    notification: &NotificationRecord,
) -> StoreResult<NewEvent> {
    let mut data = EventData::new(&notification.notification_id);
    data.data = json!({"notification": notification});
    NewEvent::new(
        request.request_id,
        ordinal,
        now,
        EventPayload::Notification(variant(data)),
    )
    .map_err(StoreError::from)
}

fn changed_current_items(
    reader: &dyn ProjectionReader,
    workspace_id: &str,
    target_agent_id: &str,
    after_version: u64,
    resource_filter: Option<&str>,
) -> StoreResult<Vec<CurrentItem>> {
    let changed = reader.context_changes(workspace_id, after_version)?;
    let mut seen = BTreeSet::new();
    let mut items = Vec::new();
    for (kind, aggregate_id) in changed {
        if !seen.insert((kind.clone(), aggregate_id.clone())) {
            continue;
        }
        if kind == "migration" {
            append_migrated_current_items(
                &mut items,
                reader,
                workspace_id,
                target_agent_id,
                after_version,
                resource_filter,
                &aggregate_id,
            )?;
            continue;
        }
        if kind == "presence" {
            if let Some(presence) = reader
                .live_presences(workspace_id)?
                .into_iter()
                .find(|presence| presence.agent_id == aggregate_id)
                && let Some(item) = presence_item(
                    reader,
                    presence,
                    workspace_id,
                    target_agent_id,
                    resource_filter,
                )?
            {
                items.push((item.0, item.1));
            }
            continue;
        }
        if kind == "handoff" {
            if let Some(handoff) = reader.handoff(workspace_id, &aggregate_id)?
                && let Some(item) = handoff_item(handoff, workspace_id, resource_filter)
            {
                items.push(item);
            }
            continue;
        }
        if let Some(aggregate) = aggregate_for_kind(&kind)
            && let Some(record) = reader
                .aggregate_records(aggregate, workspace_id)?
                .into_iter()
                .find(|record| record.aggregate_id == aggregate_id)
        {
            append_record_items(
                &mut items,
                &kind,
                record,
                workspace_id,
                target_agent_id,
                resource_filter,
            );
        }
    }
    items.sort_by_key(|(sequence, _)| *sequence);
    let mut deduplicated = Vec::with_capacity(items.len());
    for item in items {
        if !deduplicated.contains(&item) {
            deduplicated.push(item);
        }
    }
    Ok(deduplicated.into_iter().map(|(_, item)| item).collect())
}

fn append_migrated_current_items(
    items: &mut Vec<(u64, CurrentItem)>,
    reader: &dyn ProjectionReader,
    workspace_id: &str,
    target_agent_id: &str,
    after_version: u64,
    resource_filter: Option<&str>,
    aggregate_id: &str,
) -> StoreResult<()> {
    for presence in reader.live_presences(workspace_id)? {
        if presence.agent_id == aggregate_id
            && presence.origin_event_seq > after_version
            && let Some(item) = presence_item(
                reader,
                presence,
                workspace_id,
                target_agent_id,
                resource_filter,
            )?
        {
            items.push(item);
        }
    }
    for handoff in reader.handoffs(workspace_id)? {
        if handoff.origin_event_seq > after_version
            && let Some(item) = handoff_item(handoff, workspace_id, resource_filter)
        {
            items.push(item);
        }
    }
    for kind in [
        "reservation",
        "claim",
        "wait",
        "write_fence",
        "read_observation",
        "write_intent",
        "human_observation",
        "human_acknowledgement",
    ] {
        let aggregate = aggregate_for_kind(kind).expect("migration current kind is mapped");
        for record in reader.aggregate_records(aggregate, workspace_id)? {
            if record.aggregate_id == aggregate_id && record.origin_event_seq > after_version {
                append_record_items(
                    items,
                    kind,
                    record,
                    workspace_id,
                    target_agent_id,
                    resource_filter,
                );
            }
        }
    }
    Ok(())
}

fn presence_item(
    reader: &dyn ProjectionReader,
    presence: stateful_core::PresenceRecord,
    workspace_id: &str,
    target_agent_id: &str,
    resource_filter: Option<&str>,
) -> StoreResult<Option<(u64, CurrentItem)>> {
    if presence.agent_id == target_agent_id {
        return Ok(None);
    }
    let resources = reader.presence_resources(workspace_id, &presence.agent_id)?;
    let resource = resources.iter().find_map(|resource| {
        resource_relevant(resource_filter, &resource.relative_path)
            .then(|| resource.relative_path.clone())
    });
    if resource_filter.is_some() && resource.is_none() {
        return Ok(None);
    }
    let summary = match (&presence.goal_excerpt, &presence.phase) {
        (Some(goal), Some(phase)) => format!("Agent {} is {:?}: {goal}.", presence.agent_id, phase),
        (Some(goal), None) => format!("Agent {}: {goal}.", presence.agent_id),
        _ => format!("Agent {} is active.", presence.agent_id),
    };
    Ok(Some((
        presence.origin_event_seq,
        CurrentItem::new(
            CurrentItemKind::Agent,
            CurrentSeverity::Info,
            CurrentFreshness::Live,
            resource.unwrap_or_else(|| "presence".into()),
            "Understand nearby active work before editing.",
            summary,
        )
        .with_agent(presence.agent_id)
        .with_workspace(workspace_id),
    )))
}

fn handoff_item(
    handoff: stateful_core::HandoffRecord,
    workspace_id: &str,
    resource_filter: Option<&str>,
) -> Option<(u64, CurrentItem)> {
    let resource = handoff
        .files_changed
        .iter()
        .find(|path| resource_relevant(resource_filter, path));
    (resource_filter.is_none() || resource.is_some()).then(|| {
        (
            handoff.origin_event_seq,
            CurrentItem::new(
                CurrentItemKind::Finalization,
                CurrentSeverity::Info,
                CurrentFreshness::Finalized,
                resource.cloned().unwrap_or_else(|| "handoff".into()),
                "Preserve handoff context from previous work.",
                handoff.summary,
            )
            .with_agent(handoff.agent_id)
            .with_workspace(workspace_id),
        )
    })
}

fn append_record_items(
    items: &mut Vec<(u64, CurrentItem)>,
    kind: &str,
    record: CurrentRecord,
    workspace_id: &str,
    target_agent_id: &str,
    resource_filter: Option<&str>,
) {
    let sequence = record.origin_event_seq;
    let mut resources = if kind == "reservation" {
        reservation_scope_paths(&record.payload)
    } else if kind == "write_intent"
        && record.payload.get("status").and_then(Value::as_str) == Some("outcome_unknown")
    {
        write_intent_target_paths(&record.payload)
    } else {
        Vec::new()
    };
    if resources.is_empty() {
        resources.push(String::new());
    }
    for resource in resources {
        if let Some(item) = item_from_current(
            kind,
            record.clone(),
            workspace_id,
            target_agent_id,
            resource_filter,
            (!resource.is_empty()).then_some(resource.as_str()),
        ) {
            items.push((sequence, item));
        }
    }
}

fn aggregate_for_kind(kind: &str) -> Option<CurrentAggregate> {
    match kind {
        "reservation" => Some(CurrentAggregate::Reservation),
        "claim" => Some(CurrentAggregate::Claim),
        "wait" => Some(CurrentAggregate::Wait),
        "write_fence" => Some(CurrentAggregate::WriteFence),
        "read_observation" => Some(CurrentAggregate::ReadObservation),
        "write_intent" => Some(CurrentAggregate::WriteIntent),
        "human_observation" => Some(CurrentAggregate::HumanObservation),
        "human_acknowledgement" => Some(CurrentAggregate::HumanAcknowledgement),
        _ => None,
    }
}

fn item_from_current(
    kind: &str,
    record: CurrentRecord,
    workspace_id: &str,
    target_agent_id: &str,
    resource_filter: Option<&str>,
    resource_override: Option<&str>,
) -> Option<CurrentItem> {
    let value = record.payload;
    let status = value
        .get("status")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let resource = resource_override.unwrap_or_else(|| {
        value
            .get("relative_path")
            .or_else(|| value.get("path"))
            .and_then(Value::as_str)
            .unwrap_or(kind)
    });
    if !resource_relevant(resource_filter, resource) {
        return None;
    }

    let agent_id = value.get("agent_id").and_then(Value::as_str);
    let purpose = value
        .get("purpose")
        .and_then(Value::as_str)
        .unwrap_or("Coordinate with related work.");
    let (item_kind, severity, summary, next_action) = match kind {
        "reservation" if status == "active" => (
            CurrentItemKind::Reservation,
            CurrentSeverity::Warn,
            format!(
                "Agent {} reserved {resource}.",
                agent_id.unwrap_or("another agent")
            ),
            Some("Coordinate with the reservation owner before editing this resource."),
        ),
        "claim" if status == "active" => (
            CurrentItemKind::Claim,
            CurrentSeverity::Warn,
            format!(
                "Agent {} holds an active claim on {resource}.",
                agent_id.unwrap_or("another agent")
            ),
            Some("Coordinate with the claim owner before editing this resource."),
        ),
        "wait" if matches!(status, "queued" | "waiting") => (
            CurrentItemKind::WaitQueue,
            CurrentSeverity::Warn,
            format!(
                "Agent {} is queued for {resource}.",
                agent_id.unwrap_or("another agent")
            ),
            Some("Wait for the reservation to become claimable."),
        ),
        "wait" if status == "claimable" => (
            CurrentItemKind::ClaimableReservation,
            CurrentSeverity::Warn,
            format!(
                "Agent {} has a claimable reservation for {resource}.",
                agent_id.unwrap_or("another agent")
            ),
            Some("Claim the granted reservation before it expires."),
        ),
        "write_fence" if status == "active" => (
            CurrentItemKind::Claim,
            CurrentSeverity::Block,
            format!(
                "Agent {} has a write fence on {resource}.",
                agent_id.unwrap_or("another agent")
            ),
            Some("Wait for the write fence to release or coordinate with its owner."),
        ),
        "human_observation"
            if status == "pending"
                && value.get("confidence").and_then(Value::as_str) == Some("high")
                && matches!(
                    value.get("kind").and_then(Value::as_str),
                    Some("save" | "change" | "delete")
                ) =>
        {
            (
                CurrentItemKind::Claim,
                CurrentSeverity::Block,
                format!("{resource} has an unreconciled human write."),
                Some(
                    "Reread the resource, summarize the human change, then acknowledge reconciliation.",
                ),
            )
        }
        "human_observation" if status == "pending" => (
            CurrentItemKind::Claim,
            CurrentSeverity::Warn,
            format!("Human activity was observed on {resource}."),
            None,
        ),
        "write_intent" if status == "outcome_unknown" => (
            CurrentItemKind::Claim,
            CurrentSeverity::Block,
            format!("The write outcome for {resource} is unknown."),
            Some("Perform a fresh exact read and reconcile the unknown write outcome."),
        ),
        _ => return None,
    };
    let mut item = CurrentItem::new(
        item_kind,
        severity,
        CurrentFreshness::Live,
        resource,
        purpose,
        summary,
    )
    .with_workspace(workspace_id);
    if let Some(next_action) = next_action {
        item = item.with_next_action(next_action);
    }
    if let Some(agent_id) = agent_id {
        item = item.with_agent(agent_id);
        if agent_id == target_agent_id
            && matches!(kind, "reservation" | "claim" | "wait" | "write_fence")
        {
            item.severity = CurrentSeverity::Info;
            item.source_refs
                .push(stateful_core::AGENT_CONTEXT_SCOPE_SOURCE_REF.into());
        }
    }
    Some(item)
}
fn reservation_scope_paths(value: &Value) -> Vec<String> {
    value
        .get("scopes")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|scope| {
            let path = scope.get("path")?.as_str()?;
            Some(
                if scope.get("kind").and_then(Value::as_str) == Some("directory") {
                    format!("{path}/")
                } else {
                    path.into()
                },
            )
        })
        .collect()
}

fn write_intent_target_paths(value: &Value) -> Vec<String> {
    value
        .get("targets")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|target| {
            target
                .get("path")
                .and_then(Value::as_str)
                .map(str::to_owned)
        })
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

fn resource_relevant(resource_filter: Option<&str>, resource: &str) -> bool {
    let Some(filter) = resource_filter.filter(|filter| !filter.is_empty()) else {
        return true;
    };
    resource == filter
        || resource
            .strip_suffix('/')
            .is_some_and(|prefix| filter.starts_with(prefix))
        || filter
            .strip_suffix('/')
            .is_some_and(|prefix| resource.starts_with(prefix))
}
