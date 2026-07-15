use crate::{CommandOutcome, CommandPlan, ProjectionReader, Store, StoreResult};
use rusqlite::{OptionalExtension, params};
use serde_json::json;
use stateful_core::{
    ClaimEvent, EventData, EventPayload, ExplicitHandoff, HandoffEvent, HandoffRecord, HandoffStatus,
    NewEvent, PresenceEvent, PresenceRecord, PresenceResourceRelation, RequestEnvelope,
    ReservationEvent, WaitEvent, WriteFenceEvent, EXPLICIT_HANDOFF_RELEVANCE,
    FALLBACK_HANDOFF_RELEVANCE,
};
use time::OffsetDateTime;
use uuid::Uuid;

impl Store {
    pub fn finalize_handoff(
        &mut self,
        request: &RequestEnvelope<ExplicitHandoff>,
    ) -> StoreResult<CommandOutcome<HandoffRecord>> {
        request.payload.validate()?;
        let now = self.clock.now();
        self.execute_command(request, "handoff.finalize", |reader| {
            if let Some(existing) = reader.handoff(&request.workspace.workspace_id, &request.agent.agent_id)?
                && existing.explicit
            {
                return Ok(CommandPlan { events: vec![], response: existing, http_status: 200 });
            }
            let presence = reader.presence(&request.workspace.workspace_id, &request.agent.agent_id)?;
            let handoff = explicit_handoff_record(request, presence.as_ref(), now);
            let events = finalization_events(request, &handoff, 0)?;
            Ok(CommandPlan { events, response: handoff, http_status: 200 })
        })
    }

    pub fn stop_presence(
        &mut self,
        request: &RequestEnvelope<()>,
    ) -> StoreResult<CommandOutcome<Option<HandoffRecord>>> {
        let now = self.clock.now();
        self.execute_command(request, "presence.stop", |reader| {
            fallback_plan(request, reader, now, 0)
        })
    }

    pub fn expire_stale_presences(
        &mut self,
        request: &RequestEnvelope<()>,
    ) -> StoreResult<CommandOutcome<Vec<String>>> {
        let now = self.clock.now();
        self.execute_command(request, "presence.expire", |reader| {
            let mut events = Vec::new();
            let mut expired = Vec::new();
            for presence in reader.live_presences(&request.workspace.workspace_id)? {
                if presence.expires_at > now || presence.busy_until.is_some_and(|busy_until| busy_until > now) {
                    continue;
                }
                let fallback_request = request_for_agent(request, &presence);
                let plan = fallback_plan(&fallback_request, reader, now, events.len() as u32)?;
                if plan.response.is_some() {
                    expired.push(presence.agent_id);
                    events.extend(plan.events);
                }
            }
            Ok(CommandPlan { events, response: expired, http_status: 200 })
        })
    }

    pub fn expire_stale_handoffs(
        &mut self,
        request: &RequestEnvelope<()>,
    ) -> StoreResult<CommandOutcome<Vec<String>>> {
        let now = self.clock.now();
        self.execute_command(request, "handoff.expire", |reader| {
            let mut events = Vec::new();
            let mut expired = Vec::new();
            for handoff in reader.handoffs(&request.workspace.workspace_id)? {
                if handoff.expires_at > now {
                    continue;
                }
                let mut data = EventData::new(&handoff.agent_id);
                data.data = json!({"handoff": handoff});
                events.push(NewEvent::new(
                    request.request_id,
                    events.len() as u32,
                    now,
                    EventPayload::Handoff(HandoffEvent::Expired(data)),
                )?);
                expired.push(handoff.agent_id);
            }
            Ok(CommandPlan { events, response: expired, http_status: 200 })
        })
    }

    pub(crate) fn expire_current_state(&mut self, request: &RequestEnvelope<()>) -> StoreResult<()> {
        let now = self.clock.now();
        if !self.has_stale_current_state(&request.workspace.workspace_id, now)? {
            return Ok(());
        }
        self.execute_command(request, "presence.expire", |reader| {
            let mut events = Vec::new();
            for handoff in reader.handoffs(&request.workspace.workspace_id)? {
                if handoff.expires_at > now {
                    continue;
                }
                let mut data = EventData::new(&handoff.agent_id);
                data.data = json!({"handoff": handoff});
                events.push(NewEvent::new(
                    request.request_id,
                    events.len() as u32,
                    now,
                    EventPayload::Handoff(HandoffEvent::Expired(data)),
                )?);
            }
            for presence in reader.live_presences(&request.workspace.workspace_id)? {
                if presence.expires_at > now || presence.busy_until.is_some_and(|busy_until| busy_until > now) {
                    continue;
                }
                let fallback_request = request_for_agent(request, &presence);
                let plan = fallback_plan(&fallback_request, reader, now, events.len() as u32)?;
                events.extend(plan.events);
            }
            Ok(CommandPlan { events, response: (), http_status: 200 })
        })?;
        Ok(())
    }

