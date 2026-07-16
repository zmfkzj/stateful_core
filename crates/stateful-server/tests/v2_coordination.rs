mod support;

use axum::http::StatusCode;
use serde_json::json;
use stateful_server::{CoordinationMode, ServerConfig, build_router};
use stateful_store::{MutableClock, Store};
use std::time::Duration;
use tower::ServiceExt;

use support::{app, envelope, envelope_for, post, response_json, successful_post};

#[tokio::test]
async fn awareness_warns_for_missing_read_provenance_while_enforcement_denies() {
    let body = envelope(json!({
        "operation_id": "write-1",
        "action": "write_file",
        "targets": [{"path": "src/lib.rs", "before": {"exists": false, "byte_len": 0}}]
    }));
    let awareness_app = app();
    let awareness = awareness_app.clone().oneshot(post("/v2/authorize", body.clone())).await.unwrap();
    assert_eq!(awareness.status(), StatusCode::OK);
    let awareness = response_json(awareness).await;
    assert!(awareness["intent_id"].is_string());
    assert_eq!(awareness["decision"]["decision"], "warn");
    assert_eq!(awareness["decision"]["reason_code"], "missing_read_provenance");
    assert_eq!(
        response_json(
            awareness_app
                .oneshot(post("/v2/authorize", body.clone()))
                .await
                .expect("warning duplicate responds"),
        )
        .await,
        awareness,
    );

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
async fn committed_write_requires_a_new_exact_read_before_a_second_enforced_authorization() {
    let app = build_router(
        ServerConfig::new("test-token").with_coordination_mode(CoordinationMode::Enforcement),
    );
    let reservation = successful_post(
        &app,
        "/v2/reservation/declare",
        envelope_for("agent-1", "00000000-0000-4000-8000-00000000d201", json!({
            "scopes": [{"kind": "file", "path": "src/lib.rs"}],
            "action": "write_file",
            "purpose": "Update the module."
        })),
    )
    .await;
    let reservation_id = reservation["reservation_id"].as_str().expect("reservation id").to_owned();
    successful_post(
        &app,
        "/v2/claim/acquire",
        envelope_for("agent-1", "00000000-0000-4000-8000-00000000d202", json!({
            "reservation_id": reservation_id,
            "paths": [{"relative_path": "src/lib.rs", "observation": {"exists": false}}]
        })),
    )
    .await;
    successful_post(
        &app,
        "/v2/read/start",
        envelope_for("agent-1", "00000000-0000-4000-8000-00000000d203", json!({
            "operation_id": "read-1", "path": "src/lib.rs",
            "before": {"exists": false, "byte_len": 0}
        })),
    )
    .await;
    successful_post(
        &app,
        "/v2/read/complete",
        envelope_for("agent-1", "00000000-0000-4000-8000-00000000d204", json!({
            "operation_id": "read-1", "path": "src/lib.rs", "classification": "exact",
            "after": {"exists": false, "byte_len": 0}
        })),
    )
    .await;
    let first = successful_post(
        &app,
        "/v2/authorize",
        envelope_for("agent-1", "00000000-0000-4000-8000-00000000d205", json!({
            "reservation_id": reservation_id,
            "operation_id": "write-1",
            "action": "write_file",
            "targets": [{"path": "src/lib.rs", "before": {"exists": false, "byte_len": 0}}]
        })),
    )
    .await;
    successful_post(
        &app,
        "/v2/write/complete",
        envelope_for("agent-1", "00000000-0000-4000-8000-00000000d206", json!({
            "intent_id": first["intent_id"],
            "outcome": "committed",
            "post_fingerprints": [["src/lib.rs", {"exists": false, "byte_len": 0}]]
        })),
    )
    .await;

    let second = app
        .oneshot(post(
            "/v2/authorize",
            envelope_for("agent-1", "00000000-0000-4000-8000-00000000d207", json!({
                "reservation_id": reservation_id,
                "operation_id": "write-2",
                "action": "write_file",
                "targets": [{"path": "src/lib.rs", "before": {"exists": false, "byte_len": 0}}]
            })),
        ))
        .await
        .expect("second authorization responds");
    assert_eq!(second.status(), StatusCode::FORBIDDEN);
    assert_eq!(response_json(second).await["reason_code"], "stale_observation");
}

#[tokio::test]
async fn awareness_treats_expired_partial_and_structural_reads_as_missing_evidence() {
    let clock = MutableClock::from_system_now();
    let expired_app = build_router(ServerConfig::with_store(
        "test-token",
        Store::open_in_memory_with_clock(clock.clone()).expect("store opens"),
    ));
    successful_post(
        &expired_app,
        "/v2/read/start",
        envelope_for("agent-1", "00000000-0000-4000-8000-00000000d221", json!({
            "operation_id": "expired-read", "path": "src/expired.rs",
            "before": {"exists": false, "byte_len": 0}
        })),
    )
    .await;
    successful_post(
        &expired_app,
        "/v2/read/complete",
        envelope_for("agent-1", "00000000-0000-4000-8000-00000000d222", json!({
            "operation_id": "expired-read", "path": "src/expired.rs", "classification": "exact",
            "after": {"exists": false, "byte_len": 0}
        })),
    )
    .await;
    clock.advance(Duration::from_secs(61 * 60));
    let expired = expired_app
        .oneshot(post(
            "/v2/authorize",
            envelope_for("agent-1", "00000000-0000-4000-8000-00000000d223", json!({
                "operation_id": "write-expired", "action": "write_file",
                "targets": [{"path": "src/expired.rs", "before": {"exists": false, "byte_len": 0}}]
            })),
        ))
        .await
        .expect("expired authorization responds");
    assert_eq!(expired.status(), StatusCode::OK);
    assert_eq!(response_json(expired).await["decision"]["reason_code"], "missing_read_provenance");

    for (classification, path, start_id, complete_id, authorize_id) in [
        ("partial", "src/partial.rs", "00000000-0000-4000-8000-00000000d224", "00000000-0000-4000-8000-00000000d225", "00000000-0000-4000-8000-00000000d226"),
        ("structural_summary", "src/structural.rs", "00000000-0000-4000-8000-00000000d227", "00000000-0000-4000-8000-00000000d228", "00000000-0000-4000-8000-00000000d229"),
    ] {
        let app = app();
        successful_post(
            &app,
            "/v2/read/start",
            envelope_for("agent-1", start_id, json!({
                "operation_id": format!("read-{classification}"), "path": path,
                "before": {"exists": false, "byte_len": 0}
            })),
        )
        .await;
        successful_post(
            &app,
            "/v2/read/complete",
            envelope_for("agent-1", complete_id, json!({
                "operation_id": format!("read-{classification}"), "path": path,
                "classification": classification, "after": {"exists": false, "byte_len": 0}
            })),
        )
        .await;
        let response = app
            .oneshot(post(
                "/v2/authorize",
                envelope_for("agent-1", authorize_id, json!({
                    "operation_id": format!("write-{classification}"), "action": "write_file",
                    "targets": [{"path": path, "before": {"exists": false, "byte_len": 0}}]
                })),
            ))
            .await
            .expect("incomplete read authorization responds");
        assert_eq!(response.status(), StatusCode::OK, "{classification}");
        assert_eq!(
            response_json(response).await["decision"]["reason_code"],
            "missing_read_provenance",
            "{classification}",
        );
    }
}

#[tokio::test]
async fn awareness_treats_invalidated_peer_reads_as_missing_evidence() {
    let app = app();
    let first_reservation = successful_post(
        &app,
        "/v2/reservation/declare",
        envelope_for("agent-1", "00000000-0000-4000-8000-00000000d231", json!({
            "scopes": [{"kind": "file", "path": "src/lib.rs"}],
            "action": "write_file", "purpose": "Update the module."
        })),
    )
    .await;
    let first_reservation_id = first_reservation["reservation_id"].as_str().expect("reservation id").to_owned();
    successful_post(
        &app,
        "/v2/claim/acquire",
        envelope_for("agent-1", "00000000-0000-4000-8000-00000000d232", json!({
            "reservation_id": first_reservation_id,
            "paths": [{"relative_path": "src/lib.rs", "observation": {"exists": false}}]
        })),
    )
    .await;
    successful_post(
        &app,
        "/v2/read/start",
        envelope_for("agent-1", "00000000-0000-4000-8000-00000000d233", json!({
            "operation_id": "first-read", "path": "src/lib.rs",
            "before": {"exists": false, "byte_len": 0}
        })),
    )
    .await;
    successful_post(
        &app,
        "/v2/read/complete",
        envelope_for("agent-1", "00000000-0000-4000-8000-00000000d234", json!({
            "operation_id": "first-read", "path": "src/lib.rs", "classification": "exact",
            "after": {"exists": false, "byte_len": 0}
        })),
    )
    .await;

    let second_reservation = successful_post(
        &app,
        "/v2/reservation/declare",
        envelope_for("agent-2", "00000000-0000-4000-8000-00000000d235", json!({
            "scopes": [{"kind": "file", "path": "src/lib.rs"}],
            "action": "write_file", "purpose": "Coordinate the module update."
        })),
    )
    .await;
    let second_reservation_id = second_reservation["reservation_id"].as_str().expect("reservation id").to_owned();
    successful_post(
        &app,
        "/v2/claim/acquire",
        envelope_for("agent-2", "00000000-0000-4000-8000-00000000d236", json!({
            "reservation_id": second_reservation_id,
            "paths": [{"relative_path": "src/lib.rs", "observation": {"exists": false}}]
        })),
    )
    .await;
    successful_post(
        &app,
        "/v2/read/start",
        envelope_for("agent-2", "00000000-0000-4000-8000-00000000d237", json!({
            "operation_id": "second-read", "path": "src/lib.rs",
            "before": {"exists": false, "byte_len": 0}
        })),
    )
    .await;
    successful_post(
        &app,
        "/v2/read/complete",
        envelope_for("agent-2", "00000000-0000-4000-8000-00000000d238", json!({
            "operation_id": "second-read", "path": "src/lib.rs", "classification": "exact",
            "after": {"exists": false, "byte_len": 0}
        })),
    )
    .await;
    let second_intent = successful_post(
        &app,
        "/v2/authorize",
        envelope_for("agent-2", "00000000-0000-4000-8000-00000000d239", json!({
            "reservation_id": second_reservation_id,
            "operation_id": "second-write", "action": "write_file",
            "targets": [{"path": "src/lib.rs", "before": {"exists": false, "byte_len": 0}}]
        })),
    )
    .await;
    successful_post(
        &app,
        "/v2/write/complete",
        envelope_for("agent-2", "00000000-0000-4000-8000-00000000d240", json!({
            "intent_id": second_intent["intent_id"], "outcome": "committed",
            "post_fingerprints": [["src/lib.rs", {"exists": false, "byte_len": 0}]]
        })),
    )
    .await;

    let response = app
        .oneshot(post(
            "/v2/authorize",
            envelope_for("agent-1", "00000000-0000-4000-8000-00000000d241", json!({
                "reservation_id": first_reservation_id,
                "operation_id": "first-write", "action": "write_file",
                "targets": [{"path": "src/lib.rs", "before": {"exists": false, "byte_len": 0}}]
            })),
        ))
        .await
        .expect("invalidated peer read responds");
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response_json(response).await["decision"]["reason_code"],
        "missing_read_provenance",
    );
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

    let authorization = envelope_for("agent-1", "00000000-0000-4000-8000-00000000d129", json!({
        "reservation_id": reservation_id,
        "operation_id": "write-lib",
        "action": "write_file",
        "targets": [{"path": "src/lib.rs", "before": {"exists": false, "byte_len": 0}}]
    }));
    let response = successful_post(&app, "/v2/authorize", authorization.clone()).await;

    assert_eq!(response["decision"]["reason_code"], "authorized");
    assert_eq!(response["decision"]["decision"], "allow");
    assert_eq!(
        successful_post(&app, "/v2/authorize", authorization).await,
        response,
    );
}

