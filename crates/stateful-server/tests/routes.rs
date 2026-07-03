use axum::{
    Router,
    body::{Body, to_bytes},
    http::{Request, Response, StatusCode},
};
use stateful_server::{ServerConfig, build_router, serve_listener};
use stateful_store::{Event, Store};
use std::{sync::Arc, time::Duration};
use tokio_stream::StreamExt;
use tower::ServiceExt;

fn acquire_test_lease(store: &Store, agent_id: &str, workspace_id: &str, path: &str) {
    let has_matching_reservation = store
        .live_current_state(Some(path))
        .expect("live current state should load")
        .items
        .iter()
        .any(|item| {
            item.kind == stateful_core::CurrentItemKind::Reservation
                && item.agent_id.as_deref() == Some(agent_id)
        });
    if !has_matching_reservation {
        store
            .append(Event::reservation_declared(
                agent_id,
                workspace_id,
                format!("Acquire test claim for {path}."),
                [path],
            ))
            .expect("claim reservation should append");
    }
    store
        .acquire_claim(agent_id, workspace_id, path)
        .expect("claim should acquire");
}

async fn ensure_test_reservation_via_http(
    app: &Router,
    agent_id: &str,
    workspace_id: &str,
    path: &str,
) {
    let current = app
        .clone()
        .oneshot(authorized_get(&format!("/v1/current?resource={path}")))
        .await
        .expect("current request should complete");
    if current.status() == StatusCode::OK {
        let current = response_json(current, 4096).await;
        let has_matching_reservation = current["items"].as_array().is_some_and(|items| {
            items.iter().any(|item| {
                item["kind"] == "reservation"
                    && item["freshness"] == "live"
                    && item["resource"] == path
                    && item["agent_id"] == agent_id
                    && item["workspace_id"] == workspace_id
            })
        });
        if has_matching_reservation {
            return;
        }
    }

    let declare = app
        .clone()
        .oneshot(protocol_request(
            "/v1/reservation/declare",
            agent_id,
            workspace_id,
            serde_json::json!({
                "purpose": format!("Acquire test claim for {path}."),
                "files_planned": [path]
            }),
        ))
        .await
        .expect("reservation declaration should complete");
    assert_eq!(declare.status(), StatusCode::OK);
}

#[tokio::test]
async fn health_is_public_but_authorize_requires_token() {
    let app = build_router(ServerConfig::new("secret-token"));

    let health = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/health")
                .body(Body::empty())
                .expect("health request should build"),
        )
        .await
        .expect("health response should complete");
    assert_eq!(health.status(), StatusCode::OK);

    let denied = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/authorize")
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::json!({
                        "agent_id": "s1",
                        "action": "write_file",
                        "path": "src/auth.ts"
                    })
                    .to_string(),
                ))
                .expect("authorize request should build"),
        )
        .await
        .expect("authorize response should complete");
    assert_eq!(denied.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn runtime_identity_requires_token_and_returns_process_identity() {
    let app = build_router(ServerConfig::new("secret-token"));

    let denied = app
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/v1/runtime/identity")
                .body(Body::empty())
                .expect("identity request should build"),
        )
        .await
        .expect("identity response should complete");
    assert_eq!(denied.status(), StatusCode::UNAUTHORIZED);

    let response = app
        .oneshot(authorized_get("/v1/runtime/identity"))
        .await
        .expect("identity response should complete");
    assert_eq!(response.status(), StatusCode::OK);

    let body = to_bytes(response.into_body(), 1024)
        .await
        .expect("body should read");
    let json: serde_json::Value = serde_json::from_slice(&body).expect("body should be json");
    assert_eq!(json["status"], "ok");
    assert_eq!(json["pid"], std::process::id());
    assert_eq!(json["protocol_version"], "stateful.v1");
    assert_eq!(
        json["capabilities"],
        serde_json::json!(["authorize.write_directory"])
    );
}

