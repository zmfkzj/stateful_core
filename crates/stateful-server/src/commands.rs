use crate::{CoordinationMode, RUNTIME_CAPABILITIES, ServerConfig, SharedStore, protocol};
use axum::{Json, extract::{RawQuery, State}, http::{HeaderMap, StatusCode}, response::{IntoResponse, Response, sse::{Event as SseEvent, KeepAlive, Sse}}};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use stateful_core::{
    AuthorizationInput, Decision, DecisionKind, FreshnessMode, ObservationFreshness, PolicyState,
    QueryEnvelope, ReadObservationStatus, RequestEnvelope, ReservationScope, ThinSafetyState,
    V2Error, WriteIntentStart, WriteTarget, authorize_action, evaluate_thin_safety,
    normalize_relative_path,
};
use stateful_store::{PresenceRegistration, Store, StoreError};
use std::{convert::Infallible, time::Duration};
use tokio_stream::StreamExt;

#[derive(Debug, Deserialize)]
pub(crate) struct CurrentQuery {
    #[serde(default)]
    resource: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct EventsQuery {
    #[serde(default = "default_events_limit")]
    limit: u64,
}

const fn default_events_limit() -> u64 {
    100
}

#[derive(Debug, Deserialize)]
pub(crate) struct SaveCheckPayload {
    paths: Vec<String>,
}
#[derive(Debug, Deserialize, Serialize)]
pub(crate) struct AuthorizePayload {
    #[serde(default)]
    reservation_id: Option<String>,
    operation_id: String,
    action: String,
    targets: Vec<WriteTarget>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct WriteReconcilePayload {
    intent_id: String,
}

pub(crate) async fn session_register(
    State(config): State<ServerConfig>,
    protocol::V2Json(body): protocol::V2Json,
) -> Response {
    let request = match protocol::parse_request::<PresenceRegistration>(body) {
        Ok(request) => request,
        Err(response) => return response,
    };
    let request_id = request.request_id.to_string();
    protocol::command_response(&request_id, lock_store(&config.store).and_then(|mut store| store.register_presence(&request)))
}

pub(crate) async fn presence_update(
    State(config): State<ServerConfig>,
    protocol::V2Json(body): protocol::V2Json,
) -> Response {
    let kind = body.pointer("/payload/kind").and_then(Value::as_str).unwrap_or("update");
    match kind {
        "resume" => command(config, body, |store, request| store.resume_presence(request)),
        "heartbeat" => command(config, body, |store, request| store.heartbeat_presence(request)),
        "resource" => command(config, body, |store, request| store.update_presence_resource(request)),
        "tool_start" => command(config, body, |store, request| store.start_presence_tool(request)),
        "tool_result" => command(config, body, |store, request| store.complete_presence_tool(request)),
        "register" => command(config, body, |store, request| store.register_presence(request)),
        "update" => command(config, body, |store, request| store.update_presence(request)),
        _ => protocol::error_response(
            StatusCode::BAD_REQUEST,
            body.get("request_id").and_then(Value::as_str),
            V2Error::new("invalid_presence_update", "kind must be register, resume, heartbeat, update, resource, tool_start, or tool_result."),
        ),
    }
}

pub(crate) async fn read_start(State(config): State<ServerConfig>, protocol::V2Json(body): protocol::V2Json) -> Response {
    command(config, body, |store, request| store.start_read_observation(request))
}

pub(crate) async fn read_complete(State(config): State<ServerConfig>, protocol::V2Json(body): protocol::V2Json) -> Response {
    command(config, body, |store, request| store.complete_read_observation(request))
}

pub(crate) async fn write_complete(State(config): State<ServerConfig>, protocol::V2Json(body): protocol::V2Json) -> Response {
    command(config, body, |store, request| store.complete_write_intent(request))
}

pub(crate) async fn activity_finalize(
    State(config): State<ServerConfig>,
    protocol::V2Json(body): protocol::V2Json,
) -> Response {
    if body.pointer("/payload/status").is_some() {
        command(config, body, |store, request| store.finalize_handoff(request))
    } else {
        command(config, body, |store, request| store.finalize_activity(request))
    }
}

pub(crate) async fn reservation_declare(State(config): State<ServerConfig>, protocol::V2Json(body): protocol::V2Json) -> Response {
    command(config, body, |store, request| store.declare_reservation(request))
}

pub(crate) async fn reservation_request(State(config): State<ServerConfig>, protocol::V2Json(body): protocol::V2Json) -> Response {
    command(config, body, |store, request| store.request_wait(request))
}

pub(crate) async fn reservation_claim(State(config): State<ServerConfig>, protocol::V2Json(body): protocol::V2Json) -> Response {
    command(config, body, |store, request| store.grant_next_wait(request))
}

pub(crate) async fn reservation_cancel(State(config): State<ServerConfig>, protocol::V2Json(body): protocol::V2Json) -> Response {
    command(config, body, |store, request| store.cancel_wait(request))
}

pub(crate) async fn claim_acquire(State(config): State<ServerConfig>, protocol::V2Json(body): protocol::V2Json) -> Response {
    command(config, body, |store, request| store.acquire_claim(request))
}

pub(crate) async fn claim_release(State(config): State<ServerConfig>, protocol::V2Json(body): protocol::V2Json) -> Response {
    command(config, body, |store, request| store.release_claim(request))
}

pub(crate) async fn authorize(State(config): State<ServerConfig>, protocol::V2Json(body): protocol::V2Json) -> Response {
    let request = match protocol::parse_request::<AuthorizePayload>(body) {
        Ok(request) => request,
        Err(response) => return response,
    };
    let request_id = request.request_id.to_string();
    let mut store = match lock_store(&config.store) {
        Ok(store) => store,
        Err(error) => return protocol::store_error_response(&request_id, error),
    };
    if let Err(error) = store.run_maintenance() {
        return protocol::store_error_response(&request_id, error);
    }
    let decision = match authorize_request(&mut store, config.coordination_mode, &request) {
        Ok(decision) => decision,
        Err(response) => return response,
    };
    let write_payload = WriteIntentStart {
        operation_id: request.payload.operation_id.clone(),
        action: request.payload.action.clone(),
        targets: request.payload.targets.clone(),
    };
    protocol::command_response(
        &request_id,
        store.start_write_intent_authorized(&request, write_payload, decision),
    )
}

pub(crate) async fn human_observe(State(config): State<ServerConfig>, protocol::V2Json(body): protocol::V2Json) -> Response {
    command(config, body, |store, request| store.record_human_observation(request))
}

pub(crate) async fn human_save_check(
    State(config): State<ServerConfig>,
    protocol::V2Json(body): protocol::V2Json,
) -> Response {
    let request = match protocol::parse_request::<SaveCheckPayload>(body) {
        Ok(request) => request,
        Err(response) => return response,
    };
    let request_id = request.request_id.to_string();
    let response = lock_store(&config.store).and_then(|store| {
        store.unreconciled_human_observations(&request.workspace.workspace_id, &request.payload.paths)
    });
    match response {
        Ok(observations) => Json(json!({"blocked": !observations.is_empty(), "observations": observations})).into_response(),
        Err(error) => protocol::store_error_response(&request_id, error),
    }
}

pub(crate) async fn reconcile_ack(State(config): State<ServerConfig>, protocol::V2Json(body): protocol::V2Json) -> Response {
    if body.pointer("/payload/intent_id").is_some() {
        let request = match protocol::parse_request::<WriteReconcilePayload>(body) {
            Ok(request) => request,
            Err(response) => return response,
        };
        let request_id = request.request_id.to_string();
        let request = retarget(&request, request.payload.intent_id.clone());
        return protocol::command_response(&request_id, lock_store(&config.store).and_then(|store| store.reconcile_write_intent(&request)));
    }
    command(config, body, |store, request| store.acknowledge_human_reconciliation(request))
}

pub(crate) async fn context_render(State(config): State<ServerConfig>, protocol::V2Json(body): protocol::V2Json) -> Response {
    command(config, body, |store, request| {
        store.run_maintenance()?;
        store.render_context(request)
    })
}

pub(crate) async fn context_ack(State(config): State<ServerConfig>, protocol::V2Json(body): protocol::V2Json) -> Response {
    command(config, body, |store, request| store.acknowledge_context(request))
}

pub(crate) async fn notifications_poll(State(config): State<ServerConfig>, protocol::V2Json(body): protocol::V2Json) -> Response {
    let request = match protocol::parse_request::<Value>(body) {
        Ok(request) => request,
        Err(response) => return response,
    };
    let request_id = request.request_id.to_string();
    protocol::command_response(
        &request_id,
        lock_store(&config.store).and_then(|store| store.poll_notifications(&request)),
    )
}

pub(crate) async fn resume_next(State(config): State<ServerConfig>, protocol::V2Json(body): protocol::V2Json) -> Response {
    let request = match protocol::parse_request::<Value>(body) {
        Ok(request) => request,
        Err(response) => return response,
    };
    let request_id = request.request_id.to_string();
    let unit = unit_request(&request);
    let result = lock_store(&config.store).and_then(|mut store| {
        let presence = store.presence_for_request(&unit, &request.agent.agent_id)?;
        let handoff = store.handoff_for_request(&unit, &request.agent.agent_id)?;
        let deliveries = store.pending_context_deliveries(&request.agent.agent_id, &request.workspace.workspace_id)?;
        Ok::<_, StoreError>((presence, handoff, deliveries))
    });
    match result {
        Ok((presence, handoff, deliveries)) => Json(json!({"presence": presence, "handoff": handoff, "deliveries": deliveries})).into_response(),
        Err(error) => protocol::store_error_response(&request_id, error),
    }
}

pub(crate) async fn outbox_sync(State(config): State<ServerConfig>, protocol::V2Json(body): protocol::V2Json) -> Response {
    if body.pointer("/payload/event_type").is_some() {
        command(config, body, |store, request| store.enqueue_outbox(request))
    } else {
        command(config, body, |store, request| store.record_outbox_delivery(request))
    }
}

pub(crate) async fn current(
    State(config): State<ServerConfig>,
    RawQuery(raw): RawQuery,
) -> Response {
    let request = match protocol::parse_query::<CurrentQuery>(raw) {
        Ok(request) => request,
        Err(response) => return response,
    };
    let request_id = request.request_id.to_string();
    let unit = unit_query_request(&request);
    let result = lock_store(&config.store).and_then(|mut store| {
        store.run_maintenance()?;
        let presence = store.presence_for_request(&unit, &request.agent.agent_id)?;
        let handoff = store.handoff_for_request(&unit, &request.agent.agent_id)?;
        let resources = store.presence_resources_for_request(&unit, &request.agent.agent_id)?;
        let resources = match request.query.resource.as_deref() {
            Some(resource) => resources.into_iter().filter(|item| item.relative_path == resource).collect::<Vec<_>>(),
            None => resources,
        };
        let workspace_version = store.workspace_version(&request.workspace.workspace_id)?;
        let context_cursor = store.context_cursor(&request.workspace.workspace_id, &request.agent.agent_id)?;
        Ok::<_, StoreError>((presence, handoff, resources, workspace_version, context_cursor))
    });
    match result {
        Ok((presence, handoff, resources, workspace_version, context_cursor)) => Json(json!({
            "presence": presence,
            "handoff": handoff,
            "resources": resources,
            "workspace_version": workspace_version,
            "context_cursor": context_cursor,
        })).into_response(),
        Err(error) => protocol::store_error_response(&request_id, error),
    }
}

pub(crate) async fn events(
    State(config): State<ServerConfig>,
    RawQuery(raw): RawQuery,
) -> Response {
    let request = match protocol::parse_query::<EventsQuery>(raw) {
        Ok(request) => request,
        Err(response) => return response,
    };
    let request_id = request.request_id.to_string();
    match lock_store(&config.store).and_then(|store| store.recent_workspace_events(&request.workspace.workspace_id, request.query.limit)) {
        Ok(events) => Json(json!({"events": events})).into_response(),
        Err(error) => protocol::store_error_response(&request_id, error),
    }
}

pub(crate) async fn runtime_identity(
    State(config): State<ServerConfig>,
    RawQuery(raw): RawQuery,
) -> Response {
    let request = match protocol::parse_query::<Value>(raw) {
        Ok(request) => request,
        Err(response) => return response,
    };
    let request_id = request.request_id.to_string();
    match lock_store(&config.store).and_then(|store| store.workspace_version(&request.workspace.workspace_id)) {
        Ok(workspace_version) => Json(json!({
            "protocol_version": "stateful.v2",
            "journal_schema_version": 2,
            "coordination_mode": config.coordination_mode.as_str(),
            "workspace_id": request.workspace.workspace_id,
            "workspace_version": workspace_version,
            "capabilities": RUNTIME_CAPABILITIES,
        })).into_response(),
        Err(error) => protocol::store_error_response(&request_id, error),
    }
}

pub(crate) async fn notifications_stream(
    State(config): State<ServerConfig>,
    headers: HeaderMap,
    RawQuery(raw): RawQuery,
) -> Response {
    let request = match protocol::parse_query::<Value>(raw) {
        Ok(request) => request,
        Err(response) => return response,
    };
    let mut cursor = headers
        .get("last-event-id")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or_default();
    let store = config.store.clone();
    let agent_id = request.agent.agent_id;
    let workspace_id = request.workspace.workspace_id;
    let stream = tokio_stream::wrappers::IntervalStream::new(tokio::time::interval(Duration::from_millis(100)))
        .filter_map(move |_| {
            let notification = lock_store(&store)
                .and_then(|store| store.pending_notifications(&agent_id, &workspace_id))
                .ok()
                .and_then(|notifications| {
                    notifications
                        .into_iter()
                        .filter(|notification| notification.sequence > cursor)
                        .min_by_key(|notification| notification.sequence)
                });
            notification.map(|notification| {
                cursor = notification.sequence;
                Ok::<_, Infallible>(
                    SseEvent::default()
                        .id(notification.sequence.to_string())
                        .event("notification")
                        .json_data(notification)
                        .expect("notification JSON is serializable"),
                )
            })
        });
    Sse::new(stream)
        .keep_alive(KeepAlive::new().interval(Duration::from_secs(15)))
        .into_response()
}

fn command<T, R>(
    config: ServerConfig,
    body: Value,
    run: impl FnOnce(&mut Store, &RequestEnvelope<T>) -> Result<stateful_store::CommandOutcome<R>, StoreError>,
) -> Response
where
    T: serde::de::DeserializeOwned,
    R: serde::Serialize,
{
    let request = match protocol::parse_request::<T>(body) {
        Ok(request) => request,
        Err(response) => return response,
    };
    let request_id = request.request_id.to_string();
    protocol::command_response(&request_id, lock_store(&config.store).and_then(|mut store| run(&mut store, &request)))
}

fn lock_store(store: &SharedStore) -> Result<std::sync::MutexGuard<'_, Store>, StoreError> {
    store.lock().map_err(|_| StoreError::MigrationValidation("store lock poisoned".into()))
}

fn retarget<T, U>(request: &RequestEnvelope<T>, payload: U) -> RequestEnvelope<U> {
    RequestEnvelope {
        protocol_version: request.protocol_version,
        request_id: request.request_id,
        observed_at: request.observed_at,
        agent: request.agent.clone(),
        workspace: request.workspace.clone(),
        source: request.source.clone(),
        payload,
    }
}

fn unit_request(request: &RequestEnvelope<Value>) -> RequestEnvelope<()> {
    retarget(request, ())
}

fn unit_query_request<T>(request: &QueryEnvelope<T>) -> RequestEnvelope<()> {
    RequestEnvelope {
        protocol_version: request.protocol_version,
        request_id: request.request_id,
        observed_at: request.observed_at,
        agent: request.agent.clone(),
        workspace: request.workspace.clone(),
        source: request.source.clone(),
        payload: (),
    }
}

fn authorize_request(
    store: &mut Store,
    mode: CoordinationMode,
    request: &RequestEnvelope<AuthorizePayload>,
) -> Result<Decision, Response> {
    let request_id = request.request_id.to_string();
    let freshness_mode = match mode {
        CoordinationMode::Awareness => FreshnessMode::Awareness,
        CoordinationMode::Enforcement => FreshnessMode::Enforcement,
    };
    let mut result = Decision::allow("authorized", "Action is authorized.");
    for target in &request.payload.targets {
        let safety = thin_safety_state(store, request, target)
            .map_err(|error| protocol::store_error_response(&request_id, error))?;
        let decision = evaluate_thin_safety(safety, freshness_mode);
        if decision.decision == DecisionKind::Deny {
            return Err((StatusCode::FORBIDDEN, Json(decision)).into_response());
        }
        if decision.decision == DecisionKind::Warn {
            result = decision;
        }
    }

    let decision = authorization_decision(store, request)
        .map_err(|error| protocol::store_error_response(&request_id, error))?;
    if decision.decision == DecisionKind::Deny {
        if mode == CoordinationMode::Enforcement {
            return Err((StatusCode::FORBIDDEN, Json(decision)).into_response());
        }
        if result.decision == DecisionKind::Warn {
            return Ok(result);
        }
        return Ok(Decision {
            decision: DecisionKind::Warn,
            reason_code: decision.reason_code,
            message: decision.message,
            required_next_action: decision.required_next_action,
        });
    }
    if decision.decision == DecisionKind::Warn {
        result = decision;
    }
    Ok(result)
}

fn thin_safety_state(
    store: &Store,
    request: &RequestEnvelope<AuthorizePayload>,
    target: &WriteTarget,
) -> Result<ThinSafetyState, StoreError> {
    let invalid_target = target.path.is_empty() || normalize_relative_path(&target.path) != target.path;
    if invalid_target {
        return Ok(ThinSafetyState {
            invalid_target: true,
            unknown_write_outcome: false,
            observation: ObservationFreshness::Missing,
            active_fence: false,
            unreconciled_human_write: false,
        });
    }
    let active_intent = store.active_write_intent(&request.workspace.workspace_id, &target.path)?;
    let duplicate_intent = active_intent.as_ref().is_some_and(|intent| {
        intent.agent_id == request.agent.agent_id && intent.operation_id == request.payload.operation_id
    });
    let unknown_write_outcome = active_intent.is_some() && !duplicate_intent;
    let active_fence = store.active_write_fence(&request.workspace.workspace_id, &target.path)?.is_some()
        && !duplicate_intent;
    let unreconciled_human_write = !store
        .unreconciled_human_observations(&request.workspace.workspace_id, std::slice::from_ref(&target.path))?
        .is_empty();
    let observation = match store.read_observation(
        &request.workspace.workspace_id,
        &request.agent.agent_id,
        &target.path,
    )? {
        None => ObservationFreshness::Missing,
        Some(record) if record.status != ReadObservationStatus::Stabilized => ObservationFreshness::Unstable,
        Some(record) if !record.is_fresh_at(store.now()) => ObservationFreshness::Expired,
        Some(record) if !target.before.is_complete_exact() || record.after.as_ref() != Some(&target.before) => ObservationFreshness::Changed,
        Some(record) => match store.resource_version(&request.workspace.workspace_id, &target.path)? {
            Some(version) if version.version != record.resource_version => ObservationFreshness::Changed,
            _ => ObservationFreshness::Stable,
        },
    };
    Ok(ThinSafetyState {
        invalid_target: false,
        unknown_write_outcome,
        observation,
        active_fence,
        unreconciled_human_write,
    })
}

fn authorization_decision(store: &mut Store, request: &RequestEnvelope<AuthorizePayload>) -> Result<Decision, StoreError> {
    let Some(reservation_id) = request.payload.reservation_id.as_deref() else {
        return Ok(Decision::deny(
            "missing_reservation",
            "Supported writes require an active reservation.",
            "Declare an exact file or directory reservation before writing.",
        ));
    };
    let reservation = store.reservation(&request.workspace.workspace_id, reservation_id)?;
    let Some(reservation) = reservation.filter(|reservation| {
        reservation.agent_id == request.agent.agent_id
            && reservation.status == "active"
            && reservation.action == request.payload.action
    }) else {
        return Ok(Decision::deny(
            "missing_reservation",
            "The supplied reservation is not active for this agent and action.",
            "Declare an active reservation for the requested action.",
        ));
    };
    let scope = if reservation.relative_path.ends_with('/') {
        ReservationScope::directory(reservation.relative_path.clone())
    } else {
        ReservationScope::file(reservation.relative_path.clone())
    };
    let mut state = PolicyState::default().with_active_reservation_scopes(vec![scope]);
    if let Some(presence) = store.presence_for_request(&retarget(request, ()), &request.agent.agent_id)?
        && let Some(phase) = presence.phase
    {
        state = state.with_presence_phase(phase);
    }
    if request.payload.targets.is_empty() {
        return Ok(Decision::deny(
            "invalid_write_action",
            "Write action requires at least one target.",
            "Provide the target paths for the supported action.",
        ));
    }
    for target in &request.payload.targets {
        let claimed = store.active_claims_for_path(&request.workspace.workspace_id, &target.path)?
            .into_iter()
            .any(|claim| {
                claim.reservation_id == reservation.reservation_id
                    && claim.agent_id == request.agent.agent_id
                    && claim.action == request.payload.action
            });
        if !claimed {
            return Ok(Decision::deny(
                "missing_claim",
                "Each write target requires an active claim held by this reservation.",
                "Acquire an active claim for every target before writing.",
            ));
        }
    }
    let decisions = match request.payload.action.as_str() {
        "write_file" => request.payload.targets.iter()
            .map(|target| authorize_action(&state, AuthorizationInput::write_file(&target.path)))
            .collect::<Vec<_>>(),
        "write_directory" => request.payload.targets.iter()
            .map(|target| authorize_action(&state, AuthorizationInput::write_directory(&target.path)))
            .collect::<Vec<_>>(),
        "delete_file" => request.payload.targets.iter()
            .map(|target| authorize_action(&state, AuthorizationInput::delete_file(&target.path)))
            .collect::<Vec<_>>(),
        "rename_file" if request.payload.targets.len() == 2 => vec![authorize_action(
            &state,
            AuthorizationInput::rename_file(&request.payload.targets[0].path, &request.payload.targets[1].path),
        )],
        "move_file" if request.payload.targets.len() == 2 => vec![authorize_action(
            &state,
            AuthorizationInput::move_file(&request.payload.targets[0].path, &request.payload.targets[1].path),
        )],
        _ => vec![Decision::deny(
            "invalid_write_action",
            "Write action and target set are invalid.",
            "Provide a supported action with its required target paths.",
        )],
    };
    Ok(decisions.into_iter().find(|decision| decision.decision == DecisionKind::Deny)
        .unwrap_or_else(|| Decision::allow("authorized", "Action is authorized.")))
}
