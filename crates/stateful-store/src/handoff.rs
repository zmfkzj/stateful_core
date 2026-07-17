use crate::{
    ClaimRecord, CommandOutcome, CommandPlan, CurrentAggregate, ProjectionReader,
    ReservationRecord, Store, StoreError, StoreResult, WaitRecord, WriteFenceRecord,
    claims::claim_event,
    reservations::{
        append_grant_for_path, reservation_event, scope_path, typed_records, wait_event,
    },
    write_fences::fence_event,
};
use rusqlite::{OptionalExtension, params};
use serde_json::json;
use stateful_core::{
    ClaimEvent, EXPLICIT_HANDOFF_RELEVANCE, EventData, EventPayload, ExplicitHandoff,
    FALLBACK_HANDOFF_RELEVANCE, HandoffEvent, HandoffRecord, HandoffStatus, NewEvent,
    PresenceEvent, PresenceRecord, PresenceResourceRelation, RequestEnvelope, ReservationEvent,
    WaitEvent, WriteFenceEvent,
};
use time::{Duration, OffsetDateTime, format_description::well_known::Rfc3339};
use uuid::Uuid;

pub(crate) fn lazy_current_state_events<T>(
    request: &RequestEnvelope<T>,
    reader: &dyn ProjectionReader,
    now: OffsetDateTime,
) -> StoreResult<Vec<NewEvent>> {
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
    let unit_request = RequestEnvelope {
        protocol_version: request.protocol_version,
        request_id: request.request_id,
        observed_at: request.observed_at,
        agent: request.agent.clone(),
        workspace: request.workspace.clone(),
        source: request.source.clone(),
        payload: (),
    };
    let stale_presences = reader
        .live_presences(&request.workspace.workspace_id)?
        .into_iter()
        .filter(|presence| {
            presence.expires_at <= now
                && presence
                    .busy_until
                    .is_none_or(|busy_until| busy_until <= now)
        })
        .collect::<Vec<_>>();
    let stale_agent_ids = stale_presences
        .iter()
        .map(|presence| presence.agent_id.as_str())
        .collect::<Vec<_>>();
    let released_reservations = typed_records::<ReservationRecord>(
        reader,
        CurrentAggregate::Reservation,
        &request.workspace.workspace_id,
    )?
    .into_iter()
    .filter(|reservation| {
        reservation.status == "active" && stale_agent_ids.contains(&reservation.agent_id.as_str())
    })
    .collect::<Vec<_>>();
    let cancelled_wait_ids = typed_records::<WaitRecord>(
        reader,
        CurrentAggregate::Wait,
        &request.workspace.workspace_id,
    )?
    .into_iter()
    .filter(|wait| {
        stale_agent_ids.contains(&wait.agent_id.as_str())
            && matches!(wait.status.as_str(), "queued" | "claimable")
    })
    .map(|wait| wait.wait_id)
    .collect::<Vec<_>>();
    for presence in stale_presences {
        let fallback_request = request_for_agent(&unit_request, &presence);
        let plan = fallback_plan_with_promotions(
            &fallback_request,
            reader,
            now,
            "ttl",
            events.len() as u32,
            false,
        )?;
        events.extend(plan.events);
    }
    append_waiter_promotions(
        &unit_request,
        reader,
        now,
        &request.workspace.workspace_id,
        &released_reservations,
        &cancelled_wait_ids,
        &mut events,
    )?;
    Ok(events)
}

