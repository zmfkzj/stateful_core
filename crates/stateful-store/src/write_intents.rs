use crate::{
    CommandOutcome, CommandPlan, CurrentAggregate, Store, StoreError, StoreResult,
    observations::read_event,
    reservations::{expired, normalized_scope, record_from_current, scopes_conflict, timestamp, typed_records},
    write_fences::{WriteFenceRecord, fence_event},
};
use serde_json::json;
use stateful_core::{
    EventData, EventPayload, NewEvent, OBSERVATION_TTL, ReadClassification, ReadObservationEvent,
    ReadObservationRecord, ReadObservationStatus, RequestEnvelope, ResourceVersion,
    WriteFenceEvent, WriteIntentCompletion, WriteIntentEvent, WriteIntentOutcome, WriteIntentRecord,
    WriteIntentStart, WriteIntentStatus, WriteTarget,
};
use std::collections::{BTreeMap, HashSet};
use time::{Duration, OffsetDateTime};
use uuid::Uuid;

const WRITE_FENCE_TTL: Duration = Duration::minutes(5);

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct WriteIntentStartResult {
    pub intent_id: String,
    pub fence_ids: Vec<String>,
}

impl Store {
    pub fn start_write_intent(
        &self,
        request: &RequestEnvelope<WriteIntentStart>,
    ) -> StoreResult<CommandOutcome<WriteIntentStartResult>> {
        let now = self.clock.now();
        let payload = request.payload.clone();
        self.execute_command(request, "write_intent.start", |reader| {
            if payload.operation_id.trim().is_empty() || payload.action.trim().is_empty() {
                return Err(StoreError::InvalidWriteIntent);
            }
            let targets = normalize_targets(payload.targets)?;
            let existing_fences = typed_records::<WriteFenceRecord>(
                reader,
                CurrentAggregate::WriteFence,
                &request.workspace.workspace_id,
            )?;
            for target in &targets {
                if let Some(fence) = existing_fences.iter().find(|fence| {
                    fence.status == "active"
                        && !expired(&fence.expires_at, now)
                        && fence.agent_id != request.agent.agent_id
                        && scopes_conflict(&fence.relative_path, &target.path)
                }) {
                    return Err(StoreError::WriteFenceConflict {
                        path: target.path.clone(),
                        owner_agent_id: fence.agent_id.clone(),
                    });
                }
            }
            if typed_records::<WriteIntentRecord>(
                reader,
                CurrentAggregate::WriteIntent,
                &request.workspace.workspace_id,
            )?
            .iter()
            .any(|intent| {
                intent.status.blocks_writes()
                    && intent.targets.iter().any(|existing| {
                        targets.iter().any(|target| scopes_conflict(&existing.path, &target.path))
                    })
            }) {
                return Err(StoreError::InvalidWriteIntent);
            }

            let intent_id = Uuid::new_v4().to_string();
            let fences = targets
                .iter()
                .map(|target| Ok(WriteFenceRecord {
                    fence_id: Uuid::new_v4().to_string(),
                    agent_id: request.agent.agent_id.clone(),
                    workspace_id: request.workspace.workspace_id.clone(),
                    relative_path: target.path.clone(),
                    action: payload.action.clone(),
                    status: "active".into(),
                    acquired_at: timestamp(now)?,
                    expires_at: timestamp(now + WRITE_FENCE_TTL)?,
                    released_at: None,
                    origin_event_seq: 0,
                }))
                .collect::<StoreResult<Vec<_>>>()?;
            let intent = WriteIntentRecord {
                intent_id: intent_id.clone(),
                operation_id: payload.operation_id,
                workspace_id: request.workspace.workspace_id.clone(),
                agent_id: request.agent.agent_id.clone(),
                action: payload.action,
                targets,
                fence_ids: fences.iter().map(|fence| fence.fence_id.clone()).collect(),
                status: WriteIntentStatus::Started,
                started_at: now,
                completed_at: None,
                failure_code: None,
                origin_event_seq: 0,
            };
            let mut events = vec![intent_event(
                request,
                0,
                now,
                WriteIntentEvent::Started,
                &intent,
                &[],
            )?];
            for fence in &fences {
                events.push(fence_event(
                    request,
                    events.len() as u32,
                    now,
                    WriteFenceEvent::Acquired,
                    fence,
                )?);
            }
            Ok(CommandPlan {
                events,
                response: WriteIntentStartResult {
                    intent_id,
                    fence_ids: intent.fence_ids,
                },
                http_status: 200,
            })
        })
    }

