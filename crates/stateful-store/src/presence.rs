use crate::{CommandOutcome, CommandPlan, Store, StoreError, StoreResult};
use rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Serialize};
use serde_json::json;
use stateful_core::{
    EventData, EventPayload, NewEvent, PresenceEvent, PresencePhase, PresenceRecord,
    PresenceResource, PresenceResourceRelation, PresenceUpdate, RequestEnvelope, V2Error,
    BUSY_UNTIL_MAXIMUM, LAST_RESULT_MAX_SCALARS, PRESENCE_TTL,
};
use time::OffsetDateTime;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PresenceRegistration {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub first_prompt: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PresenceResourceUpdate {
    pub relative_path: String,
    pub relation: PresenceResourceRelation,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PresenceToolStart {
    pub tool_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none", with = "time::serde::rfc3339::option")]
    pub deadline: Option<OffsetDateTime>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PresenceToolResult {
    pub tool_name: String,
    pub outcome: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct StoredToolResult {
    tool_name: String,
    outcome: String,
    #[serde(with = "time::serde::rfc3339")]
    completed_at: OffsetDateTime,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    summary: Option<String>,
}

impl Store {
    pub fn register_presence(
        &mut self,
        request: &RequestEnvelope<PresenceRegistration>,
    ) -> StoreResult<CommandOutcome<PresenceRecord>> {
        self.register_or_resume_presence(request, "presence.register", false)
    }

    pub fn register_presence_via_update(
        &mut self,
        request: &RequestEnvelope<PresenceRegistration>,
    ) -> StoreResult<CommandOutcome<PresenceRecord>> {
        self.register_or_resume_presence(request, "presence.update.register", false)
    }

    pub fn resume_presence(
        &mut self,
        request: &RequestEnvelope<PresenceRegistration>,
    ) -> StoreResult<CommandOutcome<PresenceRecord>> {
        self.register_or_resume_presence(request, "presence.resume", true)
    }

    fn register_or_resume_presence(
        &mut self,
        request: &RequestEnvelope<PresenceRegistration>,
        route_kind: &'static str,
        is_resume: bool,
    ) -> StoreResult<CommandOutcome<PresenceRecord>> {
        let now = self.clock.now();
        let first_prompt = request
            .payload
            .first_prompt
            .as_ref()
            .map(|prompt| PresenceUpdate {
                goal_excerpt: Some(prompt.clone()),
                ..Default::default()
            }.normalized())
            .transpose()?
            .and_then(|update| update.goal_excerpt);
        self.execute_command(request, route_kind, |reader| {
            let existing = reader.presence(&request.workspace.workspace_id, &request.agent.agent_id)?;
            let repeated = existing.is_some();
            let presence = register_record(request, existing, first_prompt.clone(), now);
            let event = presence_event(
                request,
                0,
                now,
                if is_resume { PresenceEvent::Heartbeat } else { PresenceEvent::Registered },
                presence.clone(),
                repeated,
            )?;
            Ok(CommandPlan { events: vec![event], response: presence, http_status: 200 })
        })
    }

    pub fn heartbeat_presence(
        &mut self,
        request: &RequestEnvelope<()>,
    ) -> StoreResult<CommandOutcome<PresenceRecord>> {
        let now = self.clock.now();
        self.execute_command(request, "presence.heartbeat", |reader| {
            let mut presence = reader.presence(&request.workspace.workspace_id, &request.agent.agent_id)?
                .ok_or_else(missing_presence)?;
            refresh_presence(&mut presence, now);
            let event = presence_event(request, 0, now, PresenceEvent::Heartbeat, presence.clone(), true)?;
            Ok(CommandPlan { events: vec![event], response: presence, http_status: 200 })
        })
    }

    pub fn update_presence(
        &mut self,
        request: &RequestEnvelope<PresenceUpdate>,
    ) -> StoreResult<CommandOutcome<PresenceRecord>> {
        let now = self.clock.now();
        let update = request.payload.clone().normalized()?;
        if update.last_result.is_some() || update.busy_until.is_some() {
            return Err(StoreError::V2(V2Error::new(
                "tool_fields_require_tool_command",
                "last_result and busy_until may only be changed by typed tool commands.",
            )));
        }
        self.execute_command(request, "presence.update", |reader| {
            let mut presence = reader.presence(&request.workspace.workspace_id, &request.agent.agent_id)?
                .ok_or_else(missing_presence)?;
            if let Some(goal) = &update.goal_excerpt { presence.goal_excerpt = Some(goal.clone()); }
            if let Some(phase) = update.phase { presence.phase = Some(phase); }
            if let Some(plan) = &update.next_plan { presence.next_plan = Some(plan.clone()); }
            refresh_presence(&mut presence, now);
            let variant = if update.goal_excerpt.is_some() {
                PresenceEvent::GoalUpdated
            } else if update.phase.is_some() {
                PresenceEvent::PhaseUpdated
            } else {
                PresenceEvent::PlanUpdated
            };
            let event = presence_event(request, 0, now, variant, presence.clone(), false)?;
            Ok(CommandPlan { events: vec![event], response: presence, http_status: 200 })
        })
    }

    pub fn update_presence_resource(
        &mut self,
        request: &RequestEnvelope<PresenceResourceUpdate>,
    ) -> StoreResult<CommandOutcome<PresenceResource>> {
        let now = self.clock.now();
        self.execute_command(request, "presence.resource", |reader| {
            let mut presence = reader.presence(&request.workspace.workspace_id, &request.agent.agent_id)?
                .ok_or_else(missing_presence)?;
            let resource = PresenceResource::new(
                &request.workspace.workspace_id,
                &request.agent.agent_id,
                &request.payload.relative_path,
                request.payload.relation,
                now,
                0,
            )?;
            let repeated = reader.presence_resource(
                &request.workspace.workspace_id,
                &request.agent.agent_id,
                &resource.relative_path,
                resource.relation,
            )?.is_some();
            refresh_presence(&mut presence, now);
            let event = presence_resources_event(request, now, presence, resource.clone(), repeated)?;
            Ok(CommandPlan { events: vec![event], response: resource, http_status: 200 })
        })
    }

    pub fn start_presence_tool(
        &mut self,
        request: &RequestEnvelope<PresenceToolStart>,
    ) -> StoreResult<CommandOutcome<PresenceRecord>> {
        let now = self.clock.now();
        validate_tool_name(&request.payload.tool_name)?;
        self.execute_command(request, "presence.tool_start", |reader| {
            let mut presence = reader.presence(&request.workspace.workspace_id, &request.agent.agent_id)?
                .ok_or_else(missing_presence)?;
            let maximum = now + BUSY_UNTIL_MAXIMUM;
            let requested = request.payload.deadline.map(|deadline| deadline.min(maximum));
            presence.busy_until = match (presence.busy_until, requested) {
                (Some(active), Some(requested)) => Some(active.min(requested)),
                (Some(active), None) => Some(active),
                (None, requested) => requested,
            };
            if recognized_test_command(&request.payload.tool_name) {
                presence.phase = Some(PresencePhase::Testing);
            }
            refresh_presence(&mut presence, now);
            let event = presence_event(request, 0, now, PresenceEvent::ToolStarted, presence.clone(), false)?;
            Ok(CommandPlan { events: vec![event], response: presence, http_status: 200 })
        })
    }

    pub fn complete_presence_tool(
        &mut self,
        request: &RequestEnvelope<PresenceToolResult>,
    ) -> StoreResult<CommandOutcome<PresenceRecord>> {
        let now = self.clock.now();
        validate_tool_name(&request.payload.tool_name)?;
        validate_tool_name(&request.payload.outcome)?;
        if let Some(summary) = &request.payload.summary {
            if summary.chars().count() > LAST_RESULT_MAX_SCALARS {
                return Err(StoreError::V2(V2Error::new(
                    "last_result_too_long",
                    format!("last_result must contain at most {LAST_RESULT_MAX_SCALARS} Unicode scalar values."),
                )));
            }
        }
        let result = serde_json::to_string(&StoredToolResult {
            tool_name: request.payload.tool_name.clone(),
            outcome: request.payload.outcome.clone(),
            completed_at: now,
            summary: request.payload.summary.clone(),
        })?;
        self.execute_command(request, "presence.tool_result", |reader| {
            let mut presence = reader.presence(&request.workspace.workspace_id, &request.agent.agent_id)?
                .ok_or_else(missing_presence)?;
            presence.last_result = Some(result.clone());
            presence.busy_until = None;
            refresh_presence(&mut presence, now);
            let event = presence_event(request, 0, now, PresenceEvent::ToolCompleted, presence.clone(), false)?;
            Ok(CommandPlan { events: vec![event], response: presence, http_status: 200 })
        })
    }

    fn presence_record(&self, workspace_id: &str, agent_id: &str) -> StoreResult<Option<PresenceRecord>> {
        self.conn.query_row(
            "SELECT payload_json, origin_event_seq FROM presence_current WHERE workspace_id = ?1 AND agent_id = ?2",
            params![workspace_id, agent_id],
            |row| {
                let payload: String = row.get(0)?;
                let origin_event_seq = row.get(1)?;
                let mut record: PresenceRecord = serde_json::from_str(&payload).map_err(|error| rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(error)))?;
                record.origin_event_seq = origin_event_seq;
                Ok(record)
            },
        ).optional().map_err(StoreError::from)
    }

    pub fn presence_for_request(
        &mut self,
        request: &RequestEnvelope<()>,
        agent_id: &str,
    ) -> StoreResult<Option<PresenceRecord>> {
        self.expire_current_state(request)?;
        self.presence_record(&request.workspace.workspace_id, agent_id)
    }

    fn presence_resources(&self, workspace_id: &str, agent_id: &str) -> StoreResult<Vec<PresenceResource>> {
        let mut statement = self.conn.prepare(
            "SELECT payload_json, origin_event_seq FROM presence_resource_current WHERE workspace_id = ?1 AND agent_id = ?2 ORDER BY relative_path, relation",
        )?;
        statement.query_map(params![workspace_id, agent_id], |row| {
            let payload: String = row.get(0)?;
            let origin_event_seq = row.get(1)?;
            let mut resource: PresenceResource = serde_json::from_str(&payload).map_err(|error| rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(error)))?;
            resource.origin_event_seq = origin_event_seq;
            Ok(resource)
        })?.collect::<Result<Vec<_>, _>>().map_err(StoreError::from)
    }

    fn presence_count(&self, workspace_id: &str) -> StoreResult<u64> {
        self.conn.query_row("SELECT COUNT(*) FROM presence_current WHERE workspace_id = ?1", [workspace_id], |row| row.get(0)).map_err(StoreError::from)
    }

    pub fn presence_resources_for_request(
        &mut self,
        request: &RequestEnvelope<()>,
        agent_id: &str,
    ) -> StoreResult<Vec<PresenceResource>> {
        self.expire_current_state(request)?;
        self.presence_resources(&request.workspace.workspace_id, agent_id)
    }

    pub fn presence_count_for_request(
        &mut self,
        request: &RequestEnvelope<()>,
    ) -> StoreResult<u64> {
        self.expire_current_state(request)?;
        self.presence_count(&request.workspace.workspace_id)
    }
}

pub(crate) fn register_record<T>(
    request: &RequestEnvelope<T>,
    existing: Option<PresenceRecord>,
    first_prompt: Option<String>,
    now: OffsetDateTime,
) -> PresenceRecord {
    let mut presence = existing.unwrap_or_else(|| PresenceRecord {
        workspace_id: request.workspace.workspace_id.clone(),
        agent_id: request.agent.agent_id.clone(),
        actor_id: request.agent.actor_id.clone(),
        actor_type: request.agent.actor_type.clone(),
        owner_id: request.agent.owner_id.clone(),
        parent_agent_id: request.agent.parent_agent_id.clone(),
        parent_actor_id: request.agent.parent_actor_id.clone(),
        goal_excerpt: None,
        phase: Some(PresencePhase::Exploring),
        next_plan: None,
        last_result: None,
        registered_at: now,
        updated_at: now,
        expires_at: now + PRESENCE_TTL,
        busy_until: None,
        origin_event_seq: 0,
    });
    presence.actor_id = request.agent.actor_id.clone();
    presence.actor_type = request.agent.actor_type.clone();
    presence.owner_id = request.agent.owner_id.clone();
    presence.parent_agent_id = request.agent.parent_agent_id.clone();
    presence.parent_actor_id = request.agent.parent_actor_id.clone();
    if presence.goal_excerpt.is_none() {
        presence.goal_excerpt = first_prompt;
    }
    refresh_presence(&mut presence, now);
    presence
}

pub(crate) fn refresh_presence(presence: &mut PresenceRecord, now: OffsetDateTime) {
    presence.updated_at = now;
    presence.expires_at = now + PRESENCE_TTL;
}

pub(crate) fn presence_event<T>(
    request: &RequestEnvelope<T>,
    ordinal: u32,
    now: OffsetDateTime,
    variant: fn(EventData) -> PresenceEvent,
    presence: PresenceRecord,
    repeated: bool,
) -> StoreResult<NewEvent> {
    let mut data = EventData::new(&request.agent.agent_id);
    data.repeated = repeated;
    data.data = json!({"presence": presence});
    NewEvent::new(request.request_id, ordinal, now, EventPayload::Presence(variant(data))).map_err(StoreError::from)
}

pub(crate) fn presence_resources_event<T>(
    request: &RequestEnvelope<T>,
    now: OffsetDateTime,
    presence: PresenceRecord,
    resource: PresenceResource,
    repeated: bool,
) -> StoreResult<NewEvent> {
    let mut data = EventData::new(&request.agent.agent_id);
    data.repeated = repeated;
    data.data = json!({"presence": presence, "resource": resource});
    NewEvent::new(request.request_id, 0, now, EventPayload::Presence(PresenceEvent::ResourcesUpdated(data))).map_err(StoreError::from)
}

pub(crate) fn missing_presence() -> StoreError {
    StoreError::V2(V2Error::new("presence_not_found", "a live presence is required for this command."))
}

pub(crate) fn recognized_test_command(tool_name: &str) -> bool {
    let command = tool_name.trim().to_ascii_lowercase();
    ["cargo test", "cargo nextest", "pytest", "go test", "npm test", "bun test", "yarn test", "pnpm test", "jest"]
        .iter()
        .any(|prefix| command == *prefix || command.starts_with(&format!("{prefix} ")))
}

fn validate_tool_name(value: &str) -> StoreResult<()> {
    if value.trim().is_empty() {
        return Err(StoreError::V2(V2Error::new("invalid_tool_result", "tool_name and outcome must not be empty.")));
    }
    Ok(())
}