#[tokio::test]
async fn action_validation_precedes_thin_freshness_policy() {
    let app = build_router(
        ServerConfig::new("test-token").with_coordination_mode(CoordinationMode::Enforcement),
    );
    let response = app
        .oneshot(post(
            "/v2/authorize",
            envelope_for("agent-1", "00000000-0000-4000-8000-00000000d400", json!({
                "operation_id": "write-invalid",
                "action": "unsupported_write",
                "targets": [{"path": "src/lib.rs", "before": {"exists": false, "byte_len": 0}}]
            })),
        ))
        .await
        .expect("invalid action responds");
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    assert_eq!(response_json(response).await["reason_code"], "invalid_write_action");
}

#[tokio::test]
async fn denied_authorization_is_frozen_audited_and_rejects_reused_actor_uuid() {
    let app = build_router(
        ServerConfig::new("test-token").with_coordination_mode(CoordinationMode::Enforcement),
    );
    let reservation = successful_post(
        &app,
        "/v2/reservation/declare",
        envelope_for("agent-1", "00000000-0000-4000-8000-00000000d401", json!({
            "scopes": [{"kind": "file", "path": "src/lib.rs"}],
            "action": "write_file",
            "purpose": "Update the module."
        })),
    )
    .await;
    let reservation_id = reservation["reservation_id"].as_str().expect("reservation id").to_owned();
    successful_post(
        &app,
        "/v2/read/start",
        envelope_for("agent-1", "00000000-0000-4000-8000-00000000d402", json!({
            "operation_id": "read-1", "path": "src/lib.rs",
            "before": {"exists": false, "byte_len": 0}
        })),
    )
    .await;
    successful_post(
        &app,
        "/v2/read/complete",
        envelope_for("agent-1", "00000000-0000-4000-8000-00000000d403", json!({
            "operation_id": "read-1", "path": "src/lib.rs", "classification": "exact",
            "after": {"exists": false, "byte_len": 0}
        })),
    )
    .await;
    let authorization = envelope_for("agent-1", "00000000-0000-4000-8000-00000000d404", json!({
        "reservation_id": reservation_id,
        "operation_id": "write-1",
        "action": "unsupported_write",
        "targets": [{"path": "src/lib.rs", "before": {"exists": false, "byte_len": 0}}]
    }));
    let first = app
        .clone()
        .oneshot(post("/v2/authorize", authorization.clone()))
        .await
        .expect("initial authorization responds");
    assert_eq!(first.status(), StatusCode::FORBIDDEN);
    let first_body = response_json(first).await;
    assert_eq!(first_body["reason_code"], "invalid_write_action");

    successful_post(
        &app,
        "/v2/claim/acquire",
        envelope_for("agent-1", "00000000-0000-4000-8000-00000000d405", json!({
            "reservation_id": reservation_id,
            "paths": [{"relative_path": "src/lib.rs", "observation": {"exists": false}}]
        })),
    )
    .await;

    let retry = app
        .clone()
        .oneshot(post("/v2/authorize", authorization))
        .await
        .expect("frozen authorization responds");
    assert_eq!(retry.status(), StatusCode::FORBIDDEN);
    assert_eq!(response_json(retry).await, first_body);

    let reused = app
        .clone()
        .oneshot(post(
            "/v2/authorize",
            envelope_for("agent-2", "00000000-0000-4000-8000-00000000d404", json!({
                "reservation_id": reservation_id,
                "operation_id": "write-1",
                "action": "unsupported_write",
                "targets": [{"path": "src/lib.rs", "before": {"exists": false, "byte_len": 0}}]
            })),
        ))
        .await
        .expect("reused actor UUID responds");
    assert_eq!(reused.status(), StatusCode::CONFLICT);
    assert_eq!(response_json(reused).await["error"]["code"], "idempotency_key_reused");

    let events = app
        .oneshot(support::query_get(
            "/v2/events",
            "agent-1",
            "00000000-0000-4000-8000-00000000d406",
            "workspace-1",
        ))
        .await
        .expect("event audit responds");
    let events = response_json(events).await["events"].as_array().expect("event list").clone();
    let denials = events.iter().filter(|event| event["event_type"] == "authorization.denied").collect::<Vec<_>>();
    assert_eq!(denials.len(), 1);
    assert_eq!(
        denials[0]["payload"]["event"]["data"]["data"]["reason_code"],
        "invalid_write_action",
    );
    assert_eq!(denials[0]["payload"]["event"]["data"]["data"]["decision"], "deny");
    assert_eq!(
        events.iter().filter(|event| event["event_type"] == "write_intent.started").count(),
        0,
    );
}

