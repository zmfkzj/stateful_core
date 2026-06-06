use std::{
    collections::BTreeSet,
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant, SystemTime},
};

#[cfg(unix)]
use std::os::unix::fs::MetadataExt;

use serde::Deserialize;
use serde_json::{Value, json};

use crate::{ServerRuntime, discover_runtime_with_optional_global, post_json};

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

#[derive(Debug, Clone)]
struct PendingOutboxRecord {
    record: LocalOutboxRecord,
    raw: Value,
}

pub fn sync_outbox_in_repo(repo_root: impl AsRef<Path>) -> anyhow::Result<usize> {
    let repo_root = repo_root.as_ref();
    let runtime = discover_runtime_with_optional_global(repo_root)?;
    sync_outbox_in_repo_with_runtime(repo_root, &runtime)
}

pub fn sync_outbox_in_repo_with_runtime(
    repo_root: impl AsRef<Path>,
    runtime: &ServerRuntime,
) -> anyhow::Result<usize> {
    let repo_root = repo_root.as_ref();
    let Some(outbox_dir) = existing_trusted_outbox_dir(repo_root)? else {
        return Ok(0);
    };

    let pending_paths = {
        let _lock = acquire_outbox_lock(&outbox_dir)?;
        recover_claimed_outbox_files(&outbox_dir)?;
        outbox_files(&outbox_dir)?
    };

    let mut synced = 0_usize;
    for path in pending_paths {
        let Some(file_name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        let (claimed_path, mut active_claim) = {
            let _lock = acquire_outbox_lock(&outbox_dir)?;
            recover_claimed_outbox_files(&outbox_dir)?;
            if !path.exists() {
                continue;
            }
            let claimed_path = claim_outbox_file(&path)?;
            let active_claim = ActiveOutboxClaim::start(&claimed_path)?;
            (claimed_path, active_claim)
        };

        let mut records = read_pending_records(&claimed_path)?;
        records.sort_by(|left, right| {
            left.record
                .session_id
                .cmp(&right.record.session_id)
                .then(left.record.sequence.cmp(&right.record.sequence))
        });

        for index in 0..records.len() {
            let record = &records[index].record;
            let response = match post_json(
                runtime,
                "/v1/outbox/sync",
                &json!({
                    "outbox_id": record.outbox_id,
                    "session_id": record.session_id,
                    "workspace_id": record.workspace_id,
                    "sequence": record.sequence,
                    "event_type": record.event_type,
                    "payload": record.payload,
                }),
            ) {
                Ok(response) => response,
                Err(error) => {
                    let _lock = acquire_outbox_lock(&outbox_dir)?;
                    requeue_pending_records(&path, &records[index..])?;
                    fs::remove_file(&claimed_path)?;
                    active_claim.finish();
                    return Err(error);
                }
            };

            if !(200..300).contains(&response.status_code) {
                let _lock = acquire_outbox_lock(&outbox_dir)?;
                requeue_pending_records(&path, &records[index..])?;
                fs::remove_file(&claimed_path)?;
                active_claim.finish();
                anyhow::bail!(
                    "outbox sync failed with HTTP {}: {}",
                    response.status_code,
                    response.body
                );
            }
            synced += 1;
        }

        let _lock = acquire_outbox_lock(&outbox_dir)?;
        fs::remove_file(&claimed_path)?;
        active_claim.finish();
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
    let outbox_dir = ensure_trusted_outbox_dir(repo_root)?;
    let _lock = acquire_outbox_lock(&outbox_dir)?;
    recover_claimed_outbox_files(&outbox_dir)?;
    let stem = safe_file_stem(session_id);
    let path = outbox_dir.join(format!("{stem}.jsonl"));
    let sequence = next_sequence(&outbox_dir, &stem)?;
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

    let mut file = open_plain_outbox_append(&path, "outbox file")?;
    writeln!(file, "{record}")?;
    Ok(())
}

fn safe_file_stem(value: &str) -> String {
    value.replace(['/', '\\'], "_")
}

fn next_sequence(outbox_dir: &Path, stem: &str) -> anyhow::Result<u64> {
    let counter_path = outbox_dir.join(format!("{stem}.sequence"));
    let counter = read_outbox_counter(&counter_path)?;
    let base_name = format!("{stem}.jsonl");
    let claimed_prefix = format!("{base_name}.syncing-");
    let mut max_sequence = counter;
    for path in fs::read_dir(outbox_dir)?
        .map(|entry| entry.map(|entry| entry.path()))
        .collect::<Result<Vec<_>, _>>()?
    {
        let Some(file_name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        if file_name != base_name && !file_name.starts_with(&claimed_prefix) {
            continue;
        }
        max_sequence = max_sequence.max(max_pending_sequence_in_file(&path)?);
    }
    let next = max_sequence
        .checked_add(1)
        .ok_or_else(|| anyhow::anyhow!("outbox sequence overflow"))?;
    let tmp_path =
        counter_path.with_file_name(format!("{stem}.sequence.tmp-{}", uuid::Uuid::new_v4()));
    fs::write(&tmp_path, format!("{next}\n"))?;
    fs::rename(tmp_path, counter_path)?;
    Ok(next)
}

fn read_outbox_counter(counter_path: &Path) -> anyhow::Result<u64> {
    match fs::symlink_metadata(counter_path) {
        Ok(_) => {
            ensure_existing_plain_file(counter_path, "outbox sequence file")?;
            Ok(fs::read_to_string(counter_path)
                .ok()
                .and_then(|value| value.trim().parse::<u64>().ok())
                .unwrap_or(0))
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(0),
        Err(error) => Err(error.into()),
    }
}

fn max_pending_sequence_in_file(path: &Path) -> anyhow::Result<u64> {
    ensure_existing_plain_file(path, "outbox file")?;
    let contents = fs::read_to_string(path)?;
    let mut max_sequence = 0_u64;
    for line in contents.lines().filter(|line| !line.trim().is_empty()) {
        let Ok(raw) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        if raw.get("sync_status") != Some(&Value::String("pending".to_string())) {
            continue;
        }
        if let Some(sequence) = raw.get("sequence").and_then(Value::as_u64) {
            max_sequence = max_sequence.max(sequence);
        }
    }
    Ok(max_sequence)
}

fn outbox_dir_path(repo_root: &Path) -> PathBuf {
    repo_root.join(".stateful_core").join("outbox")
}

fn existing_trusted_outbox_dir(repo_root: &Path) -> anyhow::Result<Option<PathBuf>> {
    let state_dir = repo_root.join(".stateful_core");
    if !existing_plain_directory(&state_dir, "stateful directory")? {
        return Ok(None);
    }
    let outbox_dir = outbox_dir_path(repo_root);
    if !existing_plain_directory(&outbox_dir, "outbox directory")? {
        return Ok(None);
    }
    ensure_path_stays_in_repo(repo_root, &outbox_dir)?;
    Ok(Some(outbox_dir))
}

fn ensure_trusted_outbox_dir(repo_root: &Path) -> anyhow::Result<PathBuf> {
    let state_dir = repo_root.join(".stateful_core");
    ensure_plain_directory(&state_dir, "stateful directory")?;
    let outbox_dir = outbox_dir_path(repo_root);
    ensure_plain_directory(&outbox_dir, "outbox directory")?;
    ensure_path_stays_in_repo(repo_root, &outbox_dir)?;
    Ok(outbox_dir)
}

fn existing_plain_directory(path: &Path, label: &str) -> anyhow::Result<bool> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            anyhow::bail!("stateful outbox refuses symlinked {label}");
        }
        Ok(metadata) => Ok(metadata.is_dir()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error.into()),
    }
}

fn existing_plain_file(path: &Path, label: &str) -> anyhow::Result<bool> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            anyhow::bail!("stateful outbox refuses symlinked {label}");
        }
        Ok(metadata) if is_hard_linked_file(&metadata) => {
            anyhow::bail!("stateful outbox refuses hard-linked {label}");
        }
        Ok(metadata) => Ok(metadata.is_file()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error.into()),
    }
}

