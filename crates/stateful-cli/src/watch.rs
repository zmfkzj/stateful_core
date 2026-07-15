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
    time::{Duration, Instant},
};

use notify::{RecursiveMode, Watcher};
use stateful_core::{ActorType, SourceKind};
use stateful_store::{HumanObservationConfidence, HumanObservationInput, HumanObservationKind};

use crate::{
    GlobalPaths, RepoGate, discover_runtime_with_global, effective_workspace_id_for_repo, post_v2,
    repo_gate, repo_identity_for_enabled_repo, v2_request_envelope,
};

const FLUSH_INTERVAL: Duration = Duration::from_millis(300);

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
            Err(mpsc::RecvTimeoutError::Timeout) => flush_pending(
                &repo_root,
                &runtime,
                identity.clone(),
                &agent_id,
                &workspace_id,
                &mut pending,
            )?,
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
    pending: &mut HashMap<PathBuf, Instant>,
) {
    let now = Instant::now();
    for path in paths {
        let absolute = if path.is_absolute() {
            path
        } else {
            repo_root.join(path)
        };
        pending.insert(absolute, now);
    }
}

fn flush_pending(
    repo_root: &Path,
    runtime: &crate::ServerRuntime,
    identity: Option<crate::RepoIdentity>,
    agent_id: &str,
    workspace_id: &str,
    pending: &mut HashMap<PathBuf, Instant>,
) -> anyhow::Result<()> {
    if pending.is_empty() {
        return Ok(());
    }

    let candidates = std::mem::take(pending)
        .into_keys()
        .filter_map(|path| observed_path(repo_root, &path))
        .filter(|(_, relative)| !prefix_excluded(relative))
        .filter(|(absolute, _)| !absolute.metadata().is_ok_and(|metadata| metadata.is_dir()))
        .collect::<Vec<_>>();
    let ignored = gitignored_paths(
        repo_root,
        candidates.iter().map(|(_, relative)| relative.as_path()),
    );

    for (_absolute, relative) in candidates {
        if ignored.contains(&relative) {
            continue;
        }
        let relative_string = relative_path_string(&relative);
        let kind = if _absolute.exists() {
            HumanObservationKind::Change
        } else {
            HumanObservationKind::Delete
        };
        let request = v2_request_envelope(
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
        )?;
        let response = post_v2(runtime, "/v2/human/observe", &request)?;
        if !(200..300).contains(&response.status_code) {
            anyhow::bail!(
                "human observe failed with HTTP {}: {}",
                response.status_code,
                response.body
            );
        }
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
            let (mut stream, _) = listener.accept().expect("handshake connection should arrive");
            let mut handshake = Vec::new();
            let mut byte = [0_u8; 1];
            while !handshake.ends_with(b"\r\n\r\n") {
                std::io::Read::read_exact(&mut stream, &mut byte).expect("handshake header should read");
                handshake.push(byte[0]);
            }
            assert!(String::from_utf8(handshake).expect("handshake should be utf8").contains("GET /v2/runtime/identity?"));
            let identity = r#"{"protocol_version":"stateful.v2","journal_schema_version":2,"coordination_mode":"awareness","capabilities":["presence"]}"#;
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
        pending.insert(file, Instant::now());
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
        let body = request
            .split_once("\r\n\r\n")
            .expect("body separator")
            .1;
        let body: serde_json::Value = serde_json::from_str(body).expect("body should parse");
        assert_eq!(body["protocol_version"], "stateful.v2");
        assert_eq!(body["agent"]["actor_type"], "human");
        assert_eq!(body["payload"]["relative_path"], "src/lib.rs");
    }
