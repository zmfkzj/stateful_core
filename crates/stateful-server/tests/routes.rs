use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode},
};
use stateful_server::{ServerConfig, build_router};
use stateful_store::{Event, Store};
use std::{fs, process::Command};
use tower::ServiceExt;

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
                        "session_id": "s1",
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
async fn side_effecting_routes_intent_declare_rejects_legacy_body() {
    let app = build_router(ServerConfig::new("secret-token"));

    let response = app
        .oneshot(json_request(
            "/v1/intent/declare",
            serde_json::json!({
                "session_id": "s1",
                "workspace_id": "w1",
                "files_planned": ["src/auth.ts"]
            }),
        ))
        .await
        .expect("intent declaration should complete");
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);

    let body = to_bytes(response.into_body(), 1024)
        .await
        .expect("body should read");
    let json: serde_json::Value = serde_json::from_slice(&body).expect("body should be json");
    assert_eq!(json["decision"], "error");
    assert_eq!(json["reason_code"], "protocol_mismatch");
}

#[tokio::test]
async fn side_effecting_routes_intent_declare_accepts_protocol_envelope() {
    let app = build_router(ServerConfig::new("secret-token"));

    let response = app
        .clone()
        .oneshot(protocol_request(
            "/v1/intent/declare",
            "s1",
            "w1",
            serde_json::json!({
                "files_planned": ["src/auth.ts"]
            }),
        ))
        .await
        .expect("intent declaration should complete");
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
    assert_eq!(json["events"][0]["event_type"], "IntentDeclared");
    assert_eq!(json["events"][0]["session_id"], "s1");
    assert_eq!(json["events"][0]["workspace_id"], "w1");
    assert_eq!(json["events"][0]["repo_id"], "repo-1");

    let authorize = app
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
    assert_eq!(authorize.status(), StatusCode::OK);
    let body = to_bytes(authorize.into_body(), 1024)
        .await
        .expect("body should read");
    let json: serde_json::Value = serde_json::from_slice(&body).expect("body should be json");
    assert_eq!(json["decision"], "allow");
    assert_eq!(json["reason_code"], "authorized");
}