fn ensure_existing_plain_file(path: &Path, label: &str) -> anyhow::Result<()> {
    if !existing_plain_file(path, label)? {
        anyhow::bail!("stateful outbox {label} is not a regular file");
    }
    Ok(())
}

fn open_plain_outbox_append(path: &Path, label: &str) -> anyhow::Result<fs::File> {
    let file = match fs::symlink_metadata(path) {
        Ok(_) => {
            ensure_existing_plain_file(path, label)?;
            OpenOptions::new().append(true).open(path)?
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            OpenOptions::new().create_new(true).append(true).open(path)?
        }
        Err(error) => return Err(error.into()),
    };
    ensure_existing_plain_file(path, label)?;
    let metadata = file.metadata()?;
    if is_hard_linked_file(&metadata) {
        anyhow::bail!("stateful outbox refuses hard-linked {label}");
    }
    Ok(file)
}

#[cfg(unix)]
fn is_hard_linked_file(metadata: &fs::Metadata) -> bool {
    metadata.is_file() && metadata.nlink() > 1
}

#[cfg(not(unix))]
fn is_hard_linked_file(_metadata: &fs::Metadata) -> bool {
    false
}

fn ensure_plain_directory(path: &Path, label: &str) -> anyhow::Result<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            anyhow::bail!("stateful outbox refuses symlinked {label}");
        }
        Ok(metadata) if metadata.is_dir() => Ok(()),
        Ok(_) => anyhow::bail!("stateful outbox {label} is not a directory"),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            fs::create_dir(path)?;
            Ok(())
        }
        Err(error) => Err(error.into()),
    }
}

