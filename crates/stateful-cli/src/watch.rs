use std::{
    collections::{HashMap, HashSet},
    io::Write,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::Duration,
};

use notify::{RecursiveMode, Watcher};
use stateful_core::{ActorType, RequestEnvelope, SourceKind};
use stateful_store::{HumanObservationConfidence, HumanObservationInput, HumanObservationKind};

use crate::{
    GlobalPaths, RepoGate, discover_runtime_with_global, effective_workspace_id_for_repo, post_v2,
    repo_gate, repo_identity_for_enabled_repo, v2_request_envelope,
};

const FLUSH_INTERVAL: Duration = Duration::from_millis(300);

type PendingObservations = HashMap<PathBuf, PendingObservation>;

struct PendingObservation {
    request: Option<RequestEnvelope<HumanObservationInput>>,
}

impl PendingObservation {
    fn new() -> Self {
        Self { request: None }
    }
}

pub fn run_watch(repo: Option<PathBuf>) -> anyhow::Result<()> {
    let paths = GlobalPaths::from_env()?;
    let start = repo.unwrap_or(std::env::current_dir()?);
    let repo_root = match repo_gate(&paths, &start)? {
        RepoGate::Enabled { repo_root } => repo_root,
        RepoGate::Disabled => anyhow::bail!("stateful watch run requires an enabled repo"),
        RepoGate::OutsideGitRepo => anyhow::bail!("stateful watch run requires a Git repo"),
    };
    let runtime = discover_runtime_with_global(&repo_root, &paths)?;
    let identity = repo_identity_for_enabled_repo(&paths, &repo_root).ok();
    let workspace_id = effective_workspace_id_for_repo(&runtime.workspace_id, identity.as_ref());
    let agent_id = format!("watcher-{workspace_id}");

    let stopped = Arc::new(AtomicBool::new(false));
    signal_hook::flag::register(signal_hook::consts::SIGINT, Arc::clone(&stopped))?;

    let (sender, receiver) = mpsc::channel();
    let mut watcher = notify::recommended_watcher(move |event| {
        let _ = sender.send(event);
    })?;
    watcher.watch(&repo_root, RecursiveMode::Recursive)?;

    eprintln!("stateful watch running for {}", repo_root.display());
    let mut pending = HashMap::new();
    while !stopped.load(Ordering::Relaxed) {
        match receiver.recv_timeout(FLUSH_INTERVAL) {
            Ok(Ok(event)) => queue_event_paths(&repo_root, event.paths, &mut pending),
            Ok(Err(error)) => eprintln!("stateful watch warning: {error}"),
            Err(mpsc::RecvTimeoutError::Timeout) => {
                if let Err(error) = flush_pending(
                    &repo_root,
                    &runtime,
                    identity.clone(),
                    &agent_id,
                    &workspace_id,
                    &mut pending,
                ) {
                    eprintln!("stateful watch warning: {error}");
                }
            },
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }
    flush_pending(
        &repo_root,
        &runtime,
        identity,
        &agent_id,
        &workspace_id,
        &mut pending,
    )?;
    drop(watcher);
    Ok(())
}

fn queue_event_paths(
    repo_root: &Path,
    paths: Vec<PathBuf>,
    pending: &mut PendingObservations,
) {
    for path in paths {
        let absolute = if path.is_absolute() {
            path
        } else {
            repo_root.join(path)
        };
        pending.entry(absolute).or_insert_with(PendingObservation::new);
    }
}

fn flush_pending(
    repo_root: &Path,
    runtime: &crate::ServerRuntime,
    identity: Option<crate::RepoIdentity>,
    agent_id: &str,
    workspace_id: &str,
    pending: &mut PendingObservations,
) -> anyhow::Result<()> {
    if pending.is_empty() {
        return Ok(());
    }

    let mut candidates = Vec::new();
    let paths = pending.keys().cloned().collect::<Vec<_>>();
    for path in paths {
        let Some((absolute, relative)) = observed_path(repo_root, &path) else {
            pending.remove(&path);
            continue;
        };
        if prefix_excluded(&relative)
            || absolute.metadata().is_ok_and(|metadata| metadata.is_dir())
        {
            pending.remove(&absolute);
            continue;
        }
        candidates.push((absolute, relative));
    }
    let ignored = gitignored_paths(
        repo_root,
        candidates.iter().map(|(_, relative)| relative.as_path()),
    );

    for (absolute, relative) in candidates {
        if ignored.contains(&relative) {
            pending.remove(&absolute);
            continue;
        }
        let entry = pending
            .get_mut(&absolute)
            .expect("candidate must remain pending until acknowledged");
        if entry.request.is_none() {
            let relative_string = relative_path_string(&relative);
            let kind = if absolute.exists() {
                HumanObservationKind::Change
            } else {
                HumanObservationKind::Delete
            };
            entry.request = Some(v2_request_envelope(
                uuid::Uuid::new_v4(),
                agent_id.to_string(),
                workspace_id.to_string(),
                identity.clone(),
                ActorType::Human,
                SourceKind::Watcher,
                "human_observe",
                "stateful.watch.run",
                None,
                HumanObservationInput {
                    relative_path: relative_string,
                    kind,
                    confidence: HumanObservationConfidence::High,
                    source: "watcher".to_string(),
                    summary: "File changed by watcher.".to_string(),
                    observed_at: Some(time::OffsetDateTime::now_utc()),
                },
            )?);
        }
        let response = post_v2(
            runtime,
            "/v2/human/observe",
            entry.request.as_ref().expect("request should be initialized"),
        )?;
        if !(200..300).contains(&response.status_code) {
            anyhow::bail!(
                "human observe failed with HTTP {}: {}",
                response.status_code,
                response.body
            );
        }
        pending.remove(&absolute);
    }

    Ok(())
}

fn observed_path(repo_root: &Path, absolute: &Path) -> Option<(PathBuf, PathBuf)> {
    let relative = absolute.strip_prefix(repo_root).ok()?.to_path_buf();
    if relative.as_os_str().is_empty()
        || relative
            .components()
            .any(|component| matches!(component, std::path::Component::ParentDir))
    {
        return None;
    }
    Some((absolute.to_path_buf(), relative))
}

fn prefix_excluded(relative: &Path) -> bool {
    let mut components = relative.components();
    let Some(first) = components
        .next()
        .and_then(|component| component.as_os_str().to_str())
    else {
        return true;
    };
    matches!(first, ".git" | ".codex" | "target" | "node_modules") || first.starts_with(".stateful")
}

fn gitignored_paths<'a>(
    repo_root: &Path,
    paths: impl Iterator<Item = &'a Path>,
) -> HashSet<PathBuf> {
    let paths = paths.collect::<Vec<_>>();
    if paths.is_empty() {
        return HashSet::new();
    }

    let mut child = match Command::new("git")
        .args(["check-ignore", "--stdin"])
        .current_dir(repo_root)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
    {
        Ok(child) => child,
        Err(_) => return HashSet::new(),
    };

    if let Some(mut stdin) = child.stdin.take() {
        for path in &paths {
            if writeln!(stdin, "{}", relative_path_string(path)).is_err() {
                return HashSet::new();
            }
        }
    }

    let output = match child.wait_with_output() {
        Ok(output) => output,
        Err(_) => return HashSet::new(),
    };
    if !output.status.success() && output.status.code() != Some(1) {
        return HashSet::new();
    }

    String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(PathBuf::from)
        .collect()
}

