use axum::{
    Router,
    body::{Body, to_bytes},
    http::{Request, Response, StatusCode},
};
use stateful_server::{ServerConfig, build_router};
use stateful_store::{Event, Store};
use tower::ServiceExt;

fn acquire_test_lease(store: &Store, session_id: &str, workspace_id: &str, path: &str) {
    let has_matching_intent = store
        .live_current_state(Some(path))
        .expect("live current state should load")
        .items
        .iter()
        .any(|item| {
            item.kind == stateful_core::CurrentItemKind::Intent
                && item.session_id.as_deref() == Some(session_id)
        });
    if !has_matching_intent {
        store
            .append(Event::intent_declared(
                session_id,
                workspace_id,
                format!("Acquire test lease for {path}."),
                [path],
            ))
            .expect("lease intent should append");
    }
    store
        .acquire_lease(session_id, workspace_id, path)
        .expect("lease should acquire");
}

async fn ensure_test_intent_via_http(
    app: &Router,
    session_id: &str,
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
        let has_matching_intent = current["items"].as_array().is_some_and(|items| {
            items.iter().any(|item| {
                item["kind"] == "intent"
                    && item["freshness"] == "live"
                    && item["resource"] == path
                    && item["session_id"] == session_id
                    && item["workspace_id"] == workspace_id
            })
        });
        if has_matching_intent {
            return;
        }
    }

    let declare = app
        .clone()
        .oneshot(protocol_request(
            "/v1/intent/declare",
            session_id,
            workspace_id,
            serde_json::json!({
                "purpose": format!("Acquire test lease for {path}."),
                "files_planned": [path]
            }),
        ))
        .await
        .expect("intent declaration should complete");
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
async fn side_effecting_routes_intent_declare_rejects_legacy_body() {
    let app = build_router(ServerConfig::new("secret-token"));

    let response = app
        .oneshot(json_request(
            "/v1/intent/declare",
            serde_json::json!({
                "session_id": "s1",
                "workspace_id": "w1",
                "purpose": "Test requested work.",
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
                "purpose": "Test requested work.",
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
async fn side_effecting_routes_intent_declare_rejects_empty_purpose() {
    let app = build_router(ServerConfig::new("secret-token"));

    let response = app
        .oneshot(protocol_request(
            "/v1/intent/declare",
            "s1",
            "w1",
            serde_json::json!({
                "purpose": "   ",
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
    assert_eq!(json["status"], "error");
    assert_eq!(json["reason_code"], "missing_purpose");
}

#[tokio::test]
async fn side_effecting_routes_intent_declare_rejects_empty_files_planned() {
    let app = build_router(ServerConfig::new("secret-token"));

    let response = app
        .oneshot(protocol_request(
            "/v1/intent/declare",
            "s1",
            "w1",
            serde_json::json!({
                "purpose": "Fix auth validation behavior.",
                "files_planned": []
            }),
        ))
        .await
        .expect("intent declaration should complete");

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let body = to_bytes(response.into_body(), 1024)
        .await
        .expect("body should read");
    let json: serde_json::Value = serde_json::from_slice(&body).expect("body should be json");
    assert_eq!(json["status"], "error");
    assert_eq!(json["reason_code"], "missing_scope");
}

#[tokio::test]
async fn side_effecting_routes_intent_declare_rejects_normalized_empty_files_planned() {
    let app = build_router(ServerConfig::new("secret-token"));

    for path in ["./", "../", "/"] {
        let response = app
            .clone()
            .oneshot(protocol_request(
                "/v1/intent/declare",
                "s1",
                "w1",
                serde_json::json!({
                    "purpose": "Fix auth validation behavior.",
                    "files_planned": [path]
                }),
            ))
            .await
            .expect("intent declaration should complete");

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
                    "purpose": "Test requested work.",
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

    ensure_test_intent_via_http(&app, "s1", "w1", "src/auth.ts").await;
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
async fn declared_intent_without_same_session_lease_denies_matching_authorize_request() {
    let app = build_router(ServerConfig::new("secret-token"));

    let declare = app
        .clone()
        .oneshot(protocol_request(
            "/v1/intent/declare",
            "s1",
            "w1",
            serde_json::json!({
                "purpose": "Test requested work.",
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
    assert_eq!(json["reason_code"], "missing_lease");
}

#[tokio::test]
async fn missing_lease_cannot_be_bypassed_by_changing_session_id() {
    let app = build_router(ServerConfig::new("secret-token"));

    for session_id in ["s-original", "s-swapped"] {
        let declare = app
            .clone()
            .oneshot(protocol_request(
                "/v1/intent/declare",
                session_id,
                "w1",
                serde_json::json!({
                    "purpose": "Test requested work.",
                    "files_planned": ["src/auth.ts"]
                }),
            ))
            .await
            .expect("intent declaration should complete");
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
    assert_eq!(json["reason_code"], "missing_lease");

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
    assert_eq!(json["reason_code"], "missing_lease");
    let required_next_action = json["required_next_action"].as_str().unwrap_or_default();
    assert!(required_next_action.contains("same-session file leases"));
    assert!(required_next_action.contains("Do not change session_id"));
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
                "purpose": "Test requested work.",
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
async fn hook_native_write_requires_exact_file_intent_even_when_directory_scope_allows_write_file()
{
    let app = build_router(ServerConfig::new("secret-token"));

    let declare = app
        .clone()
        .oneshot(protocol_request(
            "/v1/intent/declare",
            "s1",
            "w1",
            serde_json::json!({
                "purpose": "Test requested work.",
                "files_planned": ["src/"]
            }),
        ))
        .await
        .expect("intent declaration should complete");
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
            .contains("exact file intent")
    );
}

#[tokio::test]
async fn hook_write_requires_tool_name_even_when_source_ref_names_native_tool() {
    let app = build_router(ServerConfig::new("secret-token"));

    let declare = app
        .clone()
        .oneshot(protocol_request(
            "/v1/intent/declare",
            "s1",
            "w1",
            serde_json::json!({
                "purpose": "Test requested work.",
                "files_planned": ["src/"]
            }),
        ))
        .await
        .expect("intent declaration should complete");
    assert_eq!(declare.status(), StatusCode::OK);

    ensure_test_intent_via_http(&app, "s1", "w1", "src/").await;
    let lease = app
        .clone()
        .oneshot(json_request(
            "/v1/lease/acquire",
            serde_json::json!({
                "session_id": "s1",
                "workspace_id": "w1",
                "path": "src/"
            }),
        ))
        .await
        .expect("directory lease acquire should complete");
    assert_eq!(lease.status(), StatusCode::OK);

    let declare = app
        .clone()
        .oneshot(protocol_request(
            "/v1/intent/declare",
            "s1",
            "w1",
            serde_json::json!({
                "purpose": "Test requested work.",
                "files_planned": ["src/auth.ts"]
            }),
        ))
        .await
        .expect("intent declaration should complete");
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
    body["source"]["source_ref"] = serde_json::json!("apply_patch");

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
    assert_eq!(json["reason_code"], "missing_tool_name");
}

#[tokio::test]
async fn hook_native_write_requires_exact_file_lease_even_when_directory_lease_covers_path() {
    let app = build_router(ServerConfig::new("secret-token"));

    let declare = app
        .clone()
        .oneshot(protocol_request(
            "/v1/intent/declare",
            "s1",
            "w1",
            serde_json::json!({
                "purpose": "Test requested work.",
                "files_planned": ["src/auth.ts"]
            }),
        ))
        .await
        .expect("intent declaration should complete");
    assert_eq!(declare.status(), StatusCode::OK);

    ensure_test_intent_via_http(&app, "s1", "w1", "src/").await;
    let lease = app
        .clone()
        .oneshot(json_request(
            "/v1/lease/acquire",
            serde_json::json!({
                "session_id": "s1",
                "workspace_id": "w1",
                "path": "src/"
            }),
        ))
        .await
        .expect("directory lease acquire should complete");

    assert_eq!(lease.status(), StatusCode::OK);

    let declare = app
        .clone()
        .oneshot(protocol_request(
            "/v1/intent/declare",
            "s1",
            "w1",
            serde_json::json!({
                "purpose": "Test requested work.",
                "files_planned": ["src/auth.ts"]
            }),
        ))
        .await
        .expect("intent declaration should complete");
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
    assert_eq!(json["reason_code"], "missing_lease");
    assert!(
        json["required_next_action"]
            .as_str()
            .unwrap_or_default()
            .contains("same-session file leases")
    );
}

#[tokio::test]
async fn hook_native_write_allows_exact_file_intent_and_exact_file_lease() {
    let app = build_router(ServerConfig::new("secret-token"));

    let declare = app
        .clone()
        .oneshot(protocol_request(
            "/v1/intent/declare",
            "s1",
            "w1",
            serde_json::json!({
                "purpose": "Test requested work.",
                "files_planned": ["src/auth.ts"]
            }),
        ))
        .await
        .expect("intent declaration should complete");
    assert_eq!(declare.status(), StatusCode::OK);

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
        .expect("file lease acquire should complete");
    assert_eq!(lease.status(), StatusCode::OK);

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
async fn cli_sandbox_write_file_keeps_directory_intent_and_lease_semantics() {
    let app = build_router(ServerConfig::new("secret-token"));

    let declare = app
        .clone()
        .oneshot(protocol_request(
            "/v1/intent/declare",
            "s1",
            "w1",
            serde_json::json!({
                "purpose": "Test requested work.",
                "files_planned": ["src/"]
            }),
        ))
        .await
        .expect("intent declaration should complete");
    assert_eq!(declare.status(), StatusCode::OK);

    ensure_test_intent_via_http(&app, "s1", "w1", "src/").await;
    let lease = app
        .clone()
        .oneshot(json_request(
            "/v1/lease/acquire",
            serde_json::json!({
                "session_id": "s1",
                "workspace_id": "w1",
                "path": "src/"
            }),
        ))
        .await
        .expect("directory lease acquire should complete");
    assert_eq!(lease.status(), StatusCode::OK);

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
    assert_eq!(json["decision"], "allow");
    assert_eq!(json["reason_code"], "authorized");
}

#[tokio::test]
async fn write_file_requires_file_lease_even_when_same_session_has_directory_lease_same_path() {
    let app = build_router(ServerConfig::new("secret-token"));

    let declare_directory = app
        .clone()
        .oneshot(protocol_request(
            "/v1/intent/declare",
            "s1",
            "w1",
            serde_json::json!({
                "purpose": "Test requested directory work.",
                "files_planned": ["target/"]
            }),
        ))
        .await
        .expect("directory intent declaration should complete");
    assert_eq!(declare_directory.status(), StatusCode::OK);

    let directory_lease = app
        .clone()
        .oneshot(json_request(
            "/v1/lease/acquire",
            serde_json::json!({
                "session_id": "s1",
                "workspace_id": "w1",
                "path": "target/"
            }),
        ))
        .await
        .expect("directory lease acquire should complete");
    assert_eq!(directory_lease.status(), StatusCode::OK);

    let declare_file = app
        .clone()
        .oneshot(protocol_request(
            "/v1/intent/declare",
            "s1",
            "w1",
            serde_json::json!({
                "purpose": "Test requested file work.",
                "files_planned": ["target"]
            }),
        ))
        .await
        .expect("file intent declaration should complete");
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
    assert_eq!(json["reason_code"], "missing_lease");

    let file_lease = app
        .clone()
        .oneshot(json_request(
            "/v1/lease/acquire",
            serde_json::json!({
                "session_id": "s1",
                "workspace_id": "w1",
                "path": "target"
            }),
        ))
        .await
        .expect("file lease acquire should complete");
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
            "/v1/intent/declare",
            "s1",
            "w2",
            serde_json::json!({
                "purpose": "Test requested work.",
                "files_planned": ["src/auth.ts"]
            }),
        ))
        .await
        .expect("intent declaration should complete");
    assert_eq!(declare_other_workspace.status(), StatusCode::OK);

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
    assert_eq!(lease.status(), StatusCode::BAD_REQUEST);
    let lease_body = response_json(lease, 2048).await;
    assert_eq!(lease_body["reason_code"], "missing_intent");
    assert_eq!(
        lease_body["message"],
        "Lease acquisition requires an active intent covering the requested path."
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
    assert_eq!(json["reason_code"], "missing_intent");

    let declare_same_workspace = app
        .clone()
        .oneshot(protocol_request(
            "/v1/intent/declare",
            "s1",
            "w1",
            serde_json::json!({
                "purpose": "Test requested work.",
                "files_planned": ["src/auth.ts"]
            }),
        ))
        .await
        .expect("intent declaration should complete");
    assert_eq!(declare_same_workspace.status(), StatusCode::OK);

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
async fn active_lease_by_other_session_denies_authorize_even_with_matching_intent() {
    let app = build_router(ServerConfig::new("secret-token"));

    ensure_test_intent_via_http(&app, "s2", "w1", "src/auth.ts").await;
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
                "purpose": "Test requested work.",
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
    let required_next_action = json["required_next_action"].as_str().unwrap_or_default();
    assert!(required_next_action.contains("Do not redeclare intent"));
    assert!(required_next_action.contains("session_id"));
}

#[tokio::test]
async fn queue_on_conflict_without_intent_denies_without_wait_record() {
    let store = Store::open_in_memory().expect("store should open");
    let app = build_router(ServerConfig::with_store("secret-token", store));

    ensure_test_intent_via_http(&app, "s1", "w1", "src/auth.ts").await;
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
    assert_eq!(json["reason_code"], "missing_intent");
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
    assert!(json["reservation"].is_null());
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
async fn queue_on_conflict_out_of_scope_denies_without_wait_record() {
    let store = Store::open_in_memory().expect("store should open");
    let app = build_router(ServerConfig::with_store("secret-token", store));

    ensure_test_intent_via_http(&app, "s1", "w1", "src/session.ts").await;
    let lease = app
        .clone()
        .oneshot(json_request(
            "/v1/lease/acquire",
            serde_json::json!({
                "session_id": "s1",
                "workspace_id": "w1",
                "path": "src/session.ts"
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
                "purpose": "Test requested work.",
                "files_planned": ["src/auth.ts"]
            }),
        ))
        .await
        .expect("intent declaration should complete");
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
    assert_eq!(json["reason_code"], "scope_mismatch");
    assert!(json.get("wait").is_none());

    let release = app
        .clone()
        .oneshot(json_request(
            "/v1/lease/release",
            serde_json::json!({
                "session_id": "s1",
                "workspace_id": "w1",
                "path": "src/session.ts"
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
    assert!(json["reservation"].is_null());
}

#[tokio::test]
async fn queued_conflict_reserves_first_waiter_after_lease_release() {
    let store = Store::open_in_memory().expect("store should open");
    let app = build_router(ServerConfig::with_store("secret-token", store));

    ensure_test_intent_via_http(&app, "s1", "w1", "src/auth.ts").await;
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
                    "purpose": "Test requested work.",
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
    assert_eq!(json["reason_code"], "active_lease_conflict");
    assert_eq!(json["wait"]["status"], "queued");
    assert_eq!(json["wait"]["queue_position"], 1);
    assert_eq!(json["wait"]["blocking_session_id"], "s1");
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
    assert_eq!(json["reservation"]["session_id"], "s2");
    let required_next_action = json["required_next_action"].as_str().unwrap_or_default();
    assert!(required_next_action.contains("Do not redeclare intent"));
    assert!(required_next_action.contains("session_id"));

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
    assert!(required_next_action.contains("state.intent.claim"));

    let claim = app
        .clone()
        .oneshot(protocol_request(
            "/v1/intent/claim",
            "s2",
            "w1",
            serde_json::json!({
                "wait_id": s2_wait_id
            }),
        ))
        .await
        .expect("intent claim should complete");
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
async fn concurrent_codex_sessions_transfer_native_edit_access_through_request_claim_and_lease() {
    let store = Store::open_in_memory().expect("store should open");
    let app = build_router(ServerConfig::with_store("secret-token", store));

    for session_id in ["codex-a", "codex-b", "codex-c"] {
        let register = app
            .clone()
            .oneshot(json_request(
                "/v1/session/register",
                serde_json::json!({
                    "session_id": session_id,
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
            "/v1/intent/declare",
            "codex-a",
            "w1",
            serde_json::json!({
                "purpose": "Test requested work.",
                "files_planned": ["src/auth.ts"]
            }),
        ))
        .await
        .expect("intent declaration should complete");
    assert_eq!(declare_a.status(), StatusCode::OK);

    let lease_a = app
        .clone()
        .oneshot(json_request(
            "/v1/lease/acquire",
            serde_json::json!({
                "session_id": "codex-a",
                "workspace_id": "w1",
                "path": "src/auth.ts"
            }),
        ))
        .await
        .expect("lease acquire should complete");
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

    let request_b = app
        .clone()
        .oneshot(protocol_request(
            "/v1/intent/request",
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
        .expect("intent request should complete");
    assert_eq!(request_b.status(), StatusCode::OK);
    let json = response_json(request_b, 2048).await;
    assert_eq!(json["request_state"], "queued");
    assert_eq!(json["wait"]["queue_position"], 1);
    assert_eq!(json["wait"]["blocking_session_id"], "codex-a");
    let codex_b_wait_id = json["wait"]["wait_id"]
        .as_str()
        .expect("wait id should be present")
        .to_string();

    let request_c = app
        .clone()
        .oneshot(protocol_request(
            "/v1/intent/request",
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
        .expect("intent request should complete");
    assert_eq!(request_c.status(), StatusCode::OK);
    let json = response_json(request_c, 2048).await;
    assert_eq!(json["request_state"], "queued");
    assert_eq!(json["wait"]["queue_position"], 2);
    assert_eq!(json["wait"]["blocking_session_id"], "codex-a");
    let codex_c_wait_id = json["wait"]["wait_id"]
        .as_str()
        .expect("wait id should be present")
        .to_string();

    let finalize_a = app
        .clone()
        .oneshot(json_request(
            "/v1/activity/finalize",
            serde_json::json!({
                "session_id": "codex-a",
                "workspace_id": "w1"
            }),
        ))
        .await
        .expect("activity finalize should complete");
    assert_eq!(finalize_a.status(), StatusCode::OK);
    let json = response_json(finalize_a, 2048).await;
    assert_eq!(json["released_leases"], 1);

    let resume_b = app
        .clone()
        .oneshot(json_request(
            "/v1/resume/next",
            serde_json::json!({
                "session_id": "codex-b",
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
    assert_eq!(json["reservation"]["session_id"], "codex-b");

    let unclaimed_b = app
        .clone()
        .oneshot(native_hook_authorize_request(
            "codex-b",
            "w1",
            "src/auth.ts",
            "apply_patch",
        ))
        .await
        .expect("authorize should complete");
    assert_eq!(unclaimed_b.status(), StatusCode::OK);
    let json = response_json(unclaimed_b, 2048).await;
    assert_eq!(json["decision"], "deny");
    assert_eq!(json["reason_code"], "reservation_claim_required");
    assert_eq!(json["reservation"]["wait_id"], codex_b_wait_id);

    let claim_b = app
        .clone()
        .oneshot(protocol_request(
            "/v1/intent/claim",
            "codex-b",
            "w1",
            serde_json::json!({
                "wait_id": codex_b_wait_id
            }),
        ))
        .await
        .expect("intent claim should complete");
    assert_eq!(claim_b.status(), StatusCode::OK);
    let json = response_json(claim_b, 2048).await;
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
    assert_eq!(json["reason_code"], "active_lease_conflict");

    let finalize_b = app
        .clone()
        .oneshot(json_request(
            "/v1/activity/finalize",
            serde_json::json!({
                "session_id": "codex-b",
                "workspace_id": "w1"
            }),
        ))
        .await
        .expect("activity finalize should complete");
    assert_eq!(finalize_b.status(), StatusCode::OK);
    let json = response_json(finalize_b, 2048).await;
    assert_eq!(json["released_leases"], 1);

    let resume_c = app
        .clone()
        .oneshot(json_request(
            "/v1/resume/next",
            serde_json::json!({
                "session_id": "codex-c",
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
            "/v1/intent/claim",
            "codex-c",
            "w1",
            serde_json::json!({
                "wait_id": codex_c_wait_id
            }),
        ))
        .await
        .expect("intent claim should complete");
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

#[tokio::test]
async fn intent_request_reserves_available_target_but_still_requires_claim() {
    let store = Store::open_in_memory().expect("store should open");
    let app = build_router(ServerConfig::with_store("secret-token", store));

    let requested = app
        .clone()
        .oneshot(protocol_request(
            "/v1/intent/request",
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
        .expect("intent request should complete");
    assert_eq!(requested.status(), StatusCode::OK);
    let body = to_bytes(requested.into_body(), 2048)
        .await
        .expect("body should read");
    let json: serde_json::Value = serde_json::from_slice(&body).expect("body should be json");
    assert_eq!(json["status"], "ok");
    assert_eq!(json["request_state"], "reserved");
    assert_eq!(json["request_id"], "request-1");
    assert_eq!(json["reservation"]["session_id"], "s1");
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
async fn lease_acquire_returns_conflict_for_active_lease_conflict() {
    let store = Store::open_in_memory().expect("store should open");
    let app = build_router(ServerConfig::with_store("secret-token", store));

    ensure_test_intent_via_http(&app, "s1", "w1", "src/auth.ts").await;
    let first = app
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
        .expect("first lease acquire should complete");
    assert_eq!(first.status(), StatusCode::OK);

    ensure_test_intent_via_http(&app, "s2", "w1", "src/auth.ts").await;
    let conflict = app
        .oneshot(json_request(
            "/v1/lease/acquire",
            serde_json::json!({
                "session_id": "s2",
                "workspace_id": "w1",
                "path": "src/auth.ts"
            }),
        ))
        .await
        .expect("conflicting lease acquire should complete");

    assert_eq!(conflict.status(), StatusCode::CONFLICT);
    let json = response_json(conflict, 2048).await;
    assert_eq!(json["status"], "error");
    assert_eq!(json["reason_code"], "lease_conflict");
}

#[tokio::test]
async fn lease_acquire_rejects_active_reservation_conflict_without_breaking_claim() {
    let store = Store::open_in_memory().expect("store should open");
    let app = build_router(ServerConfig::with_store("secret-token", store));

    let requested = app
        .clone()
        .oneshot(protocol_request(
            "/v1/intent/request",
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
        .expect("intent request should complete");
    assert_eq!(requested.status(), StatusCode::OK);
    let json = response_json(requested, 2048).await;
    assert_eq!(json["request_state"], "reserved");
    let wait_id = json["reservation"]["wait_id"]
        .as_str()
        .expect("wait id should be present")
        .to_string();

    ensure_test_intent_via_http(&app, "s2", "w1", "src/auth.ts").await;
    let conflict = app
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
    assert_eq!(conflict.status(), StatusCode::CONFLICT);
    let json = response_json(conflict, 2048).await;
    assert_eq!(json["reason_code"], "lease_conflict");

    let claim = app
        .oneshot(protocol_request(
            "/v1/intent/claim",
            "s1",
            "w1",
            serde_json::json!({
                "wait_id": wait_id
            }),
        ))
        .await
        .expect("intent claim should complete");
    assert_eq!(claim.status(), StatusCode::OK);
    let json = response_json(claim, 2048).await;
    assert_eq!(json["reservation"]["status"], "claimed");
}

#[tokio::test]
async fn intent_request_rejects_empty_purpose() {
    let store = Store::open_in_memory().expect("store should open");
    let app = build_router(ServerConfig::with_store("secret-token", store));

    let requested = app
        .oneshot(protocol_request(
            "/v1/intent/request",
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
        .expect("intent request should complete");

    assert_eq!(requested.status(), StatusCode::BAD_REQUEST);
    let body = to_bytes(requested.into_body(), 1024)
        .await
        .expect("body should read");
    let json: serde_json::Value = serde_json::from_slice(&body).expect("body should be json");
    assert_eq!(json["status"], "error");
    assert_eq!(json["reason_code"], "missing_purpose");
}

#[tokio::test]
async fn intent_request_rejects_normalized_empty_path() {
    let store = Store::open_in_memory().expect("store should open");
    let app = build_router(ServerConfig::with_store("secret-token", store));

    for path in ["./", "../", "/", "a/.."] {
        let requested = app
            .clone()
            .oneshot(protocol_request(
                "/v1/intent/request",
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
            .expect("intent request should complete");

        assert_eq!(requested.status(), StatusCode::BAD_REQUEST);
        let json = response_json(requested, 1024).await;
        assert_eq!(json["status"], "error");
        assert_eq!(json["reason_code"], "missing_scope");
        assert_eq!(
            json["message"],
            "Intent scope paths must be non-empty after normalization."
        );
    }
}

#[tokio::test]
async fn intent_request_queues_conflict_and_reuses_request_id() {
    let store = Store::open_in_memory().expect("store should open");
    let app = build_router(ServerConfig::with_store("secret-token", store));

    ensure_test_intent_via_http(&app, "s1", "w1", "src/auth.ts").await;
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
            "/v1/intent/request",
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
        .expect("intent request should complete");
    assert_eq!(first.status(), StatusCode::OK);
    let body = to_bytes(first.into_body(), 2048)
        .await
        .expect("body should read");
    let json: serde_json::Value = serde_json::from_slice(&body).expect("body should be json");
    assert_eq!(json["request_state"], "queued");
    assert_eq!(json["request_id"], "request-2");
    assert_eq!(json["wait"]["status"], "queued");
    assert_eq!(json["wait"]["queue_position"], 1);
    assert_eq!(json["wait"]["blocking_session_id"], "s1");
    assert_eq!(json["wait"]["purpose"], "Queue s2 auth changes.");
    let first_wait_id = json["wait"]["wait_id"]
        .as_str()
        .expect("wait id should be present")
        .to_string();

    let repeated = app
        .clone()
        .oneshot(protocol_request(
            "/v1/intent/request",
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
        .expect("intent request retry should complete");
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
            "/v1/intent/request",
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
        .expect("second intent request should complete");
    assert_eq!(second.status(), StatusCode::OK);
    let body = to_bytes(second.into_body(), 2048)
        .await
        .expect("body should read");
    let json: serde_json::Value = serde_json::from_slice(&body).expect("body should be json");
    assert_eq!(json["request_state"], "queued");
    assert_eq!(json["wait"]["queue_position"], 2);
}

#[tokio::test]
async fn intent_cancel_cancels_reserved_request_and_promotes_next_waiter() {
    let store = Store::open_in_memory().expect("store should open");
    let app = build_router(ServerConfig::with_store("secret-token", store));

    ensure_test_intent_via_http(&app, "s1", "w1", "src/auth.ts").await;
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

    for (session_id, request_id) in [("s2", "request-2"), ("s3", "request-3")] {
        let queued = app
            .clone()
            .oneshot(protocol_request(
                "/v1/intent/request",
                session_id,
                "w1",
                serde_json::json!({
                    "request_id": request_id,
                    "action": "write_file",
                    "path": "src/auth.ts",
                    "purpose": format!("Queue {session_id} auth changes.")
                }),
            ))
            .await
            .expect("intent request should complete");
        assert_eq!(queued.status(), StatusCode::OK);
    }

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

    let canceled = app
        .clone()
        .oneshot(protocol_request(
            "/v1/intent/cancel",
            "s2",
            "w1",
            serde_json::json!({
                "request_id": "request-2"
            }),
        ))
        .await
        .expect("intent cancel should complete");
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
                "session_id": "s3",
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
    assert_eq!(json["reservation"]["session_id"], "s3");
    assert_eq!(json["reservation"]["relative_path"], "src/auth.ts");
}

#[tokio::test]
async fn intent_cancel_rejects_other_session_request() {
    let store = Store::open_in_memory().expect("store should open");
    let app = build_router(ServerConfig::with_store("secret-token", store));

    let requested = app
        .clone()
        .oneshot(protocol_request(
            "/v1/intent/request",
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
        .expect("intent request should complete");
    assert_eq!(requested.status(), StatusCode::OK);

    let rejected = app
        .oneshot(protocol_request(
            "/v1/intent/cancel",
            "s3",
            "w1",
            serde_json::json!({
                "request_id": "request-2"
            }),
        ))
        .await
        .expect("intent cancel should complete");
    assert_eq!(rejected.status(), StatusCode::CONFLICT);
    let body = to_bytes(rejected.into_body(), 2048)
        .await
        .expect("body should read");
    let json: serde_json::Value = serde_json::from_slice(&body).expect("body should be json");
    assert_eq!(json["status"], "error");
    assert_eq!(json["reason_code"], "cancel_failed");
}

#[tokio::test]
async fn activity_finalize_releases_leases_and_notifications_poll_returns_resume_signal() {
    let store = Store::open_in_memory().expect("store should open");
    let app = build_router(ServerConfig::with_store("secret-token", store));

    ensure_test_intent_via_http(&app, "s1", "w1", "src/auth.ts").await;
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
                "purpose": "Test requested work.",
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

    ensure_test_intent_via_http(&app, "s1", "w1", "src/auth.ts").await;
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
                "purpose": "Test requested work.",
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
        "Reread the target, then call state.intent.claim for the reservation before writing."
    );
}

#[tokio::test]
async fn active_lease_by_same_session_allows_matching_authorize() {
    let app = build_router(ServerConfig::new("secret-token"));

    ensure_test_intent_via_http(&app, "s1", "w1", "src/auth.ts").await;
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
                "purpose": "Test requested work.",
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
async fn rename_file_denies_when_other_session_leases_destination() {
    let store = Store::open_in_memory().expect("store should open");
    store
        .append(Event::intent_declared(
            "s1",
            "w1",
            "Test rename authorization behavior.",
            ["src/old.ts", "src/new.ts"],
        ))
        .expect("intent should append");
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
    assert_eq!(json["reason_code"], "active_lease_conflict");
    assert!(json.get("wait").is_none());
}

#[tokio::test]
async fn rename_file_denies_when_other_session_leases_source() {
    let store = Store::open_in_memory().expect("store should open");
    store
        .append(Event::intent_declared(
            "s1",
            "w1",
            "Test rename authorization behavior.",
            ["src/old.ts", "src/new.ts"],
        ))
        .expect("intent should append");
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
    assert_eq!(json["reason_code"], "active_lease_conflict");
}

#[tokio::test]
async fn rename_file_denies_when_other_session_reserves_destination() {
    let store = Store::open_in_memory().expect("store should open");
    store
        .append(Event::intent_declared(
            "s1",
            "w1",
            "Test rename authorization behavior.",
            ["src/old.ts", "src/new.ts"],
        ))
        .expect("intent should append");
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
        .append(Event::intent_declared(
            "s1",
            "w1",
            "Test rename authorization behavior.",
            ["src/old.ts", "src/new.ts"],
        ))
        .expect("intent should append");
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
            "/v1/intent/declare",
            "s1",
            "w1",
            serde_json::json!({
                "purpose": "Test requested work.",
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
async fn write_directory_action_requires_exact_directory_intent_over_http() {
    let app = build_router(ServerConfig::new("secret-token"));

    let declare = app
        .clone()
        .oneshot(protocol_request(
            "/v1/intent/declare",
            "s1",
            "w1",
            serde_json::json!({
                "purpose": "Test requested work.",
                "files_planned": ["target/"]
            }),
        ))
        .await
        .expect("intent declaration should complete");
    assert_eq!(declare.status(), StatusCode::OK);

    ensure_test_intent_via_http(&app, "s1", "w1", "target/").await;
    let lease = app
        .clone()
        .oneshot(json_request(
            "/v1/lease/acquire",
            serde_json::json!({
                "session_id": "s1",
                "workspace_id": "w1",
                "path": "target/"
            }),
        ))
        .await
        .expect("lease acquire should complete");
    assert_eq!(lease.status(), StatusCode::OK);

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

    ensure_test_intent_via_http(&app, "s2", "w1", "target/out.txt").await;
    let lease = app
        .clone()
        .oneshot(json_request(
            "/v1/lease/acquire",
            serde_json::json!({
                "session_id": "s2",
                "workspace_id": "w1",
                "path": "target/out.txt"
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
                "purpose": "Test requested work.",
                "files_planned": ["target/"]
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
    assert_eq!(json["reason_code"], "active_lease_conflict");
}

#[tokio::test]
async fn write_file_action_denies_when_ancestor_directory_has_other_session_lease() {
    let app = build_router(ServerConfig::new("secret-token"));

    ensure_test_intent_via_http(&app, "s2", "w1", "target/").await;
    let lease = app
        .clone()
        .oneshot(json_request(
            "/v1/lease/acquire",
            serde_json::json!({
                "session_id": "s2",
                "workspace_id": "w1",
                "path": "target/"
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
                "purpose": "Test requested work.",
                "files_planned": ["target/out.txt"]
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
    assert_eq!(json["reason_code"], "active_lease_conflict");
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
            "/v1/intent/declare",
            "s1",
            "w1",
            serde_json::json!({
                "purpose": "Test requested work.",
                "files_planned": ["target/out.txt"]
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
    assert_eq!(json["reservation"]["session_id"], "s2");
    assert_eq!(json["reservation"]["relative_path"], "target");
}

#[tokio::test]
async fn write_file_action_requires_claim_when_same_session_has_ancestor_directory_reservation() {
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
            "/v1/intent/declare",
            "s2",
            "w1",
            serde_json::json!({
                "purpose": "Test requested work.",
                "files_planned": ["target/out.txt"]
            }),
        ))
        .await
        .expect("intent declaration should complete");
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

    ensure_test_intent_via_http(&app, "s2", "w1", "target/out.txt").await;
    let lease = app
        .clone()
        .oneshot(json_request(
            "/v1/lease/acquire",
            serde_json::json!({
                "session_id": "s2",
                "workspace_id": "w1",
                "path": "target/out.txt"
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
                "purpose": "Test requested work.",
                "files_planned": ["target/"]
            }),
        ))
        .await
        .expect("intent declaration should complete");
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
    assert_eq!(json["reason_code"], "active_lease_conflict");
    assert_eq!(json["wait"]["status"], "queued");

    let wait_id = json["wait"]["wait_id"]
        .as_str()
        .expect("wait id should be present")
        .to_string();
    let release = app
        .clone()
        .oneshot(json_request(
            "/v1/lease/release",
            serde_json::json!({
                "session_id": "s2",
                "workspace_id": "w1",
                "path": "target/out.txt"
            }),
        ))
        .await
        .expect("lease release should complete");
    assert_eq!(release.status(), StatusCode::OK);

    let resume = app
        .clone()
        .oneshot(json_request(
            "/v1/resume/next",
            serde_json::json!({
                "session_id": "s1",
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
    assert_eq!(json["reservation"]["session_id"], "s1");
    assert_eq!(json["reservation"]["relative_path"], "target");
    assert_eq!(json["reservation"]["action"], "write_directory");

    let claim = app
        .clone()
        .oneshot(protocol_request(
            "/v1/intent/claim",
            "s1",
            "w1",
            serde_json::json!({
                "wait_id": wait_id
            }),
        ))
        .await
        .expect("intent claim should complete");
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

    ensure_test_intent_via_http(&app, "s2", "w1", "target/").await;
    let lease = app
        .clone()
        .oneshot(json_request(
            "/v1/lease/acquire",
            serde_json::json!({
                "session_id": "s2",
                "workspace_id": "w1",
                "path": "target/"
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
                "purpose": "Test requested work.",
                "files_planned": ["target/out.txt"]
            }),
        ))
        .await
        .expect("intent declaration should complete");
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
    assert_eq!(json["reason_code"], "active_lease_conflict");
    assert_eq!(json["wait"]["status"], "queued");

    let release = app
        .clone()
        .oneshot(json_request(
            "/v1/lease/release",
            serde_json::json!({
                "session_id": "s2",
                "workspace_id": "w1",
                "path": "target/"
            }),
        ))
        .await
        .expect("lease release should complete");
    assert_eq!(release.status(), StatusCode::OK);

    let resume = app
        .oneshot(json_request(
            "/v1/resume/next",
            serde_json::json!({
                "session_id": "s1",
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
    assert_eq!(json["reservation"]["session_id"], "s1");
    assert_eq!(json["reservation"]["relative_path"], "target/out.txt");
    assert_eq!(json["reservation"]["action"], "write_file");
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
                "purpose": "Test requested work.",
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
    assert_eq!(json["items"][0]["kind"], "intent");
    assert_eq!(json["items"][0]["resource"], "src/auth.ts");
    assert_eq!(json["items"][0]["purpose"], "Test requested work.");
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
                "purpose": "Test requested work.",
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
        .append(Event::intent_declared(
            "s1",
            "w1",
            "Test supplied sqlite store authorization.",
            ["src/auth.ts"],
        ))
        .expect("intent should append");
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
            "/v1/intent/declare",
            "s1",
            "w1",
            serde_json::json!({
                "purpose": "Fix auth validation behavior.",
                "files_planned": ["src/auth.ts"]
            }),
        ))
        .await
        .expect("intent declaration should complete");
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
async fn context_render_filters_items_to_requested_workspace() {
    let app = build_router(ServerConfig::new("secret-token"));

    let declare_w1 = app
        .clone()
        .oneshot(protocol_request(
            "/v1/intent/declare",
            "s1",
            "w1",
            serde_json::json!({
                "purpose": "Fix auth validation behavior.",
                "files_planned": ["src/auth.ts"]
            }),
        ))
        .await
        .expect("w1 intent declaration should complete");
    assert_eq!(declare_w1.status(), StatusCode::OK);

    let declare_w2 = app
        .clone()
        .oneshot(protocol_request(
            "/v1/intent/declare",
            "s2",
            "w2",
            serde_json::json!({
                "purpose": "Update public docs from a separate workspace.",
                "files_planned": ["src/auth.ts"]
            }),
        ))
        .await
        .expect("w2 intent declaration should complete");
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
    assert_eq!(json["current"]["session_count"], 0);
    assert_eq!(json["current"]["active_intent_count"], 1);
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
            "/v1/intent/declare",
            "s1",
            "shared",
            serde_json::json!({
                "purpose": "Fix stateful core LAN behavior.",
                "files_planned": ["crates/stateful-cli/src/lan.rs"]
            }),
        ))
        .await
        .expect("repo-1 intent declaration should complete");
    assert_eq!(declare_repo_1.status(), StatusCode::OK);

    let mut repo_2_body = protocol_body(
        "s2",
        "shared",
        serde_json::json!({
            "purpose": "Investigate edge camera framedrops.",
            "files_planned": ["record/hw/vision_module.py"]
        }),
    );
    repo_2_body["workspace"]["root"] = serde_json::json!("/Users/arthur/Code/edge/core");
    repo_2_body["workspace"]["repo_id"] = serde_json::json!("repo-2");
    repo_2_body["workspace"]["worktree_id"] = serde_json::json!("worktree-2");
    repo_2_body["workspace"]["branch"] = serde_json::json!("frame-drops");

    let declare_repo_2 = app
        .clone()
        .oneshot(json_request("/v1/intent/declare", repo_2_body))
        .await
        .expect("repo-2 intent declaration should complete");
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
            "/v1/intent/request",
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
        .expect("intent request should complete");
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
        item["kind"] == "reservation" && item["purpose"] == "Claim queued auth update."
    }));

    let claim = app
        .clone()
        .oneshot(protocol_request(
            "/v1/intent/claim",
            "s1",
            "shared",
            serde_json::json!({
                "wait_id": wait_id
            }),
        ))
        .await
        .expect("intent claim should complete");
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
        item["kind"] == "intent" && item["purpose"] == "Claim queued auth update."
    }));
    assert!(
        items
            .iter()
            .any(|item| item["kind"] == "lease" && item["purpose"] == "Claim queued auth update.")
    );
}

#[tokio::test]
async fn intent_request_retry_backfills_identity_for_filtered_context_render() {
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
        .oneshot(json_request("/v1/intent/request", missing_identity))
        .await
        .expect("initial intent request should complete");
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
            "/v1/intent/request",
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
        .expect("retry intent request should complete");
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
        item["kind"] == "reservation" && item["purpose"] == "Backfill queued auth identity."
    }));
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

fn native_hook_authorize_request(
    session_id: &str,
    workspace_id: &str,
    path: &str,
    tool_name: &str,
) -> Request<Body> {
    let mut body = protocol_body(
        session_id,
        workspace_id,
        serde_json::json!({
            "action": "write_file",
            "path": path
        }),
    );
    body["source"]["kind"] = serde_json::json!("hook");
    body["source"]["event"] = serde_json::json!("pre_tool_use");
    body["source"]["source_ref"] = serde_json::json!(format!("hook:{session_id}:{tool_name}"));
    body["source"]["tool_name"] = serde_json::json!(tool_name);

    json_request("/v1/authorize", body)
}

async fn response_json(response: Response<Body>, limit: usize) -> serde_json::Value {
    let body = to_bytes(response.into_body(), limit)
        .await
        .expect("body should read");
    serde_json::from_slice(&body).expect("body should be json")
}

fn authorized_get(path: &str) -> Request<Body> {
    Request::builder()
        .method("GET")
        .uri(path)
        .header("authorization", "Bearer secret-token")
        .body(Body::empty())
        .expect("get request should build")
}
