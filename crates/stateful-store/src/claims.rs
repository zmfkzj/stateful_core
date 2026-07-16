use crate::{
    CommandOutcome, CommandPlan, CurrentAggregate, Store, StoreError, StoreResult,
    ReservationRecord, WaitRecord,
    reservations::{append_grant_for_path, coordination_warning_event, expired_optional, normalized_scope, overlap_warning, record_from_current, reservation_event, scope_path, scopes_conflict, timestamp, typed_records, wait_event},
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use stateful_core::{ClaimEvent, Decision, EventData, EventPayload, NewEvent, RequestEnvelope, ReservationEvent, WaitEvent};
use time::{Duration, OffsetDateTime};
use uuid::Uuid;

const CLAIM_TTL: Duration = Duration::minutes(5);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClaimObservation {
    pub exists: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_hash: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClaimPath {
    pub relative_path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub observation: Option<ClaimObservation>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClaimAcquire {
    pub reservation_id: String,
    pub paths: Vec<ClaimPath>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClaimRelease {
    pub claim_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClaimRecord {
    pub claim_id: String,
    pub reservation_id: String,
    pub agent_id: String,
    pub workspace_id: String,
    pub relative_path: String,
    pub action: String,
    pub status: String,
    pub acquired_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub observation: Option<ClaimObservation>,
    #[serde(default)]
    pub origin_event_seq: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClaimBatchAcquireResult {
    pub claims: Vec<ClaimRecord>,
    pub acquired: usize,
    pub already_held: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClaimAcquireResponse {
    #[serde(flatten)]
    pub claims: ClaimBatchAcquireResult,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decision: Option<Decision>,
}

impl Store {
    pub fn acquire_claim(
        &self,
        request: &RequestEnvelope<ClaimAcquire>,
    ) -> StoreResult<CommandOutcome<ClaimBatchAcquireResult>> {
        self.acquire_claim_with_mode(request, false).map(|outcome| CommandOutcome {
            response: outcome.response.claims,
            http_status: outcome.http_status,
            first_event_seq: outcome.first_event_seq,
            last_event_seq: outcome.last_event_seq,
            duplicate: outcome.duplicate,
        })
    }

    pub fn acquire_claim_with_mode(
        &self,
        request: &RequestEnvelope<ClaimAcquire>,
        awareness: bool,
    ) -> StoreResult<CommandOutcome<ClaimAcquireResponse>> {
        let now = self.clock.now();
        let payload = request.payload.clone();
        self.execute_command(request, "claim.acquire", |reader| {
            if payload.paths.is_empty() {
                return Err(StoreError::MissingScope);
            }
            let reservations = typed_records::<ReservationRecord>(reader, CurrentAggregate::Reservation, &request.workspace.workspace_id)?;
            let reservation = reservations.into_iter().find(|reservation| reservation.reservation_id == payload.reservation_id)
                .ok_or(StoreError::MissingReservation)?;
            if reservation.agent_id != request.agent.agent_id
                || reservation.status != "active"
                || expired_optional(reservation.expires_at.as_deref(), now)
            {
                return Err(StoreError::MissingReservation);
            }
            let normalized = payload.paths.iter().map(|path| normalized_scope(&path.relative_path)).collect::<StoreResult<Vec<_>>>()?;
            if normalized.iter().any(|path| !reservation.scopes.iter().any(|scope| scope_covers(&scope_path(scope), path))) {
                return Err(StoreError::MissingReservation);
            }
            if normalized.iter().enumerate().any(|(index, path)| {
                normalized[..index]
                    .iter()
                    .any(|other| scopes_conflict(other, path))
            }) {
                return Err(StoreError::ClaimConflict);
            }
            if let Some(path) = normalized.iter().find(|path| *path == "tmp" || *path == "tmp/") {
                return Err(StoreError::InvalidClaimPath(path.clone()));
            }
            let existing = typed_records::<ClaimRecord>(reader, CurrentAggregate::Claim, &request.workspace.workspace_id)?;
            let conflict = normalized.iter().any(|path| {
                existing.iter().any(|claim| {
                    claim.status == "active"
                        && !expired_optional(claim.expires_at.as_deref(), now)
                        && claim.agent_id != request.agent.agent_id
                        && scopes_conflict(&claim.relative_path, path)
                })
            });
            if conflict && !awareness {
                return Err(StoreError::ClaimConflict);
            }
            let decision = conflict.then(overlap_warning);
            let mut events = Vec::new();
            if let Some(decision) = &decision {
                events.push(coordination_warning_event(
                    request,
                    0,
                    now,
                    &reservation.reservation_id,
                    &reservation.action,
                    &normalized,
                    decision,
                )?);
            }
            let mut claims = Vec::new();
            let mut acquired = 0;
            let mut already_held = 0;
            for (input, relative_path) in payload.paths.iter().zip(normalized) {
                if let Some(mut claim) = existing.iter().find(|claim| {
                    claim.status == "active"
                        && !expired_optional(claim.expires_at.as_deref(), now)
                        && claim.agent_id == request.agent.agent_id
                        && claim.relative_path == relative_path
                }).cloned() {
                    already_held += 1;
                    if claim.observation != input.observation {
                        claim.observation = input.observation.clone();
                        claim.expires_at = Some(timestamp(now + CLAIM_TTL)?);
                        events.push(claim_event(request, events.len() as u32, now, ClaimEvent::ObservationRefreshed, &claim)?);
                    }
                    claims.push(claim);
                    continue;
                }
                let claim = ClaimRecord {
                    claim_id: Uuid::new_v4().to_string(),
                    reservation_id: reservation.reservation_id.clone(),
                    agent_id: request.agent.agent_id.clone(),
                    workspace_id: request.workspace.workspace_id.clone(),
                    relative_path,
                    action: reservation.action.clone(),
                    status: "active".into(),
                    acquired_at: timestamp(now)?,
                    expires_at: Some(timestamp(now + CLAIM_TTL)?),
                    observation: input.observation.clone(),
                    origin_event_seq: 0,
                };
                events.push(claim_event(request, events.len() as u32, now, ClaimEvent::Acquired, &claim)?);
                claims.push(claim);
                acquired += 1;
            }
            if let Some(wait_id) = &reservation.wait_id {
                if let Some(mut wait) = typed_records::<WaitRecord>(reader, CurrentAggregate::Wait, &request.workspace.workspace_id)?
                    .into_iter().find(|wait| wait.wait_id == *wait_id && wait.status == "claimable")
                {
                    wait.status = "claimed".into();
                    wait.reservation_expires_at = None;
                    events.push(wait_event(request, events.len() as u32, now, WaitEvent::Claimed, &wait)?);
                }
            }
            Ok(CommandPlan {
                events,
                response: ClaimAcquireResponse {
                    claims: ClaimBatchAcquireResult { claims, acquired, already_held },
                    decision,
                },
                http_status: 200,
            })
        })
    }

    pub fn release_claim(
        &self,
        request: &RequestEnvelope<ClaimRelease>,
    ) -> StoreResult<CommandOutcome<ClaimRecord>> {
        let now = self.clock.now();
        let claim_id = request.payload.claim_id.clone();
        self.execute_command(request, "claim.release", |reader| {
            let mut claim = typed_records::<ClaimRecord>(reader, CurrentAggregate::Claim, &request.workspace.workspace_id)?
                .into_iter()
                .find(|claim| claim.claim_id == claim_id)
                .ok_or(StoreError::ClaimNotFound)?;
            if claim.agent_id != request.agent.agent_id {
                return Err(StoreError::ClaimOwnerMismatch);
            }
            if claim.status != "active" {
                return Ok(CommandPlan { events: Vec::new(), response: claim, http_status: 200 });
            }
            claim.status = "released".into();
            let mut events = vec![claim_event(request, 0, now, ClaimEvent::Released, &claim)?];
            let has_active_sibling = typed_records::<ClaimRecord>(
                reader,
                CurrentAggregate::Claim,
                &request.workspace.workspace_id,
            )?
            .into_iter()
            .any(|other| {
                other.claim_id != claim.claim_id
                    && other.reservation_id == claim.reservation_id
                    && other.status == "active"
                    && !expired_optional(other.expires_at.as_deref(), now)
            });
            if has_active_sibling {
                return Ok(CommandPlan { events, response: claim, http_status: 200 });
            }
            if let Some(mut reservation) = typed_records::<ReservationRecord>(
                reader,
                CurrentAggregate::Reservation,
                &request.workspace.workspace_id,
            )?
            .into_iter()
            .find(|reservation| {
                reservation.reservation_id == claim.reservation_id
                    && reservation.status == "active"
            }) {
                reservation.status = "released".into();
                events.push(reservation_event(
                    request,
                    events.len() as u32,
                    now,
                    ReservationEvent::Released,
                    &reservation,
                )?);
                for scope in &reservation.scopes {
                    append_grant_for_path(
                        request,
                        reader,
                        now,
                        &reservation.workspace_id,
                        &scope_path(scope),
                        std::slice::from_ref(&reservation.reservation_id),
                        &[],
                        true,
                        &mut events,
                    )?;
                }
            }
            Ok(CommandPlan { events, response: claim, http_status: 200 })
        })
    }

    pub fn expire_claims(
        &self,
        request: &RequestEnvelope<()>,
    ) -> StoreResult<CommandOutcome<Vec<String>>> {
        let now = self.clock.now();
        self.execute_command(request, "claim.expire", |reader| {
            let mut events = Vec::new();
            let mut expired_claims = Vec::new();
            for mut claim in typed_records::<ClaimRecord>(reader, CurrentAggregate::Claim, &request.workspace.workspace_id)? {
                if claim.status == "active" && expired_optional(claim.expires_at.as_deref(), now) {
                    claim.status = "expired".into();
                    expired_claims.push(claim.claim_id.clone());
                    events.push(claim_event(request, events.len() as u32, now, ClaimEvent::Expired, &claim)?);
                }
            }
            Ok(CommandPlan { events, response: expired_claims, http_status: 200 })
        })
    }

    pub fn claim(&self, workspace_id: &str, claim_id: &str) -> StoreResult<Option<ClaimRecord>> {
        self.current_records(CurrentAggregate::Claim, workspace_id)?.into_iter()
            .map(record_from_current::<ClaimRecord>)
            .collect::<StoreResult<Vec<_>>>()?
            .into_iter()
            .find(|claim| claim.claim_id == claim_id)
            .map(Ok)
            .transpose()
    }

    pub fn active_claims_for_path(&self, workspace_id: &str, path: &str) -> StoreResult<Vec<ClaimRecord>> {
        let path = normalized_scope(path)?;
        let now = self.clock.now();
        let claims = self.current_records(CurrentAggregate::Claim, workspace_id)?
            .into_iter()
            .map(record_from_current::<ClaimRecord>)
            .collect::<StoreResult<Vec<_>>>()?
            .into_iter()
            .filter(|claim| claim.status == "active" && !expired_optional(claim.expires_at.as_deref(), now) && claim.relative_path == path)
            .collect();
        Ok(claims)
    }
}

pub(crate) fn claim_event<T>(
    request: &RequestEnvelope<T>,
    ordinal: u32,
    now: OffsetDateTime,
    variant: fn(EventData) -> ClaimEvent,
    claim: &ClaimRecord,
) -> StoreResult<NewEvent> {
    let mut data = EventData::new(&claim.claim_id);
    data.data = json!({"claim": claim});
    NewEvent::new(request.request_id, ordinal, now, EventPayload::Claim(variant(data))).map_err(StoreError::from)
}

fn scope_covers(scope: &str, path: &str) -> bool {
    if scope.ends_with('/') {
        path.starts_with(&format!("{}/", scope.trim_end_matches('/')))
    } else {
        scope == path
    }
}