fn relative_path_string(path: &Path) -> String {
    path.components()
        .filter_map(|component| component.as_os_str().to_str())
        .collect::<Vec<_>>()
        .join("/")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prefix_excludes_private_and_build_paths() {
        for path in [
            ".git/index",
            ".stateful/config.yml",
            ".stateful_core/runtime/server.json",
            ".codex/log.json",
            "target/debug/app",
            "node_modules/pkg/index.js",
        ] {
            assert!(
                prefix_excluded(Path::new(path)),
                "{path} should be excluded"
            );
        }
        assert!(!prefix_excluded(Path::new("src/lib.rs")));
        assert!(!prefix_excluded(Path::new(".gitignore")));
    }

    #[test]
    fn relative_path_string_uses_forward_slashes() {
        assert_eq!(relative_path_string(Path::new("src/lib.rs")), "src/lib.rs");
    }
}

#[test]
fn watcher_emits_v2_human_observation_and_reconciliation() {
    let temp = tempfile::tempdir().expect("temp dir should create");
    let repo = temp.path().join("repo");
    std::fs::create_dir_all(repo.join(".git")).expect("git marker should write");
    let file = repo.join("src").join("lib.rs");
    std::fs::create_dir_all(file.parent().expect("file parent")).expect("source dir should write");
    std::fs::write(&file, "pub fn example() {}\n").expect("source file should write");

    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("listener should bind");
    let addr = listener.local_addr().expect("listener address should load");
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let (mut stream, _) = listener
            .accept()
            .expect("handshake connection should arrive");
        let mut handshake = Vec::new();
        let mut byte = [0_u8; 1];
        while !handshake.ends_with(b"\r\n\r\n") {
            std::io::Read::read_exact(&mut stream, &mut byte)
                .expect("handshake header should read");
            handshake.push(byte[0]);
        }
        assert!(
            String::from_utf8(handshake)
                .expect("handshake should be utf8")
                .contains("GET /v2/runtime/identity?")
        );
        let identity = r#"{"protocol_version":"stateful.v2","journal_schema_version":2,"coordination_mode":"awareness","pid":42,"workspace_id":"w1","workspace_version":1,"capabilities":["presence"]}"#;
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n{identity}",
            identity.len()
        );
        std::io::Write::write_all(&mut stream, response.as_bytes())
            .expect("identity response should write");

        let (mut stream, _) = listener.accept().expect("connection should arrive");
        let mut request = Vec::new();
        let mut byte = [0_u8; 1];
        while !request.ends_with(b"\r\n\r\n") {
            std::io::Read::read_exact(&mut stream, &mut byte).expect("header byte should read");
            request.push(byte[0]);
        }
        let headers = String::from_utf8(request.clone()).expect("headers should be utf8");
        let content_length = headers
            .lines()
            .find_map(|line| line.strip_prefix("Content-Length: "))
            .expect("content length should exist")
            .parse::<usize>()
            .expect("content length should parse");
        let mut body = vec![0_u8; content_length];
        std::io::Read::read_exact(&mut stream, &mut body).expect("body should read");
        request.extend_from_slice(&body);
        tx.send(String::from_utf8(request).expect("request should be utf8"))
            .expect("request should send to test");
        std::io::Write::write_all(
            &mut stream,
            b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\n{}",
        )
        .expect("response should write");
    });

    let mut pending = HashMap::new();
    pending.insert(file, PendingObservation::new());
    flush_pending(
        &repo,
        &crate::ServerRuntime::new(format!("http://{addr}"), "token", "w1", 0),
        None,
        "human-1",
        "w1",
        &mut pending,
    )
    .expect("watcher should submit observation");

    let request = rx.recv().expect("watcher request should arrive");
    assert!(request.contains("POST /v2/human/observe HTTP/1.1"));
    let body = request.split_once("\r\n\r\n").expect("body separator").1;
    let body: serde_json::Value = serde_json::from_str(body).expect("body should parse");
    assert_eq!(body["protocol_version"], "stateful.v2");
    assert_eq!(body["agent"]["actor_type"], "human");
    assert_eq!(body["payload"]["relative_path"], "src/lib.rs");
}