#[tokio::test]
async fn authorize_accepts_matching_bearer_token() {
    let app = build_router(ServerConfig::new("secret-token"));

    let response = app
        .oneshot(protocol_request(
            "/v1/authorize",
            "s1",
            "w1",
            serde_json::json!({
                "action": "write_file",
                "path": "src/auth.ts"
            }),
        ))
        .await
        .expect("authorized response should complete");

    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn authorize_accepts_hook_source_kind() {
    let app = build_router(ServerConfig::new("secret-token"));
    let mut body = protocol_body(
        "s1",
        "w1",
        serde_json::json!({
            "action": "write_file",
            "path": "src/auth.ts"
        }),
    );
    body["source"]["kind"] = serde_json::json!("hook");
    body["source"]["event"] = serde_json::json!("pre_tool_use");

    let response = app
        .oneshot(json_request("/v1/authorize", body))
        .await
        .expect("authorized response should complete");

    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn lease_release_rejects_other_agent_owner_with_actionable_error() {
    let app = build_router(ServerConfig::new("secret-token"));
    ensure_test_reservation_via_http(&app, "s1", "w1", "target/").await;
    let acquire = app
        .clone()
        .oneshot(json_request(
            "/v1/claim/acquire",
            serde_json::json!({
                "agent_id": "s1",
                "workspace_id": "w1",
                "path": "target/"
            }),
        ))
        .await
        .expect("claim acquire should complete");
    assert_eq!(acquire.status(), StatusCode::OK);

    let response = app
        .oneshot(json_request(
            "/v1/claim/release",
            serde_json::json!({
                "agent_id": "s2",
                "workspace_id": "w1",
                "path": "target/"
            }),
        ))
        .await
        .expect("claim release should complete");

    assert_eq!(response.status(), StatusCode::CONFLICT);
    let json = response_json(response, 2048).await;
    assert_eq!(json["status"], "error");
    assert_eq!(json["reason_code"], "claim_owner_mismatch");
    assert!(
        json["message"]
            .as_str()
            .is_some_and(|message| message.contains("wait") && message.contains("coordinate")),
        "message should direct the caller to wait or coordinate: {json}"
    );
}

#[tokio::test]
async fn lease_acquire_returns_already_held_for_duplicate_same_agent() {
    let app = build_router(ServerConfig::new("secret-token"));
    ensure_test_reservation_via_http(&app, "s1", "w1", "src/auth.ts").await;

    let first = app
        .clone()
        .oneshot(json_request(
            "/v1/claim/acquire",
            serde_json::json!({
                "agent_id": "s1",
                "workspace_id": "w1",
                "path": "src/auth.ts"
            }),
        ))
        .await
        .expect("first claim acquire should complete");
    assert_eq!(first.status(), StatusCode::OK);

    let second = app
        .oneshot(json_request(
            "/v1/claim/acquire",
            serde_json::json!({
                "agent_id": "s1",
                "workspace_id": "w1",
                "path": "src/auth.ts"
            }),
        ))
        .await
        .expect("duplicate claim acquire should complete");

    assert_eq!(second.status(), StatusCode::OK);
    let json = response_json(second, 2048).await;
    assert_eq!(json["status"], "ok");
    assert_eq!(json["claim_state"], "already_held");
    assert!(
        json["message"]
            .as_str()
            .is_some_and(|message| message.contains("already holds an active claim")),
        "message should report the existing active claim: {json}"
    );
}

#[tokio::test]
async fn lease_acquire_accepts_batch_paths() {
    let app = build_router(ServerConfig::new("secret-token"));
    let declare = app
        .clone()
        .oneshot(protocol_request(
            "/v1/reservation/declare",
            "s1",
            "w1",
            serde_json::json!({
                "purpose": "Acquire claims for a batch edit.",
                "files_planned": ["src/auth.ts", "src/session.ts"]
            }),
        ))
        .await
        .expect("reservation declaration should complete");
    assert_eq!(declare.status(), StatusCode::OK);

    let response = app
        .clone()
        .oneshot(json_request(
            "/v1/claim/acquire",
            serde_json::json!({
                "agent_id": "s1",
                "workspace_id": "w1",
                "paths": ["src/auth.ts", "src/session.ts"]
            }),
        ))
        .await
        .expect("batch claim acquire should complete");

    assert_eq!(response.status(), StatusCode::OK);
    let json = response_json(response, 2048).await;
    assert_eq!(json["status"], "ok");
    assert_eq!(json["claim_state"], "acquired");
    assert_eq!(json["acquired"], 2);
    assert_eq!(json["already_held"], 0);
    assert_eq!(
        json["paths"],
        serde_json::json!(["src/auth.ts", "src/session.ts"])
    );

    let current = app
        .oneshot(authorized_get("/v1/current?resource=src/auth.ts"))
        .await
        .expect("current request should complete");
    assert_eq!(current.status(), StatusCode::OK);
    let current = response_json(current, 4096).await;
    assert!(current["items"].as_array().is_some_and(|items| {
        items.iter().any(|item| {
            item["kind"] == "claim" && item["resource"] == "src/auth.ts" && item["agent_id"] == "s1"
        })
    }));
}

#[tokio::test]
async fn lease_release_rejects_missing_same_agent_lease() {
    let app = build_router(ServerConfig::new("secret-token"));

    let response = app
        .oneshot(json_request(
            "/v1/claim/release",
            serde_json::json!({
                "agent_id": "s1",
                "workspace_id": "w1",
                "path": "target/"
            }),
        ))
        .await
        .expect("claim release should complete");

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    let json = response_json(response, 2048).await;
    assert_eq!(json["status"], "error");
    assert_eq!(json["reason_code"], "claim_not_found");
    assert!(
        json["message"]
            .as_str()
            .is_some_and(|message| message.contains("same-agent claim")),
        "message should explain no same-agent claim was released: {json}"
    );
}

#[tokio::test]
async fn side_effecting_routes_reservation_declare_rejects_legacy_body() {
    let app = build_router(ServerConfig::new("secret-token"));

    let response = app
        .oneshot(json_request(
            "/v1/reservation/declare",
            serde_json::json!({
                "agent_id": "s1",
                "workspace_id": "w1",
                "purpose": "Test requested work.",
                "files_planned": ["src/auth.ts"]
            }),
        ))
        .await
        .expect("reservation declaration should complete");
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);

    let body = to_bytes(response.into_body(), 1024)
        .await
        .expect("body should read");
    let json: serde_json::Value = serde_json::from_slice(&body).expect("body should be json");
    assert_eq!(json["decision"], "error");
    assert_eq!(json["reason_code"], "protocol_mismatch");
}

#[tokio::test]
async fn side_effecting_routes_reservation_declare_accepts_protocol_envelope() {
    let app = build_router(ServerConfig::new("secret-token"));

    let response = app
        .clone()
        .oneshot(protocol_request(
            "/v1/reservation/declare",
            "s1",
            "w1",
            serde_json::json!({
                "purpose": "Test requested work.",
                "files_planned": ["src/auth.ts"]
            }),
        ))
        .await
        .expect("reservation declaration should complete");
    assert_eq!(response.status(), StatusCode::OK);

    let events = app
        .clone()
        .oneshot(authorized_get("/v1/events"))
        .await
        .expect("events request should complete");
    assert_eq!(events.status(), StatusCode::OK);
    let body = to_bytes(events.into_body(), 2048)
        .await
        .expect("body should read");
    let json: serde_json::Value = serde_json::from_slice(&body).expect("body should be json");
    assert_eq!(json["events"][0]["event_type"], "ReservationDeclared");
    assert_eq!(json["events"][0]["agent_id"], "s1");
    assert_eq!(json["events"][0]["workspace_id"], "w1");
    assert_eq!(json["events"][0]["repo_id"], "repo-1");

    let claim = app
        .clone()
        .oneshot(json_request(
            "/v1/claim/acquire",
            serde_json::json!({
                "agent_id": "s1",
                "workspace_id": "w1",
                "path": "src/auth.ts"
            }),
        ))
        .await
        .expect("claim acquire should complete");
    assert_eq!(claim.status(), StatusCode::OK);

    let authorize = app
        .clone()
        .oneshot(native_hook_authorize_request(
            "s1",
            "w1",
            "src/auth.ts",
            "apply_patch",
        ))
        .await
        .expect("authorize should complete");
    assert_eq!(authorize.status(), StatusCode::OK);
    let body = to_bytes(authorize.into_body(), 1024)
        .await
        .expect("body should read");
    let json: serde_json::Value = serde_json::from_slice(&body).expect("body should be json");
    assert_eq!(json["decision"], "allow");
    assert_eq!(json["reason_code"], "authorized");
}

#[tokio::test]
async fn side_effecting_routes_reservation_declare_rejects_empty_purpose() {
    let app = build_router(ServerConfig::new("secret-token"));

    let response = app
        .oneshot(protocol_request(
            "/v1/reservation/declare",
            "s1",
            "w1",
            serde_json::json!({
                "purpose": "   ",
                "files_planned": ["src/auth.ts"]
            }),
        ))
        .await
        .expect("reservation declaration should complete");

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let body = to_bytes(response.into_body(), 1024)
        .await
        .expect("body should read");
    let json: serde_json::Value = serde_json::from_slice(&body).expect("body should be json");
    assert_eq!(json["status"], "error");
    assert_eq!(json["reason_code"], "missing_purpose");
}

#[tokio::test]
async fn side_effecting_routes_reservation_declare_rejects_empty_files_planned() {
    let app = build_router(ServerConfig::new("secret-token"));

    let response = app
        .oneshot(protocol_request(
            "/v1/reservation/declare",
            "s1",
            "w1",
            serde_json::json!({
                "purpose": "Fix auth validation behavior.",
                "files_planned": []
            }),
        ))
        .await
        .expect("reservation declaration should complete");

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let body = to_bytes(response.into_body(), 1024)
        .await
        .expect("body should read");
    let json: serde_json::Value = serde_json::from_slice(&body).expect("body should be json");
    assert_eq!(json["status"], "error");
    assert_eq!(json["reason_code"], "missing_scope");
}

#[tokio::test]
async fn side_effecting_routes_reservation_declare_rejects_normalized_empty_files_planned() {
    let app = build_router(ServerConfig::new("secret-token"));

    for path in ["./", "../", "/"] {
        let response = app
            .clone()
            .oneshot(protocol_request(
                "/v1/reservation/declare",
                "s1",
                "w1",
                serde_json::json!({
                    "purpose": "Fix auth validation behavior.",
                    "files_planned": [path]
                }),
            ))
            .await
            .expect("reservation declaration should complete");

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let body = to_bytes(response.into_body(), 1024)
            .await
            .expect("body should read");
        let json: serde_json::Value = serde_json::from_slice(&body).expect("body should be json");
        assert_eq!(json["status"], "error");
        assert_eq!(json["reason_code"], "missing_scope");
    }
}

#[tokio::test]
async fn side_effecting_routes_reservation_declare_does_not_store_empty_identity_sentinels() {
    let app = build_router(ServerConfig::new("secret-token"));

    let response = app
        .clone()
        .oneshot(json_request(
            "/v1/reservation/declare",
            serde_json::json!({
                "protocol_version": "stateful.v1",
                "request_id": "req-empty-identity",
                "observed_at": "2026-05-31T00:00:00Z",
                "agent": {
                    "agent_id": "s1",
                    "actor_id": "agent-1",
                    "actor_type": "agent"
                },
                "workspace": {
                    "root": "",
                    "workspace_id": "w1",
                    "repo_id": "",
                    "worktree_id": "",
                    "branch": ""
                },
                "source": {
                    "kind": "cli",
                    "event": "reservation_declare",
                    "source_ref": "routes-test"
                },
                "payload": {
                    "purpose": "Test requested work.",
                    "files_planned": ["src/auth.ts"]
                }
            }),
        ))
        .await
        .expect("reservation declaration should complete");
    assert_eq!(response.status(), StatusCode::OK);

    let events = app
        .oneshot(authorized_get("/v1/events"))
        .await
        .expect("events request should complete");
    assert_eq!(events.status(), StatusCode::OK);
    let body = to_bytes(events.into_body(), 2048)
        .await
        .expect("body should read");
    let json: serde_json::Value = serde_json::from_slice(&body).expect("body should be json");
    assert_eq!(json["events"][0]["repo_id"], serde_json::Value::Null);
    assert_eq!(json["events"][0]["worktree_id"], serde_json::Value::Null);
    assert_eq!(json["events"][0]["root"], serde_json::Value::Null);
    assert_eq!(json["events"][0]["branch"], serde_json::Value::Null);
}

#[tokio::test]
async fn authorize_rejects_legacy_body_after_protocol_enforcement() {
    let app = build_router(ServerConfig::new("secret-token"));

    let response = app
        .oneshot(json_request(
            "/v1/authorize",
            serde_json::json!({
                "agent_id": "s1",
                "workspace_id": "w1",
                "action": "write_file",
                "path": "src/auth.ts"
            }),
        ))
        .await
        .expect("authorize should complete");

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let body = to_bytes(response.into_body(), 1024)
        .await
        .expect("body should read");
    let json: serde_json::Value = serde_json::from_slice(&body).expect("body should be json");
    assert_eq!(json["decision"], "error");
    assert_eq!(json["reason_code"], "protocol_mismatch");
}

#[tokio::test]
async fn authorize_uses_policy_service_and_preserves_scope_denial() {
    let app = build_router(ServerConfig::new("secret-token"));

    let declare = app
        .clone()
        .oneshot(protocol_request(
            "/v1/reservation/declare",
            "s1",
            "w1",
            serde_json::json!({
                "purpose": "Test requested work.",
                "files_planned": ["src/auth.ts"]
            }),
        ))
        .await
        .expect("declare should complete");
    assert_eq!(declare.status(), StatusCode::OK);

    let response = app
        .oneshot(protocol_request(
            "/v1/authorize",
            "s1",
            "w1",
            serde_json::json!({
                "action": "write_file",
                "path": "src/other.ts"
            }),
        ))
        .await
        .expect("authorize should complete");

    assert_eq!(response.status(), StatusCode::OK);
    let body = to_bytes(response.into_body(), 65_536)
        .await
        .expect("body should read");
    let json: serde_json::Value = serde_json::from_slice(&body).expect("body should be json");
    assert_eq!(json["decision"], "deny");
    assert_eq!(json["reason_code"], "scope_mismatch");
}

#[tokio::test]
async fn session_register_and_heartbeat_update_current_summary() {
    let app = build_router(ServerConfig::new("secret-token"));

    let register = app
        .clone()
        .oneshot(json_request(
            "/v1/session/register",
            serde_json::json!({
                "agent_id": "s1",
                "workspace_id": "w1"
            }),
        ))
        .await
        .expect("session register should complete");
    assert_eq!(register.status(), StatusCode::OK);

    let heartbeat = app
        .clone()
        .oneshot(json_request(
            "/v1/session/heartbeat",
            serde_json::json!({
                "agent_id": "s1",
                "workspace_id": "w1"
            }),
        ))
        .await
        .expect("session heartbeat should complete");
    assert_eq!(heartbeat.status(), StatusCode::OK);

    let response = app
        .oneshot(authorized_get("/v1/current"))
        .await
        .expect("current request should complete");
    assert_eq!(response.status(), StatusCode::OK);

    let body = to_bytes(response.into_body(), 1024)
        .await
        .expect("body should read");
    let json: serde_json::Value = serde_json::from_slice(&body).expect("body should be json");
    assert_eq!(json["current"]["agent_count"], 1);
    assert_eq!(json["current"]["event_count"], 2);
}

#[tokio::test]
async fn authorize_records_implicit_session_heartbeat() {
    let app = build_router(ServerConfig::new("secret-token"));

    let response = app
        .clone()
        .oneshot(protocol_request(
            "/v1/authorize",
            "s1",
            "w1",
            serde_json::json!({
                "action": "write_file",
                "path": "src/auth.ts"
            }),
        ))
        .await
        .expect("authorize request should complete");
    assert_eq!(response.status(), StatusCode::OK);

    let events = app
        .oneshot(authorized_get("/v1/events"))
        .await
        .expect("events request should complete");
    assert_eq!(events.status(), StatusCode::OK);
    let json = response_json(events, 4096).await;
    let events = json["events"]
        .as_array()
        .expect("events should be an array");
    assert!(
        events.iter().any(|event| {
            event["event_type"] == "AgentHeartbeat"
                && event["agent_id"] == "s1"
                && event["workspace_id"] == "w1"
        }),
        "authorize should record implicit heartbeat: {events:?}"
    );
}

#[tokio::test]
async fn session_events_preserve_repo_identity_when_provided() {
    let temp = tempfile::tempdir().expect("temp dir should create");
    let temp_root = temp.path();
    let db_path = temp_root.join(".stateful_core").join("state.db");
    let store = Store::open(&db_path).expect("file store should open");
    let app = build_router(ServerConfig::with_store("secret-token", store));

    let response = app
        .clone()
        .oneshot(json_request(
            "/v1/session/register",
            serde_json::json!({
                "agent_id": "s1",
                "workspace_id": "w1",
                "repo_id": "repo-1",
                "worktree_id": "worktree-1",
                "root": "/repo",
                "branch": "main"
            }),
        ))
        .await
        .expect("session register should complete");
    assert_eq!(response.status(), StatusCode::OK);

    let events = app
        .oneshot(authorized_get("/v1/events"))
        .await
        .expect("events request should complete");
    assert_eq!(events.status(), StatusCode::OK);
    let body = to_bytes(events.into_body(), 4096)
        .await
        .expect("body should read");
    let json: serde_json::Value = serde_json::from_slice(&body).expect("body should be json");
    assert_eq!(json["events"][0]["repo_id"], "repo-1");
    assert_eq!(json["events"][0]["worktree_id"], "worktree-1");
    assert_eq!(json["events"][0]["root"], "/repo");
    assert_eq!(json["events"][0]["branch"], "main");
}

#[tokio::test]
async fn lease_finalize_route_is_available() {
    let temp = tempfile::tempdir().expect("temp dir should create");
    let temp_root = temp.path();
    let db_path = temp_root.join(".stateful_core").join("state.db");
    let store = Store::open(&db_path).expect("file store should open");
    let app = build_router(ServerConfig::with_store("secret-token", store));

    ensure_test_reservation_via_http(&app, "s1", "w1", "src/auth.ts").await;
    let claim = app
        .clone()
        .oneshot(json_request(
            "/v1/claim/acquire",
            serde_json::json!({
                "agent_id": "s1",
                "workspace_id": "w1",
                "path": "src/auth.ts"
            }),
        ))
        .await
        .expect("claim acquire should complete");
    assert_eq!(claim.status(), StatusCode::OK);

    let finalized = app
        .clone()
        .oneshot(json_request(
            "/v1/activity/finalize",
            serde_json::json!({
                "agent_id": "s1",
                "workspace_id": "w1"
            }),
        ))
        .await
        .expect("activity finalize should complete");
    assert_eq!(finalized.status(), StatusCode::OK);

    let release = app
        .oneshot(json_request(
            "/v1/claim/release",
            serde_json::json!({
                "agent_id": "s1",
                "workspace_id": "w1",
                "path": "src/auth.ts"
            }),
        ))
        .await
        .expect("claim release should complete");
    assert_eq!(release.status(), StatusCode::NOT_FOUND);
    let json = response_json(release, 2048).await;
    assert_eq!(json["status"], "error");
    assert_eq!(json["reason_code"], "claim_not_found");

    let reopened = Store::open(&db_path).expect("file store should reopen");
    assert_eq!(reopened.lease_count().expect("claim count should load"), 1);
    assert_eq!(
        reopened
            .activity_count()
            .expect("activity count should load"),
        1
    );
}

#[tokio::test]
async fn blocked_activity_phase_denies_authorized_write() {
    let temp = tempfile::tempdir().expect("temp dir should create");
    let temp_root = temp.path();
    let db_path = temp_root.join(".stateful_core").join("state.db");
    let store = Store::open(&db_path).expect("file store should open");
    let app = build_router(ServerConfig::with_store("secret-token", store));
    ensure_test_reservation_via_http(&app, "s1", "w1", "src/auth.ts").await;
    let claim = app
        .clone()
        .oneshot(json_request(
            "/v1/claim/acquire",
            serde_json::json!({
                "agent_id": "s1",
                "workspace_id": "w1",
                "path": "src/auth.ts"
            }),
        ))
        .await
        .expect("claim acquire should complete");
    assert_eq!(claim.status(), StatusCode::OK);
    let blocker = Store::open(&db_path).expect("file store should reopen");
    blocker
        .append_activity_with_phase("s1", "w1", stateful_core::ActivityPhase::Blocked)
        .expect("blocked activity should append");

    let blocked = app
        .oneshot(protocol_request(
            "/v1/authorize",
            "s1",
            "w1",
            serde_json::json!({
                "action": "write_file",
                "path": "src/auth.ts"
            }),
        ))
        .await
        .expect("authorize should complete");
    assert_eq!(blocked.status(), StatusCode::OK);
    let json = response_json(blocked, 2048).await;
    assert_eq!(json["decision"], "deny");
    assert_eq!(json["reason_code"], "inactive_session_phase");
}

#[tokio::test]
async fn declared_reservation_without_same_reservation_claim_denies_matching_authorize_request() {
    let app = build_router(ServerConfig::new("secret-token"));

    let declare = app
        .clone()
        .oneshot(protocol_request(
            "/v1/reservation/declare",
            "s1",
            "w1",
            serde_json::json!({
                "purpose": "Test requested work.",
                "files_planned": ["src/auth.ts"]
            }),
        ))
        .await
        .expect("reservation declaration should complete");
    assert_eq!(declare.status(), StatusCode::OK);

    let response = app
        .oneshot(protocol_request(
            "/v1/authorize",
            "s1",
            "w1",
            serde_json::json!({
                "action": "write_file",
                "path": "src/auth.ts"
            }),
        ))
        .await
        .expect("authorize should complete");
    assert_eq!(response.status(), StatusCode::OK);

    let body = to_bytes(response.into_body(), 1024)
        .await
        .expect("body should read");
    let json: serde_json::Value = serde_json::from_slice(&body).expect("body should be json");
    assert_eq!(json["decision"], "deny");
    assert_eq!(json["reason_code"], "missing_claim");
}

#[tokio::test]
async fn claim_from_different_reservation_does_not_authorize_write() {
    let app = build_router(ServerConfig::new("secret-token"));

    let declare_a = app
        .clone()
        .oneshot(protocol_request(
            "/v1/reservation/declare",
            "s1",
            "w1",
            serde_json::json!({
                "purpose": "Reservation A.",
                "files_planned": ["src/auth.ts"]
            }),
        ))
        .await
        .expect("reservation A should complete");
    assert_eq!(declare_a.status(), StatusCode::OK);
    let reservation_a = response_json(declare_a, 2048).await["reservation_id"]
        .as_str()
        .expect("reservation A id should be returned")
        .to_string();

    let declare_b = app
        .clone()
        .oneshot(protocol_request(
            "/v1/reservation/declare",
            "s1",
            "w1",
            serde_json::json!({
                "purpose": "Reservation B.",
                "files_planned": ["src/auth.ts"]
            }),
        ))
        .await
        .expect("reservation B should complete");
    assert_eq!(declare_b.status(), StatusCode::OK);
    let reservation_b = response_json(declare_b, 2048).await["reservation_id"]
        .as_str()
        .expect("reservation B id should be returned")
        .to_string();

    let claim = app
        .clone()
        .oneshot(json_request(
            "/v1/claim/acquire",
            serde_json::json!({
                "agent_id": "s1",
                "workspace_id": "w1",
                "reservation_id": reservation_a,
                "path": "src/auth.ts"
            }),
        ))
        .await
        .expect("claim should complete");
    assert_eq!(claim.status(), StatusCode::OK);

    let mut body = protocol_body(
        "s1",
        "w1",
        serde_json::json!({
            "action": "write_file",
            "path": "src/auth.ts",
            "reservation_id": reservation_b.clone()
        }),
    );
    body["source"]["kind"] = serde_json::json!("hook");
    body["source"]["event"] = serde_json::json!("pre_tool_use");
    body["source"]["tool_name"] = serde_json::json!("apply_patch");

    let authorize = app
        .clone()
        .oneshot(json_request("/v1/authorize", body))
        .await
        .expect("authorize should complete");
    assert_eq!(authorize.status(), StatusCode::OK);
    let json = response_json(authorize, 2048).await;
    assert_eq!(json["decision"], "deny");
    assert_eq!(json["reason_code"], "missing_claim");
    assert!(
        json["required_next_action"]
            .as_str()
            .unwrap_or_default()
            .contains("same-reservation")
    );

    let duplicate_reservation_claim = app
        .oneshot(json_request(
            "/v1/claim/acquire",
            serde_json::json!({
                "agent_id": "s1",
                "workspace_id": "w1",
                "reservation_id": reservation_b,
                "path": "src/auth.ts"
            }),
        ))
        .await
        .expect("duplicate reservation claim should complete");
    let duplicate_status = duplicate_reservation_claim.status();
    let duplicate_json = response_json(duplicate_reservation_claim, 2048).await;
    assert!(
        duplicate_status != StatusCode::OK || duplicate_json["claim_state"] != "already_held",
        "reservation B acquire must not be satisfied by reservation A claim: {duplicate_json}"
    );
}

#[tokio::test]
async fn same_reservation_claim_authorizes_write() {
    let app = build_router(ServerConfig::new("secret-token"));

    let declare = app
        .clone()
        .oneshot(protocol_request(
            "/v1/reservation/declare",
            "s1",
            "w1",
            serde_json::json!({
                "purpose": "Update auth file.",
                "files_planned": ["src/auth.ts"]
            }),
        ))
        .await
        .expect("reservation should complete");
    assert_eq!(declare.status(), StatusCode::OK);
    let reservation_id = response_json(declare, 2048).await["reservation_id"]
        .as_str()
        .expect("reservation id should be returned")
        .to_string();

    let claim = app
        .clone()
        .oneshot(json_request(
            "/v1/claim/acquire",
            serde_json::json!({
                "agent_id": "s1",
                "workspace_id": "w1",
                "reservation_id": reservation_id.clone(),
                "path": "src/auth.ts"
            }),
        ))
        .await
        .expect("claim should complete");
    assert_eq!(claim.status(), StatusCode::OK);

    let mut body = protocol_body(
        "s1",
        "w1",
        serde_json::json!({
            "action": "write_file",
            "path": "src/auth.ts",
            "reservation_id": reservation_id
        }),
    );
    body["source"]["kind"] = serde_json::json!("hook");
    body["source"]["event"] = serde_json::json!("pre_tool_use");
    body["source"]["tool_name"] = serde_json::json!("apply_patch");

    let authorize = app
        .oneshot(json_request("/v1/authorize", body))
        .await
        .expect("authorize should complete");
    assert_eq!(authorize.status(), StatusCode::OK);
    let json = response_json(authorize, 2048).await;
    assert_eq!(json["decision"], "allow");
    assert_eq!(json["reason_code"], "authorized");
}

#[tokio::test]
async fn missing_claim_cannot_be_bypassed_by_changing_agent_id() {
    let app = build_router(ServerConfig::new("secret-token"));

    for agent_id in ["s-original", "s-swapped"] {
        let declare = app
            .clone()
            .oneshot(protocol_request(
                "/v1/reservation/declare",
                agent_id,
                "w1",
                serde_json::json!({
                    "purpose": "Test requested work.",
                    "files_planned": ["src/auth.ts"]
                }),
            ))
            .await
            .expect("reservation declaration should complete");
        assert_eq!(declare.status(), StatusCode::OK);
    }

    let original = app
        .clone()
        .oneshot(native_hook_authorize_request(
            "s-original",
            "w1",
            "src/auth.ts",
            "apply_patch",
        ))
        .await
        .expect("authorize should complete");
    assert_eq!(original.status(), StatusCode::OK);
    let json = response_json(original, 2048).await;
    assert_eq!(json["decision"], "deny");
    assert_eq!(json["reason_code"], "missing_claim");

    let swapped = app
        .oneshot(native_hook_authorize_request(
            "s-swapped",
            "w1",
            "src/auth.ts",
            "apply_patch",
        ))
        .await
        .expect("authorize should complete");
    assert_eq!(swapped.status(), StatusCode::OK);
    let json = response_json(swapped, 2048).await;
    assert_eq!(json["decision"], "deny");
    assert_eq!(json["reason_code"], "missing_claim");
    let required_next_action = json["required_next_action"].as_str().unwrap_or_default();
    assert!(required_next_action.contains("same-reservation file claims"));
    assert!(required_next_action.contains("Do not change reservation_id"));
}

#[tokio::test]
async fn declared_reservation_denies_out_of_scope_authorize_request() {
    let app = build_router(ServerConfig::new("secret-token"));

    let declare = app
        .clone()
        .oneshot(protocol_request(
            "/v1/reservation/declare",
            "s1",
            "w1",
            serde_json::json!({
                "purpose": "Test requested work.",
                "files_planned": ["src/auth.ts"]
            }),
        ))
        .await
        .expect("reservation declaration should complete");
    assert_eq!(declare.status(), StatusCode::OK);

    let response = app
        .oneshot(protocol_request(
            "/v1/authorize",
            "s1",
            "w1",
            serde_json::json!({
                "action": "write_file",
                "path": "src/session.ts"
            }),
        ))
        .await
        .expect("authorize should complete");
    assert_eq!(response.status(), StatusCode::OK);

    let body = to_bytes(response.into_body(), 1024)
        .await
        .expect("body should read");
    let json: serde_json::Value = serde_json::from_slice(&body).expect("body should be json");
    assert_eq!(json["decision"], "deny");
    assert_eq!(json["reason_code"], "scope_mismatch");
}

#[tokio::test]
async fn hook_native_write_requires_exact_file_intent_even_with_directory_scope() {
    let app = build_router(ServerConfig::new("secret-token"));

    let declare = app
        .clone()
        .oneshot(protocol_request(
            "/v1/reservation/declare",
            "s1",
            "w1",
            serde_json::json!({
                "purpose": "Test requested work.",
                "files_planned": ["src/"]
            }),
        ))
        .await
        .expect("reservation declaration should complete");
    assert_eq!(declare.status(), StatusCode::OK);
    let mut body = protocol_body(
        "s1",
        "w1",
        serde_json::json!({
            "action": "write_file",
            "path": "src/auth.ts"
        }),
    );

    body["source"]["kind"] = serde_json::json!("hook");
    body["source"]["event"] = serde_json::json!("pre_tool_use");
    body["source"]["source_ref"] = serde_json::json!("hook:req-1");
    body["source"]["tool_name"] = serde_json::json!("apply_patch");

    let response = app
        .oneshot(json_request("/v1/authorize", body))
        .await
        .expect("authorize should complete");
    assert_eq!(response.status(), StatusCode::OK);

    let body = to_bytes(response.into_body(), 2048)
        .await
        .expect("body should read");
    let json: serde_json::Value = serde_json::from_slice(&body).expect("body should be json");
    assert_eq!(json["decision"], "deny");
    assert_eq!(json["reason_code"], "scope_mismatch");
    assert!(
        json["required_next_action"]
            .as_str()
            .unwrap_or_default()
            .contains("exact file scope")
    );
}

#[tokio::test]
async fn hook_file_write_without_tool_name_still_requires_exact_file_lease() {
    let app = build_router(ServerConfig::new("secret-token"));

    let declare = app
        .clone()
        .oneshot(protocol_request(
            "/v1/reservation/declare",
            "s1",
            "w1",
            serde_json::json!({
                "purpose": "Test requested work.",
                "files_planned": ["src/"]
            }),
        ))
        .await
        .expect("reservation declaration should complete");
    assert_eq!(declare.status(), StatusCode::OK);

    ensure_test_reservation_via_http(&app, "s1", "w1", "src/").await;
    let claim = app
        .clone()
        .oneshot(json_request(
            "/v1/claim/acquire",
            serde_json::json!({
                "agent_id": "s1",
                "workspace_id": "w1",
                "path": "src/"
            }),
        ))
        .await
        .expect("directory claim acquire should complete");
    assert_eq!(claim.status(), StatusCode::OK);

    let declare = app
        .clone()
        .oneshot(protocol_request(
            "/v1/reservation/declare",
            "s1",
            "w1",
            serde_json::json!({
                "purpose": "Test requested work.",
                "files_planned": ["src/auth.ts"]
            }),
        ))
        .await
        .expect("reservation declaration should complete");
    assert_eq!(declare.status(), StatusCode::OK);

    let mut body = protocol_body(
        "s1",
        "w1",
        serde_json::json!({
            "action": "write_file",
            "path": "src/auth.ts"
        }),
    );
    body["source"]["kind"] = serde_json::json!("hook");
    body["source"]["event"] = serde_json::json!("pre_tool_use");
    body["source"]["source_ref"] = serde_json::json!("hook:req-1");

    let response = app
        .oneshot(json_request("/v1/authorize", body))
        .await
        .expect("authorize should complete");
    assert_eq!(response.status(), StatusCode::OK);

    let body = to_bytes(response.into_body(), 2048)
        .await
        .expect("body should read");
    let json: serde_json::Value = serde_json::from_slice(&body).expect("body should be json");
    assert_eq!(json["decision"], "deny");
    assert_eq!(json["reason_code"], "missing_claim");
    assert!(
        json["message"]
            .as_str()
            .unwrap_or_default()
            .contains("exact active same-reservation file claims")
    );
    assert!(
        json["required_next_action"]
            .as_str()
            .unwrap_or_default()
            .contains("matching same-reservation file claims")
    );
}

#[tokio::test]
async fn hook_native_write_requires_exact_file_lease_even_when_directory_lease_covers_path() {
    let app = build_router(ServerConfig::new("secret-token"));

    let declare = app
        .clone()
        .oneshot(protocol_request(
            "/v1/reservation/declare",
            "s1",
            "w1",
            serde_json::json!({
                "purpose": "Test requested work.",
                "files_planned": ["src/auth.ts"]
            }),
        ))
        .await
        .expect("reservation declaration should complete");
    assert_eq!(declare.status(), StatusCode::OK);

    ensure_test_reservation_via_http(&app, "s1", "w1", "src/").await;
    let claim = app
        .clone()
        .oneshot(json_request(
            "/v1/claim/acquire",
            serde_json::json!({
                "agent_id": "s1",
                "workspace_id": "w1",
                "path": "src/"
            }),
        ))
        .await
        .expect("directory claim acquire should complete");

    assert_eq!(claim.status(), StatusCode::OK);

    let declare = app
        .clone()
        .oneshot(protocol_request(
            "/v1/reservation/declare",
            "s1",
            "w1",
            serde_json::json!({
                "purpose": "Test requested work.",
                "files_planned": ["src/auth.ts"]
            }),
        ))
        .await
        .expect("reservation declaration should complete");
    assert_eq!(declare.status(), StatusCode::OK);

    let mut body = protocol_body(
        "s1",
        "w1",
        serde_json::json!({
            "action": "write_file",
            "path": "src/auth.ts"
        }),
    );
    body["source"]["kind"] = serde_json::json!("hook");
    body["source"]["event"] = serde_json::json!("pre_tool_use");
    body["source"]["source_ref"] = serde_json::json!("hook:req-1");
    body["source"]["tool_name"] = serde_json::json!("Edit");

    let response = app
        .oneshot(json_request("/v1/authorize", body))
        .await
        .expect("authorize should complete");
    assert_eq!(response.status(), StatusCode::OK);

    let body = to_bytes(response.into_body(), 2048)
        .await
        .expect("body should read");
    let json: serde_json::Value = serde_json::from_slice(&body).expect("body should be json");
    assert_eq!(json["decision"], "deny");
    assert_eq!(json["reason_code"], "missing_claim");
    assert!(
        json["required_next_action"]
            .as_str()
            .unwrap_or_default()
            .contains("same-reservation file claims")
    );
}

#[tokio::test]
async fn hook_native_write_allows_exact_file_intent_and_exact_file_lease() {
    let app = build_router(ServerConfig::new("secret-token"));

    let declare = app
        .clone()
        .oneshot(protocol_request(
            "/v1/reservation/declare",
            "s1",
            "w1",
            serde_json::json!({
                "purpose": "Test requested work.",
                "files_planned": ["src/auth.ts"]
            }),
        ))
        .await
        .expect("reservation declaration should complete");
    assert_eq!(declare.status(), StatusCode::OK);

    let claim = app
        .clone()
        .oneshot(json_request(
            "/v1/claim/acquire",
            serde_json::json!({
                "agent_id": "s1",
                "workspace_id": "w1",
                "path": "src/auth.ts"
            }),
        ))
        .await
        .expect("file claim acquire should complete");
    assert_eq!(claim.status(), StatusCode::OK);

    let mut body = protocol_body(
        "s1",
        "w1",
        serde_json::json!({
            "action": "write_file",
            "path": "src/auth.ts"
        }),
    );
    body["source"]["kind"] = serde_json::json!("hook");
    body["source"]["event"] = serde_json::json!("pre_tool_use");
    body["source"]["source_ref"] = serde_json::json!("hook:req-1");
    body["source"]["tool_name"] = serde_json::json!("file_change");

    let response = app
        .oneshot(json_request("/v1/authorize", body))
        .await
        .expect("authorize should complete");
    assert_eq!(response.status(), StatusCode::OK);

    let body = to_bytes(response.into_body(), 2048)
        .await
        .expect("body should read");
    let json: serde_json::Value = serde_json::from_slice(&body).expect("body should be json");
    assert_eq!(json["decision"], "allow");
    assert_eq!(json["reason_code"], "authorized");
}

#[tokio::test]
async fn hook_native_write_denies_when_file_changed_since_claim_acquired() {
    let temp = tempfile::tempdir().expect("temp dir should create");
    let temp_root = temp.path();
    let repo_root = temp_root.join("repo");
    std::fs::create_dir_all(repo_root.join("src")).expect("repo src should be creatable");
    std::fs::write(repo_root.join("src/auth.ts"), "version one\n")
        .expect("initial target should be writable");
    let db_path = temp_root.join(".stateful_core").join("state.db");
    let store = Store::open(&db_path).expect("file store should open");
    let app = build_router(ServerConfig::with_store("secret-token", store));

    let mut declare_body = protocol_body(
        "s1",
        "w1",
        serde_json::json!({
            "purpose": "Edit auth.",
            "files_planned": ["src/auth.ts"]
        }),
    );
    declare_body["workspace"]["root"] = serde_json::json!(repo_root.to_string_lossy().to_string());
    let declare = app
        .clone()
        .oneshot(json_request("/v1/reservation/declare", declare_body))
        .await
        .expect("reservation declaration should complete");
    assert_eq!(declare.status(), StatusCode::OK);

    let acquire = app
        .clone()
        .oneshot(json_request(
            "/v1/claim/acquire",
            serde_json::json!({
                "agent_id": "s1",
                "workspace_id": "w1",
                "path": "src/auth.ts",
                "root": repo_root.to_string_lossy()
            }),
        ))
        .await
        .expect("claim acquire should complete");
    assert_eq!(acquire.status(), StatusCode::OK);

    std::fs::write(repo_root.join("src/auth.ts"), "version two\n")
        .expect("target should be externally writable");

    let mut authorize_body = protocol_body(
        "s1",
        "w1",
        serde_json::json!({
            "action": "write_file",
            "path": "src/auth.ts"
        }),
    );
    authorize_body["workspace"]["root"] =
        serde_json::json!(repo_root.to_string_lossy().to_string());
    authorize_body["source"]["kind"] = serde_json::json!("hook");
    authorize_body["source"]["event"] = serde_json::json!("pre_tool_use");
    authorize_body["source"]["source_ref"] = serde_json::json!("hook:s1:Edit");
    authorize_body["source"]["tool_name"] = serde_json::json!("Edit");

    let response = app
        .oneshot(json_request("/v1/authorize", authorize_body))
        .await
        .expect("authorize should complete");
    assert_eq!(response.status(), StatusCode::OK);
    let json = response_json(response, 2048).await;
    assert_eq!(json["decision"], "deny");
    assert_eq!(json["reason_code"], "stale_claim_observation");

    assert_eq!(
        json["required_next_action"],
        "Reread target, reacquire claim, retry same edit."
    );
}

#[tokio::test]
async fn post_write_refresh_updates_claim_observation_for_next_write() {
    let temp = tempfile::tempdir().expect("temp dir should create");
    let temp_root = temp.path();
    let repo_root = temp_root.join("repo");
    std::fs::create_dir_all(repo_root.join("src")).expect("repo src should be creatable");
    std::fs::write(repo_root.join("src/auth.ts"), "version one\n")
        .expect("initial target should be writable");
    let db_path = temp_root.join(".stateful_core").join("state.db");
    let store = Store::open(&db_path).expect("file store should open");
    let app = build_router(ServerConfig::with_store("secret-token", store));

    let mut declare_body = protocol_body(
        "s1",
        "w1",
        serde_json::json!({
            "purpose": "Edit auth.",
            "files_planned": ["src/auth.ts"]
        }),
    );
    declare_body["workspace"]["root"] = serde_json::json!(repo_root.to_string_lossy().to_string());
    let declare = app
        .clone()
        .oneshot(json_request("/v1/reservation/declare", declare_body))
        .await
        .expect("reservation declaration should complete");
    assert_eq!(declare.status(), StatusCode::OK);

    let acquire = app
        .clone()
        .oneshot(json_request(
            "/v1/claim/acquire",
            serde_json::json!({
                "agent_id": "s1",
                "workspace_id": "w1",
                "path": "src/auth.ts",
                "root": repo_root.to_string_lossy()
            }),
        ))
        .await
        .expect("claim acquire should complete");
    assert_eq!(acquire.status(), StatusCode::OK);

    std::fs::write(repo_root.join("src/auth.ts"), "version two\n")
        .expect("completed write should update target");

    let refresh = app
        .clone()
        .oneshot(json_request(
            "/v1/claim/refresh-observation",
            serde_json::json!({
                "agent_id": "s1",
                "workspace_id": "w1",
                "path": "src/auth.ts",
                "root": repo_root.to_string_lossy()
            }),
        ))
        .await
        .expect("claim observation refresh should complete");
    assert_eq!(refresh.status(), StatusCode::OK);

    let mut authorize_body = protocol_body(
        "s1",
        "w1",
        serde_json::json!({
            "action": "write_file",
            "path": "src/auth.ts"
        }),
    );
    authorize_body["workspace"]["root"] =
        serde_json::json!(repo_root.to_string_lossy().to_string());
    authorize_body["source"]["kind"] = serde_json::json!("hook");
    authorize_body["source"]["event"] = serde_json::json!("pre_tool_use");
    authorize_body["source"]["source_ref"] = serde_json::json!("hook:s1:Edit");
    authorize_body["source"]["tool_name"] = serde_json::json!("Edit");

    let response = app
        .oneshot(json_request("/v1/authorize", authorize_body))
        .await
        .expect("authorize should complete");
    assert_eq!(response.status(), StatusCode::OK);
    let json = response_json(response, 2048).await;
    assert_eq!(json["decision"], "allow");
}

#[tokio::test]
async fn cli_sandbox_write_file_requires_exact_file_intent_despite_directory_lease() {
    let app = build_router(ServerConfig::new("secret-token"));

    let declare = app
        .clone()
        .oneshot(protocol_request(
            "/v1/reservation/declare",
            "s1",
            "w1",
            serde_json::json!({
                "purpose": "Test requested work.",
                "files_planned": ["src/"]
            }),
        ))
        .await
        .expect("reservation declaration should complete");
    assert_eq!(declare.status(), StatusCode::OK);

    ensure_test_reservation_via_http(&app, "s1", "w1", "src/").await;
    let claim = app
        .clone()
        .oneshot(json_request(
            "/v1/claim/acquire",
            serde_json::json!({
                "agent_id": "s1",
                "workspace_id": "w1",
                "path": "src/"
            }),
        ))
        .await
        .expect("directory claim acquire should complete");
    assert_eq!(claim.status(), StatusCode::OK);

    let mut body = protocol_body(
        "s1",
        "w1",
        serde_json::json!({
            "action": "write_file",
            "path": "src/auth.ts"
        }),
    );
    body["source"]["kind"] = serde_json::json!("cli");
    body["source"]["event"] = serde_json::json!("sandbox_run");
    body["source"]["source_ref"] = serde_json::json!("stateful.sandbox.run");

    let response = app
        .oneshot(json_request("/v1/authorize", body))
        .await
        .expect("authorize should complete");
    assert_eq!(response.status(), StatusCode::OK);

    let body = to_bytes(response.into_body(), 2048)
        .await
        .expect("body should read");
    let json: serde_json::Value = serde_json::from_slice(&body).expect("body should be json");
    assert_eq!(json["decision"], "deny");
    assert_eq!(json["reason_code"], "scope_mismatch");
    assert!(
        json["required_next_action"]
            .as_str()
            .unwrap_or_default()
            .contains("exact file")
    );
}

#[tokio::test]
async fn write_file_requires_same_reservation_file_claim_even_when_directory_claim_has_same_path() {
    let app = build_router(ServerConfig::new("secret-token"));

    let declare_directory = app
        .clone()
        .oneshot(protocol_request(
            "/v1/reservation/declare",
            "s1",
            "w1",
            serde_json::json!({
                "purpose": "Test requested directory work.",
                "files_planned": ["target/"]
            }),
        ))
        .await
        .expect("directory reservation declaration should complete");
    assert_eq!(declare_directory.status(), StatusCode::OK);

    let directory_lease = app
        .clone()
        .oneshot(json_request(
            "/v1/claim/acquire",
            serde_json::json!({
                "agent_id": "s1",
                "workspace_id": "w1",
                "path": "target/"
            }),
        ))
        .await
        .expect("directory claim acquire should complete");
    assert_eq!(directory_lease.status(), StatusCode::OK);

    let declare_file = app
        .clone()
        .oneshot(protocol_request(
            "/v1/reservation/declare",
            "s1",
            "w1",
            serde_json::json!({
                "purpose": "Test requested file work.",
                "files_planned": ["target"]
            }),
        ))
        .await
        .expect("file reservation declaration should complete");
    assert_eq!(declare_file.status(), StatusCode::OK);

    let denied = app
        .clone()
        .oneshot(protocol_request(
            "/v1/authorize",
            "s1",
            "w1",
            serde_json::json!({
                "action": "write_file",
                "path": "target"
            }),
        ))
        .await
        .expect("authorize should complete");
    assert_eq!(denied.status(), StatusCode::OK);
    let json = response_json(denied, 2048).await;
    assert_eq!(json["decision"], "deny");
    assert_eq!(json["reason_code"], "missing_claim");

    let file_lease = app
        .clone()
        .oneshot(json_request(
            "/v1/claim/acquire",
            serde_json::json!({
                "agent_id": "s1",
                "workspace_id": "w1",
                "path": "target"
            }),
        ))
        .await
        .expect("file claim acquire should complete");
    assert_eq!(file_lease.status(), StatusCode::OK);

    let allowed = app
        .oneshot(protocol_request(
            "/v1/authorize",
            "s1",
            "w1",
            serde_json::json!({
                "action": "write_file",
                "path": "target"
            }),
        ))
        .await
        .expect("authorize should complete");
    assert_eq!(allowed.status(), StatusCode::OK);
    let json = response_json(allowed, 2048).await;
    assert_eq!(json["decision"], "allow");
}

#[tokio::test]
async fn authorize_requires_intent_in_same_workspace_as_lease() {
    let app = build_router(ServerConfig::new("secret-token"));

    let declare_other_workspace = app
        .clone()
        .oneshot(protocol_request(
            "/v1/reservation/declare",
            "s1",
            "w2",
            serde_json::json!({
                "purpose": "Test requested work.",
                "files_planned": ["src/auth.ts"]
            }),
        ))
        .await
        .expect("reservation declaration should complete");
    assert_eq!(declare_other_workspace.status(), StatusCode::OK);

    let claim = app
        .clone()
        .oneshot(json_request(
            "/v1/claim/acquire",
            serde_json::json!({
                "agent_id": "s1",
                "workspace_id": "w1",
                "path": "src/auth.ts"
            }),
        ))
        .await
        .expect("claim acquire should complete");
    assert_eq!(claim.status(), StatusCode::BAD_REQUEST);
    let claim_body = response_json(claim, 2048).await;
    assert_eq!(claim_body["reason_code"], "missing_reservation");
    assert_eq!(
        claim_body["message"],
        "Claim acquisition requires an active reservation covering the requested path."
    );

    let response = app
        .clone()
        .oneshot(protocol_request(
            "/v1/authorize",
            "s1",
            "w1",
            serde_json::json!({
                "action": "write_file",
                "path": "src/auth.ts"
            }),
        ))
        .await
        .expect("authorize should complete");
    assert_eq!(response.status(), StatusCode::OK);

    let body = to_bytes(response.into_body(), 2048)
        .await
        .expect("body should read");
    let json: serde_json::Value = serde_json::from_slice(&body).expect("body should be json");
    assert_eq!(json["decision"], "deny");
    assert_eq!(json["reason_code"], "missing_reservation");

    let declare_same_workspace = app
        .clone()
        .oneshot(protocol_request(
            "/v1/reservation/declare",
            "s1",
            "w1",
            serde_json::json!({
                "purpose": "Test requested work.",
                "files_planned": ["src/auth.ts"]
            }),
        ))
        .await
        .expect("reservation declaration should complete");
    assert_eq!(declare_same_workspace.status(), StatusCode::OK);

    let claim = app
        .clone()
        .oneshot(json_request(
            "/v1/claim/acquire",
            serde_json::json!({
                "agent_id": "s1",
                "workspace_id": "w1",
                "path": "src/auth.ts"
            }),
        ))
        .await
        .expect("claim acquire should complete");
    assert_eq!(claim.status(), StatusCode::OK);

    let response = app
        .oneshot(protocol_request(
            "/v1/authorize",
            "s1",
            "w1",
            serde_json::json!({
                "action": "write_file",
                "path": "src/auth.ts"
            }),
        ))
        .await
        .expect("authorize should complete");
    assert_eq!(response.status(), StatusCode::OK);

    let body = to_bytes(response.into_body(), 2048)
        .await
        .expect("body should read");
    let json: serde_json::Value = serde_json::from_slice(&body).expect("body should be json");
    assert_eq!(json["decision"], "allow");
    assert_eq!(json["reason_code"], "authorized");
}

#[tokio::test]
async fn active_claim_by_other_session_denies_authorize_even_with_matching_reservation() {
    let app = build_router(ServerConfig::new("secret-token"));

    ensure_test_reservation_via_http(&app, "s2", "w1", "src/auth.ts").await;
    let claim = app
        .clone()
        .oneshot(json_request(
            "/v1/claim/acquire",
            serde_json::json!({
                "agent_id": "s2",
                "workspace_id": "w1",
                "path": "src/auth.ts"
            }),
        ))
        .await
        .expect("claim acquire should complete");
    assert_eq!(claim.status(), StatusCode::OK);

    let declare = app
        .clone()
        .oneshot(protocol_request(
            "/v1/reservation/declare",
            "s1",
            "w1",
            serde_json::json!({
                "purpose": "Test requested work.",
                "files_planned": ["src/auth.ts"]
            }),
        ))
        .await
        .expect("reservation declaration should complete");
    assert_eq!(declare.status(), StatusCode::OK);

    let response = app
        .oneshot(protocol_request(
            "/v1/authorize",
            "s1",
            "w1",
            serde_json::json!({
                "action": "write_file",
                "path": "src/auth.ts"
            }),
        ))
        .await
        .expect("authorize should complete");
    assert_eq!(response.status(), StatusCode::OK);

    let body = to_bytes(response.into_body(), 1024)
        .await
        .expect("body should read");
    let json: serde_json::Value = serde_json::from_slice(&body).expect("body should be json");
    assert_eq!(json["decision"], "deny");
    assert_eq!(json["reason_code"], "active_claim_conflict");
    let required_next_action = json["required_next_action"].as_str().unwrap_or_default();
    assert!(required_next_action.contains("Do not redeclare reservation"));
    assert!(required_next_action.contains("agent_id"));
}

#[tokio::test]
async fn authorize_denies_when_target_changed_since_base_observation() {
    let app = build_router(ServerConfig::new("secret-token"));
    let temp = tempfile::tempdir().expect("temp dir should create");
    let temp_root = temp.path();
    let src_dir = temp_root.join("src");
    std::fs::create_dir_all(&src_dir).expect("src directory should be created");
    let target = src_dir.join("auth.ts");
    std::fs::write(&target, "original contents\n").expect("target file should be created");

    let mut declare_body = protocol_body(
        "s1",
        "w1",
        serde_json::json!({
            "purpose": "Test requested work.",
            "files_planned": ["src/auth.ts"]
        }),
    );
    declare_body["workspace"]["root"] = serde_json::json!(temp_root.to_string_lossy());
    let declare = app
        .clone()
        .oneshot(json_request("/v1/reservation/declare", declare_body))
        .await
        .expect("reservation declaration should complete");
    assert_eq!(declare.status(), StatusCode::OK);

    let claim = app
        .clone()
        .oneshot(json_request(
            "/v1/claim/acquire",
            serde_json::json!({
                "agent_id": "s1",
                "workspace_id": "w1",
                "path": "src/auth.ts"
            }),
        ))
        .await
        .expect("claim acquire should complete");
    assert_eq!(claim.status(), StatusCode::OK);

    std::fs::write(&target, "updated contents\n").expect("target file should be changed");

    let mut authorize_body = protocol_body(
        "s1",
        "w1",
        serde_json::json!({
            "action": "write_file",
            "path": "src/auth.ts",
            "base_observations": [{
                "path": "src/auth.ts",
                "exists": true,
                "content_hash": test_content_hash(b"original contents\n")
            }]
        }),
    );
    authorize_body["workspace"]["root"] = serde_json::json!(temp_root.to_string_lossy());
    let response = app
        .oneshot(json_request("/v1/authorize", authorize_body))
        .await
        .expect("authorize should complete");
    assert_eq!(response.status(), StatusCode::OK);

    let json = response_json(response, 2048).await;
    assert_eq!(json["decision"], "deny");
    assert_eq!(json["reason_code"], "stale_target_observation");
    assert_eq!(
        json["required_next_action"],
        "Reread target, retry same edit with fresh base observation."
    );
}

#[tokio::test]
async fn queue_on_conflict_without_intent_enqueues_wait_record() {
    let store = Store::open_in_memory().expect("store should open");
    let app = build_router(ServerConfig::with_store("secret-token", store));

    ensure_test_reservation_via_http(&app, "s1", "w1", "src/auth.ts").await;
    let claim = app
        .clone()
        .oneshot(json_request(
            "/v1/claim/acquire",
            serde_json::json!({
                "agent_id": "s1",
                "workspace_id": "w1",
                "path": "src/auth.ts"
            }),
        ))
        .await
        .expect("claim acquire should complete");
    assert_eq!(claim.status(), StatusCode::OK);

    let response = app
        .clone()
        .oneshot(protocol_request(
            "/v1/authorize",
            "s2",
            "w1",
            serde_json::json!({
                "action": "write_file",
                "path": "src/auth.ts",
                "queue_on_conflict": true,
                "purpose": "Queue requested write after blocker clears."
            }),
        ))
        .await
        .expect("authorize should complete");
    assert_eq!(response.status(), StatusCode::OK);

    let body = to_bytes(response.into_body(), 2048)
        .await
        .expect("body should read");
    let json: serde_json::Value = serde_json::from_slice(&body).expect("body should be json");
    assert_eq!(json["decision"], "deny");
    assert_eq!(json["reason_code"], "active_claim_conflict");
    assert_eq!(json["wait"]["status"], "queued");
    let wait_id = json["wait"]["wait_id"]
        .as_str()
        .expect("wait_id should be present")
        .to_string();

    let release = app
        .clone()
        .oneshot(json_request(
            "/v1/claim/release",
            serde_json::json!({
                "agent_id": "s1",
                "workspace_id": "w1",
                "path": "src/auth.ts"
            }),
        ))
        .await
        .expect("claim release should complete");
    assert_eq!(release.status(), StatusCode::OK);

    let resume = app
        .oneshot(json_request(
            "/v1/resume/next",
            serde_json::json!({
                "agent_id": "s2",
                "workspace_id": "w1"
            }),
        ))
        .await
        .expect("resume next should complete");
    assert_eq!(resume.status(), StatusCode::OK);

    let body = to_bytes(resume.into_body(), 2048)
        .await
        .expect("body should read");
    let json: serde_json::Value = serde_json::from_slice(&body).expect("body should be json");
    assert_eq!(json["resume_available"], true);
    assert_eq!(json["reservation"]["wait_id"], wait_id);
}

#[tokio::test]
async fn queue_on_conflict_rejects_missing_purpose() {
    let store = Store::open_in_memory().expect("store should open");
    let app = build_router(ServerConfig::with_store("secret-token", store));

    let response = app
        .oneshot(protocol_request(
            "/v1/authorize",
            "s2",
            "w1",
            serde_json::json!({
                "action": "write_file",
                "path": "src/auth.ts",
                "queue_on_conflict": true
            }),
        ))
        .await
        .expect("authorize should complete");

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let body = to_bytes(response.into_body(), 1024)
        .await
        .expect("body should read");
    let json: serde_json::Value = serde_json::from_slice(&body).expect("body should be json");
    assert_eq!(json["status"], "error");
    assert_eq!(json["reason_code"], "missing_purpose");
}

#[tokio::test]
async fn queue_on_conflict_out_of_scope_enqueues_wait_record() {
    let store = Store::open_in_memory().expect("store should open");
    let app = build_router(ServerConfig::with_store("secret-token", store));

    ensure_test_reservation_via_http(&app, "s1", "w1", "src/session.ts").await;
    let claim = app
        .clone()
        .oneshot(json_request(
            "/v1/claim/acquire",
            serde_json::json!({
                "agent_id": "s1",
                "workspace_id": "w1",
                "path": "src/session.ts"
            }),
        ))
        .await
        .expect("claim acquire should complete");
    assert_eq!(claim.status(), StatusCode::OK);

    let declare = app
        .clone()
        .oneshot(protocol_request(
            "/v1/reservation/declare",
            "s2",
            "w1",
            serde_json::json!({
                "purpose": "Test requested work.",
                "files_planned": ["src/auth.ts"]
            }),
        ))
        .await
        .expect("reservation declaration should complete");
    assert_eq!(declare.status(), StatusCode::OK);

    let response = app
        .clone()
        .oneshot(protocol_request(
            "/v1/authorize",
            "s2",
            "w1",
            serde_json::json!({
                "action": "write_file",
                "path": "src/session.ts",
                "queue_on_conflict": true,
                "purpose": "Queue requested write after blocker clears."
            }),
        ))
        .await
        .expect("authorize should complete");
    assert_eq!(response.status(), StatusCode::OK);

    let body = to_bytes(response.into_body(), 2048)
        .await
        .expect("body should read");
    let json: serde_json::Value = serde_json::from_slice(&body).expect("body should be json");
    assert_eq!(json["decision"], "deny");
    assert_eq!(json["reason_code"], "active_claim_conflict");
    assert_eq!(json["wait"]["status"], "queued");
    let wait_id = json["wait"]["wait_id"]
        .as_str()
        .expect("wait_id should be present")
        .to_string();

    let release = app
        .clone()
        .oneshot(json_request(
            "/v1/claim/release",
            serde_json::json!({
                "agent_id": "s1",
                "workspace_id": "w1",
                "path": "src/session.ts"
            }),
        ))
        .await
        .expect("claim release should complete");
    assert_eq!(release.status(), StatusCode::OK);

    let resume = app
        .oneshot(json_request(
            "/v1/resume/next",
            serde_json::json!({
                "agent_id": "s2",
                "workspace_id": "w1"
            }),
        ))
        .await
        .expect("resume next should complete");
    assert_eq!(resume.status(), StatusCode::OK);

    let body = to_bytes(resume.into_body(), 2048)
        .await
        .expect("body should read");
    let json: serde_json::Value = serde_json::from_slice(&body).expect("body should be json");
    assert_eq!(json["resume_available"], true);
    assert_eq!(json["reservation"]["wait_id"], wait_id);
}

#[tokio::test]
async fn authorize_lazily_claims_reserved_intent_and_lease() {
    let store = Store::open_in_memory().expect("store should open");
    let app = build_router(ServerConfig::with_store("secret-token", store));

    let request = app
        .clone()
        .oneshot(protocol_request(
            "/v1/reservation/request",
            "s1",
            "w1",
            serde_json::json!({
                "request_id": "request-auth",
                "action": "write_file",
                "path": "src/auth.ts",
                "purpose": "Reserve auth changes before claiming."
            }),
        ))
        .await
        .expect("reservation request should complete");
    assert_eq!(request.status(), StatusCode::OK);
    let json = response_json(request, 2048).await;
    assert_eq!(json["request_state"], "reserved");
    let wait_id = json["reservation"]["wait_id"]
        .as_str()
        .expect("reserved request should include wait_id")
        .to_string();

    let authorize = app
        .clone()
        .oneshot(native_hook_authorize_request(
            "s1",
            "w1",
            "src/auth.ts",
            "apply_patch",
        ))
        .await
        .expect("authorize should complete");
    assert_eq!(authorize.status(), StatusCode::OK);

    let json = response_json(authorize, 2048).await;
    assert_eq!(json["decision"], "allow", "{json}");
    assert_eq!(json["reason_code"], "authorized");
    assert_eq!(json["reservation"]["wait_id"], wait_id);
    assert_eq!(json["reservation"]["status"], "claimed");
    assert_eq!(
        json["reservation"]["purpose"],
        "Reserve auth changes before claiming."
    );

    ensure_test_reservation_via_http(&app, "s2", "w1", "src/auth.ts").await;
    let blocked = app
        .oneshot(protocol_request(
            "/v1/authorize",
            "s2",
            "w1",
            serde_json::json!({
                "action": "write_file",
                "path": "src/auth.ts"
            }),
        ))
        .await
        .expect("authorize should complete");
    assert_eq!(blocked.status(), StatusCode::OK);
    let json = response_json(blocked, 2048).await;
    assert_eq!(json["decision"], "deny");
    assert_eq!(json["reason_code"], "active_claim_conflict");
}

#[tokio::test]
async fn authorize_lazily_claims_supplied_reserved_reservation_id() {
    let store = Store::open_in_memory().expect("store should open");
    let app = build_router(ServerConfig::with_store("secret-token", store));

    let request = app
        .clone()
        .oneshot(protocol_request(
            "/v1/reservation/request",
            "s1",
            "w1",
            serde_json::json!({
                "request_id": "request-auth-with-id",
                "action": "write_file",
                "path": "src/auth.ts",
                "purpose": "Reserve auth changes before claiming."
            }),
        ))
        .await
        .expect("reservation request should complete");
    assert_eq!(request.status(), StatusCode::OK);
    let json = response_json(request, 2048).await;
    assert_eq!(json["request_state"], "reserved");
    let wait_id = json["reservation"]["wait_id"]
        .as_str()
        .expect("reserved request should include wait_id")
        .to_string();

    let mut body = protocol_body(
        "s1",
        "w1",
        serde_json::json!({
            "action": "write_file",
            "path": "src/auth.ts",
            "reservation_id": wait_id.clone()
        }),
    );
    body["source"]["kind"] = serde_json::json!("hook");

    let authorize = app
        .oneshot(json_request("/v1/authorize", body))
        .await
        .expect("authorize should complete");
    assert_eq!(authorize.status(), StatusCode::OK);

    let json = response_json(authorize, 2048).await;
    assert_eq!(json["decision"], "allow", "{json}");
    assert_eq!(json["reason_code"], "authorized");
    assert_eq!(json["reservation"]["wait_id"], wait_id);
    assert_eq!(json["reservation"]["status"], "claimed");
}

#[tokio::test]
async fn queued_conflict_reserves_first_waiter_after_lease_release() {
    let store = Store::open_in_memory().expect("store should open");
    let app = build_router(ServerConfig::with_store("secret-token", store));

    ensure_test_reservation_via_http(&app, "s1", "w1", "src/auth.ts").await;
    let claim = app
        .clone()
        .oneshot(json_request(
            "/v1/claim/acquire",
            serde_json::json!({
                "agent_id": "s1",
                "workspace_id": "w1",
                "path": "src/auth.ts"
            }),
        ))
        .await
        .expect("claim acquire should complete");
    assert_eq!(claim.status(), StatusCode::OK);

    for agent_id in ["s2", "s3"] {
        let declare = app
            .clone()
            .oneshot(protocol_request(
                "/v1/reservation/declare",
                agent_id,
                "w1",
                serde_json::json!({
                    "purpose": "Test requested work.",
                    "files_planned": ["src/auth.ts"]
                }),
            ))
            .await
            .expect("reservation declaration should complete");
        assert_eq!(declare.status(), StatusCode::OK);
    }

    let queued_b = app
        .clone()
        .oneshot(protocol_request(
            "/v1/authorize",
            "s2",
            "w1",
            serde_json::json!({
                "action": "write_file",
                "path": "src/auth.ts",
                "queue_on_conflict": true,
                "purpose": "Queue requested write after blocker clears."
            }),
        ))
        .await
        .expect("authorize should complete");
    assert_eq!(queued_b.status(), StatusCode::OK);
    let body = to_bytes(queued_b.into_body(), 2048)
        .await
        .expect("body should read");
    let json: serde_json::Value = serde_json::from_slice(&body).expect("body should be json");
    assert_eq!(json["decision"], "deny");
    assert_eq!(json["reason_code"], "active_claim_conflict");
    assert_eq!(json["wait"]["status"], "queued");
    assert_eq!(json["wait"]["queue_position"], 1);
    assert_eq!(json["wait"]["blocking_agent_id"], "s1");
    let s2_wait_id = json["wait"]["wait_id"]
        .as_str()
        .expect("wait id should be present")
        .to_string();

    let queued_c = app
        .clone()
        .oneshot(protocol_request(
            "/v1/authorize",
            "s3",
            "w1",
            serde_json::json!({
                "action": "write_file",
                "path": "src/auth.ts",
                "queue_on_conflict": true,
                "purpose": "Queue requested write after blocker clears."
            }),
        ))
        .await
        .expect("authorize should complete");
    let body = to_bytes(queued_c.into_body(), 2048)
        .await
        .expect("body should read");
    let json: serde_json::Value = serde_json::from_slice(&body).expect("body should be json");
    assert_eq!(json["wait"]["queue_position"], 2);

    let release = app
        .clone()
        .oneshot(json_request(
            "/v1/claim/release",
            serde_json::json!({
                "agent_id": "s1",
                "workspace_id": "w1",
                "path": "src/auth.ts"
            }),
        ))
        .await
        .expect("claim release should complete");
    assert_eq!(release.status(), StatusCode::OK);

    let intentless_probe = app
        .clone()
        .oneshot(protocol_request(
            "/v1/authorize",
            "s4",
            "w1",
            serde_json::json!({
                "action": "write_file",
                "path": "src/auth.ts"
            }),
        ))
        .await
        .expect("authorize should complete");
    let body = to_bytes(intentless_probe.into_body(), 2048)
        .await
        .expect("body should read");
    let json: serde_json::Value = serde_json::from_slice(&body).expect("body should be json");
    assert_eq!(json["decision"], "deny");
    assert_eq!(json["reason_code"], "missing_reservation");
    assert!(json["reservation"].is_null());

    let blocked_c = app
        .clone()
        .oneshot(protocol_request(
            "/v1/authorize",
            "s3",
            "w1",
            serde_json::json!({
                "action": "write_file",
                "path": "src/auth.ts",
                "queue_on_conflict": true,
                "purpose": "Queue requested write after blocker clears."
            }),
        ))
        .await
        .expect("authorize should complete");
    let body = to_bytes(blocked_c.into_body(), 2048)
        .await
        .expect("body should read");
    let json: serde_json::Value = serde_json::from_slice(&body).expect("body should be json");
    assert_eq!(json["decision"], "deny");
    assert_eq!(json["reason_code"], "reservation_conflict");
    assert_eq!(json["reservation"]["agent_id"], "s2");
    let required_next_action = json["required_next_action"].as_str().unwrap_or_default();
    assert!(required_next_action.contains("Do not redeclare reservation"));
    assert!(required_next_action.contains("agent_id"));

    let unclaimed_b = app
        .clone()
        .oneshot(protocol_request(
            "/v1/authorize",
            "s2",
            "w1",
            serde_json::json!({
                "action": "write_file",
                "path": "src/auth.ts"
            }),
        ))
        .await
        .expect("authorize should complete");
    let body = to_bytes(unclaimed_b.into_body(), 2048)
        .await
        .expect("body should read");
    let json: serde_json::Value = serde_json::from_slice(&body).expect("body should be json");
    assert_eq!(json["decision"], "deny");
    assert_eq!(json["reason_code"], "reservation_claim_required");
    assert_eq!(json["reservation"]["status"], "reserved");
    let required_next_action = json["required_next_action"].as_str().unwrap_or_default();
    assert!(required_next_action.contains("state.reservation.claim"));

    let claim = app
        .clone()
        .oneshot(protocol_request(
            "/v1/reservation/claim",
            "s2",
            "w1",
            serde_json::json!({
                "wait_id": s2_wait_id
            }),
        ))
        .await
        .expect("reservation claim should complete");
    assert_eq!(claim.status(), StatusCode::OK);
    let body = to_bytes(claim.into_body(), 2048)
        .await
        .expect("body should read");
    let json: serde_json::Value = serde_json::from_slice(&body).expect("body should be json");
    assert_eq!(json["status"], "ok");
    assert_eq!(json["reservation"]["status"], "claimed");

    let allowed_b = app
        .clone()
        .oneshot(protocol_request(
            "/v1/authorize",
            "s2",
            "w1",
            serde_json::json!({
                "action": "write_file",
                "path": "src/auth.ts"
            }),
        ))
        .await
        .expect("authorize should complete");
    let body = to_bytes(allowed_b.into_body(), 2048)
        .await
        .expect("body should read");
    let json: serde_json::Value = serde_json::from_slice(&body).expect("body should be json");
    assert_eq!(json["decision"], "allow");
    assert_eq!(json["reason_code"], "authorized");
    assert!(json.get("reservation").is_none());
}

#[tokio::test]
async fn concurrent_codex_agents_transfer_native_edit_access_through_request_claim_and_lease() {
    let store = Store::open_in_memory().expect("store should open");
    let app = build_router(ServerConfig::with_store("secret-token", store));

    for agent_id in ["codex-a", "codex-b", "codex-c"] {
        let register = app
            .clone()
            .oneshot(json_request(
                "/v1/session/register",
                serde_json::json!({
                    "agent_id": agent_id,
                    "workspace_id": "w1"
                }),
            ))
            .await
            .expect("session register should complete");
        assert_eq!(register.status(), StatusCode::OK);
    }

    let declare_a = app
        .clone()
        .oneshot(protocol_request(
            "/v1/reservation/declare",
            "codex-a",
            "w1",
            serde_json::json!({
                "purpose": "Test requested work.",
                "files_planned": ["src/auth.ts"]
            }),
        ))
        .await
        .expect("reservation declaration should complete");
    assert_eq!(declare_a.status(), StatusCode::OK);

    let lease_a = app
        .clone()
        .oneshot(json_request(
            "/v1/claim/acquire",
            serde_json::json!({
                "agent_id": "codex-a",
                "workspace_id": "w1",
                "path": "src/auth.ts"
            }),
        ))
        .await
        .expect("claim acquire should complete");
    assert_eq!(lease_a.status(), StatusCode::OK);

    let allowed_a = app
        .clone()
        .oneshot(native_hook_authorize_request(
            "codex-a",
            "w1",
            "src/auth.ts",
            "apply_patch",
        ))
        .await
        .expect("authorize should complete");
    assert_eq!(allowed_a.status(), StatusCode::OK);
    let json = response_json(allowed_a, 2048).await;
    assert_eq!(json["decision"], "allow");
    assert_eq!(json["reason_code"], "authorized");

    ensure_test_reservation_via_http(&app, "codex-b", "w1", "src/auth.ts").await;
    ensure_test_reservation_via_http(&app, "codex-c", "w1", "src/auth.ts").await;

    let request_b = app
        .clone()
        .oneshot(protocol_request(
            "/v1/reservation/request",
            "codex-b",
            "w1",
            serde_json::json!({
                "request_id": "request-codex-b",
                "action": "write_file",
                "path": "src/auth.ts",
                "purpose": "Queue codex-b after codex-a finishes auth changes."
            }),
        ))
        .await
        .expect("reservation request should complete");
    assert_eq!(request_b.status(), StatusCode::OK);
    let json = response_json(request_b, 2048).await;
    assert_eq!(json["request_state"], "queued");
    assert_eq!(json["wait"]["queue_position"], 1);
    assert_eq!(json["wait"]["blocking_agent_id"], "codex-a");
    let codex_b_wait_id = json["wait"]["wait_id"]
        .as_str()
        .expect("wait id should be present")
        .to_string();

    let request_c = app
        .clone()
        .oneshot(protocol_request(
            "/v1/reservation/request",
            "codex-c",
            "w1",
            serde_json::json!({
                "request_id": "request-codex-c",
                "action": "write_file",
                "path": "src/auth.ts",
                "purpose": "Queue codex-c after codex-a finishes auth changes."
            }),
        ))
        .await
        .expect("reservation request should complete");
    assert_eq!(request_c.status(), StatusCode::OK);
    let json = response_json(request_c, 2048).await;
    assert_eq!(json["request_state"], "queued");
    assert_eq!(json["wait"]["queue_position"], 2);
    assert_eq!(json["wait"]["blocking_agent_id"], "codex-a");
    let codex_c_wait_id = json["wait"]["wait_id"]
        .as_str()
        .expect("wait id should be present")
        .to_string();

    let finalize_a = app
        .clone()
        .oneshot(json_request(
            "/v1/activity/finalize",
            serde_json::json!({
                "agent_id": "codex-a",
                "workspace_id": "w1"
            }),
        ))
        .await
        .expect("activity finalize should complete");
    assert_eq!(finalize_a.status(), StatusCode::OK);
    let json = response_json(finalize_a, 2048).await;
    assert_eq!(json["released_claims"], 1);

    let resume_b = app
        .clone()
        .oneshot(json_request(
            "/v1/resume/next",
            serde_json::json!({
                "agent_id": "codex-b",
                "workspace_id": "w1"
            }),
        ))
        .await
        .expect("resume next should complete");
    assert_eq!(resume_b.status(), StatusCode::OK);
    let json = response_json(resume_b, 2048).await;
    assert_eq!(json["resume_available"], true);
    assert_eq!(json["reservation"]["wait_id"], codex_b_wait_id);
    assert_eq!(json["reservation"]["status"], "reserved");

    let blocked_c = app
        .clone()
        .oneshot(native_hook_authorize_request(
            "codex-c",
            "w1",
            "src/auth.ts",
            "Edit",
        ))
        .await
        .expect("authorize should complete");
    assert_eq!(blocked_c.status(), StatusCode::OK);
    let json = response_json(blocked_c, 2048).await;
    assert_eq!(json["decision"], "deny");
    assert_eq!(json["reason_code"], "reservation_conflict");
    assert_eq!(json["reservation"]["agent_id"], "codex-b");

    let lazy_claim_b = app
        .clone()
        .oneshot(native_hook_authorize_request(
            "codex-b",
            "w1",
            "src/auth.ts",
            "apply_patch",
        ))
        .await
        .expect("authorize should complete");
    assert_eq!(lazy_claim_b.status(), StatusCode::OK);
    let json = response_json(lazy_claim_b, 2048).await;
    assert_eq!(json["decision"], "allow");
    assert_eq!(json["reason_code"], "authorized");
    assert_eq!(json["reservation"]["wait_id"], codex_b_wait_id);
    assert_eq!(json["reservation"]["status"], "claimed");

    let allowed_b = app
        .clone()
        .oneshot(native_hook_authorize_request(
            "codex-b",
            "w1",
            "src/auth.ts",
            "Edit",
        ))
        .await
        .expect("authorize should complete");
    assert_eq!(allowed_b.status(), StatusCode::OK);
    let json = response_json(allowed_b, 2048).await;
    assert_eq!(json["decision"], "allow");
    assert_eq!(json["reason_code"], "authorized");

    let blocked_a = app
        .clone()
        .oneshot(native_hook_authorize_request(
            "codex-a",
            "w1",
            "src/auth.ts",
            "apply_patch",
        ))
        .await
        .expect("authorize should complete");
    assert_eq!(blocked_a.status(), StatusCode::OK);
    let json = response_json(blocked_a, 2048).await;
    assert_eq!(json["decision"], "deny");
    assert_eq!(json["reason_code"], "missing_reservation");

    let finalize_b = app
        .clone()
        .oneshot(json_request(
            "/v1/activity/finalize",
            serde_json::json!({
                "agent_id": "codex-b",
                "workspace_id": "w1"
            }),
        ))
        .await
        .expect("activity finalize should complete");
    assert_eq!(finalize_b.status(), StatusCode::OK);
    let json = response_json(finalize_b, 2048).await;
    assert_eq!(json["released_claims"], 1);

    let resume_c = app
        .clone()
        .oneshot(json_request(
            "/v1/resume/next",
            serde_json::json!({
                "agent_id": "codex-c",
                "workspace_id": "w1"
            }),
        ))
        .await
        .expect("resume next should complete");
    assert_eq!(resume_c.status(), StatusCode::OK);
    let json = response_json(resume_c, 2048).await;
    assert_eq!(json["resume_available"], true);
    assert_eq!(json["reservation"]["wait_id"], codex_c_wait_id);
    assert_eq!(json["reservation"]["status"], "reserved");

    let claim_c = app
        .clone()
        .oneshot(protocol_request(
            "/v1/reservation/claim",
            "codex-c",
            "w1",
            serde_json::json!({
                "wait_id": codex_c_wait_id
            }),
        ))
        .await
        .expect("reservation claim should complete");
    assert_eq!(claim_c.status(), StatusCode::OK);
    let json = response_json(claim_c, 2048).await;
    assert_eq!(json["reservation"]["status"], "claimed");

    let allowed_c = app
        .oneshot(native_hook_authorize_request(
            "codex-c",
            "w1",
            "src/auth.ts",
            "file_change",
        ))
        .await
        .expect("authorize should complete");
    assert_eq!(allowed_c.status(), StatusCode::OK);
    let json = response_json(allowed_c, 2048).await;
    assert_eq!(json["decision"], "allow");
    assert_eq!(json["reason_code"], "authorized");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_reservation_requests_reserve_exactly_one_file_backed_waiter() {
    let temp = tempfile::tempdir().expect("temp dir should create");
    let temp_root = temp.path();
    let db_path = temp_root.join(".stateful_core").join("state.db");
    let store = Store::open(&db_path).expect("file-backed store should open");
    let app = build_router(ServerConfig::with_store("secret-token", store));
    let agent_count = 12;

    for index in 0..agent_count {
        let declare = app
            .clone()
            .oneshot(protocol_request(
                "/v1/reservation/declare",
                &format!("stress-{index}"),
                "w1",
                serde_json::json!({
                    "purpose": format!("Prepare stress request {index}."),
                    "files_planned": ["src/concurrent.ts"]
                }),
            ))
            .await
            .expect("reservation declaration should complete");
        assert_eq!(declare.status(), StatusCode::OK);
    }

    let barrier = Arc::new(tokio::sync::Barrier::new(agent_count));
    let handles = (0..agent_count)
        .map(|index| {
            let app = app.clone();
            let barrier = Arc::clone(&barrier);
            tokio::spawn(async move {
                barrier.wait().await;
                let response = app
                    .oneshot(protocol_request(
                        "/v1/reservation/request",
                        &format!("stress-{index}"),
                        "w1",
                        serde_json::json!({
                            "request_id": format!("request-stress-{index}"),
                            "action": "write_file",
                            "path": "src/concurrent.ts",
                            "purpose": format!("Stress request {index}.")
                        }),
                    ))
                    .await
                    .expect("reservation request should complete");
                assert_eq!(response.status(), StatusCode::OK);
                response_json(response, 16 * 1024 * 1024).await
            })
        })
        .collect::<Vec<_>>();

    let mut states = Vec::new();
    for handle in handles {
        let json = handle.await.expect("request task should not panic");
        states.push(
            json["request_state"]
                .as_str()
                .expect("request state should be present")
                .to_string(),
        );
    }

    assert_eq!(
        states
            .iter()
            .filter(|state| state.as_str() == "reserved")
            .count(),
        1
    );
    assert_eq!(
        states
            .iter()
            .filter(|state| state.as_str() == "queued")
            .count(),
        agent_count - 1
    );

    let reopened = Store::open(&db_path).expect("file-backed store should reopen");
    let mut persisted_reserved = 0;
    let mut persisted_queued = 0;
    for index in 0..agent_count {
        let waiter = reopened
            .waiter_by_request_id(format!("request-stress-{index}"))
            .expect("waiter lookup should succeed")
            .expect("waiter should persist");
        match waiter.status.as_str() {
            "reserved" => persisted_reserved += 1,
            "queued" => persisted_queued += 1,
            status => panic!("unexpected waiter status {status}"),
        }
    }
    assert_eq!(persisted_reserved, 1);
    assert_eq!(persisted_queued, agent_count - 1);
}

#[tokio::test]
async fn reservation_request_reserves_available_target_but_still_requires_claim() {
    let store = Store::open_in_memory().expect("store should open");
    let app = build_router(ServerConfig::with_store("secret-token", store));

    ensure_test_reservation_via_http(&app, "s1", "w1", "src/auth.ts").await;

    let requested = app
        .clone()
        .oneshot(protocol_request(
            "/v1/reservation/request",
            "s1",
            "w1",
            serde_json::json!({
                "request_id": "request-1",
                "action": "write_file",
                "path": "src/auth.ts",
                "purpose": "Reserve auth file before writing."
            }),
        ))
        .await
        .expect("reservation request should complete");
    assert_eq!(requested.status(), StatusCode::OK);
    let body = to_bytes(requested.into_body(), 2048)
        .await
        .expect("body should read");
    let json: serde_json::Value = serde_json::from_slice(&body).expect("body should be json");
    assert_eq!(json["status"], "ok");
    assert_eq!(json["request_state"], "reserved");
    assert_eq!(json["request_id"], "request-1");
    assert_eq!(json["reservation"]["agent_id"], "s1");
    assert_eq!(json["reservation"]["relative_path"], "src/auth.ts");
    assert_eq!(json["reservation"]["status"], "reserved");
    assert_eq!(
        json["reservation"]["purpose"],
        "Reserve auth file before writing."
    );
    let wait_id = json["reservation"]["wait_id"]
        .as_str()
        .expect("wait id should be present")
        .to_string();

    let unclaimed = app
        .oneshot(protocol_request(
            "/v1/authorize",
            "s1",
            "w1",
            serde_json::json!({
                "action": "write_file",
                "path": "src/auth.ts"
            }),
        ))
        .await
        .expect("authorize should complete");
    assert_eq!(unclaimed.status(), StatusCode::OK);
    let body = to_bytes(unclaimed.into_body(), 2048)
        .await
        .expect("body should read");
    let json: serde_json::Value = serde_json::from_slice(&body).expect("body should be json");
    assert_eq!(json["decision"], "deny");
    assert_eq!(json["reason_code"], "reservation_claim_required");
    assert_eq!(json["reservation"]["wait_id"], wait_id);
}

#[tokio::test]
async fn claim_acquire_with_active_reservation_returns_reservation_claim_guidance() {
    let store = Store::open_in_memory().expect("store should open");
    let app = build_router(ServerConfig::with_store("secret-token", store));

    ensure_test_reservation_via_http(&app, "s1", "w1", "src/auth.ts").await;

    let requested = app
        .clone()
        .oneshot(protocol_request(
            "/v1/reservation/request",
            "s1",
            "w1",
            serde_json::json!({
                "request_id": "request-claim-claim",
                "action": "write_file",
                "path": "src/auth.ts",
                "purpose": "Reserve auth file before acquiring the claim."
            }),
        ))
        .await
        .expect("reservation request should complete");
    assert_eq!(requested.status(), StatusCode::OK);
    let json = response_json(requested, 2048).await;
    assert_eq!(json["request_state"], "reserved");
    let wait_id = json["reservation"]["wait_id"]
        .as_str()
        .expect("wait id should be present")
        .to_string();

    let acquire = app
        .oneshot(json_request(
            "/v1/claim/acquire",
            serde_json::json!({
                "agent_id": "s1",
                "workspace_id": "w1",
                "path": "src/auth.ts"
            }),
        ))
        .await
        .expect("claim acquire should complete");
    assert_eq!(acquire.status(), StatusCode::CONFLICT);
    let json = response_json(acquire, 2048).await;
    assert_eq!(json["reason_code"], "reservation_claim_required");
    assert_eq!(json["reservation"]["wait_id"], wait_id);
    assert_eq!(json["reservation"]["status"], "reserved");
    assert_eq!(json["reservation"]["agent_id"], "s1");
    assert!(
        json["required_next_action"]
            .as_str()
            .unwrap_or_default()
            .contains("state.reservation.claim"),
        "unexpected response: {json}"
    );
}

#[tokio::test]
async fn reservation_claim_preserves_existing_session_intent_scope() {
    let store = Store::open_in_memory().expect("store should open");
    let app = build_router(ServerConfig::with_store("secret-token", store));

    let declare_existing = app
        .clone()
        .oneshot(protocol_request(
            "/v1/reservation/declare",
            "s2",
            "w1",
            serde_json::json!({
                "purpose": "Continue parser refactor.",
                "files_planned": ["src/parser.ts"]
            }),
        ))
        .await
        .expect("existing reservation declaration should complete");
    assert_eq!(declare_existing.status(), StatusCode::OK);

    let existing_lease = app
        .clone()
        .oneshot(json_request(
            "/v1/claim/acquire",
            serde_json::json!({
                "agent_id": "s2",
                "workspace_id": "w1",
                "path": "src/parser.ts"
            }),
        ))
        .await
        .expect("existing claim acquisition should complete");
    assert_eq!(existing_lease.status(), StatusCode::OK);

    let request = app
        .clone()
        .oneshot(protocol_request(
            "/v1/reservation/request",
            "s2",
            "w1",
            serde_json::json!({
                "request_id": "request-auth-claim",
                "action": "write_file",
                "path": "src/auth.ts",
                "purpose": "Claim auth update while parser refactor remains active."
            }),
        ))
        .await
        .expect("reservation request should complete");
    assert_eq!(request.status(), StatusCode::OK);
    let json = response_json(request, 4096).await;
    assert_eq!(json["request_state"], "reserved");
    let wait_id = json["reservation"]["wait_id"]
        .as_str()
        .expect("reservation should include wait id")
        .to_string();

    let claim = app
        .clone()
        .oneshot(protocol_request(
            "/v1/reservation/claim",
            "s2",
            "w1",
            serde_json::json!({
                "wait_id": wait_id
            }),
        ))
        .await
        .expect("reservation claim should complete");
    assert_eq!(claim.status(), StatusCode::OK);

    let existing_scope = app
        .oneshot(protocol_request(
            "/v1/authorize",
            "s2",
            "w1",
            serde_json::json!({
                "action": "write_file",
                "path": "src/parser.ts"
            }),
        ))
        .await
        .expect("authorize should complete");
    let json = response_json(existing_scope, 2048).await;
    assert_eq!(
        json["decision"], "allow",
        "unexpected authorization response: {json}"
    );
}

#[tokio::test]
async fn lease_acquire_returns_conflict_for_active_claim_conflict() {
    let store = Store::open_in_memory().expect("store should open");
    let app = build_router(ServerConfig::with_store("secret-token", store));

    ensure_test_reservation_via_http(&app, "s1", "w1", "src/auth.ts").await;
    let first = app
        .clone()
        .oneshot(json_request(
            "/v1/claim/acquire",
            serde_json::json!({
                "agent_id": "s1",
                "workspace_id": "w1",
                "path": "src/auth.ts"
            }),
        ))
        .await
        .expect("first claim acquire should complete");
    assert_eq!(first.status(), StatusCode::OK);

    ensure_test_reservation_via_http(&app, "s2", "w1", "src/auth.ts").await;
    let conflict = app
        .oneshot(json_request(
            "/v1/claim/acquire",
            serde_json::json!({
                "agent_id": "s2",
                "workspace_id": "w1",
                "path": "src/auth.ts"
            }),
        ))
        .await
        .expect("conflicting claim acquire should complete");

    assert_eq!(conflict.status(), StatusCode::CONFLICT);
    let json = response_json(conflict, 2048).await;
    assert_eq!(json["status"], "error");
    assert_eq!(json["reason_code"], "claim_conflict");
    let required_next_action = json["required_next_action"]
        .as_str()
        .expect("claim conflict should include recovery guidance");
    assert!(required_next_action.contains("state.reservation.request"));
    assert!(required_next_action.contains("state.notifications.poll"));
    assert!(required_next_action.contains("state.resume.next"));
    assert!(required_next_action.contains("state.reservation.claim"));
}

#[tokio::test]
async fn lease_acquire_allows_existing_directory_observation() {
    let temp = tempfile::tempdir().expect("temp dir should create");
    let temp_root = temp.path();
    let repo_root = temp_root.join("repo");
    std::fs::create_dir_all(repo_root.join("tmp/build")).expect("repo tmp should be creatable");
    let db_path = temp_root.join(".stateful_core").join("state.db");
    let store = Store::open(&db_path).expect("file store should open");
    let app = build_router(ServerConfig::with_store("secret-token", store));

    let mut declare_body = protocol_body(
        "s1",
        "w1",
        serde_json::json!({
            "purpose": "Write build artifacts.",
            "files_planned": ["tmp/build/"]
        }),
    );
    declare_body["workspace"]["root"] = serde_json::json!(repo_root.to_string_lossy().to_string());
    let declare = app
        .clone()
        .oneshot(json_request("/v1/reservation/declare", declare_body))
        .await
        .expect("reservation declaration should complete");
    assert_eq!(declare.status(), StatusCode::OK);

    let acquire = app
        .clone()
        .oneshot(json_request(
            "/v1/claim/acquire",
            serde_json::json!({
                "agent_id": "s1",
                "workspace_id": "w1",
                "path": "tmp/build/",
                "root": repo_root.to_string_lossy()
            }),
        ))
        .await
        .expect("directory claim acquire should complete");
    assert_eq!(acquire.status(), StatusCode::OK);
}

#[tokio::test]
async fn lease_acquire_rejects_direct_tmp_directory_resource() {
    let store = Store::open_in_memory().expect("store should open");
    let app = build_router(ServerConfig::with_store("secret-token", store));

    ensure_test_reservation_via_http(&app, "s1", "w1", "tmp/").await;
    let acquire = app
        .oneshot(json_request(
            "/v1/claim/acquire",
            serde_json::json!({
                "agent_id": "s1",
                "workspace_id": "w1",
                "path": "tmp/"
            }),
        ))
        .await
        .expect("direct tmp claim acquire should complete");

    assert_eq!(acquire.status(), StatusCode::BAD_REQUEST);
    let json = response_json(acquire, 2048).await;
    assert_eq!(json["reason_code"], "invalid_claim_path");
    assert!(
        json["message"]
            .as_str()
            .unwrap_or_default()
            .contains("direct tmp claims are not allowed"),
        "unexpected response: {json}"
    );
}

#[tokio::test]
async fn lease_acquire_rejects_directory_observation_for_file_path() {
    let temp = tempfile::tempdir().expect("temp dir should create");
    let temp_root = temp.path();
    let repo_root = temp_root.join("repo");
    std::fs::create_dir_all(repo_root.join("tmp")).expect("repo tmp should be creatable");
    let db_path = temp_root.join(".stateful_core").join("state.db");
    let store = Store::open(&db_path).expect("file store should open");
    let app = build_router(ServerConfig::with_store("secret-token", store));

    let mut declare_body = protocol_body(
        "s1",
        "w1",
        serde_json::json!({
            "purpose": "Edit file path.",
            "files_planned": ["tmp"]
        }),
    );
    declare_body["workspace"]["root"] = serde_json::json!(repo_root.to_string_lossy().to_string());
    let declare = app
        .clone()
        .oneshot(json_request("/v1/reservation/declare", declare_body))
        .await
        .expect("reservation declaration should complete");
    assert_eq!(declare.status(), StatusCode::OK);

    let acquire = app
        .clone()
        .oneshot(json_request(
            "/v1/claim/acquire",
            serde_json::json!({
                "agent_id": "s1",
                "workspace_id": "w1",
                "path": "tmp",
                "root": repo_root.to_string_lossy()
            }),
        ))
        .await
        .expect("file claim acquire should complete");
    assert_eq!(acquire.status(), StatusCode::INTERNAL_SERVER_ERROR);
    let json = response_json(acquire, 2048).await;
    assert!(
        json["message"]
            .as_str()
            .expect("error message should be present")
            .contains("Is a directory"),
        "unexpected response: {json}"
    );
}

#[tokio::test]
async fn lease_acquire_rejects_active_reservation_conflict_without_breaking_claim() {
    let store = Store::open_in_memory().expect("store should open");
    let app = build_router(ServerConfig::with_store("secret-token", store));

    let requested = app
        .clone()
        .oneshot(protocol_request(
            "/v1/reservation/request",
            "s1",
            "w1",
            serde_json::json!({
                "request_id": "request-reserved",
                "action": "write_file",
                "path": "src/auth.ts",
                "purpose": "Reserve auth file before writing."
            }),
        ))
        .await
        .expect("reservation request should complete");
    assert_eq!(requested.status(), StatusCode::OK);
    let json = response_json(requested, 2048).await;
    assert_eq!(json["request_state"], "reserved");
    let wait_id = json["reservation"]["wait_id"]
        .as_str()
        .expect("wait id should be present")
        .to_string();

    ensure_test_reservation_via_http(&app, "s2", "w1", "src/auth.ts").await;
    let conflict = app
        .clone()
        .oneshot(json_request(
            "/v1/claim/acquire",
            serde_json::json!({
                "agent_id": "s2",
                "workspace_id": "w1",
                "path": "src/auth.ts"
            }),
        ))
        .await
        .expect("claim acquire should complete");
    assert_eq!(conflict.status(), StatusCode::CONFLICT);
    let json = response_json(conflict, 2048).await;
    assert_eq!(json["reason_code"], "claim_conflict");

    let claim = app
        .oneshot(protocol_request(
            "/v1/reservation/claim",
            "s1",
            "w1",
            serde_json::json!({
                "wait_id": wait_id
            }),
        ))
        .await
        .expect("reservation claim should complete");
    assert_eq!(claim.status(), StatusCode::OK);
    let json = response_json(claim, 2048).await;
    assert_eq!(json["reservation"]["status"], "claimed");
}

#[tokio::test]
async fn reservation_claim_rolls_back_reservation_when_intent_event_append_fails() {
    let temp = tempfile::tempdir().expect("temp dir should create");
    let temp_root = temp.path();
    let db_path = temp_root.join(".stateful_core").join("state.db");
    let store = Store::open(&db_path).expect("file store should open");
    let wait = store
        .enqueue_waiter(
            "s1",
            "w1",
            "src/auth.ts",
            "write_file",
            "Claim reserved auth changes.",
            Some("s0"),
        )
        .expect("waiter should enqueue");
    store
        .promote_next_waiter("w1", "src/auth.ts")
        .expect("waiter should reserve");
    let app = build_router(ServerConfig::with_store("secret-token", store));

    let trigger_conn =
        rusqlite::Connection::open(&db_path).expect("trigger connection should open");
    trigger_conn
        .execute_batch(
            "CREATE TRIGGER fail_claim_intent_event
             BEFORE INSERT ON events
             WHEN NEW.event_type = 'ReservationDeclared'
             BEGIN
                 SELECT RAISE(ABORT, 'simulated reservation event append failure');
             END;",
        )
        .expect("failure trigger should install");

    let claim = app
        .oneshot(protocol_request(
            "/v1/reservation/claim",
            "s1",
            "w1",
            serde_json::json!({
                "wait_id": wait.wait_id
            }),
        ))
        .await
        .expect("reservation claim should complete");
    assert_eq!(claim.status(), StatusCode::CONFLICT);
    let json = response_json(claim, 2048).await;
    assert_eq!(json["reason_code"], "claim_failed");
    assert!(
        json["message"]
            .as_str()
            .expect("message should be string")
            .contains("simulated reservation event append failure")
    );

    let status: String = trigger_conn
        .query_row(
            "SELECT status FROM wait_queue WHERE wait_id = ?1",
            [&wait.wait_id],
            |row| row.get::<_, String>(0),
        )
        .expect("waiter status should load");
    assert_eq!(status, "reserved");

    drop(trigger_conn);
}

#[tokio::test]
async fn reservation_request_rejects_empty_purpose() {
    let store = Store::open_in_memory().expect("store should open");
    let app = build_router(ServerConfig::with_store("secret-token", store));

    let requested = app
        .oneshot(protocol_request(
            "/v1/reservation/request",
            "s1",
            "w1",
            serde_json::json!({
                "request_id": "request-1",
                "action": "write_file",
                "path": "src/auth.ts",
                "purpose": ""
            }),
        ))
        .await
        .expect("reservation request should complete");

    assert_eq!(requested.status(), StatusCode::BAD_REQUEST);
    let body = to_bytes(requested.into_body(), 1024)
        .await
        .expect("body should read");
    let json: serde_json::Value = serde_json::from_slice(&body).expect("body should be json");
    assert_eq!(json["status"], "error");
    assert_eq!(json["reason_code"], "missing_purpose");
}

#[tokio::test]
async fn reservation_request_rejects_normalized_empty_path() {
    let store = Store::open_in_memory().expect("store should open");
    let app = build_router(ServerConfig::with_store("secret-token", store));

    for path in ["./", "../", "/", "a/.."] {
        let requested = app
            .clone()
            .oneshot(protocol_request(
                "/v1/reservation/request",
                "s1",
                "w1",
                serde_json::json!({
                    "request_id": format!("request-{path}"),
                    "action": "write_file",
                    "path": path,
                    "purpose": "Reserve path before writing."
                }),
            ))
            .await
            .expect("reservation request should complete");

        assert_eq!(requested.status(), StatusCode::BAD_REQUEST);
        let json = response_json(requested, 1024).await;
        assert_eq!(json["status"], "error");
        assert_eq!(json["reason_code"], "missing_scope");
        assert_eq!(
            json["message"],
            "Reservation scope paths must be non-empty after normalization."
        );
    }
}

#[tokio::test]
async fn reservation_request_queues_conflict_and_reuses_request_id() {
    let store = Store::open_in_memory().expect("store should open");
    let app = build_router(ServerConfig::with_store("secret-token", store));

    ensure_test_reservation_via_http(&app, "s1", "w1", "src/auth.ts").await;
    let claim = app
        .clone()
        .oneshot(json_request(
            "/v1/claim/acquire",
            serde_json::json!({
                "agent_id": "s1",
                "workspace_id": "w1",
                "path": "src/auth.ts"
            }),
        ))
        .await
        .expect("claim acquire should complete");
    assert_eq!(claim.status(), StatusCode::OK);

    let first = app
        .clone()
        .oneshot(protocol_request(
            "/v1/reservation/request",
            "s2",
            "w1",
            serde_json::json!({
                "request_id": "request-2",
                "action": "write_file",
                "path": "src/auth.ts",
                "purpose": "Queue s2 auth changes."
            }),
        ))
        .await
        .expect("reservation request should complete");
    assert_eq!(first.status(), StatusCode::OK);
    let body = to_bytes(first.into_body(), 2048)
        .await
        .expect("body should read");
    let json: serde_json::Value = serde_json::from_slice(&body).expect("body should be json");
    assert_eq!(json["request_state"], "queued");
    assert_eq!(json["request_id"], "request-2");
    assert_eq!(json["wait"]["status"], "queued");
    assert_eq!(json["wait"]["queue_position"], 1);
    assert_eq!(json["wait"]["blocking_agent_id"], "s1");
    assert_eq!(json["wait"]["purpose"], "Queue s2 auth changes.");
    let first_wait_id = json["wait"]["wait_id"]
        .as_str()
        .expect("wait id should be present")
        .to_string();

    let repeated = app
        .clone()
        .oneshot(protocol_request(
            "/v1/reservation/request",
            "s2",
            "w1",
            serde_json::json!({
                "request_id": "request-2",
                "action": "write_file",
                "path": "src/auth.ts",
                "purpose": "Retry should not replace queued request purpose."
            }),
        ))
        .await
        .expect("reservation request retry should complete");
    assert_eq!(repeated.status(), StatusCode::OK);
    let body = to_bytes(repeated.into_body(), 2048)
        .await
        .expect("body should read");
    let json: serde_json::Value = serde_json::from_slice(&body).expect("body should be json");
    assert_eq!(json["request_state"], "queued");
    assert_eq!(json["wait"]["wait_id"], first_wait_id);
    assert_eq!(json["wait"]["queue_position"], 1);
    assert_eq!(json["wait"]["purpose"], "Queue s2 auth changes.");

    let second = app
        .oneshot(protocol_request(
            "/v1/reservation/request",
            "s3",
            "w1",
            serde_json::json!({
                "request_id": "request-3",
                "action": "write_file",
                "path": "src/auth.ts",
                "purpose": "Queue s3 auth changes."
            }),
        ))
        .await
        .expect("second reservation request should complete");
    assert_eq!(second.status(), StatusCode::OK);
    let body = to_bytes(second.into_body(), 2048)
        .await
        .expect("body should read");
    let json: serde_json::Value = serde_json::from_slice(&body).expect("body should be json");
    assert_eq!(json["request_state"], "queued");
    assert_eq!(json["wait"]["queue_position"], 2);
}

#[tokio::test]
async fn reservation_cancel_cancels_reserved_request_and_promotes_next_waiter() {
    let store = Store::open_in_memory().expect("store should open");
    let app = build_router(ServerConfig::with_store("secret-token", store));

    ensure_test_reservation_via_http(&app, "s1", "w1", "src/auth.ts").await;
    let claim = app
        .clone()
        .oneshot(json_request(
            "/v1/claim/acquire",
            serde_json::json!({
                "agent_id": "s1",
                "workspace_id": "w1",
                "path": "src/auth.ts"
            }),
        ))
        .await
        .expect("claim acquire should complete");
    assert_eq!(claim.status(), StatusCode::OK);

    for (agent_id, request_id) in [("s2", "request-2"), ("s3", "request-3")] {
        let queued = app
            .clone()
            .oneshot(protocol_request(
                "/v1/reservation/request",
                agent_id,
                "w1",
                serde_json::json!({
                    "request_id": request_id,
                    "action": "write_file",
                    "path": "src/auth.ts",
                    "purpose": format!("Queue {agent_id} auth changes.")
                }),
            ))
            .await
            .expect("reservation request should complete");
        assert_eq!(queued.status(), StatusCode::OK);
    }

    let release = app
        .clone()
        .oneshot(json_request(
            "/v1/claim/release",
            serde_json::json!({
                "agent_id": "s1",
                "workspace_id": "w1",
                "path": "src/auth.ts"
            }),
        ))
        .await
        .expect("claim release should complete");
    assert_eq!(release.status(), StatusCode::OK);

    let canceled = app
        .clone()
        .oneshot(protocol_request(
            "/v1/reservation/cancel",
            "s2",
            "w1",
            serde_json::json!({
                "request_id": "request-2"
            }),
        ))
        .await
        .expect("reservation cancel should complete");
    assert_eq!(canceled.status(), StatusCode::OK);
    let body = to_bytes(canceled.into_body(), 2048)
        .await
        .expect("body should read");
    let json: serde_json::Value = serde_json::from_slice(&body).expect("body should be json");
    assert_eq!(json["status"], "ok");
    assert_eq!(json["request_state"], "canceled");
    assert_eq!(json["request_id"], "request-2");
    assert_eq!(json["wait"]["status"], "canceled");

    let resume = app
        .oneshot(json_request(
            "/v1/resume/next",
            serde_json::json!({
                "agent_id": "s3",
                "workspace_id": "w1"
            }),
        ))
        .await
        .expect("resume next should complete");
    assert_eq!(resume.status(), StatusCode::OK);
    let body = to_bytes(resume.into_body(), 2048)
        .await
        .expect("body should read");
    let json: serde_json::Value = serde_json::from_slice(&body).expect("body should be json");
    assert_eq!(json["resume_available"], true);
    assert_eq!(json["reservation"]["agent_id"], "s3");
    assert_eq!(json["reservation"]["relative_path"], "src/auth.ts");
}

#[tokio::test]
async fn reservation_cancel_rejects_other_session_request() {
    let store = Store::open_in_memory().expect("store should open");
    let app = build_router(ServerConfig::with_store("secret-token", store));

    let requested = app
        .clone()
        .oneshot(protocol_request(
            "/v1/reservation/request",
            "s2",
            "w1",
            serde_json::json!({
                "request_id": "request-2",
                "action": "write_file",
                "path": "src/auth.ts",
                "purpose": "Queue s2 auth changes."
            }),
        ))
        .await
        .expect("reservation request should complete");
    assert_eq!(requested.status(), StatusCode::OK);

    let rejected = app
        .oneshot(protocol_request(
            "/v1/reservation/cancel",
            "s3",
            "w1",
            serde_json::json!({
                "request_id": "request-2"
            }),
        ))
        .await
        .expect("reservation cancel should complete");
    assert_eq!(rejected.status(), StatusCode::CONFLICT);
    let body = to_bytes(rejected.into_body(), 2048)
        .await
        .expect("body should read");
    let json: serde_json::Value = serde_json::from_slice(&body).expect("body should be json");
    assert_eq!(json["status"], "error");
    assert_eq!(json["reason_code"], "cancel_failed");
}

#[tokio::test]
async fn activity_finalize_reclaims_claims_and_notifications_poll_returns_resume_signal() {
    let store = Store::open_in_memory().expect("store should open");
    let app = build_router(ServerConfig::with_store("secret-token", store));

    ensure_test_reservation_via_http(&app, "s1", "w1", "src/auth.ts").await;
    let claim = app
        .clone()
        .oneshot(json_request(
            "/v1/claim/acquire",
            serde_json::json!({
                "agent_id": "s1",
                "workspace_id": "w1",
                "path": "src/auth.ts"
            }),
        ))
        .await
        .expect("claim acquire should complete");
    assert_eq!(claim.status(), StatusCode::OK);

    let declare = app
        .clone()
        .oneshot(protocol_request(
            "/v1/reservation/declare",
            "s2",
            "w1",
            serde_json::json!({
                "purpose": "Test requested work.",
                "files_planned": ["src/auth.ts"]
            }),
        ))
        .await
        .expect("reservation declaration should complete");
    assert_eq!(declare.status(), StatusCode::OK);

    let queued = app
        .clone()
        .oneshot(protocol_request(
            "/v1/authorize",
            "s2",
            "w1",
            serde_json::json!({
                "action": "write_file",
                "path": "src/auth.ts",
                "queue_on_conflict": true,
                "purpose": "Queue requested write after blocker clears."
            }),
        ))
        .await
        .expect("authorize should complete");
    assert_eq!(queued.status(), StatusCode::OK);
    let queued = response_json(queued, 2048).await;
    let wait_id = queued["wait"]["wait_id"]
        .as_str()
        .expect("wait_id should be present")
        .to_string();

    let finalize = app
        .clone()
        .oneshot(json_request(
            "/v1/activity/finalize",
            serde_json::json!({
                "agent_id": "s1",
                "workspace_id": "w1"
            }),
        ))
        .await
        .expect("finalize should complete");
    assert_eq!(finalize.status(), StatusCode::OK);

    let poll = app
        .clone()
        .oneshot(json_request(
            "/v1/notifications/poll",
            serde_json::json!({
                "agent_id": "s2",
                "workspace_id": "w1"
            }),
        ))
        .await
        .expect("notification poll should complete");
    assert_eq!(poll.status(), StatusCode::OK);

    let body = to_bytes(poll.into_body(), 2048)
        .await
        .expect("body should read");
    let json: serde_json::Value = serde_json::from_slice(&body).expect("body should be json");
    assert_eq!(json["status"], "ok");
    assert_eq!(json["notifications"][0]["kind"], "reservation_granted");
    assert_eq!(
        json["notifications"][0]["payload"]["relative_path"],
        "src/auth.ts"
    );
    assert_eq!(
        json["notifications"][0]["payload"]["purpose"],
        "Queue requested write after blocker clears."
    );
    assert_eq!(json["notifications"][0]["payload"]["wait_id"], wait_id);
    assert_eq!(
        json["notifications"][0]["payload"]["reservation_id"],
        wait_id
    );

    let second_poll = app
        .oneshot(json_request(
            "/v1/notifications/poll",
            serde_json::json!({
                "agent_id": "s2",
                "workspace_id": "w1"
            }),
        ))
        .await
        .expect("second notification poll should complete");
    assert_eq!(second_poll.status(), StatusCode::OK);
    let body = to_bytes(second_poll.into_body(), 2048)
        .await
        .expect("body should read");
    let json: serde_json::Value = serde_json::from_slice(&body).expect("body should be json");
    assert_eq!(json["status"], "ok");
    assert_eq!(
        json["notifications"]
            .as_array()
            .expect("notifications array")
            .len(),
        0
    );
}

#[tokio::test]
async fn notifications_stream_emits_reservation_granted_sse() {
    let store = Store::open_in_memory().expect("store should open");
    let app = build_router(ServerConfig::with_store("secret-token", store));

    ensure_test_reservation_via_http(&app, "s1", "w1", "src/auth.ts").await;
    let claim = app
        .clone()
        .oneshot(json_request(
            "/v1/claim/acquire",
            serde_json::json!({
                "agent_id": "s1",
                "workspace_id": "w1",
                "path": "src/auth.ts"
            }),
        ))
        .await
        .expect("claim acquire should complete");
    assert_eq!(claim.status(), StatusCode::OK);

    let declare = app
        .clone()
        .oneshot(protocol_request(
            "/v1/reservation/declare",
            "s2",
            "w1",
            serde_json::json!({
                "purpose": "Test requested work.",
                "files_planned": ["src/auth.ts"]
            }),
        ))
        .await
        .expect("reservation declaration should complete");
    assert_eq!(declare.status(), StatusCode::OK);

    let queued = app
        .clone()
        .oneshot(protocol_request(
            "/v1/authorize",
            "s2",
            "w1",
            serde_json::json!({
                "action": "write_file",
                "path": "src/auth.ts",
                "queue_on_conflict": true,
                "purpose": "Queue requested write after blocker clears."
            }),
        ))
        .await
        .expect("authorize should complete");
    assert_eq!(queued.status(), StatusCode::OK);

    let finalize = app
        .clone()
        .oneshot(json_request(
            "/v1/activity/finalize",
            serde_json::json!({
                "agent_id": "s1",
                "workspace_id": "w1"
            }),
        ))
        .await
        .expect("finalize should complete");
    assert_eq!(finalize.status(), StatusCode::OK);

    let stream = app
        .oneshot(authorized_get(
            "/v1/notifications/stream?agent_id=s2&workspace_id=w1",
        ))
        .await
        .expect("notification stream should complete");
    assert_eq!(stream.status(), StatusCode::OK);
    assert_eq!(stream.headers()["content-type"], "text/event-stream");

    let mut body = stream.into_body().into_data_stream();
    let chunk = tokio::time::timeout(Duration::from_secs(2), body.next())
        .await
        .expect("stream should emit notification")
        .expect("stream item should exist")
        .expect("stream chunk should read");
    let text = String::from_utf8(chunk.to_vec()).expect("chunk should be utf8");
    assert!(text.contains("event: reservation_granted"));
    assert!(text.contains("id: 1"));
    assert!(text.contains("\"sequence\":1"));
    assert!(text.contains("\"relative_path\":\"src/auth.ts\""));
    assert!(text.contains("\"purpose\":\"Queue requested write after blocker clears.\""));
    assert!(text.contains("state.reservation.claim"));
}

#[tokio::test]
async fn notifications_stream_replays_until_last_event_id_acknowledges() {
    let store = Store::open_in_memory().expect("store should open");
    let app = build_router(ServerConfig::with_store("secret-token", store));

    ensure_test_reservation_via_http(&app, "s1", "w1", "src/replay.ts").await;
    let claim = app
        .clone()
        .oneshot(json_request(
            "/v1/claim/acquire",
            serde_json::json!({
                "agent_id": "s1",
                "workspace_id": "w1",
                "path": "src/replay.ts"
            }),
        ))
        .await
        .expect("claim acquire should complete");
    assert_eq!(claim.status(), StatusCode::OK);

    let declare = app
        .clone()
        .oneshot(protocol_request(
            "/v1/reservation/declare",
            "s2",
            "w1",
            serde_json::json!({
                "purpose": "Test replayable SSE notification.",
                "files_planned": ["src/replay.ts"]
            }),
        ))
        .await
        .expect("reservation declaration should complete");
    assert_eq!(declare.status(), StatusCode::OK);

    let queued = app
        .clone()
        .oneshot(protocol_request(
            "/v1/authorize",
            "s2",
            "w1",
            serde_json::json!({
                "action": "write_file",
                "path": "src/replay.ts",
                "queue_on_conflict": true,
                "purpose": "Queue replay test after blocker clears."
            }),
        ))
        .await
        .expect("authorize should complete");
    assert_eq!(queued.status(), StatusCode::OK);

    let finalize = app
        .clone()
        .oneshot(json_request(
            "/v1/activity/finalize",
            serde_json::json!({
                "agent_id": "s1",
                "workspace_id": "w1"
            }),
        ))
        .await
        .expect("finalize should complete");
    assert_eq!(finalize.status(), StatusCode::OK);

    let first_stream = app
        .clone()
        .oneshot(authorized_get(
            "/v1/notifications/stream?agent_id=s2&workspace_id=w1",
        ))
        .await
        .expect("first stream should complete");
    assert_eq!(first_stream.status(), StatusCode::OK);
    let mut first_body = first_stream.into_body().into_data_stream();
    let first_chunk = tokio::time::timeout(Duration::from_secs(2), first_body.next())
        .await
        .expect("first stream should emit notification")
        .expect("first stream item should exist")
        .expect("first stream chunk should read");
    let first_text = String::from_utf8(first_chunk.to_vec()).expect("first chunk should be utf8");
    assert!(first_text.contains("id: 1"));
    assert!(first_text.contains("\"relative_path\":\"src/replay.ts\""));
    drop(first_body);

    let replay_stream = app
        .clone()
        .oneshot(authorized_get(
            "/v1/notifications/stream?agent_id=s2&workspace_id=w1",
        ))
        .await
        .expect("replay stream should complete");
    assert_eq!(replay_stream.status(), StatusCode::OK);
    let mut replay_body = replay_stream.into_body().into_data_stream();
    let replay_chunk = tokio::time::timeout(Duration::from_secs(2), replay_body.next())
        .await
        .expect("replay stream should emit notification")
        .expect("replay stream item should exist")
        .expect("replay stream chunk should read");
    let replay_text =
        String::from_utf8(replay_chunk.to_vec()).expect("replay chunk should be utf8");
    assert!(replay_text.contains("id: 1"));
    assert!(replay_text.contains("\"relative_path\":\"src/replay.ts\""));
    drop(replay_body);

    let acked_stream = app
        .clone()
        .oneshot(authorized_get_with_last_event_id(
            "/v1/notifications/stream?agent_id=s2&workspace_id=w1",
            "1",
        ))
        .await
        .expect("acked stream should complete");
    assert_eq!(acked_stream.status(), StatusCode::OK);
    let mut acked_body = acked_stream.into_body().into_data_stream();
    assert!(
        tokio::time::timeout(Duration::from_millis(200), acked_body.next())
            .await
            .is_err(),
        "acked stream should not replay sequence 1"
    );
    drop(acked_body);

    let poll = app
        .oneshot(json_request(
            "/v1/notifications/poll",
            serde_json::json!({
                "agent_id": "s2",
                "workspace_id": "w1"
            }),
        ))
        .await
        .expect("notification poll should complete");
    assert_eq!(poll.status(), StatusCode::OK);
    let json = response_json(poll, 2048).await;
    assert_eq!(
        json["notifications"]
            .as_array()
            .expect("notifications array")
            .len(),
        0
    );
}

#[tokio::test]
async fn notifications_stream_rejects_invalid_agent_id() {
    let app = build_router(ServerConfig::new("secret-token"));

    let response = app
        .oneshot(authorized_get(
            "/v1/notifications/stream?agent_id=bad/id&workspace_id=w1",
        ))
        .await
        .expect("notification stream should complete");
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let json = response_json(response, 1024).await;
    assert_eq!(json["reason_code"], "invalid_agent_id");
}

#[tokio::test]
async fn activity_finalize_clears_active_reservation_for_agent() {
    let app = build_router(ServerConfig::new("secret-token"));

    ensure_test_reservation_via_http(&app, "s1", "w1", "src/auth.ts").await;

    let before = app
        .clone()
        .oneshot(authorized_get("/v1/current?resource=src/auth.ts"))
        .await
        .expect("current request should complete");
    assert_eq!(before.status(), StatusCode::OK);
    let json = response_json(before, 4096).await;
    assert!(json["items"].as_array().is_some_and(|items| {
        items
            .iter()
            .any(|item| item["kind"] == "reservation" && item["agent_id"] == "s1")
    }));

    let finalize = app
        .clone()
        .oneshot(json_request(
            "/v1/activity/finalize",
            serde_json::json!({
                "agent_id": "s1",
                "workspace_id": "w1"
            }),
        ))
        .await
        .expect("finalize should complete");
    assert_eq!(finalize.status(), StatusCode::OK);
    let json = response_json(finalize, 2048).await;
    assert_eq!(json["released_claims"], 0);
    assert_eq!(json["completed_reservations"], 1);

    let after = app
        .oneshot(authorized_get("/v1/current?resource=src/auth.ts"))
        .await
        .expect("current request should complete");
    assert_eq!(after.status(), StatusCode::OK);
    let json = response_json(after, 4096).await;
    assert!(json["items"].as_array().is_some_and(|items| {
        !items
            .iter()
            .any(|item| item["kind"] == "reservation" && item["agent_id"] == "s1")
    }));
    assert_eq!(json["current"]["active_reservation_count"], 0);
}

#[tokio::test]
async fn activity_finalize_rolls_back_activity_and_lease_release_when_intent_completion_fails() {
    let temp = tempfile::tempdir().expect("temp dir should create");
    let temp_root = temp.path();
    let db_path = temp_root.join(".stateful_core").join("state.db");
    let store = Store::open(&db_path).expect("file store should open");
    let app = build_router(ServerConfig::with_store("secret-token", store));

    ensure_test_reservation_via_http(&app, "s1", "w1", "src/auth.ts").await;
    let claim = app
        .clone()
        .oneshot(json_request(
            "/v1/claim/acquire",
            serde_json::json!({
                "agent_id": "s1",
                "workspace_id": "w1",
                "path": "src/auth.ts"
            }),
        ))
        .await
        .expect("claim acquire should complete");
    assert_eq!(claim.status(), StatusCode::OK);

    let trigger_conn =
        rusqlite::Connection::open(&db_path).expect("trigger connection should open");
    trigger_conn
        .execute_batch(
            "CREATE TRIGGER fail_finalize_intent_completion
             BEFORE UPDATE OF status ON reservations
             WHEN NEW.status = 'completed'
             BEGIN
                 SELECT RAISE(ABORT, 'simulated reservation completion failure');
             END;",
        )
        .expect("failure trigger should install");

    let finalize = app
        .oneshot(json_request(
            "/v1/activity/finalize",
            serde_json::json!({
                "agent_id": "s1",
                "workspace_id": "w1"
            }),
        ))
        .await
        .expect("finalize should complete");
    assert_eq!(finalize.status(), StatusCode::INTERNAL_SERVER_ERROR);
    let json = response_json(finalize, 2048).await;
    assert!(
        json["message"]
            .as_str()
            .expect("error should be string")
            .contains("simulated reservation completion failure")
    );

    let activity_count: u64 = trigger_conn
        .query_row(
            "SELECT COUNT(*) FROM activities
             WHERE agent_id = 's1'
               AND workspace_id = 'w1'",
            [],
            |row| row.get::<_, u64>(0),
        )
        .expect("activity count should load");
    assert_eq!(activity_count, 1);

    let active_claim_count: u64 = trigger_conn
        .query_row(
            "SELECT COUNT(*) FROM claims
             WHERE workspace_id = 'w1'
               AND agent_id = 's1'
               AND relative_path = 'src/auth.ts'
               AND status = 'active'",
            [],
            |row| row.get::<_, u64>(0),
        )
        .expect("active claim count should load");
    assert_eq!(active_claim_count, 1);

    let active_reservation_count: u64 = trigger_conn
        .query_row(
            "SELECT COUNT(*) FROM reservations
             WHERE workspace_id = 'w1'
               AND agent_id = 's1'
               AND status = 'active'",
            [],
            |row| row.get::<_, u64>(0),
        )
        .expect("active reservation count should load");
    assert_eq!(active_reservation_count, 1);

    drop(trigger_conn);
}

#[tokio::test]
async fn resume_next_returns_active_reservation_for_agent() {
    let store = Store::open_in_memory().expect("store should open");
    let app = build_router(ServerConfig::with_store("secret-token", store));

    ensure_test_reservation_via_http(&app, "s1", "w1", "src/auth.ts").await;
    let claim = app
        .clone()
        .oneshot(json_request(
            "/v1/claim/acquire",
            serde_json::json!({
                "agent_id": "s1",
                "workspace_id": "w1",
                "path": "src/auth.ts"
            }),
        ))
        .await
        .expect("claim acquire should complete");
    assert_eq!(claim.status(), StatusCode::OK);

    let declare = app
        .clone()
        .oneshot(protocol_request(
            "/v1/reservation/declare",
            "s2",
            "w1",
            serde_json::json!({
                "purpose": "Test requested work.",
                "files_planned": ["src/auth.ts"]
            }),
        ))
        .await
        .expect("reservation declaration should complete");
    assert_eq!(declare.status(), StatusCode::OK);

    let queued = app
        .clone()
        .oneshot(protocol_request(
            "/v1/authorize",
            "s2",
            "w1",
            serde_json::json!({
                "action": "write_file",
                "path": "src/auth.ts",
                "queue_on_conflict": true,
                "purpose": "Queue requested write after blocker clears."
            }),
        ))
        .await
        .expect("authorize should complete");
    assert_eq!(queued.status(), StatusCode::OK);

    let release = app
        .clone()
        .oneshot(json_request(
            "/v1/claim/release",
            serde_json::json!({
                "agent_id": "s1",
                "workspace_id": "w1",
                "path": "src/auth.ts"
            }),
        ))
        .await
        .expect("claim release should complete");
    assert_eq!(release.status(), StatusCode::OK);

    let resume = app
        .oneshot(json_request(
            "/v1/resume/next",
            serde_json::json!({
                "agent_id": "s2",
                "workspace_id": "w1"
            }),
        ))
        .await
        .expect("resume next should complete");
    assert_eq!(resume.status(), StatusCode::OK);

    let body = to_bytes(resume.into_body(), 2048)
        .await
        .expect("body should read");
    let json: serde_json::Value = serde_json::from_slice(&body).expect("body should be json");
    assert_eq!(json["status"], "ok");
    assert_eq!(json["resume_available"], true);
    assert_eq!(json["reservation"]["agent_id"], "s2");
    assert_eq!(json["reservation"]["relative_path"], "src/auth.ts");
    assert_eq!(
        json["reservation"]["purpose"],
        "Queue requested write after blocker clears."
    );
    assert_eq!(
        json["required_next_action"],
        "Reread the target, then call state.reservation.claim for the reservation before writing."
    );
}

#[tokio::test]
async fn active_claim_allows_matching_authorize_without_explicit_reservation_id() {
    let app = build_router(ServerConfig::new("secret-token"));

    ensure_test_reservation_via_http(&app, "s1", "w1", "src/auth.ts").await;
    let claim = app
        .clone()
        .oneshot(json_request(
            "/v1/claim/acquire",
            serde_json::json!({
                "agent_id": "s1",
                "workspace_id": "w1",
                "path": "src/auth.ts"
            }),
        ))
        .await
        .expect("claim acquire should complete");
    assert_eq!(claim.status(), StatusCode::OK);

    let declare = app
        .clone()
        .oneshot(protocol_request(
            "/v1/reservation/declare",
            "s1",
            "w1",
            serde_json::json!({
                "purpose": "Test requested work.",
                "files_planned": ["src/auth.ts"]
            }),
        ))
        .await
        .expect("reservation declaration should complete");
    assert_eq!(declare.status(), StatusCode::OK);

    let response = app
        .oneshot(protocol_request(
            "/v1/authorize",
            "s1",
            "w1",
            serde_json::json!({
                "action": "write_file",
                "path": "src/auth.ts"
            }),
        ))
        .await
        .expect("authorize should complete");
    assert_eq!(response.status(), StatusCode::OK);

    let body = to_bytes(response.into_body(), 1024)
        .await
        .expect("body should read");
    let json: serde_json::Value = serde_json::from_slice(&body).expect("body should be json");
    assert_eq!(json["decision"], "allow");
    assert_eq!(json["reason_code"], "authorized");
}

#[tokio::test]
async fn repo_write_authorization_requires_lease_and_updates_rendered_state_until_release() {
    let app = build_router(ServerConfig::new("secret-token"));

    let declare = app
        .clone()
        .oneshot(protocol_request(
            "/v1/reservation/declare",
            "s1",
            "w1",
            serde_json::json!({
                "purpose": "Update auth implementation.",
                "files_planned": ["src/auth.ts"]
            }),
        ))
        .await
        .expect("reservation declaration should complete");
    assert_eq!(declare.status(), StatusCode::OK);

    let before_lease = app
        .clone()
        .oneshot(native_hook_authorize_request(
            "s1",
            "w1",
            "src/auth.ts",
            "Edit",
        ))
        .await
        .expect("authorize before claim should complete");
    assert_eq!(before_lease.status(), StatusCode::OK);
    let json = response_json(before_lease, 2048).await;
    assert_eq!(json["decision"], "deny");
    assert_eq!(json["reason_code"], "missing_claim");
    assert!(
        json.get("wait").is_none() && json.get("reservation").is_none(),
        "missing same-reservation claim should deny automatically without approval queue state: {json}"
    );

    let claim = app
        .clone()
        .oneshot(json_request(
            "/v1/claim/acquire",
            serde_json::json!({
                "agent_id": "s1",
                "workspace_id": "w1",
                "path": "src/auth.ts"
            }),
        ))
        .await
        .expect("claim acquire should complete");
    assert_eq!(claim.status(), StatusCode::OK);

    let with_lease = app
        .clone()
        .oneshot(native_hook_authorize_request(
            "s1",
            "w1",
            "src/auth.ts",
            "Edit",
        ))
        .await
        .expect("authorize with claim should complete");
    assert_eq!(with_lease.status(), StatusCode::OK);
    let json = response_json(with_lease, 2048).await;
    assert_eq!(json["decision"], "allow");
    assert_eq!(json["reason_code"], "authorized");

    let rendered_with_lease = app
        .clone()
        .oneshot(json_request(
            "/v1/context/render",
            serde_json::json!({
                "mode": "detailed",
                "workspace_id": "w1",
                "agent_id": "s1"
            }),
        ))
        .await
        .expect("context render with claim should complete");
    assert_eq!(rendered_with_lease.status(), StatusCode::OK);
    let json = response_json(rendered_with_lease, 4096).await;
    let items = json["items"].as_array().expect("items should be an array");
    assert!(
        items.iter().any(|item| {
            item["kind"] == "claim"
                && item["resource"] == "src/auth.ts"
                && item["agent_id"] == "s1"
                && item["source_refs"]
                    .as_array()
                    .is_some_and(|refs| refs.iter().any(|value| value == "AgentContextScope"))
        }),
        "render should show same-reservation claim before write completes: {items:?}"
    );

    let release = app
        .clone()
        .oneshot(json_request(
            "/v1/claim/release",
            serde_json::json!({
                "agent_id": "s1",
                "workspace_id": "w1",
                "path": "src/auth.ts"
            }),
        ))
        .await
        .expect("claim release should complete");
    assert_eq!(release.status(), StatusCode::OK);

    let rendered_after_release = app
        .clone()
        .oneshot(json_request(
            "/v1/context/render",
            serde_json::json!({
                "mode": "detailed",
                "workspace_id": "w1",
                "agent_id": "s1"
            }),
        ))
        .await
        .expect("context render after release should complete");
    assert_eq!(rendered_after_release.status(), StatusCode::OK);
    let json = response_json(rendered_after_release, 4096).await;
    let items = json["items"].as_array().expect("items should be an array");
    assert!(
        !items.iter().any(|item| {
            item["kind"] == "claim" && item["resource"] == "src/auth.ts" && item["agent_id"] == "s1"
        }),
        "render should remove claim after release: {items:?}"
    );
    assert!(
        items.iter().any(|item| {
            item["kind"] == "reservation"
                && item["resource"] == "src/auth.ts"
                && item["agent_id"] == "s1"
        }),
        "render should keep declared reservation visible after claim release: {items:?}"
    );

    let after_release = app
        .oneshot(native_hook_authorize_request(
            "s1",
            "w1",
            "src/auth.ts",
            "Edit",
        ))
        .await
        .expect("authorize after release should complete");
    assert_eq!(after_release.status(), StatusCode::OK);
    let json = response_json(after_release, 2048).await;
    assert_eq!(json["decision"], "deny");
    assert_eq!(json["reason_code"], "missing_claim");
}

#[tokio::test]
async fn rename_file_denies_when_other_session_claims_destination() {
    let store = Store::open_in_memory().expect("store should open");
    store
        .append(Event::reservation_declared(
            "s1",
            "w1",
            "Test rename authorization behavior.",
            ["src/old.ts", "src/new.ts"],
        ))
        .expect("reservation should append");
    acquire_test_lease(&store, "s1", "w1", "src/old.ts");
    acquire_test_lease(&store, "s2", "w1", "src/new.ts");
    let app = build_router(ServerConfig::with_store("secret-token", store));

    let response = app
        .oneshot(protocol_request(
            "/v1/authorize",
            "s1",
            "w1",
            serde_json::json!({
                "action": "rename_file",
                "path": "src/old.ts",
                "old_path": "src/old.ts",
                "new_path": "src/new.ts",
                "queue_on_conflict": true,
                "purpose": "Queue requested rename after blocker clears."
            }),
        ))
        .await
        .expect("authorize should complete");

    let body = to_bytes(response.into_body(), 1024)
        .await
        .expect("body should read");
    let json: serde_json::Value = serde_json::from_slice(&body).expect("body should be json");
    assert_eq!(json["decision"], "deny");
    assert_eq!(json["reason_code"], "active_claim_conflict");
    assert!(json.get("wait").is_none());
}

#[tokio::test]
async fn rename_file_denies_when_other_session_claims_source() {
    let store = Store::open_in_memory().expect("store should open");
    store
        .append(Event::reservation_declared(
            "s1",
            "w1",
            "Test rename authorization behavior.",
            ["src/old.ts", "src/new.ts"],
        ))
        .expect("reservation should append");
    acquire_test_lease(&store, "s2", "w1", "src/old.ts");
    acquire_test_lease(&store, "s1", "w1", "src/new.ts");
    let app = build_router(ServerConfig::with_store("secret-token", store));

    let response = app
        .oneshot(protocol_request(
            "/v1/authorize",
            "s1",
            "w1",
            serde_json::json!({
                "action": "rename_file",
                "path": "src/old.ts",
                "old_path": "src/old.ts",
                "new_path": "src/new.ts"
            }),
        ))
        .await
        .expect("authorize should complete");

    let body = to_bytes(response.into_body(), 1024)
        .await
        .expect("body should read");
    let json: serde_json::Value = serde_json::from_slice(&body).expect("body should be json");
    assert_eq!(json["decision"], "deny");
    assert_eq!(json["reason_code"], "active_claim_conflict");
}

#[tokio::test]
async fn rename_file_denies_when_other_session_reserves_destination() {
    let store = Store::open_in_memory().expect("store should open");
    store
        .append(Event::reservation_declared(
            "s1",
            "w1",
            "Test rename authorization behavior.",
            ["src/old.ts", "src/new.ts"],
        ))
        .expect("reservation should append");
    store
        .enqueue_waiter(
            "s2",
            "w1",
            "src/new.ts",
            "write_file",
            "Queue requested file write after blocker clears.",
            Some("s3"),
        )
        .expect("destination waiter should enqueue");
    store
        .promote_next_waiter("w1", "src/new.ts")
        .expect("destination waiter should promote");
    let app = build_router(ServerConfig::with_store("secret-token", store));

    let response = app
        .oneshot(protocol_request(
            "/v1/authorize",
            "s1",
            "w1",
            serde_json::json!({
                "action": "rename_file",
                "path": "src/old.ts",
                "old_path": "src/old.ts",
                "new_path": "src/new.ts"
            }),
        ))
        .await
        .expect("authorize should complete");

    let body = to_bytes(response.into_body(), 2048)
        .await
        .expect("body should read");
    let json: serde_json::Value = serde_json::from_slice(&body).expect("body should be json");
    assert_eq!(json["decision"], "deny");
    assert_eq!(json["reason_code"], "reservation_conflict");
    assert_eq!(json["reservation"]["relative_path"], "src/new.ts");
}

#[tokio::test]
async fn rename_file_denies_when_other_session_reserves_source() {
    let store = Store::open_in_memory().expect("store should open");
    store
        .append(Event::reservation_declared(
            "s1",
            "w1",
            "Test rename authorization behavior.",
            ["src/old.ts", "src/new.ts"],
        ))
        .expect("reservation should append");
    store
        .enqueue_waiter(
            "s2",
            "w1",
            "src/old.ts",
            "write_file",
            "Queue requested file write after blocker clears.",
            Some("s3"),
        )
        .expect("source waiter should enqueue");
    store
        .promote_next_waiter("w1", "src/old.ts")
        .expect("source waiter should promote");
    let app = build_router(ServerConfig::with_store("secret-token", store));

    let response = app
        .oneshot(protocol_request(
            "/v1/authorize",
            "s1",
            "w1",
            serde_json::json!({
                "action": "rename_file",
                "path": "src/old.ts",
                "old_path": "src/old.ts",
                "new_path": "src/new.ts"
            }),
        ))
        .await
        .expect("authorize should complete");

    let body = to_bytes(response.into_body(), 2048)
        .await
        .expect("body should read");
    let json: serde_json::Value = serde_json::from_slice(&body).expect("body should be json");
    assert_eq!(json["decision"], "deny");
    assert_eq!(json["reason_code"], "reservation_conflict");
    assert_eq!(json["reservation"]["relative_path"], "src/old.ts");
}

#[tokio::test]
async fn rename_file_requires_old_and_new_paths() {
    let app = build_router(ServerConfig::new("secret-token"));

    for payload in [
        serde_json::json!({
            "action": "rename_file",
            "path": "src/old.ts",
            "old_path": "src/old.ts"
        }),
        serde_json::json!({
            "action": "rename_file",
            "path": "src/old.ts",
            "new_path": "src/new.ts"
        }),
        serde_json::json!({
            "action": "move_file",
            "path": "src/old.ts",
            "old_path": "",
            "new_path": "src/new.ts"
        }),
    ] {
        let response = app
            .clone()
            .oneshot(protocol_request("/v1/authorize", "s1", "w1", payload))
            .await
            .expect("authorize should complete");

        let body = to_bytes(response.into_body(), 1024)
            .await
            .expect("body should read");
        let json: serde_json::Value = serde_json::from_slice(&body).expect("body should be json");
        assert_eq!(json["decision"], "deny");
        assert_eq!(json["reason_code"], "missing_rename_paths");
    }
}

#[tokio::test]
async fn delete_file_action_requires_exact_file_intent_over_http() {
    let app = build_router(ServerConfig::new("secret-token"));

    let declare = app
        .clone()
        .oneshot(protocol_request(
            "/v1/reservation/declare",
            "s1",
            "w1",
            serde_json::json!({
                "purpose": "Test requested work.",
                "files_planned": ["src/"]
            }),
        ))
        .await
        .expect("reservation declaration should complete");
    assert_eq!(declare.status(), StatusCode::OK);

    let response = app
        .oneshot(protocol_request(
            "/v1/authorize",
            "s1",
            "w1",
            serde_json::json!({
                "action": "delete_file",
                "path": "src/auth.ts"
            }),
        ))
        .await
        .expect("authorize should complete");
    assert_eq!(response.status(), StatusCode::OK);

    let body = to_bytes(response.into_body(), 1024)
        .await
        .expect("body should read");
    let json: serde_json::Value = serde_json::from_slice(&body).expect("body should be json");
    assert_eq!(json["decision"], "deny");
    assert_eq!(json["reason_code"], "scope_mismatch");
}

#[tokio::test]
async fn write_directory_action_requires_exact_directory_intent_over_http() {
    let app = build_router(ServerConfig::new("secret-token"));

    let declare = app
        .clone()
        .oneshot(protocol_request(
            "/v1/reservation/declare",
            "s1",
            "w1",
            serde_json::json!({
                "purpose": "Test requested work.",
                "files_planned": ["target/"]
            }),
        ))
        .await
        .expect("reservation declaration should complete");
    assert_eq!(declare.status(), StatusCode::OK);

    ensure_test_reservation_via_http(&app, "s1", "w1", "target/").await;
    let claim = app
        .clone()
        .oneshot(json_request(
            "/v1/claim/acquire",
            serde_json::json!({
                "agent_id": "s1",
                "workspace_id": "w1",
                "path": "target/"
            }),
        ))
        .await
        .expect("claim acquire should complete");
    assert_eq!(claim.status(), StatusCode::OK);

    let allowed = app
        .clone()
        .oneshot(protocol_request(
            "/v1/authorize",
            "s1",
            "w1",
            serde_json::json!({
                "action": "write_directory",
                "path": "target/"
            }),
        ))
        .await
        .expect("authorize should complete");
    assert_eq!(allowed.status(), StatusCode::OK);
    let body = to_bytes(allowed.into_body(), 1024)
        .await
        .expect("body should read");
    let json: serde_json::Value = serde_json::from_slice(&body).expect("body should be json");
    assert_eq!(json["decision"], "allow");

    let denied = app
        .oneshot(protocol_request(
            "/v1/authorize",
            "s1",
            "w1",
            serde_json::json!({
                "action": "write_directory",
                "path": "target/debug/"
            }),
        ))
        .await
        .expect("authorize should complete");
    assert_eq!(denied.status(), StatusCode::OK);
    let body = to_bytes(denied.into_body(), 1024)
        .await
        .expect("body should read");
    let json: serde_json::Value = serde_json::from_slice(&body).expect("body should be json");
    assert_eq!(json["decision"], "deny");
    assert_eq!(json["reason_code"], "scope_mismatch");
}

#[tokio::test]
async fn write_directory_action_denies_when_subtree_has_other_session_lease() {
    let app = build_router(ServerConfig::new("secret-token"));

    ensure_test_reservation_via_http(&app, "s2", "w1", "target/out.txt").await;
    let claim = app
        .clone()
        .oneshot(json_request(
            "/v1/claim/acquire",
            serde_json::json!({
                "agent_id": "s2",
                "workspace_id": "w1",
                "path": "target/out.txt"
            }),
        ))
        .await
        .expect("claim acquire should complete");
    assert_eq!(claim.status(), StatusCode::OK);

    let declare = app
        .clone()
        .oneshot(protocol_request(
            "/v1/reservation/declare",
            "s1",
            "w1",
            serde_json::json!({
                "purpose": "Test requested work.",
                "files_planned": ["target/"]
            }),
        ))
        .await
        .expect("reservation declaration should complete");
    assert_eq!(declare.status(), StatusCode::OK);

    let response = app
        .oneshot(protocol_request(
            "/v1/authorize",
            "s1",
            "w1",
            serde_json::json!({
                "action": "write_directory",
                "path": "target/"
            }),
        ))
        .await
        .expect("authorize should complete");
    assert_eq!(response.status(), StatusCode::OK);

    let body = to_bytes(response.into_body(), 1024)
        .await
        .expect("body should read");
    let json: serde_json::Value = serde_json::from_slice(&body).expect("body should be json");
    assert_eq!(json["decision"], "deny");
    assert_eq!(json["reason_code"], "active_claim_conflict");
}

#[tokio::test]
async fn write_file_action_denies_when_ancestor_directory_has_other_session_lease() {
    let app = build_router(ServerConfig::new("secret-token"));

    ensure_test_reservation_via_http(&app, "s2", "w1", "target/").await;
    let claim = app
        .clone()
        .oneshot(json_request(
            "/v1/claim/acquire",
            serde_json::json!({
                "agent_id": "s2",
                "workspace_id": "w1",
                "path": "target/"
            }),
        ))
        .await
        .expect("claim acquire should complete");
    assert_eq!(claim.status(), StatusCode::OK);

    let declare = app
        .clone()
        .oneshot(protocol_request(
            "/v1/reservation/declare",
            "s1",
            "w1",
            serde_json::json!({
                "purpose": "Test requested work.",
                "files_planned": ["target/out.txt"]
            }),
        ))
        .await
        .expect("reservation declaration should complete");
    assert_eq!(declare.status(), StatusCode::OK);

    let response = app
        .oneshot(protocol_request(
            "/v1/authorize",
            "s1",
            "w1",
            serde_json::json!({
                "action": "write_file",
                "path": "target/out.txt"
            }),
        ))
        .await
        .expect("authorize should complete");
    assert_eq!(response.status(), StatusCode::OK);

    let body = to_bytes(response.into_body(), 1024)
        .await
        .expect("body should read");
    let json: serde_json::Value = serde_json::from_slice(&body).expect("body should be json");
    assert_eq!(json["decision"], "deny");
    assert_eq!(json["reason_code"], "active_claim_conflict");
}

#[tokio::test]
async fn write_file_action_denies_when_ancestor_directory_has_other_session_reservation() {
    let store = Store::open_in_memory().expect("store should open");
    store
        .enqueue_waiter(
            "s2",
            "w1",
            "target",
            "write_directory",
            "Queue requested directory write after blocker clears.",
            Some("s3"),
        )
        .expect("directory waiter should enqueue");
    store
        .promote_next_waiter("w1", "target")
        .expect("directory waiter should promote");
    let app = build_router(ServerConfig::with_store("secret-token", store));

    let declare = app
        .clone()
        .oneshot(protocol_request(
            "/v1/reservation/declare",
            "s1",
            "w1",
            serde_json::json!({
                "purpose": "Test requested work.",
                "files_planned": ["target/out.txt"]
            }),
        ))
        .await
        .expect("reservation declaration should complete");
    assert_eq!(declare.status(), StatusCode::OK);

    let response = app
        .oneshot(protocol_request(
            "/v1/authorize",
            "s1",
            "w1",
            serde_json::json!({
                "action": "write_file",
                "path": "target/out.txt"
            }),
        ))
        .await
        .expect("authorize should complete");
    assert_eq!(response.status(), StatusCode::OK);

    let body = to_bytes(response.into_body(), 2048)
        .await
        .expect("body should read");
    let json: serde_json::Value = serde_json::from_slice(&body).expect("body should be json");
    assert_eq!(json["decision"], "deny");
    assert_eq!(json["reason_code"], "reservation_conflict");
    assert_eq!(json["reservation"]["agent_id"], "s2");
    assert_eq!(json["reservation"]["relative_path"], "target");
}

#[tokio::test]
async fn write_file_action_requires_claim_when_ancestor_directory_reservation_is_claimable() {
    let store = Store::open_in_memory().expect("store should open");
    store
        .enqueue_waiter(
            "s2",
            "w1",
            "target",
            "write_directory",
            "Queue requested directory write after blocker clears.",
            Some("s3"),
        )
        .expect("directory waiter should enqueue");
    let wait = store
        .promote_next_waiter("w1", "target")
        .expect("directory waiter should promote")
        .expect("reservation should be created");
    let app = build_router(ServerConfig::with_store("secret-token", store));

    let declare = app
        .clone()
        .oneshot(protocol_request(
            "/v1/reservation/declare",
            "s2",
            "w1",
            serde_json::json!({
                "purpose": "Test requested work.",
                "files_planned": ["target/out.txt"]
            }),
        ))
        .await
        .expect("reservation declaration should complete");
    assert_eq!(declare.status(), StatusCode::OK);

    let response = app
        .oneshot(protocol_request(
            "/v1/authorize",
            "s2",
            "w1",
            serde_json::json!({
                "action": "write_file",
                "path": "target/out.txt"
            }),
        ))
        .await
        .expect("authorize should complete");
    assert_eq!(response.status(), StatusCode::OK);

    let body = to_bytes(response.into_body(), 2048)
        .await
        .expect("body should read");
    let json: serde_json::Value = serde_json::from_slice(&body).expect("body should be json");
    assert_eq!(json["decision"], "deny");
    assert_eq!(json["reason_code"], "reservation_claim_required");
    assert_eq!(json["reservation"]["wait_id"], wait.wait_id);
    assert_eq!(json["reservation"]["relative_path"], "target");
}

#[tokio::test]
async fn queued_write_directory_conflict_reserves_waiter_after_child_lease_release() {
    let store = Store::open_in_memory().expect("store should open");
    let app = build_router(ServerConfig::with_store("secret-token", store));

    ensure_test_reservation_via_http(&app, "s2", "w1", "target/out.txt").await;
    let claim = app
        .clone()
        .oneshot(json_request(
            "/v1/claim/acquire",
            serde_json::json!({
                "agent_id": "s2",
                "workspace_id": "w1",
                "path": "target/out.txt"
            }),
        ))
        .await
        .expect("claim acquire should complete");
    assert_eq!(claim.status(), StatusCode::OK);

    let declare = app
        .clone()
        .oneshot(protocol_request(
            "/v1/reservation/declare",
            "s1",
            "w1",
            serde_json::json!({
                "purpose": "Test requested work.",
                "files_planned": ["target/"]
            }),
        ))
        .await
        .expect("reservation declaration should complete");
    assert_eq!(declare.status(), StatusCode::OK);

    let queued = app
        .clone()
        .oneshot(protocol_request(
            "/v1/authorize",
            "s1",
            "w1",
            serde_json::json!({
                "action": "write_directory",
                "path": "target/",
                "queue_on_conflict": true,
                "purpose": "Queue requested directory write after blocker clears."
            }),
        ))
        .await
        .expect("authorize should complete");
    assert_eq!(queued.status(), StatusCode::OK);
    let body = to_bytes(queued.into_body(), 2048)
        .await
        .expect("body should read");
    let json: serde_json::Value = serde_json::from_slice(&body).expect("body should be json");
    assert_eq!(json["decision"], "deny");
    assert_eq!(json["reason_code"], "active_claim_conflict");
    assert_eq!(json["wait"]["status"], "queued");

    let wait_id = json["wait"]["wait_id"]
        .as_str()
        .expect("wait id should be present")
        .to_string();
    let release = app
        .clone()
        .oneshot(json_request(
            "/v1/claim/release",
            serde_json::json!({
                "agent_id": "s2",
                "workspace_id": "w1",
                "path": "target/out.txt"
            }),
        ))
        .await
        .expect("claim release should complete");
    assert_eq!(release.status(), StatusCode::OK);

    let resume = app
        .clone()
        .oneshot(json_request(
            "/v1/resume/next",
            serde_json::json!({
                "agent_id": "s1",
                "workspace_id": "w1"
            }),
        ))
        .await
        .expect("resume next should complete");
    assert_eq!(resume.status(), StatusCode::OK);
    let body = to_bytes(resume.into_body(), 2048)
        .await
        .expect("body should read");
    let json: serde_json::Value = serde_json::from_slice(&body).expect("body should be json");
    assert_eq!(json["resume_available"], true);
    assert_eq!(json["reservation"]["agent_id"], "s1");
    assert_eq!(json["reservation"]["relative_path"], "target");
    assert_eq!(json["reservation"]["action"], "write_directory");

    let claim = app
        .clone()
        .oneshot(protocol_request(
            "/v1/reservation/claim",
            "s1",
            "w1",
            serde_json::json!({
                "wait_id": wait_id
            }),
        ))
        .await
        .expect("reservation claim should complete");
    assert_eq!(claim.status(), StatusCode::OK);

    let allowed = app
        .oneshot(protocol_request(
            "/v1/authorize",
            "s1",
            "w1",
            serde_json::json!({
                "action": "write_directory",
                "path": "target/"
            }),
        ))
        .await
        .expect("authorize should complete");
    assert_eq!(allowed.status(), StatusCode::OK);
    let json = response_json(allowed, 2048).await;
    assert_eq!(json["decision"], "allow");
    assert_eq!(json["reason_code"], "authorized");
}

#[tokio::test]
async fn queued_child_file_conflict_reserves_waiter_after_directory_lease_release() {
    let store = Store::open_in_memory().expect("store should open");
    let app = build_router(ServerConfig::with_store("secret-token", store));

    ensure_test_reservation_via_http(&app, "s2", "w1", "target/").await;
    let claim = app
        .clone()
        .oneshot(json_request(
            "/v1/claim/acquire",
            serde_json::json!({
                "agent_id": "s2",
                "workspace_id": "w1",
                "path": "target/"
            }),
        ))
        .await
        .expect("claim acquire should complete");
    assert_eq!(claim.status(), StatusCode::OK);

    let declare = app
        .clone()
        .oneshot(protocol_request(
            "/v1/reservation/declare",
            "s1",
            "w1",
            serde_json::json!({
                "purpose": "Test requested work.",
                "files_planned": ["target/out.txt"]
            }),
        ))
        .await
        .expect("reservation declaration should complete");
    assert_eq!(declare.status(), StatusCode::OK);

    let queued = app
        .clone()
        .oneshot(protocol_request(
            "/v1/authorize",
            "s1",
            "w1",
            serde_json::json!({
                "action": "write_file",
                "path": "target/out.txt",
                "queue_on_conflict": true,
                "purpose": "Queue requested file write after blocker clears."
            }),
        ))
        .await
        .expect("authorize should complete");
    assert_eq!(queued.status(), StatusCode::OK);
    let body = to_bytes(queued.into_body(), 2048)
        .await
        .expect("body should read");
    let json: serde_json::Value = serde_json::from_slice(&body).expect("body should be json");
    assert_eq!(json["decision"], "deny");
    assert_eq!(json["reason_code"], "active_claim_conflict");
    assert_eq!(json["wait"]["status"], "queued");

    let release = app
        .clone()
        .oneshot(json_request(
            "/v1/claim/release",
            serde_json::json!({
                "agent_id": "s2",
                "workspace_id": "w1",
                "path": "target/"
            }),
        ))
        .await
        .expect("claim release should complete");
    assert_eq!(release.status(), StatusCode::OK);

    let resume = app
        .oneshot(json_request(
            "/v1/resume/next",
            serde_json::json!({
                "agent_id": "s1",
                "workspace_id": "w1"
            }),
        ))
        .await
        .expect("resume next should complete");
    assert_eq!(resume.status(), StatusCode::OK);
    let body = to_bytes(resume.into_body(), 2048)
        .await
        .expect("body should read");
    let json: serde_json::Value = serde_json::from_slice(&body).expect("body should be json");
    assert_eq!(json["resume_available"], true);
    assert_eq!(json["reservation"]["agent_id"], "s1");
    assert_eq!(json["reservation"]["relative_path"], "target/out.txt");
    assert_eq!(json["reservation"]["action"], "write_file");
}

#[tokio::test]
async fn current_returns_materialized_state_summary() {
    let app = build_router(ServerConfig::new("secret-token"));

    let declare = app
        .clone()
        .oneshot(protocol_request(
            "/v1/reservation/declare",
            "s1",
            "w1",
            serde_json::json!({
                "purpose": "Test requested work.",
                "files_planned": ["src/auth.ts"]
            }),
        ))
        .await
        .expect("reservation declaration should complete");
    assert_eq!(declare.status(), StatusCode::OK);

    let response = app
        .oneshot(authorized_get("/v1/current"))
        .await
        .expect("current request should complete");
    assert_eq!(response.status(), StatusCode::OK);

    let body = to_bytes(response.into_body(), 1024)
        .await
        .expect("body should read");
    let json: serde_json::Value = serde_json::from_slice(&body).expect("body should be json");
    assert_eq!(json["status"], "ok");
    assert_eq!(json["current"]["active_reservation_count"], 1);
    assert_eq!(json["current"]["event_count"], 1);
    assert_eq!(json["items"][0]["kind"], "reservation");
    assert_eq!(json["items"][0]["resource"], "src/auth.ts");
    assert_eq!(json["items"][0]["purpose"], "Test requested work.");
}

#[tokio::test]
async fn events_returns_recent_audit_events() {
    let store = Store::open_in_memory().expect("store should open");
    for index in 0..101 {
        store
            .append(
                Event::agent_registered(format!("old-session-{index}"), "w1")
                    .with_event_id(format!("old-event-{index}")),
            )
            .expect("old event should append");
    }
    store
        .append(Event::agent_registered("new-session", "w1").with_event_id("new-event"))
        .expect("new event should append");
    let app = build_router(ServerConfig::with_store("secret-token", store));

    let response = app
        .oneshot(authorized_get("/v1/events"))
        .await
        .expect("events request should complete");
    assert_eq!(response.status(), StatusCode::OK);

    let body = to_bytes(response.into_body(), 1_048_576)
        .await
        .expect("body should read");
    let json: serde_json::Value = serde_json::from_slice(&body).expect("body should be json");
    assert_eq!(json["status"], "ok");
    let event_ids = json["events"]
        .as_array()
        .expect("events should be an array")
        .iter()
        .map(|event| event["event_id"].as_str().unwrap_or_default())
        .collect::<Vec<_>>();
    assert_eq!(event_ids.len(), 100);
    assert_eq!(event_ids[0], "new-event");
    assert!(!event_ids.contains(&"old-event-0"));
}

#[tokio::test]
async fn lease_acquire_records_audit_event() {
    let app = build_router(ServerConfig::new("secret-token"));

    let declare = app
        .clone()
        .oneshot(protocol_request(
            "/v1/reservation/declare",
            "s1",
            "w1",
            serde_json::json!({
                "purpose": "Edit auth.",
                "files_planned": ["src/auth.ts"]
            }),
        ))
        .await
        .expect("reservation declaration should complete");
    assert_eq!(declare.status(), StatusCode::OK);

    let acquire = app
        .clone()
        .oneshot(json_request(
            "/v1/claim/acquire",
            serde_json::json!({
                "agent_id": "s1",
                "workspace_id": "w1",
                "path": "src/auth.ts"
            }),
        ))
        .await
        .expect("claim acquire should complete");
    assert_eq!(acquire.status(), StatusCode::OK);

    let response = app
        .oneshot(authorized_get("/v1/events"))
        .await
        .expect("events request should complete");
    assert_eq!(response.status(), StatusCode::OK);

    let json = response_json(response, 4096).await;
    let events = json["events"]
        .as_array()
        .expect("events should be an array");
    assert!(
        events.iter().any(|event| {
            event["event_type"] == "ClaimAcquired"
                && event["agent_id"] == "s1"
                && event["workspace_id"] == "w1"
        }),
        "claim acquisition should be present in audit events: {events:?}"
    );
}

#[tokio::test]
async fn lease_acquire_rolls_back_lease_when_audit_event_append_fails() {
    let temp = tempfile::tempdir().expect("temp dir should create");
    let temp_root = temp.path();
    let db_path = temp_root.join(".stateful_core").join("state.db");
    let store = Store::open(&db_path).expect("file store should open");
    let app = build_router(ServerConfig::with_store("secret-token", store));

    let declare = app
        .clone()
        .oneshot(protocol_request(
            "/v1/reservation/declare",
            "s1",
            "w1",
            serde_json::json!({
                "purpose": "Edit auth.",
                "files_planned": ["src/auth.ts"]
            }),
        ))
        .await
        .expect("reservation declaration should complete");
    assert_eq!(declare.status(), StatusCode::OK);

    let trigger_conn =
        rusqlite::Connection::open(&db_path).expect("trigger connection should open");
    trigger_conn
        .execute_batch(
            "CREATE TRIGGER fail_claim_acquired_event
             BEFORE INSERT ON events
             WHEN NEW.event_type = 'ClaimAcquired'
             BEGIN
                 SELECT RAISE(ABORT, 'simulated claim audit event append failure');
             END;",
        )
        .expect("failure trigger should install");

    let acquire = app
        .oneshot(json_request(
            "/v1/claim/acquire",
            serde_json::json!({
                "agent_id": "s1",
                "workspace_id": "w1",
                "path": "src/auth.ts"
            }),
        ))
        .await
        .expect("claim acquire should complete");
    assert_eq!(acquire.status(), StatusCode::INTERNAL_SERVER_ERROR);
    let json = response_json(acquire, 2048).await;
    assert!(
        json["message"]
            .as_str()
            .expect("error should be string")
            .contains("simulated claim audit event append failure")
    );

    let active_claim_count: u64 = trigger_conn
        .query_row(
            "SELECT COUNT(*) FROM claims
             WHERE workspace_id = 'w1'
               AND agent_id = 's1'
               AND relative_path = 'src/auth.ts'
               AND status = 'active'",
            [],
            |row| row.get::<_, u64>(0),
        )
        .expect("active claim count should load");
    assert_eq!(active_claim_count, 0);

    drop(trigger_conn);
}

#[tokio::test]
async fn reservation_request_records_audit_event() {
    let app = build_router(ServerConfig::new("secret-token"));

    let request = app
        .clone()
        .oneshot(protocol_request(
            "/v1/reservation/request",
            "s1",
            "w1",
            serde_json::json!({
                "request_id": "request-1",
                "action": "write_file",
                "path": "src/auth.ts",
                "purpose": "Reserve auth file before writing."
            }),
        ))
        .await
        .expect("reservation request should complete");
    assert_eq!(request.status(), StatusCode::OK);

    let response = app
        .oneshot(authorized_get("/v1/events"))
        .await
        .expect("events request should complete");
    assert_eq!(response.status(), StatusCode::OK);

    let json = response_json(response, 4096).await;
    let events = json["events"]
        .as_array()
        .expect("events should be an array");
    assert!(
        events.iter().any(|event| {
            event["event_type"] == "ReservationRequested"
                && event["agent_id"] == "s1"
                && event["workspace_id"] == "w1"
                && event["payload"]["request_id"] == "request-1"
                && event["payload"]["relative_path"] == "src/auth.ts"
                && event["payload"]["request_state"] == "reserved"
        }),
        "reservation request should be present in audit events: {events:?}"
    );
}

#[tokio::test]
async fn reservation_request_rolls_back_waiter_when_audit_event_append_fails() {
    let temp = tempfile::tempdir().expect("temp dir should create");
    let temp_root = temp.path();
    let db_path = temp_root.join(".stateful_core").join("state.db");
    let store = Store::open(&db_path).expect("file store should open");
    let app = build_router(ServerConfig::with_store("secret-token", store));

    let trigger_conn =
        rusqlite::Connection::open(&db_path).expect("trigger connection should open");
    trigger_conn
        .execute_batch(
            "CREATE TRIGGER fail_reservation_requested_event
             BEFORE INSERT ON events
             WHEN NEW.event_type = 'ReservationRequested'
             BEGIN
                 SELECT RAISE(ABORT, 'simulated reservation request audit event append failure');
             END;",
        )
        .expect("failure trigger should install");

    let request = app
        .oneshot(protocol_request(
            "/v1/reservation/request",
            "s1",
            "w1",
            serde_json::json!({
                "request_id": "request-1",
                "action": "write_file",
                "path": "src/auth.ts",
                "purpose": "Reserve auth file before writing."
            }),
        ))
        .await
        .expect("reservation request should complete");
    assert_eq!(request.status(), StatusCode::INTERNAL_SERVER_ERROR);
    let json = response_json(request, 2048).await;
    assert!(
        json["message"]
            .as_str()
            .expect("message should be string")
            .contains("simulated reservation request audit event append failure")
    );

    let waiter_count: u64 = trigger_conn
        .query_row(
            "SELECT COUNT(*) FROM wait_queue WHERE request_id = 'request-1'",
            [],
            |row| row.get::<_, u64>(0),
        )
        .expect("waiter count should load");
    assert_eq!(waiter_count, 0);

    drop(trigger_conn);
}

#[tokio::test]
async fn authorize_denial_records_audit_event() {
    let app = build_router(ServerConfig::new("secret-token"));

    let declare = app
        .clone()
        .oneshot(protocol_request(
            "/v1/reservation/declare",
            "s1",
            "w1",
            serde_json::json!({
                "purpose": "Test requested work.",
                "files_planned": ["src/auth.ts"]
            }),
        ))
        .await
        .expect("declare should complete");
    assert_eq!(declare.status(), StatusCode::OK);

    let response = app
        .clone()
        .oneshot(protocol_request(
            "/v1/authorize",
            "s1",
            "w1",
            serde_json::json!({
                "action": "write_file",
                "path": "src/other.ts"
            }),
        ))
        .await
        .expect("authorize should complete");
    assert_eq!(response.status(), StatusCode::OK);
    let json = response_json(response, 2048).await;
    assert_eq!(json["decision"], "deny");
    assert_eq!(json["reason_code"], "scope_mismatch");

    let response = app
        .oneshot(authorized_get("/v1/events"))
        .await
        .expect("events request should complete");
    assert_eq!(response.status(), StatusCode::OK);

    let json = response_json(response, 4096).await;
    let events = json["events"]
        .as_array()
        .expect("events should be an array");
    assert!(
        events.iter().any(|event| {
            event["event_type"] == "AuthorizationDenied"
                && event["agent_id"] == "s1"
                && event["workspace_id"] == "w1"
                && event["payload"]["reason_code"] == "scope_mismatch"
                && event["payload"]["action"] == "write_file"
                && event["payload"]["path"] == "src/other.ts"
        }),
        "authorization denial should be present in audit events: {events:?}"
    );
}

#[tokio::test]
async fn authorize_denial_audit_event_includes_queued_wait_details() {
    let app = build_router(ServerConfig::new("secret-token"));

    ensure_test_reservation_via_http(&app, "s1", "w1", "src/auth.ts").await;
    let claim = app
        .clone()
        .oneshot(json_request(
            "/v1/claim/acquire",
            serde_json::json!({
                "agent_id": "s1",
                "workspace_id": "w1",
                "path": "src/auth.ts"
            }),
        ))
        .await
        .expect("claim acquire should complete");
    assert_eq!(claim.status(), StatusCode::OK);

    ensure_test_reservation_via_http(&app, "s2", "w1", "src/auth.ts").await;
    let queued = app
        .clone()
        .oneshot(protocol_request(
            "/v1/authorize",
            "s2",
            "w1",
            serde_json::json!({
                "action": "write_file",
                "path": "src/auth.ts",
                "queue_on_conflict": true,
                "purpose": "Queue requested write after blocker clears."
            }),
        ))
        .await
        .expect("authorize should complete");
    assert_eq!(queued.status(), StatusCode::OK);
    let json = response_json(queued, 4096).await;
    assert_eq!(json["decision"], "deny");
    assert_eq!(json["reason_code"], "active_claim_conflict");
    let wait_id = json["wait"]["wait_id"]
        .as_str()
        .expect("wait id should be present")
        .to_string();

    let response = app
        .oneshot(authorized_get("/v1/events"))
        .await
        .expect("events request should complete");
    assert_eq!(response.status(), StatusCode::OK);

    let json = response_json(response, 8192).await;
    let events = json["events"]
        .as_array()
        .expect("events should be an array");
    assert!(
        events.iter().any(|event| {
            event["event_type"] == "AuthorizationDenied"
                && event["agent_id"] == "s2"
                && event["workspace_id"] == "w1"
                && event["payload"]["reason_code"] == "active_claim_conflict"
                && event["payload"]["wait"]["wait_id"] == wait_id
                && event["payload"]["wait"]["status"] == "queued"
                && event["payload"]["wait"]["queue_position"] == 1
                && event["payload"]["wait"]["blocking_agent_id"] == "s1"
        }),
        "authorization denial audit event should include queued wait details: {events:?}"
    );
}

#[tokio::test]
async fn authorize_queue_rolls_back_waiter_when_audit_event_append_fails() {
    let temp = tempfile::tempdir().expect("temp dir should create");
    let temp_root = temp.path();
    let db_path = temp_root.join(".stateful_core").join("state.db");
    let store = Store::open(&db_path).expect("file store should open");
    let app = build_router(ServerConfig::with_store("secret-token", store));

    ensure_test_reservation_via_http(&app, "s1", "w1", "src/auth.ts").await;
    let claim = app
        .clone()
        .oneshot(json_request(
            "/v1/claim/acquire",
            serde_json::json!({
                "agent_id": "s1",
                "workspace_id": "w1",
                "path": "src/auth.ts"
            }),
        ))
        .await
        .expect("claim acquire should complete");
    assert_eq!(claim.status(), StatusCode::OK);

    let declare = app
        .clone()
        .oneshot(protocol_request(
            "/v1/reservation/declare",
            "s2",
            "w1",
            serde_json::json!({
                "purpose": "Edit auth after blocker clears.",
                "files_planned": ["src/auth.ts"]
            }),
        ))
        .await
        .expect("reservation declaration should complete");
    assert_eq!(declare.status(), StatusCode::OK);

    let trigger_conn =
        rusqlite::Connection::open(&db_path).expect("trigger connection should open");
    trigger_conn
        .execute_batch(
            "CREATE TRIGGER fail_authorization_denied_event
             BEFORE INSERT ON events
             WHEN NEW.event_type = 'AuthorizationDenied'
             BEGIN
                 SELECT RAISE(ABORT, 'simulated authorization audit event append failure');
             END;",
        )
        .expect("failure trigger should install");

    let response = app
        .oneshot(protocol_request(
            "/v1/authorize",
            "s2",
            "w1",
            serde_json::json!({
                "action": "write_file",
                "path": "src/auth.ts",
                "queue_on_conflict": true,
                "purpose": "Queue auth edit after blocker clears."
            }),
        ))
        .await
        .expect("authorize should complete");
    assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
    let json = response_json(response, 2048).await;
    assert!(
        json["message"]
            .as_str()
            .expect("message should be string")
            .contains("simulated authorization audit event append failure")
    );

    let waiter_count: u64 = trigger_conn
        .query_row(
            "SELECT COUNT(*) FROM wait_queue
             WHERE agent_id = 's2'
               AND workspace_id = 'w1'
               AND relative_path = 'src/auth.ts'",
            [],
            |row| row.get::<_, u64>(0),
        )
        .expect("waiter count should load");
    assert_eq!(waiter_count, 0);

    drop(trigger_conn);
}

#[tokio::test]
async fn lifecycle_mutations_record_audit_events() {
    let app = build_router(ServerConfig::new("secret-token"));

    ensure_test_reservation_via_http(&app, "s1", "w1", "src/release.ts").await;
    let acquire_release_path = app
        .clone()
        .oneshot(json_request(
            "/v1/claim/acquire",
            serde_json::json!({
                "agent_id": "s1",
                "workspace_id": "w1",
                "path": "src/release.ts"
            }),
        ))
        .await
        .expect("claim acquire should complete");
    assert_eq!(acquire_release_path.status(), StatusCode::OK);

    let release = app
        .clone()
        .oneshot(json_request(
            "/v1/claim/release",
            serde_json::json!({
                "agent_id": "s1",
                "workspace_id": "w1",
                "path": "src/release.ts"
            }),
        ))
        .await
        .expect("claim release should complete");
    assert_eq!(release.status(), StatusCode::OK);

    ensure_test_reservation_via_http(&app, "blocker", "w1", "src/claim.ts").await;
    let blocker_lease = app
        .clone()
        .oneshot(json_request(
            "/v1/claim/acquire",
            serde_json::json!({
                "agent_id": "blocker",
                "workspace_id": "w1",
                "path": "src/claim.ts"
            }),
        ))
        .await
        .expect("blocker claim should complete");
    assert_eq!(blocker_lease.status(), StatusCode::OK);

    let claim_request = app
        .clone()
        .oneshot(protocol_request(
            "/v1/reservation/request",
            "claimer",
            "w1",
            serde_json::json!({
                "request_id": "request-claim",
                "action": "write_file",
                "path": "src/claim.ts",
                "purpose": "Claim queued work."
            }),
        ))
        .await
        .expect("reservation request should complete");
    assert_eq!(claim_request.status(), StatusCode::OK);
    let claim_request_json = response_json(claim_request, 4096).await;
    let claim_wait_id = claim_request_json["wait"]["wait_id"]
        .as_str()
        .expect("claim wait id should be present")
        .to_string();

    let release_blocker = app
        .clone()
        .oneshot(json_request(
            "/v1/claim/release",
            serde_json::json!({
                "agent_id": "blocker",
                "workspace_id": "w1",
                "path": "src/claim.ts"
            }),
        ))
        .await
        .expect("blocker release should complete");
    assert_eq!(release_blocker.status(), StatusCode::OK);

    let claim = app
        .clone()
        .oneshot(protocol_request(
            "/v1/reservation/claim",
            "claimer",
            "w1",
            serde_json::json!({
                "wait_id": claim_wait_id
            }),
        ))
        .await
        .expect("reservation claim should complete");
    assert_eq!(claim.status(), StatusCode::OK);

    ensure_test_reservation_via_http(&app, "cancel-blocker", "w1", "src/cancel.ts").await;
    let cancel_blocker_lease = app
        .clone()
        .oneshot(json_request(
            "/v1/claim/acquire",
            serde_json::json!({
                "agent_id": "cancel-blocker",
                "workspace_id": "w1",
                "path": "src/cancel.ts"
            }),
        ))
        .await
        .expect("cancel blocker claim should complete");
    assert_eq!(cancel_blocker_lease.status(), StatusCode::OK);

    let cancel_request = app
        .clone()
        .oneshot(protocol_request(
            "/v1/reservation/request",
            "cancelable",
            "w1",
            serde_json::json!({
                "request_id": "request-cancel",
                "action": "write_file",
                "path": "src/cancel.ts",
                "purpose": "Cancel queued work."
            }),
        ))
        .await
        .expect("cancel reservation request should complete");
    assert_eq!(cancel_request.status(), StatusCode::OK);

    let release_cancel_blocker = app
        .clone()
        .oneshot(json_request(
            "/v1/claim/release",
            serde_json::json!({
                "agent_id": "cancel-blocker",
                "workspace_id": "w1",
                "path": "src/cancel.ts"
            }),
        ))
        .await
        .expect("cancel blocker release should complete");
    assert_eq!(release_cancel_blocker.status(), StatusCode::OK);

    let cancel = app
        .clone()
        .oneshot(protocol_request(
            "/v1/reservation/cancel",
            "cancelable",
            "w1",
            serde_json::json!({
                "request_id": "request-cancel"
            }),
        ))
        .await
        .expect("reservation cancel should complete");
    assert_eq!(cancel.status(), StatusCode::OK);

    ensure_test_reservation_via_http(&app, "finalizer", "w1", "src/finalize.ts").await;
    let finalize_lease = app
        .clone()
        .oneshot(json_request(
            "/v1/claim/acquire",
            serde_json::json!({
                "agent_id": "finalizer",
                "workspace_id": "w1",
                "path": "src/finalize.ts"
            }),
        ))
        .await
        .expect("finalize claim should complete");
    assert_eq!(finalize_lease.status(), StatusCode::OK);

    let finalize = app
        .clone()
        .oneshot(json_request(
            "/v1/activity/finalize",
            serde_json::json!({
                "agent_id": "finalizer",
                "workspace_id": "w1"
            }),
        ))
        .await
        .expect("activity finalize should complete");
    assert_eq!(finalize.status(), StatusCode::OK);

    let response = app
        .oneshot(authorized_get("/v1/events"))
        .await
        .expect("events request should complete");
    assert_eq!(response.status(), StatusCode::OK);
    let json = response_json(response, 16384).await;
    let events = json["events"]
        .as_array()
        .expect("events should be an array");

    assert!(
        events.iter().any(|event| {
            event["event_type"] == "ClaimReleased"
                && event["agent_id"] == "s1"
                && event["workspace_id"] == "w1"
                && event["payload"]["path"] == "src/release.ts"
        }),
        "claim release should be present in audit events: {events:?}"
    );
    assert!(
        events.iter().any(|event| {
            event["event_type"] == "ReservationClaimed"
                && event["agent_id"] == "claimer"
                && event["workspace_id"] == "w1"
                && event["payload"]["wait_id"] == claim_wait_id
                && event["payload"]["relative_path"] == "src/claim.ts"
        }),
        "reservation claim should be present in audit events: {events:?}"
    );
    assert!(
        events.iter().any(|event| {
            event["event_type"] == "ReservationCanceled"
                && event["agent_id"] == "cancelable"
                && event["workspace_id"] == "w1"
                && event["payload"]["request_id"] == "request-cancel"
                && event["payload"]["relative_path"] == "src/cancel.ts"
        }),
        "reservation cancel should be present in audit events: {events:?}"
    );
    assert!(
        events.iter().any(|event| {
            event["event_type"] == "ActivityFinalized"
                && event["agent_id"] == "finalizer"
                && event["workspace_id"] == "w1"
                && event["payload"]["released_claims"] == 1
                && event["payload"]["completed_reservations"] == 1
        }),
        "activity finalization should be present in audit events: {events:?}"
    );
}

#[tokio::test]
async fn authorize_uses_supplied_sqlite_store() {
    let temp = tempfile::tempdir().expect("temp dir should create");
    let temp_root = temp.path();
    let db_path = temp_root.join(".stateful_core").join("state.db");
    let store = Store::open(&db_path).expect("file store should open");
    store
        .append(Event::reservation_declared(
            "s1",
            "w1",
            "Test supplied sqlite store authorization.",
            ["src/auth.ts"],
        ))
        .expect("reservation should append");
    acquire_test_lease(&store, "s1", "w1", "src/auth.ts");
    let app = build_router(ServerConfig::with_store("secret-token", store));

    let response = app
        .oneshot(protocol_request(
            "/v1/authorize",
            "s1",
            "w1",
            serde_json::json!({
                "action": "write_file",
                "path": "src/auth.ts"
            }),
        ))
        .await
        .expect("authorize should complete");

    let body = to_bytes(response.into_body(), 1024)
        .await
        .expect("body should read");
    let json: serde_json::Value = serde_json::from_slice(&body).expect("body should be json");
    assert_eq!(json["decision"], "allow");
}

#[tokio::test]
async fn serve_listener_expires_stale_reservations_without_request_activity() {
    let temp = tempfile::tempdir().expect("temp dir should create");
    let temp_root = temp.path();
    let db_path = temp_root.join(".stateful_core").join("state.db");
    let setup_store = Store::open(&db_path).expect("file store should open");
    let first = setup_store
        .enqueue_waiter(
            "s2",
            "w1",
            "src/auth.ts",
            "write_file",
            "Queue requested file write after blocker clears.",
            Some("s1"),
        )
        .expect("first waiter should enqueue");
    let second = setup_store
        .enqueue_waiter(
            "s3",
            "w1",
            "src/auth.ts",
            "write_file",
            "Queue requested file write after blocker clears.",
            Some("s1"),
        )
        .expect("second waiter should enqueue");
    setup_store
        .promote_next_waiter("w1", "src/auth.ts")
        .expect("first waiter should promote");
    drop(setup_store);

    let conn = rusqlite::Connection::open(&db_path).expect("db should reopen");
    conn.execute(
        "UPDATE wait_queue SET reservation_expires_at = '1970-01-01T00:00:00Z'
         WHERE wait_id = ?1",
        [&first.wait_id],
    )
    .expect("reservation should be made stale");
    drop(conn);

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("listener should bind");
    let store = Store::open(&db_path).expect("server store should open");
    let config = ServerConfig::with_store("secret-token", store)
        .with_maintenance_interval(Duration::from_millis(20));
    let server = tokio::spawn(async move { serve_listener(listener, config).await });

    tokio::time::sleep(Duration::from_millis(150)).await;

    let conn = rusqlite::Connection::open(&db_path).expect("db should reopen");
    let first_status: String = conn
        .query_row(
            "SELECT status FROM wait_queue WHERE wait_id = ?1",
            [&first.wait_id],
            |row| row.get(0),
        )
        .expect("first waiter status should load");
    let second_status: String = conn
        .query_row(
            "SELECT status FROM wait_queue WHERE wait_id = ?1",
            [&second.wait_id],
            |row| row.get(0),
        )
        .expect("second waiter status should load");

    server.abort();
    let _ = server.await;
    drop(conn);

    assert_eq!(first_status, "expired");
    assert_eq!(second_status, "reserved");
}

#[tokio::test]
async fn serve_listener_prunes_old_history_without_request_activity() {
    let temp = tempfile::tempdir().expect("temp dir should create");
    let temp_root = temp.path();
    let db_path = temp_root.join(".stateful_core").join("state.db");
    let setup_store = Store::open(&db_path).expect("file store should open");
    let mut old_event = Event::agent_registered("old-session", "w1").with_event_id("old-event");
    old_event.created_at = "1970-01-01T00:00:00Z".to_string();
    setup_store
        .append(old_event)
        .expect("old event should append");
    let recent_event =
        Event::agent_registered("recent-session", "w1").with_event_id("recent-event");
    setup_store
        .append(recent_event)
        .expect("recent event should append");
    drop(setup_store);

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("listener should bind");
    let store = Store::open(&db_path).expect("server store should open");
    let config = ServerConfig::with_store("secret-token", store)
        .with_maintenance_interval(Duration::from_millis(20));
    let server = tokio::spawn(async move { serve_listener(listener, config).await });

    tokio::time::sleep(Duration::from_millis(150)).await;

    let conn = rusqlite::Connection::open(&db_path).expect("db should reopen");
    let event_ids = {
        let mut statement = conn
            .prepare("SELECT event_id FROM events ORDER BY event_id ASC")
            .expect("events query should prepare");
        statement
            .query_map([], |row| row.get::<_, String>(0))
            .expect("events query should execute")
            .collect::<Result<Vec<_>, _>>()
            .expect("event ids should load")
    };

    server.abort();
    let _ = server.await;
    drop(conn);

    assert_eq!(event_ids, vec!["recent-event"]);
}

#[tokio::test]
async fn context_render_returns_empty_prompt_when_no_blocking_state_exists() {
    let app = build_router(ServerConfig::new("secret-token"));

    let response = app
        .oneshot(json_request(
            "/v1/context/render",
            serde_json::json!({
                "mode": "detailed",
                "resource": "src/auth.ts",
                "workspace_id": "w1"
            }),
        ))
        .await
        .expect("context render should complete");
    assert_eq!(response.status(), StatusCode::OK);

    let body = to_bytes(response.into_body(), 2048)
        .await
        .expect("body should read");
    let json: serde_json::Value = serde_json::from_slice(&body).expect("body should be json");
    assert_eq!(json["status"], "ok");
    assert_eq!(json["prompt_text"], "");
}

#[tokio::test]
async fn context_render_rejects_missing_workspace_id() {
    let app = build_router(ServerConfig::new("secret-token"));

    let response = app
        .oneshot(json_request(
            "/v1/context/render",
            serde_json::json!({
                "mode": "detailed",
                "resource": "src/auth.ts"
            }),
        ))
        .await
        .expect("context render should complete");
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);

    let json = response_json(response, 2048).await;
    assert_eq!(json["status"], "error");
    assert_eq!(json["message"], "workspace_id is required");
}

#[tokio::test]
async fn context_render_includes_live_current_state_purpose() {
    let app = build_router(ServerConfig::new("secret-token"));

    let declare = app
        .clone()
        .oneshot(protocol_request(
            "/v1/reservation/declare",
            "s1",
            "w1",
            serde_json::json!({
                "purpose": "Fix auth validation behavior.",
                "files_planned": ["src/auth.ts"]
            }),
        ))
        .await
        .expect("reservation declaration should complete");
    assert_eq!(declare.status(), StatusCode::OK);

    let response = app
        .oneshot(json_request(
            "/v1/context/render",
            serde_json::json!({
                "mode": "detailed",
                "resource": "src/auth.ts",
                "workspace_id": "w1"
            }),
        ))
        .await
        .expect("context render should complete");
    assert_eq!(response.status(), StatusCode::OK);

    let body = to_bytes(response.into_body(), 4096)
        .await
        .expect("body should read");
    let json: serde_json::Value = serde_json::from_slice(&body).expect("body should be json");
    assert_eq!(json["items"][0]["purpose"], "Fix auth validation behavior.");
    assert!(
        json["prompt_text"]
            .as_str()
            .unwrap_or_default()
            .contains("purpose: Fix auth validation behavior")
    );
}

#[tokio::test]
async fn context_render_treats_empty_resource_as_unfiltered() {
    let app = build_router(ServerConfig::new("secret-token"));

    let declare = app
        .clone()
        .oneshot(protocol_request(
            "/v1/reservation/declare",
            "s1",
            "w1",
            serde_json::json!({
                "purpose": "Fix auth validation behavior.",
                "files_planned": ["src/auth.ts"]
            }),
        ))
        .await
        .expect("reservation declaration should complete");
    assert_eq!(declare.status(), StatusCode::OK);

    let response = app
        .oneshot(json_request(
            "/v1/context/render",
            serde_json::json!({
                "mode": "detailed",
                "resource": "",
                "workspace_id": "w1"
            }),
        ))
        .await
        .expect("context render should complete");
    assert_eq!(response.status(), StatusCode::OK);

    let json = response_json(response, 4096).await;
    assert_eq!(json["items"][0]["resource"], "src/auth.ts");
    assert!(
        json["prompt_text"]
            .as_str()
            .unwrap_or_default()
            .contains("purpose: Fix auth validation behavior")
    );
}

#[tokio::test]
async fn context_render_lists_agent_context_lease_as_active_scope() {
    let store = Store::open_in_memory().expect("store should open");
    acquire_test_lease(&store, "s1", "w1", "src/auth.ts");
    acquire_test_lease(&store, "s2", "w1", "src/session.ts");
    let app = build_router(ServerConfig::with_store("secret-token", store));

    let response = app
        .oneshot(json_request(
            "/v1/context/render",
            serde_json::json!({
                "mode": "detailed",
                "workspace_id": "w1",
                "agent_id": "s1"
            }),
        ))
        .await
        .expect("context render should complete");
    assert_eq!(response.status(), StatusCode::OK);

    let json = response_json(response, 4096).await;
    let items = json["items"].as_array().expect("items should be an array");
    assert!(
        items.iter().any(|item| {
            item["kind"] == "claim"
                && item["agent_id"] == "s1"
                && item["severity"] == "info"
                && item["source_refs"]
                    .as_array()
                    .is_some_and(|refs| refs.iter().any(|value| value == "AgentContextScope"))
        }),
        "current agent claim should be reported as own active scope: {items:?}"
    );
    assert!(
        items
            .iter()
            .any(|item| item["kind"] == "claim" && item["agent_id"] == "s2"),
        "other agent claim should remain visible: {items:?}"
    );
    assert!(
        json["prompt_text"]
            .as_str()
            .unwrap_or_default()
            .contains("Your Active Scope"),
        "prompt should include active scope section: {}",
        json["prompt_text"]
    );
}

#[tokio::test]
async fn context_render_lists_agent_context_intent_as_active_scope() {
    let app = build_router(ServerConfig::new("secret-token"));

    let current_declare = app
        .clone()
        .oneshot(protocol_request(
            "/v1/reservation/declare",
            "s1",
            "w1",
            serde_json::json!({
                "purpose": "Current agent edits auth.",
                "files_planned": ["src/auth.ts"]
            }),
        ))
        .await
        .expect("current reservation declaration should complete");
    assert_eq!(current_declare.status(), StatusCode::OK);

    let other_declare = app
        .clone()
        .oneshot(protocol_request(
            "/v1/reservation/declare",
            "s2",
            "w1",
            serde_json::json!({
                "purpose": "Other agent edits session handling.",
                "files_planned": ["src/session.ts"]
            }),
        ))
        .await
        .expect("other reservation declaration should complete");
    assert_eq!(other_declare.status(), StatusCode::OK);

    let response = app
        .oneshot(json_request(
            "/v1/context/render",
            serde_json::json!({
                "mode": "detailed",
                "workspace_id": "w1",
                "agent_id": "s1"
            }),
        ))
        .await
        .expect("context render should complete");
    assert_eq!(response.status(), StatusCode::OK);

    let json = response_json(response, 4096).await;
    let items = json["items"].as_array().expect("items should be an array");
    assert!(
        items.iter().any(|item| {
            item["kind"] == "reservation"
                && item["agent_id"] == "s1"
                && item["severity"] == "info"
                && item["source_refs"]
                    .as_array()
                    .is_some_and(|refs| refs.iter().any(|value| value == "AgentContextScope"))
        }),
        "current agent reservation should be reported as own active scope: {items:?}"
    );
    assert!(
        items
            .iter()
            .any(|item| item["kind"] == "reservation" && item["agent_id"] == "s2"),
        "other agent reservation should remain visible: {items:?}"
    );
    assert!(
        json["prompt_text"]
            .as_str()
            .unwrap_or_default()
            .contains("Your Active Scope"),
        "prompt should include active scope section: {}",
        json["prompt_text"]
    );
}

#[tokio::test]
async fn context_render_filters_items_to_requested_workspace() {
    let app = build_router(ServerConfig::new("secret-token"));

    let declare_w1 = app
        .clone()
        .oneshot(protocol_request(
            "/v1/reservation/declare",
            "s1",
            "w1",
            serde_json::json!({
                "purpose": "Fix auth validation behavior.",
                "files_planned": ["src/auth.ts"]
            }),
        ))
        .await
        .expect("w1 reservation declaration should complete");
    assert_eq!(declare_w1.status(), StatusCode::OK);

    let declare_w2 = app
        .clone()
        .oneshot(protocol_request(
            "/v1/reservation/declare",
            "s2",
            "w2",
            serde_json::json!({
                "purpose": "Update public docs from a separate workspace.",
                "files_planned": ["src/auth.ts"]
            }),
        ))
        .await
        .expect("w2 reservation declaration should complete");
    assert_eq!(declare_w2.status(), StatusCode::OK);

    let response = app
        .oneshot(json_request(
            "/v1/context/render",
            serde_json::json!({
                "mode": "detailed",
                "resource": "src/auth.ts",
                "workspace_id": "w1"
            }),
        ))
        .await
        .expect("context render should complete");
    assert_eq!(response.status(), StatusCode::OK);

    let json = response_json(response, 4096).await;
    let items = json["items"].as_array().expect("items should be an array");
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["workspace_id"], "w1");
    assert_eq!(items[0]["purpose"], "Fix auth validation behavior.");
    assert_eq!(json["current"]["agent_count"], 0);
    assert_eq!(json["current"]["active_reservation_count"], 1);
    assert_eq!(json["current"]["event_count"], 1);
    let prompt_text = json["prompt_text"].as_str().unwrap_or_default();
    assert!(prompt_text.contains("purpose: Fix auth validation behavior"));
    assert!(!prompt_text.contains("Update public docs from a separate workspace"));
}

