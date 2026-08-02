use std::{env, fs, path::Path, sync::mpsc, thread};

use anyhow::{Result, bail};
use clap::{Parser, Subcommand};
use serde::Serialize;
use stateful_core::{
    CoordinationSettings, MutationOperation, ResourceObservation, ResourceResolver,
};
use stateful_store::{
    CommandContext, ReadCompleteInput, ReadStartInput, Store, TaskStartInput, WriteCompleteInput,
    WritePrepareInput, WritePrepareResult, WriteResultStatus, WriteTerminal,
};
use uuid::Uuid;

const WORKSPACE_ID: &str = "smoke-workspace";
const TASK_EXPIRY: &str = "2026-08-02T01:00:00Z";
const REQUEST_EXPIRY: &str = "2026-08-02T00:30:00Z";
const LEASE_EXPIRY: &str = "2026-08-02T00:45:00Z";
const ATTEMPT_DEADLINE: &str = "2026-08-02T00:10:00Z";

#[derive(Parser)]
#[command(name = "stateful-bench", arg_required_else_help = true)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    Smoke,
}

#[derive(Debug, PartialEq, Eq, Serialize)]
struct SmokeResult {
    status: &'static str,
    same_file_conflict_rejected: bool,
    disjoint_files_allowed: bool,
}

pub fn run_cli() -> Result<()> {
    match Cli::parse().command {
        Command::Smoke => println!("{}", serde_json::to_string(&run_smoke()?)?),
    }
    Ok(())
}

fn run_smoke() -> Result<SmokeResult> {
    let directory = env::temp_dir().join(format!("stateful-bench-smoke-{}", Uuid::new_v4()));
    fs::create_dir(&directory)?;
    let result = smoke_at(&directory.join("stateful.sqlite3"));
    let _ = fs::remove_dir_all(directory);
    result
}

