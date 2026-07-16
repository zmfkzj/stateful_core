use crate::{
    ClaimRecord, CommandOutcome, CommandPlan, CurrentAggregate, CurrentRecord, ProjectionReader, Store,
    StoreError, StoreResult,
    claims::claim_event,
    notifications::{DeliveryRecord, delivery_event},
};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::json;
use stateful_core::{
    ClaimEvent, EventData, EventPayload, NewEvent, NotificationEvent, RecoveryEvent, RequestEnvelope,
    ReservationEvent, ReservationScope, WaitEvent, normalize_relative_path,
};
use time::{Duration, OffsetDateTime};
use uuid::Uuid;

const RESERVATION_TTL: Duration = Duration::minutes(15);
const RESERVATION_MAX_LIFETIME: Duration = Duration::hours(1);
const CLAIMABLE_TTL: Duration = Duration::minutes(2);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReservationDeclaration {
    pub scopes: Vec<ReservationScope>,
    pub action: String,
    pub purpose: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReservationHeartbeat {
    pub reservation_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReservationRelease {
    pub reservation_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WaitRequest {
    pub relative_path: String,
    pub action: String,
    pub purpose: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blocking_agent_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WaitCancellation {
    pub wait_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WaitGrant {
    pub relative_path: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReservationRecord {
    pub reservation_id: String,
    pub agent_id: String,
    pub workspace_id: String,
    pub scopes: Vec<ReservationScope>,
    pub action: String,
    pub purpose: String,
    pub status: String,
    pub declared_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_expires_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wait_id: Option<String>,
    #[serde(default)]
    pub origin_event_seq: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WaitRecord {
    pub wait_id: String,
    pub request_id: String,
    pub agent_id: String,
    pub workspace_id: String,
    pub relative_path: String,
    pub action: String,
    pub purpose: String,
    pub status: String,
    pub requested_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reservation_expires_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blocking_agent_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reservation_id: Option<String>,
    #[serde(default)]
    pub origin_event_seq: u64,
}

impl Store {
    pub fn declare_reservation(
        &self,
        request: &RequestEnvelope<ReservationDeclaration>,
    ) -> StoreResult<CommandOutcome<ReservationRecord>> {
        let now = self.clock.now();
        let payload = request.payload.clone();
        self.execute_command(request, "reservation.declare", |reader| {
            let scopes = normalized_reservation_scopes(&payload.scopes)?;
            let purpose = required_purpose(&payload.purpose)?;
            let active = typed_records::<ReservationRecord>(
                reader,
                CurrentAggregate::Reservation,
                &request.workspace.workspace_id,
            )?;
            if active.iter().any(|reservation| {
                reservation.status == "active"
                    && !expired_optional(reservation.expires_at.as_deref(), now)
                    && reservation.agent_id != request.agent.agent_id
                    && reservation.scopes.iter().any(|existing| {
                        scopes.iter().any(|scope| scopes_conflict(&scope_path(existing), &scope_path(scope)))
                    })
            }) {
                return Err(StoreError::ReservationOwnerMismatch);
            }
            let reservation = reservation_record(
                request,
                request.request_id.to_string(),
                scopes,
                payload.action.clone(),
                purpose,
                now,
                None,
                RESERVATION_TTL,
            );
            Ok(CommandPlan {
                events: vec![reservation_event(request, 0, now, ReservationEvent::Declared, &reservation)?],
                response: reservation,
                http_status: 200,
            })
        })
    }

    pub fn heartbeat_reservation(
        &self,
        request: &RequestEnvelope<ReservationHeartbeat>,
    ) -> StoreResult<CommandOutcome<ReservationRecord>> {
        let now = self.clock.now();
        let reservation_id = request.payload.reservation_id.clone();
        self.execute_command(request, "reservation.heartbeat", |reader| {
            let mut reservation = typed_records::<ReservationRecord>(reader, CurrentAggregate::Reservation, &request.workspace.workspace_id)?
                .into_iter()
                .find(|record| record.reservation_id == reservation_id)
                .ok_or(StoreError::ReservationRequestNotFound)?;
            if reservation.agent_id != request.agent.agent_id {
                return Err(StoreError::ReservationOwnerMismatch);
            }
            if reservation.status != "active" || expired_optional(reservation.expires_at.as_deref(), now) {
                return Err(StoreError::ReservationRequestNotCancelable);
            }
            if let Some(maximum) = reservation.max_expires_at.as_deref().map(parse_time).transpose()? {
                reservation.expires_at = Some(timestamp((now + RESERVATION_TTL).min(maximum))?);
            }
            Ok(CommandPlan {
                events: vec![reservation_event(request, 0, now, ReservationEvent::Refreshed, &reservation)?],
                response: reservation,
                http_status: 200,
            })
        })
    }

    pub fn release_reservation(
        &self,
        request: &RequestEnvelope<ReservationRelease>,
    ) -> StoreResult<CommandOutcome<ReservationRecord>> {
        let now = self.clock.now();
        let reservation_id = request.payload.reservation_id.clone();
        self.execute_command(request, "reservation.release", |reader| {
            let mut reservation = typed_records::<ReservationRecord>(reader, CurrentAggregate::Reservation, &request.workspace.workspace_id)?
                .into_iter()
                .find(|record| record.reservation_id == reservation_id)
                .ok_or(StoreError::ReservationRequestNotFound)?;
            if reservation.agent_id != request.agent.agent_id {
                return Err(StoreError::ReservationOwnerMismatch);
            }
            if reservation.status != "active" {
                return Ok(CommandPlan { events: Vec::new(), response: reservation, http_status: 200 });
            }
            reservation.status = "released".into();
            let mut events = vec![reservation_event(request, 0, now, ReservationEvent::Released, &reservation)?];
            for mut claim in typed_records::<ClaimRecord>(reader, CurrentAggregate::Claim, &reservation.workspace_id)? {
                if claim.reservation_id == reservation.reservation_id && claim.status == "active" {
                    claim.status = "released".into();
                    events.push(claim_event(request, events.len() as u32, now, ClaimEvent::Released, &claim)?);
                }
            }
            for scope in &reservation.scopes {
                append_grant_for_path(
                    request, reader, now, &reservation.workspace_id, &scope_path(scope),
                    std::slice::from_ref(&reservation.reservation_id), &[], true, &mut events,
                )?;
            }
            Ok(CommandPlan { events, response: reservation, http_status: 200 })
        })
    }

    pub fn request_wait(
        &self,
        request: &RequestEnvelope<WaitRequest>,
    ) -> StoreResult<CommandOutcome<WaitRecord>> {
        let now = self.clock.now();
        let payload = request.payload.clone();
        self.execute_command(request, "wait.request", |reader| {
            let relative_path = normalized_wait_relative_path(&payload.relative_path, &payload.action)?;
            let purpose = required_purpose(&payload.purpose)?;
            let waits = typed_records::<WaitRecord>(reader, CurrentAggregate::Wait, &request.workspace.workspace_id)?;
            if let Some(existing) = waits.into_iter().find(|wait| {
                wait.agent_id == request.agent.agent_id
                    && wait.relative_path == relative_path
                    && wait.action == payload.action
                    && matches!(wait.status.as_str(), "queued" | "claimable")
            }) {
                return Ok(CommandPlan { events: Vec::new(), response: existing, http_status: 200 });
            }
            let wait = WaitRecord {
                wait_id: Uuid::new_v4().to_string(),
                request_id: request.request_id.to_string(),
                agent_id: request.agent.agent_id.clone(),
                workspace_id: request.workspace.workspace_id.clone(),
                relative_path,
                action: payload.action,
                purpose,
                status: "queued".into(),
                requested_at: timestamp(now)?,
                reservation_expires_at: None,
                blocking_agent_id: payload.blocking_agent_id,
                reservation_id: None,
                origin_event_seq: 0,
            };
            Ok(CommandPlan {
                events: vec![wait_event(request, 0, now, WaitEvent::Requested, &wait)?],
                response: wait,
                http_status: 200,
            })
        })
    }

    pub fn cancel_wait(
        &self,
        request: &RequestEnvelope<WaitCancellation>,
    ) -> StoreResult<CommandOutcome<WaitRecord>> {
        let now = self.clock.now();
        let wait_id = request.payload.wait_id.clone();
        self.execute_command(request, "wait.cancel", |reader| {
            let mut wait = typed_records::<WaitRecord>(reader, CurrentAggregate::Wait, &request.workspace.workspace_id)?
                .into_iter()
                .find(|record| record.wait_id == wait_id)
                .ok_or(StoreError::ReservationRequestNotFound)?;
            if wait.agent_id != request.agent.agent_id {
                return Err(StoreError::ReservationRequestOwnerMismatch);
            }
            if !matches!(wait.status.as_str(), "queued" | "claimable") {
                return Ok(CommandPlan { events: Vec::new(), response: wait, http_status: 200 });
            }
            let claimable_reservation = wait.reservation_id.clone();
            wait.status = "canceled".into();
            wait.reservation_expires_at = None;
            let mut events = vec![wait_event(request, 0, now, WaitEvent::Cancelled, &wait)?];
            if let Some(reservation_id) = claimable_reservation {
                if let Some(mut reservation) = typed_records::<ReservationRecord>(
                    reader, CurrentAggregate::Reservation, &request.workspace.workspace_id,
                )?.into_iter().find(|reservation| reservation.reservation_id == reservation_id && reservation.status == "active") {
                    reservation.status = "released".into();
                    events.push(reservation_event(request, events.len() as u32, now, ReservationEvent::Released, &reservation)?);
                    for scope in &reservation.scopes {
                        append_grant_for_path(
                            request, reader, now, &reservation.workspace_id, &scope_path(scope),
                            std::slice::from_ref(&reservation.reservation_id), &[], true, &mut events,
                        )?;
                    }
                }
            }
            Ok(CommandPlan { events, response: wait, http_status: 200 })
        })
    }

    pub fn grant_next_wait(
        &self,
        request: &RequestEnvelope<WaitGrant>,
    ) -> StoreResult<CommandOutcome<Option<WaitRecord>>> {
        let now = self.clock.now();
        let relative_path = normalized_scope(&request.payload.relative_path)?;
        self.execute_command(request, "wait.grant", |reader| {
            let mut events = Vec::new();
            let wait = append_grant_for_path(
                request, reader, now, &request.workspace.workspace_id, &relative_path, &[], &[], false, &mut events,
            )?;
            Ok(CommandPlan { events, response: wait, http_status: 200 })
        })
    }

    pub fn expire_reservations(
        &self,
        request: &RequestEnvelope<()>,
    ) -> StoreResult<CommandOutcome<Vec<String>>> {
        let now = self.clock.now();
        self.execute_command(request, "reservation.expire", |reader| {
            let mut events = Vec::new();
            let mut expired_ids = Vec::new();
            let mut released_paths = Vec::new();
            for mut reservation in typed_records::<ReservationRecord>(reader, CurrentAggregate::Reservation, &request.workspace.workspace_id)? {
                if reservation.status == "active" && expired_optional(reservation.expires_at.as_deref(), now) {
                    reservation.status = "expired".into();
                    for scope in &reservation.scopes {
                        let path = scope_path(scope);
                        if !released_paths.contains(&path) {
                            released_paths.push(path);
                        }
                    }
                    expired_ids.push(reservation.reservation_id.clone());
                    events.push(reservation_event(request, events.len() as u32, now, ReservationEvent::Expired, &reservation)?);
                }
            }
            for mut claim in typed_records::<ClaimRecord>(reader, CurrentAggregate::Claim, &request.workspace.workspace_id)? {
                if claim.status == "active" && expired_ids.contains(&claim.reservation_id) {
                    claim.status = "released".into();
                    events.push(claim_event(request, events.len() as u32, now, ClaimEvent::Released, &claim)?);
                }
            }
            for mut wait in typed_records::<WaitRecord>(reader, CurrentAggregate::Wait, &request.workspace.workspace_id)? {
                if wait.status == "claimable"
                    && wait.reservation_expires_at.as_deref().is_some_and(|value| expired(value, now))
                {
                    wait.status = "expired".into();
                    wait.reservation_expires_at = None;
                    events.push(wait_event(request, events.len() as u32, now, WaitEvent::Expired, &wait)?);
                }
            }
            for path in released_paths {
                append_grant_for_path(
                    request, reader, now, &request.workspace.workspace_id, &path, &expired_ids, &[], true, &mut events,
                )?;
            }
            Ok(CommandPlan { events, response: expired_ids, http_status: 200 })
        })
    }

    pub fn reservation(&self, workspace_id: &str, reservation_id: &str) -> StoreResult<Option<ReservationRecord>> {
        self.current_records(CurrentAggregate::Reservation, workspace_id)?.into_iter()
            .map(record_from_current::<ReservationRecord>)
            .collect::<StoreResult<Vec<_>>>()?
            .into_iter()
            .find(|record| record.reservation_id == reservation_id)
            .map(Ok)
            .transpose()
    }

    pub fn wait(&self, workspace_id: &str, wait_id: &str) -> StoreResult<Option<WaitRecord>> {
        self.current_records(CurrentAggregate::Wait, workspace_id)?.into_iter()
            .map(|record| {
                let mut wait = record_from_current::<WaitRecord>(record)?;
                normalize_wait(&mut wait)?;
                Ok(wait)
            })
            .collect::<StoreResult<Vec<_>>>()?
            .into_iter()
            .find(|record| record.wait_id == wait_id)
            .map(Ok)
            .transpose()
    }
}

pub(crate) fn typed_records<T: DeserializeOwned>(
    reader: &dyn ProjectionReader,
    aggregate: CurrentAggregate,
    workspace_id: &str,
) -> StoreResult<Vec<T>> {
    reader.aggregate_records(aggregate, workspace_id)?.into_iter().map(record_from_current).collect()
}

pub(crate) fn record_from_current<T: DeserializeOwned>(record: CurrentRecord) -> StoreResult<T> {
    let mut payload = record.payload;
    if let Some(data) = payload.as_object_mut() {
        data.insert("origin_event_seq".into(), serde_json::Value::from(record.origin_event_seq));
    }
    serde_json::from_value(payload).map_err(StoreError::from)
}

pub(crate) fn reservation_event<T>(
    request: &RequestEnvelope<T>,
    ordinal: u32,
    now: OffsetDateTime,
    variant: fn(EventData) -> ReservationEvent,
    reservation: &ReservationRecord,
) -> StoreResult<NewEvent> {
    let mut data = EventData::new(&reservation.reservation_id);
    data.data = json!({"reservation": reservation});
    NewEvent::new(request.request_id, ordinal, now, EventPayload::Reservation(variant(data))).map_err(StoreError::from)
}

pub(crate) fn wait_event<T>(
    request: &RequestEnvelope<T>,
    ordinal: u32,
    now: OffsetDateTime,
    variant: fn(EventData) -> WaitEvent,
    wait: &WaitRecord,
) -> StoreResult<NewEvent> {
    let mut data = EventData::new(&wait.wait_id);
    data.data = json!({"wait": wait});
    NewEvent::new(request.request_id, ordinal, now, EventPayload::Wait(variant(data))).map_err(StoreError::from)
}

pub(crate) fn append_grant_for_path<T>(
    request: &RequestEnvelope<T>,
    reader: &dyn ProjectionReader,
    now: OffsetDateTime,
    workspace_id: &str,
    relative_path: &str,
    ignored_reservation_ids: &[String],
    ignored_wait_ids: &[String],
    grant_all_non_conflicting: bool,
    events: &mut Vec<NewEvent>,
) -> StoreResult<Option<WaitRecord>> {
    let active_reservations = typed_records::<ReservationRecord>(reader, CurrentAggregate::Reservation, workspace_id)?;
    let active_claims = typed_records::<ClaimRecord>(reader, CurrentAggregate::Claim, workspace_id)?;
    let candidate_is_blocked = |candidate_path: &str| {
        active_reservations.iter().any(|reservation| {
            reservation.status == "active"
                && !ignored_reservation_ids.contains(&reservation.reservation_id)
                && !expired_optional(reservation.expires_at.as_deref(), now)
                && reservation
                    .scopes
                    .iter()
                    .any(|scope| scopes_conflict(&scope_path(scope), candidate_path))
        }) || active_claims.iter().any(|claim| {
            claim.status == "active"
                && !ignored_reservation_ids.contains(&claim.reservation_id)
                && !expired_optional(claim.expires_at.as_deref(), now)
                && scopes_conflict(&claim.relative_path, candidate_path)
        })
    };

    let mut waits = typed_records::<WaitRecord>(reader, CurrentAggregate::Wait, workspace_id)?;
    for wait in &mut waits {
        normalize_wait(wait)?;
    }
    waits.sort_by(|left, right| {
        left.requested_at.cmp(&right.requested_at)
            .then_with(|| left.origin_event_seq.cmp(&right.origin_event_seq))
            .then_with(|| left.wait_id.cmp(&right.wait_id))
    });
    let notifications = typed_records::<serde_json::Value>(reader, CurrentAggregate::Notification, workspace_id)?;
    let mut sequence = notifications.iter()
        .filter_map(|notification| notification.get("sequence").and_then(serde_json::Value::as_u64))
        .max()
        .unwrap_or(0)
        + 1;
    let mut granted: Vec<String> = Vec::new();
    let mut first = None;
    for mut wait in waits {
        if wait.status != "queued"
            || ignored_wait_ids.contains(&wait.wait_id)
            || !scopes_conflict(&wait.relative_path, relative_path)
            || candidate_is_blocked(&wait.relative_path)
            || pending_grant_conflicts(events, &wait.relative_path)
            || granted.iter().any(|scope| scopes_conflict(scope, &wait.relative_path))
        {
            continue;
        }
        let scope = if wait.relative_path.ends_with('/') {
            ReservationScope::directory(&wait.relative_path)
        } else {
            ReservationScope::file(&wait.relative_path)
        };
        let mut reservation = reservation_record(
            request,
            wait.wait_id.clone(),
            vec![scope],
            wait.action.clone(),
            wait.purpose.clone(),
            now,
            Some(wait.wait_id.clone()),
            CLAIMABLE_TTL,
        );
        reservation.agent_id = wait.agent_id.clone();
        wait.status = "claimable".into();
        wait.reservation_id = Some(reservation.reservation_id.clone());
        wait.reservation_expires_at = reservation.expires_at.clone();
        events.push(reservation_event(request, events.len() as u32, now, ReservationEvent::Declared, &reservation)?);
        events.push(wait_event(request, events.len() as u32, now, WaitEvent::BecameClaimable, &wait)?);
        let mut data = EventData::new(&wait.wait_id);
        data.data = json!({"notification": {
            "notification_id": wait.wait_id,
            "target_agent_id": wait.agent_id,
            "workspace_id": workspace_id,
            "sequence": sequence,
            "kind": "reservation_granted",
            "payload": {"wait_id": wait.wait_id, "reservation_id": reservation.reservation_id, "purpose": wait.purpose},
            "status": "queued",
            "created_at": timestamp(now)?,
            "expires_at": reservation.expires_at,
        }});
        events.push(NewEvent::new(request.request_id, events.len() as u32, now, EventPayload::Notification(NotificationEvent::Created(data)))?);
        let delivery = DeliveryRecord {
            delivery_id: wait.wait_id.clone(),
            notification_id: wait.wait_id.clone(),
            workspace_id: workspace_id.into(),
            status: "queued".into(),
            attempts: 0,
            last_error: None,
            retry_at: None,
            delivered_at: None,
            origin_event_seq: 0,
        };
        events.push(delivery_event(request, events.len() as u32, now, RecoveryEvent::Queued, &delivery)?);
        sequence += 1;
        granted.push(wait.relative_path.clone());
        if first.is_none() {
            first = Some(wait);
        }
        if !grant_all_non_conflicting {
            break;
        }
    }
    Ok(first)
}

fn pending_grant_conflicts(events: &[NewEvent], candidate_path: &str) -> bool {
    events.iter().any(|event| {
        let EventPayload::Reservation(ReservationEvent::Declared(data)) = &event.payload else {
            return false;
        };
        data.data
            .get("reservation")
            .and_then(|reservation| reservation.get("scopes"))
            .and_then(serde_json::Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|scope| {
                let path = scope.get("path")?.as_str()?;
                Some(if scope.get("kind").and_then(serde_json::Value::as_str) == Some("directory") {
                    format!("{path}/")
                } else {
                    path.into()
                })
            })
            .any(|scope| scopes_conflict(&scope, candidate_path))
    })
}

fn reservation_record<T>(
    request: &RequestEnvelope<T>,
    reservation_id: String,
    scopes: Vec<ReservationScope>,
    action: String,
    purpose: String,
    now: OffsetDateTime,
    wait_id: Option<String>,
    ttl: Duration,
) -> ReservationRecord {
    ReservationRecord {
        reservation_id,
        agent_id: request.agent.agent_id.clone(),
        workspace_id: request.workspace.workspace_id.clone(),
        scopes,
        action,
        purpose,
        status: "active".into(),
        declared_at: timestamp(now).expect("fixed command time must format"),
        expires_at: Some(timestamp(now + ttl).expect("fixed command time must format")),
        max_expires_at: Some(timestamp(now + RESERVATION_MAX_LIFETIME).expect("fixed command time must format")),
        wait_id,
        origin_event_seq: 0,
    }
}

pub(crate) fn normalized_scope(value: &str) -> StoreResult<String> {
    let normalized = normalize_relative_path(value);
    if normalized.is_empty() {
        return Err(StoreError::MissingScope);
    }
    Ok(if value.ends_with('/') { format!("{}/", normalized.trim_end_matches('/')) } else { normalized })
}

fn normalize_wait(wait: &mut WaitRecord) -> StoreResult<()> {
    wait.relative_path = normalized_wait_relative_path(&wait.relative_path, &wait.action)?;
    Ok(())
}

fn normalized_wait_relative_path(relative_path: &str, action: &str) -> StoreResult<String> {
    let mut relative_path = normalized_scope(relative_path)?;
    if action == "write_directory" && !relative_path.ends_with('/') {
        relative_path.push('/');
    }
    Ok(relative_path)
}

pub(crate) fn normalized_reservation_scopes(scopes: &[ReservationScope]) -> StoreResult<Vec<ReservationScope>> {
    if scopes.is_empty() {
        return Err(StoreError::MissingScope);
    }
    scopes
        .iter()
        .map(|scope| {
            let path = normalized_scope(match scope {
                ReservationScope::File(path) | ReservationScope::Directory(path) => path,
            })?;
            Ok(match scope {
                ReservationScope::File(_) => ReservationScope::file(path.trim_end_matches('/')),
                ReservationScope::Directory(_) => ReservationScope::directory(path.trim_end_matches('/')),
            })
        })
        .collect()
}

pub(crate) fn scope_path(scope: &ReservationScope) -> String {
    match scope {
        ReservationScope::File(path) => path.clone(),
        ReservationScope::Directory(path) => format!("{}/", path.trim_end_matches('/')),
    }
}

pub(crate) fn required_purpose(value: &str) -> StoreResult<String> {
    let purpose = value.trim();
    if purpose.is_empty() { return Err(StoreError::MissingPurpose); }
    Ok(purpose.into())
}

pub(crate) fn scopes_conflict(left: &str, right: &str) -> bool {
    let left_directory = left.ends_with('/');
    let right_directory = right.ends_with('/');
    let left = left.trim_end_matches('/');
    let right = right.trim_end_matches('/');
    match (left_directory, right_directory) {
        (false, false) => left == right,
        (true, false) => right.starts_with(&format!("{left}/")),
        (false, true) => left.starts_with(&format!("{right}/")),
        (true, true) => left == right || left.starts_with(&format!("{right}/")) || right.starts_with(&format!("{left}/")),
    }
}

pub(crate) fn timestamp(value: OffsetDateTime) -> StoreResult<String> {
    value.format(&time::format_description::well_known::Rfc3339)
        .map_err(|error| StoreError::InvalidTimestamp(error.to_string()))
}

pub(crate) fn parse_time(value: &str) -> StoreResult<OffsetDateTime> {
    OffsetDateTime::parse(value, &time::format_description::well_known::Rfc3339)
        .map_err(|_| StoreError::InvalidTimestamp(value.into()))
}

pub(crate) fn expired(value: &str, now: OffsetDateTime) -> bool {
    parse_time(value).is_ok_and(|time| time <= now)
}

pub(crate) fn expired_optional(value: Option<&str>, now: OffsetDateTime) -> bool {
    value.is_some_and(|value| expired(value, now))
}