#[tokio::test]
async fn context_render_filters_items_to_requested_repo_identity() {
    let app = build_router(ServerConfig::new("secret-token"));

    let declare_repo_1 = app
        .clone()
        .oneshot(protocol_request(
            "/v1/reservation/declare",
            "s1",
            "shared",
            serde_json::json!({
                "purpose": "Fix stateful core LAN behavior.",
                "files_planned": ["crates/stateful-cli/src/lan.rs"]
            }),
        ))
        .await
        .expect("repo-1 reservation declaration should complete");
    assert_eq!(declare_repo_1.status(), StatusCode::OK);

    let mut repo_2_body = protocol_body(
        "s2",
        "shared",
        serde_json::json!({
            "purpose": "Investigate edge camera framedrops.",
            "files_planned": ["record/hw/vision_module.py"]
        }),
    );
    repo_2_body["workspace"]["root"] = serde_json::json!("/workspace/edge/core");
    repo_2_body["workspace"]["repo_id"] = serde_json::json!("repo-2");
    repo_2_body["workspace"]["worktree_id"] = serde_json::json!("worktree-2");
    repo_2_body["workspace"]["branch"] = serde_json::json!("frame-drops");

    let declare_repo_2 = app
        .clone()
        .oneshot(json_request("/v1/reservation/declare", repo_2_body))
        .await
        .expect("repo-2 reservation declaration should complete");
    assert_eq!(declare_repo_2.status(), StatusCode::OK);

    let response = app
        .oneshot(json_request(
            "/v1/context/render",
            serde_json::json!({
                "mode": "detailed",
                "workspace_id": "shared",
                "repo_id": "repo-1",
                "worktree_id": "worktree-1",
                "root": "/repo"
            }),
        ))
        .await
        .expect("context render should complete");
    assert_eq!(response.status(), StatusCode::OK);

    let json = response_json(response, 4096).await;
    let items = json["items"].as_array().expect("items should be an array");
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["purpose"], "Fix stateful core LAN behavior.");
    let prompt_text = json["prompt_text"].as_str().unwrap_or_default();
    assert!(prompt_text.contains("Fix stateful core LAN behavior"));
    assert!(!prompt_text.contains("Investigate edge camera framedrops"));
}