fn smoke_at(database: &Path) -> Result<SmokeResult> {
    let mut first = Store::open(database)?;
    let mut second = Store::open(database)?;
    let root = database
        .parent()
        .ok_or_else(|| anyhow::anyhow!("smoke database has no parent directory"))?;
    fs::create_dir_all(root.join("src"))?;
    for path in ["src/shared.rs", "src/left.rs", "src/right.rs"] {
        fs::write(root.join(path), path)?;
    }
    let resolver = ResourceResolver::new(WORKSPACE_ID, root)?;

    start_task(&mut first, "task-a", "agent-a", "2026-08-02T00:00:00Z")?;
    start_task(&mut second, "task-b", "agent-b", "2026-08-02T00:00:00Z")?;

    let shared = observation(&resolver, "src/shared.rs")?;
    read_exact(
        &mut first,
        "task-a",
        "agent-a",
        "read-a",
        &shared,
        "2026-08-02T00:00:01Z",
        "2026-08-02T00:00:02Z",
    )?;
    if !matches!(
        prepare_write(
            &mut first,
            "task-a",
            "agent-a",
            "write-a",
            "src/shared.rs",
            &shared,
            "2026-08-02T00:00:03Z",
        )?,
        WritePrepareResult::Ready { .. }
    ) {
        bail!("first same-file write was not granted");
    }

    read_exact(
        &mut second,
        "task-b",
        "agent-b",
        "read-b",
        &shared,
        "2026-08-02T00:00:04Z",
        "2026-08-02T00:00:05Z",
    )?;
    let same_file_conflict_rejected = matches!(
        prepare_write(
            &mut second,
            "task-b",
            "agent-b",
            "write-b",
            "src/shared.rs",
            &shared,
            "2026-08-02T00:00:06Z",
        )?,
        WritePrepareResult::Queued { .. }
    );
    if !same_file_conflict_rejected {
        bail!("second same-file write was not queued");
    }

    start_task(&mut first, "task-c", "agent-c", "2026-08-02T00:00:10Z")?;
    start_task(&mut second, "task-d", "agent-d", "2026-08-02T00:00:10Z")?;
    let left = observation(&resolver, "src/left.rs")?;
    let right = observation(&resolver, "src/right.rs")?;
    read_exact(
        &mut first,
        "task-c",
        "agent-c",
        "read-c",
        &left,
        "2026-08-02T00:00:11Z",
        "2026-08-02T00:00:12Z",
    )?;
    read_exact(
        &mut second,
        "task-d",
        "agent-d",
        "read-d",
        &right,
        "2026-08-02T00:00:11Z",
        "2026-08-02T00:00:12Z",
    )?;
    let (left_attempt, left_permit) = ready_attempt(
        prepare_write(
            &mut first,
            "task-c",
            "agent-c",
            "write-c",
            "src/left.rs",
            &left,
            "2026-08-02T00:00:13Z",
        )?,
        "left",
    )?;
    let (left_started_tx, left_started_rx) = mpsc::channel();
    let (left_finish_tx, left_finish_rx) = mpsc::channel();
    let left_path = root.join("src/left.rs");
    let left_thread = thread::spawn(move || -> Result<()> {
        left_started_tx
            .send(())
            .map_err(|_| anyhow::anyhow!("left mutation start receiver dropped"))?;
        let write_result = fs::write(left_path, b"left-after");
        left_finish_rx
            .recv()
            .map_err(|_| anyhow::anyhow!("left mutation finish sender dropped"))?;
        write_result?;
        Ok(())
    });
    left_started_rx
        .recv()
        .map_err(|_| anyhow::anyhow!("left mutation thread ended before starting"))?;

    let right_prepared = prepare_write(
        &mut second,
        "task-d",
        "agent-d",
        "write-d",
        "src/right.rs",
        &right,
        "2026-08-02T00:00:13Z",
    )
    .and_then(|result| ready_attempt(result, "right"));
    let (right_attempt, right_permit) = match right_prepared {
        Ok(ready) => ready,
        Err(error) => {
            let _ = left_finish_tx.send(());
            let _ = left_thread.join();
            return Err(error);
        }
    };

    let right_path = root.join("src/right.rs");
    let right_thread = thread::spawn(move || fs::write(right_path, b"right-after"));
    let right_result = right_thread
        .join()
        .map_err(|_| anyhow::anyhow!("right mutation thread panicked"));
    let finish_result = left_finish_tx.send(());
    let left_result = left_thread
        .join()
        .map_err(|_| anyhow::anyhow!("left mutation thread panicked"));
    right_result??;
    finish_result.map_err(|_| anyhow::anyhow!("left mutation thread ended before release"))?;
    left_result??;

    let left_post = observation(&resolver, "src/left.rs")?;
    let right_post = observation(&resolver, "src/right.rs")?;
    let left_status = complete_write(
        &mut first,
        "task-c",
        "agent-c",
        "write-c",
        left_attempt,
        left_permit,
        &left_post,
    )?;
    let right_status = complete_write(
        &mut second,
        "task-d",
        "agent-d",
        "write-d",
        right_attempt,
        right_permit,
        &right_post,
    )?;
    let disjoint_files_allowed = left_status == WriteResultStatus::Completed
        && right_status == WriteResultStatus::Completed
        && fs::read(root.join("src/left.rs"))? == b"left-after"
        && fs::read(root.join("src/right.rs"))? == b"right-after";
    if !disjoint_files_allowed {
        bail!("concurrently granted disjoint writes did not complete");
    }

    Ok(SmokeResult {
        status: "ok",
        same_file_conflict_rejected,
        disjoint_files_allowed,
    })
}

fn context(task_id: &str, agent_id: &str, request_id: &str, observed_at: &str) -> CommandContext {
    CommandContext {
        request_id: request_id.to_string(),
        task_id: task_id.to_string(),
        agent_id: agent_id.to_string(),
        workspace_id: WORKSPACE_ID.to_string(),
        observed_at: observed_at.to_string(),
    }
}

