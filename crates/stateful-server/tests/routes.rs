use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode},
};
use serde_json::{Value, json};
use stateful_server::{ServerConfig, build_router};
use tower::ServiceExt;

fn command(request_id: &str, payload: Value) -> Value {
    json!({
        "protocol_version": "stateful.v2",
        "contract_revision": "lease-1",
        "task_id": "task-1",
        "request_id": request_id,
        "observed_at": "2026-08-02T00:00:00Z",
        "agent": {
            "agent_id": "agent-1",
            "actor_id": "actor-1",
            "actor_type": "agent"
        },
        "workspace": {
            "root": "/tmp/workspace",
            "workspace_id": "workspace-1",
            "repo_id": "repo-1",
            "worktree_id": "worktree-1",
            "branch": "main"
        },
        "source": {
            "kind": "cli",
            "event": "test",
            "source_ref": "route-test"
        },
        "payload": payload
    })
}

async fn call(
    method: &str,
    uri: &str,
    body: Option<Value>,
    authenticated: bool,
) -> (StatusCode, Value) {
    let app = build_router(ServerConfig::new("secret"));
    let mut request = Request::builder().method(method).uri(uri);
    if authenticated {
        request = request.header("authorization", "Bearer secret");
    }
    let request = request
        .header("content-type", "application/json")
        .body(Body::from(
            body.map_or_else(String::new, |value| value.to_string()),
        ))
        .expect("request should build");
    let response = app.oneshot(request).await.expect("request should complete");
    let status = response.status();
    let bytes = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body should load");
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).expect("body should be JSON")
    };
    (status, body)
}

async fn authenticated_post(app: axum::Router, uri: &str, body: Value) -> (StatusCode, Value) {
    let request = Request::builder()
        .method("POST")
        .uri(uri)
        .header("authorization", "Bearer secret")
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .expect("request should build");
    let response = app.oneshot(request).await.expect("request should complete");
    let status = response.status();
    let body = serde_json::from_slice(
        &to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body should load"),
    )
    .expect("body should be JSON");
    (status, body)
}
#[tokio::test]
async fn health_is_public_but_v2_state_requires_authentication() {
    assert_eq!(call("GET", "/health", None, false).await.0, StatusCode::OK);
    let (status, body) = call("GET", "/v2/status", None, false).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(body["reason_code"], "unauthorized");
    assert_eq!(
        call("POST", "/v2/commits/prepare", Some(json!({})), false)
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
}

#[tokio::test]
async fn task_command_uses_v2_envelope_and_updates_status() {
    let app = build_router(ServerConfig::new("secret"));
    let start = command(
        "request-1",
        json!({
            "next_action": "edit src/lib.rs",
            "settings": {
                "heartbeat_interval_seconds": 1,
                "inactivity_timeout_seconds": 5,
                "lease_expiry_seconds": 60,
                "offer_ttl_seconds": 120
            },
            "expires_at": "2026-08-02T00:00:05Z"
        }),
    );
    let request = Request::builder()
        .method("POST")
        .uri("/v2/tasks/start")
        .header("authorization", "Bearer secret")
        .header("content-type", "application/json")
        .body(Body::from(start.to_string()))
        .expect("request should build");
    let response = app
        .clone()
        .oneshot(request)
        .await
        .expect("start should complete");
    assert_eq!(response.status(), StatusCode::OK);
    let body: Value = serde_json::from_slice(
        &to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body should load"),
    )
    .expect("body should be JSON");
    assert_eq!(body["request_id"], "request-1");
    assert_eq!(body["payload"]["status"], "active");

    let mismatch = command(
        "request-1",
        json!({
            "next_action": "different action",
            "settings": {
                "heartbeat_interval_seconds": 1,
                "inactivity_timeout_seconds": 5,
                "lease_expiry_seconds": 60,
                "offer_ttl_seconds": 120
            },
            "expires_at": "2026-08-02T00:00:05Z"
        }),
    );
    let (status, body) = authenticated_post(app.clone(), "/v2/tasks/start", mismatch).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(body["reason_code"], "idempotency_mismatch");
    assert_eq!(body["request_id"], "request-1");

    let mut wrong_agent = command(
        "request-2",
        json!({
            "next_action": "edit src/lib.rs",
            "expires_at": "2026-08-02T00:00:05Z"
        }),
    );
    wrong_agent["agent"]["agent_id"] = json!("agent-2");
    wrong_agent["agent"]["actor_id"] = json!("actor-2");
    let (status, body) = authenticated_post(app.clone(), "/v2/tasks/heartbeat", wrong_agent).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(body["reason_code"], "ownership_violation");
    assert_eq!(body["request_id"], "request-2");

    let request = Request::builder()
        .method("GET")
        .uri("/v2/status")
        .header("authorization", "Bearer secret")
        .body(Body::empty())
        .expect("request should build");
    let response = app.oneshot(request).await.expect("status should complete");
    let body: Value = serde_json::from_slice(
        &to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body should load"),
    )
    .expect("body should be JSON");
    assert_eq!(body["payload"]["active_tasks"], 1);
}

#[tokio::test]
async fn v1_routes_and_protocol_values_are_rejected() {
    assert_eq!(
        call("GET", "/v1/current", None, true).await.0,
        StatusCode::NOT_FOUND
    );
    let mut body = command(
        "request-1",
        json!({
            "next_action": "edit src/lib.rs",
            "settings": {
                "heartbeat_interval_seconds": 1,
                "inactivity_timeout_seconds": 5,
                "lease_expiry_seconds": 60,
                "offer_ttl_seconds": 120
            },
            "expires_at": "2026-08-02T00:00:05Z"
        }),
    );
    body["protocol_version"] = json!("stateful.v1");
    let (status, body) = call("POST", "/v2/tasks/start", Some(body), true).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["reason_code"], "protocol_mismatch");
}