#[tokio::test]
async fn context_render_keeps_identity_filtered_queued_workflow_state_visible() {
    let app = build_router(ServerConfig::new("secret-token"));

    let request = app
        .clone()
        .oneshot(protocol_request(
            "/v1/reservation/request",
            "s1",
            "shared",
            serde_json::json!({
                "request_id": "request-1",
                "action": "write_file",
                "path": "src/auth.ts",
                "purpose": "Claim queued auth update."
            }),
        ))
        .await
        .expect("reservation request should complete");
    assert_eq!(request.status(), StatusCode::OK);
    let json = response_json(request, 4096).await;
    assert_eq!(json["request_state"], "reserved");
    let wait_id = json["reservation"]["wait_id"]
        .as_str()
        .expect("reservation should include wait_id")
        .to_string();

    let response = app
        .clone()
        .oneshot(json_request(
            "/v1/context/render",
            serde_json::json!({
                "mode": "detailed",
                "workspace_id": "shared",
                "repo_id": "repo-1",
                "worktree_id": "worktree-1",
                "root": "/repo"
            }),
        ))
        .await
        .expect("context render should complete");
    assert_eq!(response.status(), StatusCode::OK);
    let json = response_json(response, 4096).await;
    let items = json["items"].as_array().expect("items should be an array");
    assert!(items.iter().any(|item| {
        item["kind"] == "claimable_reservation" && item["purpose"] == "Claim queued auth update."
    }));

    let claim = app
        .clone()
        .oneshot(protocol_request(
            "/v1/reservation/claim",
            "s1",
            "shared",
            serde_json::json!({
                "wait_id": wait_id
            }),
        ))
        .await
        .expect("reservation claim should complete");
    assert_eq!(claim.status(), StatusCode::OK);

    let response = app
        .oneshot(json_request(
            "/v1/context/render",
            serde_json::json!({
                "mode": "detailed",
                "workspace_id": "shared",
                "repo_id": "repo-1",
                "worktree_id": "worktree-1",
                "root": "/repo"
            }),
        ))
        .await
        .expect("context render should complete");
    assert_eq!(response.status(), StatusCode::OK);
    let json = response_json(response, 8192).await;
    let items = json["items"].as_array().expect("items should be an array");
    assert!(items.iter().any(|item| {
        item["kind"] == "reservation" && item["purpose"] == "Claim queued auth update."
    }));
    assert!(
        items
            .iter()
            .any(|item| item["kind"] == "claim" && item["purpose"] == "Claim queued auth update.")
    );
}

