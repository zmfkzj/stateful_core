mod support;

use axum::http::StatusCode;
use serde_json::json;
use stateful_server::{CoordinationMode, ServerConfig, build_router};
use tower::ServiceExt;

use support::{app, envelope, envelope_for, post, response_json, successful_post};

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
        ServerConfig::new("test-token").with_coordination_mode(CoordinationMode::Enforcement),
    )
    .oneshot(post("/v2/authorize", body))
    .await
    .unwrap();
    assert_eq!(enforcement.status(), StatusCode::FORBIDDEN);
    assert_eq!(response_json(enforcement).await["reason_code"], "missing_read_provenance");
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
        "/v2/human/observe",
        envelope_for("agent-2", "00000000-0000-4000-8000-00000000d110", json!({
            "relative_path": "src/lib.rs", "kind": "save", "confidence": "high",
            "source": "watcher", "summary": "human save"
        })),
    )
    .await;
    let reservation = successful_post(
        &app,
        "/v2/reservation/declare",
        envelope_for("agent-1", "00000000-0000-4000-8000-00000000d105", json!({
            "scopes": [{"kind": "file", "path": "src/lib.rs"}],
            "action": "write_file",
            "purpose": "Update the module."
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
            "reservation_id": reservation_id, "decision": "adopt", "files_reread": ["src/lib.rs"], "human_change_summary": "adopted"
        })),
    )
    .await;

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
        .oneshot(support::query_get("/v2/current", "agent-1", "00000000-0000-4000-8000-00000000d122", "workspace-1"))
        .await
        .unwrap();
    assert_eq!(current.status(), StatusCode::OK);
    let events = app
        .clone()
        .oneshot(support::query_get("/v2/events", "agent-1", "00000000-0000-4000-8000-00000000d123", "workspace-1"))
        .await
        .unwrap();
    assert_eq!(events.status(), StatusCode::OK);
    assert!(response_json(events).await["events"].as_array().expect("events array").len() > 1);
    assert_eq!(
        app.oneshot(support::query_get(
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
async fn mixed_scope_reservation_authorizes_file_write_with_directory_claim_label() {
    let app = app();
    let reservation = successful_post(
        &app,
        "/v2/reservation/declare",
        envelope_for("agent-1", "00000000-0000-4000-8000-00000000d125", json!({
            "scopes": [
                {"kind": "directory", "path": "src"},
                {"kind": "file", "path": "src/lib.rs"}
            ],
            "action": "write_directory",
            "purpose": "Update the directory and its module."
        })),
    )
    .await;
    let reservation_id = reservation["reservation_id"].as_str().expect("reservation id");

    successful_post(
        &app,
        "/v2/claim/acquire",
        envelope_for("agent-1", "00000000-0000-4000-8000-00000000d126", json!({
            "reservation_id": reservation_id,
            "paths": [{"relative_path": "src/lib.rs", "observation": {"exists": false}}]
        })),
    )
    .await;
    successful_post(
        &app,
        "/v2/read/start",
        envelope_for("agent-1", "00000000-0000-4000-8000-00000000d127", json!({
            "operation_id": "read-lib", "path": "src/lib.rs",
            "before": {"exists": false, "byte_len": 0}
        })),
    )
    .await;
    successful_post(
        &app,
        "/v2/read/complete",
        envelope_for("agent-1", "00000000-0000-4000-8000-00000000d128", json!({
            "operation_id": "read-lib", "path": "src/lib.rs", "classification": "exact",
            "after": {"exists": false, "byte_len": 0}
        })),
    )
    .await;

    let response = successful_post(
        &app,
        "/v2/authorize",
        envelope_for("agent-1", "00000000-0000-4000-8000-00000000d129", json!({
            "reservation_id": reservation_id,
            "operation_id": "write-lib",
            "action": "write_file",
            "targets": [{"path": "src/lib.rs", "before": {"exists": false, "byte_len": 0}}]
        })),
    )
    .await;

    assert_eq!(response["decision"]["reason_code"], "authorized");
    assert_eq!(response["decision"]["decision"], "allow");
}
