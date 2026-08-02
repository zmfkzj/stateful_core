use stateful_core::{
    ContentDigest, CoordinationSettings, DigestAlgorithm, EntryState, MutationOperation,
    ObjectKind, ObjectState, ResourceKey, ResourceKind, ResourceObservation, TaskStatus,
};
use stateful_store::{
    CommandContext, LeaseActivateInput, LeaseReleaseInput, LeaseReleaseStatus, LeaseRequestState,
    ReadCompleteInput, ReadResultStatus, ReadStartInput, Store, StoreError, TaskEndInput,
    TaskHeartbeatInput, TaskStartInput, WriteCompleteInput, WritePrepareInput, WritePrepareResult,
    WriteResultStatus, WriteTerminal,
};

fn context(request_id: &str, task_id: &str, agent_id: &str, observed_at: &str) -> CommandContext {
    CommandContext {
        request_id: request_id.to_string(),
        task_id: task_id.to_string(),
        agent_id: agent_id.to_string(),
        workspace_id: "workspace-1".to_string(),
        observed_at: observed_at.to_string(),
    }
}

fn observation(value: &str) -> Vec<ResourceObservation> {
    keyed_observation("object-1", "src/lib.rs", value)
}

fn keyed_observation(resource_id: &str, path: &str, value: &str) -> Vec<ResourceObservation> {
    let inode = resource_id.bytes().map(u64::from).sum::<u64>();
    let name = path
        .rsplit('/')
        .next()
        .expect("test path should have a basename");
    vec![
        ResourceObservation::Object {
            resource: ResourceKey {
                workspace_id: "workspace-1".to_string(),
                kind: ResourceKind::Object,
                resource_id: format!("object:1:{inode}"),
                canonical_path: path.to_string(),
            },
            observed: ObjectState::Present {
                kind: ObjectKind::RegularFile,
                blake3: ContentDigest {
                    algorithm: DigestAlgorithm::Blake3,
                    value: value.to_string(),
                },
                byte_len: value.len() as u64,
            },
            generation: 0,
        },
        ResourceObservation::Entry {
            resource: ResourceKey {
                workspace_id: "workspace-1".to_string(),
                kind: ResourceKind::Entry,
                resource_id: format!("entry:1:1:{name}"),
                canonical_path: path.to_string(),
            },
            observed: EntryState::Present {
                kind: ObjectKind::RegularFile,
                device: 1,
                inode,
                empty: None,
            },
            generation: 0,
        },
    ]
}

fn absent_observation(path: &str) -> Vec<ResourceObservation> {
    let name = path
        .rsplit('/')
        .next()
        .expect("test path should have a basename");
    vec![ResourceObservation::Entry {
        resource: ResourceKey {
            workspace_id: "workspace-1".to_string(),
            kind: ResourceKind::Entry,
            resource_id: format!("entry:1:1:{name}"),
            canonical_path: path.to_string(),
        },
        observed: EntryState::Absent,
        generation: 0,
    }]
}

fn start_task(store: &mut Store, task_id: &str, agent_id: &str, second: u8) {
    store
        .task_start(
            &context(
                &format!("{task_id}-start"),
                task_id,
                agent_id,
                &format!("2026-08-02T00:00:{second:02}Z"),
            ),
            &TaskStartInput {
                next_action: "edit src/lib.rs".to_string(),
                settings: CoordinationSettings {
                    heartbeat_interval_seconds: 60,
                    inactivity_timeout_seconds: 3_600,
                    lease_expiry_seconds: 7_200,
                    offer_ttl_seconds: 7_200,
                },
                expires_at: "2026-08-02T01:00:00Z".to_string(),
                runtime_process: None,
            },
        )
        .expect("task should start");
}

fn read_exact(
    store: &mut Store,
    task_id: &str,
    agent_id: &str,
    read_id: &str,
    value: &str,
    start_second: u8,
) {
    let invocation_id = format!("{read_id}-invocation");
    store
        .read_start(
            &context(
                &format!("{read_id}-start-command"),
                task_id,
                agent_id,
                &format!("2026-08-02T00:00:{start_second:02}Z"),
            ),
            &ReadStartInput {
                read_id: read_id.to_string(),
                invocation_id: invocation_id.clone(),
                resources: observation(value),
            },
        )
        .expect("read should start");
    store
        .read_complete(
            &context(
                &format!("{read_id}-complete-command"),
                task_id,
                agent_id,
                &format!("2026-08-02T00:00:{:02}Z", start_second + 1),
            ),
            &ReadCompleteInput {
                read_id: read_id.to_string(),
                invocation_id,
                resources: observation(value),
                terminal_success: true,
                complete: true,
                stable: true,
                exact: true,
            },
        )
        .expect("read should complete");
}

