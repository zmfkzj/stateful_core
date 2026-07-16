mod support;

use axum::{Router, http::StatusCode};
use serde_json::json;
use stateful_store::{MutableClock, PresenceRegistration, Store};
use std::time::Duration;
use tower::ServiceExt;

use support::{app, app_with_store, envelope_for, get, query_get, response_json, successful_post};

fn app_with_clock(clock: MutableClock) -> Router {
    app_with_store(Store::open_in_memory_with_clock(clock).expect("store opens"))
}

fn clock() -> MutableClock {
    MutableClock::from_system_now()
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
    assert_eq!(body["pid"], std::process::id());
}

#[tokio::test]
async fn health_and_runtime_identity_are_available_with_bearer_enforcement() {
    let app = app();
    assert_eq!(app.clone().oneshot(get("/health")).await.unwrap().status(), StatusCode::OK);
    assert_eq!(
        app.clone()
            .oneshot(axum::http::Request::builder().uri("/v2/current").body(axum::body::Body::empty()).unwrap())
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
async fn expired_notifications_are_not_polled_or_streamed() {
    let clock = clock();
    let app = app_with_clock(clock.clone());
    let wait = successful_post(
        &app,
        "/v2/reservation/request",
        envelope_for("agent-2", "00000000-0000-4000-8000-00000000d302", json!({
            "relative_path": "src/lib.rs", "action": "write_file", "purpose": "Need the file."
        })),
    )
    .await;
    successful_post(
        &app,
        "/v2/reservation/claim",
        envelope_for("agent-1", "00000000-0000-4000-8000-00000000d303", json!({
            "relative_path": "src/lib.rs"
        })),
    )
    .await;
    clock.advance(Duration::from_secs(3 * 60));
    let callback = successful_post(
        &app,
        "/v2/notifications/poll",
        envelope_for("agent-2", "00000000-0000-4000-8000-00000000d317", json!({
            "notification_id": wait["wait_id"], "sequence": 1, "outcome": "delivered"
        })),
    )
    .await;
    assert_eq!(callback["status"], "queued", "expired notification must not be marked delivered");
    let polled = successful_post(
        &app,
        "/v2/notifications/poll",
        envelope_for("agent-2", "00000000-0000-4000-8000-00000000d304", json!({})),
    )
    .await;
    assert_eq!(polled, json!([]), "expired notification {} must not poll", wait["wait_id"]);
    let response = app.oneshot(support::get("/v2/notifications/stream?protocol_version=stateful.v2&request_id=00000000-0000-4000-8000-0000feed0001&observed_at=2026-07-16T00%3A00%3A00Z&agent_id=agent-2&actor_id=agent-2-actor&actor_type=agent&root=%2Fworkspace&workspace_id=workspace-1&repo_id=repo-1&worktree_id=worktree-1&branch=main&kind=hook&event=test&source_ref=stream-test")).await.expect("stream response");
    assert_eq!(response.status(), StatusCode::OK);
    let mut stream = response.into_body().into_data_stream();
    assert!(
        tokio::time::timeout(Duration::from_millis(250), tokio_stream::StreamExt::next(&mut stream)).await.is_err(),
        "expired notification must not be emitted by SSE",
    );
}

#[tokio::test]
async fn expired_context_acknowledgement_does_not_advance_the_cursor() {
    let clock = clock();
    let app = app_with_clock(clock.clone());
    successful_post(
        &app,
        "/v2/session/register",
        envelope_for("agent-1", "00000000-0000-4000-8000-00000000d305", json!({"first_prompt": "work"})),
    )
    .await;
    let delivery = successful_post(
        &app,
        "/v2/context/render",
        envelope_for("agent-1", "00000000-0000-4000-8000-00000000d306", json!({"mode": "brief"})),
    )
    .await;
    clock.advance(Duration::from_secs(25 * 60 * 60));
    let acknowledgement = successful_post(
        &app,
        "/v2/context/ack",
        envelope_for("agent-1", "00000000-0000-4000-8000-00000000d307", json!({
            "delivery_id": delivery["delivery_id"],
            "sequence": delivery["sequence"],
            "workspace_version": delivery["workspace_version"],
        })),
    )
    .await;
    assert_eq!(acknowledgement["cursor"], 0, "expired delivery must be dead-lettered before acknowledgement");
}

#[tokio::test]
async fn resume_next_excludes_context_deliveries_expired_before_the_ticker() {
    let clock = clock();
    let app = app_with_clock(clock.clone());
    successful_post(
        &app,
        "/v2/session/register",
        envelope_for("agent-1", "00000000-0000-4000-8000-00000000d308", json!({"first_prompt": "work"})),
    )
    .await;
    successful_post(
        &app,
        "/v2/context/render",
        envelope_for("agent-1", "00000000-0000-4000-8000-00000000d309", json!({"mode": "brief"})),
    )
    .await;
    clock.advance(Duration::from_secs(25 * 60 * 60));
    let response = app
        .oneshot(support::post(
            "/v2/resume/next",
            envelope_for("agent-1", "00000000-0000-4000-8000-00000000d310", json!({})),
        ))
        .await
        .expect("resume response");
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response_json(response).await["deliveries"], json!([]));
}

#[tokio::test]
async fn health_reports_replay_validation_failure_then_a_healthy_store() {
    let mut failed_store = Store::open_in_memory().expect("store opens");
    let request = stateful_core::RequestEnvelope::<PresenceRegistration>::from_json(
        envelope_for("agent-1", "00000000-0000-4000-8000-00000000d313", json!({"first_prompt": "work"})).to_string(),
    )
    .expect("request parses");
    failed_store.register_presence(&request).expect("presence registers");
    failed_store.corrupt_journal_metadata_for_tests("actor_type", "").expect("journal corruption injects");
    assert_eq!(
        app_with_store(failed_store).oneshot(get("/health")).await.expect("failure health response").status(),
        StatusCode::SERVICE_UNAVAILABLE,
    );
    assert_eq!(app().oneshot(get("/health")).await.expect("healthy health response").status(), StatusCode::OK);
}