#[tokio::test]
async fn reservation_request_retry_backfills_identity_for_filtered_context_render() {
    let app = build_router(ServerConfig::new("secret-token"));

    let mut missing_identity = protocol_body(
        "s1",
        "shared",
        serde_json::json!({
            "request_id": "request-backfill",
            "action": "write_file",
            "path": "src/auth.ts",
            "purpose": "Backfill queued auth identity."
        }),
    );
    missing_identity["workspace"]["repo_id"] = serde_json::json!("");
    missing_identity["workspace"]["worktree_id"] = serde_json::json!("");
    missing_identity["workspace"]["root"] = serde_json::json!("");
    missing_identity["workspace"]["branch"] = serde_json::json!("");

    let request = app
        .clone()
        .oneshot(json_request("/v1/reservation/request", missing_identity))
        .await
        .expect("initial reservation request should complete");
    assert_eq!(request.status(), StatusCode::OK);
    let json = response_json(request, 4096).await;
    assert_eq!(json["request_state"], "reserved");

    let filtered = app
        .clone()
        .oneshot(json_request(
            "/v1/context/render",
            serde_json::json!({
                "mode": "detailed",
                "workspace_id": "shared",
                "repo_id": "repo-1",
                "worktree_id": "worktree-1",
                "root": "/repo"
            }),
        ))
        .await
        .expect("context render should complete");
    assert_eq!(filtered.status(), StatusCode::OK);
    let json = response_json(filtered, 4096).await;
    let items = json["items"].as_array().expect("items should be an array");
    assert!(
        !items
            .iter()
            .any(|item| item["purpose"] == "Backfill queued auth identity.")
    );

    let retry = app
        .clone()
        .oneshot(protocol_request(
            "/v1/reservation/request",
            "s1",
            "shared",
            serde_json::json!({
                "request_id": "request-backfill",
                "action": "write_file",
                "path": "src/auth.ts",
                "purpose": "Retry keeps original purpose."
            }),
        ))
        .await
        .expect("retry reservation request should complete");
    assert_eq!(retry.status(), StatusCode::OK);
    let json = response_json(retry, 4096).await;
    assert_eq!(json["request_state"], "reserved");
    assert_eq!(
        json["reservation"]["purpose"],
        "Backfill queued auth identity."
    );

    let filtered = app
        .oneshot(json_request(
            "/v1/context/render",
            serde_json::json!({
                "mode": "detailed",
                "workspace_id": "shared",
                "repo_id": "repo-1",
                "worktree_id": "worktree-1",
                "root": "/repo"
            }),
        ))
        .await
        .expect("context render should complete");
    assert_eq!(filtered.status(), StatusCode::OK);
    let json = response_json(filtered, 4096).await;
    let items = json["items"].as_array().expect("items should be an array");
    assert!(items.iter().any(|item| {
        item["kind"] == "claimable_reservation"
            && item["purpose"] == "Backfill queued auth identity."
    }));
}