impl Store {
    pub fn finalize_handoff(
        &mut self,
        request: &RequestEnvelope<ExplicitHandoff>,
    ) -> StoreResult<CommandOutcome<HandoffRecord>> {
        request.payload.validate()?;
        let now = self.clock.now();
        self.execute_command(request, "handoff.finalize", |reader| {
            let presence =
                reader.presence(&request.workspace.workspace_id, &request.agent.agent_id)?;
            if let Some(presence) = &presence {
                ensure_presence_owner(request, presence)?;
            }
            if presence.is_none()
                && let Some(existing) =
                    reader.handoff(&request.workspace.workspace_id, &request.agent.agent_id)?
            {
                ensure_handoff_owner(request, &existing)?;
                if existing.explicit {
                    return Ok(CommandPlan {
                        events: vec![],
                        response: existing,
                        http_status: 200,
                    });
                }
            }
            let handoff = explicit_handoff_record(request, presence.as_ref(), now);
            let events = finalization_events(request, reader, &handoff, None, 0, true)?;
            Ok(CommandPlan {
                events,
                response: handoff,
                http_status: 200,
            })
        })
    }

    pub fn stop_presence(
        &mut self,
        request: &RequestEnvelope<()>,
    ) -> StoreResult<CommandOutcome<Option<HandoffRecord>>> {
        let now = self.clock.now();
        self.execute_command(request, "presence.stop", |reader| {
            fallback_plan(request, reader, now, "stop", 0)
        })
    }

    pub fn expire_stale_presences(
        &mut self,
        request: &RequestEnvelope<()>,
    ) -> StoreResult<CommandOutcome<Vec<String>>> {
        let now = self.clock.now();
        self.execute_command(request, "presence.expire", |reader| {
            let mut events = Vec::new();
            let stale_presences = reader
                .live_presences(&request.workspace.workspace_id)?
                .into_iter()
                .filter(|presence| {
                    presence.expires_at <= now
                        && presence
                            .busy_until
                            .is_none_or(|busy_until| busy_until <= now)
                })
                .collect::<Vec<_>>();
            let stale_agent_ids = stale_presences
                .iter()
                .map(|presence| presence.agent_id.as_str())
                .collect::<Vec<_>>();
            let released_reservations = typed_records::<ReservationRecord>(
                reader,
                CurrentAggregate::Reservation,
                &request.workspace.workspace_id,
            )?
            .into_iter()
            .filter(|reservation| {
                reservation.status == "active"
                    && stale_agent_ids.contains(&reservation.agent_id.as_str())
            })
            .collect::<Vec<_>>();
            let cancelled_wait_ids = typed_records::<WaitRecord>(
                reader,
                CurrentAggregate::Wait,
                &request.workspace.workspace_id,
            )?
            .into_iter()
            .filter(|wait| {
                stale_agent_ids.contains(&wait.agent_id.as_str())
                    && matches!(wait.status.as_str(), "queued" | "claimable")
            })
            .map(|wait| wait.wait_id)
            .collect::<Vec<_>>();
            let mut expired = Vec::new();
            for presence in stale_presences {
                let fallback_request = request_for_agent(request, &presence);
                let plan = fallback_plan_with_promotions(
                    &fallback_request,
                    reader,
                    now,
                    "ttl",
                    events.len() as u32,
                    false,
                )?;
                if plan.response.is_some() {
                    expired.push(presence.agent_id);
                    events.extend(plan.events);
                }
            }
            append_waiter_promotions(
                request,
                reader,
                now,
                &request.workspace.workspace_id,
                &released_reservations,
                &cancelled_wait_ids,
                &mut events,
            )?;
            Ok(CommandPlan {
                events,
                response: expired,
                http_status: 200,
            })
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
            Ok(CommandPlan {
                events,
                response: expired,
                http_status: 200,
            })
        })
    }

    pub(crate) fn expire_current_state(
        &mut self,
        request: &RequestEnvelope<()>,
    ) -> StoreResult<()> {
        let now = self.clock.now();
        if !self.has_stale_current_state(&request.workspace.workspace_id, now)? {
            return Ok(());
        }
        self.execute_command(request, "presence.expire", |reader| {
            let events = lazy_current_state_events(request, reader, now)?;
            Ok(CommandPlan {
                events,
                response: (),
                http_status: 200,
            })
        })?;
        Ok(())
    }