    fn has_stale_current_state(&self, workspace_id: &str, now: OffsetDateTime) -> StoreResult<bool> {
        let now = crate::format_timestamp(now);
        self.conn.query_row(
            "SELECT EXISTS(
                SELECT 1 FROM presence_current
                WHERE workspace_id = ?1 AND json_extract(payload_json, '$.expires_at') <= ?2
                  AND (json_extract(payload_json, '$.busy_until') IS NULL OR json_extract(payload_json, '$.busy_until') <= ?2)
                UNION ALL
                SELECT 1 FROM handoff_current
                WHERE workspace_id = ?1 AND json_extract(payload_json, '$.expires_at') <= ?2
            )",
            params![workspace_id, now],
            |row| row.get::<_, bool>(0),
        ).map_err(crate::StoreError::from)
    }

    pub(crate) fn startup_housekeeping(&mut self) -> StoreResult<()> {
        let workspaces = {
            let mut statement = self.conn.prepare(
                "SELECT workspace_id FROM presence_current UNION SELECT workspace_id FROM handoff_current",
            )?;
            statement.query_map([], |row| row.get::<_, String>(0))?
                .collect::<Result<Vec<_>, _>>()?
        };
        for workspace_id in workspaces {
            self.expire_current_state(&self.system_maintenance_request(&workspace_id)?)?;
        }
        Ok(())
    }

    fn system_maintenance_request(&self, workspace_id: &str) -> StoreResult<RequestEnvelope<()>> {
        let identity = self.conn.query_row(
            "SELECT repo_id, worktree_id, root, branch FROM journal_events WHERE workspace_id = ?1 ORDER BY event_seq DESC LIMIT 1",
            [workspace_id],
            |row| Ok((
                row.get::<_, Option<String>>(0)?,
                row.get::<_, Option<String>>(1)?,
                row.get::<_, Option<String>>(2)?,
                row.get::<_, Option<String>>(3)?,
            )),
        ).optional()?;
        let (repo_id, worktree_id, root, branch) = identity.unwrap_or_default();
        RequestEnvelope::new(
            Uuid::new_v4(),
            self.clock.now(),
            stateful_core::AgentIdentity {
                agent_id: "stateful-maintenance".into(),
                turn_id: None,
                actor_id: "stateful-maintenance".into(),
                actor_type: stateful_core::ActorType::System,
                owner_id: None,
                parent_agent_id: None,
                parent_actor_id: None,
            },
            stateful_core::WorkspaceIdentity {
                root: root.unwrap_or_else(|| "/".into()),
                workspace_id: workspace_id.into(),
                repo_id: repo_id.unwrap_or_else(|| "unknown".into()),
                worktree_id: worktree_id.unwrap_or_else(|| "unknown".into()),
                branch: branch.unwrap_or_else(|| "unknown".into()),
            },
            stateful_core::SourceRef {
                kind: stateful_core::SourceKind::Server,
                event: "startup_maintenance".into(),
                tool_name: None,
                source_ref: "stateful.startup-maintenance".into(),
            },
            (),
        ).map_err(crate::StoreError::from)
    }

    pub fn handoff_for_request(
        &mut self,
        request: &RequestEnvelope<()>,
        agent_id: &str,
    ) -> StoreResult<Option<HandoffRecord>> {
        self.expire_current_state(request)?;
        self.handoff_record(&request.workspace.workspace_id, agent_id)
    }

    fn handoff_record(&self, workspace_id: &str, agent_id: &str) -> StoreResult<Option<HandoffRecord>> {
        self.conn.query_row(
            "SELECT payload_json, origin_event_seq FROM handoff_current WHERE workspace_id = ?1 AND aggregate_id = ?2",
            params![workspace_id, agent_id],
            |row| {
                let payload: String = row.get(0)?;
                let origin_event_seq = row.get(1)?;
                let mut handoff: HandoffRecord = serde_json::from_str(&payload).map_err(|error| rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(error)))?;
                handoff.origin_event_seq = origin_event_seq;
                Ok(handoff)
            },
        ).optional().map_err(crate::StoreError::from)
    }
}

fn fallback_plan(
    request: &RequestEnvelope<()>,
    reader: &dyn ProjectionReader,
    now: OffsetDateTime,
    ordinal: u32,
) -> StoreResult<CommandPlan<Option<HandoffRecord>>> {
    if let Some(existing) = reader.handoff(&request.workspace.workspace_id, &request.agent.agent_id)?
        && existing.expires_at > now
    {
        return Ok(CommandPlan { events: vec![], response: Some(existing), http_status: 200 });
    }
    let Some(presence) = reader.presence(&request.workspace.workspace_id, &request.agent.agent_id)? else {
        return Ok(CommandPlan { events: vec![], response: None, http_status: 200 });
    };
    let resources = reader.presence_resources(&request.workspace.workspace_id, &request.agent.agent_id)?;
    let handoff = fallback_handoff_record(request, &presence, resources, now);
    let events = finalization_events(request, &handoff, ordinal)?;
    Ok(CommandPlan { events, response: Some(handoff), http_status: 200 })
}