#[test]
fn watcher_retries_failed_and_unsent_paths_with_exact_request_envelopes() {
    let temp = tempfile::tempdir().expect("temp dir should create");
    let repo = temp.path().join("repo");
    std::fs::create_dir_all(repo.join(".git")).expect("git marker should write");
    let first = repo.join("src").join("a.rs");
    let second = repo.join("src").join("b.rs");
    std::fs::create_dir_all(first.parent().expect("source directory")).expect("source directory");
    std::fs::write(&first, "a").expect("first file should write");
    std::fs::write(&second, "b").expect("second file should write");

    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("listener should bind");
    let addr = listener.local_addr().expect("listener address should load");
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let identity = r#"{"protocol_version":"stateful.v2","journal_schema_version":2,"coordination_mode":"awareness","pid":42,"workspace_id":"w1","workspace_version":1,"capabilities":["presence"]}"#;
        let mut observation_count = 0;
        for _ in 0..6 {
            let (mut stream, _) = listener.accept().expect("request should arrive");
            let request = read_watcher_http_request(&mut stream);
            if request.starts_with("GET /v2/runtime/identity?") {
                write_watcher_http_response(&mut stream, 200, identity);
                continue;
            }
            tx.send(request).expect("observation should send");
            let status = if observation_count == 0 { 503 } else { 200 };
            observation_count += 1;
            write_watcher_http_response(&mut stream, status, "{}");
        }
    });

    let runtime = crate::ServerRuntime::new(format!("http://{addr}"), "token", "w1", 0);
    let mut pending = HashMap::new();
    pending.insert(first, PendingObservation::new());
    pending.insert(second, PendingObservation::new());

    assert!(
        flush_pending(&repo, &runtime, None, "human-1", "w1", &mut pending).is_err(),
        "the first rejected observation should fail the flush"
    );
    assert_eq!(
        pending.len(),
        2,
        "the failed and unsent observations must remain queued"
    );
    flush_pending(&repo, &runtime, None, "human-1", "w1", &mut pending)
        .expect("retry should deliver the retained observations");
    assert!(pending.is_empty(), "only acknowledged observations may be removed");

    let requests = (0..3)
        .map(|_| rx.recv().expect("all observation requests should arrive"))
        .map(|request| {
            let body = request
                .split_once("\r\n\r\n")
                .expect("request should include a body")
                .1;
            serde_json::from_str::<serde_json::Value>(body).expect("body should parse")
        })
        .collect::<Vec<_>>();
    let failed_path = requests[0]["payload"]["relative_path"].clone();
    assert_eq!(
        requests[0]["request_id"], requests[1]["request_id"],
        "the retry must preserve the failed observation request id"
    );
    assert_eq!(
        requests[1]["payload"]["relative_path"], failed_path,
        "the retry must preserve the failed observation path"
    );
    let delivered = requests[1..]
        .iter()
        .map(|request| request["payload"]["relative_path"].as_str().expect("path").to_string())
        .collect::<HashSet<_>>();
    assert_eq!(
        delivered,
        HashSet::from(["src/a.rs".to_string(), "src/b.rs".to_string()]),
        "retry should acknowledge every path exactly once"
    );
}

