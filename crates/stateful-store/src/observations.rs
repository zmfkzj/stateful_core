use crate::{
    CommandOutcome, CommandPlan, CurrentAggregate, Store, StoreError, StoreResult,
    reservations::{normalized_scope, record_from_current, typed_records},
    presence::{presence_for_resource_update, resource_update_event},
};
use serde_json::json;
use stateful_core::{
    EventData, EventPayload, NewEvent, OBSERVATION_TTL, ReadCompletion, ReadObservationRecord,
    ReadObservationStart, ReadObservationStatus, RequestEnvelope, ResourceVersion,
    ReadObservationEvent, PresenceResourceRelation, observation_status,
};
use time::OffsetDateTime;

impl Store {
    pub fn start_read_observation(
        &self,
        request: &RequestEnvelope<ReadObservationStart>,
    ) -> StoreResult<CommandOutcome<ReadObservationRecord>> {
        let now = self.clock.now();
        let payload = request.payload.clone();
        self.execute_command(request, "read_observation.start", |reader| {
            let path = normalized_scope(&payload.path)?;
            if payload.operation_id.trim().is_empty() {
                return Err(StoreError::InvalidReadOperation);
            }
            let resource_version = typed_records::<ResourceVersion>(
                reader,
                CurrentAggregate::ResourceWrite,
                &request.workspace.workspace_id,
            )?
            .into_iter()
            .find(|version| version.path == path)
            .map_or(0, |version| version.version);
            let record = ReadObservationRecord {
                workspace_id: request.workspace.workspace_id.clone(),
                agent_id: request.agent.agent_id.clone(),
                actor_id: request.agent.actor_id.clone(),
                operation_id: payload.operation_id,
                path,
                status: ReadObservationStatus::Started,
                classification: stateful_core::ReadClassification::Ambiguous,
                before: payload.before,
                after: None,
                semantic_marker: None,
                observed_at: now,
                expires_at: None,
                resource_version,
                origin_event_seq: 0,
            };
            Ok(CommandPlan {
                events: vec![read_event(request, 0, now, ReadObservationEvent::Started, &record)?],
                response: record,
                http_status: 200,
            })
        })
    }

    pub fn complete_read_observation(
        &self,
        request: &RequestEnvelope<ReadCompletion>,
    ) -> StoreResult<CommandOutcome<ReadObservationRecord>> {
        let now = self.clock.now();
        let payload = request.payload.clone();
        self.execute_command(request, "read_observation.complete", |reader| {
            let path = normalized_scope(&payload.path)?;
            let start = typed_records::<ReadObservationRecord>(
                reader,
                CurrentAggregate::ReadOperation,
                &request.workspace.workspace_id,
            )?
            .into_iter()
            .find(|record| {
                record.agent_id == request.agent.agent_id
                    && record.actor_id == request.agent.actor_id
                    && record.operation_id == payload.operation_id
                    && record.path == path
            })
            .ok_or(StoreError::ReadOperationNotFound)?;
            let current_resource_version = typed_records::<ResourceVersion>(
                reader,
                CurrentAggregate::ResourceWrite,
                &request.workspace.workspace_id,
            )?
            .into_iter()
            .find(|version| version.path == path)
            .map_or(0, |version| version.version);
            let mut status = observation_status(
                payload.classification,
                &start.before,
                payload.after.as_ref(),
                payload.semantic_marker.as_deref(),
            );
            if current_resource_version != start.resource_version && status == ReadObservationStatus::Stabilized {
                status = ReadObservationStatus::Unstable;
            }
            let record = ReadObservationRecord {
                workspace_id: request.workspace.workspace_id.clone(),
                agent_id: start.agent_id.clone(),
                actor_id: start.actor_id.clone(),
                operation_id: payload.operation_id,
                path,
                status,
                classification: payload.classification,
                before: start.before,
                after: payload.after,
                semantic_marker: payload.semantic_marker,
                observed_at: now,
                expires_at: (status == ReadObservationStatus::Stabilized).then_some(now + OBSERVATION_TTL),
                resource_version: start.resource_version,
                origin_event_seq: 0,
            };
            let variant = match status {
                ReadObservationStatus::Stabilized => ReadObservationEvent::Stabilized,
                ReadObservationStatus::Aborted => ReadObservationEvent::Aborted,
                ReadObservationStatus::Unstable => ReadObservationEvent::Unstable,
                _ => return Err(StoreError::InvalidReadOperation),
            };
            let mut events = vec![read_event(request, 0, now, variant, &record)?];
            if status == ReadObservationStatus::Stabilized {
                let mut presence = presence_for_resource_update(reader, request, now)?;
                events.push(resource_update_event(
                    reader,
                    request,
                    now,
                    events.len() as u32,
                    &mut presence,
                    &record.path,
                    PresenceResourceRelation::Read,
                )?);
            }
            Ok(CommandPlan {
                events,
                response: record,
                http_status: 200,
            })
        })
    }

    pub fn expire_read_observations(
        &self,
        request: &RequestEnvelope<()>,
    ) -> StoreResult<CommandOutcome<Vec<String>>> {
        let now = self.clock.now();
        self.execute_command(request, "read_observation.expire", |reader| {
            let mut events = Vec::new();
            let mut expired = Vec::new();
            for mut record in typed_records::<ReadObservationRecord>(
                reader,
                CurrentAggregate::ReadObservation,
                &request.workspace.workspace_id,
            )? {
                if record.status == ReadObservationStatus::Stabilized
                    && record.expires_at.is_none_or(|expires_at| expires_at <= now)
                {
                    record.status = ReadObservationStatus::Expired;
                    expired.push(record.path.clone());
                    events.push(read_event(
                        request,
                        events.len() as u32,
                        now,
                        ReadObservationEvent::Expired,
                        &record,
                    )?);
                }
            }
            Ok(CommandPlan { events, response: expired, http_status: 200 })
        })
    }

    pub fn read_observation(
        &self,
        workspace_id: &str,
        agent_id: &str,
        path: &str,
    ) -> StoreResult<Option<ReadObservationRecord>> {
        let path = normalized_scope(path)?;
        self.current_records(CurrentAggregate::ReadObservation, workspace_id)?
            .into_iter()
            .map(record_from_current::<ReadObservationRecord>)
            .collect::<StoreResult<Vec<_>>>()?
            .into_iter()
            .find(|record| record.agent_id == agent_id && record.path == path)
            .map(Ok)
            .transpose()
    }
}

pub(crate) fn read_event<T>(
    request: &RequestEnvelope<T>,
    ordinal: u32,
    now: OffsetDateTime,
    variant: fn(EventData) -> ReadObservationEvent,
    record: &ReadObservationRecord,
) -> StoreResult<NewEvent> {
    let mut data = EventData::new(match record.status {
        ReadObservationStatus::Started => record.operation_id.clone(),
        _ => record.path.clone(),
    });
    data.data = json!({"read_observation": record});
    NewEvent::new(request.request_id, ordinal, now, EventPayload::ReadObservation(variant(data)))
        .map_err(StoreError::from)
}
