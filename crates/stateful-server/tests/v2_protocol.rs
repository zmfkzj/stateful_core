use axum::{Router, body::Body, http::{Request, StatusCode}};
use serde_json::{Value, json};
use stateful_server::{ServerConfig, build_router};
use tower::ServiceExt;

fn app() -> Router {
    build_router(ServerConfig::new("test-token"))
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

async fn response_json(response: axum::response::Response) -> Value {
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
    serde_json::from_slice(&bytes).unwrap()
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
    assert!(response_json(awareness).await["intent_id"].is_string());

    let enforcement = build_router(
        ServerConfig::new("test-token").with_coordination_mode(stateful_server::CoordinationMode::Enforcement),
    )
    .oneshot(post("/v2/authorize", body))
    .await
    .unwrap();
    assert_eq!(enforcement.status(), StatusCode::FORBIDDEN);
    assert_eq!(response_json(enforcement).await["reason_code"], "missing_read_provenance");
}