#[cfg(test)]

#[test]
fn watcher_discards_permanently_undeliverable_paths() {
    let temp = tempfile::tempdir().expect("temp dir should create");
    let repo = temp.path().join("repo");
    std::fs::create_dir_all(repo.join(".git")).expect("git marker should write");
    let directory = repo.join("src");
    std::fs::create_dir_all(&directory).expect("directory should write");
    let mut pending = HashMap::new();
    pending.insert(repo.join(".git").join("index"), PendingObservation::new());
    pending.insert(directory, PendingObservation::new());
    pending.insert(temp.path().join("outside.rs"), PendingObservation::new());

    flush_pending(
        &repo,
        &crate::ServerRuntime::new("http://127.0.0.1:1", "token", "w1", 0),
        None,
        "human-1",
        "w1",
        &mut pending,
    )
    .expect("permanent filters should not need delivery");

    assert!(
        pending.is_empty(),
        "outside-root, excluded, and directory paths must not remain queued"
    );
}

#[cfg(test)]

fn read_watcher_http_request(stream: &mut std::net::TcpStream) -> String {
    let mut request = Vec::new();
    let mut byte = [0_u8; 1];
    while !request.ends_with(b"\r\n\r\n") {
        std::io::Read::read_exact(stream, &mut byte).expect("header byte should read");
        request.push(byte[0]);
    }
    let headers = String::from_utf8(request.clone()).expect("headers should be utf8");
    let content_length = headers
        .lines()
        .find_map(|line| line.strip_prefix("Content-Length: "))
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or_default();
    let mut body = vec![0_u8; content_length];
    std::io::Read::read_exact(stream, &mut body).expect("body should read");
    request.extend_from_slice(&body);
    String::from_utf8(request).expect("request should be utf8")
}

