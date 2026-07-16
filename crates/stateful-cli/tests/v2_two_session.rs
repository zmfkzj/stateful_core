use serde_json::{Value, json};
use stateful_cli::{
    HttpResponse, RepoIdentity, ServerRuntime, get_json, get_v2, post_json, post_v2,
    v2_query_for_runtime, v2_request_envelope,
};
use stateful_core::{ActorType, RequestEnvelope, SourceKind, fingerprint_path};
use stateful_server::{CoordinationMode, ServerConfig, serve_listener_until};
use stateful_store::{MutableClock, Store};
use std::{
    path::Path,
    process::Command,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use tempfile::TempDir;
use tokio::time::sleep;
use uuid::Uuid;

const AGENT_A: &str = "agent-a";
const AGENT_B: &str = "agent-b";
const WORKSPACE_ID: &str = "two-session-workspace";
const SHARED_PATH: &str = "src/shared.rs";
const FENCE_PATH: &str = "src/inflight.rs";
const HUMAN_PATH: &str = "src/human.rs";

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn two_sessions_coordinate_over_real_tcp_and_replay_persistent_journal() {
    let temp = TempDir::new().expect("temporary fixture should create");
    let workspace = temp.path().join("workspace");
    initialize_git_workspace(&workspace);
    let identity = repo_identity(&workspace);
    let database = temp.path().join("presence.sqlite");
    let clock = MutableClock::from_system_now();
    let config = ServerConfig::with_store(
        "two-session-token",
        Store::open_with_clock(&database, clock.clone()).expect("persistent store should open"),
    )
    .with_coordination_mode(CoordinationMode::Awareness)
    .with_maintenance_interval(Duration::from_secs(3_600));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("real TCP listener should bind");
    let runtime = ServerRuntime::new(
        format!("http://{}", listener.local_addr().expect("listener address should load")),
        "two-session-token",
        WORKSPACE_ID,
        0,
    );
    let stopping = Arc::new(AtomicBool::new(false));
    let server = tokio::spawn(serve_listener_until(
        listener,
        config,
        wait_for_stop(stopping.clone()),
    ));
    wait_for_health(&runtime).await;

    post_success(
        &runtime,
        &identity,
        AGENT_A,
        "/v2/session/register",
        json!({"first_prompt": "Coordinate the shared file."}),
    );
    let initial = fingerprint(&workspace, SHARED_PATH);
    read_exact(&runtime, &identity, AGENT_A, "a-initial-read", SHARED_PATH, initial.clone());

    post_success(
        &runtime,
        &identity,
        AGENT_B,
        "/v2/session/register",
        json!({"first_prompt": "Update the shared file."}),
    );
    let queued_before_b_warning = queue_event_count(&events(&runtime, &identity, AGENT_B));
    let b_write = authorize(
        &runtime,
        &identity,
        AGENT_B,
        "b-write",
        SHARED_PATH,
        initial.clone(),
    );
    assert_eq!(b_write["decision"]["decision"], "warn");
    assert_eq!(b_write["decision"]["reason_code"], "missing_read_provenance");
    assert_eq!(
        queue_event_count(&events(&runtime, &identity, AGENT_B)),
        queued_before_b_warning,
        "the read-provenance awareness warning must not enqueue waits or notifications"
    );
    let b_notifications = post_success(
        &runtime,
        &identity,
        AGENT_B,
        "/v2/notifications/poll",
        json!({}),
    );
    assert_eq!(b_notifications, json!([]), "awareness warnings must not queue notifications");
    std::fs::create_dir_all(workspace.join("src")).expect("source directory should create");
    std::fs::write(workspace.join(SHARED_PATH), "B\n").expect("B write should reach the Git workspace");
    let b_after = fingerprint(&workspace, SHARED_PATH);
    complete_write(
        &runtime,
        &identity,
        AGENT_B,
        b_write["intent_id"].as_str().expect("B intent id"),
        SHARED_PATH,
        b_after.clone(),
    );

    let queued_before_b_inflight_warning = queue_event_count(&events(&runtime, &identity, AGENT_B));
    let b_inflight = authorize(
        &runtime,
        &identity,
        AGENT_B,
        "b-inflight",
        FENCE_PATH,
        fingerprint(&workspace, FENCE_PATH),
    );
    assert_eq!(b_inflight["decision"]["decision"], "warn");
    assert_eq!(
        queue_event_count(&events(&runtime, &identity, AGENT_B)),
        queued_before_b_inflight_warning,
        "the in-flight awareness warning must not enqueue waits or notifications"
    );
    let b_inflight_notifications = post_success(
        &runtime,
        &identity,
        AGENT_B,
        "/v2/notifications/poll",
        json!({}),
    );
    assert_eq!(
        b_inflight_notifications,
        json!([]),
        "awareness warnings must not queue notifications"
    );
    let in_flight_denial = post_response(
        &runtime,
        &identity,
        AGENT_A,
        "/v2/authorize",
        json!({
            "operation_id": "a-fence-denial",
            "action": "write_file",
            "targets": [{"path": FENCE_PATH, "before": fingerprint(&workspace, FENCE_PATH)}]
        }),
    );
    assert_hard_denial(in_flight_denial, "unknown_write_outcome");

    post_success(
        &runtime,
        &identity,
        AGENT_B,
        "/v2/human/observe",
        json!({
            "relative_path": HUMAN_PATH,
            "kind": "save",
            "confidence": "high",
            "source": "two-session-smoke",
            "summary": "A human saved this file."
        }),
    );
    let human_denial = post_response(
        &runtime,
        &identity,
        AGENT_A,
        "/v2/authorize",
        json!({
            "operation_id": "a-human-denial",
            "action": "write_file",
            "targets": [{"path": HUMAN_PATH, "before": fingerprint(&workspace, HUMAN_PATH)}]
        }),
    );
    assert_hard_denial(human_denial, "unreconciled_human_write");

    let notifications = post_success(
        &runtime,
        &identity,
        AGENT_A,
        "/v2/notifications/poll",
        json!({}),
    );
    assert_eq!(notifications, json!([]), "awareness warnings must not queue notifications");
    let events_before_context = events(&runtime, &identity, AGENT_A);
    assert!(
        events_before_context.iter().all(|event| {
            let event_type = event["event_type"].as_str().expect("event type should serialize");
            !event_type.starts_with("wait.") && !event_type.starts_with("notification.")
        }),
        "awareness warnings must not enqueue waits or notifications"
    );

    let a_context = post_success(
        &runtime,
        &identity,
        AGENT_A,
        "/v2/context/render",
        json!({"mode": "detailed", "resource": SHARED_PATH}),
    );
    assert_eq!(a_context["changed"], true);
    assert!(
        a_context["workspace_version"].as_u64().expect("workspace version should be numeric") > 0,
        "A must receive a versioned context delta after B commits"
    );
    let mut delivery_ids = vec![ack_delivery(&runtime, &identity, AGENT_A, &a_context)];

    let stale_denial = post_response(
        &runtime,
        &identity,
        AGENT_A,
        "/v2/authorize",
        json!({
            "operation_id": "a-stale-write",
            "action": "write_file",
            "targets": [{"path": SHARED_PATH, "before": initial}]
        }),
    );
    assert_hard_denial(stale_denial, "stale_observation");

    read_exact(
        &runtime,
        &identity,
        AGENT_A,
        "a-reread",
        SHARED_PATH,
        b_after.clone(),
    );
    let queued_before_a_warning = queue_event_count(&events(&runtime, &identity, AGENT_A));
    let notifications_before_a_warning = post_success(
        &runtime,
        &identity,
        AGENT_A,
        "/v2/notifications/poll",
        json!({}),
    );
    let a_write = authorize(
        &runtime,
        &identity,
        AGENT_A,
        "a-write",
        SHARED_PATH,
        b_after,
    );
    assert_eq!(a_write["decision"]["decision"], "warn");
    assert_eq!(a_write["decision"]["reason_code"], "missing_reservation");
    assert_eq!(
        queue_event_count(&events(&runtime, &identity, AGENT_A)),
        queued_before_a_warning,
        "the reservation awareness warning must not enqueue waits or notifications"
    );
    let a_notifications = post_success(
        &runtime,
        &identity,
        AGENT_A,
        "/v2/notifications/poll",
        json!({}),
    );
    assert_eq!(
        a_notifications,
        notifications_before_a_warning,
        "the reservation awareness warning must not create a notification"
    );
    std::fs::write(workspace.join(SHARED_PATH), "A\n").expect("A write should reach the Git workspace");
    let a_after = fingerprint(&workspace, SHARED_PATH);
    complete_write(
        &runtime,
        &identity,
        AGENT_A,
        a_write["intent_id"].as_str().expect("A intent id"),
        SHARED_PATH,
        a_after,
    );

    let explicit = post_success(
        &runtime,
        &identity,
        AGENT_B,
        "/v2/activity/finalize",
        json!({
            "status": "done",
            "summary": "B completed the shared-file update.",
            "files_changed": [SHARED_PATH],
            "tests_run": ["cargo test -p stateful-cli --test v2_two_session"],
            "remaining_work": [],
            "next_plan": "A can continue from the committed state."
        }),
    );
    assert_eq!(explicit["explicit"], true);
    assert_eq!(explicit["status"], "done");
    assert_eq!(explicit["summary"], "B completed the shared-file update.");

    let handoff_context = post_success(
        &runtime,
        &identity,
        AGENT_A,
        "/v2/context/render",
        json!({"mode": "detailed", "resource": SHARED_PATH}),
    );
    assert!(
        handoff_context["items"].as_array().expect("context items should be an array").iter().any(|item| {
            item["agent_id"] == AGENT_B && item["summary"] == "B completed the shared-file update."
        }),
        "A must receive B's explicit handoff in a delivered context"
    );
    delivery_ids.push(ack_delivery(&runtime, &identity, AGENT_A, &handoff_context));

    clock.advance(Duration::from_secs(16 * 60));
    let fallback_context = post_success(
        &runtime,
        &identity,
        AGENT_A,
        "/v2/context/render",
        json!({"mode": "detailed", "resource": SHARED_PATH}),
    );
    assert_eq!(fallback_context["changed"], true, "expiry must produce a context version");
    delivery_ids.push(ack_delivery(&runtime, &identity, AGENT_A, &fallback_context));
    let current_a = current(&runtime, &identity, AGENT_A);
    assert_eq!(current_a["handoff"]["explicit"], false, "A must expire into fallback without Stop");
    assert_eq!(
        current_a["handoff"]["summary"],
        "Session ended with no explicit handoff supplied.",
        "fallback handoff must remain distinct from B's explicit handoff"
    );
    assert_ne!(current_a["handoff"]["summary"], explicit["summary"]);

    stopping.store(true, Ordering::Release);
    server
        .await
        .expect("server task should join")
        .expect("server should stop gracefully");

    let mut reopened = Store::open_with_clock(&database, clock).expect("persistent store should reopen");
    assert!(
        reopened
            .pending_context_deliveries(AGENT_A, WORKSPACE_ID)
            .expect("pending deliveries should load")
            .is_empty(),
        "every delivered context must be ACKed exactly once"
    );
    for delivery_id in &delivery_ids {
        let delivery = reopened
            .context_delivery(WORKSPACE_ID, delivery_id)
            .expect("context delivery should load")
            .expect("ACKed delivery should persist");
        assert_eq!(delivery.status, "acknowledged");
        assert!(delivery.origin_event_seq > 0, "replay must preserve delivery origin event sequences");
    }
    let all_events = reopened
        .recent_workspace_events(WORKSPACE_ID, 1_000)
        .expect("journal events should load");
    assert_eq!(
        all_events
            .iter()
            .filter(|event| event.event_type == "context.delivery_acknowledged")
            .count(),
        delivery_ids.len(),
        "each delivery must have exactly one ACK event"
    );
    let before_replay = reopened.projection_snapshot().expect("complete projection snapshot should load");
    reopened.rebuild_projections().expect("journal should replay into empty projections");
    let after_replay = reopened.projection_snapshot().expect("replayed projection snapshot should load");
    assert_eq!(before_replay, after_replay, "replay must preserve all projection bytes and origin sequences");
}

async fn wait_for_stop(stopping: Arc<AtomicBool>) {
    while !stopping.load(Ordering::Acquire) {
        sleep(Duration::from_millis(5)).await;
    }
}

async fn wait_for_health(runtime: &ServerRuntime) {
    for _ in 0..100 {
        if get_json(runtime, "/health").is_ok_and(|response| response.status_code == 200) {
            return;
        }
        sleep(Duration::from_millis(5)).await;
    }
    panic!("real listener did not become healthy");
}

fn initialize_git_workspace(workspace: &Path) {
    let output = Command::new("git")
        .args(["init", "--quiet", "--initial-branch=main"])
        .arg(workspace)
        .output()
        .expect("git should initialize the temporary workspace");
    assert!(output.status.success(), "git init failed: {}", String::from_utf8_lossy(&output.stderr));
}

fn repo_identity(workspace: &Path) -> RepoIdentity {
    RepoIdentity {
        repo_id: "two-session-repo".into(),
        worktree_id: "two-session-worktree".into(),
        root: workspace.display().to_string(),
        branch: "main".into(),
    }
}

fn fingerprint(workspace: &Path, path: &str) -> Value {
    serde_json::to_value(fingerprint_path(&workspace.join(path)).expect("workspace fingerprint should load"))
        .expect("fingerprint should serialize")
}

fn request(identity: &RepoIdentity, agent_id: &str, payload: Value) -> RequestEnvelope<Value> {
    v2_request_envelope(
        Uuid::new_v4(),
        agent_id.into(),
        WORKSPACE_ID.into(),
        Some(identity.clone()),
        ActorType::Agent,
        SourceKind::Cli,
        "test.v2_two_session",
        "crates/stateful-cli/tests/v2_two_session.rs",
        Some("v2_two_session".into()),
        payload,
    )
    .expect("V2 request envelope should build")
}

fn post_response(
    runtime: &ServerRuntime,
    identity: &RepoIdentity,
    agent_id: &str,
    route: &str,
    payload: Value,
) -> HttpResponse {
    let request = request(identity, agent_id, payload);
    post_json(
        runtime,
        route,
        &serde_json::to_value(request).expect("V2 request should serialize"),
    )
    .expect("authenticated V2 request should complete")
}

fn post_success(
    runtime: &ServerRuntime,
    identity: &RepoIdentity,
    agent_id: &str,
    route: &str,
    payload: Value,
) -> Value {
    response_json(
        post_v2(runtime, route, &request(identity, agent_id, payload))
            .expect("authenticated V2 request should succeed"),
        200,
    )
}

fn response_json(response: HttpResponse, expected_status: u16) -> Value {
    assert_eq!(response.status_code, expected_status, "unexpected response body: {}", response.body);
    serde_json::from_str(&response.body).expect("response body should be JSON")
}

fn assert_hard_denial(response: HttpResponse, reason_code: &str) {
    let body = response_json(response, 403);
    assert_eq!(body["decision"], "deny");
    assert_eq!(body["reason_code"], reason_code);
}

fn read_exact(
    runtime: &ServerRuntime,
    identity: &RepoIdentity,
    agent_id: &str,
    operation_id: &str,
    path: &str,
    fingerprint: Value,
) {
    let started = post_success(
        runtime,
        identity,
        agent_id,
        "/v2/read/start",
        json!({"operation_id": operation_id, "path": path, "before": fingerprint}),
    );
    assert_eq!(started["status"], "started");
    let completed = post_success(
        runtime,
        identity,
        agent_id,
        "/v2/read/complete",
        json!({"operation_id": operation_id, "path": path, "classification": "exact", "after": fingerprint}),
    );
    assert_eq!(completed["status"], "stabilized");
    assert_eq!(completed["classification"], "exact");
}

fn authorize(
    runtime: &ServerRuntime,
    identity: &RepoIdentity,
    agent_id: &str,
    operation_id: &str,
    path: &str,
    before: Value,
) -> Value {
    post_success(
        runtime,
        identity,
        agent_id,
        "/v2/authorize",
        json!({
            "operation_id": operation_id,
            "action": "write_file",
            "targets": [{"path": path, "before": before}]
        }),
    )
}

fn complete_write(
    runtime: &ServerRuntime,
    identity: &RepoIdentity,
    agent_id: &str,
    intent_id: &str,
    path: &str,
    after: Value,
) {
    let completed = post_success(
        runtime,
        identity,
        agent_id,
        "/v2/write/complete",
        json!({
            "intent_id": intent_id,
            "outcome": "committed",
            "post_fingerprints": [[path, after]]
        }),
    );
    assert_eq!(completed["status"], "committed");
}

fn ack_delivery(runtime: &ServerRuntime, identity: &RepoIdentity, agent_id: &str, context: &Value) -> String {
    let delivery_id = context["delivery_id"].as_str().expect("context delivery id should be present").to_owned();
    let sequence = context["sequence"].as_u64().expect("context sequence should be present");
    let workspace_version = context["workspace_version"].as_u64().expect("context version should be present");
    let acknowledgement = post_success(
        runtime,
        identity,
        agent_id,
        "/v2/context/ack",
        json!({
            "delivery_id": delivery_id,
            "sequence": sequence,
            "workspace_version": workspace_version
        }),
    );
    assert_eq!(acknowledgement["acknowledged_version"], workspace_version);
    assert_eq!(acknowledgement["cursor"], workspace_version);
    delivery_id
}

fn queue_event_count(events: &[Value]) -> usize {
    events
        .iter()
        .filter(|event| {
            let event_type = event["event_type"].as_str().expect("event type should serialize");
            event_type.starts_with("wait.") || event_type.starts_with("notification.")
        })
        .count()
}

fn events(runtime: &ServerRuntime, identity: &RepoIdentity, agent_id: &str) -> Vec<Value> {
    let request = v2_query_for_runtime(
        Uuid::new_v4(),
        agent_id.into(),
        WORKSPACE_ID.into(),
        Some(identity.clone()),
        SourceKind::Cli,
        "test.v2_two_session.events",
        "crates/stateful-cli/tests/v2_two_session.rs",
        Some("v2_two_session".into()),
        json!({"limit": 1_000}),
    )
    .expect("V2 events query should build");
    response_json(
        get_v2(runtime, "/v2/events", &request).expect("authenticated V2 events query should complete"),
        200,
    )["events"]
        .as_array()
        .expect("events response should contain an array")
        .clone()
}

fn current(runtime: &ServerRuntime, identity: &RepoIdentity, agent_id: &str) -> Value {
    let request = v2_query_for_runtime(
        Uuid::new_v4(),
        agent_id.into(),
        WORKSPACE_ID.into(),
        Some(identity.clone()),
        SourceKind::Cli,
        "test.v2_two_session.current",
        "crates/stateful-cli/tests/v2_two_session.rs",
        Some("v2_two_session".into()),
        json!({}),
    )
    .expect("V2 current query should build");
    response_json(
        get_v2(runtime, "/v2/current", &request).expect("authenticated V2 current query should complete"),
        200,
    )
}