    fn has_stale_current_state(
        &self,
        workspace_id: &str,
        now: OffsetDateTime,
    ) -> StoreResult<bool> {
        let mut statement = self.conn.prepare(
            "SELECT payload_json, 'presence' FROM presence_current WHERE workspace_id = ?1
             UNION ALL
             SELECT payload_json, 'handoff' FROM handoff_current WHERE workspace_id = ?1",
        )?;
        let rows = statement.query_map([workspace_id], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?;
        for row in rows {
            let (payload, kind) = row?;
            if kind == "presence" {
                let presence: PresenceRecord = serde_json::from_str(&payload)?;
                if presence.expires_at <= now
                    && presence
                        .busy_until
                        .is_none_or(|busy_until| busy_until <= now)
                {
                    return Ok(true);
                }
            } else {
                let handoff: HandoffRecord = serde_json::from_str(&payload)?;
                if handoff.expires_at <= now {
                    return Ok(true);
                }
            }
        }
        Ok(false)
    }

    pub(crate) fn startup_housekeeping(&mut self) -> StoreResult<()> {
        let workspaces = {
            let mut statement = self
                .conn
                .prepare("SELECT DISTINCT workspace_id FROM journal_events")?;
            statement
                .query_map([], |row| row.get::<_, String>(0))?
                .collect::<Result<Vec<_>, _>>()?
        };
        for workspace_id in workspaces {
            self.maintain_workspace(&workspace_id)?;
        }
        Ok(())
    }

    pub fn run_maintenance(&mut self) -> StoreResult<()> {
        self.startup_housekeeping()
    }

    fn maintain_workspace(&mut self, workspace_id: &str) -> StoreResult<()> {
        let request = self.system_maintenance_request(workspace_id)?;
        if self.has_stale_current_state(workspace_id, self.clock.now())? {
            self.expire_current_state(&maintenance_request(&request))?;
        }
        if self.has_expired_current_record(
            workspace_id,
            CurrentAggregate::Reservation,
            &["active"],
            "expires_at",
            None,
        )? || self.has_expired_current_record(
            workspace_id,
            CurrentAggregate::Wait,
            &["claimable"],
            "reservation_expires_at",
            None,
        )? {
            self.expire_reservations(&maintenance_request(&request))?;
        }
        if self.has_expired_current_record(
            workspace_id,
            CurrentAggregate::Claim,
            &["active"],
            "expires_at",
            None,
        )? {
            self.expire_claims(&maintenance_request(&request))?;
        }
        if self.has_expired_current_record(
            workspace_id,
            CurrentAggregate::WriteFence,
            &["active"],
            "expires_at",
            None,
        )? {
            self.expire_write_fences(&maintenance_request(&request))?;
        }
        if self.has_expired_current_record(
            workspace_id,
            CurrentAggregate::ReadObservation,
            &["stabilized"],
            "expires_at",
            None,
        )? {
            self.expire_read_observations(&maintenance_request(&request))?;
        }
        if self.has_expired_current_record(
            workspace_id,
            CurrentAggregate::HumanObservation,
            &["pending"],
            "expires_at",
            None,
        )? {
            self.expire_human_observations(&maintenance_request(&request))?;
        }
        if self.has_expired_current_record(
            workspace_id,
            CurrentAggregate::Notification,
            &["queued"],
            "expires_at",
            None,
        )? {
            self.expire_notifications(&maintenance_request(&request))?;
        }
        if self.has_expired_current_record(
            workspace_id,
            CurrentAggregate::ContextDelivery,
            &["pending", "superseded"],
            "expires_at",
            None,
        )? {
            self.expire_context_deliveries(&maintenance_request(&request))?;
        }
        Ok(())
    }

    fn has_expired_current_record(
        &self,
        workspace_id: &str,
        aggregate: CurrentAggregate,
        statuses: &[&str],
        timestamp_field: &str,
        grace: Option<Duration>,
    ) -> StoreResult<bool> {
        let now = self.clock.now();
        Ok(self
            .current_records(aggregate, workspace_id)?
            .into_iter()
            .any(|record| {
                let Some(status) = record
                    .payload
                    .get("status")
                    .and_then(serde_json::Value::as_str)
                else {
                    return false;
                };
                let Some(timestamp) = record
                    .payload
                    .get(timestamp_field)
                    .and_then(serde_json::Value::as_str)
                else {
                    return false;
                };
                statuses.contains(&status)
                    && OffsetDateTime::parse(timestamp, &Rfc3339)
                        .is_ok_and(|timestamp| timestamp + grace.unwrap_or(Duration::ZERO) <= now)
            }))
    }
    pub(crate) fn system_maintenance_request(
        &self,
        workspace_id: &str,
    ) -> StoreResult<RequestEnvelope<()>> {
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
        )
        .map_err(crate::StoreError::from)
    }

