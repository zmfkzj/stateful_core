use std::{
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
};

use serde::Deserialize;
use serde_json::{Value, json};

use crate::{discover_runtime_with_optional_global, post_json};

#[derive(Debug, Clone, Deserialize)]
struct LocalOutboxRecord {
    outbox_id: String,
    event_type: String,
    session_id: String,
    workspace_id: String,
    sequence: u64,
    payload: Value,
    sync_status: String,
}

pub fn sync_outbox_in_repo(repo_root: impl AsRef<Path>) -> anyhow::Result<usize> {
    let repo_root = repo_root.as_ref();
    let runtime = discover_runtime_with_optional_global(repo_root)?;
    let outbox_dir = outbox_dir_path(repo_root);
    if !outbox_dir.is_dir() {
        return Ok(0);
    }

    let mut synced = 0_usize;
    for path in outbox_files(&outbox_dir)? {
        let mut records = read_pending_records(&path)?;
        records.sort_by(|left, right| {
            left.session_id
                .cmp(&right.session_id)
                .then(left.sequence.cmp(&right.sequence))
        });

        for record in records {
            let response = post_json(
                &runtime,
                "/v1/outbox/sync",
                &json!({
                    "outbox_id": record.outbox_id,
                    "session_id": record.session_id,
                    "workspace_id": record.workspace_id,
                    "sequence": record.sequence,
                    "event_type": record.event_type,
                    "payload": record.payload,
                }),
            )?;

            if !(200..300).contains(&response.status_code) {
                anyhow::bail!(
                    "outbox sync failed with HTTP {}: {}",
                    response.status_code,
                    response.body
                );
            }
            synced += 1;
        }

        fs::remove_file(path)?;
    }

    Ok(synced)
}

pub(crate) fn queue_session_heartbeat_outbox(
    repo_root: impl AsRef<Path>,
    runtime_workspace_id: &str,
    session_id: &str,
    reason: &str,
) -> anyhow::Result<()> {
    let repo_root = repo_root.as_ref();
    let outbox_dir = outbox_dir_path(repo_root);
    fs::create_dir_all(&outbox_dir)?;
    let path = outbox_dir.join(format!("{}.jsonl", safe_file_stem(session_id)));
    let sequence = next_sequence(&path)?;
    let record = json!({
        "outbox_id": uuid::Uuid::new_v4().to_string(),
        "event_type": "SessionHeartbeatQueued",
        "session_id": session_id,
        "actor_id": "unknown",
        "workspace_id": runtime_workspace_id,
        "sequence": sequence,
        "created_at": "2026-05-31T00:00:00Z",
        "payload": {
            "reason": reason
        },
        "sync_status": "pending"
    });

    let mut file = OpenOptions::new().create(true).append(true).open(path)?;
    writeln!(file, "{record}")?;
    Ok(())
}

fn safe_file_stem(value: &str) -> String {
    value.replace(['/', '\\'], "_")
}

fn next_sequence(path: &Path) -> anyhow::Result<u64> {
    if !path.is_file() {
        return Ok(1);
    }

    let count = fs::read_to_string(path)?
        .lines()
        .filter(|line| !line.trim().is_empty())
        .count();
    Ok(count as u64 + 1)
}

fn outbox_dir_path(repo_root: &Path) -> PathBuf {
    repo_root.join(".stateful_core").join("outbox")
}

fn outbox_files(outbox_dir: &Path) -> anyhow::Result<Vec<PathBuf>> {
    let mut files = fs::read_dir(outbox_dir)?
        .map(|entry| entry.map(|entry| entry.path()))
        .collect::<Result<Vec<_>, _>>()?;
    files.retain(|path| path.extension().is_some_and(|ext| ext == "jsonl"));
    files.sort();
    Ok(files)
}

fn read_pending_records(path: &Path) -> anyhow::Result<Vec<LocalOutboxRecord>> {
    let contents = fs::read_to_string(path)?;
    contents
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| serde_json::from_str::<LocalOutboxRecord>(line).map_err(Into::into))
        .filter_map(|record| match record {
            Ok(record) if record.sync_status == "pending" => Some(Ok(record)),
            Ok(_) => None,
            Err(error) => Some(Err(error)),
        })
        .collect()
}
