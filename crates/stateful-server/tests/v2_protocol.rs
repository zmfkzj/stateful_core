use axum::{Router, body::Body, http::{Request, StatusCode}};
use serde_json::{Value, json};
use stateful_server::{ServerConfig, build_router};
use stateful_store::Store;
use std::time::Duration;
use tokio_stream::StreamExt;
use tower::ServiceExt;

fn app() -> Router {
    build_router(ServerConfig::new("test-token"))
}

fn app_with_store(store: Store) -> Router {
    build_router(ServerConfig::with_store("test-token", store))
}

fn identity() -> Value {
    json!({
        "agent_id": "agent-1",
        "actor_id": "actor-1",
        "actor_type": "agent",
        "root": "/workspace",
        "workspace_id": "workspace-1",
        "repo_id": "repo-1",
        "worktree_id": "worktree-1",
        "branch": "main",
        "kind": "hook",
        "event": "test",
        "source_ref": "test-1"
    })
}

fn envelope(payload: Value) -> Value {
    let mut body = identity();
    body["protocol_version"] = json!("stateful.v2");
    body["request_id"] = json!("00000000-0000-4000-8000-000000000001");
    body["observed_at"] = json!("2026-07-16T00:00:00Z");
    body["agent"] = json!({
        "agent_id": body["agent_id"],
        "actor_id": body["actor_id"],
        "actor_type": body["actor_type"]
    });
    body["workspace"] = json!({
        "root": body["root"],
        "workspace_id": body["workspace_id"],
        "repo_id": body["repo_id"],
        "worktree_id": body["worktree_id"],
        "branch": body["branch"]
    });
    body["source"] = json!({
        "kind": body["kind"],
        "event": body["event"],
        "source_ref": body["source_ref"]
    });
    body["payload"] = payload;
    body
}

fn envelope_for(agent_id: &str, request_id: &str, payload: Value) -> Value {
    let mut body = envelope(payload);
    body["request_id"] = json!(request_id);
    body["agent_id"] = json!(agent_id);
    body["actor_id"] = json!(format!("{agent_id}-actor"));
    body["source_ref"] = json!(format!("test-{request_id}"));
    body["agent"]["agent_id"] = json!(agent_id);
    body["agent"]["actor_id"] = json!(format!("{agent_id}-actor"));
    body["source"]["source_ref"] = json!(format!("test-{request_id}"));
    body
}

fn post(path: &str, body: Value) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri(path)
        .header("authorization", "Bearer test-token")
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .unwrap()
}

fn get(path: &str) -> Request<Body> {
    Request::builder()
        .uri(path)
        .header("authorization", "Bearer test-token")
        .body(Body::empty())
        .unwrap()
}

fn stream_get(last_event_id: Option<u64>) -> Request<Body> {
    let uri = "/v2/notifications/stream?protocol_version=stateful.v2&request_id=00000000-0000-4000-8000-0000feed0001&observed_at=2026-07-16T00%3A00%3A00Z&agent_id=agent-2&actor_id=agent-2-actor&actor_type=agent&root=%2Fworkspace&workspace_id=workspace-1&repo_id=repo-1&worktree_id=worktree-1&branch=main&kind=hook&event=test&source_ref=stream-test";
    let mut builder = Request::builder()
        .uri(uri)
        .header("authorization", "Bearer test-token");
    if let Some(last_event_id) = last_event_id {
        builder = builder.header("last-event-id", last_event_id);
    }
    builder.body(Body::empty()).unwrap()
}

async fn response_json(response: axum::response::Response) -> Value {
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

fn query_get(path: &str, agent_id: &str, request_id: &str, workspace_id: &str) -> Request<Body> {
    get(&format!(
        "{path}?protocol_version=stateful.v2&request_id={request_id}&observed_at=2026-07-16T00%3A00%3A00Z&agent_id={agent_id}&actor_id={agent_id}-actor&actor_type=agent&root=%2Fworkspace&workspace_id={workspace_id}&repo_id=repo-1&worktree_id=worktree-1&branch=main&kind=hook&event=test&source_ref=query-{request_id}",
    ))
}

async fn successful_post(app: &Router, path: &str, body: Value) -> Value {
    let response = app.clone().oneshot(post(path, body)).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK, "{path}");
    response_json(response).await
}