#[tokio::test]
async fn outbox_sync_accepts_idempotent_events() {
    let app = build_router(ServerConfig::new("secret-token"));

    for _ in 0..2 {
        let response = app
            .clone()
            .oneshot(json_request(
                "/v1/outbox/sync",
                serde_json::json!({
                    "outbox_id": "outbox-1",
                    "agent_id": "s1",
                    "workspace_id": "w1",
                    "sequence": 1,
                    "event_type": "HeartbeatObserved",
                    "payload": {"ok": true}
                }),
            ))
            .await
            .expect("outbox sync should complete");
        assert_eq!(response.status(), StatusCode::OK);
    }
}

#[tokio::test]
async fn outbox_sync_persists_full_event_payload() {
    let temp = tempfile::tempdir().expect("temp dir should create");
    let temp_root = temp.path();
    let db_path = temp_root.join(".stateful_core").join("state.db");
    let store = Store::open(&db_path).expect("file store should open");
    let app = build_router(ServerConfig::with_store("secret-token", store));

    let response = app
        .oneshot(json_request(
            "/v1/outbox/sync",
            serde_json::json!({
                "outbox_id": "outbox-full",
                "agent_id": "s1",
                "workspace_id": "w1",
                "sequence": 7,
                "event_type": "HeartbeatObserved",
                "payload": {"error": "server unavailable", "retry": true}
            }),
        ))
        .await
        .expect("outbox sync should complete");
    assert_eq!(response.status(), StatusCode::OK);

    let reopened = Store::open(&db_path).expect("file store should reopen");
    let stored = reopened
        .outbox_entry("outbox-full")
        .expect("outbox entry lookup should succeed")
        .expect("outbox entry should exist");
    assert_eq!(stored.workspace_id, "w1");
    assert_eq!(stored.event_type, "HeartbeatObserved");
    assert_eq!(
        stored.payload,
        serde_json::json!({"error": "server unavailable", "retry": true})
    );
}

