use crate::{
    CommandOutcome, CommandPlan, Store, StoreResult,
    presence::{presence_event, register_record},
};
use serde::{Deserialize, Serialize};
use stateful_core::{PresenceEvent, PresencePhase, RequestEnvelope};

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
            if let Some(presence) = &existing {
                crate::handoff::ensure_presence_owner(request, presence)?;
            } else if let Some(handoff) = reader.handoff(&request.workspace.workspace_id, &request.agent.agent_id)?
                && handoff.expires_at > now
            {
                crate::handoff::ensure_handoff_owner(request, &handoff)?;
            }
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
        let finalization_request = RequestEnvelope {
            protocol_version: request.protocol_version,
            request_id: request.request_id,
            observed_at: request.observed_at,
            agent: request.agent.clone(),
            workspace: request.workspace.clone(),
            source: request.source.clone(),
            payload: (),
        };
        self.execute_command(request, "activity.finalize", |reader| {
            if reader.presence(&request.workspace.workspace_id, &request.agent.agent_id)?.is_none() {
                return Ok(CommandPlan {
                    events: Vec::new(),
                    response: ActivityOutcome { finalized: false },
                    http_status: 200,
                });
            }
            let plan = crate::handoff::fallback_plan(&finalization_request, reader, now, "stop", 0)?;
            Ok(CommandPlan {
                events: plan.events,
                response: ActivityOutcome { finalized: plan.response.is_some() },
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
