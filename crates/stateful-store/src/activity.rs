use crate::{
    CommandOutcome, CommandPlan, Store, StoreResult,
    presence::{presence_event, register_record},
};
use serde::{Deserialize, Serialize};
use stateful_core::{EventData, EventPayload, PresenceEvent, PresencePhase, RequestEnvelope};

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
            let event = stateful_core::NewEvent::new(
                request.request_id,
                0,
                now,
                EventPayload::Presence(PresenceEvent::Finalized(data)),
            )?;
            Ok(CommandPlan {
                events: vec![event],
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