#[tokio::test]
async fn claim_refresh_observation_error_uses_status_error_envelope() {
    let app = build_router(ServerConfig::new("secret-token"));

    let response = app
        .oneshot(json_request(
            "/v1/claim/refresh-observation",
            serde_json::json!({
                "agent_id": "s1",
                "workspace_id": "w1",
                "path": "src/auth.ts"
            }),
        ))
        .await
        .expect("claim refresh should complete");

    let status = response.status();
    assert!(
        status.is_client_error() || status.is_server_error(),
        "missing root should map to an error status, got {status}"
    );
    let json = response_json(response, 2048).await;
    assert_eq!(
        json["status"], "error",
        "unified envelope should mark errors with status=error: {json}"
    );
    assert!(
        json["message"]
            .as_str()
            .is_some_and(|message| !message.is_empty()),
        "unified envelope should carry a non-empty message: {json}"
    );
    assert!(
        json.get("error").is_none(),
        "legacy bare top-level error field should be gone: {json}"
    );
}

#[tokio::test]
async fn activity_finalize_store_failure_uses_status_error_envelope() {
    let temp = tempfile::tempdir().expect("temp dir should create");
    let temp_root = temp.path();
    let db_path = temp_root.join(".stateful_core").join("state.db");
    let store = Store::open(&db_path).expect("file store should open");
    let app = build_router(ServerConfig::with_store("secret-token", store));

    ensure_test_reservation_via_http(&app, "s1", "w1", "src/auth.ts").await;
    let claim = app
        .clone()
        .oneshot(json_request(
            "/v1/claim/acquire",
            serde_json::json!({
                "agent_id": "s1",
                "workspace_id": "w1",
                "path": "src/auth.ts"
            }),
        ))
        .await
        .expect("claim acquire should complete");
    assert_eq!(claim.status(), StatusCode::OK);

    let trigger_conn =
        rusqlite::Connection::open(&db_path).expect("trigger connection should open");
    trigger_conn
        .execute_batch(
            "CREATE TRIGGER fail_finalize_envelope
             BEFORE UPDATE OF status ON reservations
             WHEN NEW.status = 'completed'
             BEGIN
                 SELECT RAISE(ABORT, 'simulated finalize failure');
             END;",
        )
        .expect("failure trigger should install");

    let finalize = app
        .oneshot(json_request(
            "/v1/activity/finalize",
            serde_json::json!({
                "agent_id": "s1",
                "workspace_id": "w1"
            }),
        ))
        .await
        .expect("finalize should complete");

    let status = finalize.status();
    assert!(
        status.is_client_error() || status.is_server_error(),
        "store failure should map to an error status, got {status}"
    );
    let json = response_json(finalize, 2048).await;
    assert_eq!(
        json["status"], "error",
        "unified envelope should mark errors with status=error: {json}"
    );
    assert!(
        json["message"]
            .as_str()
            .is_some_and(|message| message.contains("simulated finalize failure")),
        "unified envelope message should surface the failure reason: {json}"
    );
    assert!(
        json.get("error").is_none(),
        "legacy bare top-level error field should be gone: {json}"
    );

    drop(trigger_conn);
}