fn read_exact_resources(
    store: &mut Store,
    task_id: &str,
    agent_id: &str,
    read_id: &str,
    resources: Vec<ResourceObservation>,
    start_second: u8,
) {
    let invocation_id = format!("{read_id}-invocation");
    store
        .read_start(
            &context(
                &format!("{read_id}-start-command"),
                task_id,
                agent_id,
                &format!("2026-08-02T00:00:{start_second:02}Z"),
            ),
            &ReadStartInput {
                read_id: read_id.to_string(),
                invocation_id: invocation_id.clone(),
                resources: resources.clone(),
            },
        )
        .expect("read should start");
    store
        .read_complete(
            &context(
                &format!("{read_id}-complete-command"),
                task_id,
                agent_id,
                &format!("2026-08-02T00:00:{:02}Z", start_second + 1),
            ),
            &ReadCompleteInput {
                read_id: read_id.to_string(),
                invocation_id,
                resources,
                terminal_success: true,
                complete: true,
                stable: true,
                exact: true,
            },
        )
        .expect("read should complete");
}

fn prepare_write(
    store: &mut Store,
    task_id: &str,
    agent_id: &str,
    request_id: &str,
    invocation_id: &str,
    value: &str,
    second: u8,
) -> WritePrepareResult {
    store
        .write_prepare(
            &context(
                request_id,
                task_id,
                agent_id,
                &format!("2026-08-02T00:00:{second:02}Z"),
            ),
            &WritePrepareInput {
                invocation_id: invocation_id.to_string(),
                operation: MutationOperation::Update {
                    path: "src/lib.rs".to_string(),
                },
                current: observation(value),
                request_expires_at: "2026-08-02T00:30:00Z".to_string(),
                lease_expires_at: "2026-08-02T00:45:00Z".to_string(),
                attempt_deadline: "2026-08-02T00:10:00Z".to_string(),
            },
        )
        .expect("write prepare should return a decision")
}

#[allow(clippy::too_many_arguments)]
fn prepare_write_resources(
    store: &mut Store,
    task_id: &str,
    agent_id: &str,
    request_id: &str,
    invocation_id: &str,
    operation: MutationOperation,
    resources: Vec<ResourceObservation>,
    second: u8,
) -> WritePrepareResult {
    store
        .write_prepare(
            &context(
                request_id,
                task_id,
                agent_id,
                &format!("2026-08-02T00:00:{second:02}Z"),
            ),
            &WritePrepareInput {
                invocation_id: invocation_id.to_string(),
                operation,
                current: resources,
                request_expires_at: "2026-08-02T00:30:00Z".to_string(),
                lease_expires_at: "2026-08-02T00:45:00Z".to_string(),
                attempt_deadline: "2026-08-02T00:10:00Z".to_string(),
            },
        )
        .expect("write prepare should return a decision")
}

#[allow(clippy::too_many_arguments)]
fn complete_write(
    store: &mut Store,
    task_id: &str,
    agent_id: &str,
    request_id: &str,
    invocation_id: &str,
    attempt_id: &str,
    permit_id: &str,
    value: &str,
    second: u8,
) {
    let result = store
        .write_complete(
            &context(
                request_id,
                task_id,
                agent_id,
                &format!("2026-08-02T00:00:{second:02}Z"),
            ),
            &WriteCompleteInput {
                attempt_id: attempt_id.to_string(),
                permit_id: permit_id.to_string(),
                invocation_id: invocation_id.to_string(),
                terminal: WriteTerminal::Success,
                post_resources: observation(value),
                expected_post_resources: observation(value),
                error: None,
            },
        )
        .expect("write should complete");
    assert_eq!(result.status, WriteResultStatus::Completed);
}

#[test]
fn task_and_lease_deadlines_are_store_enforced() {
    let mut store = Store::open_in_memory().expect("store should open");
    let settings = CoordinationSettings::default();
    store
        .task_start(
            &context("start", "task-1", "agent-1", "2026-08-02T00:00:00Z"),
            &TaskStartInput {
                next_action: "edit src/lib.rs".to_string(),
                settings,
                expires_at: "2026-08-02T00:00:05Z".to_string(),
                runtime_process: None,
            },
        )
        .expect("canonical task lifetime should be accepted");
    assert!(matches!(
        store.task_heartbeat(
            &context(
                "late-heartbeat",
                "task-1",
                "agent-1",
                "2026-08-02T00:00:06Z"
            ),
            &TaskHeartbeatInput {
                next_action: "continue".to_string(),
                expires_at: "2026-08-02T00:00:11Z".to_string(),
            },
        ),
        Err(StoreError::InvalidState(_))
    ));
    assert!(matches!(
        store.task_start(
            &context(
                "too-long-start",
                "task-2",
                "agent-2",
                "2026-08-02T00:00:00Z"
            ),
            &TaskStartInput {
                next_action: "edit src/lib.rs".to_string(),
                settings,
                expires_at: "2026-08-02T00:00:06Z".to_string(),
                runtime_process: None,
            },
        ),
        Err(StoreError::InvalidInput(_))
    ));

    store
        .task_start(
            &context("writer-start", "task-3", "agent-3", "2026-08-02T00:00:00Z"),
            &TaskStartInput {
                next_action: "edit src/lib.rs".to_string(),
                settings,
                expires_at: "2026-08-02T00:00:05Z".to_string(),
                runtime_process: None,
            },
        )
        .expect("writer should start");
    assert!(matches!(
        store.write_prepare(
            &context("long-lease", "task-3", "agent-3", "2026-08-02T00:00:01Z"),
            &WritePrepareInput {
                invocation_id: "write-1".to_string(),
                operation: MutationOperation::Update {
                    path: "src/lib.rs".to_string(),
                },
                current: observation("before"),
                request_expires_at: "2026-08-02T00:02:01Z".to_string(),
                lease_expires_at: "2026-08-02T00:01:02Z".to_string(),
                attempt_deadline: "2026-08-02T00:10:00Z".to_string(),
            },
        ),
        Err(StoreError::InvalidInput(_))
    ));
}