fn start_task(store: &mut Store, task_id: &str, agent_id: &str, observed_at: &str) -> Result<()> {
    store.task_start(
        &context(task_id, agent_id, &format!("{task_id}-start"), observed_at),
        &TaskStartInput {
            next_action: "write".to_string(),
            settings: CoordinationSettings {
                heartbeat_interval_seconds: 60,
                inactivity_timeout_seconds: 3_600,
                lease_expiry_seconds: 7_200,
                offer_ttl_seconds: 7_200,
            },
            expires_at: TASK_EXPIRY.to_string(),
            runtime_process: None,
        },
    )?;
    Ok(())
}

fn read_exact(
    store: &mut Store,
    task_id: &str,
    agent_id: &str,
    read_id: &str,
    resources: &[ResourceObservation],
    started_at: &str,
    completed_at: &str,
) -> Result<()> {
    let invocation_id = format!("{read_id}-invocation");
    store.read_start(
        &context(task_id, agent_id, &format!("{read_id}-start"), started_at),
        &ReadStartInput {
            read_id: read_id.to_string(),
            invocation_id: invocation_id.clone(),
            resources: resources.to_vec(),
        },
    )?;
    store.read_complete(
        &context(
            task_id,
            agent_id,
            &format!("{read_id}-complete"),
            completed_at,
        ),
        &ReadCompleteInput {
            read_id: read_id.to_string(),
            invocation_id,
            resources: resources.to_vec(),
            terminal_success: true,
            complete: true,
            stable: true,
            exact: true,
        },
    )?;
    Ok(())
}

fn prepare_write(
    store: &mut Store,
    task_id: &str,
    agent_id: &str,
    request_id: &str,
    path: &str,
    current: &[ResourceObservation],
    observed_at: &str,
) -> Result<WritePrepareResult> {
    Ok(store.write_prepare(
        &context(task_id, agent_id, request_id, observed_at),
        &WritePrepareInput {
            invocation_id: format!("{request_id}-invocation"),
            operation: MutationOperation::Update {
                path: path.to_string(),
            },
            current: current.to_vec(),
            request_expires_at: REQUEST_EXPIRY.to_string(),
            lease_expires_at: LEASE_EXPIRY.to_string(),
            attempt_deadline: ATTEMPT_DEADLINE.to_string(),
        },
    )?)
}

fn ready_attempt(result: WritePrepareResult, label: &str) -> Result<(String, String)> {
    match result {
        WritePrepareResult::Ready {
            attempt_id,
            permit_id,
            ..
        } => Ok((attempt_id, permit_id)),
        other => bail!("{label} disjoint write was not granted: {other:?}"),
    }
}

fn complete_write(
    store: &mut Store,
    task_id: &str,
    agent_id: &str,
    request_id: &str,
    attempt_id: String,
    permit_id: String,
    post: &[ResourceObservation],
) -> Result<WriteResultStatus> {
    Ok(store
        .write_complete(
            &context(
                task_id,
                agent_id,
                &format!("{request_id}-complete"),
                "2026-08-02T00:00:14Z",
            ),
            &WriteCompleteInput {
                attempt_id,
                permit_id,
                invocation_id: format!("{request_id}-invocation"),
                terminal: WriteTerminal::Success,
                post_resources: post.to_vec(),
                expected_post_resources: post.to_vec(),
                error: None,
            },
        )?
        .status)
}

fn observation(resolver: &ResourceResolver, path: &str) -> Result<Vec<ResourceObservation>> {
    Ok(resolver.observe_operation(&MutationOperation::Update {
        path: path.to_string(),
    })?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn smoke_proves_exclusive_and_disjoint_writes() {
        assert_eq!(
            run_smoke().expect("smoke scenario should succeed"),
            SmokeResult {
                status: "ok",
                same_file_conflict_rejected: true,
                disjoint_files_allowed: true,
            }
        );
    }
}
