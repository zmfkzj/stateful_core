mod support;

use axum::{body::Body, http::{Request, StatusCode}};
use serde_json::json;
use stateful_store::Store;
use tower::ServiceExt;

use support::{app, app_with_store, envelope, envelope_for, get, post, response_json};

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
async fn malformed_post_and_invalid_flattened_query_return_v2_errors() {
    let post_response = app()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v2/session/register")
                .header("authorization", "Bearer test-token")
                .header("content-type", "application/json")
                .body(Body::from("{"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(post_response.status(), StatusCode::BAD_REQUEST);
    assert_eq!(response_json(post_response).await["protocol_version"], "stateful.v2");

    let get_response = app().oneshot(get("/v2/current?protocol_version=stateful.v1")).await.unwrap();
    assert_eq!(get_response.status(), StatusCode::BAD_REQUEST);
    assert_eq!(response_json(get_response).await["protocol_version"], "stateful.v2");
}

#[tokio::test]
async fn request_id_reused_across_mutation_routes_is_rejected() {
    let app = app();
    let request_id = "00000000-0000-4000-8000-00000000c001";
    assert_eq!(
        app.clone()
            .oneshot(post(
                "/v2/session/register",
                envelope_for("agent-1", request_id, json!({"first_prompt": "work"})),
            ))
            .await
            .unwrap()
            .status(),
        StatusCode::OK,
    );

    let response = app
        .oneshot(post(
            "/v2/presence/update",
            envelope_for("agent-1", request_id, json!({"kind": "heartbeat"})),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CONFLICT);
    assert_eq!(response_json(response).await["error"]["code"], "idempotency_key_reused");
}


#[tokio::test]
async fn store_failures_return_sanitized_v2_internal_errors() {
    let mut store = Store::open_in_memory().expect("store opens");
    store.fail_projector_on_event_for_tests(1);
    let response = app_with_store(store)
        .oneshot(post(
            "/v2/session/register",
            envelope_for(
                "agent-1",
                "00000000-0000-4000-8000-00000000d002",
                json!({"first_prompt": "work"}),
            ),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
    let body = response_json(response).await;
    assert_eq!(body["error"]["code"], "internal_error");
    assert_eq!(body["error"]["message"], "The server could not complete the request.");
}

#[tokio::test]
async fn request_id_reused_between_register_routes_is_rejected() {
    let app = app();
    let request_id = "00000000-0000-4000-8000-00000000d301";
    assert_eq!(
        app.clone()
            .oneshot(post(
                "/v2/session/register",
                envelope_for("agent-1", request_id, json!({"first_prompt": "work"})),
            ))
            .await
            .expect("registration response")
            .status(),
        StatusCode::OK,
    );
    let response = app
        .oneshot(post(
            "/v2/presence/update",
            envelope_for("agent-1", request_id, json!({"kind": "register", "first_prompt": "work"})),
        ))
        .await
        .expect("duplicate route response");
    assert_eq!(response.status(), StatusCode::CONFLICT);
    assert_eq!(response_json(response).await["error"]["code"], "idempotency_key_reused");
}


#[tokio::test]
async fn bearer_failures_use_the_v2_error_envelope_with_an_action() {
    let response = app()
        .oneshot(Request::builder().uri("/v2/current").body(Body::empty()).expect("request"))
        .await
        .expect("response");
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    let body = response_json(response).await;
    assert_eq!(body["protocol_version"], "stateful.v2");
    assert_eq!(body["error"]["code"], "unauthorized");
    assert!(body["error"]["required_next_action"].is_string());
}