#[test]
fn completed_read_attempt_is_one_shot() {
    let mut store = Store::open_in_memory().expect("store should open");
    start_task(&mut store, "task-1", "agent-1", 1);
    read_exact(&mut store, "task-1", "agent-1", "read-1", "before", 2);

    assert!(matches!(
        store.read_complete(
            &context("read-again", "task-1", "agent-1", "2026-08-02T00:00:04Z"),
            &ReadCompleteInput {
                read_id: "read-1".to_string(),
                invocation_id: "read-1-invocation".to_string(),
                resources: observation("before"),
                terminal_success: true,
                complete: true,
                stable: true,
                exact: true,
            },
        ),
        Err(StoreError::InvalidState(_))
    ));
}

#[test]
fn accepted_command_persists_one_event_projection_and_receipt() {
    let directory = tempfile::tempdir().expect("temporary directory should exist");
    let path = directory.path().join("state.db");
    {
        let mut store = Store::open(&path).expect("store should open");
        start_task(&mut store, "task-1", "agent-1", 1);
    }
    {
        let mut store = Store::open(&path).expect("store should reopen");
        start_task(&mut store, "task-1", "agent-1", 1);
    }
    let connection = rusqlite::Connection::open(path).expect("database should open");
    for table in ["tasks", "command_events", "command_receipts"] {
        let count: u64 = connection
            .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
                row.get(0)
            })
            .expect("count should load");
        assert_eq!(count, 1, "{table} should contain one durable command");
    }
}

#[test]
fn task_read_write_round_trip_is_idempotent() {
    let mut store = Store::open_in_memory().expect("store should open");
    start_task(&mut store, "task-1", "agent-1", 1);
    read_exact(&mut store, "task-1", "agent-1", "read-1", "before", 2);

    let prepared = prepare_write(
        &mut store,
        "task-1",
        "agent-1",
        "prepare-1",
        "write-1",
        "before",
        4,
    );
    let WritePrepareResult::Ready {
        attempt_id,
        permit_id,
        lease_batch_ids,
    } = prepared
    else {
        panic!("write should be ready");
    };
    assert_eq!(lease_batch_ids.len(), 1);

    complete_write(
        &mut store,
        "task-1",
        "agent-1",
        "complete-1",
        "write-1",
        &attempt_id,
        &permit_id,
        "after",
        5,
    );
    complete_write(
        &mut store,
        "task-1",
        "agent-1",
        "complete-1",
        "write-1",
        &attempt_id,
        &permit_id,
        "after",
        5,
    );

    let WritePrepareResult::Ready {
        attempt_id,
        permit_id,
        ..
    } = prepare_write(
        &mut store,
        "task-1",
        "agent-1",
        "prepare-2",
        "write-2",
        "after",
        6,
    )
    else {
        panic!("verified own write should authorize the next write");
    };
    complete_write(
        &mut store,
        "task-1",
        "agent-1",
        "complete-2",
        "write-2",
        &attempt_id,
        &permit_id,
        "after-again",
        7,
    );

    let finalized = store
        .task_finalize(
            &context("finalize-1", "task-1", "agent-1", "2026-08-02T00:00:08Z"),
            &TaskEndInput { handoff: None },
        )
        .expect("task should finalize");
    assert_eq!(finalized.status, TaskStatus::Completed);
    assert_eq!(
        store.status().expect("status should load").executing_writes,
        0
    );
}

