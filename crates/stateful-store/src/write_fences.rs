use crate::{
    CommandOutcome, CommandPlan, CurrentAggregate, ProjectionReader, Store, StoreError, StoreResult,
    reservations::{expired, normalized_scope, record_from_current, scopes_conflict, timestamp, typed_records},
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use stateful_core::{EventData, EventPayload, NewEvent, RequestEnvelope, WriteFenceEvent};
use time::{Duration, OffsetDateTime};
use uuid::Uuid;

const WRITE_FENCE_TTL: Duration = Duration::minutes(5);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WriteFenceAcquire {
    pub paths: Vec<String>,
    pub action: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WriteFenceRelease {
    pub fence_ids: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WriteFenceRecord {
    pub fence_id: String,
    pub agent_id: String,
    pub workspace_id: String,
    pub relative_path: String,
    pub action: String,
    pub status: String,
    pub acquired_at: String,
    pub expires_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub released_at: Option<String>,
    #[serde(default)]
    pub origin_event_seq: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WriteFenceAcquireResult {
    pub fences: Vec<WriteFenceRecord>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conflict: Option<WriteFenceConflict>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WriteFenceConflict {
    pub path: String,
    pub owner_agent_id: String,
}

impl Store {
    pub fn acquire_write_fences(
        &self,
        request: &RequestEnvelope<WriteFenceAcquire>,
    ) -> StoreResult<CommandOutcome<WriteFenceAcquireResult>> {
        let now = self.clock.now();
        let payload = request.payload.clone();
        self.execute_command(request, "write_fence.acquire", |reader| {
            if payload.paths.is_empty() { return Err(StoreError::MissingScope); }
            let paths = payload.paths.iter().map(|path| normalized_scope(path)).collect::<StoreResult<Vec<_>>>()?;
            let existing = typed_records::<WriteFenceRecord>(reader, CurrentAggregate::WriteFence, &request.workspace.workspace_id)?;
            if let Some(conflict) = existing.iter().find(|fence| {
                fence.status == "active"
                    && !expired(&fence.expires_at, now)
                    && fence.agent_id != request.agent.agent_id
                    && paths.iter().any(|path| scopes_conflict(&fence.relative_path, path))
            }) {
                let conflict = WriteFenceConflict { path: conflict.relative_path.clone(), owner_agent_id: conflict.agent_id.clone() };
                return Ok(CommandPlan {
                    events: Vec::new(),
                    response: WriteFenceAcquireResult { fences: Vec::new(), conflict: Some(conflict) },
                    http_status: 409,
                });
            }
            let mut events = Vec::new();
            let mut fences = Vec::new();
            for path in paths {
                if let Some(mut fence) = existing.iter().find(|fence| {
                    fence.status == "active"
                        && !expired(&fence.expires_at, now)
                        && fence.agent_id == request.agent.agent_id
                        && fence.relative_path == path
                        && fence.action == payload.action
                }).cloned() {
                    fence.expires_at = timestamp(now + WRITE_FENCE_TTL)?;
                    events.push(fence_event(request, events.len() as u32, now, WriteFenceEvent::Acquired, &fence)?);
                    fences.push(fence);
                    continue;
                }
                let fence = WriteFenceRecord {
                    fence_id: Uuid::new_v4().to_string(),
                    agent_id: request.agent.agent_id.clone(),
                    workspace_id: request.workspace.workspace_id.clone(),
                    relative_path: path,
                    action: payload.action.clone(),
                    status: "active".into(),
                    acquired_at: timestamp(now)?,
                    expires_at: timestamp(now + WRITE_FENCE_TTL)?,
                    released_at: None,
                    origin_event_seq: 0,
                };
                events.push(fence_event(request, events.len() as u32, now, WriteFenceEvent::Acquired, &fence)?);
                fences.push(fence);
            }
            Ok(CommandPlan { events, response: WriteFenceAcquireResult { fences, conflict: None }, http_status: 200 })
        })
    }

    pub fn release_write_fences(
        &self,
        request: &RequestEnvelope<WriteFenceRelease>,
    ) -> StoreResult<CommandOutcome<Vec<WriteFenceRecord>>> {
        let now = self.clock.now();
        let fence_ids = request.payload.fence_ids.clone();
        self.execute_command(request, "write_fence.release", |reader| {
            let existing = typed_records::<WriteFenceRecord>(reader, CurrentAggregate::WriteFence, &request.workspace.workspace_id)?;
            let mut released = Vec::new();
            let mut events = Vec::new();
            for fence_id in &fence_ids {
                let mut fence = existing.iter().find(|fence| fence.fence_id == *fence_id)
                    .cloned().ok_or(StoreError::ClaimNotFound)?;
                if fence.agent_id != request.agent.agent_id { return Err(StoreError::ClaimOwnerMismatch); }
                if fence.status == "active" {
                    fence.status = "released".into();
                    fence.released_at = Some(timestamp(now)?);
                    events.push(fence_event(request, events.len() as u32, now, WriteFenceEvent::Released, &fence)?);
                }
                released.push(fence);
            }
            Ok(CommandPlan { events, response: released, http_status: 200 })
        })
    }

    pub fn expire_write_fences(
        &self,
        request: &RequestEnvelope<()>,
    ) -> StoreResult<CommandOutcome<Vec<String>>> {
        let now = self.clock.now();
        self.execute_command(request, "write_fence.expire", |reader| {
            let mut events = Vec::new();
            let mut expired_fences = Vec::new();
            for mut fence in typed_records::<WriteFenceRecord>(reader, CurrentAggregate::WriteFence, &request.workspace.workspace_id)? {
                if fence.status == "active" && expired(&fence.expires_at, now) {
                    fence.status = "expired".into();
                    fence.released_at = Some(timestamp(now)?);
                    expired_fences.push(fence.fence_id.clone());
                    events.push(fence_event(request, events.len() as u32, now, WriteFenceEvent::Expired, &fence)?);
                }
            }
            Ok(CommandPlan { events, response: expired_fences, http_status: 200 })
        })
    }

    pub fn write_fence(&self, workspace_id: &str, fence_id: &str) -> StoreResult<Option<WriteFenceRecord>> {
        self.current_records(CurrentAggregate::WriteFence, workspace_id)?.into_iter()
            .map(record_from_current::<WriteFenceRecord>)
            .collect::<StoreResult<Vec<_>>>()?
            .into_iter()
            .find(|fence| fence.fence_id == fence_id)
            .map(Ok)
            .transpose()
    }
}

pub(crate) fn active_fence_owner(
    reader: &dyn ProjectionReader,
    workspace_id: &str,
    path: &str,
    observed_at: OffsetDateTime,
) -> StoreResult<Option<String>> {
    Ok(typed_records::<WriteFenceRecord>(reader, CurrentAggregate::WriteFence, workspace_id)?
        .into_iter()
        .find(|fence| {
            fence.status == "active"
                && parse_before_or_at(&fence.acquired_at, observed_at)
                && parse_after_or_at(&fence.expires_at, observed_at)
                && scopes_conflict(&fence.relative_path, path)
        })
        .map(|fence| fence.agent_id))
}

fn parse_before_or_at(value: &str, observed_at: OffsetDateTime) -> bool {
    crate::reservations::parse_time(value).is_ok_and(|time| time <= observed_at)
}

fn parse_after_or_at(value: &str, observed_at: OffsetDateTime) -> bool {
    crate::reservations::parse_time(value).is_ok_and(|time| time >= observed_at)
}

pub(crate) fn fence_event<T>(
    request: &RequestEnvelope<T>,
    ordinal: u32,
    now: OffsetDateTime,
    variant: fn(EventData) -> WriteFenceEvent,
    fence: &WriteFenceRecord,
) -> StoreResult<NewEvent> {
    let mut data = EventData::new(&fence.fence_id);
    data.data = json!({"write_fence": fence});
    NewEvent::new(request.request_id, ordinal, now, EventPayload::WriteFence(variant(data))).map_err(StoreError::from)
}
