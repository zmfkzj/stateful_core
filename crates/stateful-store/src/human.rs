use crate::{
    CommandOutcome, CommandPlan, CurrentAggregate, ReservationRecord, Store, StoreError,
    StoreResult,
    reservations::{expired, normalized_scope, record_from_current, timestamp, typed_records},
    write_fences::active_fence_owner,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use stateful_core::{
    EventData, EventPayload, HumanAcknowledgementEvent, HumanObservationEvent,
    ReadObservationRecord, ReconciliationDecision, RequestEnvelope, ReservationScope, V2Error,
};
use std::str::FromStr;
use time::{Duration, OffsetDateTime};
use uuid::Uuid;

const EPHEMERAL_OBSERVATION_TTL: Duration = Duration::minutes(5);
const HUMAN_WRITE_OBSERVATION_TTL: Duration = Duration::hours(24);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HumanObservationKind {
    Save,
    Change,
    Delete,
    Presence,
    Dirty,
}

impl HumanObservationKind {
    fn is_write(self) -> bool {
        matches!(self, Self::Save | Self::Change | Self::Delete)
    }
    fn is_ephemeral(self) -> bool {
        matches!(self, Self::Presence | Self::Dirty)
    }
}

impl FromStr for HumanObservationKind {
    type Err = String;
    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "save" => Ok(Self::Save),
            "change" => Ok(Self::Change),
            "delete" => Ok(Self::Delete),
            "presence" => Ok(Self::Presence),
            "dirty" => Ok(Self::Dirty),
            _ => Err(format!("unknown human observation kind: {value}")),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum HumanObservationConfidence {
    High,
    Low,
}

impl FromStr for HumanObservationConfidence {
    type Err = String;
    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "high" => Ok(Self::High),
            "low" => Ok(Self::Low),
            _ => Err(format!("unknown human observation confidence: {value}")),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HumanObservationInput {
    pub relative_path: String,
    pub kind: HumanObservationKind,
    pub confidence: HumanObservationConfidence,
    pub source: String,
    pub summary: String,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "time::serde::rfc3339::option"
    )]
    pub observed_at: Option<OffsetDateTime>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReconciliationAckInput {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reservation_id: Option<String>,
    pub decision: ReconciliationDecision,
    pub files_reread: Vec<String>,
    pub human_change_summary: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HumanObservationRecord {
    pub observation_id: String,
    pub workspace_id: String,
    pub relative_path: String,
    pub kind: HumanObservationKind,
    pub confidence: HumanObservationConfidence,
    pub source: String,
    pub observed_at: String,
    pub summary: String,
    pub status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reconciled_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decision: Option<ReconciliationDecision>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reconciled_by_agent_id: Option<String>,
    #[serde(default)]
    pub origin_event_seq: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HumanReconciliationAcknowledgementRecord {
    pub acknowledgement_id: String,
    pub workspace_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reservation_id: Option<String>,
    pub decision: ReconciliationDecision,
    pub files_reread: Vec<String>,
    pub human_change_summary: String,
    pub acknowledged_by_agent_id: String,
    pub acknowledged_at: String,
    #[serde(default)]
    pub origin_event_seq: u64,
}

impl Store {
    pub fn record_human_observation(
        &self,
        request: &RequestEnvelope<HumanObservationInput>,
    ) -> StoreResult<CommandOutcome<HumanObservationRecord>> {
        let now = self.clock.now();
        let payload = request.payload.clone();
        self.execute_command(request, "human.observe", |reader| {
            let relative_path = normalized_scope(&payload.relative_path)?;
            let observed_at = payload.observed_at.unwrap_or(now);
            let attributed_owner = (payload.kind.is_write()
                && payload.confidence == HumanObservationConfidence::High)
                .then(|| {
                    active_fence_owner(
                        reader,
                        &request.workspace.workspace_id,
                        &relative_path,
                        observed_at,
                    )
                })
                .transpose()?
                .flatten();
            let attributed = attributed_owner.is_some();
            let observation = HumanObservationRecord {
                observation_id: Uuid::new_v4().to_string(),
                workspace_id: request.workspace.workspace_id.clone(),
                relative_path,
                kind: payload.kind,
                confidence: payload.confidence,
                source: payload.source,
                observed_at: timestamp(observed_at)?,
                summary: payload.summary,
                status: if attributed {
                    "reconciled".into()
                } else {
                    "pending".into()
                },
                expires_at: Some(timestamp(
                    observed_at
                        + if payload.kind.is_ephemeral() {
                            EPHEMERAL_OBSERVATION_TTL
                        } else {
                            HUMAN_WRITE_OBSERVATION_TTL
                        },
                )?),
                reconciled_at: attributed.then(|| timestamp(observed_at)).transpose()?,
                decision: None,
                reconciled_by_agent_id: attributed_owner,
                origin_event_seq: 0,
            };
            Ok(CommandPlan {
                events: vec![observation_event(
                    request,
                    0,
                    now,
                    HumanObservationEvent::Observed,
                    &observation,
                )?],
                response: observation,
                http_status: 200,
            })
        })
    }

    pub fn acknowledge_human_reconciliation(
        &self,
        request: &RequestEnvelope<ReconciliationAckInput>,
    ) -> StoreResult<CommandOutcome<u64>> {
        let now = self.clock.now();
        let payload = request.payload.clone();
        self.execute_command(request, "human.reconcile", |reader| {
            let paths = payload.files_reread.iter()
                .map(|path| normalized_scope(path))
                .collect::<StoreResult<Vec<_>>>()?;
            if payload.decision.clears_human_write_block() {
                let reservation_id = payload.reservation_id.as_deref().ok_or_else(|| reconciliation_error(
                    "missing_reservation",
                    "Adopt and reapply reconciliation require an active reservation.",
                    "Declare or provide the active reservation before acknowledging the human change.",
                ))?;
                let reservation = typed_records::<ReservationRecord>(
                    reader,
                    CurrentAggregate::Reservation,
                    &request.workspace.workspace_id,
                )?
                .into_iter()
                .find(|reservation| reservation.reservation_id == reservation_id)
                .ok_or_else(|| reconciliation_error(
                    "missing_reservation",
                    "The supplied reservation does not exist in this workspace.",
                    "Provide an active reservation owned by the acknowledging agent.",
                ))?;
                if reservation.agent_id != request.agent.agent_id {
                    return Err(reconciliation_error(
                        "reservation_owner_mismatch",
                        "The supplied reservation belongs to another agent.",
                        "Use an active reservation owned by the acknowledging agent.",
                    ));
                }
                if reservation.status != "active"
                    || reservation.expires_at.as_deref().is_some_and(|expires_at| expired(expires_at, now))
                {
                    return Err(reconciliation_error(
                        "missing_reservation",
                        "The supplied reservation is not active.",
                        "Declare or refresh an active reservation before acknowledging the human change.",
                    ));
                }
                let rereads = typed_records::<ReadObservationRecord>(
                    reader,
                    CurrentAggregate::ReadObservation,
                    &request.workspace.workspace_id,
                )?;
                let human_writes = typed_records::<HumanObservationRecord>(
                    reader,
                    CurrentAggregate::HumanObservation,
                    &request.workspace.workspace_id,
                )?
                .into_iter()
                .filter(|observation| {
                    observation.status == "pending"
                        && observation.kind.is_write()
                        && observation.confidence == HumanObservationConfidence::High
                })
                .collect::<Vec<_>>();
                for path in &paths {
                    if !reservation.scopes.iter().any(|scope| {
                        matches!(scope, ReservationScope::File(scope_path) if scope_path == path)
                    }) {
                        return Err(reconciliation_error(
                            "scope_mismatch",
                            "Each reread file requires an exact active file reservation scope.",
                            "Declare exact file scopes for every reread path.",
                        ));
                    }
                    let reread = rereads.iter().find(|reread| {
                        reread.agent_id == request.agent.agent_id && reread.path == *path
                    }).ok_or_else(|| reconciliation_error(
                        "missing_read_provenance",
                        "Each reconciled human write requires a fresh exact reread.",
                        "Read the exact file completely before acknowledging the human change.",
                    ))?;
                    if !reread.is_fresh_at(now)
                        || human_writes.iter().any(|observation| {
                            observation.relative_path == *path
                                && reread.origin_event_seq <= observation.origin_event_seq
                        })
                    {
                        return Err(reconciliation_error(
                            "stale_observation",
                            "The supplied reread is stale, inexact, or predates the human change.",
                            "Perform a fresh complete exact reread after the human change before acknowledging it.",
                        ));
                    }
                }
            }
            let acknowledgement = HumanReconciliationAcknowledgementRecord {
                acknowledgement_id: request.request_id.to_string(),
                workspace_id: request.workspace.workspace_id.clone(),
                reservation_id: payload.reservation_id.clone(),
                decision: payload.decision,
                files_reread: paths.clone(),
                human_change_summary: payload.human_change_summary.clone(),
                acknowledged_by_agent_id: request.agent.agent_id.clone(),
                acknowledged_at: timestamp(now)?,
                origin_event_seq: 0,
            };
            let mut events = vec![acknowledgement_event(request, 0, now, &acknowledgement)?];
            let mut count = 0;
            if payload.decision.clears_human_write_block() && !paths.is_empty() {
                for mut observation in typed_records::<HumanObservationRecord>(
                    reader,
                    CurrentAggregate::HumanObservation,
                    &request.workspace.workspace_id,
                )? {
                    if observation.status != "pending"
                        || !paths.contains(&observation.relative_path)
                    {
                        continue;
                    }
                    observation.status = "reconciled".into();
                    observation.reconciled_at = Some(timestamp(now)?);
                    observation.reconciled_by_agent_id = Some(request.agent.agent_id.clone());
                    observation.decision = Some(payload.decision);
                    count += 1;
                    events.push(observation_event(
                        request,
                        events.len() as u32,
                        now,
                        HumanObservationEvent::Reconciled,
                        &observation,
                    )?);
                }
            } else if !paths.is_empty() {
                for mut observation in typed_records::<HumanObservationRecord>(
                    reader,
                    CurrentAggregate::HumanObservation,
                    &request.workspace.workspace_id,
                )? {
                    if observation.status != "pending"
                        || !paths.contains(&observation.relative_path)
                    {
                        continue;
                    }
                    observation.decision = Some(payload.decision);
                    events.push(observation_event(
                        request,
                        events.len() as u32,
                        now,
                        HumanObservationEvent::Reconciled,
                        &observation,
                    )?);
                }
            }
            Ok(CommandPlan { events, response: count, http_status: 200 })
        })
    }

    pub fn expire_human_observations(
        &self,
        request: &RequestEnvelope<()>,
    ) -> StoreResult<CommandOutcome<Vec<String>>> {
        let now = self.clock.now();
        self.execute_command(request, "human.expire", |reader| {
            let mut events = Vec::new();
            let mut expired_ids = Vec::new();
            for mut observation in typed_records::<HumanObservationRecord>(
                reader,
                CurrentAggregate::HumanObservation,
                &request.workspace.workspace_id,
            )? {
                if observation.status == "pending"
                    && observation
                        .expires_at
                        .as_deref()
                        .is_some_and(|value| expired(value, now))
                {
                    observation.status = "expired".into();
                    expired_ids.push(observation.observation_id.clone());
                    events.push(observation_event(
                        request,
                        events.len() as u32,
                        now,
                        HumanObservationEvent::Expired,
                        &observation,
                    )?);
                }
            }
            Ok(CommandPlan {
                events,
                response: expired_ids,
                http_status: 200,
            })
        })
    }

    pub fn unreconciled_human_observations(
        &self,
        workspace_id: &str,
        paths: &[String],
    ) -> StoreResult<Vec<HumanObservationRecord>> {
        let paths = paths
            .iter()
            .map(|path| normalized_scope(path))
            .collect::<StoreResult<Vec<_>>>()?;
        Ok(self
            .current_records(CurrentAggregate::HumanObservation, workspace_id)?
            .into_iter()
            .map(record_from_current::<HumanObservationRecord>)
            .collect::<StoreResult<Vec<_>>>()?
            .into_iter()
            .filter(|observation| {
                observation.status == "pending"
                    && observation.kind.is_write()
                    && observation.confidence == HumanObservationConfidence::High
                    && paths.contains(&observation.relative_path)
            })
            .collect())
    }

    pub fn human_reconciliation_acknowledgements(
        &self,
        workspace_id: &str,
    ) -> StoreResult<Vec<HumanReconciliationAcknowledgementRecord>> {
        self.current_records(CurrentAggregate::HumanAcknowledgement, workspace_id)?
            .into_iter()
            .map(record_from_current::<HumanReconciliationAcknowledgementRecord>)
            .collect()
    }
}

fn reconciliation_error(
    code: &'static str,
    message: &'static str,
    next_action: &'static str,
) -> StoreError {
    V2Error::new(code, message)
        .with_required_next_action(next_action)
        .into()
}

fn observation_event<T>(
    request: &RequestEnvelope<T>,
    ordinal: u32,
    now: OffsetDateTime,
    variant: fn(EventData) -> HumanObservationEvent,
    observation: &HumanObservationRecord,
) -> StoreResult<stateful_core::NewEvent> {
    let mut data = EventData::new(&observation.observation_id);
    data.data = json!({"observation": observation});
    stateful_core::NewEvent::new(
        request.request_id,
        ordinal,
        now,
        EventPayload::HumanObservation(variant(data)),
    )
    .map_err(StoreError::from)
}
fn acknowledgement_event<T>(
    request: &RequestEnvelope<T>,
    ordinal: u32,
    now: OffsetDateTime,
    acknowledgement: &HumanReconciliationAcknowledgementRecord,
) -> StoreResult<stateful_core::NewEvent> {
    let mut data = EventData::new(&acknowledgement.acknowledgement_id);
    data.data = json!({"acknowledgement": acknowledgement});
    stateful_core::NewEvent::new(
        request.request_id,
        ordinal,
        now,
        EventPayload::HumanAcknowledgement(HumanAcknowledgementEvent::Recorded(data)),
    )
    .map_err(StoreError::from)
}