#[test]
fn queued_writer_rereads_after_offer_and_activation_before_ready() {
    let mut store = Store::open_in_memory().expect("store should open");
    start_task(&mut store, "task-1", "agent-1", 1);
    read_exact(&mut store, "task-1", "agent-1", "read-1", "v1", 2);
    let WritePrepareResult::Ready {
        attempt_id,
        permit_id,
        lease_batch_ids,
    } = prepare_write(
        &mut store,
        "task-1",
        "agent-1",
        "prepare-1",
        "write-1",
        "v1",
        4,
    )
    else {
        panic!("first writer should be ready");
    };
    complete_write(
        &mut store,
        "task-1",
        "agent-1",
        "complete-1",
        "write-1",
        &attempt_id,
        &permit_id,
        "v2",
        5,
    );

    start_task(&mut store, "task-2", "agent-2", 6);
    read_exact(&mut store, "task-2", "agent-2", "read-2", "v2", 7);
    let WritePrepareResult::Queued { batch_id } = prepare_write(
        &mut store,
        "task-2",
        "agent-2",
        "prepare-2",
        "write-2",
        "v2",
        9,
    ) else {
        panic!("second writer should queue");
    };

    store
        .lease_release(
            &context("release-1", "task-1", "agent-1", "2026-08-02T00:00:10Z"),
            &LeaseReleaseInput {
                batch_id: lease_batch_ids[0].clone(),
            },
        )
        .expect("first lease should release");
    let offered = store
        .lease_request_status("workspace-1", "task-2", &batch_id, "2026-08-02T00:00:10Z")
        .expect("queued request should be offered");
    assert_eq!(offered.state, LeaseRequestState::Offered);
    let offer_id = offered.offer_id.expect("offer should have an id");

    let stale_activation = store
        .lease_activate(
            &context(
                "activate-stale",
                "task-2",
                "agent-2",
                "2026-08-02T00:00:11Z",
            ),
            &LeaseActivateInput {
                batch_id: batch_id.clone(),
                offer_id: offer_id.clone(),
                version: offered.version,
                lease_expires_at: "2026-08-02T00:45:00Z".to_string(),
            },
        )
        .expect("stale activation should return a decision");
    assert!(!stale_activation.active);

    read_exact(&mut store, "task-2", "agent-2", "read-3", "v2", 12);
    let WritePrepareResult::Queued {
        batch_id: retry_batch_id,
    } = prepare_write(
        &mut store,
        "task-2",
        "agent-2",
        "prepare-activation",
        "write-2-retry",
        "v2",
        13,
    )
    else {
        panic!("fresh retry should preserve the offered request");
    };
    assert_eq!(retry_batch_id, batch_id);
    let activated = store
        .lease_activate(
            &context(
                "activate-fresh",
                "task-2",
                "agent-2",
                "2026-08-02T00:00:14Z",
            ),
            &LeaseActivateInput {
                batch_id: batch_id.clone(),
                offer_id,
                version: offered.version,
                lease_expires_at: "2026-08-02T00:45:00Z".to_string(),
            },
        )
        .expect("fresh activation should return a decision");
    assert!(activated.active);
    read_exact(&mut store, "task-2", "agent-2", "read-4", "v2", 15);

    assert!(matches!(
        prepare_write(
            &mut store,
            "task-2",
            "agent-2",
            "prepare-4",
            "write-2",
            "v2",
            17,
        ),
        WritePrepareResult::Ready { .. }
    ));
}

#[test]
fn same_agent_disjoint_writer_queues_until_first_lease_releases() {
    let mut store = Store::open_in_memory().expect("store should open");
    start_task(&mut store, "task-1", "agent-1", 1);
    read_exact(&mut store, "task-1", "agent-1", "read-1", "v1", 2);
    let WritePrepareResult::Ready {
        attempt_id,
        permit_id,
        lease_batch_ids,
    } = prepare_write(
        &mut store,
        "task-1",
        "agent-1",
        "prepare-1",
        "write-1",
        "v1",
        4,
    )
    else {
        panic!("first writer should be ready");
    };
    complete_write(
        &mut store,
        "task-1",
        "agent-1",
        "complete-1",
        "write-1",
        &attempt_id,
        &permit_id,
        "v2",
        5,
    );

    start_task(&mut store, "task-2", "agent-1", 6);
    let second_resources = keyed_observation("object-2", "src/other.rs", "v1");
    read_exact_resources(
        &mut store,
        "task-2",
        "agent-1",
        "read-2",
        second_resources.clone(),
        7,
    );
    let WritePrepareResult::Queued { batch_id } = prepare_write_resources(
        &mut store,
        "task-2",
        "agent-1",
        "prepare-2",
        "write-2",
        MutationOperation::Update {
            path: "src/other.rs".to_string(),
        },
        second_resources.clone(),
        9,
    ) else {
        panic!("second task for the same agent should queue");
    };
    let status = store.status().expect("status should load");
    assert_eq!(status.active_leases, 1);
    assert_eq!(status.queued_requests, 1);

    store
        .lease_release(
            &context("release-1", "task-1", "agent-1", "2026-08-02T00:00:10Z"),
            &LeaseReleaseInput {
                batch_id: lease_batch_ids[0].clone(),
            },
        )
        .expect("first lease should release");
    let offered = store
        .lease_request_status("workspace-1", "task-2", &batch_id, "2026-08-02T00:00:10Z")
        .expect("queued request should be offered");
    assert_eq!(offered.state, LeaseRequestState::Offered);
    let offer_id = offered.offer_id.expect("offer should have an id");

    start_task(&mut store, "task-3", "agent-1", 11);
    read_exact(&mut store, "task-3", "agent-1", "read-3", "v2", 12);
    let WritePrepareResult::Ready {
        attempt_id: blocking_attempt_id,
        permit_id: blocking_permit_id,
        lease_batch_ids: blocking_lease_batch_ids,
    } = prepare_write(
        &mut store,
        "task-3",
        "agent-1",
        "prepare-3",
        "write-3",
        "v2",
        14,
    )
    else {
        panic!("new lease should be ready while the earlier request is only offered");
    };
    complete_write(
        &mut store,
        "task-3",
        "agent-1",
        "complete-3",
        "write-3",
        &blocking_attempt_id,
        &blocking_permit_id,
        "v3",
        15,
    );

    read_exact_resources(
        &mut store,
        "task-2",
        "agent-1",
        "read-4",
        second_resources.clone(),
        16,
    );
    let blocked_activation = store
        .lease_activate(
            &context(
                "activate-while-blocked",
                "task-2",
                "agent-1",
                "2026-08-02T00:00:18Z",
            ),
            &LeaseActivateInput {
                batch_id: batch_id.clone(),
                offer_id: offer_id.clone(),
                version: offered.version,
                lease_expires_at: "2026-08-02T00:45:00Z".to_string(),
            },
        )
        .expect("blocked activation should return a decision");
    assert!(!blocked_activation.active);

    store
        .lease_release(
            &context("release-3", "task-3", "agent-1", "2026-08-02T00:00:19Z"),
            &LeaseReleaseInput {
                batch_id: blocking_lease_batch_ids[0].clone(),
            },
        )
        .expect("blocking lease should release");
    let activated = store
        .lease_activate(
            &context("activate-2", "task-2", "agent-1", "2026-08-02T00:00:20Z"),
            &LeaseActivateInput {
                batch_id: batch_id.clone(),
                offer_id,
                version: offered.version,
                lease_expires_at: "2026-08-02T00:45:00Z".to_string(),
            },
        )
        .expect("offered lease should activate");
    assert!(activated.active);
    assert_eq!(store.status().expect("status should load").active_leases, 1);
    assert!(matches!(
        prepare_write_resources(
            &mut store,
            "task-2",
            "agent-1",
            "prepare-4",
            "write-2",
            MutationOperation::Update {
                path: "src/other.rs".to_string(),
            },
            second_resources,
            21,
        ),
        WritePrepareResult::Ready { .. }
    ));
}