    pub fn complete_write_intent(
        &self,
        request: &RequestEnvelope<WriteIntentCompletion>,
    ) -> StoreResult<CommandOutcome<WriteIntentRecord>> {
        let now = self.clock.now();
        let payload = request.payload.clone();
        self.execute_command(request, "write_intent.complete", |reader| {
            let intent = find_owned_intent(reader, request, &payload.intent_id)?;
            if intent.status != WriteIntentStatus::Started {
                return Err(StoreError::InvalidWriteIntent);
            }
            let mut completed = intent.clone();
            completed.completed_at = Some(now);
            completed.failure_code = payload.failure_code;
            let mut events = Vec::new();
            match payload.outcome {
                WriteIntentOutcome::Failed => {
                    completed.status = WriteIntentStatus::Failed;
                    events.push(intent_event(
                        request,
                        0,
                        now,
                        WriteIntentEvent::Failed,
                        &completed,
                        &[],
                    )?);
                }
                WriteIntentOutcome::Committed => {
                    let posts = post_fingerprints(&intent.targets, payload.post_fingerprints)?;
                    completed.status = WriteIntentStatus::Committed;
                    let versions = next_resource_versions(reader, request, &intent, &posts)?;
                    events.push(intent_event(
                        request,
                        0,
                        now,
                        WriteIntentEvent::Committed,
                        &completed,
                        &versions,
                    )?);
                    append_peer_observation_invalidations(
                        request,
                        reader,
                        now,
                        &intent.targets,
                        &mut events,
                    )?;
                    for (target, version) in intent.targets.iter().zip(&versions) {
                        let writer_observation = ReadObservationRecord {
                            workspace_id: request.workspace.workspace_id.clone(),
                            agent_id: request.agent.agent_id.clone(),
                            operation_id: intent.operation_id.clone(),
                            path: target.path.clone(),
                            status: ReadObservationStatus::Stabilized,
                            classification: ReadClassification::Exact,
                            before: target.before.clone(),
                            after: Some(version.fingerprint.clone()),
                            semantic_marker: None,
                            observed_at: now,
                            expires_at: Some(now + OBSERVATION_TTL),
                            resource_version: version.version,
                            origin_event_seq: 0,
                        };
                        events.push(read_event(
                            request,
                            events.len() as u32,
                            now,
                            ReadObservationEvent::Stabilized,
                            &writer_observation,
                        )?);
                    }
                }
            }
            append_fence_releases(request, reader, now, &intent, &mut events)?;
            Ok(CommandPlan { events, response: completed, http_status: 200 })
        })
    }

    pub fn recover_write_intent(
        &self,
        request: &RequestEnvelope<(String, Vec<(String, stateful_core::ContentFingerprint)>)>,
    ) -> StoreResult<CommandOutcome<WriteIntentRecord>> {
        let now = self.clock.now();
        let (intent_id, actual) = request.payload.clone();
        self.execute_command(request, "write_intent.recover", |reader| {
            let intent = find_owned_intent(reader, request, &intent_id)?;
            if intent.status != WriteIntentStatus::Started {
                return Err(StoreError::InvalidWriteIntent);
            }
            let actual = post_fingerprints(&intent.targets, actual)?;
            let mut recovered = intent.clone();
            recovered.completed_at = Some(now);
            let unchanged = intent.targets.iter().all(|target| actual[&target.path] == target.before);
            let mut events = Vec::new();
            if unchanged {
                recovered.status = WriteIntentStatus::Reconciled;
                events.push(intent_event(
                    request,
                    0,
                    now,
                    WriteIntentEvent::Reconciled,
                    &recovered,
                    &[],
                )?);
                append_fence_releases(request, reader, now, &intent, &mut events)?;
            } else {
                recovered.status = WriteIntentStatus::OutcomeUnknown;
                events.push(intent_event(
                    request,
                    0,
                    now,
                    WriteIntentEvent::OutcomeUnknown,
                    &recovered,
                    &[],
                )?);
            }
            Ok(CommandPlan { events, response: recovered, http_status: 200 })
        })
    }

