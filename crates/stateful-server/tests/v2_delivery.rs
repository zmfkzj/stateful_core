mod support;

use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use serde_json::json;
use std::time::Duration;
use tokio_stream::StreamExt;
use tower::ServiceExt;

use support::{app, envelope_for, post, query_get, response_json, successful_post};

fn stream_get(last_event_id: Option<u64>) -> Request<Body> {
    let uri = "/v2/notifications/stream?protocol_version=stateful.v2&request_id=00000000-0000-4000-8000-0000feed0001&observed_at=2026-07-16T00%3A00%3A00Z&agent_id=agent-2&actor_id=agent-2-actor&actor_type=agent&root=%2Fworkspace&workspace_id=workspace-1&repo_id=repo-1&worktree_id=worktree-1&branch=main&kind=hook&event=test&source_ref=stream-test";
    let mut builder = Request::builder()
        .uri(uri)
        .header("authorization", "Bearer test-token");
    if let Some(last_event_id) = last_event_id {
        builder = builder.header("last-event-id", last_event_id);
    }
    builder.body(Body::empty()).expect("stream request builds")
}

#[tokio::test]
async fn sse_reconnect_acknowledges_live_sequence_without_replay() {
    let app = app();
    let response = app
        .clone()
        .oneshot(stream_get(None))
        .await
        .expect("initial SSE connection receives a response");
    assert_eq!(response.status(), StatusCode::OK);
    let mut stream = response.into_body().into_data_stream();

    let wait = response_json(
        app.clone()
            .oneshot(post(
                "/v2/reservation/request",
                envelope_for(
                    "agent-2",
                    "00000000-0000-4000-8000-00000000c002",
                    json!({"relative_path": "src/lib.rs", "action": "write_file", "purpose": "Need the file."}),
                ),
            ))
            .await
            .expect("reservation request receives a response"),
    )
    .await;
    assert!(wait["wait_id"].is_string());
    let grant = app
        .clone()
        .oneshot(post(
            "/v2/reservation/claim",
            envelope_for(
                "agent-1",
                "00000000-0000-4000-8000-00000000c003",
                json!({"relative_path": "src/lib.rs"}),
            ),
        ))
        .await
        .expect("reservation claim receives a response");
    assert_eq!(grant.status(), StatusCode::OK);

    let bytes = tokio::time::timeout(Duration::from_secs(1), stream.next())
        .await
        .expect("live notification arrives")
        .expect("stream remains open")
        .expect("SSE data is valid");
    let event = String::from_utf8(bytes.to_vec()).expect("SSE is UTF-8");
    assert!(event.contains("id: 1"));

    let response = app
        .clone()
        .oneshot(stream_get(Some(1)))
        .await
        .expect("reconnected SSE connection receives a response");
    assert_eq!(response.status(), StatusCode::OK);
    let mut reconnected = response.into_body().into_data_stream();
    assert!(
        tokio::time::timeout(Duration::from_millis(250), reconnected.next())
            .await
            .is_err(),
        "acknowledged notifications must not replay",
    );

    let response = app
        .oneshot(stream_get(None))
        .await
        .expect("fresh SSE connection receives a response");
    assert_eq!(response.status(), StatusCode::OK);
    let mut fresh_connection = response.into_body().into_data_stream();
    assert!(
        tokio::time::timeout(Duration::from_millis(250), fresh_connection.next())
            .await
            .is_err(),
        "the delivery acknowledgement must persist beyond its reconnect cursor",
    );
}

#[tokio::test]
async fn context_acknowledgement_requires_the_delivery_sequence() {
    let app = app();
    successful_post(
        &app,
        "/v2/session/register",
        envelope_for(
            "agent-1",
            "00000000-0000-4000-8000-00000000d201",
            json!({"first_prompt": "work"}),
        ),
    )
    .await;
    let delivery = successful_post(
        &app,
        "/v2/context/render",
        envelope_for(
            "agent-1",
            "00000000-0000-4000-8000-00000000d202",
            json!({"mode": "brief"}),
        ),
    )
    .await;
    let invalid = app
        .clone()
        .oneshot(post(
            "/v2/context/ack",
            envelope_for(
                "agent-1",
                "00000000-0000-4000-8000-00000000d203",
                json!({
                    "delivery_id": delivery["delivery_id"],
                    "sequence": delivery["sequence"].as_u64().expect("sequence") + 1,
                    "workspace_version": delivery["workspace_version"]
                }),
            ),
        ))
        .await
        .expect("invalid context acknowledgement receives a response");
    assert_eq!(invalid.status(), StatusCode::BAD_REQUEST);
    assert_eq!(
        response_json(invalid).await["error"]["code"],
        "invalid_context_delivery"
    );
    successful_post(
        &app,
        "/v2/context/ack",
        envelope_for(
            "agent-1",
            "00000000-0000-4000-8000-00000000d204",
            json!({
                "delivery_id": delivery["delivery_id"],
                "sequence": delivery["sequence"],
                "workspace_version": delivery["workspace_version"]
            }),
        ),
    )
    .await;
}

