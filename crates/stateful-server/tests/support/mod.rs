#![allow(dead_code)]

use axum::{
    Router,
    body::Body,
    http::{Request, StatusCode},
};
use serde_json::{Value, json};
use stateful_server::{ServerConfig, build_router};
use stateful_store::Store;
use tower::ServiceExt;

pub fn app() -> Router {
    build_router(ServerConfig::new("test-token"))
}

pub fn app_with_store(store: Store) -> Router {
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

pub fn envelope(payload: Value) -> Value {
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

pub fn envelope_for(agent_id: &str, request_id: &str, payload: Value) -> Value {
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

pub fn post(path: &str, body: Value) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri(path)
        .header("authorization", "Bearer test-token")
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .expect("test POST request builds")
}

pub fn get(path: &str) -> Request<Body> {
    Request::builder()
        .uri(path)
        .header("authorization", "Bearer test-token")
        .body(Body::empty())
        .expect("test GET request builds")
}

pub async fn response_json(response: axum::response::Response) -> Value {
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("response body reads");
    serde_json::from_slice(&bytes).expect("response body is valid JSON")
}

pub fn query_get(
    path: &str,
    agent_id: &str,
    request_id: &str,
    workspace_id: &str,
) -> Request<Body> {
    get(&format!(
        "{path}?protocol_version=stateful.v2&request_id={request_id}&observed_at=2026-07-16T00%3A00%3A00Z&agent_id={agent_id}&actor_id={agent_id}-actor&actor_type=agent&root=%2Fworkspace&workspace_id={workspace_id}&repo_id=repo-1&worktree_id=worktree-1&branch=main&kind=hook&event=test&source_ref=query-{request_id}",
    ))
}

pub async fn successful_post(app: &Router, path: &str, body: Value) -> Value {
    let response = app
        .clone()
        .oneshot(post(path, body))
        .await
        .expect("test request receives a response");
    assert_eq!(response.status(), StatusCode::OK, "{path}");
    response_json(response).await
}