fn explicit_handoff_record(
    request: &RequestEnvelope<ExplicitHandoff>,
    presence: Option<&PresenceRecord>,
    now: OffsetDateTime,
) -> HandoffRecord {
    let (last_result, actor_id, actor_type, owner_id, parent_agent_id, parent_actor_id) = presence
        .map(|presence| (
            presence.last_result.clone(),
            presence.actor_id.clone(),
            presence.actor_type.clone(),
            presence.owner_id.clone(),
            presence.parent_agent_id.clone(),
            presence.parent_actor_id.clone(),
        ))
        .unwrap_or_else(|| (
            None,
            request.agent.actor_id.clone(),
            request.agent.actor_type.clone(),
            request.agent.owner_id.clone(),
            request.agent.parent_agent_id.clone(),
            request.agent.parent_actor_id.clone(),
        ));
    HandoffRecord {
        workspace_id: request.workspace.workspace_id.clone(),
        agent_id: request.agent.agent_id.clone(),
        actor_id,
        actor_type,
        owner_id,
        parent_agent_id,
        parent_actor_id,
        status: request.payload.status,
        summary: request.payload.summary.clone(),
        files_changed: request.payload.files_changed.clone(),
        tests_run: request.payload.tests_run.clone(),
        remaining_work: request.payload.remaining_work.clone(),
        next_plan: request.payload.next_plan.clone(),
        last_result,
        explicit: true,
        finalized_at: now,
        expires_at: now + EXPLICIT_HANDOFF_RELEVANCE,
        origin_event_seq: 0,
    }
}

fn fallback_handoff_record(
    request: &RequestEnvelope<()>,
    presence: &PresenceRecord,
    resources: Vec<stateful_core::PresenceResource>,
    now: OffsetDateTime,
) -> HandoffRecord {
    let files_changed = resources.into_iter()
        .filter(|resource| resource.relation == PresenceResourceRelation::Changed)
        .map(|resource| resource.relative_path)
        .collect();
    let remaining_work = presence.next_plan.iter().cloned().collect();
    HandoffRecord {
        workspace_id: request.workspace.workspace_id.clone(),
        agent_id: request.agent.agent_id.clone(),
        actor_id: presence.actor_id.clone(),
        actor_type: presence.actor_type.clone(),
        owner_id: presence.owner_id.clone(),
        parent_agent_id: presence.parent_agent_id.clone(),
        parent_actor_id: presence.parent_actor_id.clone(),
        status: HandoffStatus::Unknown,
        summary: "Session ended with no explicit handoff supplied.".into(),
        files_changed,
        tests_run: Vec::new(),
        remaining_work,
        next_plan: presence.next_plan.clone(),
        last_result: presence.last_result.clone(),
        explicit: false,
        finalized_at: now,
        expires_at: now + FALLBACK_HANDOFF_RELEVANCE,
        origin_event_seq: 0,
    }
}

fn finalization_events<T>(
    request: &RequestEnvelope<T>,
    handoff: &HandoffRecord,
    ordinal: u32,
) -> StoreResult<Vec<NewEvent>> {
    let now = handoff.finalized_at;
    let mut handoff_data = EventData::new(&handoff.agent_id);
    handoff_data.data = json!({"handoff": handoff});
    let mut presence_data = EventData::new(&handoff.agent_id);
    presence_data.data = json!({"agent_id": handoff.agent_id});
    let mut cleanup_data = EventData::new(&handoff.agent_id);
    cleanup_data.data = json!({"agent_id": handoff.agent_id, "cleanup": true});
    [
        EventPayload::Handoff(HandoffEvent::Finalized(handoff_data)),
        EventPayload::Presence(PresenceEvent::Finalized(presence_data)),
        EventPayload::Reservation(ReservationEvent::Released(cleanup_data.clone())),
        EventPayload::Claim(ClaimEvent::Released(cleanup_data.clone())),
        EventPayload::Wait(WaitEvent::Cancelled(cleanup_data.clone())),
        EventPayload::WriteFence(WriteFenceEvent::Released(cleanup_data)),
    ].into_iter().enumerate().map(|(index, payload)| {
        NewEvent::new(request.request_id, ordinal + index as u32, now, payload).map_err(crate::StoreError::from)
    }).collect()
}

fn request_for_agent(
    request: &RequestEnvelope<()>,
    presence: &PresenceRecord,
) -> RequestEnvelope<()> {
    let mut request = request.clone();
    request.agent.agent_id = presence.agent_id.clone();
    request
}