#[tokio::test]
async fn event_reads_are_scoped_to_the_requested_workspace() {
    let app = app();
    successful_post(
        &app,
        "/v2/session/register",
        envelope_for(
            "agent-1",
            "00000000-0000-4000-8000-00000000d205",
            json!({"first_prompt": "workspace one"}),
        ),
    )
    .await;
    let mut other_workspace = envelope_for(
        "agent-1",
        "00000000-0000-4000-8000-00000000d206",
        json!({"first_prompt": "workspace two"}),
    );
    other_workspace["workspace_id"] = json!("workspace-2");
    other_workspace["workspace"]["workspace_id"] = json!("workspace-2");
    successful_post(&app, "/v2/session/register", other_workspace).await;

    let response = app
        .oneshot(query_get(
            "/v2/events",
            "agent-1",
            "00000000-0000-4000-8000-00000000d207",
            "workspace-1",
        ))
        .await
        .expect("scoped events query receives a response");
    assert_eq!(response.status(), StatusCode::OK);
    assert!(
        response_json(response).await["events"]
            .as_array()
            .expect("events array")
            .iter()
            .all(|event| event["workspace_id"] == "workspace-1"),
    );
}

#[tokio::test]
async fn lost_poll_response_redelivers_until_sequence_acknowledgement() {
    let app = app();
    let wait = successful_post(
        &app,
        "/v2/reservation/request",
        envelope_for(
            "agent-2",
            "00000000-0000-4000-8000-00000000d314",
            json!({
                "relative_path": "src/lib.rs", "action": "write_file", "purpose": "Need the file."
            }),
        ),
    )
    .await;
    successful_post(
        &app,
        "/v2/reservation/claim",
        envelope_for(
            "agent-1",
            "00000000-0000-4000-8000-00000000d315",
            json!({
                "relative_path": "src/lib.rs"
            }),
        ),
    )
    .await;
    let first = successful_post(
        &app,
        "/v2/notifications/poll",
        envelope_for("agent-2", "00000000-0000-4000-8000-00000000d316", json!({})),
    )
    .await;
    let second = successful_post(
        &app,
        "/v2/notifications/poll",
        envelope_for("agent-2", "00000000-0000-4000-8000-00000000d317", json!({})),
    )
    .await;
    assert_eq!(first.as_array().map(Vec::len), Some(1));
    assert_eq!(second, first, "a lost poll response must remain pending");
    assert_eq!(first[0]["notification_id"], wait["wait_id"]);

    let response = app
        .clone()
        .oneshot(stream_get(None))
        .await
        .expect("stream response");
    assert_eq!(response.status(), StatusCode::OK);
    let mut stream = response.into_body().into_data_stream();
    let bytes = tokio::time::timeout(Duration::from_secs(1), stream.next())
        .await
        .expect("unacknowledged notification arrives")
        .expect("stream remains open")
        .expect("SSE data is valid");
    let event = String::from_utf8(bytes.to_vec()).expect("SSE is UTF-8");
    assert!(event.contains(wait["wait_id"].as_str().expect("wait id")));

    let acknowledged = successful_post(
        &app,
        "/v2/notifications/poll",
        envelope_for(
            "agent-2",
            "00000000-0000-4000-8000-00000000d318",
            json!({"sequence": 1}),
        ),
    )
    .await;
    assert_eq!(acknowledged, json!([wait["wait_id"]]));
    let after_ack = successful_post(
        &app,
        "/v2/notifications/poll",
        envelope_for("agent-2", "00000000-0000-4000-8000-00000000d319", json!({})),
    )
    .await;
    assert_eq!(after_ack, json!([]));
    let response = app
        .oneshot(stream_get(None))
        .await
        .expect("stream response");
    assert_eq!(response.status(), StatusCode::OK);
    let mut stream = response.into_body().into_data_stream();
    assert!(
        tokio::time::timeout(Duration::from_millis(250), stream.next())
            .await
            .is_err(),
        "acknowledged notification must not replay to SSE",
    );
}

#[tokio::test]
async fn forged_last_event_id_does_not_hide_a_new_notification() {
    let app = app();
    let response = app
        .clone()
        .oneshot(stream_get(Some(999)))
        .await
        .expect("stream response");
    assert_eq!(response.status(), StatusCode::OK);
    let mut stream = response.into_body().into_data_stream();
    successful_post(
        &app,
        "/v2/reservation/request",
        envelope_for(
            "agent-2",
            "00000000-0000-4000-8000-00000000d311",
            json!({
                "relative_path": "src/lib.rs", "action": "write_file", "purpose": "Need the file."
            }),
        ),
    )
    .await;
    successful_post(
        &app,
        "/v2/reservation/claim",
        envelope_for(
            "agent-1",
            "00000000-0000-4000-8000-00000000d312",
            json!({
                "relative_path": "src/lib.rs"
            }),
        ),
    )
    .await;
    let event = tokio::time::timeout(Duration::from_secs(1), stream.next())
        .await
        .expect("notification must arrive after forged cursor")
        .expect("stream remains open")
        .expect("SSE data is valid");
    assert!(
        String::from_utf8(event.to_vec())
            .expect("SSE is UTF-8")
            .contains("id: 1")
    );
}