fn ensure_path_stays_in_repo(repo_root: &Path, path: &Path) -> anyhow::Result<()> {
    let canonical_repo = repo_root.canonicalize()?;
    let canonical_path = path.canonicalize()?;
    if !canonical_path.starts_with(canonical_repo) {
        anyhow::bail!("stateful outbox path escapes the repo");
    }
    Ok(())
}

fn outbox_files(outbox_dir: &Path) -> anyhow::Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    for entry in fs::read_dir(outbox_dir)? {
        let path = entry?.path();
        if !path.extension().is_some_and(|ext| ext == "jsonl") {
            continue;
        }
        if existing_plain_file(&path, "outbox file")? {
            files.push(path);
        }
    }
    files.sort();
    Ok(files)
}

fn recover_claimed_outbox_files(outbox_dir: &Path) -> anyhow::Result<()> {
    let mut claimed_files = Vec::new();
    for entry in fs::read_dir(outbox_dir)? {
        let path = entry?.path();
        let is_claimed = path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.contains(".jsonl.syncing-") && !name.ends_with(".active"));
        if is_claimed && existing_plain_file(&path, "claimed outbox file")? {
            claimed_files.push(path);
        }
    }
    claimed_files.sort();

    for claimed in claimed_files {
        if active_outbox_claim_is_fresh(&claimed)? {
            continue;
        }
        let _ = fs::remove_file(active_outbox_claim_marker_path(&claimed));
        let file_name = claimed
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| anyhow::anyhow!("claimed outbox file has no UTF-8 file name"))?;
        let Some((base_name, _suffix)) = file_name.split_once(".syncing-") else {
            continue;
        };
        let base_path = outbox_dir.join(base_name);
        merge_claimed_records(&base_path, &claimed)?;
    }

    Ok(())
}

fn claim_outbox_file(path: &Path) -> anyhow::Result<PathBuf> {
    if !existing_plain_file(path, "outbox file")? {
        anyhow::bail!("stateful outbox file is not a regular file");
    }
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| anyhow::anyhow!("outbox file has no UTF-8 file name"))?;
    let claimed = path.with_file_name(format!("{file_name}.syncing-{}", uuid::Uuid::new_v4()));
    fs::rename(path, &claimed)?;
    if !existing_plain_file(&claimed, "claimed outbox file")? {
        anyhow::bail!("stateful claimed outbox file is not a regular file");
    }
    Ok(claimed)
}

fn active_outbox_claim_marker_path(claimed_path: &Path) -> PathBuf {
    claimed_path.with_file_name(format!(
        "{}.active",
        claimed_path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("outbox.jsonl.syncing")
    ))
}

fn active_outbox_claim_is_fresh(claimed_path: &Path) -> anyhow::Result<bool> {
    match fs::symlink_metadata(active_outbox_claim_marker_path(claimed_path)) {
        Ok(metadata)
            if metadata.file_type().is_symlink()
                || !metadata.is_file()
                || is_hard_linked_file(&metadata) =>
        {
            Ok(false)
        }
        Ok(metadata) => Ok(metadata
            .modified()
            .ok()
            .and_then(|modified| modified.elapsed().ok())
            .is_some_and(|elapsed| elapsed < OUTBOX_LOCK_STALE_AFTER)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error.into()),
    }
}