#[tokio::test]
async fn awareness_persists_overlapping_reservation_and_claim_warnings() {
    let app = app();
    let first_reservation = successful_post(
        &app,
        "/v2/reservation/declare",
        envelope_for("agent-1", "00000000-0000-4000-8000-00000000d411", json!({
            "scopes": [{"kind": "file", "path": "src/lib.rs"}],
            "action": "write_file",
            "purpose": "Update the module."
        })),
    )
    .await;
    let second_reservation_response = app
        .clone()
        .oneshot(post(
            "/v2/reservation/declare",
            envelope_for("agent-2", "00000000-0000-4000-8000-00000000d412", json!({
                "scopes": [{"kind": "file", "path": "src/lib.rs"}],
                "action": "write_file",
                "purpose": "Coordinate the module update."
            })),
        ))
        .await
        .expect("overlapping reservation responds");
    assert_eq!(second_reservation_response.status(), StatusCode::OK);
    let second_reservation = response_json(second_reservation_response).await;
    assert_eq!(second_reservation["decision"]["decision"], "warn");
    assert_eq!(second_reservation["decision"]["reason_code"], "coordination_conflict");

    successful_post(
        &app,
        "/v2/claim/acquire",
        envelope_for("agent-1", "00000000-0000-4000-8000-00000000d413", json!({
            "reservation_id": first_reservation["reservation_id"],
            "paths": [{"relative_path": "src/lib.rs", "observation": {"exists": false}}]
        })),
    )
    .await;
    let second_claim = app
        .clone()
        .oneshot(post(
            "/v2/claim/acquire",
            envelope_for("agent-2", "00000000-0000-4000-8000-00000000d414", json!({
                "reservation_id": second_reservation["reservation_id"],
                "paths": [{"relative_path": "src/lib.rs", "observation": {"exists": false}}]
            })),
        ))
        .await
        .expect("overlapping claim responds");
    assert_eq!(second_claim.status(), StatusCode::OK);
    let second_claim = response_json(second_claim).await;
    assert_eq!(second_claim["decision"]["decision"], "warn");
    assert_eq!(second_claim["decision"]["reason_code"], "coordination_conflict");

    let events = app
        .oneshot(support::query_get(
            "/v2/events",
            "agent-1",
            "00000000-0000-4000-8000-00000000d415",
            "workspace-1",
        ))
        .await
        .expect("warning audit responds");
    let events = response_json(events).await["events"].as_array().expect("event list").clone();
    let warnings = events.iter().filter(|event| {
        event["event_type"] == "authorization.warned"
            && event["payload"]["event"]["data"]["data"]["reason_code"] == "coordination_conflict"
    }).count();
    assert_eq!(warnings, 2);
    let enforcement = build_router(
        ServerConfig::new("test-token").with_coordination_mode(CoordinationMode::Enforcement),
    );
    successful_post(
        &enforcement,
        "/v2/reservation/declare",
        envelope_for("agent-1", "00000000-0000-4000-8000-00000000d416", json!({
            "scopes": [{"kind": "file", "path": "src/lib.rs"}],
            "action": "write_file",
            "purpose": "Update the module."
        })),
    )
    .await;
    let denied = enforcement
        .oneshot(post(
            "/v2/reservation/declare",
            envelope_for("agent-2", "00000000-0000-4000-8000-00000000d417", json!({
                "scopes": [{"kind": "file", "path": "src/lib.rs"}],
                "action": "write_file",
                "purpose": "Coordinate the module update."
            })),
        ))
        .await
        .expect("enforcement overlap responds");
    assert_eq!(denied.status(), StatusCode::FORBIDDEN);
}