#[tokio::test]
async fn side_effecting_routes_intent_declare_does_not_store_empty_identity_sentinels() {
    let app = build_router(ServerConfig::new("secret-token"));

    let response = app
        .clone()
        .oneshot(json_request(
            "/v1/intent/declare",
            serde_json::json!({
                "protocol_version": "stateful.v1",
                "request_id": "req-empty-identity",
                "observed_at": "2026-05-31T00:00:00Z",
                "session": {
                    "session_id": "s1",
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
                    "event": "intent_declare",
                    "source_ref": "routes-test"
                },
                "payload": {
                    "files_planned": ["src/auth.ts"]
                }
            }),
        ))
        .await
        .expect("intent declaration should complete");
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
                "session_id": "s1",
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
            "/v1/intent/declare",
            "s1",
            "w1",
            serde_json::json!({
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
    let body = to_bytes(response.into_body(), 2048)
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
                "session_id": "s1",
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
                "session_id": "s1",
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
    assert_eq!(json["current"]["session_count"], 1);
    assert_eq!(json["current"]["event_count"], 2);
}

#[tokio::test]
async fn session_events_preserve_repo_identity_when_provided() {
    let temp_root = std::env::temp_dir().join(format!(
        "stateful-session-identity-store-{}",
        std::process::id()
    ));
    if temp_root.exists() {
        std::fs::remove_dir_all(&temp_root).expect("old temp root should be removable");
    }
    let db_path = temp_root.join(".stateful_core").join("state.db");
    let store = Store::open(&db_path).expect("file store should open");
    let app = build_router(ServerConfig::with_store("secret-token", store));

    let response = app
        .clone()
        .oneshot(json_request(
            "/v1/session/register",
            serde_json::json!({
                "session_id": "s1",
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

    std::fs::remove_dir_all(&temp_root).expect("temp root should be removable");
}

#[tokio::test]
async fn lease_activity_and_conflict_routes_are_available() {
    let temp_root = std::env::temp_dir().join(format!(
        "stateful-coordination-store-{}",
        std::process::id()
    ));
    if temp_root.exists() {
        std::fs::remove_dir_all(&temp_root).expect("old temp root should be removable");
    }
    let db_path = temp_root.join(".stateful_core").join("state.db");
    let store = Store::open(&db_path).expect("file store should open");
    let app = build_router(ServerConfig::with_store("secret-token", store));

    let lease = app
        .clone()
        .oneshot(json_request(
            "/v1/lease/acquire",
            serde_json::json!({
                "session_id": "s1",
                "workspace_id": "w1",
                "path": "src/auth.ts"
            }),
        ))
        .await
        .expect("lease acquire should complete");
    assert_eq!(lease.status(), StatusCode::OK);

    let activity = app
        .clone()
        .oneshot(json_request(
            "/v1/activity/observe",
            serde_json::json!({
                "session_id": "s1",
                "workspace_id": "w1"
            }),
        ))
        .await
        .expect("activity observe should complete");
    assert_eq!(activity.status(), StatusCode::OK);

    let finalized = app
        .clone()
        .oneshot(json_request(
            "/v1/activity/finalize",
            serde_json::json!({
                "session_id": "s1",
                "workspace_id": "w1"
            }),
        ))
        .await
        .expect("activity finalize should complete");
    assert_eq!(finalized.status(), StatusCode::OK);

    let conflict = app
        .clone()
        .oneshot(json_request(
            "/v1/conflicts/check",
            serde_json::json!({
                "session_id": "s1",
                "action": "write_file",
                "path": "src/auth.ts"
            }),
        ))
        .await
        .expect("conflict check should complete");
    assert_eq!(conflict.status(), StatusCode::OK);
    let body = to_bytes(conflict.into_body(), 1024)
        .await
        .expect("body should read");
    let json: serde_json::Value = serde_json::from_slice(&body).expect("body should be json");
    assert_eq!(json["decision"], "deny");
    assert_eq!(json["reason_code"], "missing_intent");

    let release = app
        .oneshot(json_request(
            "/v1/lease/release",
            serde_json::json!({
                "session_id": "s1",
                "workspace_id": "w1",
                "path": "src/auth.ts"
            }),
        ))
        .await
        .expect("lease release should complete");
    assert_eq!(release.status(), StatusCode::OK);

    let reopened = Store::open(&db_path).expect("file store should reopen");
    assert_eq!(reopened.lease_count().expect("lease count should load"), 1);
    assert_eq!(
        reopened
            .activity_count()
            .expect("activity count should load"),
        2
    );

    std::fs::remove_dir_all(&temp_root).expect("temp root should be removable");
}

#[tokio::test]
async fn declared_intent_allows_matching_authorize_request() {
    let app = build_router(ServerConfig::new("secret-token"));

    let declare = app
        .clone()
        .oneshot(protocol_request(
            "/v1/intent/declare",
            "s1",
            "w1",
            serde_json::json!({
                "files_planned": ["src/auth.ts"]
            }),
        ))
        .await
        .expect("intent declaration should complete");
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
async fn declared_intent_denies_out_of_scope_authorize_request() {
    let app = build_router(ServerConfig::new("secret-token"));

    let declare = app
        .clone()
        .oneshot(protocol_request(
            "/v1/intent/declare",
            "s1",
            "w1",
            serde_json::json!({
                "files_planned": ["src/auth.ts"]
            }),
        ))
        .await
        .expect("intent declaration should complete");
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
async fn active_lease_by_other_session_denies_authorize_even_with_matching_intent() {
    let app = build_router(ServerConfig::new("secret-token"));

    let lease = app
        .clone()
        .oneshot(json_request(
            "/v1/lease/acquire",
            serde_json::json!({
                "session_id": "s2",
                "workspace_id": "w1",
                "path": "src/auth.ts"
            }),
        ))
        .await
        .expect("lease acquire should complete");
    assert_eq!(lease.status(), StatusCode::OK);

    let declare = app
        .clone()
        .oneshot(protocol_request(
            "/v1/intent/declare",
            "s1",
            "w1",
            serde_json::json!({
                "files_planned": ["src/auth.ts"]
            }),
        ))
        .await
        .expect("intent declaration should complete");
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
    assert_eq!(json["reason_code"], "active_lease_conflict");
}

#[tokio::test]
async fn queued_conflict_reserves_first_waiter_after_lease_release() {
    let store = Store::open_in_memory().expect("store should open");
    let app = build_router(ServerConfig::with_store("secret-token", store));

    let lease = app
        .clone()
        .oneshot(json_request(
            "/v1/lease/acquire",
            serde_json::json!({
                "session_id": "s1",
                "workspace_id": "w1",
                "path": "src/auth.ts"
            }),
        ))
        .await
        .expect("lease acquire should complete");
    assert_eq!(lease.status(), StatusCode::OK);

    for session_id in ["s2", "s3"] {
        let declare = app
            .clone()
            .oneshot(protocol_request(
                "/v1/intent/declare",
                session_id,
                "w1",
                serde_json::json!({
                    "files_planned": ["src/auth.ts"]
                }),
            ))
            .await
            .expect("intent declaration should complete");
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
                "queue_on_conflict": true
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
    assert_eq!(json["reason_code"], "active_lease_conflict");
    assert_eq!(json["wait"]["status"], "queued");
    assert_eq!(json["wait"]["queue_position"], 1);
    assert_eq!(json["wait"]["blocking_session_id"], "s1");

    let queued_c = app
        .clone()
        .oneshot(protocol_request(
            "/v1/authorize",
            "s3",
            "w1",
            serde_json::json!({
                "action": "write_file",
                "path": "src/auth.ts",
                "queue_on_conflict": true
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
            "/v1/lease/release",
            serde_json::json!({
                "session_id": "s1",
                "workspace_id": "w1",
                "path": "src/auth.ts"
            }),
        ))
        .await
        .expect("lease release should complete");
    assert_eq!(release.status(), StatusCode::OK);

    let blocked_c = app
        .clone()
        .oneshot(protocol_request(
            "/v1/authorize",
            "s3",
            "w1",
            serde_json::json!({
                "action": "write_file",
                "path": "src/auth.ts",
                "queue_on_conflict": true
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
    assert_eq!(json["reservation"]["session_id"], "s2");

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
    assert_eq!(json["reservation"]["status"], "claimed");
}

#[tokio::test]
async fn activity_finalize_releases_leases_and_notifications_poll_returns_resume_signal() {
    let store = Store::open_in_memory().expect("store should open");
    let app = build_router(ServerConfig::with_store("secret-token", store));

    let lease = app
        .clone()
        .oneshot(json_request(
            "/v1/lease/acquire",
            serde_json::json!({
                "session_id": "s1",
                "workspace_id": "w1",
                "path": "src/auth.ts"
            }),
        ))
        .await
        .expect("lease acquire should complete");
    assert_eq!(lease.status(), StatusCode::OK);

    let declare = app
        .clone()
        .oneshot(protocol_request(
            "/v1/intent/declare",
            "s2",
            "w1",
            serde_json::json!({
                "files_planned": ["src/auth.ts"]
            }),
        ))
        .await
        .expect("intent declaration should complete");
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
                "queue_on_conflict": true
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
                "session_id": "s1",
                "workspace_id": "w1"
            }),
        ))
        .await
        .expect("finalize should complete");
    assert_eq!(finalize.status(), StatusCode::OK);

    let poll = app
        .oneshot(json_request(
            "/v1/notifications/poll",
            serde_json::json!({
                "session_id": "s2",
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
}

#[tokio::test]
async fn resume_next_returns_active_reservation_for_session() {
    let store = Store::open_in_memory().expect("store should open");
    let app = build_router(ServerConfig::with_store("secret-token", store));

    let lease = app
        .clone()
        .oneshot(json_request(
            "/v1/lease/acquire",
            serde_json::json!({
                "session_id": "s1",
                "workspace_id": "w1",
                "path": "src/auth.ts"
            }),
        ))
        .await
        .expect("lease acquire should complete");
    assert_eq!(lease.status(), StatusCode::OK);

    let queued = app
        .clone()
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
    assert_eq!(queued.status(), StatusCode::OK);

    let release = app
        .clone()
        .oneshot(json_request(
            "/v1/lease/release",
            serde_json::json!({
                "session_id": "s1",
                "workspace_id": "w1",
                "path": "src/auth.ts"
            }),
        ))
        .await
        .expect("lease release should complete");
    assert_eq!(release.status(), StatusCode::OK);

    let resume = app
        .oneshot(json_request(
            "/v1/resume/next",
            serde_json::json!({
                "session_id": "s2",
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
    assert_eq!(json["reservation"]["session_id"], "s2");
    assert_eq!(json["reservation"]["relative_path"], "src/auth.ts");
    assert_eq!(
        json["required_next_action"],
        "Claim the reservation by retrying the write after rereading the file."
    );
}

#[tokio::test]
async fn active_lease_by_same_session_allows_matching_authorize() {
    let app = build_router(ServerConfig::new("secret-token"));

    let lease = app
        .clone()
        .oneshot(json_request(
            "/v1/lease/acquire",
            serde_json::json!({
                "session_id": "s1",
                "workspace_id": "w1",
                "path": "src/auth.ts"
            }),
        ))
        .await
        .expect("lease acquire should complete");
    assert_eq!(lease.status(), StatusCode::OK);

    let declare = app
        .clone()
        .oneshot(protocol_request(
            "/v1/intent/declare",
            "s1",
            "w1",
            serde_json::json!({
                "files_planned": ["src/auth.ts"]
            }),
        ))
        .await
        .expect("intent declaration should complete");
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
async fn delete_file_action_requires_exact_file_intent_over_http() {
    let app = build_router(ServerConfig::new("secret-token"));

    let declare = app
        .clone()
        .oneshot(protocol_request(
            "/v1/intent/declare",
            "s1",
            "w1",
            serde_json::json!({
                "files_planned": ["src/"]
            }),
        ))
        .await
        .expect("intent declaration should complete");
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
async fn current_returns_materialized_state_summary() {
    let app = build_router(ServerConfig::new("secret-token"));

    let declare = app
        .clone()
        .oneshot(protocol_request(
            "/v1/intent/declare",
            "s1",
            "w1",
            serde_json::json!({
                "files_planned": ["src/auth.ts"]
            }),
        ))
        .await
        .expect("intent declaration should complete");
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
    assert_eq!(json["current"]["active_intent_count"], 1);
    assert_eq!(json["current"]["event_count"], 1);
}

#[tokio::test]
async fn events_returns_recent_audit_events() {
    let app = build_router(ServerConfig::new("secret-token"));

    let declare = app
        .clone()
        .oneshot(protocol_request(
            "/v1/intent/declare",
            "s1",
            "w1",
            serde_json::json!({
                "files_planned": ["src/auth.ts"]
            }),
        ))
        .await
        .expect("intent declaration should complete");
    assert_eq!(declare.status(), StatusCode::OK);

    let response = app
        .oneshot(authorized_get("/v1/events"))
        .await
        .expect("events request should complete");
    assert_eq!(response.status(), StatusCode::OK);

    let body = to_bytes(response.into_body(), 2048)
        .await
        .expect("body should read");
    let json: serde_json::Value = serde_json::from_slice(&body).expect("body should be json");
    assert_eq!(json["status"], "ok");
    assert_eq!(json["events"][0]["event_type"], "IntentDeclared");
}

#[tokio::test]
async fn authorize_uses_supplied_sqlite_store() {
    let temp_root =
        std::env::temp_dir().join(format!("stateful-server-store-{}", std::process::id()));
    if temp_root.exists() {
        std::fs::remove_dir_all(&temp_root).expect("old temp root should be removable");
    }
    let db_path = temp_root.join(".stateful_core").join("state.db");
    let store = Store::open(&db_path).expect("file store should open");
    store
        .append(Event::intent_declared("s1", "w1", ["src/auth.ts"]))
        .expect("intent should append");
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

    std::fs::remove_dir_all(&temp_root).expect("temp root should be removable");
}

#[tokio::test]
async fn context_render_returns_empty_prompt_when_no_blocking_state_exists() {
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
    assert_eq!(response.status(), StatusCode::OK);

    let body = to_bytes(response.into_body(), 2048)
        .await
        .expect("body should read");
    let json: serde_json::Value = serde_json::from_slice(&body).expect("body should be json");
    assert_eq!(json["status"], "ok");
    assert_eq!(json["prompt_text"], "");
}

#[tokio::test]
async fn reconcile_ack_records_acknowledgement() {
    let app = build_router(ServerConfig::new("secret-token"));

    let response = app
        .oneshot(json_request(
            "/v1/reconcile/ack",
            serde_json::json!({
                "session_id": "s1",
                "workspace_id": "w1",
                "decision": "adopt",
                "files_reread": ["src/auth.ts"],
                "human_change_summary": "User adjusted auth guard."
            }),
        ))
        .await
        .expect("reconcile ack should complete");
    assert_eq!(response.status(), StatusCode::OK);

    let body = to_bytes(response.into_body(), 1024)
        .await
        .expect("body should read");
    let json: serde_json::Value = serde_json::from_slice(&body).expect("body should be json");
    assert_eq!(json["status"], "ok");
    assert_eq!(json["clears_human_write_block"], true);
}

#[tokio::test]
async fn reconcile_ack_persists_acknowledgement() {
    let temp_root =
        std::env::temp_dir().join(format!("stateful-reconcile-store-{}", std::process::id()));
    if temp_root.exists() {
        std::fs::remove_dir_all(&temp_root).expect("old temp root should be removable");
    }
    let db_path = temp_root.join(".stateful_core").join("state.db");
    let store = Store::open(&db_path).expect("file store should open");
    let app = build_router(ServerConfig::with_store("secret-token", store));

    let response = app
        .oneshot(json_request(
            "/v1/reconcile/ack",
            serde_json::json!({
                "session_id": "s1",
                "workspace_id": "w1",
                "decision": "reapply",
                "files_reread": ["src/auth.ts"],
                "human_change_summary": "User edited guard."
            }),
        ))
        .await
        .expect("reconcile ack should complete");
    assert_eq!(response.status(), StatusCode::OK);

    let reopened = Store::open(&db_path).expect("file store should reopen");
    assert_eq!(
        reopened
            .reconciliation_count()
            .expect("reconciliation count should load"),
        1
    );

    std::fs::remove_dir_all(&temp_root).expect("temp root should be removable");
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
                    "session_id": "s1",
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
async fn validation_run_executes_controlled_profile() {
    let repo = TestRepo::new("server-validation-pass");
    repo.write_validation(
        r#"
profiles:
  - profile_id: pass
    description: Pass
    command: true
    cwd: .
    timeout_seconds: 10
    denied_writes:
      - src/**
"#,
    );
    let app = build_router(ServerConfig::new("secret-token"));

    let response = app
        .oneshot(json_request(
            "/v1/validation/run",
            serde_json::json!({
                "workspace_id": "w1",
                "repo_root": repo.path(),
                "profile": "pass"
            }),
        ))
        .await
        .expect("validation run should complete");
    assert_eq!(response.status(), StatusCode::OK);

    let body = to_bytes(response.into_body(), 2048)
        .await
        .expect("body should read");
    let json: serde_json::Value = serde_json::from_slice(&body).expect("body should be json");
    assert_eq!(json["profile_id"], "pass");
    assert_eq!(json["status"], "passed");
}

#[tokio::test]
async fn validation_run_persists_result() {
    let repo = TestRepo::new("server-validation-store");
    repo.write_validation(
        r#"
profiles:
  - profile_id: pass
    description: Pass
    command: true
    cwd: .
    timeout_seconds: 10
    denied_writes:
      - src/**
"#,
    );
    let temp_root =
        std::env::temp_dir().join(format!("stateful-validation-store-{}", std::process::id()));
    if temp_root.exists() {
        std::fs::remove_dir_all(&temp_root).expect("old temp root should be removable");
    }
    let db_path = temp_root.join(".stateful_core").join("state.db");
    let store = Store::open(&db_path).expect("file store should open");
    let app = build_router(ServerConfig::with_store("secret-token", store));

    let response = app
        .oneshot(json_request(
            "/v1/validation/run",
            serde_json::json!({
                "workspace_id": "w1",
                "repo_root": repo.path(),
                "profile": "pass"
            }),
        ))
        .await
        .expect("validation run should complete");
    assert_eq!(response.status(), StatusCode::OK);

    let reopened = Store::open(&db_path).expect("file store should reopen");
    assert_eq!(
        reopened
            .validation_count()
            .expect("validation count should load"),
        1
    );

    std::fs::remove_dir_all(&temp_root).expect("temp root should be removable");
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
    session_id: &str,
    workspace_id: &str,
    payload: serde_json::Value,
) -> serde_json::Value {
    serde_json::json!({
        "protocol_version": "stateful.v1",
        "request_id": "req-1",
        "observed_at": "2026-05-31T00:00:00Z",
        "session": {
            "session_id": session_id,
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
            "event": "intent_declare",
            "source_ref": "routes-test"
        },
        "payload": payload
    })
}

fn protocol_request(
    path: &str,
    session_id: &str,
    workspace_id: &str,
    payload: serde_json::Value,
) -> Request<Body> {
    json_request(path, protocol_body(session_id, workspace_id, payload))
}

fn authorized_get(path: &str) -> Request<Body> {
    Request::builder()
        .method("GET")
        .uri(path)
        .header("authorization", "Bearer secret-token")
        .body(Body::empty())
        .expect("get request should build")
}

struct TestRepo {
    root: std::path::PathBuf,
}

impl TestRepo {
    fn new(name: &str) -> Self {
        let root = std::env::temp_dir().join(format!("{name}-{}", std::process::id()));
        if root.exists() {
            fs::remove_dir_all(&root).expect("old temp repo should be removable");
        }
        fs::create_dir_all(root.join("src")).expect("src dir should be creatable");
        fs::create_dir_all(root.join(".stateful")).expect("stateful dir should be creatable");
        fs::write(root.join("src/auth.ts"), "initial\n").expect("source file should write");
        run_git(&root, ["init"]);
        run_git(&root, ["add", "."]);
        run_git(&root, ["commit", "-m", "initial"]);

        Self { root }
    }

    fn path(&self) -> &std::path::Path {
        &self.root
    }

    fn write_validation(&self, contents: &str) {
        fs::write(self.root.join(".stateful/validation.yml"), contents)
            .expect("validation config should write");
    }
}

impl Drop for TestRepo {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn run_git<const N: usize>(root: &std::path::Path, args: [&str; N]) {
    let status = Command::new("git")
        .args(args)
        .current_dir(root)
        .env("GIT_AUTHOR_NAME", "stateful test")
        .env("GIT_AUTHOR_EMAIL", "stateful@example.invalid")
        .env("GIT_COMMITTER_NAME", "stateful test")
        .env("GIT_COMMITTER_EMAIL", "stateful@example.invalid")
        .status()
        .expect("git command should run");
    assert!(status.success(), "git command should succeed");
}