#[test]
fn queued_request_growth_supersedes_old_batch_with_resource_union() {
    let directory = tempfile::tempdir().expect("temporary directory should exist");
    let path = directory.path().join("state.db");
    let mut store = Store::open(&path).expect("store should open");
    start_task(&mut store, "task-1", "agent-1", 1);
    read_exact(&mut store, "task-1", "agent-1", "read-1", "v1", 2);
    let WritePrepareResult::Ready {
        attempt_id,
        permit_id,
        lease_batch_ids,
    } = prepare_write(
        &mut store,
        "task-1",
        "agent-1",
        "prepare-1",
        "write-1",
        "v1",
        4,
    )
    else {
        panic!("first writer should be ready");
    };
    complete_write(
        &mut store,
        "task-1",
        "agent-1",
        "complete-1",
        "write-1",
        &attempt_id,
        &permit_id,
        "v2",
        5,
    );

    start_task(&mut store, "task-2", "agent-2", 6);
    let a = observation("v2");
    read_exact(&mut store, "task-2", "agent-2", "read-2", "v2", 7);
    let WritePrepareResult::Queued {
        batch_id: old_batch_id,
    } = prepare_write(
        &mut store,
        "task-2",
        "agent-2",
        "prepare-2",
        "write-2",
        "v2",
        9,
    )
    else {
        panic!("request for held resource should queue");
    };
    store
        .lease_release(
            &context("release-1", "task-1", "agent-1", "2026-08-02T00:00:10Z"),
            &LeaseReleaseInput {
                batch_id: lease_batch_ids[0].clone(),
            },
        )
        .expect("first lease should release");
    assert_eq!(
        store
            .lease_request_status(
                "workspace-1",
                "task-2",
                &old_batch_id,
                "2026-08-02T00:00:10Z"
            )
            .expect("old request should be offered")
            .state,
        LeaseRequestState::Offered
    );

    let b = absent_observation("src/other.rs");
    let resources = [a, b].concat();
    read_exact_resources(
        &mut store,
        "task-2",
        "agent-2",
        "read-3",
        resources.clone(),
        11,
    );
    let WritePrepareResult::Queued {
        batch_id: new_batch_id,
    } = prepare_write_resources(
        &mut store,
        "task-2",
        "agent-2",
        "prepare-3",
        "write-3",
        MutationOperation::Rename {
            old_path: "src/lib.rs".to_string(),
            new_path: "src/other.rs".to_string(),
            entry_only: false,
        },
        resources.clone(),
        13,
    )
    else {
        panic!("expanded request should be enqueued");
    };

    let connection = rusqlite::Connection::open(path).expect("database should open");
    let (state, superseded_by): (String, Option<String>) = connection
        .query_row(
            "SELECT state, superseded_by FROM lease_requests WHERE batch_id = ?1",
            rusqlite::params![&old_batch_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .expect("old request should load");
    assert_eq!(state, "superseded");
    assert_eq!(superseded_by.as_deref(), Some(new_batch_id.as_str()));
    let replacement_state: String = connection
        .query_row(
            "SELECT state FROM lease_requests WHERE batch_id = ?1",
            [&new_batch_id],
            |row| row.get(0),
        )
        .expect("replacement request should load");
    assert_eq!(replacement_state, "offered");
    let nonterminal_count: u64 = connection
        .query_row(
            "SELECT COUNT(*) FROM lease_requests
             WHERE workspace_id = ?1 AND task_id = ?2
               AND state IN ('queued', 'offered', 'activated')",
            rusqlite::params!["workspace-1", "task-2"],
            |row| row.get(0),
        )
        .expect("nonterminal request count should load");
    assert_eq!(nonterminal_count, 1);
    let resource_count: u64 = connection
        .query_row(
            "SELECT COUNT(*) FROM lease_request_resources WHERE batch_id = ?1",
            rusqlite::params![&new_batch_id],
            |row| row.get(0),
        )
        .expect("new request resource count should load");
    assert_eq!(resource_count, resources.len() as u64);
    for resource in resources {
        let count: u64 = connection
            .query_row(
                "SELECT COUNT(*) FROM lease_request_resources
                 WHERE batch_id = ?1 AND resource_id = ?2",
                rusqlite::params![&new_batch_id, &resource.resource().resource_id],
                |row| row.get(0),
            )
            .expect("new request resource should load");
        assert_eq!(count, 1);
    }
}

#[test]
fn delayed_read_completion_cannot_overwrite_newer_evidence() {
    let mut store = Store::open_in_memory().expect("store should open");
    start_task(&mut store, "task-1", "agent-1", 1);
    store
        .read_start(
            &context(
                "old-read-start",
                "task-1",
                "agent-1",
                "2026-08-02T00:00:02Z",
            ),
            &ReadStartInput {
                read_id: "old-read".to_string(),
                invocation_id: "old-invocation".to_string(),
                resources: observation("old"),
            },
        )
        .expect("old read should start");
    read_exact(&mut store, "task-1", "agent-1", "new-read", "new", 3);

    let delayed = store
        .read_complete(
            &context(
                "old-read-complete",
                "task-1",
                "agent-1",
                "2026-08-02T00:00:05Z",
            ),
            &ReadCompleteInput {
                read_id: "old-read".to_string(),
                invocation_id: "old-invocation".to_string(),
                resources: observation("old"),
                terminal_success: true,
                complete: true,
                stable: true,
                exact: true,
            },
        )
        .expect("delayed completion should return a terminal decision");
    assert_eq!(delayed.status, ReadResultStatus::Failed);
    assert!(matches!(
        prepare_write(
            &mut store,
            "task-1",
            "agent-1",
            "prepare-after-delayed-read",
            "write-after-delayed-read",
            "new",
            6,
        ),
        WritePrepareResult::Ready { .. }
    ));
}

#[test]
fn parallel_reads_do_not_block_writers_and_peer_writer_queues_without_mutating() {
    let mut store = Store::open_in_memory().expect("store should open");
    start_task(&mut store, "task-1", "agent-1", 1);
    start_task(&mut store, "task-2", "agent-2", 2);
    read_exact(&mut store, "task-1", "agent-1", "read-1", "shared", 3);
    read_exact(&mut store, "task-2", "agent-2", "read-2", "shared", 5);

    assert!(matches!(
        prepare_write(
            &mut store,
            "task-1",
            "agent-1",
            "prepare-1",
            "write-1",
            "shared",
            7,
        ),
        WritePrepareResult::Ready { .. }
    ));
    assert!(matches!(
        prepare_write(
            &mut store,
            "task-2",
            "agent-2",
            "prepare-2",
            "write-2",
            "shared",
            8,
        ),
        WritePrepareResult::Queued { .. }
    ));
}

#[test]
fn earlier_multi_resource_waiter_blocks_a_later_free_delta_writer() {
    let mut store = Store::open_in_memory().expect("store should open");
    start_task(&mut store, "task-1", "agent-1", 1);
    start_task(&mut store, "task-2", "agent-2", 2);
    start_task(&mut store, "task-3", "agent-3", 3);
    read_exact(&mut store, "task-1", "agent-1", "read-1", "x1", 4);
    let WritePrepareResult::Ready {
        attempt_id,
        permit_id,
        lease_batch_ids,
    } = prepare_write(
        &mut store,
        "task-1",
        "agent-1",
        "prepare-1",
        "write-1",
        "x1",
        6,
    )
    else {
        panic!("first writer should be ready");
    };
    complete_write(
        &mut store,
        "task-1",
        "agent-1",
        "complete-1",
        "write-1",
        &attempt_id,
        &permit_id,
        "x2",
        7,
    );

    let x = keyed_observation("object-1", "src/x.rs", "x2");
    let y = absent_observation("src/y.rs");
    read_exact_resources(
        &mut store,
        "task-2",
        "agent-2",
        "read-2",
        [x.clone(), y.clone()].concat(),
        8,
    );
    let WritePrepareResult::Queued { batch_id: first } = prepare_write_resources(
        &mut store,
        "task-2",
        "agent-2",
        "prepare-2",
        "write-2",
        MutationOperation::Rename {
            old_path: "src/x.rs".to_string(),
            new_path: "src/y.rs".to_string(),
            entry_only: false,
        },
        [x, y.clone()].concat(),
        10,
    ) else {
        panic!("multi-resource writer should queue");
    };

    read_exact_resources(&mut store, "task-3", "agent-3", "read-3", y.clone(), 11);
    let WritePrepareResult::Queued { batch_id: second } = prepare_write_resources(
        &mut store,
        "task-3",
        "agent-3",
        "prepare-3",
        "write-3",
        MutationOperation::Create {
            path: "src/y.rs".to_string(),
        },
        y,
        13,
    ) else {
        panic!("later free-delta writer must not overtake");
    };

    store
        .lease_release(
            &context("release-1", "task-1", "agent-1", "2026-08-02T00:00:14Z"),
            &LeaseReleaseInput {
                batch_id: lease_batch_ids[0].clone(),
            },
        )
        .expect("first lease should release");
    assert_eq!(
        store
            .lease_request_status("workspace-1", "task-2", &first, "2026-08-02T00:00:14Z",)
            .expect("first request should load")
            .state,
        LeaseRequestState::Offered
    );
    assert_eq!(
        store
            .lease_request_status("workspace-1", "task-3", &second, "2026-08-02T00:00:14Z",)
            .expect("second request should load")
            .state,
        LeaseRequestState::Queued
    );
}

#[test]
fn uncertain_write_keeps_a_draining_lease_until_explicit_release() {
    let mut store = Store::open_in_memory().expect("store should open");
    start_task(&mut store, "task-1", "agent-1", 1);
    read_exact(&mut store, "task-1", "agent-1", "read-1", "before", 2);
    let WritePrepareResult::Ready {
        attempt_id,
        permit_id,
        lease_batch_ids,
    } = prepare_write(
        &mut store,
        "task-1",
        "agent-1",
        "prepare-1",
        "write-1",
        "before",
        4,
    )
    else {
        panic!("write should be ready");
    };
    let result = store
        .write_complete(
            &context("uncertain-1", "task-1", "agent-1", "2026-08-02T00:00:05Z"),
            &WriteCompleteInput {
                attempt_id,
                permit_id,
                invocation_id: "write-1".to_string(),
                terminal: WriteTerminal::Uncertain,
                post_resources: Vec::new(),
                expected_post_resources: Vec::new(),
                error: Some("terminal result unavailable".to_string()),
            },
        )
        .expect("uncertain terminal should persist");
    assert_eq!(result.status, WriteResultStatus::Uncertain);
    let status = store.status().expect("status should load");
    assert_eq!(status.draining_leases, 1);
    assert_eq!(status.uncertain_writes, 1);

    let finalized = store
        .task_finalize(
            &context("finalize-1", "task-1", "agent-1", "2026-08-02T00:00:06Z"),
            &TaskEndInput { handoff: None },
        )
        .expect("task should begin draining");
    assert_eq!(finalized.status, TaskStatus::Draining);
    let released = store
        .lease_release(
            &context("release-1", "task-1", "agent-1", "2026-08-02T00:00:07Z"),
            &LeaseReleaseInput {
                batch_id: lease_batch_ids[0].clone(),
            },
        )
        .expect("verified recovery should release");
    assert_eq!(released.status, LeaseReleaseStatus::Released);
    let finalized = store
        .task_finalize(
            &context("finalize-2", "task-1", "agent-1", "2026-08-02T00:00:08Z"),
            &TaskEndInput { handoff: None },
        )
        .expect("released task should finalize");
    assert_eq!(finalized.status, TaskStatus::Completed);
}

#[test]
fn file_store_replays_live_write_state_and_receipt_after_restart() {
    let directory = tempfile::tempdir().expect("temporary directory should exist");
    let path = directory.path().join("state.db");
    let mut store = Store::open(&path).expect("file store should open");
    start_task(&mut store, "task-1", "agent-1", 1);
    read_exact(&mut store, "task-1", "agent-1", "read-1", "before", 2);
    let prepared = prepare_write(
        &mut store,
        "task-1",
        "agent-1",
        "prepare-1",
        "write-1",
        "before",
        4,
    );
    assert!(matches!(prepared, WritePrepareResult::Ready { .. }));
    drop(store);

    let mut reopened = Store::open(&path).expect("file store should reopen");
    let status = reopened.status().expect("status should survive restart");
    assert_eq!(status.active_leases, 1);
    assert_eq!(status.executing_writes, 1);
    assert_eq!(
        prepare_write(
            &mut reopened,
            "task-1",
            "agent-1",
            "prepare-1",
            "write-1",
            "before",
            4,
        ),
        prepared
    );
}

#[test]
fn independent_connections_serialize_same_resource_writers() {
    let directory = tempfile::tempdir().expect("temporary directory should exist");
    let path = directory.path().join("state.db");
    let mut first = Store::open(&path).expect("first store should open");
    let mut second = Store::open(&path).expect("second store should open");
    start_task(&mut first, "task-1", "agent-1", 1);
    start_task(&mut second, "task-2", "agent-2", 2);
    read_exact(&mut first, "task-1", "agent-1", "read-1", "shared", 3);
    read_exact(&mut second, "task-2", "agent-2", "read-2", "shared", 5);

    let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
    let first_barrier = barrier.clone();
    let first = std::thread::spawn(move || {
        first_barrier.wait();
        prepare_write(
            &mut first,
            "task-1",
            "agent-1",
            "prepare-1",
            "write-1",
            "shared",
            7,
        )
    });
    let second = std::thread::spawn(move || {
        barrier.wait();
        prepare_write(
            &mut second,
            "task-2",
            "agent-2",
            "prepare-2",
            "write-2",
            "shared",
            7,
        )
    });
    let results = [
        first.join().expect("first writer should finish"),
        second.join().expect("second writer should finish"),
    ];
    assert_eq!(
        results
            .iter()
            .filter(|result| matches!(result, WritePrepareResult::Ready { .. }))
            .count(),
        1
    );
    assert_eq!(
        results
            .iter()
            .filter(|result| matches!(result, WritePrepareResult::Queued { .. }))
            .count(),
        1
    );
}

#[test]
fn read_during_an_in_flight_peer_write_cannot_authorize_after_release() {
    let mut store = Store::open_in_memory().expect("store should open");
    start_task(&mut store, "task-1", "agent-1", 1);
    start_task(&mut store, "task-2", "agent-2", 2);
    read_exact(&mut store, "task-1", "agent-1", "read-1", "before", 3);
    let WritePrepareResult::Ready {
        attempt_id,
        permit_id,
        lease_batch_ids,
    } = prepare_write(
        &mut store,
        "task-1",
        "agent-1",
        "prepare-1",
        "write-1",
        "before",
        5,
    )
    else {
        panic!("first writer should be ready");
    };
    read_exact(&mut store, "task-2", "agent-2", "read-2", "before", 6);
    complete_write(
        &mut store,
        "task-1",
        "agent-1",
        "complete-1",
        "write-1",
        &attempt_id,
        &permit_id,
        "after",
        8,
    );
    store
        .lease_release(
            &context("release-1", "task-1", "agent-1", "2026-08-02T00:00:09Z"),
            &LeaseReleaseInput {
                batch_id: lease_batch_ids[0].clone(),
            },
        )
        .expect("first lease should release");
    assert!(matches!(
        prepare_write(
            &mut store,
            "task-2",
            "agent-2",
            "prepare-2",
            "write-2",
            "before",
            10,
        ),
        WritePrepareResult::RereadRequired { .. }
    ));
}

#[test]
fn overdue_write_stays_draining_until_supervisor_reports_terminal_state() {
    let mut store = Store::open_in_memory().expect("store should open");
    start_task(&mut store, "task-1", "agent-1", 1);
    read_exact(&mut store, "task-1", "agent-1", "read-1", "before", 2);
    let WritePrepareResult::Ready {
        attempt_id,
        permit_id,
        lease_batch_ids,
    } = prepare_write(
        &mut store,
        "task-1",
        "agent-1",
        "prepare-1",
        "write-1",
        "before",
        4,
    )
    else {
        panic!("write should be ready");
    };
    store
        .maintain("2026-08-02T00:11:00Z")
        .expect("maintenance should mark the overdue lease");
    let status = store.status().expect("status should load");
    assert_eq!(status.draining_leases, 1);
    assert_eq!(status.executing_writes, 1);
    store
        .write_complete(
            &context(
                "timeout-terminal",
                "task-1",
                "agent-1",
                "2026-08-02T00:11:01Z",
            ),
            &WriteCompleteInput {
                attempt_id,
                permit_id,
                invocation_id: "write-1".to_string(),
                terminal: WriteTerminal::Uncertain,
                post_resources: Vec::new(),
                expected_post_resources: Vec::new(),
                error: Some("supervisor verified process termination".to_string()),
            },
        )
        .expect("supervisor should close the timed-out attempt");
    let released = store
        .lease_release(
            &context(
                "timeout-release",
                "task-1",
                "agent-1",
                "2026-08-02T00:11:02Z",
            ),
            &LeaseReleaseInput {
                batch_id: lease_batch_ids[0].clone(),
            },
        )
        .expect("verified timeout recovery should release");
    assert_eq!(released.status, LeaseReleaseStatus::Released);
    assert_eq!(
        store
            .task_finalize(
                &context(
                    "timeout-finalize",
                    "task-1",
                    "agent-1",
                    "2026-08-02T00:11:03Z",
                ),
                &TaskEndInput { handoff: None },
            )
            .expect("recovered task should finalize")
            .status,
        TaskStatus::Completed
    );
}

#[test]
fn independent_connections_allow_disjoint_resource_writers() {
    let directory = tempfile::tempdir().expect("temporary directory should exist");
    let path = directory.path().join("state.db");
    let mut first = Store::open(&path).expect("first store should open");
    let mut second = Store::open(&path).expect("second store should open");
    start_task(&mut first, "task-1", "agent-1", 1);
    start_task(&mut second, "task-2", "agent-2", 2);
    read_exact_resources(
        &mut first,
        "task-1",
        "agent-1",
        "read-1",
        keyed_observation("object-1", "src/one.rs", "one"),
        3,
    );
    read_exact_resources(
        &mut second,
        "task-2",
        "agent-2",
        "read-2",
        keyed_observation("object-2", "src/two.rs", "two"),
        5,
    );

    let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
    let first_barrier = barrier.clone();
    let first = std::thread::spawn(move || {
        first_barrier.wait();
        prepare_write_resources(
            &mut first,
            "task-1",
            "agent-1",
            "prepare-1",
            "write-1",
            MutationOperation::Update {
                path: "src/one.rs".to_string(),
            },
            keyed_observation("object-1", "src/one.rs", "one"),
            7,
        )
    });
    let second = std::thread::spawn(move || {
        barrier.wait();
        prepare_write_resources(
            &mut second,
            "task-2",
            "agent-2",
            "prepare-2",
            "write-2",
            MutationOperation::Update {
                path: "src/two.rs".to_string(),
            },
            keyed_observation("object-2", "src/two.rs", "two"),
            7,
        )
    });
    for result in [
        first.join().expect("first writer should finish"),
        second.join().expect("second writer should finish"),
    ] {
        assert!(matches!(result, WritePrepareResult::Ready { .. }));
    }
}
