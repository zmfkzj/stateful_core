use crate::{
    ClaimRecord, CommandOutcome, CommandPlan, CurrentAggregate, ReservationRecord, Store, StoreResult,
    WaitRecord, WriteFenceRecord,
    claims::claim_event,
    presence::{presence_event, register_record},
    reservations::{append_grant_for_path, reservation_event, typed_records, wait_event},
    write_fences::fence_event,
};
use serde::{Deserialize, Serialize};
use stateful_core::{ClaimEvent, EventData, EventPayload, PresenceEvent, PresencePhase, RequestEnvelope, ReservationEvent, WaitEvent, WriteFenceEvent};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActivityStart {
    pub phase: PresencePhase,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct ActivityFinalization {}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActivityOutcome {
    pub finalized: bool,
}

impl Store {
    pub fn start_activity(
        &mut self,
        request: &RequestEnvelope<ActivityStart>,
    ) -> StoreResult<CommandOutcome<stateful_core::PresenceRecord>> {
        let now = self.clock.now();
        let phase = request.payload.phase;
        self.execute_command(request, "activity.start", |reader| {
            let existing = reader.presence(&request.workspace.workspace_id, &request.agent.agent_id)?;
            let mut activity = register_record(request, existing, None, now);
            activity.phase = Some(phase);
            let event = presence_event(request, 0, now, PresenceEvent::PhaseUpdated, activity.clone(), false)?;
            Ok(CommandPlan { events: vec![event], response: activity, http_status: 200 })
        })
    }

    pub fn finalize_activity(
        &mut self,
        request: &RequestEnvelope<ActivityFinalization>,
    ) -> StoreResult<CommandOutcome<ActivityOutcome>> {
        let now = self.clock.now();
        self.execute_command(request, "activity.finalize", |reader| {
            if reader.presence(&request.workspace.workspace_id, &request.agent.agent_id)?.is_none() {
                return Ok(CommandPlan {
                    events: Vec::new(),
                    response: ActivityOutcome { finalized: false },
                    http_status: 200,
                });
            }
            let mut data = EventData::new(&request.agent.agent_id);
            data.data = serde_json::json!({"agent_id": request.agent.agent_id, "status": "finalized"});
            let mut events = vec![stateful_core::NewEvent::new(
                request.request_id, 0, now, EventPayload::Presence(PresenceEvent::Finalized(data)),
            )?];
            let workspace_id = &request.workspace.workspace_id;
            let mut released_reservations = Vec::new();
            for mut reservation in typed_records::<ReservationRecord>(reader, CurrentAggregate::Reservation, workspace_id)? {
                if reservation.agent_id == request.agent.agent_id && reservation.status == "active" {
                    reservation.status = "released".into();
                    released_reservations.push(reservation.clone());
                    events.push(reservation_event(request, events.len() as u32, now, ReservationEvent::Released, &reservation)?);
                }
            }
            let released_reservation_ids = released_reservations.iter()
                .map(|reservation| reservation.reservation_id.clone())
                .collect::<Vec<_>>();
            for mut claim in typed_records::<ClaimRecord>(reader, CurrentAggregate::Claim, workspace_id)? {
                if claim.agent_id == request.agent.agent_id && claim.status == "active" {
                    claim.status = "released".into();
                    events.push(claim_event(request, events.len() as u32, now, ClaimEvent::Released, &claim)?);
                }
            }
            let mut cancelled_wait_ids = Vec::new();
            for mut wait in typed_records::<WaitRecord>(reader, CurrentAggregate::Wait, workspace_id)? {
                if wait.agent_id == request.agent.agent_id && matches!(wait.status.as_str(), "queued" | "claimable") {
                    wait.status = "canceled".into();
                    wait.reservation_expires_at = None;
                    cancelled_wait_ids.push(wait.wait_id.clone());
                    events.push(wait_event(request, events.len() as u32, now, WaitEvent::Cancelled, &wait)?);
                }
            }
            for mut fence in typed_records::<WriteFenceRecord>(reader, CurrentAggregate::WriteFence, workspace_id)? {
                if fence.agent_id == request.agent.agent_id && fence.status == "active" {
                    fence.status = "released".into();
                    fence.released_at = Some(crate::reservations::timestamp(now)?);
                    events.push(fence_event(request, events.len() as u32, now, WriteFenceEvent::Released, &fence)?);
                }
            }
            for reservation in released_reservations {
                append_grant_for_path(
                    request,
                    reader,
                    now,
                    workspace_id,
                    &reservation.relative_path,
                    &released_reservation_ids,
                    &cancelled_wait_ids,
                    true,
                    &mut events,
                )?;
            }
            Ok(CommandPlan {
                events,
                response: ActivityOutcome { finalized: true },
                http_status: 200,
            })
        })
    }

    pub fn activity_count(&self, workspace_id: &str) -> StoreResult<u64> {
        self.conn
            .query_row(
                "SELECT COUNT(*) FROM presence_current WHERE workspace_id = ?1",
                [workspace_id],
                |row| row.get(0),
            )
            .map_err(crate::StoreError::from)
    }
}
