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
            "req-authorize-token",
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
async fn side_effecting_routes_fail_closed_without_protocol_metadata() {
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
    let body = to_bytes(response.into_body(), 2048)
        .await
        .expect("body should read");
    let json: serde_json::Value = serde_json::from_slice(&body).expect("body should be json");
    assert_eq!(json["decision"], "error");
    assert_eq!(json["reason_code"], "protocol_mismatch");
}

#[tokio::test]
async fn side_effecting_routes_accept_v1_protocol_metadata() {
    let app = build_router(ServerConfig::new("secret-token"));

    let response = app
        .oneshot(protocol_request(
            "/v1/intent/declare",
            "req-declare-1",
            "s1",
            "w1",
            serde_json::json!({
                "files_planned": ["src/auth.ts"]
            }),
        ))
        .await
        .expect("intent declaration should complete");

    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn side_effecting_routes_fail_closed_on_major_protocol_mismatch() {
    let app = build_router(ServerConfig::new("secret-token"));

    let mut body = protocol_body(
        "req-declare-2",
        "s1",
        "w1",
        serde_json::json!({
            "files_planned": ["src/auth.ts"]
        }),
    );
    body["protocol_version"] = serde_json::json!("stateful.v2");

    let response = app
        .oneshot(json_request("/v1/intent/declare", body))
        .await
        .expect("intent declaration should complete");

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let body = to_bytes(response.into_body(), 2048)
        .await
        .expect("body should read");
    let json: serde_json::Value = serde_json::from_slice(&body).expect("body should be json");
    assert_eq!(json["reason_code"], "protocol_mismatch");
}

#[tokio::test]
async fn side_effecting_routes_fail_closed_on_malformed_protocol_metadata() {
    let app = build_router(ServerConfig::new("secret-token"));

    let mut numeric_protocol_version = protocol_body(
        "req-declare-3",
        "s1",
        "w1",
        serde_json::json!({
            "files_planned": ["src/auth.ts"]
        }),
    );
    numeric_protocol_version["protocol_version"] = serde_json::json!(1);

    let mut non_object_session = protocol_body(
        "req-declare-4",
        "s1",
        "w1",
        serde_json::json!({
            "files_planned": ["src/auth.ts"]
        }),
    );
    non_object_session["session"] = serde_json::json!("s1");

    for body in [numeric_protocol_version, non_object_session] {
        let response = app
            .clone()
            .oneshot(json_request("/v1/intent/declare", body))
            .await
            .expect("intent declaration should complete");

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let body = to_bytes(response.into_body(), 2048)
            .await
            .expect("body should read");
        let json: serde_json::Value = serde_json::from_slice(&body).expect("body should be json");
        assert_eq!(json["decision"], "error");
        assert_eq!(json["reason_code"], "protocol_mismatch");
    }
}

#[tokio::test]
async fn side_effecting_routes_fail_closed_on_invalid_v1_protocol_metadata() {
    let app = build_router(ServerConfig::new("secret-token"));

    let mut missing_observed_at = protocol_body(
        "req-declare-5",
        "s1",
        "w1",
        serde_json::json!({
            "files_planned": ["src/auth.ts"]
        }),
    );
    missing_observed_at
        .as_object_mut()
        .expect("body should be object")
        .remove("observed_at");

    let mut blank_observed_at = protocol_body(
        "req-declare-6",
        "s1",
        "w1",
        serde_json::json!({
            "files_planned": ["src/auth.ts"]
        }),
    );
    blank_observed_at["observed_at"] = serde_json::json!("   ");

    let mut malformed_observed_at = protocol_body(
        "req-declare-malformed-observed-at",
        "s1",
        "w1",
        serde_json::json!({
            "files_planned": ["src/auth.ts"]
        }),
    );
    malformed_observed_at["observed_at"] = serde_json::json!("not-a-date");

    let mut blank_request_id = protocol_body(
        "req-declare-7",
        "s1",
        "w1",
        serde_json::json!({
            "files_planned": ["src/auth.ts"]
        }),
    );
    blank_request_id["request_id"] = serde_json::json!("   ");

    let mut empty_request_id = protocol_body(
        "req-declare-empty",
        "s1",
        "w1",
        serde_json::json!({
            "files_planned": ["src/auth.ts"]
        }),
    );
    empty_request_id["request_id"] = serde_json::json!("");

    let mut invalid_actor_type = protocol_body(
        "req-declare-8",
        "s1",
        "w1",
        serde_json::json!({
            "files_planned": ["src/auth.ts"]
        }),
    );
    invalid_actor_type["session"]["actor_type"] = serde_json::json!("robot");

    let mut invalid_source_kind = protocol_body(
        "req-declare-9",
        "s1",
        "w1",
        serde_json::json!({
            "files_planned": ["src/auth.ts"]
        }),
    );
    invalid_source_kind["source"]["kind"] = serde_json::json!("browser");

    let mut blank_session_id = protocol_body(
        "req-declare-10",
        "s1",
        "w1",
        serde_json::json!({
            "files_planned": ["src/auth.ts"]
        }),
    );
    blank_session_id["session"]["session_id"] = serde_json::json!(" ");

    let mut missing_actor_id = protocol_body(
        "req-declare-11",
        "s1",
        "w1",
        serde_json::json!({
            "files_planned": ["src/auth.ts"]
        }),
    );
    missing_actor_id["session"]
        .as_object_mut()
        .expect("session should be object")
        .remove("actor_id");

    let mut blank_workspace_id = protocol_body(
        "req-declare-12",
        "s1",
        "w1",
        serde_json::json!({
            "files_planned": ["src/auth.ts"]
        }),
    );
    blank_workspace_id["workspace"]["workspace_id"] = serde_json::json!("");

    let mut missing_workspace_root = protocol_body(
        "req-declare-13",
        "s1",
        "w1",
        serde_json::json!({
            "files_planned": ["src/auth.ts"]
        }),
    );
    missing_workspace_root["workspace"]
        .as_object_mut()
        .expect("workspace should be object")
        .remove("root");

    for body in [
        missing_observed_at,
        blank_observed_at,
        malformed_observed_at,
        blank_request_id,
        empty_request_id,
        invalid_actor_type,
        invalid_source_kind,
        blank_session_id,
        missing_actor_id,
        blank_workspace_id,
        missing_workspace_root,
    ] {
        let response = app
            .clone()
            .oneshot(json_request("/v1/intent/declare", body))
            .await
            .expect("intent declaration should complete");

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let body = to_bytes(response.into_body(), 2048)
            .await
            .expect("body should read");
        let json: serde_json::Value = serde_json::from_slice(&body).expect("body should be json");
        assert_eq!(json["decision"], "error");
        assert_eq!(json["reason_code"], "protocol_mismatch");
    }
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
        .oneshot(protocol_request(
            "/v1/conflicts/check",
            "req-conflicts-lease-activity",
            "s1",
            "w1",
            serde_json::json!({
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
            "req-declare-s1-auth",
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
            "req-authorize-s1-auth",
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
            "req-declare-s1-scope",
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
            "req-authorize-s1-scope",
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
async fn authorize_uses_policy_service_and_preserves_scope_decision() {
    let app = build_router(ServerConfig::new("secret-token"));

    let declare = app
        .clone()
        .oneshot(protocol_request(
            "/v1/intent/declare",
            "req-policy-declare",
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
            "req-policy-authorize",
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
    let body = to_bytes(response.into_body(), 2048)
        .await
        .expect("body should read");
    let json: serde_json::Value = serde_json::from_slice(&body).expect("body should be json");
    assert_eq!(json["decision"], "deny");
    assert_eq!(json["reason_code"], "scope_mismatch");
}

#[tokio::test]
async fn authorize_valid_protocol_with_malformed_payload_returns_invalid_request() {
    let app = build_router(ServerConfig::new("secret-token"));

    let response = app
        .oneshot(protocol_request(
            "/v1/authorize",
            "req-authorize-malformed-payload",
            "s1",
            "w1",
            serde_json::json!({
                "path": "src/auth.ts"
            }),
        ))
        .await
        .expect("authorize should complete");

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let body = to_bytes(response.into_body(), 2048)
        .await
        .expect("body should read");
    let json: serde_json::Value = serde_json::from_slice(&body).expect("body should be json");
    assert_eq!(json["decision"], "error");
    assert_eq!(json["reason_code"], "invalid_request");
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
            "req-declare-s1-lease-conflict",
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
            "req-authorize-s1-lease-conflict",
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
async fn conflicts_check_does_not_enqueue_waiters_on_queue_requested() {
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

    let conflict = app
        .clone()
        .oneshot(protocol_request(
            "/v1/conflicts/check",
            "req-conflicts-s2-no-queue",
            "s2",
            "w1",
            serde_json::json!({
                "action": "write_file",
                "path": "src/auth.ts",
                "queue_on_conflict": true
            }),
        ))
        .await
        .expect("conflict check should complete");
    assert_eq!(conflict.status(), StatusCode::OK);

    let body = to_bytes(conflict.into_body(), 2048)
        .await
        .expect("body should read");
    let json: serde_json::Value = serde_json::from_slice(&body).expect("body should be json");
    assert_eq!(json["decision"], "deny");
    assert_eq!(json["reason_code"], "active_lease_conflict");
    assert!(json.get("wait").is_none());

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
    assert_eq!(json["resume_available"], false);
}

#[tokio::test]
async fn authorize_queue_retries_with_new_request_ids_reuse_waiter() {
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

    let first = app
        .clone()
        .oneshot(protocol_request(
            "/v1/authorize",
            "req-authorize-s2-queue-retry-1",
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
    assert_eq!(first.status(), StatusCode::OK);
    let body = to_bytes(first.into_body(), 2048)
        .await
        .expect("body should read");
    let first_json: serde_json::Value = serde_json::from_slice(&body).expect("body should be json");

    let second = app
        .oneshot(protocol_request(
            "/v1/authorize",
            "req-authorize-s2-queue-retry-2",
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
    assert_eq!(second.status(), StatusCode::OK);
    let body = to_bytes(second.into_body(), 2048)
        .await
        .expect("body should read");
    let second_json: serde_json::Value =
        serde_json::from_slice(&body).expect("body should be json");

    assert_eq!(
        first_json["wait"]["wait_id"],
        second_json["wait"]["wait_id"]
    );
    assert_eq!(first_json["wait"]["queue_position"], 1);
    assert_eq!(second_json["wait"]["queue_position"], 1);
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
        let request_id = format!("req-declare-{session_id}-queue");
        let declare = app
            .clone()
            .oneshot(protocol_request(
                "/v1/intent/declare",
                &request_id,
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
            "req-authorize-s2-queue",
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
            "req-authorize-s3-queue",
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
            "req-authorize-s3-reservation-blocked",
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

    let requires_claim_b = app
        .clone()
        .oneshot(protocol_request(
            "/v1/authorize",
            "req-authorize-s2-reservation-owner",
            "s2",
            "w1",
            serde_json::json!({
                "action": "write_file",
                "path": "src/auth.ts"
            }),
        ))
        .await
        .expect("authorize should complete");
    let body = to_bytes(requires_claim_b.into_body(), 2048)
        .await
        .expect("body should read");
    let json: serde_json::Value = serde_json::from_slice(&body).expect("body should be json");
    assert_eq!(json["decision"], "deny");
    assert_eq!(json["reason_code"], "reservation_requires_claim");
    assert_eq!(json["reservation"]["status"], "reserved");
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
            "req-declare-s2-notify",
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
            "req-authorize-s2-notify-queue",
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
            "req-authorize-s2-resume-queue",
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
            "req-declare-s1-same-lease",
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
            "req-authorize-s1-same-lease",
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
            "req-declare-s1-delete",
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
            "req-authorize-s1-delete",
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
            "req-declare-s1-current",
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
            "req-declare-s1-events",
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
async fn intent_declare_persists_envelope_workspace_identity() {
    let app = build_router(ServerConfig::new("secret-token"));

    let mut body = protocol_body(
        "req-declare-s1-envelope-identity",
        "s1",
        "w-envelope",
        serde_json::json!({
            "files_planned": ["src/auth.ts"]
        }),
    );
    body["workspace"]["repo_id"] = serde_json::json!("repo-envelope");
    body["workspace"]["worktree_id"] = serde_json::json!("worktree-envelope");
    body["workspace"]["root"] = serde_json::json!("/repo-envelope");
    body["workspace"]["branch"] = serde_json::json!("feature/envelope");

    let declare = app
        .clone()
        .oneshot(json_request("/v1/intent/declare", body))
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
    let event = &json["events"][0];
    assert_eq!(event["event_type"], "IntentDeclared");
    assert_eq!(event["workspace_id"], "w-envelope");
    assert_eq!(event["repo_id"], "repo-envelope");
    assert_eq!(event["worktree_id"], "worktree-envelope");
    assert_eq!(event["root"], "/repo-envelope");
    assert_eq!(event["branch"], "feature/envelope");
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
            "req-authorize-sqlite-store",
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
    request_id: &str,
    session_id: &str,
    workspace_id: &str,
    payload: serde_json::Value,
) -> serde_json::Value {
    let mut body = serde_json::json!({
        "protocol_version": "stateful.v1",
        "request_id": request_id,
        "observed_at": "2026-06-03T00:00:00Z",
        "session": {
            "session_id": session_id,
            "actor_id": session_id,
            "actor_type": "agent"
        },
        "workspace": {
            "workspace_id": workspace_id,
            "root": "/repo",
            "repo_id": "repo-1",
            "worktree_id": "worktree-1",
            "branch": "main"
        },
        "source": {
            "kind": "cli",
            "event": "test",
            "source_ref": "routes.rs"
        }
    });
    let object = body.as_object_mut().expect("body should be object");
    for (key, value) in payload.as_object().expect("payload should be object") {
        object.insert(key.clone(), value.clone());
    }
    body
}

fn protocol_request(
    path: &str,
    request_id: &str,
    session_id: &str,
    workspace_id: &str,
    payload: serde_json::Value,
) -> Request<Body> {
    json_request(
        path,
        protocol_body(request_id, session_id, workspace_id, payload),
    )
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