#[tokio::test]
async fn outbox_sync_store_failure_uses_status_error_envelope() {
    let temp = tempfile::tempdir().expect("temp dir should create");
    let temp_root = temp.path();
    let db_path = temp_root.join(".stateful_core").join("state.db");
    let store = Store::open(&db_path).expect("file store should open");
    let app = build_router(ServerConfig::with_store("secret-token", store));

    let trigger_conn =
        rusqlite::Connection::open(&db_path).expect("trigger connection should open");
    trigger_conn
        .execute_batch(
            "CREATE TRIGGER fail_outbox_envelope
             BEFORE INSERT ON outbox
             BEGIN
                 SELECT RAISE(ABORT, 'simulated outbox failure');
             END;",
        )
        .expect("failure trigger should install");

    let response = app
        .oneshot(json_request(
            "/v1/outbox/sync",
            serde_json::json!({
                "outbox_id": "outbox-envelope-1",
                "agent_id": "s1",
                "workspace_id": "w1",
                "sequence": 1,
                "event_type": "HeartbeatObserved",
                "payload": {"ok": true}
            }),
        ))
        .await
        .expect("outbox sync should complete");

    let status = response.status();
    assert!(
        status.is_client_error() || status.is_server_error(),
        "store failure should map to an error status, got {status}"
    );
    let json = response_json(response, 2048).await;
    assert_eq!(
        json["status"], "error",
        "unified envelope should mark errors with status=error: {json}"
    );
    assert!(
        json["message"]
            .as_str()
            .is_some_and(|message| message.contains("simulated outbox failure")),
        "unified envelope message should surface the failure reason: {json}"
    );
    assert!(
        json.get("error").is_none(),
        "legacy bare top-level error field should be gone: {json}"
    );

    drop(trigger_conn);
}

#[tokio::test]
async fn removed_compat_endpoints_return_not_found() {
    let app = build_router(ServerConfig::new("secret-token"));

    for (path, body) in [
        (
            "/v1/activity/observe",
            serde_json::json!({"agent_id": "s1", "workspace_id": "w1"}),
        ),
        (
            "/v1/conflicts/check",
            serde_json::json!({
                "agent_id": "s1",
                "action": "write_file",
                "path": "src/auth.ts"
            }),
        ),
        (
            "/v1/reconcile/ack",
            serde_json::json!({
                "agent_id": "s1",
                "workspace_id": "w1",
                "decision": "adopt",
                "files_reread": ["src/auth.ts"],
                "human_change_summary": "User adjusted auth guard."
            }),
        ),
    ] {
        let response = app
            .clone()
            .oneshot(json_request(path, body))
            .await
            .expect("request should complete");
        assert_eq!(
            response.status(),
            StatusCode::NOT_FOUND,
            "{path} should be removed and answer 404"
        );
    }
}

#[tokio::test]
async fn protected_route_rejects_wrong_bearer_token() {
    let app = build_router(ServerConfig::new("secret-token"));

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/authorize")
                .header("authorization", "Bearer wrong-token")
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::json!({
                        "agent_id": "s1",
                        "action": "write_file",
                        "path": "src/auth.ts"
                    })
                    .to_string(),
                ))
                .expect("authorize request should build"),
        )
        .await
        .expect("authorize response should complete");

    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

fn json_request(path: &str, body: serde_json::Value) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri(path)
        .header("authorization", "Bearer secret-token")
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .expect("json request should build")
}

fn protocol_body(
    agent_id: &str,
    workspace_id: &str,
    payload: serde_json::Value,
) -> serde_json::Value {
    serde_json::json!({
        "protocol_version": "stateful.v1",
        "request_id": "req-1",
        "observed_at": "2026-05-31T00:00:00Z",
        "agent": {
            "agent_id": agent_id,
            "actor_id": "agent-1",
            "actor_type": "agent"
        },
        "workspace": {
            "root": "/repo",
            "workspace_id": workspace_id,
            "repo_id": "repo-1",
            "worktree_id": "worktree-1",
            "branch": "main"
        },
        "source": {
            "kind": "cli",
            "event": "reservation_declare",
            "source_ref": "routes-test"
        },
        "payload": payload
    })
}

fn protocol_request(
    path: &str,
    agent_id: &str,
    workspace_id: &str,
    payload: serde_json::Value,
) -> Request<Body> {
    json_request(path, protocol_body(agent_id, workspace_id, payload))
}

fn native_hook_authorize_request(
    agent_id: &str,
    workspace_id: &str,
    path: &str,
    tool_name: &str,
) -> Request<Body> {
    let mut body = protocol_body(
        agent_id,
        workspace_id,
        serde_json::json!({
            "action": "write_file",
            "path": path
        }),
    );
    body["source"]["kind"] = serde_json::json!("hook");
    body["source"]["event"] = serde_json::json!("pre_tool_use");
    body["source"]["source_ref"] = serde_json::json!(format!("hook:{agent_id}:{tool_name}"));
    body["source"]["tool_name"] = serde_json::json!(tool_name);

    json_request("/v1/authorize", body)
}

async fn response_json(response: Response<Body>, limit: usize) -> serde_json::Value {
    let body = to_bytes(response.into_body(), limit)
        .await
        .expect("body should read");
    serde_json::from_slice(&body).expect("body should be json")
}

fn test_content_hash(bytes: &[u8]) -> String {
    let mut hash = 0xcbf29ce484222325u64;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("fnv1a64:{hash:016x}")
}

fn authorized_get(path: &str) -> Request<Body> {
    Request::builder()
        .method("GET")
        .uri(path)
        .header("authorization", "Bearer secret-token")
        .body(Body::empty())
        .expect("get request should build")
}

fn authorized_get_with_last_event_id(path: &str, last_event_id: &str) -> Request<Body> {
    Request::builder()
        .method("GET")
        .uri(path)
        .header("authorization", "Bearer secret-token")
        .header("last-event-id", last_event_id)
        .body(Body::empty())
        .expect("get request should build")
}