struct ActiveOutboxClaim {
    marker_path: PathBuf,
    stop_heartbeat: Arc<AtomicBool>,
    heartbeat: Option<JoinHandle<()>>,
}

impl ActiveOutboxClaim {
    fn start(claimed_path: &Path) -> anyhow::Result<Self> {
        let marker_path = active_outbox_claim_marker_path(claimed_path);
        fs::write(&marker_path, format!("pid={}\n", std::process::id()))?;
        let stop_heartbeat = Arc::new(AtomicBool::new(false));
        let stop_for_thread = Arc::clone(&stop_heartbeat);
        let marker_for_thread = marker_path.clone();
        let heartbeat = thread::spawn(move || {
            while !stop_for_thread.load(Ordering::Relaxed) {
                let _ = fs::write(&marker_for_thread, "active\n");
                let start = Instant::now();
                while start.elapsed() < OUTBOX_LOCK_HEARTBEAT_INTERVAL {
                    if stop_for_thread.load(Ordering::Relaxed) {
                        return;
                    }
                    thread::sleep(Duration::from_millis(50));
                }
            }
        });

        Ok(Self {
            marker_path,
            stop_heartbeat,
            heartbeat: Some(heartbeat),
        })
    }

    fn finish(&mut self) {
        self.stop_heartbeat.store(true, Ordering::Relaxed);
        if let Some(heartbeat) = self.heartbeat.take() {
            let _ = heartbeat.join();
        }
        let _ = fs::remove_file(&self.marker_path);
    }
}

impl Drop for ActiveOutboxClaim {
    fn drop(&mut self) {
        self.finish();
    }
}

const OUTBOX_LOCK_WAIT: Duration = Duration::from_secs(60);
const OUTBOX_LOCK_STALE_AFTER: Duration = Duration::from_secs(30);
const OUTBOX_LOCK_HEARTBEAT_INTERVAL: Duration = Duration::from_secs(1);

struct OutboxLock {
    path: PathBuf,
    stop_heartbeat: Arc<AtomicBool>,
    heartbeat: Option<JoinHandle<()>>,
}

impl OutboxLock {
    fn new(path: PathBuf) -> anyhow::Result<Self> {
        let owner_path = path.join("owner");
        let heartbeat_path = path.join("heartbeat");
        fs::write(
            &owner_path,
            format!("pid={}\n", std::process::id()),
        )?;
        fs::write(&heartbeat_path, "held\n")?;

        let stop_heartbeat = Arc::new(AtomicBool::new(false));
        let stop_for_thread = Arc::clone(&stop_heartbeat);
        let heartbeat_for_thread = heartbeat_path.clone();
        let heartbeat = thread::spawn(move || {
            while !stop_for_thread.load(Ordering::Relaxed) {
                let _ = fs::write(&heartbeat_for_thread, "held\n");
                let start = Instant::now();
                while start.elapsed() < OUTBOX_LOCK_HEARTBEAT_INTERVAL {
                    if stop_for_thread.load(Ordering::Relaxed) {
                        return;
                    }
                    thread::sleep(Duration::from_millis(50));
                }
            }
        });

        Ok(Self {
            path,
            stop_heartbeat,
            heartbeat: Some(heartbeat),
        })
    }
}

impl Drop for OutboxLock {
    fn drop(&mut self) {
        self.stop_heartbeat.store(true, Ordering::Relaxed);
        if let Some(heartbeat) = self.heartbeat.take() {
            let _ = heartbeat.join();
        }
        let _ = fs::remove_file(self.path.join("heartbeat"));
        let _ = fs::remove_file(self.path.join("owner"));
        let _ = fs::remove_dir(&self.path);
    }
}

fn acquire_outbox_lock(outbox_dir: &Path) -> anyhow::Result<OutboxLock> {
    fs::create_dir_all(outbox_dir)?;
    let lock_path = outbox_dir.join(".lock");
    let start = Instant::now();
    loop {
        match fs::create_dir(&lock_path) {
            Ok(()) => return OutboxLock::new(lock_path),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                if remove_stale_outbox_lock(&lock_path)? {
                    continue;
                }
                if start.elapsed() >= OUTBOX_LOCK_WAIT {
                    anyhow::bail!("timed out waiting for outbox lock {}", lock_path.display());
                }
                thread::sleep(Duration::from_millis(25));
            }
            Err(error) => return Err(error.into()),
        }
    }
}