    pub fn handoff_for_request(
        &mut self,
        request: &RequestEnvelope<()>,
        agent_id: &str,
    ) -> StoreResult<Option<HandoffRecord>> {
        self.expire_current_state(request)?;
        self.handoff_record(&request.workspace.workspace_id, agent_id)
    }

    fn handoff_record(
        &self,
        workspace_id: &str,
        agent_id: &str,
    ) -> StoreResult<Option<HandoffRecord>> {
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

pub(crate) fn fallback_plan(
    request: &RequestEnvelope<()>,
    reader: &dyn ProjectionReader,
    now: OffsetDateTime,
    fallback_cause: &'static str,
    ordinal: u32,
) -> StoreResult<CommandPlan<Option<HandoffRecord>>> {
    fallback_plan_with_promotions(request, reader, now, fallback_cause, ordinal, true)
}

fn fallback_plan_with_promotions(
    request: &RequestEnvelope<()>,
    reader: &dyn ProjectionReader,
    now: OffsetDateTime,
    fallback_cause: &'static str,
    ordinal: u32,
    promote_waiters: bool,
) -> StoreResult<CommandPlan<Option<HandoffRecord>>> {
    let presence = reader.presence(&request.workspace.workspace_id, &request.agent.agent_id)?;
    if let Some(presence) = &presence {
        ensure_presence_owner(request, presence)?;
    }
    if presence.is_none()
        && let Some(existing) =
            reader.handoff(&request.workspace.workspace_id, &request.agent.agent_id)?
        && existing.expires_at > now
    {
        ensure_handoff_owner(request, &existing)?;
        return Ok(CommandPlan {
            events: Vec::new(),
            response: Some(existing),
            http_status: 200,
        });
    }
    let Some(presence) = presence else {
        return Ok(CommandPlan {
            events: vec![],
            response: None,
            http_status: 200,
        });
    };
    let resources =
        reader.presence_resources(&request.workspace.workspace_id, &request.agent.agent_id)?;
    let handoff = fallback_handoff_record(request, &presence, resources, now);
    let events = finalization_events(
        request,
        reader,
        &handoff,
        Some(fallback_cause),
        ordinal,
        promote_waiters,
    )?;
    Ok(CommandPlan {
        events,
        response: Some(handoff),
        http_status: 200,
    })
}

fn explicit_handoff_record(
    request: &RequestEnvelope<ExplicitHandoff>,
    presence: Option<&PresenceRecord>,
    now: OffsetDateTime,
) -> HandoffRecord {
    let (last_result, actor_id, actor_type, owner_id, parent_agent_id, parent_actor_id) = presence
        .map(|presence| {
            (
                presence.last_result.clone(),
                presence.actor_id.clone(),
                presence.actor_type.clone(),
                presence.owner_id.clone(),
                presence.parent_agent_id.clone(),
                presence.parent_actor_id.clone(),
            )
        })
        .unwrap_or_else(|| {
            (
                None,
                request.agent.actor_id.clone(),
                request.agent.actor_type.clone(),
                request.agent.owner_id.clone(),
                request.agent.parent_agent_id.clone(),
                request.agent.parent_actor_id.clone(),
            )
        });
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

fn maintenance_request(request: &RequestEnvelope<()>) -> RequestEnvelope<()> {
    let mut request = request.clone();
    request.request_id = Uuid::new_v4();
    request
}

fn fallback_handoff_record(
    request: &RequestEnvelope<()>,
    presence: &PresenceRecord,
    resources: Vec<stateful_core::PresenceResource>,
    now: OffsetDateTime,
) -> HandoffRecord {
    let files_changed = resources
        .into_iter()
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
    reader: &dyn ProjectionReader,
    handoff: &HandoffRecord,
    fallback_cause: Option<&str>,
    ordinal: u32,
    promote_waiters: bool,
) -> StoreResult<Vec<NewEvent>> {
    let now = handoff.finalized_at;
    let mut handoff_data = EventData::new(&handoff.agent_id);
    handoff_data.data = match fallback_cause {
        Some(cause) => json!({"handoff": handoff, "fallback_cause": cause}),
        None => json!({"handoff": handoff}),
    };
    let mut events = vec![NewEvent::new(
        request.request_id,
        ordinal,
        now,
        EventPayload::Handoff(HandoffEvent::Finalized(handoff_data)),
    )?];
    events.extend(presence_finalization_events(
        request,
        reader,
        &handoff.agent_id,
        now,
        ordinal + 1,
        promote_waiters,
    )?);
    Ok(events)
}

pub(crate) fn presence_finalization_events<T>(
    request: &RequestEnvelope<T>,
    reader: &dyn ProjectionReader,
    agent_id: &str,
    now: OffsetDateTime,
    ordinal: u32,
    promote_waiters: bool,
) -> StoreResult<Vec<NewEvent>> {
    let workspace_id = &request.workspace.workspace_id;
    let mut presence_data = EventData::new(agent_id);
    presence_data.data = json!({"agent_id": agent_id});
    let mut events = vec![NewEvent::new(
        request.request_id,
        0,
        now,
        EventPayload::Presence(PresenceEvent::Finalized(presence_data)),
    )?];
    let mut released_reservations = Vec::new();
    for mut reservation in
        typed_records::<ReservationRecord>(reader, CurrentAggregate::Reservation, workspace_id)?
    {
        if reservation.agent_id == agent_id && reservation.status == "active" {
            reservation.status = "released".into();
            released_reservations.push(reservation.clone());
            events.push(reservation_event(
                request,
                events.len() as u32,
                now,
                ReservationEvent::Released,
                &reservation,
            )?);
        }
    }
    let released_reservation_ids = released_reservations
        .iter()
        .map(|reservation| reservation.reservation_id.clone())
        .collect::<Vec<_>>();
    for mut claim in typed_records::<ClaimRecord>(reader, CurrentAggregate::Claim, workspace_id)? {
        if claim.status == "active"
            && (claim.agent_id == agent_id
                || released_reservation_ids.contains(&claim.reservation_id))
        {
            claim.status = "released".into();
            events.push(claim_event(
                request,
                events.len() as u32,
                now,
                ClaimEvent::Released,
                &claim,
            )?);
        }
    }
    let mut cancelled_wait_ids = Vec::new();
    for mut wait in typed_records::<WaitRecord>(reader, CurrentAggregate::Wait, workspace_id)? {
        if wait.agent_id == agent_id && matches!(wait.status.as_str(), "queued" | "claimable") {
            wait.status = "canceled".into();
            wait.reservation_expires_at = None;
            cancelled_wait_ids.push(wait.wait_id.clone());
            events.push(wait_event(
                request,
                events.len() as u32,
                now,
                WaitEvent::Cancelled,
                &wait,
            )?);
        }
    }
    for mut fence in
        typed_records::<WriteFenceRecord>(reader, CurrentAggregate::WriteFence, workspace_id)?
    {
        if fence.status == "active" && fence.is_owned_by(&request.agent) {
            fence.status = "released".into();
            fence.released_at = Some(crate::reservations::timestamp(now)?);
            events.push(fence_event(
                request,
                events.len() as u32,
                now,
                WriteFenceEvent::Released,
                &fence,
            )?);
        }
    }
    if events.len() == 1 {
        let mut cleanup_data = EventData::new(agent_id);
        cleanup_data.data = json!({"agent_id": agent_id, "cleanup": true, "actor": request.agent});
        for payload in [
            EventPayload::Reservation(ReservationEvent::Released(cleanup_data.clone())),
            EventPayload::Claim(ClaimEvent::Released(cleanup_data.clone())),
            EventPayload::Wait(WaitEvent::Cancelled(cleanup_data.clone())),
            EventPayload::WriteFence(WriteFenceEvent::Released(cleanup_data)),
        ] {
            events.push(NewEvent::new(
                request.request_id,
                events.len() as u32,
                now,
                payload,
            )?);
        }
    }
    if promote_waiters {
        append_waiter_promotions(
            request,
            reader,
            now,
            workspace_id,
            &released_reservations,
            &cancelled_wait_ids,
            &mut events,
        )?;
    }
    for event in &mut events {
        event.event_ordinal += ordinal;
    }
    Ok(events)
}

fn append_waiter_promotions<T>(
    request: &RequestEnvelope<T>,
    reader: &dyn ProjectionReader,
    now: OffsetDateTime,
    workspace_id: &str,
    released_reservations: &[ReservationRecord],
    cancelled_wait_ids: &[String],
    events: &mut Vec<NewEvent>,
) -> StoreResult<()> {
    let released_reservation_ids = released_reservations
        .iter()
        .map(|reservation| reservation.reservation_id.clone())
        .collect::<Vec<_>>();
    for reservation in released_reservations {
        for scope in &reservation.scopes {
            append_grant_for_path(
                request,
                reader,
                now,
                workspace_id,
                &scope_path(scope),
                &released_reservation_ids,
                cancelled_wait_ids,
                true,
                events,
            )?;
        }
    }
    Ok(())
}

pub(crate) fn ensure_presence_owner<T>(
    request: &RequestEnvelope<T>,
    presence: &PresenceRecord,
) -> StoreResult<()> {
    if presence.agent_id != request.agent.agent_id
        || presence.actor_id != request.agent.actor_id
        || presence.actor_type != request.agent.actor_type
        || presence.owner_id != request.agent.owner_id
        || presence.parent_agent_id != request.agent.parent_agent_id
        || presence.parent_actor_id != request.agent.parent_actor_id
    {
        return Err(StoreError::ReservationOwnerMismatch);
    }
    Ok(())
}

pub(crate) fn ensure_handoff_owner<T>(
    request: &RequestEnvelope<T>,
    handoff: &HandoffRecord,
) -> StoreResult<()> {
    if handoff.agent_id != request.agent.agent_id
        || handoff.actor_id != request.agent.actor_id
        || handoff.actor_type != request.agent.actor_type
        || handoff.owner_id != request.agent.owner_id
        || handoff.parent_agent_id != request.agent.parent_agent_id
        || handoff.parent_actor_id != request.agent.parent_actor_id
    {
        return Err(StoreError::ReservationOwnerMismatch);
    }
    Ok(())
}

fn request_for_agent(
    request: &RequestEnvelope<()>,
    presence: &PresenceRecord,
) -> RequestEnvelope<()> {
    let mut request = request.clone();
    request.agent.agent_id = presence.agent_id.clone();
    request.agent.actor_id = presence.actor_id.clone();
    request.agent.actor_type = presence.actor_type.clone();
    request.agent.owner_id = presence.owner_id.clone();
    request.agent.parent_agent_id = presence.parent_agent_id.clone();
    request.agent.parent_actor_id = presence.parent_actor_id.clone();
    request
}