    pub fn reconcile_write_intent(
        &self,
        request: &RequestEnvelope<String>,
    ) -> StoreResult<CommandOutcome<WriteIntentRecord>> {
        let now = self.clock.now();
        let intent_id = request.payload.clone();
        self.execute_command(request, "write_intent.reconcile", |reader| {
            let intent = find_owned_intent(reader, request, &intent_id)?;
            if intent.status != WriteIntentStatus::OutcomeUnknown {
                return Err(StoreError::InvalidWriteIntent);
            }
            let observations = typed_records::<ReadObservationRecord>(
                reader,
                CurrentAggregate::ReadObservation,
                &request.workspace.workspace_id,
            )?;
            let mut rereads = BTreeMap::new();
            let mut posts = BTreeMap::new();
            for target in &intent.targets {
                let observation = observations
                    .iter()
                    .find(|observation| {
                        observation.agent_id == request.agent.agent_id
                            && observation.path == target.path
                            && observation.classification == ReadClassification::Exact
                            && observation.is_fresh_at(now)
                            && observation.origin_event_seq > intent.origin_event_seq
                            && observation.before.is_complete_exact()
                    })
                    .ok_or(StoreError::InvalidReadOperation)?;
                let after = observation
                    .after
                    .as_ref()
                    .filter(|fingerprint| fingerprint.is_complete_exact())
                    .cloned()
                    .ok_or(StoreError::InvalidReadOperation)?;
                rereads.insert(target.path.clone(), observation.clone());
                posts.insert(target.path.clone(), after);
            }
            let versions = next_resource_versions(reader, request, &intent, &posts)?;
            let mut reconciled = intent.clone();
            reconciled.status = WriteIntentStatus::Reconciled;
            reconciled.completed_at = Some(now);
            let mut events = vec![intent_event(
                request,
                0,
                now,
                WriteIntentEvent::Reconciled,
                &reconciled,
                &versions,
            )?];
            for version in &versions {
                let mut observation = rereads
                    .remove(&version.path)
                    .ok_or(StoreError::InvalidReadOperation)?;
                observation.resource_version = version.version;
                events.push(read_event(
                    request,
                    events.len() as u32,
                    now,
                    ReadObservationEvent::Stabilized,
                    &observation,
                )?);
            }
            append_peer_observation_invalidations(
                request,
                reader,
                now,
                &intent.targets,
                &mut events,
            )?;
            append_fence_releases(request, reader, now, &intent, &mut events)?;
            Ok(CommandPlan { events, response: reconciled, http_status: 200 })
        })
    }

    pub fn active_write_intent(
        &self,
        workspace_id: &str,
        path: &str,
    ) -> StoreResult<Option<WriteIntentRecord>> {
        let path = normalized_scope(path)?;
        self.current_records(CurrentAggregate::WriteIntent, workspace_id)?
            .into_iter()
            .map(record_from_current::<WriteIntentRecord>)
            .collect::<StoreResult<Vec<_>>>()?
            .into_iter()
            .find(|intent| {
                intent.status.blocks_writes()
                    && intent.targets.iter().any(|target| scopes_conflict(&target.path, &path))
            })
            .map(Ok)
            .transpose()
    }

    pub fn active_write_fence(
        &self,
        workspace_id: &str,
        path: &str,
    ) -> StoreResult<Option<WriteFenceRecord>> {
        let path = normalized_scope(path)?;
        let now = self.clock.now();
        self.current_records(CurrentAggregate::WriteFence, workspace_id)?
            .into_iter()
            .map(record_from_current::<WriteFenceRecord>)
            .collect::<StoreResult<Vec<_>>>()?
            .into_iter()
            .find(|fence| {
                fence.status == "active" && !expired(&fence.expires_at, now)
                    && scopes_conflict(&fence.relative_path, &path)
            })
            .map(Ok)
            .transpose()
    }

    pub fn resource_version(
        &self,
        workspace_id: &str,
        path: &str,
    ) -> StoreResult<Option<ResourceVersion>> {
        let path = normalized_scope(path)?;
        self.current_records(CurrentAggregate::ResourceWrite, workspace_id)?
            .into_iter()
            .map(record_from_current::<ResourceVersion>)
            .collect::<StoreResult<Vec<_>>>()?
            .into_iter()
            .find(|version| version.path == path)
            .map(Ok)
            .transpose()
    }
}

fn normalize_targets(targets: Vec<WriteTarget>) -> StoreResult<Vec<WriteTarget>> {
    if targets.is_empty() {
        return Err(StoreError::MissingScope);
    }
    let mut seen = HashSet::with_capacity(targets.len());
    let mut normalized = Vec::with_capacity(targets.len());
    for mut target in targets {
        target.path = normalized_scope(&target.path)?;
        if !target.before.is_complete_exact() || !seen.insert(target.path.clone()) {
            return Err(StoreError::InvalidWriteIntent);
        }
        normalized.push(target);
    }
    Ok(normalized)
}