#[tokio::test]
async fn v1_routes_are_absent() {
    assert_eq!(app().oneshot(get("/v1/current")).await.unwrap().status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn v2_body_with_v1_protocol_returns_unsupported_protocol() {
    let mut body = envelope(json!({"first_prompt": "work"}));
    body["protocol_version"] = json!("stateful.v1");
    let response = app().oneshot(post("/v2/session/register", body)).await.unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert_eq!(response_json(response).await["error"]["code"], "unsupported_protocol");
}

#[tokio::test]
async fn get_queries_reject_missing_identity() {
    let response = app().oneshot(get("/v2/current?protocol_version=stateful.v2")).await.unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn duplicate_mutation_returns_identical_frozen_response() {
    let request = envelope(json!({"first_prompt": "work"}));
    let app = app();
    let first = app.clone().oneshot(post("/v2/session/register", request.clone())).await.unwrap();
    assert_eq!(first.status(), StatusCode::OK);
    let first = response_json(first).await;
    let second = app.oneshot(post("/v2/session/register", request)).await.unwrap();
    assert_eq!(second.status(), StatusCode::OK);
    assert_eq!(response_json(second).await, first);
}

#[tokio::test]
async fn runtime_identity_reports_v2_schema_mode_version_and_capabilities() {
    let response = app().oneshot(get("/v2/runtime/identity?protocol_version=stateful.v2&request_id=00000000-0000-4000-8000-000000000001&observed_at=2026-07-16T00%3A00%3A00Z&agent_id=agent-1&actor_id=actor-1&actor_type=agent&root=%2Fworkspace&workspace_id=workspace-1&repo_id=repo-1&worktree_id=worktree-1&branch=main&kind=hook&event=test&source_ref=test-1")).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = response_json(response).await;
    assert_eq!(body["protocol_version"], "stateful.v2");
    assert_eq!(body["journal_schema_version"], 2);
    assert_eq!(body["coordination_mode"], "awareness");
    assert_eq!(body["workspace_id"], "workspace-1");
    assert!(body["capabilities"].as_array().unwrap().iter().any(|value| value == "presence"));
}

#[tokio::test]
async fn v2_post_surface_is_registered() {
    for path in [
        "/v2/session/register", "/v2/presence/update", "/v2/read/start", "/v2/read/complete",
        "/v2/write/complete", "/v2/activity/finalize", "/v2/reservation/declare", "/v2/reservation/request",
        "/v2/reservation/claim", "/v2/reservation/cancel", "/v2/claim/acquire", "/v2/claim/release",
        "/v2/authorize", "/v2/human/observe", "/v2/human/save-check", "/v2/reconcile/ack",
        "/v2/context/render", "/v2/context/ack", "/v2/notifications/poll", "/v2/resume/next", "/v2/outbox/sync",
    ] {
        assert_ne!(app().oneshot(post(path, envelope(json!({})))).await.unwrap().status(), StatusCode::NOT_FOUND, "{path}");
    }
}

#[tokio::test]
async fn v2_get_surface_is_registered() {
    for path in ["/v2/current", "/v2/events", "/v2/notifications/stream", "/v2/runtime/identity"] {
        assert_ne!(app().oneshot(get(path)).await.unwrap().status(), StatusCode::NOT_FOUND, "{path}");
    }
}

#[tokio::test]
async fn payload_cannot_override_request_actor() {
    let response = app()
        .oneshot(post(
            "/v2/session/register",
            envelope(json!({"first_prompt": "work", "actor_id": "forged-actor"})),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response_json(response).await["actor_id"], "actor-1");
}

#[tokio::test]
async fn nil_request_id_is_rejected_without_server_replacement() {
    let mut body = envelope(json!({"first_prompt": "work"}));
    body["request_id"] = json!("00000000-0000-0000-0000-000000000000");
    let response = app().oneshot(post("/v2/session/register", body)).await.unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let body = response_json(response).await;
    assert_eq!(body["request_id"], "00000000-0000-0000-0000-000000000000");
    assert_eq!(body["error"]["code"], "invalid_request_id");
}

macro_rules! mutation_rejects_non_v2 {
    ($name:ident, $path:literal) => {
        #[tokio::test]
        async fn $name() {
            let mut body = envelope(json!({}));
            body["protocol_version"] = json!("stateful.v1");
            let response = app().oneshot(post($path, body)).await.unwrap();
            assert_eq!(response.status(), StatusCode::BAD_REQUEST);
            assert_eq!(response_json(response).await["error"]["code"], "unsupported_protocol");
        }
    };
}

mutation_rejects_non_v2!(presence_update_requires_v2, "/v2/presence/update");
mutation_rejects_non_v2!(read_start_requires_v2, "/v2/read/start");
mutation_rejects_non_v2!(read_complete_requires_v2, "/v2/read/complete");
mutation_rejects_non_v2!(write_complete_requires_v2, "/v2/write/complete");
mutation_rejects_non_v2!(activity_finalize_requires_v2, "/v2/activity/finalize");
mutation_rejects_non_v2!(reservation_declare_requires_v2, "/v2/reservation/declare");
mutation_rejects_non_v2!(reservation_request_requires_v2, "/v2/reservation/request");
mutation_rejects_non_v2!(reservation_claim_requires_v2, "/v2/reservation/claim");
mutation_rejects_non_v2!(reservation_cancel_requires_v2, "/v2/reservation/cancel");
mutation_rejects_non_v2!(claim_acquire_requires_v2, "/v2/claim/acquire");
mutation_rejects_non_v2!(claim_release_requires_v2, "/v2/claim/release");
mutation_rejects_non_v2!(authorize_requires_v2, "/v2/authorize");
mutation_rejects_non_v2!(human_observe_requires_v2, "/v2/human/observe");
mutation_rejects_non_v2!(human_save_check_requires_v2, "/v2/human/save-check");
mutation_rejects_non_v2!(reconcile_ack_requires_v2, "/v2/reconcile/ack");
mutation_rejects_non_v2!(context_render_requires_v2, "/v2/context/render");
mutation_rejects_non_v2!(context_ack_requires_v2, "/v2/context/ack");
mutation_rejects_non_v2!(notifications_poll_requires_v2, "/v2/notifications/poll");
mutation_rejects_non_v2!(resume_next_requires_v2, "/v2/resume/next");
mutation_rejects_non_v2!(outbox_sync_requires_v2, "/v2/outbox/sync");

#[tokio::test]
async fn awareness_warns_for_missing_read_provenance_while_enforcement_denies() {
    let body = envelope(json!({
        "operation_id": "write-1",
        "action": "write_file",
        "targets": [{"path": "src/lib.rs", "before": {"exists": false, "byte_len": 0}}]
    }));
    let awareness = app().oneshot(post("/v2/authorize", body.clone())).await.unwrap();
    assert_eq!(awareness.status(), StatusCode::OK);
    let awareness = response_json(awareness).await;
    assert!(awareness["intent_id"].is_string());
    assert_eq!(awareness["decision"]["decision"], "warn");
    assert_eq!(awareness["decision"]["reason_code"], "missing_read_provenance");

    let enforcement = build_router(
        ServerConfig::new("test-token").with_coordination_mode(stateful_server::CoordinationMode::Enforcement),
    )
    .oneshot(post("/v2/authorize", body))
    .await
    .unwrap();
    assert_eq!(enforcement.status(), StatusCode::FORBIDDEN);
    assert_eq!(response_json(enforcement).await["reason_code"], "missing_read_provenance");
}

#[tokio::test]
async fn malformed_post_and_invalid_flattened_query_return_v2_errors() {
    let post_response = app()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v2/session/register")
                .header("authorization", "Bearer test-token")
                .header("content-type", "application/json")
                .body(Body::from("{"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(post_response.status(), StatusCode::BAD_REQUEST);
    assert_eq!(response_json(post_response).await["protocol_version"], "stateful.v2");

    let get_response = app().oneshot(get("/v2/current?protocol_version=stateful.v1")).await.unwrap();
    assert_eq!(get_response.status(), StatusCode::BAD_REQUEST);
    assert_eq!(response_json(get_response).await["protocol_version"], "stateful.v2");
}

#[tokio::test]
async fn request_id_reused_across_mutation_routes_is_rejected() {
    let app = app();
    let request_id = "00000000-0000-4000-8000-00000000c001";
    assert_eq!(
        app.clone()
            .oneshot(post(
                "/v2/session/register",
                envelope_for("agent-1", request_id, json!({"first_prompt": "work"})),
            ))
            .await
            .unwrap()
            .status(),
        StatusCode::OK,
    );

    let response = app
        .oneshot(post(
            "/v2/presence/update",
            envelope_for("agent-1", request_id, json!({"kind": "heartbeat"})),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CONFLICT);
    assert_eq!(response_json(response).await["error"]["code"], "idempotency_key_reused");
}

#[tokio::test]
async fn sse_reconnect_acknowledges_live_sequence_without_replay() {
    let app = app();
    let response = app
        .clone()
        .oneshot(stream_get(None))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let mut stream = response.into_body().into_data_stream();

    let wait = response_json(
        app.clone()
            .oneshot(post(
                "/v2/reservation/request",
                envelope_for(
                    "agent-2",
                    "00000000-0000-4000-8000-00000000c002",
                    json!({"relative_path": "src/lib.rs", "action": "write_file", "purpose": "Need the file."}),
                ),
            ))
            .await
            .unwrap(),
    )
    .await;
    assert!(wait["wait_id"].is_string());
    let grant = app
        .clone()
        .oneshot(post(
            "/v2/reservation/claim",
            envelope_for(
                "agent-1",
                "00000000-0000-4000-8000-00000000c003",
                json!({"relative_path": "src/lib.rs"}),
            ),
        ))
        .await
        .unwrap();
    assert_eq!(grant.status(), StatusCode::OK);

    let bytes = tokio::time::timeout(Duration::from_secs(1), stream.next())
        .await
        .expect("live notification arrives")
        .expect("stream remains open")
        .expect("SSE data is valid");
    let event = String::from_utf8(bytes.to_vec()).expect("SSE is UTF-8");
    assert!(event.contains("id: 1"));

    let response = app
        .clone()
        .oneshot(stream_get(Some(1)))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let mut reconnected = response.into_body().into_data_stream();
    assert!(
        tokio::time::timeout(Duration::from_millis(250), reconnected.next())
            .await
            .is_err(),
        "acknowledged notifications must not replay",
    );

    let response = app.oneshot(stream_get(None)).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let mut fresh_connection = response.into_body().into_data_stream();
    assert!(
        tokio::time::timeout(Duration::from_millis(250), fresh_connection.next())
            .await
            .is_err(),
        "the delivery acknowledgement must persist beyond its reconnect cursor",
    );
}

#[tokio::test]
async fn health_and_runtime_identity_are_available_with_bearer_enforcement() {
    let app = app();
    assert_eq!(app.clone().oneshot(get("/health")).await.unwrap().status(), StatusCode::OK);
    assert_eq!(
        app.clone()
            .oneshot(Request::builder().uri("/v2/current").body(Body::empty()).unwrap())
            .await
            .unwrap()
            .status(),
        StatusCode::UNAUTHORIZED,
    );
    assert_eq!(
        app.clone()
            .oneshot(query_get(
                "/v2/runtime/identity",
                "agent-1",
                "00000000-0000-4000-8000-00000000d001",
                "workspace-1",
            ))
            .await
            .unwrap()
            .status(),
        StatusCode::OK,
    );
    assert_eq!(app.oneshot(get("/health")).await.unwrap().status(), StatusCode::OK);
}

#[tokio::test]
async fn store_failures_return_sanitized_v2_internal_errors() {
    let mut store = Store::open_in_memory().expect("store opens");
    store.fail_projector_on_event_for_tests(1);
    let response = app_with_store(store)
        .oneshot(post(
            "/v2/session/register",
            envelope_for(
                "agent-1",
                "00000000-0000-4000-8000-00000000d002",
                json!({"first_prompt": "work"}),
            ),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
    let body = response_json(response).await;
    assert_eq!(body["error"]["code"], "internal_error");
    assert_eq!(body["error"]["message"], "The server could not complete the request.");
}

#[tokio::test]
async fn every_v2_route_executes_a_real_store_flow() {
    let app = app();
    successful_post(
        &app,
        "/v2/session/register",
        envelope_for("agent-1", "00000000-0000-4000-8000-00000000d101", json!({"first_prompt": "work"})),
    )
    .await;
    successful_post(
        &app,
        "/v2/presence/update",
        envelope_for("agent-1", "00000000-0000-4000-8000-00000000d102", json!({"kind": "update", "phase": "editing"})),
    )
    .await;
    successful_post(
        &app,
        "/v2/read/start",
        envelope_for("agent-1", "00000000-0000-4000-8000-00000000d103", json!({
            "operation_id": "read-1", "path": "src/lib.rs", "before": {"exists": false, "byte_len": 0}
        })),
    )
    .await;
    successful_post(
        &app,
        "/v2/read/complete",
        envelope_for("agent-1", "00000000-0000-4000-8000-00000000d104", json!({
            "operation_id": "read-1", "path": "src/lib.rs", "classification": "exact",
            "after": {"exists": false, "byte_len": 0}
        })),
    )
    .await;
    successful_post(
        &app,
        "/v2/human/observe",
        envelope_for("agent-2", "00000000-0000-4000-8000-00000000d110", json!({
            "relative_path": "src/lib.rs", "kind": "save", "confidence": "high",
            "source": "watcher", "summary": "human save"
        })),
    )
    .await;
    let save_check = successful_post(
        &app,
        "/v2/human/save-check",
        envelope_for("agent-1", "00000000-0000-4000-8000-00000000d111", json!({"paths": ["src/lib.rs"]})),
    )
    .await;
    assert_eq!(save_check["blocked"], true);
    successful_post(
        &app,
        "/v2/reconcile/ack",
        envelope_for("agent-1", "00000000-0000-4000-8000-00000000d112", json!({
            "decision": "adopt", "files_reread": ["src/lib.rs"], "human_change_summary": "adopted"
        })),
    )
    .await;


    let reservation = successful_post(
        &app,
        "/v2/reservation/declare",
        envelope_for("agent-1", "00000000-0000-4000-8000-00000000d105", json!({
            "relative_path": "src/lib.rs", "action": "write_file", "purpose": "Update the module."
        })),
    )
    .await;
    let reservation_id = reservation["reservation_id"].as_str().expect("reservation id");
    let claims = successful_post(
        &app,
        "/v2/claim/acquire",
        envelope_for("agent-1", "00000000-0000-4000-8000-00000000d106", json!({
            "reservation_id": reservation_id,
            "paths": [{"relative_path": "src/lib.rs", "observation": {"exists": false}}]
        })),
    )
    .await;
    let claim_id = claims["claims"][0]["claim_id"].as_str().expect("claim id");
    let intent = successful_post(
        &app,
        "/v2/authorize",
        envelope_for("agent-1", "00000000-0000-4000-8000-00000000d107", json!({
            "reservation_id": reservation_id,
            "operation_id": "write-1",
            "action": "write_file",
            "targets": [{"path": "src/lib.rs", "before": {"exists": false, "byte_len": 0}}]
        })),
    )
    .await;
    assert_eq!(intent["decision"]["decision"], "allow");
    let intent_id = intent["intent_id"].as_str().expect("write intent id");
    successful_post(
        &app,
        "/v2/write/complete",
        envelope_for("agent-1", "00000000-0000-4000-8000-00000000d108", json!({
            "intent_id": intent_id, "outcome": "committed",
            "post_fingerprints": [["src/lib.rs", {"exists": false, "byte_len": 0}]]
        })),
    )
    .await;
    successful_post(
        &app,
        "/v2/claim/release",
        envelope_for("agent-1", "00000000-0000-4000-8000-00000000d109", json!({"claim_id": claim_id})),
    )
    .await;


    let context = successful_post(
        &app,
        "/v2/context/render",
        envelope_for("agent-1", "00000000-0000-4000-8000-00000000d113", json!({"mode": "brief"})),
    )
    .await;
    assert_eq!(context["changed"], true);
    successful_post(
        &app,
        "/v2/context/ack",
        envelope_for("agent-1", "00000000-0000-4000-8000-00000000d114", json!({
            "delivery_id": context["delivery_id"],
            "sequence": context["sequence"],
            "workspace_version": context["workspace_version"]
        })),
    )
    .await;

    let wait = successful_post(
        &app,
        "/v2/reservation/request",
        envelope_for("agent-2", "00000000-0000-4000-8000-00000000d115", json!({
            "relative_path": "src/queued.rs", "action": "write_file", "purpose": "Need this file."
        })),
    )
    .await;
    let wait_id = wait["wait_id"].as_str().expect("wait id");
    let granted = successful_post(
        &app,
        "/v2/reservation/claim",
        envelope_for("agent-1", "00000000-0000-4000-8000-00000000d116", json!({"relative_path": "src/queued.rs"})),
    )
    .await;
    assert_eq!(granted["wait_id"], wait_id);
    successful_post(
        &app,
        "/v2/reservation/cancel",
        envelope_for("agent-2", "00000000-0000-4000-8000-00000000d117", json!({"wait_id": wait_id})),
    )
    .await;
    assert!(
        successful_post(
            &app,
            "/v2/notifications/poll",
            envelope_for("agent-2", "00000000-0000-4000-8000-00000000d118", json!({})),
        )
        .await
        .is_array(),
    );
    successful_post(
        &app,
        "/v2/resume/next",
        envelope_for("agent-2", "00000000-0000-4000-8000-00000000d119", json!({})),
    )
    .await;
    successful_post(
        &app,
        "/v2/outbox/sync",
        envelope_for("agent-1", "00000000-0000-4000-8000-00000000d120", json!({
            "outbox_id": "outbox-1", "sequence": 1, "event_type": "heartbeat", "payload": {"ok": true}
        })),
    )
    .await;
    successful_post(
        &app,
        "/v2/activity/finalize",
        envelope_for("agent-1", "00000000-0000-4000-8000-00000000d121", json!({})),
    )
    .await;

    let current = app
        .clone()
        .oneshot(query_get("/v2/current", "agent-1", "00000000-0000-4000-8000-00000000d122", "workspace-1"))
        .await
        .unwrap();
    assert_eq!(current.status(), StatusCode::OK);
    let events = app
        .clone()
        .oneshot(query_get("/v2/events", "agent-1", "00000000-0000-4000-8000-00000000d123", "workspace-1"))
        .await
        .unwrap();
    assert_eq!(events.status(), StatusCode::OK);
    assert!(response_json(events).await["events"].as_array().expect("events array").len() > 1);
    assert_eq!(
        app.oneshot(query_get(
            "/v2/runtime/identity",
            "agent-1",
            "00000000-0000-4000-8000-00000000d124",
            "workspace-1",
        ))
        .await
        .unwrap()
        .status(),
        StatusCode::OK,
    );
}

#[tokio::test]
async fn context_acknowledgement_requires_the_delivery_sequence() {
    let app = app();
    successful_post(
        &app,
        "/v2/session/register",
        envelope_for("agent-1", "00000000-0000-4000-8000-00000000d201", json!({"first_prompt": "work"})),
    )
    .await;
    let delivery = successful_post(
        &app,
        "/v2/context/render",
        envelope_for("agent-1", "00000000-0000-4000-8000-00000000d202", json!({"mode": "brief"})),
    )
    .await;
    let invalid = app
        .clone()
        .oneshot(post(
            "/v2/context/ack",
            envelope_for("agent-1", "00000000-0000-4000-8000-00000000d203", json!({
                "delivery_id": delivery["delivery_id"],
                "sequence": delivery["sequence"].as_u64().expect("sequence") + 1,
                "workspace_version": delivery["workspace_version"]
            })),
        ))
        .await
        .unwrap();
    assert_eq!(invalid.status(), StatusCode::BAD_REQUEST);
    assert_eq!(response_json(invalid).await["error"]["code"], "invalid_context_delivery");
    successful_post(
        &app,
        "/v2/context/ack",
        envelope_for("agent-1", "00000000-0000-4000-8000-00000000d204", json!({
            "delivery_id": delivery["delivery_id"],
            "sequence": delivery["sequence"],
            "workspace_version": delivery["workspace_version"]
        })),
    )
    .await;
}

#[tokio::test]
async fn event_reads_are_scoped_to_the_requested_workspace() {
    let app = app();
    successful_post(
        &app,
        "/v2/session/register",
        envelope_for("agent-1", "00000000-0000-4000-8000-00000000d205", json!({"first_prompt": "workspace one"})),
    )
    .await;
    let mut other_workspace = envelope_for(
        "agent-1",
        "00000000-0000-4000-8000-00000000d206",
        json!({"first_prompt": "workspace two"}),
    );
    other_workspace["workspace_id"] = json!("workspace-2");
    other_workspace["workspace"]["workspace_id"] = json!("workspace-2");
    successful_post(&app, "/v2/session/register", other_workspace).await;

    let response = app
        .oneshot(query_get("/v2/events", "agent-1", "00000000-0000-4000-8000-00000000d207", "workspace-1"))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert!(
        response_json(response).await["events"]
            .as_array()
            .expect("events array")
            .iter()
            .all(|event| event["workspace_id"] == "workspace-1"),
    );
}