#[cfg(test)]

fn write_watcher_http_response(stream: &mut std::net::TcpStream, status: u16, body: &str) {
    let reason = if status == 200 { "OK" } else { "Service Unavailable" };
    std::io::Write::write_all(
        stream,
        format!(
            "HTTP/1.1 {status} {reason}\r\nContent-Length: {}\r\n\r\n{body}",
            body.len()
        )
        .as_bytes(),
    )
    .expect("response should write");
}

#[test]
fn watcher_observation_blocks_writes_until_reread_and_reconciliation() {
    let temp = tempfile::tempdir().expect("temp dir should create");
    let repo = temp.path().join("repo");
    std::fs::create_dir_all(repo.join(".git")).expect("git marker should write");
    let file = repo.join("src").join("lib.rs");
    std::fs::create_dir_all(file.parent().expect("file parent")).expect("source dir should write");
    std::fs::write(&file, "pub fn example() {}\n").expect("source file should write");

    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("listener should bind");
    listener
        .set_nonblocking(true)
        .expect("listener should become nonblocking");
    let addr = listener.local_addr().expect("listener address should load");
    let server = stateful_server::ServerConfig::with_store(
        "token",
        stateful_store::Store::open_in_memory().expect("server store should open"),
    )
    .with_coordination_mode(stateful_server::CoordinationMode::Enforcement);
    std::thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("server runtime should build");
        runtime.block_on(async move {
            let listener =
                tokio::net::TcpListener::from_std(listener).expect("tokio listener should convert");
            stateful_server::serve_listener(listener, server)
                .await
                .expect("server should run");
        });
    });

    let runtime = crate::ServerRuntime::new(format!("http://{addr}"), "token", "w1", 0);
    let mut pending = HashMap::new();
    pending.insert(file.clone(), PendingObservation::new());
    flush_pending(&repo, &runtime, None, "human-1", "w1", &mut pending)
        .expect("watcher should submit the human observation");
    assert!(pending.is_empty(), "watcher should flush the observed path");

    let fingerprint = stateful_core::fingerprint_path(&file).expect("file should fingerprint");
    let declare = crate::v2_request_envelope(
        uuid::Uuid::new_v4(),
        "agent-1".to_string(),
        "w1".to_string(),
        None,
        ActorType::Agent,
        SourceKind::Cli,
        "reservation_declare",
        "watcher-reconciliation-test",
        None,
        serde_json::json!({
            "scopes": [{"kind": "file", "path": "src/lib.rs"}],
            "action": "write_file",
            "purpose": "Reconcile the watcher-observed file."
        }),
    )
    .expect("reservation envelope should build");
    let declared = crate::post_v2(&runtime, "/v2/reservation/declare", &declare)
        .expect("reservation should declare");
    let reservation_id = serde_json::from_str::<serde_json::Value>(&declared.body)
        .expect("reservation response should parse")["reservation_id"]
        .as_str()
        .expect("reservation response should include id")
        .to_string();

    let claim = crate::v2_request_envelope(
        uuid::Uuid::new_v4(),
        "agent-1".to_string(),
        "w1".to_string(),
        None,
        ActorType::Agent,
        SourceKind::Cli,
        "claim_acquire",
        "watcher-reconciliation-test",
        None,
        serde_json::json!({
            "reservation_id": reservation_id,
            "paths": [{
                "relative_path": "src/lib.rs",
                "observation": fingerprint
            }]
        }),
    )
    .expect("claim envelope should build");
    crate::post_v2(&runtime, "/v2/claim/acquire", &claim).expect("reservation should claim");

    let denied = crate::v2_request_envelope(
        uuid::Uuid::new_v4(),
        "agent-1".to_string(),
        "w1".to_string(),
        None,
        ActorType::Agent,
        SourceKind::Cli,
        "authorize",
        "watcher-reconciliation-test",
        None,
        serde_json::json!({
            "reservation_id": reservation_id,
            "operation_id": "write-before-reconciliation",
            "action": "write_file",
            "targets": [{"path": "src/lib.rs", "before": fingerprint}]
        }),
    )
    .expect("authorization envelope should build");
    let denied = crate::post_json(
        &runtime,
        "/v2/authorize",
        &serde_json::to_value(denied).expect("authorization should serialize"),
    )
    .expect("authorization response should arrive");
    assert_eq!(denied.status_code, 403);
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&denied.body).expect("denial should parse")["reason_code"],
        "unreconciled_human_write",
        "watcher observations must not auto-acknowledge"
    );

    let read_start = crate::v2_request_envelope(
        uuid::Uuid::new_v4(),
        "agent-1".to_string(),
        "w1".to_string(),
        None,
        ActorType::Agent,
        SourceKind::Cli,
        "read_start",
        "watcher-reconciliation-test",
        None,
        serde_json::json!({
            "operation_id": "watcher-reconciliation-read",
            "path": "src/lib.rs",
            "before": fingerprint
        }),
    )
    .expect("read start envelope should build");
    crate::post_v2(&runtime, "/v2/read/start", &read_start).expect("exact reread should start");
    let read_complete = crate::v2_request_envelope(
        uuid::Uuid::new_v4(),
        "agent-1".to_string(),
        "w1".to_string(),
        None,
        ActorType::Agent,
        SourceKind::Cli,
        "read_complete",
        "watcher-reconciliation-test",
        None,
        serde_json::json!({
            "operation_id": "watcher-reconciliation-read",
            "path": "src/lib.rs",
            "classification": "exact",
            "after": fingerprint
        }),
    )
    .expect("read completion envelope should build");
    crate::post_v2(&runtime, "/v2/read/complete", &read_complete)
        .expect("exact reread should complete");

    let acknowledgement = crate::v2_request_envelope(
        uuid::Uuid::new_v4(),
        "agent-1".to_string(),
        "w1".to_string(),
        None,
        ActorType::Agent,
        SourceKind::Cli,
        "reconcile_ack",
        "watcher-reconciliation-test",
        None,
        serde_json::json!({
            "decision": "adopt",
            "files_reread": ["src/lib.rs"],
            "human_change_summary": "Adopted the watcher-observed change.",
            "resources": ["src/lib.rs"],
            "reservation_id": reservation_id,
            "conflict_with_plan": false
        }),
    )
    .expect("reconciliation envelope should build");
    crate::post_v2(&runtime, "/v2/reconcile/ack", &acknowledgement)
        .expect("reservation-covered reconciliation should acknowledge");

    let authorized = crate::v2_request_envelope(
        uuid::Uuid::new_v4(),
        "agent-1".to_string(),
        "w1".to_string(),
        None,
        ActorType::Agent,
        SourceKind::Cli,
        "authorize",
        "watcher-reconciliation-test",
        None,
        serde_json::json!({
            "reservation_id": reservation_id,
            "operation_id": "write-after-reconciliation",
            "action": "write_file",
            "targets": [{"path": "src/lib.rs", "before": fingerprint}]
        }),
    )
    .expect("authorization envelope should build");
    let authorized = crate::post_json(
        &runtime,
        "/v2/authorize",
        &serde_json::to_value(authorized).expect("authorization should serialize"),
    )
    .expect("authorization response should arrive");
    assert_eq!(authorized.status_code, 200);
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&authorized.body)
            .expect("authorization should parse")["decision"]["decision"],
        "allow",
        "the reconciliation should clear only the watcher safety block"
    );
}