fn find_owned_intent<T>(
    reader: &dyn crate::ProjectionReader,
    request: &RequestEnvelope<T>,
    intent_id: &str,
) -> StoreResult<WriteIntentRecord> {
    let intent = typed_records::<WriteIntentRecord>(
        reader,
        CurrentAggregate::WriteIntent,
        &request.workspace.workspace_id,
    )?
    .into_iter()
    .find(|intent| intent.intent_id == intent_id)
    .ok_or(StoreError::WriteIntentNotFound)?;
    if intent.agent_id != request.agent.agent_id {
        return Err(StoreError::WriteIntentOwnerMismatch);
    }
    Ok(intent)
}

fn post_fingerprints(
    targets: &[WriteTarget],
    fingerprints: Vec<(String, stateful_core::ContentFingerprint)>,
) -> StoreResult<BTreeMap<String, stateful_core::ContentFingerprint>> {
    if fingerprints.len() != targets.len() {
        return Err(StoreError::InvalidWriteIntent);
    }
    let mut values = BTreeMap::new();
    for (path, fingerprint) in fingerprints {
        let path = normalized_scope(&path)?;
        if !fingerprint.is_complete_exact() || values.insert(path, fingerprint).is_some() {
            return Err(StoreError::InvalidWriteIntent);
        }
    }
    if targets.iter().any(|target| !values.contains_key(&target.path)) {
        return Err(StoreError::InvalidWriteIntent);
    }
    Ok(values)
}

fn next_resource_versions<T>(
    reader: &dyn crate::ProjectionReader,
    request: &RequestEnvelope<T>,
    intent: &WriteIntentRecord,
    posts: &BTreeMap<String, stateful_core::ContentFingerprint>,
) -> StoreResult<Vec<ResourceVersion>> {
    let current = typed_records::<ResourceVersion>(
        reader,
        CurrentAggregate::ResourceWrite,
        &request.workspace.workspace_id,
    )?;
    Ok(intent.targets.iter().map(|target| ResourceVersion {
        workspace_id: request.workspace.workspace_id.clone(),
        path: target.path.clone(),
        version: current.iter().find(|version| version.path == target.path).map_or(1, |version| version.version + 1),
        fingerprint: posts[&target.path].clone(),
        writer_agent_id: request.agent.agent_id.clone(),
        intent_id: intent.intent_id.clone(),
        origin_event_seq: 0,
    }).collect())
}

fn append_peer_observation_invalidations<T>(
    request: &RequestEnvelope<T>,
    reader: &dyn crate::ProjectionReader,
    now: OffsetDateTime,
    targets: &[WriteTarget],
    events: &mut Vec<NewEvent>,
) -> StoreResult<()> {
    for observation in typed_records::<ReadObservationRecord>(
        reader,
        CurrentAggregate::ReadObservation,
        &request.workspace.workspace_id,
    )? {
        if observation.agent_id != request.agent.agent_id
            && observation.is_stable()
            && targets.iter().any(|target| target.path == observation.path)
        {
            let mut invalidated = observation;
            invalidated.status = ReadObservationStatus::Invalidated;
            invalidated.expires_at = None;
            events.push(read_event(
                request,
                events.len() as u32,
                now,
                ReadObservationEvent::Invalidated,
                &invalidated,
            )?);
        }
    }
    Ok(())
}

fn append_fence_releases<T>(
    request: &RequestEnvelope<T>,
    reader: &dyn crate::ProjectionReader,
    now: OffsetDateTime,
    intent: &WriteIntentRecord,
    events: &mut Vec<NewEvent>,
) -> StoreResult<()> {
    let fences = typed_records::<WriteFenceRecord>(
        reader,
        CurrentAggregate::WriteFence,
        &request.workspace.workspace_id,
    )?;
    for fence_id in &intent.fence_ids {
        let mut fence = fences.iter().find(|fence| fence.fence_id == *fence_id)
            .cloned().ok_or(StoreError::WriteIntentNotFound)?;
        if fence.status == "active" {
            fence.status = "released".into();
            fence.released_at = Some(timestamp(now)?);
            events.push(fence_event(
                request,
                events.len() as u32,
                now,
                WriteFenceEvent::Released,
                &fence,
            )?);
        }
    }
    Ok(())
}

fn intent_event<T>(
    request: &RequestEnvelope<T>,
    ordinal: u32,
    now: OffsetDateTime,
    variant: fn(EventData) -> WriteIntentEvent,
    intent: &WriteIntentRecord,
    resource_versions: &[ResourceVersion],
) -> StoreResult<NewEvent> {
    let mut data = EventData::new(&intent.intent_id);
    data.data = json!({
        "write_intent": intent,
        "resource_versions": resource_versions,
    });
    NewEvent::new(request.request_id, ordinal, now, EventPayload::WriteIntent(variant(data)))
        .map_err(StoreError::from)
}