fn remove_stale_outbox_lock(lock_path: &Path) -> anyhow::Result<bool> {
    match fs::symlink_metadata(lock_path) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
            return remove_outbox_lock_path(lock_path);
        }
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(true),
        Err(error) => return Err(error.into()),
    }

    let heartbeat_path = lock_path.join("heartbeat");
    let metadata = match fs::symlink_metadata(&heartbeat_path) {
        Ok(metadata)
            if metadata.file_type().is_symlink()
                || !metadata.is_file()
                || is_hard_linked_file(&metadata) =>
        {
            return remove_outbox_lock_path(lock_path);
        }
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            fs::symlink_metadata(lock_path)?
        }
        Err(error) => return Err(error.into()),
    };

    let modified = metadata.modified().unwrap_or(SystemTime::now());
    if modified.elapsed().unwrap_or_default() < OUTBOX_LOCK_STALE_AFTER {
        return Ok(false);
    }

    let stale_path = lock_path.with_file_name(format!(".lock.stale-{}", uuid::Uuid::new_v4()));
    match fs::rename(lock_path, &stale_path) {
        Ok(()) => {
            remove_outbox_lock_path_after_rename(&stale_path)?;
            Ok(true)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(true),
        Err(error) => Err(error.into()),
    }
}

fn remove_outbox_lock_path(lock_path: &Path) -> anyhow::Result<bool> {
    let stale_path = lock_path.with_file_name(format!(".lock.stale-{}", uuid::Uuid::new_v4()));
    match fs::rename(lock_path, &stale_path) {
        Ok(()) => {
            remove_outbox_lock_path_after_rename(&stale_path)?;
            Ok(true)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(true),
        Err(error) => Err(error.into()),
    }
}

fn remove_outbox_lock_path_after_rename(path: &Path) -> anyhow::Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    if metadata.is_dir() && !metadata.file_type().is_symlink() {
        fs::remove_dir_all(path)?;
    } else {
        fs::remove_file(path)?;
    }
    Ok(())
}

fn read_pending_records(path: &Path) -> anyhow::Result<Vec<PendingOutboxRecord>> {
    let contents = fs::read_to_string(path)?;
    let mut records = Vec::new();
    for line in contents.lines().filter(|line| !line.trim().is_empty()) {
        let Ok(raw) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        let Ok(record) = serde_json::from_value::<LocalOutboxRecord>(raw.clone()) else {
            continue;
        };
        if record.sync_status == "pending" {
            records.push(PendingOutboxRecord { record, raw });
        }
    }
    Ok(records)
}

fn merge_claimed_records(base_path: &Path, claimed_path: &Path) -> anyhow::Result<()> {
    let Some(parent) = base_path.parent() else {
        anyhow::bail!("outbox file path has no parent");
    };
    fs::create_dir_all(parent)?;
    ensure_existing_plain_file(claimed_path, "claimed outbox file")?;
    if !existing_plain_file(base_path, "outbox file")? {
        fs::rename(claimed_path, base_path)?;
        ensure_existing_plain_file(base_path, "outbox file")?;
        return Ok(());
    }

    let tmp_path = base_path.with_file_name(format!(
        "{}.merge-{}",
        base_path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("outbox.jsonl"),
        uuid::Uuid::new_v4()
    ));
    {
        let mut merged = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&tmp_path)?;
        let mut seen = BTreeSet::new();
        for path in [base_path, claimed_path] {
            let contents = fs::read_to_string(path)?;
            for line in contents.lines().filter(|line| !line.trim().is_empty()) {
                if seen.insert(outbox_record_dedupe_key(line)) {
                    writeln!(merged, "{line}")?;
                }
            }
        }
    }
    fs::rename(&tmp_path, base_path)?;
    fs::remove_file(claimed_path)?;
    Ok(())
}

fn outbox_record_dedupe_key(line: &str) -> String {
    serde_json::from_str::<Value>(line)
        .ok()
        .and_then(|value| {
            value
                .get("outbox_id")
                .and_then(Value::as_str)
                .map(|id| format!("outbox_id:{id}"))
        })
        .unwrap_or_else(|| format!("raw:{line}"))
}

fn requeue_pending_records(path: &Path, records: &[PendingOutboxRecord]) -> anyhow::Result<()> {
    if records.is_empty() {
        return Ok(());
    }
    let Some(parent) = path.parent() else {
        anyhow::bail!("outbox file path has no parent");
    };
    fs::create_dir_all(parent)?;
    let mut file = open_plain_outbox_append(path, "outbox file")?;
    for record in records {
        writeln!(file, "{}", record.raw)?;
    }
    Ok(())
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    fn temp_root(label: &str) -> PathBuf {
        let temp_root = std::env::temp_dir().join(format!("{label}-{}", std::process::id()));
        if temp_root.exists() {
            fs::remove_dir_all(&temp_root).expect("old temp root should be removable");
        }
        fs::create_dir_all(&temp_root).expect("temp root should be creatable");
        temp_root
    }

    #[test]
    fn open_plain_outbox_append_refuses_symlink_destination() {
        let temp_root = temp_root("stateful-outbox-append-symlink");
        let victim = temp_root.join("victim.jsonl");
        let link = temp_root.join("s1.jsonl");
        fs::write(&victim, "victim\n").expect("victim should write");
        std::os::unix::fs::symlink(&victim, &link).expect("symlink should create");

        let error = open_plain_outbox_append(&link, "outbox file")
            .expect_err("symlink append destination should fail");

        assert!(error.to_string().contains("symlinked outbox file"));
        assert_eq!(
            fs::read_to_string(&victim).expect("victim should read"),
            "victim\n"
        );

        fs::remove_dir_all(&temp_root).expect("temp root should be removable");
    }

    #[test]
    fn open_plain_outbox_append_refuses_hard_link_destination() {
        let temp_root = temp_root("stateful-outbox-append-hardlink");
        let victim = temp_root.join("victim.jsonl");
        let link = temp_root.join("s1.jsonl");
        fs::write(&victim, "victim\n").expect("victim should write");
        fs::hard_link(&victim, &link).expect("hard link should create");

        let error = open_plain_outbox_append(&link, "outbox file")
            .expect_err("hard-link append destination should fail");

        assert!(error.to_string().contains("hard-linked outbox file"));
        assert_eq!(
            fs::read_to_string(&victim).expect("victim should read"),
            "victim\n"
        );

        fs::remove_dir_all(&temp_root).expect("temp root should be removable");
    }

    #[test]
    fn max_pending_sequence_refuses_symlinked_outbox_file() {
        let temp_root = temp_root("stateful-outbox-sequence-symlink");
        let victim = temp_root.join("victim.jsonl");
        let link = temp_root.join("s1.jsonl");
        fs::write(
            &victim,
            "{\"sequence\":1,\"sync_status\":\"pending\"}\n",
        )
        .expect("victim should write");
        std::os::unix::fs::symlink(&victim, &link).expect("symlink should create");

        let error = max_pending_sequence_in_file(&link)
            .expect_err("symlink sequence source should fail");

        assert!(error.to_string().contains("symlinked outbox file"));

        fs::remove_dir_all(&temp_root).expect("temp root should be removable");
    }

    #[test]
    fn read_outbox_counter_refuses_symlinked_counter() {
        let temp_root = temp_root("stateful-outbox-counter-symlink");
        let victim = temp_root.join("victim.sequence");
        let link = temp_root.join("s1.sequence");
        fs::write(&victim, "1\n").expect("victim should write");
        std::os::unix::fs::symlink(&victim, &link).expect("symlink should create");

        let error = read_outbox_counter(&link).expect_err("symlink counter should fail");

        assert!(error.to_string().contains("symlinked outbox sequence file"));

        fs::remove_dir_all(&temp_root).expect("temp root should be removable");
    }

    #[test]
    fn next_sequence_rejects_overflow() {
        let temp_root = temp_root("stateful-outbox-sequence-overflow");
        fs::write(
            temp_root.join("s1.jsonl"),
            "{\"sequence\":18446744073709551615,\"sync_status\":\"pending\"}\n",
        )
        .expect("max sequence file should write");

        let error = next_sequence(&temp_root, "s1").expect_err("sequence overflow should fail");

        assert!(error.to_string().contains("outbox sequence overflow"));

        fs::remove_dir_all(&temp_root).expect("temp root should be removable");
    }
}
